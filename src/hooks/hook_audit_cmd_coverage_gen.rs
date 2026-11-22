use super::{base_command, default_log_path, run};
use crate::core::{test_isolation, user_env};
use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use std::process::Command;

fn successful_stdout(command: &mut Command) -> String {
    let output = command.output().expect("run the isolated rtk binary");
    assert!(
        output.status.success(),
        "rtk exited with {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("audit report should be UTF-8")
}

fn check_report(dir: &Path, since_days: u64, verbose: bool, check_stdout: impl FnOnce(&str)) {
    user_env::with_path("RTK_AUDIT_DIR", Some(dir), || {
        run(since_days, u8::from(verbose)).expect("audit should succeed");
    });

    // Exercise run in process even when the CLI is unavailable for output checks.
    if !test_isolation::rtk_binary_is_built() {
        return;
    }

    let mut command = test_isolation::rtk_command();
    command.env("RTK_AUDIT_DIR", dir);
    if verbose {
        command.arg("-v");
    }
    command
        .args(["hook", "audit", "--since"])
        .arg(since_days.to_string());
    check_stdout(&successful_stdout(&mut command));
}

#[test]
fn audit_distinguishes_missing_empty_and_expired_logs() {
    let dir = test_isolation::tempdir();
    let log_path = dir.path().join("hook-audit.log");

    check_report(dir.path(), 0, false, |stdout| {
        let mut lines = stdout.lines();
        assert_eq!(
            lines.next().unwrap(),
            format!("No audit log found at {}", log_path.display())
        );
        assert!(lines.next().unwrap().contains("RTK_HOOK_AUDIT=1"));
        assert_eq!(lines.next(), None);
    });

    fs::write(&log_path, "\nnot an audit entry\n").unwrap();
    check_report(dir.path(), 0, false, |stdout| {
        assert_eq!(stdout.trim_end(), "Audit log is empty.");
    });

    fs::write(
        &log_path,
        "1900-01-01T00:00:00Z | rewrite | git status | rtk git status\n",
    )
    .unwrap();
    check_report(dir.path(), 7, false, |stdout| {
        assert_eq!(stdout.trim_end(), "No entries in the last 7 days.");
    });
}

#[test]
fn audit_reports_read_errors_at_the_configured_path() {
    let dir = test_isolation::tempdir();
    let log_path = dir.path().join("hook-audit.log");
    fs::write(&log_path, b"\xff").unwrap();

    user_env::with_path("RTK_AUDIT_DIR", Some(dir.path()), || {
        assert_eq!(default_log_path(), log_path);
        let error = run(0, 0).expect_err("invalid UTF-8 is not an empty audit log");
        assert_eq!(
            error.to_string(),
            format!("Failed to read {}", log_path.display())
        );
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::InvalidData
        );
    });
}

#[test]
fn audit_ranks_rewrites_and_skip_reasons_for_the_requested_period() {
    let dir = test_isolation::tempdir();
    let log_path = dir.path().join("hook-audit.log");
    let recent = crate::core::utils::days_ago_cutoff(1)
        .strftime("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    let assignments = "FIRST=one  SECOND=two";
    assert_eq!(base_command(assignments), assignments);

    let mut log = String::from(
        "not an audit entry\n1900-01-01T00:00:00Z | rewrite | git status | rtk git status\n",
    );
    for (original, count) in [
        ("ls", 1),
        ("CI=1 git status --short", 3),
        ("pwd", 2),
        (assignments, 4),
        ("cargo test --lib", 5),
        ("rg src --files", 3),
        ("git status --porcelain", 3),
    ] {
        for _ in 0..count {
            writeln!(log, "{recent} | rewrite | {original} | -").unwrap();
        }
    }
    for (reason, original) in [
        ("no_match", "echo hello"),
        ("already_rewritten", "rtk git log"),
        ("no_match", "git status --ignored"),
    ] {
        writeln!(log, "{recent} | skip:{reason} | {original} | -").unwrap();
    }
    fs::write(&log_path, log).unwrap();

    let separator = "─".repeat(30);
    check_report(dir.path(), 0, false, |stdout| {
        assert_eq!(
            stdout.lines().collect::<Vec<_>>(),
            [
                "Hook Audit (all time)",
                separator.as_str(),
                "Total invocations: 25",
                "Rewrites:          22 (88.0%)",
                "Skips:             3 (12.0%)",
                "  no_match:      2",
                "  already_rewritten: 1",
                "Top commands: git status (7), cargo test (5), FIRST=one  SECOND=two (4), rg src (3), pwd (2)",
            ]
        );
    });

    check_report(dir.path(), 7, true, |stdout| {
        let log_line = format!("Log: {}", log_path.display());
        assert_eq!(
            stdout.lines().collect::<Vec<_>>(),
            [
                "Hook Audit (last 7 days)",
                separator.as_str(),
                "Total invocations: 24",
                "Rewrites:          21 (87.5%)",
                "Skips:             3 (12.5%)",
                "  no_match:      2",
                "  already_rewritten: 1",
                "Top commands: git status (6), cargo test (5), FIRST=one  SECOND=two (4), rg src (3), pwd (2)",
                "",
                log_line.as_str(),
            ]
        );
    });
}

#[test]
fn audit_defaults_to_the_childs_isolated_home() {
    if !test_isolation::rtk_binary_is_built() {
        return;
    }

    let home = test_isolation::tempdir();
    let expected = home
        .path()
        .join(".local")
        .join("share")
        .join("rtk")
        .join("hook-audit.log");
    let mut command = test_isolation::rtk_command();
    command
        .env("HOME", home.path())
        .env_remove("RTK_AUDIT_DIR")
        .args(["hook", "audit"]);

    let stdout = successful_stdout(&mut command);
    let reported = stdout
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("No audit log found at "))
        .expect("audit should report the missing default log path");
    assert_eq!(Path::new(reported), expected.as_path());
}
