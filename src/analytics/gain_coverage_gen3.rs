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

    // The graph renderer shares the summary view; exercise its branches
    // (empty input, long and short date labels, a zero maximum).
    print_ascii_graph(&[]);
    print_ascii_graph(&[
        ("2024-01-15".to_string(), 100),
        ("short".to_string(), 50),
        ("2024-01-16".to_string(), 0),
    ]);
    print_ascii_graph(&[("2024-02-01".to_string(), 0)]);
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
        .unwrap_or_else(|_| dir.path().to_path_buf());
    assert_eq!(PathBuf::from(scope), expected);
}

#[test]
fn exports_succeed_on_an_empty_tracker() {
    let tracker = Tracker::new_in_memory().expect("tracker");

    assert!(export_json(&tracker, false, false, false, true, None).is_ok());
    assert!(export_json(&tracker, true, true, true, false, Some("nowhere")).is_ok());
    assert!(export_csv(&tracker, false, false, false, true, None).is_ok());
    assert!(export_csv(&tracker, true, true, true, false, Some("nowhere")).is_ok());

    let days = tracker.get_all_days_filtered(None).expect("days");
    assert!(days.is_empty());
}

#[test]
fn recall_stats_report_no_activity_on_a_fresh_root() {
    let dir = test_isolation::tempdir();
    let result = test_isolation::with_root(dir.path(), show_recall_stats);
    assert!(result.is_ok());
    let snapshot = test_isolation::with_root(dir.path(), crate::core::retriever::stats_snapshot)
        .expect("snapshot");
    assert!(snapshot.is_empty());
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
