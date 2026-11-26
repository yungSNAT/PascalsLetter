use super::*;

#[test]
fn test_summary_counts_results_and_limits_failure_details() {
    let output = [
        "ok first case",
        "7 passed",
        "✓ another case",
        "0 failed",
        "2 failed",
        "3 skipped",
        "4 ignored",
        "[x] first failure",
        "FAIL second case",
        "[x] third failure",
        "[x] fourth failure",
        "[x] omitted failure",
    ]
    .join("\n");

    // The passed and skipped counts and the first failure entry are left
    // unpinned: the heuristic adds marker lines to a reported count, lets
    // "4 ignored" replace "3 skipped", and lists the "2 failed" count line
    // as a failure.
    let summary = summarize_output(&output, "cargo test", false);
    let lines: Vec<_> = summary.lines().collect();
    assert_eq!(
        lines[..4],
        [
            "[FAIL] Command: cargo test",
            "   12 lines of output",
            "",
            "Test Results:",
        ]
    );
    assert!(lines.contains(&"   [FAIL] 2 failed"), "{summary}");
    let failures = lines
        .iter()
        .position(|line| *line == "   Failures:")
        .expect("a failures section");
    assert!(lines[failures + 1..].len() <= 5, "{summary}");
    for shown in ["[x] first failure", "FAIL second case", "[x] third failure"] {
        assert!(
            lines.contains(&format!("   • {shown}").as_str()),
            "{summary}"
        );
    }
    assert!(!summary.contains("omitted failure"), "{summary}");
    assert!(!summary.contains("0 failed"), "{summary}");
}

#[test]
fn extract_number_matches_the_requested_label_and_rejects_invalid_input() {
    assert_eq!(extract_number("3 passed; 12failed", "failed"), Some(12));
    assert_eq!(extract_number("3 passed", "failed"), None);
    assert_eq!(extract_number("3 passed", "["), None);
    assert_eq!(
        extract_number(&format!("{}0 passed", usize::MAX), "passed"),
        None
    );
}

#[test]
fn build_summary_distinguishes_diagnostics_from_zero_counts() {
    let output = [
        "Compiling core",
        "Compiled helper",
        "WARNING: unused setting",
        "0 errors and 0 warnings",
        "error: first diagnostic",
        "error: second diagnostic",
        "error: third diagnostic",
        "error: fourth diagnostic",
        "error: fifth diagnostic",
        "error: omitted diagnostic",
    ]
    .join("\n");

    let summary = summarize_output(&output, "show transcript", false);
    assert_eq!(
        summary.lines().collect::<Vec<_>>(),
        [
            "[FAIL] Command: show transcript",
            "   10 lines of output",
            "",
            "Build Summary:",
            "   2 crates/files compiled",
            "   [error] 6 errors",
            "   [warn] 1 warnings",
            "",
            "   Errors:",
            "   • error: first diagnostic",
            "   • error: second diagnostic",
            "   • error: third diagnostic",
            "   • error: fourth diagnostic",
            "   • error: fifth diagnostic",
        ]
    );

    let clean = summarize_output(
        "Compiled helper\n0 errors and 0 warnings",
        "compile helper",
        true,
    );
    assert_eq!(
        clean.lines().collect::<Vec<_>>(),
        [
            "[ok] Command: compile helper",
            "   2 lines of output",
            "",
            "Build Summary:",
            "   1 crates/files compiled",
            "   [ok] Build successful",
        ]
    );
}

#[test]
fn log_summary_counts_only_the_highest_severity_on_each_line() {
    let output = [
        "ERROR: WARN: [INFO] secondary tags",
        "FATAL unable to open stream",
        "WARN: [INFO] slow reply",
        "[INFO] ready",
        "heartbeat",
    ]
    .join("\n");

    let summary = summarize_output(&output, "read events", true);
    assert_eq!(
        summary.lines().collect::<Vec<_>>(),
        [
            "[ok] Command: read events",
            "   5 lines of output",
            "",
            "Log Summary:",
            "   [error] 2 errors",
            "   [warn] 1 warnings",
            "   [info] 1 info",
        ]
    );
}

#[test]
fn list_summary_skips_blank_rows_and_reports_omitted_items() {
    let items = (0..=MAX_SUMMARY_LIST)
        .map(|index| format!("item-{index}"))
        .collect::<Vec<_>>();
    let summary = summarize_output(&items.join("\n\n"), "show items", true);

    let mut expected = vec![format!("List ({} items):", items.len())];
    expected.extend(
        items
            .iter()
            .take(MAX_SUMMARY_LIST)
            .map(|item| format!("   • {item}")),
    );
    expected.push("   ... +1 more".to_owned());
    assert_eq!(summary.lines().skip(3).collect::<Vec<_>>(), expected);
}

#[test]
fn json_summary_reports_structure_and_caps_the_key_list() {
    let keys = (0..=MAX_SUMMARY_KEYS)
        .map(|index| format!("key-{index:04}"))
        .collect::<Vec<_>>();
    let object = serde_json::Value::Object(
        keys.iter()
            .map(|key| (key.clone(), serde_json::Value::Null))
            .collect(),
    );
    let summary = summarize_output(&object.to_string(), "show data", true);

    let mut expected = vec![
        "JSON Output:".to_owned(),
        format!("   Object with {} keys:", keys.len()),
    ];
    expected.extend(
        keys.iter()
            .take(MAX_SUMMARY_KEYS)
            .map(|key| format!("   • {key}")),
    );
    expected.push("   ... +1 more keys".to_owned());
    assert_eq!(summary.lines().skip(3).collect::<Vec<_>>(), expected);

    for (output, detail) in [
        (" \n  [1, {\"name\":\"a\"}, null]", "   Array with 3 items"),
        (" {broken", "   (Invalid JSON)"),
    ] {
        let summary = summarize_output(output, "show data", true);
        assert_eq!(
            summary.lines().skip(3).collect::<Vec<_>>(),
            ["JSON Output:", detail]
        );
    }

    // Scalar JSON is supported by the formatter even though the detector
    // only routes object- and array-shaped output to it.
    let mut scalar = Vec::new();
    summarize_json(" \t true ", &mut scalar);
    assert_eq!(scalar, ["JSON Output:", "   true"]);
}

#[test]
fn tabular_output_keeps_only_nonblank_head_and_tail_lines() {
    let output = [
        "field\tvalue",
        "  ",
        "alpha",
        "beta",
        "gamma",
        "middle-one",
        "middle-two",
        "middle-three",
        "middle-four",
        "tail-a",
        "",
        "tail-b",
    ]
    .join("\n");

    let summary = summarize_output(&output, "show rows", true);
    assert_eq!(
        summary.lines().collect::<Vec<_>>(),
        [
            "[ok] Command: show rows",
            "   12 lines of output",
            "",
            "Output:",
            "   field\tvalue",
            "   alpha",
            "   beta",
            "   gamma",
            "   ...",
            "   tail-a",
            "   tail-b",
        ]
    );
}

#[cfg(unix)]
fn write_executable(path: &std::path::Path, script: &str) {
    use std::os::unix::fs::PermissionsExt;

    std::fs::write(path, script).expect("write the fake executable");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .expect("make the fake tool executable");
}

#[cfg(unix)]
#[test]
fn verbose_summary_reports_a_found_tool_with_a_missing_interpreter() {
    use crate::core::test_isolation;

    if !test_isolation::rtk_binary_ready() {
        return;
    }

    let dir = test_isolation::tempdir();
    let tool = "rtk-summary-bad-interpreter";
    let interpreter = dir.path().join("missing-interpreter");
    write_executable(
        &dir.path().join(tool),
        &format!("#!{}\n:\n", interpreter.display()),
    );

    let mut cmd = test_isolation::rtk_command();
    let inherited_path = cmd
        .get_envs()
        .find(|(name, _)| *name == "PATH")
        .and_then(|(_, value)| value)
        .expect("isolated rtk command provides PATH");
    let path = std::env::join_paths(
        std::iter::once(dir.path().to_path_buf()).chain(std::env::split_paths(inherited_path)),
    )
    .expect("assemble the isolated PATH");

    let output = cmd
        .env("PATH", path)
        .args(["-v", "summary", tool])
        .output()
        .expect("run the isolated rtk binary");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(127), "{stderr}");
    assert!(
        stderr
            .lines()
            .any(|line| line == format!("Running and summarizing: {tool}")),
        "{stderr}"
    );
    assert!(stdout.contains(tool), "{stdout}");
}

#[cfg(unix)]
#[test]
fn summary_propagates_an_invalid_process_argument_as_an_error() {
    use crate::core::test_isolation;

    let dir = test_isolation::tempdir();
    let tool = dir.path().join("rtk-summary-unused-tool");
    write_executable(&tool, "#!/bin/sh\n:\n");
    let args = [
        tool.to_str().expect("UTF-8 scratch path").to_owned(),
        // Command rejects an embedded NUL before any tool can run.
        "invalid\0argument".to_owned(),
    ];

    let error = test_isolation::with_root(dir.path(), || {
        let _guard = test_isolation::enter(dir.path());
        run(&args, None, 0)
    })
    .expect_err("invalid arguments must not become a summarized exit status");

    assert_eq!(error.to_string(), "Failed to execute command");
    assert!(error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io_error| io_error.kind() == std::io::ErrorKind::InvalidInput)
    }));
}
