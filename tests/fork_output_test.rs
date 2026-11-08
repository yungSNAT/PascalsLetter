// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: Apache-2.0

//! Output PascalsLetter keeps or counts where upstream RTK did not. Each case
//! goes through the binary, so `scripts/fork-proof.sh` can run it against
//! upstream RTK too.

use std::io::Write;
use std::process::Stdio;

mod common;

#[test]
fn cargo_test_keeps_compiler_warnings_on_a_passing_run() {
    let run = "   Compiling demo v0.1.0
warning: function `unused_helper` is never used
 --> src/lib.rs:9:4
  |
9 | fn unused_helper() -> i32 { 42 }
  |    ^^^^^^^^^^^^^
  |
  = note: `#[warn(dead_code)]` (part of `#[warn(unused)]`) on by default

warning: `demo` (lib) generated 1 warning
    Finished test [unoptimized + debuginfo] target(s) in 0.05s
     Running unittests src/lib.rs (target/debug/deps/demo-abc123)

running 3 tests
test tests::a ... ok
test tests::b ... ok
test tests::c ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
";
    let mut child = common::rtk_command()
        .args(["pipe", "-f", "cargo-test"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("run rtk pipe");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(run.as_bytes())
        .expect("write cargo output");
    let out = child.wait_with_output().expect("rtk pipe output");
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(stdout.contains("unused_helper"), "{stdout}");
    assert!(stdout.contains("3 passed"), "{stdout}");
}

/// `rtk gain` with one row past the 7,500-token cap and one that rtk made worse.
#[test]
fn gain_counts_an_oversized_row_at_the_cap_and_nets_a_worse_one() {
    let dir = common::tempdir();
    let db = dir.path().join("history.db");
    let gain = || {
        let out = common::rtk_command()
            .args(["gain", "--format", "json"])
            .env("RTK_DB_PATH", &db)
            .output()
            .expect("run rtk gain");
        assert!(out.status.success(), "{out:?}");
        let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("gain JSON");
        json["summary"].clone()
    };
    // rtk creates its own schema; the rows go in directly.
    gain();
    let conn = rusqlite::Connection::open(&db).expect("open the tracker");
    for (input, output) in [(1_000_000, 100), (100, 150)] {
        conn.execute(
            "INSERT INTO commands (timestamp, original_cmd, rtk_cmd, input_tokens,
                 output_tokens, saved_tokens, savings_pct, exec_time_ms, project_path)
             VALUES (datetime('now'), 'x', 'rtk x', ?1, ?2, ?1 - ?2, 0.0, 5, '')",
            rusqlite::params![input, output],
        )
        .expect("insert a row");
    }

    let summary = gain();
    assert_eq!(summary["total_commands"], 2, "{summary}");
    assert_eq!(summary["total_input"], 7_500 + 100, "{summary}");
    assert_eq!(summary["total_saved"], (7_500 - 100) - 50, "{summary}");
}

/// A shell script named `name` in `dir`, which a test puts on the child's `PATH`.
#[cfg(unix)]
fn fake_tool(dir: &std::path::Path, name: &str, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write the fake tool");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("make the fake tool executable");
}

#[cfg(unix)]
#[test]
fn a_progress_redraw_reaches_the_filter_as_its_last_state() {
    let bin = common::tempdir();
    fake_tool(
        bin.path(),
        "npm",
        r"printf 'Downloading 37%%\rDownloading 100%%\nbuilt\n'",
    );
    let out = common::rtk_command()
        .args(["npm", "run", "build"])
        .env("PATH", bin.path())
        .output()
        .expect("run rtk npm");
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(stdout.contains("Downloading 100%"), "{stdout:?}");
    assert!(!stdout.contains("37%"), "{stdout:?}");
}

#[cfg(unix)]
#[test]
fn cc_economics_npx_fallback_names_an_exact_ccusage_release() {
    let bin = common::tempdir();
    let calls = bin.path().join("calls");
    // No ccusage on PATH, and an npx that records its arguments and fails.
    fake_tool(
        bin.path(),
        "npx",
        &format!("printf '%s\\n' \"$*\" >> '{}'\nexit 1", calls.display()),
    );
    common::rtk_command()
        .args(["cc-economics", "--weekly", "--format", "json"])
        .env("PATH", bin.path())
        .output()
        .expect("run rtk cc-economics");
    let calls = std::fs::read_to_string(&calls).expect("npx was run");

    let package = calls
        .split_whitespace()
        .find(|word| word.starts_with("ccusage"))
        .unwrap_or_else(|| panic!("npx ran no ccusage: {calls}"));
    let version = package.strip_prefix("ccusage@").unwrap_or_default();
    let parts: Vec<_> = version.split('.').collect();
    assert!(
        parts.len() == 3
            && parts
                .iter()
                .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())),
        "{package} is not an exact release"
    );
    // rtk builds this file in (`include_str!`), so the fork always has it. Upstream
    // has none: there the exact release above is the whole check, so an upstream
    // that pins ccusage its own way passes, and the proof reports the fix.
    let pin = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/analytics/ccusage-pin/package.json");
    if let Ok(pin) = std::fs::read_to_string(pin) {
        let pin: serde_json::Value = serde_json::from_str(&pin).expect("the ccusage pin is JSON");
        assert_eq!(
            version, pin["dependencies"]["ccusage"],
            "{package} is not the pinned release"
        );
    }
}
