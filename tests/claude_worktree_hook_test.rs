// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: Apache-2.0

//! Claude Code's worktree guard refuses `rtk` with a git-named word among its
//! operands (rtk-ai/rtk#4591), so in a managed worktree the hook leaves such a
//! command as written. Each case goes through the binary, so
//! `scripts/fork-proof.sh` can run it against upstream RTK too.

use std::io::Write;
use std::process::{Command, Stdio};

mod common;

/// The command `rtk hook claude` substitutes for `command` run in `cwd`, or
/// `None` when it leaves the command alone.
fn hook(command: &str, cwd: &std::path::Path) -> Option<String> {
    hook_with(common::rtk_command(), command, cwd)
}

/// [`hook`], run by `rtk`.
fn hook_with(mut rtk: Command, command: &str, cwd: &std::path::Path) -> Option<String> {
    let payload = serde_json::json!({
        "tool_name": "Bash",
        "tool_input": { "command": command },
        "cwd": cwd,
    })
    .to_string();
    let mut child = rtk
        .args(["hook", "claude"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn rtk hook claude");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(payload.as_bytes())
        .expect("write payload");
    let out = child.wait_with_output().expect("wait rtk");
    assert_eq!(
        out.status.code(),
        Some(0),
        "rtk hook claude failed for {command:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    if stdout.trim().is_empty() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("hook emitted valid JSON");
    v.pointer("/hookSpecificOutput/updatedInput/command")
        .and_then(|c| c.as_str())
        .map(str::to_owned)
}

#[test]
fn claude_worktree_leaves_a_git_operand_unrewritten() {
    let repo = tempfile::TempDir::new().expect("tempdir");
    let worktree = repo.path().join(".claude/worktrees/feat-x");
    std::fs::create_dir_all(&worktree).expect("mkdir worktree");
    for command in ["grep -n git README.md", r#"grep -c "git push" README.md"#] {
        // Outside a worktree first, so a broken hook fails here, not below.
        assert_eq!(
            hook(command, repo.path()).as_deref(),
            Some(format!("rtk {command}").as_str()),
            "{command} outside a worktree"
        );
        assert_eq!(hook(command, &worktree), None, "{command}");
    }
}

/// A `mode = "swap"` route rule runs the command after `rtk` (`rtk <task> ...
/// -- sh -c 'git status'`), which the guard refuses as well.
#[cfg(feature = "engine")]
#[test]
fn claude_worktree_keeps_git_out_of_a_pipeline_swap() {
    const RULE: &str = r#"[upstream.haiku]
provider = "anthropic"
model = "claude-haiku-4-5"

[task.check-build]
kind = "digest"
upstream = "haiku"

[[hook.before_tool.shell]]
name = "swap-git"
when = { command_matches = "git status" }
action = "route"
task = "check-build"
mode = "swap"
"#;
    let dir = common::tempdir();
    let config = dir.path().join("config");
    std::fs::create_dir_all(&config).expect("mkdir config");
    std::fs::write(config.join("pipeline.toml"), RULE).expect("write pipeline");
    let repo = dir.path().join("repo");
    let worktree = repo.join(".claude/worktrees/feat-x");
    std::fs::create_dir_all(&worktree).expect("mkdir worktree");
    let rtk = || {
        let mut rtk = common::rtk_command();
        rtk.env("RTK_CONFIG_DIR", &config);
        rtk
    };

    // Outside a worktree first, so a rule that never fires fails here.
    let outside = hook_with(rtk(), "git status", &repo);
    assert!(
        outside
            .as_deref()
            .is_some_and(|c| c.starts_with("rtk check-build ")),
        "control: the swap rule must fire outside a worktree: {outside:?}"
    );
    assert_eq!(hook_with(rtk(), "git status", &worktree), None);
}
