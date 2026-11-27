use super::*;
use crate::core::test_isolation;

#[test]
fn shorten_path_keeps_short_paths_and_elides_long_ones() {
    // Four components or fewer come back untouched.
    assert_eq!(shorten_path("a/b/c/d"), "a/b/c/d");
    // Longer relative paths keep the first component and the last two.
    assert_eq!(shorten_path("a/b/c/d/e/f"), "a/.../e/f");
    assert_eq!(
        shorten_path("first/x/y/z/second/last"),
        "first/.../second/last"
    );

    #[cfg(unix)]
    {
        assert_eq!(shorten_path("/a/b"), "/a/b");
        assert_eq!(shorten_path("/home/user/projects/rtk/src"), "/.../rtk/src");
    }
}

#[test]
fn resolve_project_scope_uses_working_dir_only_when_requested() {
    assert_eq!(resolve_project_scope(false).expect("no scope"), None);

    let dir = test_isolation::tempdir();
    let _guard = test_isolation::enter(dir.path());
    let scope = resolve_project_scope(true)
        .expect("scope resolves")
        .expect("scope is set");
    let expected = dir
        .path()
        .canonicalize()
        .expect("the temporary directory canonicalises");
    assert_eq!(PathBuf::from(scope), expected);
}

#[test]
fn bypass_check_flags_sessions_that_overuse_rtk_disabled() {
    let dir = test_isolation::tempdir();
    let claude_dir = dir.path().join(".claude");
    let project_dir = claude_dir.join("projects").join("-tmp-proj");
    std::fs::create_dir_all(&project_dir).expect("mkdir");

    let tool_use = |id: &str, cmd: &str| {
        serde_json::json!({
            "type": "assistant",
            "timestamp": "2024-01-15T10:00:00Z",
            "message": {
                "role": "assistant",
                "content": [{
                    "type": "tool_use",
                    "id": id,
                    "name": "Bash",
                    "input": { "command": cmd }
                }]
            }
        })
        .to_string()
    };
    let tool_result = |id: &str| {
        serde_json::json!({
            "type": "user",
            "timestamp": "2024-01-15T10:00:01Z",
            "message": {
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": id,
                    "content": "ok"
                }]
            }
        })
        .to_string()
    };
    let lines = [
        tool_use("toolu_1", "RTK_DISABLED=1 git status"),
        tool_result("toolu_1"),
        tool_use("toolu_2", "RTK_DISABLED=1 git log"),
        tool_result("toolu_2"),
        tool_use("toolu_3", "ls -la"),
        tool_result("toolu_3"),
    ];
    std::fs::write(project_dir.join("session.jsonl"), lines.join("\n") + "\n")
        .expect("write session");

    let cfg = claude_dir.to_string_lossy().to_string();
    let warning =
        crate::core::user_env::with_vars(&[("CLAUDE_CONFIG_DIR", Some(cfg.as_str()))], || {
            test_isolation::with_root(dir.path(), check_rtk_disabled_bypass)
        });
    let warning = warning.expect("bypass rate above 10% produces a warning");
    assert!(warning.contains("2 commands"), "{warning}");
    assert!(warning.contains("67%"), "{warning}");
    assert!(warning.contains("RTK_DISABLED=1"), "{warning}");
}
