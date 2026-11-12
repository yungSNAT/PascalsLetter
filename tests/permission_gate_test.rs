//! The permission gate reads a command as the shell does before an allow rule
//! may cover it: a quote inside a `#` comment does not hide the next line, and
//! zsh's `&!` disown operator does not hide the command after it. Each case goes
//! through the binary, so `scripts/fork-proof.sh` can run it against upstream RTK.

mod common;

/// `rtk rewrite`'s exit code for `command`, with only the given project rules.
fn rewrite_exit(command: &str, deny: &[&str], allow: &[&str]) -> Option<i32> {
    let root = common::tempdir();
    let home = root.path().join("home");
    let project = root.path().join("project");
    std::fs::create_dir_all(home.join(".config")).expect("mkdir home");
    std::fs::create_dir_all(root.path().join("claude-home")).expect("mkdir claude home");
    std::fs::create_dir_all(project.join(".claude")).expect("mkdir project");
    let rules = |rules: &[&str]| {
        rules
            .iter()
            .map(|r| format!("\"Bash({r})\""))
            .collect::<Vec<_>>()
            .join(", ")
    };
    std::fs::write(
        project.join(".claude/settings.json"),
        format!(
            r#"{{"permissions": {{"deny": [{}], "allow": [{}]}}}}"#,
            rules(deny),
            rules(allow)
        ),
    )
    .expect("write settings");
    common::rtk_command()
        .args(["rewrite", command])
        .current_dir(&project)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("CLAUDE_CONFIG_DIR", root.path().join("claude-home"))
        .env("RTK_DB_PATH", project.join("rtk.db"))
        .env("LC_ALL", "C")
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
