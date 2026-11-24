//! CI guard: every `src/cmds/**/*_cmd.rs` file a PR adds or modifies must carry
//! an inline `#[cfg(test)]` module.
//!
//! `rtk-dev check-test-presence [BASE]` (BASE defaults to `origin/develop`) checks
//! the files changed in `BASE...HEAD`; `--self-test` checks the detection itself.

use std::fs;
use std::process::{Command, ExitCode, Stdio};

const MARKER: &[u8] = b"#[cfg(test)]";
const SELF_TEST_FILE: &str = "src/cmds/system/_rtk_check_self_test_cmd.rs";

pub fn run(args: &[String]) -> ExitCode {
    match args.first().map(String::as_str) {
        Some("--self-test") => self_test(),
        Some(base) if !base.is_empty() => check(base),
        _ => check("origin/develop"),
    }
}

/// A file without tests must be caught.
fn self_test() -> ExitCode {
    if let Err(e) = fs::write(SELF_TEST_FILE, "pub fn run() {}\n") {
        eprintln!("rtk-dev check-test-presence: {SELF_TEST_FILE}: {e}");
        return ExitCode::from(1);
    }
    let caught = !has_tests(SELF_TEST_FILE);
    let _ = fs::remove_file(SELF_TEST_FILE);
    if !caught {
        println!("FAIL: self-test broken (false negative)");
        return ExitCode::from(1);
    }
    println!("PASS: --self-test detection works correctly");
    ExitCode::SUCCESS
}

fn check(base: &str) -> ExitCode {
    // A git failure (an unknown base, say) reads as no changes, as in the script.
    let changed: Vec<String> = Command::new("git")
        .args(["diff", "--name-only", "--diff-filter=AM", "--no-renames"])
        .arg(format!("{base}...HEAD"))
        .stderr(Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
        .lines()
        .filter(|path| is_cmd_file(path))
        .map(String::from)
        .collect();

    if changed.is_empty() {
        println!("check-test-presence: no *_cmd.rs changes detected — OK");
        return ExitCode::SUCCESS;
    }
    println!(
        "check-test-presence: checking {} filter module(s)...",
        changed.len()
    );
    println!();

    let mut failed = false;
    // The script read each name with `IFS= read -r`, so names are used as they are.
    for file in &changed {
        if !std::path::Path::new(file).is_file() {
            continue;
        }
        if has_tests(file) {
            println!("  PASS  {file}");
        } else {
            println!("  FAIL  {file}");
            println!("        Missing #[cfg(test)] module.");
            println!("        Every *_cmd.rs filter must include inline unit tests.");
            println!("        Reference: src/cmds/cloud/aws_cmd.rs");
            println!();
            failed = true;
        }
    }
    println!();
    if failed {
        println!("check-test-presence: FAILED — add tests before merging.");
        println!("See .claude/rules/cli-testing.md for the testing guide.");
        ExitCode::from(1)
    } else {
        println!("check-test-presence: all filter modules have tests — OK");
        ExitCode::SUCCESS
    }
}

/// `grep -E 'src/cmds/.+_cmd\.rs$'`
fn is_cmd_file(path: &str) -> bool {
    path.find("src/cmds/")
        .is_some_and(|i| path.len() >= i + "src/cmds/".len() + 1 + "_cmd.rs".len())
        && path.ends_with("_cmd.rs")
}

fn has_tests(path: &str) -> bool {
    fs::read(path).is_ok_and(|bytes| bytes.windows(MARKER.len()).any(|w| w == MARKER))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmd_file_pattern_matches_grep() {
        assert!(is_cmd_file("src/cmds/git/git_cmd.rs"));
        assert!(is_cmd_file("crates/x/src/cmds/a_cmd.rs"));
        assert!(!is_cmd_file("src/cmds/_cmd.rs"));
        assert!(is_cmd_file("src/cmds/x_cmd.rs"));
        assert!(!is_cmd_file("src/cmds/git/git_cmd.rs.bak"));
        assert!(!is_cmd_file("src/core/runner.rs"));
    }
}
