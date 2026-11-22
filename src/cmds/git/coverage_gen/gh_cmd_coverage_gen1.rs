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
        "name": "rtk",
        "owner": {"login": "rtk-ai"},
        "description": "Token killer",
        "url": "https://github.com/rtk-ai/rtk",
        "stargazerCount": 42,
        "forkCount": 5,
        "isPrivate": false
    });
    assert_eq!(
        format_repo_view(&public),
        "rtk-ai/rtk\n  [public]\n  Token killer\n  42 stars | 5 forks\n  https://github.com/rtk-ai/rtk\n"
    );
    let private = json!({"name": "secret", "owner": {"login": "me"}, "isPrivate": true});
    assert_eq!(
        format_repo_view(&private),
        "me/secret\n  [private]\n  0 stars | 0 forks\n  \n"
    );
}

#[cfg(unix)]
fn run_rtk_gh(path_value: &str, args: &[&str]) -> std::process::Output {
    let mut cmd = crate::core::test_isolation::rtk_command();
    cmd.env("PATH", path_value).arg("gh").args(args);
    cmd.output().expect("run rtk")
}

#[cfg(unix)]
#[test]
fn rtk_gh_subcommands_filter_fake_gh_output() {
    use std::os::unix::fs::PermissionsExt;

    if !crate::core::test_isolation::rtk_binary_is_built() {
        return;
    }
    let dir = crate::core::test_isolation::tempdir();
    let script = r#"#!/bin/sh
case "$1 $2" in
  "issue view") printf '%s\n' '{"number":42,"title":"Bug here","state":"OPEN","author":{"login":"octo"},"body":"Details","url":"https://x/42"}' ;;
  "run list") printf '%s\n' '[{"databaseId":7,"name":"CI","status":"completed","conclusion":"success"}]' ;;
  "run view") printf 'Status: completed\nJOBS\nX test failed\n' ;;
  "repo view") printf '%s\n' '{"name":"r","owner":{"login":"o"},"description":"d","url":"u","stargazerCount":3,"forkCount":1,"isPrivate":false}' ;;
  "pr create") printf 'https://github.com/o/r/pull/9\n' ;;
  *) exit 3 ;;
esac
"#;
    let gh = dir.path().join("gh");
    std::fs::write(&gh, script).expect("write fake gh");
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).expect("chmod");

    let base = crate::core::test_isolation::rtk_command();
    let inherited = base
        .get_envs()
        .find(|(k, _)| *k == "PATH")
        .and_then(|(_, v)| v)
        .map(|v| v.to_string_lossy().into_owned())
        .unwrap_or_default();
    let path_value = format!("{}:{}", dir.path().display(), inherited);

    let out = run_rtk_gh(&path_value, &["issue", "view", "42"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains("[open] Issue #42: Bug here"), "{stdout}");
    assert!(stdout.contains("Details"), "{stdout}");

    let out = run_rtk_gh(&path_value, &["run", "list"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains("[ok] CI [7]"), "{stdout}");

    let out = run_rtk_gh(&path_value, &["run", "view", "7"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains("Workflow Run #7"), "{stdout}");
    assert!(stdout.contains("[FAIL] X test failed"), "{stdout}");

    let out = run_rtk_gh(&path_value, &["repo", "view"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains("o/r"), "{stdout}");
    assert!(stdout.contains("3 stars | 1 forks"), "{stdout}");

    let out = run_rtk_gh(&path_value, &["pr", "create", "--fill"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains("ok created"), "{stdout}");
    assert!(stdout.contains("#9"), "{stdout}");
}
