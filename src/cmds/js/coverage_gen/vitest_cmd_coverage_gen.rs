use super::*;

fn assert_verbose_format(report: &str, expected: &TestResult) {
    let root = crate::core::test_isolation::tempdir();
    crate::core::test_isolation::with_root(root.path(), || {
        let combined = format!("{report}\nreporter diagnostic");
        let formatted = format_test_output("vitest", report, &combined, false, 1);

        assert!(!formatted.truncated);
        assert_eq!(
            formatted.text,
            expected.format(FormatMode::from_verbosity(1))
        );
    });
}

#[test]
fn json_report_keeps_only_failed_assertions_and_joins_their_messages() {
    let report = r#"{
        "numTotalTests": 3,
        "numPassedTests": 1,
        "numFailedTests": 1,
        "numPendingTests": 1,
        "testResults": [{
            "name": "checkout.test.ts",
            "assertionResults": [
                {"fullName": "checkout accepts cash", "status": "passed", "failureMessages": []},
                {"fullName": "checkout rejects expired cards", "status": "failed",
                 "failureMessages": ["expected declined", "received approved"]},
                {"fullName": "checkout accepts vouchers", "status": "pending", "failureMessages": []}
            ]
        }]
    }"#;

    let ParseResult::Full(result) = VitestParser::parse(report) else {
        panic!("expected a full JSON result");
    };
    assert_eq!(
        (result.total, result.passed, result.failed, result.skipped),
        (3, 1, 1, 1)
    );
    assert_eq!(result.duration_ms, None);
    let [failure] = result.failures.as_slice() else {
        panic!("expected only the failed assertion");
    };
    assert_eq!(failure.test_name, "checkout rejects expired cards");
    assert_eq!(failure.file_path, "checkout.test.ts");
    assert_eq!(
        failure.error_message,
        "expected declined\nreceived approved"
    );
    assert!(failure.stack_trace.is_none());

    assert_verbose_format(report, &result);
}

#[test]
fn text_report_recovers_duration_and_keeps_failure_details_separate() {
    let report = concat!(
        "\x1b[31mFAIL\x1b[0m checkout rejects expired cards\n",
        "  expected declined  \n",
        "    received approved\n",
        "PASS checkout accepts cash\n",
        "[x] refunds reject duplicate requests\n",
        "Tests  2 failed | 3 passed (5)\n",
        "Duration  1.25s\n",
    );

    let ParseResult::Degraded(result, warnings) = VitestParser::parse(report) else {
        panic!("expected a degraded result from the text reporter");
    };
    assert!(matches!(
        warnings.as_slice(),
        [warning] if warning.starts_with("JSON parse failed:")
    ));
    assert_eq!(
        (result.total, result.passed, result.failed, result.skipped),
        (5, 3, 2, 0)
    );
    assert_eq!(result.duration_ms, Some(1_250));

    let [first, second] = result.failures.as_slice() else {
        panic!("expected a failure for each of the two markers");
    };
    assert_eq!(first.test_name, "FAIL checkout rejects expired cards");
    assert_eq!(first.error_message, "expected declined\nreceived approved");
    assert_eq!(second.test_name, "[x] refunds reject duplicate requests");
    assert!(second.error_message.is_empty());
    for failure in &result.failures {
        assert!(failure.file_path.is_empty());
        assert!(failure.stack_trace.is_none());
    }

    assert_verbose_format(report, &result);
}

#[test]
fn explicit_reporter_keeps_combined_output_but_parse_failure_keeps_stdout() {
    let root = crate::core::test_isolation::tempdir();
    crate::core::test_isolation::with_root(root.path(), || {
        crate::core::user_env::with_vars(&[("RTK_TEE", Some("0"))], || {
            let stdout = "reporter setup complete";
            let combined = "reporter setup complete\nreporter diagnostic";

            let explicit = format_test_output("vitest", stdout, combined, true, 0);
            assert_eq!(explicit.text, combined);
            assert!(!explicit.truncated);

            let fallback = format_test_output("vitest", stdout, combined, false, 0);
            assert_eq!(fallback.text, stdout);
            assert!(!fallback.truncated);
            assert_eq!(
                render_test_output(&fallback, combined, "vitest_run", 0),
                stdout
            );
        });
    });
}

#[cfg(unix)]
#[test]
fn cli_runs_vitest_and_jest_with_their_respective_json_flags() {
    use crate::core::test_isolation::{rtk_binary_is_built, rtk_command, tempdir};
    use std::os::unix::fs::PermissionsExt;

    if !rtk_binary_is_built() {
        return;
    }

    let project = tempdir();
    let bin = project.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    std::fs::write(
        project.path().join("package.json"),
        r#"{"name":"coverage-fixture","packageManager":"npm@10.0.0"}"#,
    )
    .unwrap();

    // Accept direct invocation or a package-manager prefix, then check the
    // framework arguments before emitting a report.
    let script = r#"#!/bin/sh
framework=${0##*/}
case "$framework" in
    vitest|jest) ;;
    *)
        while [ "$#" -gt 0 ]; do
            case "$1" in
                vitest|jest) break ;;
            esac
            shift
        done
        [ "$#" -gt 0 ] || exit 97
        framework=$1
        shift
        ;;
esac
case "$framework:$#:$*" in
    'vitest:3:run --reporter=json probe.test.js') passed=13 ;;
    'jest:3:--no-watch --json probe.test.js') passed=17 ;;
    *) printf 'unexpected arguments: %s %s\n' "$framework" "$*" >&2; exit 98 ;;
esac
printf '{\n  "numTotalTests": %s,\n  "numPassedTests": %s,\n  "numFailedTests": 0,\n  "testResults": []\n}\n' "$passed" "$passed"
"#;

    for name in [
        "vitest", "jest", "npx", "npm", "pnpm", "pnpx", "yarn", "bun", "bunx", "corepack",
    ] {
        let path = bin.join(name);
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    for (framework, passed) in [("vitest", 13), ("jest", 17)] {
        let mut command = rtk_command();
        let inherited_path = command
            .get_envs()
            .find_map(|(key, value)| if key == "PATH" { value } else { None })
            .expect("isolated rtk command should have a PATH");
        let path = std::env::join_paths(
            std::iter::once(bin.clone()).chain(std::env::split_paths(inherited_path)),
        )
        .unwrap();

        let output = command
            .current_dir(project.path())
            .env("PATH", path)
            .stdin(std::process::Stdio::null())
            .args([framework, "probe.test.js"])
            .output()
            .unwrap();

        assert_eq!(
            output.status.code(),
            Some(0),
            "{framework}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        let expected = TestResult {
            total: passed,
            passed,
            failed: 0,
            skipped: 0,
            duration_ms: None,
            failures: Vec::new(),
        }
        .format(FormatMode::from_verbosity(0));
        assert!(!stdout.trim().is_empty(), "{framework} printed no summary");
        assert_eq!(
            strip_ansi(stdout.trim()),
            strip_ansi(expected.trim()),
            "{framework} should print the parsed summary"
        );
    }
}
