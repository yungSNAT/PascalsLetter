use super::*;
use serde_json::json;

#[test]
fn format_issue_list_caps_long_lists_and_reports_overflow() {
    let total = CAP_LIST + 3;
    let issues: Vec<Value> = (1..=total)
        .map(|n| {
            json!({
                "number": n,
                "title": format!("t{}x", n),
                "state": if n % 2 == 1 { "OPEN" } else { "CLOSED" },
            })
        })
        .collect();
    let input = Value::Array(issues);
    let dir = crate::core::test_isolation::tempdir();
    crate::core::test_isolation::with_root(dir.path(), || {
        let out = format_issue_list(&input);
        assert!(out.starts_with("Issues\n"), "{out}");
        assert!(out.contains("  [open] #1 t1x\n"), "{out}");
        assert!(out.contains("  [closed] #2 t2x\n"), "{out}");
        let shown = format!("t{}x", CAP_LIST);
        assert!(out.contains(&shown), "{out}");
        let hidden = format!("t{}x", total);
        assert!(!out.contains(&hidden), "{out}");
        assert!(out.contains("  … +3 more\n"), "{out}");
    });
    assert_eq!(format_issue_list(&json!([])), "No Issues\n");
    assert_eq!(format_issue_list(&json!({"a": 1})), "");
}

#[test]
fn format_issue_view_closed_with_description() {
    let input = json!({
        "number": 7,
        "title": "Crash on start",
        "state": "CLOSED",
        "author": {"login": "octo"},
        "url": "https://github.com/o/r/issues/7",
        "body": "<!-- template -->\nSteps:\n1. run it"
    });
    let out = format_issue_view(&input);
    assert!(
        out.starts_with("[closed] Issue #7: Crash on start\n"),
        "{out}"
    );
    assert!(out.contains("  Author: @octo\n"), "{out}");
    assert!(out.contains("  Status: CLOSED\n"), "{out}");
    assert!(
        out.contains("  URL: https://github.com/o/r/issues/7\n"),
        "{out}"
    );
    assert!(out.contains("\n  Description:\n"), "{out}");
    assert!(out.contains("    Steps:\n"), "{out}");
    assert!(out.contains("    1. run it\n"), "{out}");
    assert!(!out.contains("template"), "{out}");
}

#[test]
fn format_run_list_maps_conclusions_to_icons() {
    let input = json!([
        {"databaseId": 1, "name": "build", "status": "completed", "conclusion": "success"},
        {"databaseId": 2, "name": "test", "status": "completed", "conclusion": "failure"},
        {"databaseId": 3, "name": "lint", "status": "completed", "conclusion": "cancelled"},
        {"databaseId": 4, "name": "deploy", "status": "in_progress", "conclusion": ""},
        {"databaseId": 5, "name": "nightly", "status": "queued"}
    ]);
    let out = format_run_list(&input);
    assert_eq!(
        out,
        "Workflow Runs\n  [ok] build [1]\n  [FAIL] test [2]\n  [X] lint [3]\n  [time] deploy [4]\n  [pending] nightly [5]\n"
    );
    assert_eq!(format_run_list(&json!({"x": 1})), "");
    assert_eq!(format_run_list(&json!([])), "Workflow Runs\n");
}

#[test]
fn format_run_view_keeps_status_and_failed_jobs() {
    let stdout = "✓ main CI · 123\nTriggered via push\nStatus: completed\nConclusion: failure\n\nJOBS\n✓ build in 1m\nbuild success line\nX test in 2m (fail)\n[x] lint broke\nsomething neutral\n";
    let out = format_run_view(stdout, "123");
    assert_eq!(
        out,
        "Workflow Run #123\n  Status: completed\n  Conclusion: failure\n  [FAIL] X test in 2m (fail)\n  [FAIL] [x] lint broke\n"
    );
}

#[test]
fn format_repo_view_public_and_private() {
    let public = json!({
        "name": "r",
        "owner": {"login": "o"},
        "description": "Fixture repository",
        "url": "https://github.com/o/r",
        "stargazerCount": 42,
        "forkCount": 5,
        "isPrivate": false
    });
    assert_eq!(
        format_repo_view(&public),
        "o/r\n  [public]\n  Fixture repository\n  42 stars | 5 forks\n  https://github.com/o/r\n"
    );
    let private = json!({"name": "secret", "owner": {"login": "me"}, "isPrivate": true});
    // A missing URL prints an empty indented line today; only the lines
    // before it are pinned.
    let out = format_repo_view(&private);
    assert!(
        out.starts_with("me/secret\n  [private]\n  0 stars | 0 forks\n"),
        "{out}"
    );
}
