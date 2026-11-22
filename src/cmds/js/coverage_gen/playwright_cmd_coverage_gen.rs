use super::{OutputParser, ParseResult, PlaywrightParser};

#[test]
fn failed_json_attempt_without_error_details_gets_a_default_message() {
    let report = r#"{
        "stats": {"expected": 0, "unexpected": 1, "skipped": 0},
        "suites": [{
            "title": "checkout.spec.ts",
            "specs": [{
                "title": "submits an order",
                "tests": [{
                    "status": "unexpected",
                    "results": [{"status": "timedOut"}]
                }]
            }]
        }]
    }"#;

    let ParseResult::Full(data) = PlaywrightParser::parse(report) else {
        panic!("expected a full JSON parse");
    };
    assert_eq!(
        (data.total, data.passed, data.failed, data.skipped),
        (1, 0, 1, 0)
    );

    let [failure] = data.failures.as_slice() else {
        panic!("expected one failure even without error details");
    };
    assert_eq!(failure.test_name, "submits an order");
    assert_eq!(failure.file_path, "checkout.spec.ts");
    assert_eq!(failure.error_message, "Test failed");
    assert!(failure.stack_trace.is_none());
}

#[test]
fn text_report_recovers_counts_failure_locations_and_duration() {
    for (duration, expected_ms) in [("42.75ms", 42), ("1.25m", 75_000)] {
        let report = format!(
            "\x1b[31m×\x1b[0m 1 [chromium] › checkout.spec.ts:6:2 › submits an order\n\
             ✗ 2 [firefox] › retry.spec.tsx:8:3 › retries payment\n\
             ✓ 3 [webkit] › cart.spec.ts:4:2 › shows an empty cart\n\
             2 failed\n1 flaky\n3 skipped\n4 passed ({duration})"
        );

        let ParseResult::Degraded(data, warnings) = PlaywrightParser::parse(&report) else {
            panic!("expected a degraded parse of the text report");
        };
        assert_eq!(warnings.len(), 1);
        assert_eq!(
            (data.total, data.passed, data.failed, data.skipped),
            (9, 4, 2, 3)
        );
        assert_eq!(data.duration_ms, Some(expected_ms));

        let failures: Vec<_> = data
            .failures
            .iter()
            .map(|failure| {
                assert!(failure.error_message.is_empty());
                assert!(failure.stack_trace.is_none());
                (failure.test_name.as_str(), failure.file_path.as_str())
            })
            .collect();
        assert_eq!(
            failures,
            [
                ("× 1 [chromium] › checkout.spec.ts", "checkout.spec.ts"),
                ("✗ 2 [firefox] › retry.spec.tsx", "retry.spec.tsx"),
            ]
        );
    }
}

#[cfg(unix)]
#[test]
fn cli_routes_playwright_through_the_package_manager_and_preserves_status() {
    use crate::core::test_isolation::{rtk_binary_is_built, rtk_command, tempdir};
    use std::fmt::Write as _;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Stdio;

    if !rtk_binary_is_built() {
        return;
    }

    let bin = tempdir();
    let noise = "reporter-internal-noise";
    let json_report = serde_json::json!({
        "config": {"fixturePadding": noise.repeat(160)},
        "stats": {"expected": 0, "unexpected": 1, "skipped": 0, "duration": 1500},
        "suites": [{
            "title": "checkout.spec.ts",
            "specs": [{
                "title": "submits an order",
                "tests": [{
                    "status": "unexpected",
                    "results": [{
                        "status": "timedOut",
                        "errors": [{"message": "checkout timed out"}]
                    }]
                }]
            }]
        }]
    })
    .to_string();
    let noise_lines = format!("{noise}\n").repeat(100);
    let text_report = format!(
        "{noise_lines}✗ 1 [firefox] › retry.spec.ts:8:3 › retries payment\n\
         1 failed\n3 passed (1.5m)"
    );
    let help_report = "Playwright fixture help for the test command";

    let test_args = ["test", "--reporter=line", "checkout flow.spec.ts"];
    let exec_args = [
        "exec",
        "--",
        "playwright",
        "test",
        "--reporter=json",
        "checkout flow.spec.ts",
    ];
    let help_args = ["help", "test"];
    let npx_args = ["--no-install", "--", "playwright", "help", "test"];

    let cases = [
        (
            "pnpm",
            "pnpm-lock.yaml",
            test_args.as_slice(),
            exec_args.as_slice(),
            json_report.as_str(),
            "checkout timed out",
            17,
        ),
        (
            "yarn",
            "yarn.lock",
            test_args.as_slice(),
            exec_args.as_slice(),
            text_report.as_str(),
            "retry.spec.ts",
            19,
        ),
        (
            "npx",
            "package-lock.json",
            help_args.as_slice(),
            npx_args.as_slice(),
            help_report,
            help_report,
            0,
        ),
    ];

    // Stub all three tools before running any case, including if routing regresses.
    for &(tool, _, _, tool_args, report, _, status) in &cases {
        let mut script = format!("#!/bin/sh\n[ \"$#\" -eq {} ] || exit 97\n", tool_args.len());
        for (index, arg) in tool_args.iter().enumerate() {
            writeln!(script, "[ \"${{{}}}\" = '{arg}' ] || exit 97", index + 1).unwrap();
        }
        writeln!(
            script,
            "printf '%s\\n' '{}'\nexit {status}",
            report.replace('\'', "'\\''")
        )
        .unwrap();

        let executable = bin.path().join(tool);
        fs::write(&executable, script).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    }

    for (tool, lockfile, args, _, _, expected, status) in cases {
        let project = tempdir();
        fs::write(project.path().join(lockfile), "").unwrap();

        let mut cmd = rtk_command();
        let inherited_path = cmd
            .get_envs()
            .find_map(|(key, value)| if key == "PATH" { value } else { None })
            .expect("isolated rtk command should have a PATH");
        let path = std::env::join_paths(
            std::iter::once(bin.path().to_path_buf()).chain(std::env::split_paths(inherited_path)),
        )
        .unwrap();

        let output = cmd
            .current_dir(project.path())
            .stdin(Stdio::null())
            .env("PATH", path)
            .args(["-v", "playwright"])
            .args(args)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert_eq!(
            output.status.code(),
            Some(status),
            "{tool}: {stdout}\n{stderr}"
        );
        assert!(
            stderr.contains(&format!("Running: playwright {}", args.join(" "))),
            "{tool}: {stderr}"
        );
        if tool == "npx" {
            assert_eq!(stdout.trim(), expected, "{tool}: {stderr}");
        } else {
            assert!(stdout.contains(expected), "{tool}: {stdout}");
            assert!(
                !stdout.contains(noise),
                "{tool} returned the unfiltered report"
            );
        }
    }
}
