use super::*;
use crate::core::{test_isolation, user_env};

fn with_isolated_tee(check: impl FnOnce()) {
    let root = test_isolation::tempdir();
    test_isolation::with_root(root.path(), || {
        user_env::with_path("RTK_TEE_DIR", Some(root.path()), || {
            user_env::with_vars(&[("RTK_TEE", Some("1"))], check);
        });
    });
}

fn numbered_diagnostics(count: usize) -> Vec<binlog::BinlogIssue> {
    (1..=count)
        .map(|index| binlog::BinlogIssue {
            code: String::new(),
            file: String::new(),
            line: 0,
            column: 0,
            message: format!("diagnostic #{index}"),
        })
        .collect()
}

fn split_diagnostic_output(output: &str) -> (&str, &str, &str) {
    let (sections, verdict) = output.rsplit_once("\n\n").expect("final verdict");
    let (warnings, errors) = sections
        .strip_prefix("Warnings:\n")
        .and_then(|sections| sections.split_once("\n\nErrors:\n"))
        .expect("warnings followed by errors");
    (warnings, errors, verdict)
}

fn assert_diagnostic_overflow<'a>(
    section: &'a str,
    kind: &str,
    cap: usize,
    count: usize,
) -> &'a str {
    let mut lines = section.lines();
    for index in 1..=cap {
        let expected = format!("  {kind} diagnostic #{index}");
        assert_eq!(lines.next(), Some(expected.as_str()));
    }

    let hidden = count - cap;
    let overflow = format!("  … +{hidden} more {kind}s");
    assert_eq!(lines.next(), Some(overflow.as_str()));

    let hint = lines.next().expect("overflow should include a recall hint");
    let prefix = format!("  [+{hidden} hidden: rtk recall ");
    let recall_id = hint
        .strip_prefix(prefix.as_str())
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or_else(|| panic!("unexpected {kind} recall hint: {hint}"));
    assert!(
        !recall_id.is_empty() && recall_id.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "invalid recall ID in {hint}"
    );
    assert_eq!(
        lines.next(),
        None,
        "overflow must stay out of inline output"
    );
    recall_id
}

#[test]
fn dotnet_test_caps_both_diagnostic_sections_and_emits_recall_hints() {
    with_isolated_tee(|| {
        let errors = numbered_diagnostics(CAP_WARNINGS + 2);
        let warnings = numbered_diagnostics(CAP_WARNINGS + 3);
        let summary = binlog::TestSummary {
            passed: 4,
            failed: 1,
            total: 5,
            project_count: 1,
            duration_text: Some("2 s".to_string()),
            ..Default::default()
        };
        let output = format_test_output(&summary, &errors, &warnings, Path::new("test.binlog"));
        let (warning_section, error_section, verdict) = split_diagnostic_output(&output);

        let warning_id =
            assert_diagnostic_overflow(warning_section, "warning", CAP_WARNINGS, warnings.len());
        let error_id =
            assert_diagnostic_overflow(error_section, "error", CAP_WARNINGS, errors.len());
        assert_ne!(warning_id, error_id, "each section needs its own recall ID");
        assert_eq!(
            verdict,
            format!(
                "fail dotnet test: 4 passed, 1 failed, 0 skipped, {} warnings in 1 projects (2 s)",
                warnings.len()
            )
        );
    });
}

#[test]
fn dotnet_restore_caps_diagnostics_without_changing_verdict_totals() {
    with_isolated_tee(|| {
        let errors = numbered_diagnostics(CAP_ERRORS + 2);
        let warnings = numbered_diagnostics(CAP_WARNINGS + 3);
        let summary = binlog::RestoreSummary {
            restored_projects: 2,
            errors: errors.len(),
            warnings: warnings.len(),
            duration_text: Some("00:00:01.25".to_string()),
        };
        let output =
            format_restore_output(&summary, &errors, &warnings, Path::new("restore.binlog"));
        let (warning_section, error_section, verdict) = split_diagnostic_output(&output);

        let warning_id =
            assert_diagnostic_overflow(warning_section, "warning", CAP_WARNINGS, warnings.len());
        let error_id = assert_diagnostic_overflow(error_section, "error", CAP_ERRORS, errors.len());
        assert_ne!(warning_id, error_id, "each section needs its own recall ID");
        assert_eq!(
            verdict,
            format!(
                "fail dotnet restore: 2 projects, {} errors, {} warnings (00:00:01.25)",
                errors.len(),
                warnings.len()
            )
        );
    });
}
