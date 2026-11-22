// Tests for the private filters, stream handlers and runners in `cargo_cmd.rs`.
// Pure filter functions get captured-style text. The runners are reached by
// spawning the rtk binary with a fake `cargo` script on PATH.

use super::*;

/// Runs `f` with user directories redirected to a fresh temp root, so tee
/// files written by overflow hints never land in the real home directory.
fn with_isolated_root<R>(f: impl FnOnce() -> R) -> R {
    let tmp = tempfile::tempdir().expect("create temp root");
    crate::core::test_isolation::with_root(tmp.path(), f)
}

#[test]
fn test_compute_test_summary_none_and_raw_fallback() {
    // No summary lines and no compile errors: nothing to summarize.
    let handler = CargoTestHandler::new();
    assert_eq!(
        handler.compute_test_summary("nothing interesting here"),
        None
    );
    assert_eq!(handler.format_summary(0, "nothing interesting here"), None);

    // A failed `test result:` line cannot be aggregated, so it is shown verbatim.
    let line = "test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out";
    let mut handler = CargoTestHandler::new();
    assert!(handler.should_skip(line));
    assert_eq!(
        handler.compute_test_summary(line).as_deref(),
        Some(format!("{line}\n").as_str())
    );
}

#[test]
fn test_filter_cargo_install_bare_names_and_installed_line() {
    assert_eq!(
        filter_cargo_install("  Installing rtk\n"),
        "cargo install (rtk, 0 deps compiled)"
    );

    let output =
        "  Installed foo v1.2.3 (executable `foo`)\nwarning: `foo` (bin) generated 2 warnings\n";
    let result = filter_cargo_install(output);
    assert_eq!(result, "cargo install (foo v1.2.3, 0 deps compiled)");
    assert!(!result.contains("generated"), "got: {result}");
}

#[test]
fn test_filter_cargo_install_errors_without_crate_info() {
    let output = "some preamble\nerror[E0001]: first\nerror[E0002]: second\n";
    assert_eq!(
        filter_cargo_install(output),
        "cargo install: 2 errors\nerror[E0001]: first\n\nerror[E0002]: second"
    );
}

#[test]
fn test_filter_cargo_install_caps_errors() {
    let total = CAP_ERRORS + 2;
    let mut output = String::new();
    for i in 0..total {
        output.push_str(&format!("error[E0308]: e{i}\n"));
    }
    let result = with_isolated_root(|| filter_cargo_install(&output));
    assert!(
        result.starts_with(&format!("cargo install: {total} errors\n")),
        "got: {result}"
    );
    assert!(result.contains("… +2 more issues"), "got: {result}");
}

#[test]
fn test_flush_failure_block_edge_cases() {
    let mut failures: Vec<String> = Vec::new();
    let mut header = String::new();
    let mut body: Vec<String> = Vec::new();
    flush_failure_block(&mut header, &mut body, &mut failures);
    assert!(failures.is_empty());

    let mut header = "FAIL [ 0.1s] (1/1) x tests::a".to_string();
    flush_failure_block(&mut header, &mut body, &mut failures);
    assert_eq!(failures, vec!["FAIL [ 0.1s] (1/1) x tests::a".to_string()]);
    assert!(header.is_empty());
}

#[test]
fn test_filter_cargo_nextest_without_binary_count() {
    let pass = "    Starting something unexpected\n        PASS [   0.010s] (1/1) a\n     Summary [   0.010s] 1 test run: 1 passed, 0 skipped\n";
    assert_eq!(
        filter_cargo_nextest(pass),
        "cargo nextest: 1 passed (0.010s)"
    );

    let fail = "        FAIL [   0.010s] (1/1) proj tests::bad\n  panicked here\n     Summary [   0.010s] 1 test run: 0 passed, 1 failed\n";
    assert_eq!(
        filter_cargo_nextest(fail),
        "FAIL [   0.010s] (1/1) proj tests::bad\n  panicked here\ncargo nextest: 0 passed, 1 failed (0.010s)"
    );
}

#[test]
fn test_filter_cargo_nextest_fallbacks_keep_failures() {
    // No summary at all: the open failure block is closed at the end.
    let no_summary = "        FAIL [   0.010s] (1/1) proj tests::bad\n    boom\n";
    assert_eq!(
        filter_cargo_nextest(no_summary),
        "FAIL [   0.010s] (1/1) proj tests::bad\n    boom"
    );

    // Unparseable summary: failures then the raw summary line.
    let malformed =
        "        FAIL [   0.010s] (1/1) proj tests::bad\n    boom\n     Summary MALFORMED\n";
    assert_eq!(
        filter_cargo_nextest(malformed),
        "FAIL [   0.010s] (1/1) proj tests::bad\n    boom\nSummary MALFORMED"
    );
}

#[test]
fn test_extract_json_diagnostics_skips_odd_lines() {
    let output = concat!(
        "{not json\n",
        r#"{"reason":"compiler-message"}"#,
        "\n",
        r#"{"reason":"compiler-message","message":{"level":"note","message":"a note"}}"#,
        "\n",
        r#"{"reason":"compiler-message","message":{"level":"warning","message":"plain warning"}}"#,
        "\n",
        r#"{"reason":"compiler-message","message":{"level":"error","message":"","rendered":null}}"#,
        "\n",
    );
    let json = extract_json_diagnostics(output);
    assert!(json.errors.is_empty());
    assert_eq!(json.warnings, vec!["plain warning".to_string()]);
}

#[test]
fn test_filter_cargo_build_caps_blocks_and_skips_downloads() {
    let total = CAP_ERRORS + 3;
    let mut output = String::from("   Downloading crates ...\n");
    for i in 0..total {
        output.push_str(&format!("error[E0308]: mismatch {i}\n"));
    }
    let result = with_isolated_root(|| filter_cargo_build_labeled(&output, "build", 1));
    assert!(
        result.starts_with(&format!(
            "cargo build: {total} errors, 0 warnings (0 crates)"
        )),
        "got: {result}"
    );
    assert!(result.contains("… +3 more issues"), "got: {result}");
    assert!(!result.contains("Downloading"), "got: {result}");
}

#[test]
fn test_filter_cargo_test_summary_without_duration() {
    let output = "test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n";
    assert_eq!(filter_cargo_test(output), "cargo test: 3 passed (1 suite)");
}

#[test]
fn test_filter_cargo_test_unterminated_failure_block() {
    let output = "failures:\n---- a stdout ----\nthread panicked\n";
    let result = filter_cargo_test(output);
    assert!(result.starts_with("FAILURES (1):"), "got: {result}");
    assert!(result.contains("---- a stdout ----"), "got: {result}");
    assert!(result.contains("thread panicked"), "got: {result}");
}

#[test]
fn test_filter_cargo_test_caps_failures() {
    let total = CAP_WARNINGS + 1;
    let mut output = String::from("failures:\n\n");
    for i in 0..total {
        output.push_str(&format!("---- t{i} stdout ----\npanicked {i}\n\n"));
    }
    output.push_str(&format!(
        "test result: FAILED. 0 passed; {total} failed; 0 ignored; 0 measured; 0 filtered out\n"
    ));
    let result = with_isolated_root(|| filter_cargo_test(&output));
    assert!(
        result.starts_with(&format!("FAILURES ({total}):")),
        "got: {result}"
    );
    assert!(result.contains("… +1 more failures"), "got: {result}");
    assert!(result.contains("test result: FAILED."), "got: {result}");
}

#[test]
fn test_filter_cargo_test_caps_trailing_warnings_and_falls_back_to_tail() {
    let total = CAP_WARNINGS + 2;
    let mut output = String::new();
    for i in 0..total {
        output.push_str(&format!(
            "warning: unused variable `v{i}`\n --> src/lib.rs:{}:1\n",
            i + 1
        ));
    }
    let result = filter_cargo_test(&output);
    assert!(
        result.starts_with("warning: unused variable `v0`"),
        "got: {result}"
    );
    assert!(result.contains("… +2 more warnings"), "got: {result}");
    // The tail fallback repeats the last lines of the raw output.
    let last = format!("warning: unused variable `v{}`", total - 1);
    assert!(result.contains(&last), "got: {result}");
}

#[test]
fn test_filter_cargo_test_bare_could_not_compile_shows_tail() {
    let output =
        "   Compiling foo v0.1.0\nerror: could not compile `foo` (lib) due to 1 previous error\n";
    assert_eq!(
        filter_cargo_test(output),
        "error: could not compile `foo` (lib) due to 1 previous error"
    );
}

#[test]
fn test_cap_error_block_with_early_help() {
    let fix: Vec<String> = (0..20).map(|n| format!("   ~ fix {n}")).collect();
    let mut block = vec!["error: x", "  --> src/lib.rs:1:1", "help: try"];
    block.extend(fix.iter().map(String::as_str));
    let capped = cap_error_block(&block);
    assert_eq!(capped.len(), 14);
    assert_eq!(
        capped[..3],
        ["error: x", "  --> src/lib.rs:1:1", "help: try"]
    );
    assert_eq!(capped[12], "   ~ fix 9");
    assert_eq!(capped[13], "… +10 more lines");
}

#[test]
fn test_filter_cargo_clippy_flushes_block_on_progress_line() {
    let output =
        " --> src/orphan.rs:1:1\nerror: bad thing\n --> src/a.rs:1:1\n    Checking demo v0.1.0\n";
    let result = filter_cargo_clippy(output);
    assert!(
        result.starts_with("cargo clippy: 1 errors, 0 warnings"),
        "got: {result}"
    );
    assert!(
        result.contains("Errors:\n  error: bad thing\n   --> src/a.rs:1:1"),
        "got: {result}"
    );
    assert!(result.contains("bad thing (1x)"), "got: {result}");
    assert!(!result.contains("orphan"), "got: {result}");
}

#[test]
fn test_filter_cargo_clippy_caps_error_blocks() {
    let total = CAP_WARNINGS + 1;
    let mut output = String::new();
    for i in 0..total {
        output.push_str(&format!(
            "error[E0308]: e{i}\n --> src/l.rs:{}:1\n\n",
            i + 1
        ));
    }
    let result = with_isolated_root(|| filter_cargo_clippy(&output));
    assert!(
        result.starts_with(&format!("cargo clippy: {total} errors, 0 warnings")),
        "got: {result}"
    );
    assert!(result.contains("… +1 more errors"), "got: {result}");
    assert!(
        result.contains(&format!("E0308 ({total}x)")),
        "got: {result}"
    );
}

#[cfg(unix)]
const FAKE_CARGO: &str = r#"#!/bin/sh
case "$1" in
check)
  echo '   Compiling demo v0.1.0'
  echo '{"reason":"compiler-message","message":{"level":"error","message":"mismatched types","rendered":"error[E0308]: mismatched types"}}'
  echo '{"reason":"build-finished","success":false}'
  exit 101
  ;;
install)
  echo '  Installing demo v1.0.0'
  echo '   Compiling dep_a v0.1.0'
  echo '   Compiling dep_b v0.1.0'
  echo '   Compiling demo v1.0.0'
  echo '    Finished `release` profile [optimized] target(s) in 1.00s'
  echo '  Installed package `demo v1.0.0` (executable `demo`)'
  exit 0
  ;;
nextest)
  echo '    Starting 3 tests across 1 binary'
  echo '        PASS [   0.001s] (1/3) demo tests::a'
  echo '        PASS [   0.001s] (2/3) demo tests::b'
  echo '        PASS [   0.001s] (3/3) demo tests::c'
  echo '     Summary [   0.010s] 3 tests run: 3 passed, 0 skipped'
  exit 0
  ;;
esac
exit 0
"#;

#[cfg(unix)]
fn rtk_with_fake_cargo(dir: &std::path::Path) -> std::process::Command {
    use std::os::unix::fs::PermissionsExt;
    let script = dir.join("cargo");
    std::fs::write(&script, FAKE_CARGO).expect("write fake cargo");
    let mut perms = std::fs::metadata(&script).expect("stat").permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script, perms).expect("chmod");

    let mut cmd = crate::core::test_isolation::rtk_command();
    let inherited = cmd
        .get_envs()
        .find(|(k, _)| k.to_str() == Some("PATH"))
        .and_then(|(_, v)| v.map(|v| v.to_os_string()));
    let mut path = dir.as_os_str().to_os_string();
    if let Some(old) = inherited {
        path.push(":");
        path.push(old);
    }
    cmd.env("PATH", path);
    cmd
}

#[cfg(unix)]
fn combined_output(out: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[cfg(unix)]
#[test]
fn test_rtk_cargo_runners_filter_fake_cargo_output() {
    if !crate::core::test_isolation::rtk_binary_is_built() {
        return;
    }
    let tools = tempfile::tempdir().expect("tool dir");

    let out = rtk_with_fake_cargo(tools.path())
        .args(["cargo", "check", "--message-format=json"])
        .output()
        .expect("run rtk cargo check");
    let text = combined_output(&out);
    assert!(text.contains("cargo check: 1 errors"), "got: {text}");
    assert!(!out.status.success(), "got: {text}");

    let out = rtk_with_fake_cargo(tools.path())
        .args(["cargo", "install", "demo"])
        .output()
        .expect("run rtk cargo install");
    let text = combined_output(&out);
    assert!(text.contains("3 deps compiled"), "got: {text}");
    assert!(out.status.success(), "got: {text}");

    let out = rtk_with_fake_cargo(tools.path())
        .args(["cargo", "nextest", "run"])
        .output()
        .expect("run rtk cargo nextest");
    let text = combined_output(&out);
    assert!(
        text.contains("cargo nextest: 3 passed (1 binary, 0.010s)"),
        "got: {text}"
    );
    assert!(out.status.success(), "got: {text}");
}
