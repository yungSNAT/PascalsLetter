use super::{CAP_WARNINGS, analyze_logs, reduced, run_file};
use crate::core::test_isolation;
use std::fs;
use std::io::{self, Write};
use std::process::Stdio;

#[test]
fn log_summary_caps_unique_groups_but_keeps_the_most_frequent_warning() {
    let error_limit = CAP_WARNINGS;
    let warning_limit = reduced(CAP_WARNINGS, 5);
    let error_total = error_limit + 2;
    let unique_warning_total = warning_limit + 3;
    let warning_total = unique_warning_total + 1;

    let first_warning = "2024-01-01T10:00:00 WARN: retry job 1234";
    let mut logs = vec![
        first_warning.to_owned(),
        "2024-01-01T10:00:01 WARN: retry job 5678".to_owned(),
    ];
    logs.extend((0..error_total).map(|index| format!("ERROR: failure e{index}")));
    logs.extend((0..unique_warning_total - 1).map(|index| format!("WARN: signal w{index}")));

    let summary = analyze_logs(&logs.join("\n"));
    let (header, details) = summary.split_once("\n[ERRORS]\n").unwrap();
    let (errors, warnings) = details.split_once("\n[WARNINGS]\n").unwrap();

    assert_eq!(
        header.trim_end(),
        format!(
            "Log Summary\n   [error] {error_total} errors ({error_total} unique)\n\
             \x20  [warn] {warning_total} warnings ({unique_warning_total} unique)\n\
             \x20  [info] 0 info messages"
        )
    );
    assert_eq!(errors.lines().count(), error_limit + 1);
    assert_eq!(errors.lines().last(), Some("   ... +2 more unique errors"));
    assert_eq!(warnings.lines().count(), warning_limit + 1);
    assert_eq!(
        warnings.lines().next().unwrap(),
        format!("   [×2] {first_warning}")
    );
    assert_eq!(
        warnings.lines().last(),
        Some("   ... +3 more unique warnings")
    );
}

#[test]
fn run_file_rejects_invalid_utf8_and_accepts_a_valid_log() {
    let scratch = test_isolation::tempdir();
    let _working_dir = test_isolation::enter(scratch.path());
    let file = scratch.path().join("application.log");

    test_isolation::with_root(scratch.path(), || {
        fs::write(&file, [0xff]).unwrap();
        let error = run_file(&file, 0).unwrap_err();
        assert_eq!(
            error.downcast_ref::<io::Error>().unwrap().kind(),
            io::ErrorKind::InvalidData
        );

        let input = "WARN: retrying request\n".repeat(32);
        fs::write(&file, &input).unwrap();
        run_file(&file, 1).unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), input);
    });
}

#[test]
fn log_command_summarizes_both_a_file_and_piped_input() {
    if !test_isolation::rtk_binary_is_built() {
        return;
    }

    let scratch = test_isolation::tempdir();
    let file = scratch.path().join("application.log");
    let input = "WARN: retrying request\r\nINFO: waiting for upstream\r\n".repeat(32);
    fs::write(&file, &input).unwrap();

    let from_file = test_isolation::rtk_command()
        .args(["-v", "log"])
        .arg(&file)
        .stdin(Stdio::null())
        .output()
        .unwrap();

    let mut child = test_isolation::rtk_command()
        .arg("log")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut pipe = child.stdin.take().unwrap();
    pipe.write_all(input.as_bytes()).unwrap();
    drop(pipe);
    let from_pipe = child.wait_with_output().unwrap();

    let file_stderr = String::from_utf8_lossy(&from_file.stderr);
    assert!(
        file_stderr
            .lines()
            .any(|line| line.starts_with("Analyzing log: ") && line.ends_with("application.log")),
        "missing verbose file diagnostic: {file_stderr}"
    );

    for output in [from_file, from_pipe] {
        assert!(
            output.status.success(),
            "rtk log failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        for expected in [
            "Log Summary",
            "   [warn] 32 warnings (1 unique)",
            "   [info] 32 info messages",
            "   [×32] WARN: retrying request",
        ] {
            assert!(
                stdout.lines().any(|line| line == expected),
                "missing {expected:?} in log output:\n{stdout}"
            );
        }
    }
}
