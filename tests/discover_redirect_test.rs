//! `rtk discover` counts only commands the hook could have rewritten as
//! missed savings. A command writing to a file, such as `git log > log.txt`,
//! is one the rewriter turns down with any configuration, so it is not one.

mod common;

/// One Claude Code session: `git log > log.txt`, then `git log -n 5`.
const SESSION: &str = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"git log > log.txt"}}]}}
{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":""}]}}
{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t2","name":"Bash","input":{"command":"git log -n 5"}}]}}
{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t2","content":"commit abc\nAuthor: x\n\n    msg\n"}]}}
"#;

#[test]
fn a_command_writing_to_a_file_is_not_a_missed_saving() {
    let root = common::tempdir();
    let claude = root.path().join(".claude");
    let project = claude.join("projects").join("-demo");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("session.jsonl"), SESSION).unwrap();

    let out = common::rtk_command_in(root.path())
        // Named outright: on Windows `dirs::home_dir` asks the shell API, not
        // the variables the isolated child is given.
        .env("CLAUDE_CONFIG_DIR", &claude)
        .args(["discover", "--all", "--format", "json"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();

    assert_eq!(report["total_commands"], 2, "{report}");
    let missed: u64 = report["supported"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["count"].as_u64().unwrap())
        .sum();
    assert_eq!(
        missed, 1,
        "only `git log -n 5` is a missed saving: {report}"
    );
}
