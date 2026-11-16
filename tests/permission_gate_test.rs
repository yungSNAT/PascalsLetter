//! The permission gate reads a command as the shell does before an allow rule
//! may cover it, and reads the rules as Claude Code does. Each case goes through
//! the binary (`rtk rewrite`, or `rtk hook claude` as Claude Code calls it), so
//! `scripts/fork-proof.sh` can run it against upstream RTK.

use std::io::Write;
use std::process::Stdio;

mod common;

/// `rtk` started in a project whose `.claude/settings.json` holds only the given
/// Bash rules, with a Claude home of its own. Keep the `TempDir` alive.
fn with_rules(
    deny: &[&str],
    ask: &[&str],
    allow: &[&str],
) -> (tempfile::TempDir, std::process::Command) {
    let root = common::tempdir();
    let project = root.path().join("project");
    let rules = |rules: &[&str]| {
        rules
            .iter()
            .map(|r| format!("\"Bash({r})\""))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut cmd = common::rtk_command_in(root.path());
    std::fs::create_dir_all(project.join(".claude")).expect("mkdir project");
    std::fs::create_dir_all(root.path().join("claude-home")).expect("mkdir claude home");
    std::fs::write(
        project.join(".claude/settings.json"),
        format!(
            r#"{{"permissions": {{"deny": [{}], "ask": [{}], "allow": [{}]}}}}"#,
            rules(deny),
            rules(ask),
            rules(allow)
        ),
    )
    .expect("write settings");
    // Windows finds the home through the shell API, so name Claude's own
    // directory too, or the developer's global rules would apply.
    cmd.env("CLAUDE_CONFIG_DIR", root.path().join("claude-home"));
    (root, cmd)
}

/// `rtk rewrite`'s exit code for `command`, with only the given project rules.
fn rewrite_exit(command: &str, deny: &[&str], allow: &[&str]) -> Option<i32> {
    let (_root, mut cmd) = with_rules(deny, &[], allow);
    cmd.args(["rewrite", command])
        .output()
        .expect("spawn rtk rewrite")
        .status
        .code()
}

/// What Claude Code reads from `rtk hook claude` for `command`, with only the
/// given project rules: its `permissionDecision` (`allow` approves the command
/// without asking), `prompt` for a rewrite left to Claude Code's own prompt,
/// or `native` when the hook leaves the command to Claude Code untouched.
fn hook_decision(command: &str, deny: &[&str], ask: &[&str], allow: &[&str]) -> String {
    let (root, mut cmd) = with_rules(deny, ask, allow);
    let payload = serde_json::json!({
        "tool_name": "Bash",
        "tool_input": { "command": command },
        "cwd": root.path().join("project"),
    })
    .to_string();
    let mut child = cmd
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
        return "native".into();
    }
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("hook emitted valid JSON");
    v.pointer("/hookSpecificOutput/permissionDecision")
        .and_then(|d| d.as_str())
        .unwrap_or("prompt")
        .to_string()
}

#[test]
fn deny_rules_see_commands_after_a_comment_quote_and_a_zsh_disown() {
    for command in ["ls -la # it's fine\nrm -rf x", "ls &! rm -rf x"] {
        // Exit 2: a deny rule matched, so the host's own deny applies.
        assert_eq!(
            rewrite_exit(command, &["rm:*"], &["ls:*"]),
            Some(2),
            "{command:?}"
        );
    }
}

#[test]
fn a_deny_rule_sees_a_command_its_prefix_reaches_over_a_continued_line() {
    assert_eq!(
        rewrite_exit("env \\\n  FOO=1 ls -la", &["ls -la:*"], &["*"]),
        Some(2)
    );
}

const DENY_FORCE: &[&str] = &["git push * --force"];
const ALLOW_PUSH: &[&str] = &["git push *"];

/// The allow rule approves a push the deny rule does not name, so a test that
/// sees no approval for a forced push sees the deny rule, not a broken hook.
fn assert_a_plain_push_is_approved() {
    assert_eq!(
        hook_decision("git push origin main", DENY_FORCE, &[], ALLOW_PUSH),
        "allow"
    );
}

#[test]
fn a_deny_rule_sees_a_word_made_by_brace_expansion() {
    assert_a_plain_push_is_approved();
    // bash and zsh run `git push origin --force --force`.
    let decision = hook_decision("git push origin --force{,}", DENY_FORCE, &[], ALLOW_PUSH);
    assert_ne!(decision, "allow");
}

#[test]
fn an_ask_rule_sees_a_word_made_by_brace_expansion() {
    assert_a_plain_push_is_approved();
    let decision = hook_decision("git push origin --force{,}", &[], DENY_FORCE, ALLOW_PUSH);
    assert_ne!(decision, "allow");
}

#[test]
fn a_deny_literal_after_a_wildcard_matches_before_a_comment() {
    assert_a_plain_push_is_approved();
    let decision = hook_decision(
        "git push origin --force # done",
        DENY_FORCE,
        &[],
        ALLOW_PUSH,
    );
    assert_ne!(decision, "allow");
}

#[test]
fn a_deny_literal_after_a_wildcard_matches_when_quoted() {
    assert_a_plain_push_is_approved();
    for command in [
        "git push origin '--force'",
        "git push origin \"--force\"",
        "git push origin $'--force'",
    ] {
        let decision = hook_decision(command, DENY_FORCE, &[], ALLOW_PUSH);
        assert_ne!(decision, "allow", "{command:?}");
    }
}

#[test]
fn a_zsh_reading_of_a_dollar_quote_asks() {
    assert_a_plain_push_is_approved();
    // zsh reads `$'\u'` with no digits as NUL, so it passes `--force`.
    let decision = hook_decision("git push origin --force$'\\u'", DENY_FORCE, &[], ALLOW_PUSH);
    assert_ne!(decision, "allow");
}

#[test]
fn an_allow_rule_without_a_wildcard_covers_only_the_whole_command() {
    let allow = ["npm run build"];
    assert_eq!(hook_decision("npm run build", &[], &[], &allow), "allow");
    assert_ne!(
        hook_decision("npm run build --watch", &[], &[], &allow),
        "allow"
    );
}

#[test]
fn a_middle_wildcard_in_an_allow_rule_needs_text_after_it() {
    let allow = ["git -C * diff:*"];
    assert_eq!(
        hook_decision("git -C /p diff --stat", &[], &[], &allow),
        "allow"
    );
    // Claude Code asks about the bare form, so the hook must not approve it.
    assert_ne!(hook_decision("git -C /p diff", &[], &[], &allow), "allow");
}

#[test]
fn quotes_inside_a_double_quoted_expansion_are_not_approved() {
    let allow = ["*"];
    assert_eq!(hook_decision("ls -la", &[], &[], &allow), "allow");
    for command in ["ls \"${x:-\"a\"}\"", "ls \"${x#'a'}\""] {
        assert_ne!(
            hook_decision(command, &[], &[], &allow),
            "allow",
            "{command:?}"
        );
    }
}

#[test]
fn a_continued_line_inside_a_shell_wrapper_is_joined_before_the_deny_check() {
    // The inner bash runs `git push`; read as `git pu sh`, no deny rule matched.
    assert_eq!(
        hook_decision("bash -c 'git pu\\\nsh'", &["git push:*"], &[], &["*"]),
        "native"
    );
}
