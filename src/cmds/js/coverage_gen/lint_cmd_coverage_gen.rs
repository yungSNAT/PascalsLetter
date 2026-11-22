use super::*;

#[test]
fn structured_filters_handle_clean_and_malformed_output() {
    let scratch = crate::core::test_isolation::tempdir();
    crate::core::test_isolation::with_root(scratch.path(), || {
        assert_eq!(filter_eslint_json("[]"), "ESLint: No issues found");

        let malformed = "not JSON: fixture diagnostic";
        let eslint = filter_eslint_json(malformed);
        assert!(
            eslint.starts_with("ESLint output (JSON parse failed:"),
            "{eslint}"
        );
        assert_eq!(eslint.lines().last(), Some(malformed));

        let pylint = filter_pylint_json(malformed);
        assert!(
            pylint.starts_with("Pylint output (JSON parse failed:"),
            "{pylint}"
        );
        assert_eq!(pylint.lines().last(), Some(malformed));
    });

    let compact = compact_path(r"C:\workspace\project\lib\widget.ts");
    assert_eq!(
        std::path::Path::new(&compact),
        std::path::Path::new("lib").join("widget.ts")
    );
}

fn with_lint_tee(test: impl FnOnce()) {
    use crate::core::{test_isolation, user_env};

    let scratch = test_isolation::tempdir();
    let _working_dir = test_isolation::enter(scratch.path());
    test_isolation::with_root(scratch.path(), || {
        user_env::with_path("RTK_TEE_DIR", Some(scratch.path()), || {
            user_env::with_vars(
                &[("RTK_TEE", Some("1")), ("RTK_TEE_MODE", Some("always"))],
                test,
            );
        });
    });
}

fn preview_before_hint<'a>(report: &'a str, overflow: &str) -> &'a str {
    let (preview, hint) = report
        .split_once(overflow)
        .unwrap_or_else(|| panic!("missing overflow marker in {report:?}"));
    let hint = hint
        .strip_prefix("\n  ")
        .unwrap_or_else(|| panic!("missing indented overflow hint in {report:?}"));
    assert!(!hint.trim().is_empty(), "{report}");
    preview
}

#[test]
fn eslint_caps_files_without_counting_ruleless_errors_as_rules() {
    with_lint_tee(|| {
        let fatal = serde_json::json!({
            "ruleId": null,
            "severity": 2,
            "message": "Unterminated string",
            "line": 1,
            "column": 1
        });
        let warning = serde_json::json!({
            "ruleId": "prefer-const",
            "severity": 1,
            "message": "Use const",
            "line": 2,
            "column": 1
        });
        let mut files: Vec<_> = (0..CAP_WARNINGS)
            .map(|index| {
                serde_json::json!({
                    "filePath": format!("visible-{index}.js"),
                    "messages": [&fatal, &warning],
                    "errorCount": 1,
                    "warningCount": 1
                })
            })
            .collect();
        for name in ["omitted-a.js", "omitted-b.js"] {
            files.push(serde_json::json!({
                "filePath": name,
                "messages": [&fatal],
                "errorCount": 1,
                "warningCount": 0
            }));
        }

        let report = filter_eslint_json(&serde_json::to_string(&files).unwrap());
        let preview = preview_before_hint(&report, "\n… +2 more files");
        assert!(
            preview.starts_with(&format!(
                "ESLint: {} errors, {CAP_WARNINGS} warnings in {} files\n\
             Top rules:\n  prefer-const ({CAP_WARNINGS}x)\n\nTop files:\n",
                CAP_WARNINGS + 2,
                CAP_WARNINGS + 2,
            )),
            "{report}"
        );
        assert_eq!(preview.matches(" (2 issues)").count(), CAP_WARNINGS);
        assert_eq!(
            preview.matches("\n    prefer-const (1)").count(),
            CAP_WARNINGS
        );
        assert!(!preview.contains("omitted-"), "{report}");
    });
}

#[test]
fn pylint_breaks_down_severity_and_caps_the_file_list() {
    with_lint_tee(|| {
        let diagnostic = |path: &str, kind: &str, symbol: &str, id: &str| {
            serde_json::json!({
                "type": kind,
                "module": "fixture",
                "obj": "",
                "line": 1,
                "column": 0,
                "path": path,
                "symbol": symbol,
                "message": "Fixture diagnostic",
                "message-id": id
            })
        };
        let mut diagnostics = Vec::new();
        for index in 0..CAP_WARNINGS {
            let path = format!("visible-{index}.py");
            for (kind, symbol, id) in [
                ("error", "undefined-variable", "E0602"),
                ("warning", "unused-variable", "W0612"),
                ("convention", "invalid-name", "C0103"),
                ("refactor", "too-many-branches", "R0912"),
                ("info", "suppressed-message", "I0020"),
            ] {
                diagnostics.push(diagnostic(&path, kind, symbol, id));
            }
        }
        for path in ["omitted-a.py", "omitted-b.py"] {
            diagnostics.push(diagnostic(path, "info", "suppressed-message", "I0020"));
        }

        let report = filter_pylint_json(&serde_json::to_string(&diagnostics).unwrap());
        let preview = preview_before_hint(&report, "\n… +2 more files");
        assert!(
            preview.starts_with(&format!(
                "Pylint: {} issues in {} files\n  {CAP_WARNINGS} errors, \
             {CAP_WARNINGS} warnings, {CAP_WARNINGS} conventions, {CAP_WARNINGS} refactors\n\
             Top rules:\n  suppressed-message (I0020) ({}x)\n",
                diagnostics.len(),
                CAP_WARNINGS + 2,
                CAP_WARNINGS + 2,
            )),
            "{report}"
        );
        assert!(preview.contains("\n\nTop files:\n"), "{report}");
        assert_eq!(preview.matches(" (5 issues)").count(), CAP_WARNINGS);
        assert!(!preview.contains("omitted-"), "{report}");
    });
}

#[test]
fn generic_filter_classifies_and_shortens_diagnostics_before_capping() {
    let clean = "Checked all files\nSummary: 0 ERRORS";
    assert_eq!(filter_generic_lint(clean), "Lint: No issues found");

    with_lint_tee(|| {
        let warning = format!("WaRnInG: {}", "long diagnostic detail ".repeat(12));
        let mut lines = vec![clean.to_owned(), warning.clone()];
        lines.extend((1..=CAP_ERRORS + 1).map(|index| format!("ERROR: diagnostic {index:03}")));

        let report = filter_generic_lint(&lines.join("\n"));
        let preview = preview_before_hint(&report, "\n… +2 more issues");
        let mut shown = preview.lines();
        let summary = format!("Lint: {} errors, 1 warnings", CAP_ERRORS + 1);
        assert_eq!(shown.next(), Some(summary.as_str()));
        let shortened = shown.next().unwrap();
        assert!(shortened.starts_with("WaRnInG: long diagnostic detail"));
        assert!(shortened.len() < warning.len(), "{shortened}");
        let expected: Vec<_> = (1..CAP_ERRORS)
            .map(|index| format!("ERROR: diagnostic {index:03}"))
            .collect();
        assert_eq!(
            shown.filter(|line| !line.is_empty()).collect::<Vec<_>>(),
            expected
        );
    });
}

#[cfg(all(unix, feature = "engine"))]
fn lint_with_fake_tool(
    tool: &str,
    args: &[&str],
    expected_args: &[&str],
    script: &str,
) -> std::process::Output {
    use crate::core::test_isolation;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let scratch = test_isolation::tempdir();
    let bin = scratch.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let executable = bin.join(tool);
    let argv_file = scratch.path().join("argv");
    fs::write(
        &executable,
        format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$LINT_FIXTURE_ARGV\"\n{script}\n"),
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();

    let mut command = test_isolation::rtk_command();
    let inherited_path = command
        .get_envs()
        .find(|(key, _)| *key == "PATH")
        .and_then(|(_, value)| value)
        .expect("isolated rtk command should provide PATH");
    let path =
        std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(inherited_path)))
            .unwrap();
    let output = command
        .env("PATH", path)
        .env("LINT_FIXTURE_ARGV", &argv_file)
        .env("RTK_TEE", "0")
        .current_dir(scratch.path())
        .args(["-v", "lint"])
        .args(args)
        .output()
        .unwrap();

    let argv = fs::read_to_string(argv_file)
        .unwrap_or_else(|error| panic!("{tool} did not record its arguments: {error}; {output:?}"));
    let actual: Vec<_> = argv.lines().collect();
    if tool == "npx" {
        assert!(
            actual.ends_with(expected_args),
            "unexpected npx argv: {actual:?}"
        );
    } else {
        assert_eq!(actual, expected_args, "unexpected {tool} argv");
    }
    output
}

#[cfg(all(unix, feature = "engine"))]
#[test]
fn lint_dispatches_to_the_right_filter_and_preserves_tool_status() {
    if !crate::core::test_isolation::rtk_binary_is_built() {
        return;
    }

    // Make the raw capture longer than the summary, even for empty JSON.
    let notes = r#"
i=0
while [ "$i" -lt 20 ]; do
    printf 'note: fixture progress update for lint step %s\n' "$i" >&2
    i=$((i + 1))
done
"#;
    let cases = [
        (
            "npx",
            ["npx", "--max-warnings=0"].as_slice(),
            ["eslint", "-f", "json", "--max-warnings=0", "."].as_slice(),
            "printf '[]\\n'",
            0,
            "eslint: no issues found",
        ),
        (
            "ruff",
            ["ruff", "check", "--output-format=text"].as_slice(),
            ["check", "--output-format=json", "."].as_slice(),
            "printf '[]\\n'",
            0,
            "ruff: no issues found",
        ),
        (
            "ruff",
            ["ruff", "sample.py"].as_slice(),
            ["check", "--output-format=json", "sample.py"].as_slice(),
            ":",
            0,
            "ruff: no issues found",
        ),
        (
            "pylint",
            ["pylint", "--output-format=text"].as_slice(),
            ["--output-format=json2", "."].as_slice(),
            "printf '[]\\n'",
            0,
            "pylint: no issues found",
        ),
        (
            "mypy",
            ["mypy", "sample.py"].as_slice(),
            ["sample.py"].as_slice(),
            "printf '%s\\n' \
             'sample.py:10: error: Incompatible types in assignment  [assignment]' \
             'Found 1 error in 1 file (checked 1 source file)' >&2\nexit 1",
            1,
            "mypy:",
        ),
        (
            "flake8",
            ["flake8", "sample.py"].as_slice(),
            ["sample.py"].as_slice(),
            "printf '%s\\n' 'WARNING: fixture diagnostic' >&2\nexit 1",
            1,
            "lint: 0 errors, 1 warnings",
        ),
    ];

    for (tool, args, argv, body, status, summary) in cases {
        let output = lint_with_fake_tool(tool, args, argv, &format!("{notes}{body}"));
        assert_eq!(output.status.code(), Some(status), "{tool}: {output:?}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.to_ascii_lowercase().contains(summary),
            "{tool}: {output:?}"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        let linter = if tool == "npx" { "eslint" } else { tool };
        assert!(stderr.contains(&format!("Running: {linter} with structured output")));
    }
}

#[cfg(all(unix, feature = "engine"))]
#[test]
fn lint_reports_abnormal_status_without_rendering_partial_results() {
    if !crate::core::test_isolation::rtk_binary_is_built() {
        return;
    }

    let output = lint_with_fake_tool(
        "flake8",
        &["flake8", "sample.py"],
        &["sample.py"],
        "printf '%s\\n' 'ERROR: partial result'\n\
         printf '%s\\n' detail-one detail-two detail-three detail-four detail-five detail-six >&2\n\
         exit 137",
    );
    assert_eq!(output.status.code(), Some(137), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Linter process terminated abnormally"),
        "{stderr}"
    );
    assert!(
        stderr.contains("stderr: detail-one\ndetail-two\ndetail-three\ndetail-four\ndetail-five"),
        "{stderr}"
    );
    assert!(!stderr.contains("detail-six"), "{stderr}");
}
