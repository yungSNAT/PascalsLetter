use super::*;
use crate::core::{tee, test_isolation};

#[test]
fn replacing_the_tee_label_preserves_failure_options() {
    let opts = RunOptions::with_tee("original")
        .early_exit_on_failure()
        .tee("replacement");

    assert_eq!(opts.tee_label, Some("replacement"));
    assert!(opts.skip_filter_on_failure);
}

#[test]
fn stderr_cap_requires_both_excess_lines_and_a_smaller_result() {
    let long_line = "x".repeat(tee::MIN_TEE_SIZE);
    let short_report = format!("{long_line}\n");
    assert_eq!(
        forwarded_stderr(&short_report, 0, "summary", |_| {
            panic!("a report within the line budget needs no recovery entry")
        }),
        short_report
    );

    // Exactly one line would be dropped, and it is shorter than the elision
    // notice alone. The large final line puts the report above the store floor.
    let progress = "still working\n".repeat(MAX_FORWARDED_STDERR_LINES - 1);
    let raw = format!("x\n{progress}{long_line}\n");
    let mut asked_for_hint = false;
    let shown = forwarded_stderr(&raw, 0, "summary", |saved| {
        asked_for_hint = true;
        assert_eq!(saved, raw);
        Some("[full output: saved]".to_string())
    });

    assert!(asked_for_hint);
    assert_eq!(shown, raw);
}

#[test]
fn error_filters_keep_context_but_close_blocks_at_boundaries() {
    let raw = concat!(
        "linking objects\n",
        "error: could not link\n",
        "\n",
        "  missing symbol\n",
        " \t\n",
        "\n",
        "  unrelated detail\n",
        "warning: unused value\n",
        "\tdeclared here\n",
        "continuing\n",
        "  unrelated progress\n",
    );
    let expected =
        "error: could not link\n\n  missing symbol\n \t\nwarning: unused value\n\tdeclared here";

    assert_eq!(filter_errors(raw), expected);

    let mut filter = ErrorStreamFilter::new();
    let streamed: String = raw
        .lines()
        .filter_map(|line| filter.feed_line(line))
        .collect();
    assert_eq!(streamed, format!("{expected}\n"));
    assert_eq!(filter.on_exit(1, raw), None);
}

#[test]
fn test_summaries_keep_ecosystem_specific_failures_and_totals() {
    let cases = [
        (
            TestEcosystem::Cargo,
            concat!(
                "running 2 tests\n",
                "    unrelated setup detail\n",
                "test adds ... ok\n",
                "test subtracts ... \x1b[31mFAILED\x1b[0m\n",
                "failures:\n",
                "    subtracts\n",
                "test result: FAILED. 1 passed; 1 failed; 0 ignored\n",
            ),
            concat!(
                "[FAIL] FAILURES:\n",
                "  test subtracts ... FAILED\n",
                "  subtracts\n",
                "\n",
                "SUMMARY:\n",
                "  test result: FAILED. 1 passed; 1 failed; 0 ignored\n",
            ),
        ),
        (
            TestEcosystem::Pytest,
            concat!(
                "collected 2 items\n",
                "FAILED test_math.py::test_subtract - AssertionError\n",
                "=== 1 failed, 1 passed in 0.01s ===\n",
            ),
            concat!(
                "[FAIL] FAILURES:\n",
                "  FAILED test_math.py::test_subtract - AssertionError\n",
                "\n",
                "SUMMARY:\n",
                "  === 1 failed, 1 passed in 0.01s ===\n",
            ),
        ),
        (
            TestEcosystem::Jest,
            concat!(
                "FAIL math.test.js\n",
                "  ✕ subtracts\n",
                "    unrelated console output\n",
                "Test Suites: 1 failed, 1 total\n",
                "Tests:       1 failed, 1 passed, 2 total\n",
            ),
            concat!(
                "[FAIL] FAILURES:\n",
                "  FAIL math.test.js\n",
                "  ✕ subtracts\n",
                "\n",
                "SUMMARY:\n",
                "  Test Suites: 1 failed, 1 total\n",
                "  Tests:       1 failed, 1 passed, 2 total\n",
            ),
        ),
        (
            TestEcosystem::Deno,
            concat!(
                "TS2322 [ERROR]: Expected a number.\n",
                "const count: number = \"one\";\n",
                "\n",
                "FAILED | 0 passed | 1 failed (1ms)\n",
            ),
            concat!(
                "[FAIL] FAILURES:\n",
                "  TS2322 [ERROR]: Expected a number.\n",
                "\n",
                "SUMMARY:\n",
                "  FAILED | 0 passed | 1 failed (1ms)\n",
            ),
        ),
    ];

    for (ecosystem, raw, expected) in cases {
        assert_eq!(
            extract_test_summary(raw, ecosystem),
            expected,
            "{ecosystem:?}"
        );
    }

    // Go: the failures are pinned exactly; the summary repeats them today, so
    // it is checked only for the package result lines.
    let go = extract_test_summary(
        concat!(
            "=== RUN   TestSubtract\n",
            "--- FAIL: TestSubtract (0.00s)\n",
            "    math_test.go:14: expected 1, got 2\n",
            "FAIL\tmath\t0.003s\n",
            "ok  \tgeometry\t0.002s\n",
        ),
        TestEcosystem::Go,
    );
    let (failures, summary) = go.split_once("\nSUMMARY:\n").expect("summary section");
    assert_eq!(
        failures, "[FAIL] FAILURES:\n  --- FAIL: TestSubtract (0.00s)\n  FAIL\tmath\t0.003s\n",
        "{go}"
    );
    assert!(summary.contains("  FAIL\tmath\t0.003s\n"), "{go}");
    assert!(summary.contains("  ok  \tgeometry\t0.002s\n"), "{go}");
}

#[test]
fn bun_coverage_cap_keeps_unparseable_rows_ahead_of_fully_covered_files() {
    const HEADER: &str = "File | % Funcs | % Lines | Uncovered Line #s";
    const UNKNOWN: &str = "unknown.test.ts | 100.00 | n/a |";
    let covered: Vec<_> = (0..MAX_BUN_COVERAGE_ROWS + 64)
        .map(|i| format!("fixture_{i:03}.test.ts | 100.00 | 100.00 |"))
        .collect();
    let raw = format!("{HEADER}\n{}\n{UNKNOWN}\n\n 1 pass\n", covered.join("\n"));
    let root = test_isolation::tempdir();
    let summary = test_isolation::with_root(root.path(), || {
        extract_test_summary(&raw, TestEcosystem::Bun)
    });

    let shown: Vec<_> = summary
        .lines()
        .filter_map(|line| line.strip_prefix("  "))
        .filter(|line| line.contains('|'))
        .collect();
    let mut expected = vec![HEADER];
    expected.extend(
        covered[..MAX_BUN_COVERAGE_ROWS - 2]
            .iter()
            .map(String::as_str),
    );
    expected.push(UNKNOWN);
    assert_eq!(shown, expected);
    assert!(summary.contains(&format!(
        "... +{} more files",
        covered.len() + 2 - MAX_BUN_COVERAGE_ROWS
    )));
    assert!(
        summary.contains("rtk recall ") || summary.contains("tail -n +1 "),
        "missing recovery hint:\n{summary}"
    );
    assert!(summary.ends_with("   1 pass\n"));
}

#[test]
fn runner_modes_report_spawn_errors_under_the_tool_name() {
    let root = test_isolation::tempdir();
    let _cwd = test_isolation::enter(root.path());
    test_isolation::with_root(root.path(), || {
        let missing = root.path().join("missing-runner-tool");
        let command = || {
            // nosemgrep: dynamic-command-execution -- a test: a deliberately missing program in a temp dir
            let mut cmd = Command::new(&missing);
            cmd.env_clear().current_dir(root.path());
            cmd
        };

        let captured = run_test_cmd(
            command(),
            "test fixture",
            "test",
            "test-fixture",
            TestEcosystem::Cargo,
            1,
            false,
        )
        .expect_err("a failed spawn is not an empty test run");
        assert_eq!(captured.to_string(), "Failed to run test fixture");

        let streamed = run_err_cmd(command(), "error fixture", "check", "error-fixture", 1)
            .expect_err("a failed spawn must not produce a clean streamed result");
        assert_eq!(streamed.to_string(), "Failed to run error fixture");

        let tool = missing.to_str().expect("scratch tool path must be UTF-8");
        let passthrough =
            run_passthrough(tool, &[], 1).expect_err("passthrough must report a failed spawn");
        assert_eq!(passthrough.to_string(), format!("Failed to run {tool}"));
    });
}

#[test]
fn unrunnable_report_passes_the_diagnostic_and_status_to_the_renderer() {
    let root = test_isolation::tempdir();
    let _cwd = test_isolation::enter(root.path());
    test_isolation::with_root(root.path(), || {
        let outcome = crate::core::shell::Unrunnable {
            code: 126,
            message: "fixture tool could not be executed".to_string(),
        };
        let mut rendered = false;
        let code = report_unrunnable("fixture", "check", &outcome, 1, |raw, code| {
            rendered = true;
            assert_eq!(raw, "fixture tool could not be executed");
            assert_eq!(code, 126);
            format!("[FAIL] {raw} (exit code: {code})")
        });

        assert!(rendered);
        assert_eq!(code, 126);
    });
}
