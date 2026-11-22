use super::*;
use std::fmt::Write as _;

#[test]
fn a_subcommand_name_is_not_a_global_option() {
    let args = ["--isolated".to_string(), "format".to_string()];
    let tokens = tokenize_grammar(&args, &ruff_takes_value, Dialect::Posix);

    assert!(ruff_is_global_token(tokens[0]));
    assert!(!ruff_is_global_token(tokens[1]));
}

#[test]
fn check_summary_caps_violations_and_appends_the_tail_hint_when_available() {
    use crate::core::{tee, test_isolation, user_env};

    let mut full = String::new();
    let diagnostics: Vec<_> = (1..=52)
        .map(|row| {
            writeln!(full, "  sample.py:{row}:8 F401 Unused import").unwrap();
            serde_json::json!({
                "code": "F401",
                "message": "Unused import",
                "location": {"row": row, "column": 8},
                "filename": "sample.py",
                "fix": null,
            })
        })
        .collect();
    let input = serde_json::to_string(&diagnostics).unwrap();

    for tee_setting in ["0", "1"] {
        let root = test_isolation::tempdir();
        test_isolation::with_root(root.path(), || {
            user_env::with_path("RTK_TEE_DIR", Some(root.path()), || {
                user_env::with_vars(&[("RTK_TEE", Some(tee_setting))], || {
                    // Use a distinct label so the probe cannot collide with Ruff's file.
                    let hint_available =
                        tee::force_tee_tail_hint(&full, "ruff-coverage-probe", 51).is_some();

                    let output = filter_ruff_check_json(&input);
                    let (summary, details) = output.split_once("Violations:\n").unwrap();
                    assert_eq!(
                        summary,
                        "Ruff: 52 issues in 1 files\n\
                         Top rules:\n  F401 (52x): Unused import\n\n\
                         Top files:\n  sample.py (52 issues)\n    F401 (52)\n\n"
                    );

                    let mut lines = details.lines();
                    for row in 1..=50 {
                        let expected = format!("  sample.py:{row}:8 F401");
                        assert_eq!(lines.next(), Some(expected.as_str()));
                    }
                    assert_eq!(lines.next(), Some("  … +2 more"));
                    assert!(!details.contains("sample.py:51:8"));
                    assert!(!details.contains("sample.py:52:8"));
                    assert_eq!(lines.next().is_some(), hint_available, "{output}");
                });
            });
        });
    }
}

#[test]
fn format_check_limits_listed_files_and_reports_the_unchanged_count() {
    let total = CAP_WARNINGS + 2;
    let mut input = String::new();
    for index in 1..=total {
        writeln!(input, "Would reformat: file_{index}.py").unwrap();
    }

    let mut expected = format!("Ruff format: {total} files need formatting\n");
    for index in 1..=CAP_WARNINGS {
        writeln!(expected, "{index}. file_{index}.py").unwrap();
    }
    expected.push_str("\n... +2 more files\n");

    let hint = "\n[hint] Run `ruff format` to format these files";
    assert_eq!(filter_ruff_format(&input), format!("{expected}{hint}"));

    writeln!(
        input,
        "{total} files would be reformatted, 3 files left unchanged"
    )
    .unwrap();
    assert_eq!(
        filter_ruff_format(&input),
        format!("{expected}\n3 files already formatted\n{hint}")
    );
}

#[test]
fn format_filter_distinguishes_a_check_without_paths_from_a_write_summary() {
    assert_eq!(
        filter_ruff_format("  Would reformat 0 files\n"),
        "Ruff format: All files formatted correctly"
    );
    assert_eq!(
        filter_ruff_format("\n  2 files reformatted  \n"),
        "2 files reformatted"
    );
}
