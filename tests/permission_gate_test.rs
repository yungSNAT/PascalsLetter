//! The permission gate reads a command as the shell does before an allow rule
//! may cover it: a quote inside a `#` comment does not hide the next line, and
//! zsh's `&!` disown operator does not hide the command after it. Each case goes
//! through the binary, so `scripts/fork-proof.sh` can run it against upstream RTK.

mod common;

/// `rtk rewrite`'s exit code for `command`, with only the given project rules.
fn rewrite_exit(command: &str, deny: &[&str], allow: &[&str]) -> Option<i32> {
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
            r#"{{"permissions": {{"deny": [{}], "allow": [{}]}}}}"#,
            rules(deny),
            rules(allow)
        ),
    )
    .expect("write settings");
    // Windows finds the home through the shell API, so name Claude's own
    // directory too, or the developer's global rules would apply.
    cmd.args(["rewrite", command])
        .env("CLAUDE_CONFIG_DIR", root.path().join("claude-home"))
        .output()
        .expect("spawn rtk rewrite")
        .status
        .code()
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
