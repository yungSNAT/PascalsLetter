use super::*;
use crate::core::test_isolation;
use std::{fs, path::Path};

fn with_project_filters(content: &str, test: impl FnOnce(&Path)) {
    let root = test_isolation::tempdir();
    test_isolation::with_root(root.path(), || {
        let _working_dir = test_isolation::enter(root.path());
        let directory = root.path().join(".rtk");
        fs::create_dir(&directory).expect("create project filter directory");
        let path = directory.join("filters.toml");
        fs::write(&path, content).expect("write project filters");
        test(&path);
    });
}

fn trust_project_filters(path: &Path, content: &str) {
    let hash = crate::hooks::integrity::compute_hash_bytes(content.as_bytes());
    crate::hooks::trust::trust_filter_with_hash(path, &hash).expect("trust project filters");
}

#[test]
fn malformed_regexes_report_the_offending_field() {
    for (definition, field) in [
        ("match_command = '^['", "match_command"),
        (
            "match_command = '^coverage-tool$'\nstrip_lines_matching = ['[']",
            "strip_lines_matching",
        ),
        (
            "match_command = '^coverage-tool$'\nkeep_lines_matching = ['[']",
            "keep_lines_matching",
        ),
    ] {
        let def = toml::from_str(definition).expect("valid TOML filter definition");
        let error = compile_filter("coverage-tool".to_owned(), def)
            .expect_err("invalid regex must prevent compilation");
        assert!(
            error.starts_with(&format!("invalid {field} regex:")),
            "{error}"
        );
    }

    let unfinished_flags = "(?i^coverage-tool";
    assert_eq!(strip_inline_flags(unfinished_flags), unfinished_flags);
    assert!(!is_fully_anchored(unfinished_flags));
}

#[test]
fn reserved_command_warning_does_not_prevent_compiling_a_filter() {
    let filters = TomlFilterRegistry::parse_and_compile(
        r#"
schema_version = 1
[filters.coverage-git]
match_command = '^git$'
head_lines = 2
tail_lines = 2
"#,
        "test",
    )
    .expect("valid filter file");
    let filter = find_filter_in("git", &filters).expect("shadowed filter still compiles");
    assert_eq!(filter.name, "coverage-git");

    let exact_fit = "first\nsecond\nthird\nlast";
    assert_eq!(
        apply_filter_with_info(filter, exact_fit),
        (exact_fit.to_owned(), Lossiness::None)
    );
    assert_eq!(
        apply_filter_with_info(filter, "first\nsecond\nmiddle\nthird\nlast"),
        (
            "first\nsecond\n... (1 lines omitted)\nthird\nlast".to_owned(),
            Lossiness::Whole,
        )
    );
}

#[test]
fn match_set_preserves_valid_patterns_when_a_trusted_pattern_is_invalid() {
    let content = r#"
schema_version = 1
[filters.coverage-match-valid]
match_command = '^coverage-match-valid$'
[filters.coverage-match-invalid]
match_command = '^coverage-match-invalid['
"#;
    with_project_filters(content, |path| {
        trust_project_filters(path, content);
        assert!(
            collect_match_patterns()
                .iter()
                .any(|pattern| pattern == "^coverage-match-invalid["),
            "the malformed but anchored regex must reach the match-set builder"
        );

        let set = build_match_set();
        assert!(set.is_match("coverage-match-valid"));
        assert!(set.is_match("make all"));
        assert!(!set.is_match("coverage-match-invalid"));
        assert!(!set.is_match("wrapper coverage-match-valid"));
    });
}

#[test]
fn verify_checks_trusted_project_filters_and_skips_invalid_entries() {
    let content = r#"
schema_version = 1

[filters.coverage-verify-good]
match_command = '^coverage-verify-good$'
strip_lines_matching = ['^noise$']

[filters.coverage-verify-broken]
match_command = '^['

[filters.coverage-verify-untested]
match_command = '^coverage-verify-untested$'

[[tests.coverage-verify-good]]
name = "strips noise"
input = "noise\nkeep\n"
expected = "keep\n"

[[tests.coverage-verify-broken]]
name = "cannot compile"
input = "input"
expected = "input"
"#;
    let selected = Some("coverage-verify-good");
    with_project_filters(content, |path| {
        let untrusted = run_filter_tests(selected);
        assert!(untrusted.outcomes.is_empty());
        assert!(untrusted.filters_without_tests.is_empty());

        trust_project_filters(path, content);
        let verified = run_filter_tests(None);
        let project_outcomes: Vec<_> = verified
            .outcomes
            .iter()
            .filter(|outcome| outcome.filter_name.starts_with("coverage-verify-"))
            .collect();
        assert_eq!(project_outcomes.len(), 1);
        let outcome = project_outcomes[0];
        assert_eq!(outcome.filter_name, "coverage-verify-good");
        assert_eq!(outcome.test_name, "strips noise");
        assert!(outcome.passed);
        assert_eq!(outcome.actual, "keep");
        assert_eq!(outcome.expected, "keep");

        let project_without_tests: Vec<_> = verified
            .filters_without_tests
            .iter()
            .filter(|name| name.starts_with("coverage-verify-"))
            .map(String::as_str)
            .collect();
        assert_eq!(project_without_tests, ["coverage-verify-untested"]);

        let mut filters = Vec::new();
        TomlFilterRegistry::extend_with_trusted(&mut filters, path);
        assert_eq!(filters.len(), 2);

        let malformed = "schema_version = [";
        fs::write(path, malformed).expect("replace project filters with malformed TOML");
        trust_project_filters(path, malformed);
        let (status, verified_content) =
            crate::hooks::trust::check_trust_with_content(path).expect("check malformed fixture");
        assert!(matches!(status, crate::hooks::trust::TrustStatus::Trusted));
        assert_eq!(verified_content.as_deref(), Some(malformed));

        TomlFilterRegistry::extend_with_trusted(&mut filters, path);
        assert_eq!(filters.len(), 2);
        let existing = find_filter_in("coverage-verify-good", &filters)
            .expect("malformed file must not discard previously compiled filters");
        assert_eq!(apply_filter(existing, "noise\nkeep"), "keep");

        let rejected = run_filter_tests(selected);
        assert!(rejected.outcomes.is_empty());
        assert!(rejected.filters_without_tests.is_empty());
    });
}

#[test]
fn debug_lookup_returns_a_builtin_match_or_passthrough() {
    user_env::with_vars(&[("RTK_TOML_DEBUG", Some("1"))], || {
        assert_eq!(
            find_matching_filter("make all").map(|filter| filter.name.as_str()),
            Some("make")
        );
        assert!(find_matching_filter("rtk-coverage-no-builtin-match").is_none());
    });
}
