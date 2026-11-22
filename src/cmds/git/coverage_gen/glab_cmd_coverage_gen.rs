use super::*;

fn mr_items(n: usize) -> Value {
    Value::Array(
        (0..n)
            .map(|i| {
                serde_json::json!({
                    "iid": i as i64 + 1,
                    "title": format!("Change {}", i),
                    "state": "opened",
                    "author": {"username": "dev"}
                })
            })
            .collect(),
    )
}

#[test]
fn markdown_tilde_fence_and_unclosed_fence_are_preserved() {
    let out = filter_markdown_body("Intro\n~~~\ncode <!-- keep -->\n~~~\nAfter <!-- drop -->");
    assert!(out.contains("code <!-- keep -->"));
    assert!(out.contains("Intro"));
    assert!(out.contains("After"));
    assert!(!out.contains("drop"));

    let out = filter_markdown_body("Text <!-- gone -->\n```\nunterminated <!-- x -->");
    assert!(out.contains("unterminated <!-- x -->"));
    assert!(!out.contains("gone"));
}

#[test]
fn icons_cover_every_status() {
    assert_eq!(state_icon("locked"), "?");
    assert_eq!(pipeline_icon("canceled"), "[cancel]");
    assert_eq!(pipeline_icon("cancelled"), "[cancel]");
    assert_eq!(pipeline_icon("pending"), "[pend]");
    assert_eq!(pipeline_icon("skipped"), "[skip]");
    assert_eq!(pipeline_icon("manual"), "?");
}

#[test]
fn takes_value_tables_classify_flags() {
    assert!(inherited_takes_value(TokenKind::Long, "repo").is_some());
    assert!(inherited_takes_value(TokenKind::Long, "group").is_some());
    assert!(inherited_takes_value(TokenKind::Short, "g").is_some());
    assert!(inherited_takes_value(TokenKind::Long, "web").is_none());
    assert!(inherited_takes_value(TokenKind::Positional, "repo").is_none());
    assert!(inherited_takes_value(TokenKind::DashDash, "").is_none());

    assert!(view_takes_value(TokenKind::Long, "output").is_some());
    assert!(view_takes_value(TokenKind::Positional, "page").is_none());

    assert!(mr_merge_takes_value(TokenKind::Short, "m").is_some());
    assert!(mr_merge_takes_value(TokenKind::Long, "sha").is_some());
    assert!(mr_merge_takes_value(TokenKind::Positional, "m").is_none());

    assert!(mr_approve_takes_value(TokenKind::Long, "sha").is_some());
    assert!(mr_approve_takes_value(TokenKind::Long, "repo").is_some());
    assert!(mr_approve_takes_value(TokenKind::Positional, "sha").is_none());

    assert!(mr_note_takes_value(TokenKind::Long, "message").is_some());
    assert!(mr_note_takes_value(TokenKind::Positional, "message").is_none());

    assert!(mr_update_takes_value(TokenKind::Short, "u").is_some());
    assert!(mr_update_takes_value(TokenKind::Positional, "title").is_none());

    let args: Vec<String> = vec!["--sha".into(), "abc".into(), "9".into()];
    let (id, extra) = split_identifier(&args, &mr_approve_takes_value);
    assert_eq!(id.as_deref(), Some("9"));
    assert_eq!(extra, vec!["--sha", "abc"]);

    let args: Vec<String> = vec!["--message".into(), "hi".into(), "4".into()];
    let (id, _) = split_identifier(&args, &mr_note_takes_value);
    assert_eq!(id.as_deref(), Some("4"));

    let args: Vec<String> = vec!["--repo".into(), "g/p".into(), "8".into()];
    let (id, _) = split_identifier(&args, &view_takes_value);
    assert_eq!(id.as_deref(), Some("8"));
}

#[test]
fn mr_view_conflict_pipeline_and_description() {
    let json = serde_json::json!({
        "iid": 3,
        "title": "T",
        "state": "locked",
        "author": {"username": "u"},
        "web_url": "https://e/x",
        "merge_status": "cannot_be_merged",
        "source_branch": "s",
        "target_branch": "t",
        "head_pipeline": {"status": "canceled"},
        "description": "## Summary\n<!-- hidden -->\nDetails here"
    });
    let out = format_mr_view(&json);
    assert!(out.starts_with("? MR !3: T\n"));
    assert!(out.contains("locked | [conflict]"));
    assert!(out.contains("Pipeline: [cancel] canceled"));
    assert!(out.contains("\n  ## Summary\n"));
    assert!(out.contains("  Details here\n"));
    assert!(!out.contains("hidden"));

    let json = serde_json::json!({
        "iid": 4,
        "title": "T",
        "state": "opened",
        "merge_status": "can_be_merged",
        "description": "<!-- only a comment -->"
    });
    let out = format_mr_view(&json);
    assert!(!out.contains("comment"));
    assert!(!out.contains("\n\n"));
}

#[test]
fn issue_view_opened_with_description() {
    let json = serde_json::json!({
        "iid": 5,
        "title": "Crash",
        "state": "opened",
        "author": {"username": "bo"},
        "web_url": "https://e/i/5",
        "description": "Steps to reproduce\n\n\n\nRun it"
    });
    let out = format_issue_view(&json);
    assert!(out.starts_with("[open] Issue #5: Crash\n"));
    assert!(out.contains("  Author: @bo\n"));
    assert!(out.contains("\n  Description:\n"));
    assert!(out.contains("    Steps to reproduce\n"));
    assert!(out.contains("    Run it\n"));
}

#[test]
fn ci_list_formats_and_handles_non_array() {
    assert!(format_ci_list(&Value::Object(Default::default())).is_empty());

    let json = serde_json::json!([
        {"id": 1, "status": "pending", "ref": "main"},
        {"id": 2, "status": "skipped", "ref": "dev"},
        {"id": 3, "status": "cancelled", "ref": "x"},
        {"id": 4, "status": "weird", "ref": "y"}
    ]);
    let out = format_ci_list(&json);
    assert!(out.starts_with("Pipelines\n"));
    assert!(out.contains("  [pend] #1 pending (main)\n"));
    assert!(out.contains("  [skip] #2 skipped (dev)\n"));
    assert!(out.contains("  [cancel] #3 cancelled (x)\n"));
    assert!(out.contains("  ? #4 weird (y)\n"));
}

#[test]
fn long_lists_are_capped_with_more_marker() {
    let dir = crate::core::test_isolation::tempdir();

    let mrs = mr_items(CAP_LIST + 5);
    let out = crate::core::test_isolation::with_root(dir.path(), || format_mr_list(&mrs));
    assert!(out.starts_with("Merge Requests\n"));
    assert!(out.contains("  … +5 more\n"));
    assert!(out.contains(&format!("!{} ", CAP_LIST)));
    assert!(!out.contains(&format!("!{} ", CAP_LIST + 1)));

    let issues = Value::Array(
        (0..CAP_LIST + 3)
            .map(|i| serde_json::json!({"iid": i as i64 + 1, "title": "Bug", "state": "closed"}))
            .collect(),
    );
    let out = crate::core::test_isolation::with_root(dir.path(), || format_issue_list(&issues));
    assert!(out.starts_with("Issues\n"));
    assert!(out.contains("  [closed] #1 Bug\n"));
    assert!(out.contains("  … +3 more\n"));

    let pipelines = Value::Array(
        (0..CAP_WARNINGS + 2)
            .map(|i| serde_json::json!({"id": i as i64 + 1, "status": "success", "ref": "main"}))
            .collect(),
    );
    let out = crate::core::test_isolation::with_root(dir.path(), || format_ci_list(&pipelines));
    assert!(out.starts_with("Pipelines\n"));
    assert!(out.contains("  [ok] #1 success (main)\n"));
    assert!(out.contains("  … +2 more\n"));
}

#[cfg(unix)]
const FAKE_GLAB: &str = r##"#!/bin/sh
for a in "$@"; do
  if [ "$a" = "--web" ]; then echo "fake-passthrough $*"; exit 0; fi
done
case "$1 $2" in
"mr list") echo '[{"iid":7,"title":"Fix parser","state":"opened","author":{"username":"al"}}]' ;;
"mr view") echo '{"iid":42,"title":"Widget","state":"merged","author":{"username":"al"},"web_url":"u","merge_status":"can_be_merged","source_branch":"f","target_branch":"main"}' ;;
"mr create") echo 'https://gitlab.example.com/g/p/-/merge_requests/42' ;;
"mr diff") printf 'diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n-old\n+new\n' ;;
"issue list") echo '[{"iid":5,"title":"Crash","state":"opened"}]' ;;
"issue view") echo '{"iid":5,"title":"Crash","state":"opened","author":{"username":"bo"},"web_url":"w","description":"Steps"}' ;;
"ci list") echo '[{"id":9,"status":"failed","ref":"main"}]' ;;
"mr merge"|"mr approve"|"mr note"|"mr update") echo done ;;
*) echo "fake-passthrough $*" ;;
esac
"##;

#[cfg(unix)]
#[test]
fn rtk_glab_filters_fake_glab_output() {
    use std::os::unix::fs::PermissionsExt;
    if !crate::core::test_isolation::rtk_binary_is_built() {
        return;
    }
    let dir = crate::core::test_isolation::tempdir();
    let script = dir.path().join("glab");
    std::fs::write(&script, FAKE_GLAB).unwrap();
    let mut perms = std::fs::metadata(&script).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script, perms).unwrap();

    let base_path = crate::core::test_isolation::rtk_command()
        .get_envs()
        .find(|(k, _)| k.to_str() == Some("PATH"))
        .and_then(|(_, v)| v.map(|v| v.to_os_string()))
        .unwrap_or_default();
    let mut path = dir.path().as_os_str().to_os_string();
    path.push(":");
    path.push(&base_path);

    let run = |args: &[&str]| -> (String, bool) {
        let out = crate::core::test_isolation::rtk_command()
            .env("PATH", &path)
            .arg("glab")
            .args(args)
            .output()
            .unwrap();
        (
            String::from_utf8_lossy(&out.stdout).into_owned(),
            out.status.success(),
        )
    };

    let (out, ok) = run(&["mr", "list"]);
    assert!(ok, "{}", out);
    assert!(out.contains("Merge Requests"), "{}", out);
    assert!(out.contains("!7"), "{}", out);

    let (out, ok) = run(&["mr", "view", "42"]);
    assert!(ok);
    assert!(out.contains("[merged] MR !42: Widget"), "{}", out);

    let (out, ok) = run(&["mr", "view", "42", "--web"]);
    assert!(ok);
    assert!(out.contains("fake-passthrough mr view 42 --web"), "{}", out);

    let (out, ok) = run(&["mr", "create", "--fill"]);
    assert!(ok);
    assert!(out.contains("ok created !42"), "{}", out);

    let (out, ok) = run(&["mr", "diff", "42"]);
    assert!(ok);
    assert!(out.contains("a.rs"), "{}", out);

    let (out, _) = run(&["mr", "merge", "42"]);
    assert!(out.contains("ok merged !42"), "{}", out);
    let (out, _) = run(&["mr", "approve", "42"]);
    assert!(out.contains("ok approved !42"), "{}", out);
    let (out, _) = run(&["mr", "note", "-m", "hi", "3"]);
    assert!(out.contains("ok noted !3"), "{}", out);
    let (out, _) = run(&["mr", "update", "12", "-l", "bug"]);
    assert!(out.contains("ok updated !12"), "{}", out);

    let (out, _) = run(&["mr", "rebase", "5"]);
    assert!(out.contains("fake-passthrough mr rebase 5"), "{}", out);
    let (out, _) = run(&["mr"]);
    assert!(out.contains("fake-passthrough mr"), "{}", out);

    let (out, _) = run(&["issue", "list"]);
    assert!(out.contains("Issues"), "{}", out);
    assert!(out.contains("#5 Crash"), "{}", out);
    let (out, _) = run(&["issue", "view", "5"]);
    assert!(out.contains("[open] Issue #5: Crash"), "{}", out);
    assert!(out.contains("Description:"), "{}", out);
    let (out, _) = run(&["issue", "view", "5", "--web"]);
    assert!(
        out.contains("fake-passthrough issue view 5 --web"),
        "{}",
        out
    );
    let (out, _) = run(&["issue", "close", "5"]);
    assert!(out.contains("fake-passthrough issue close 5"), "{}", out);
    let (out, _) = run(&["issue"]);
    assert!(out.contains("fake-passthrough issue"), "{}", out);

    let (out, _) = run(&["pipeline", "list"]);
    assert!(out.contains("Pipelines"), "{}", out);
    assert!(out.contains("[fail] #9 failed (main)"), "{}", out);
    let (out, _) = run(&["ci"]);
    assert!(out.contains("fake-passthrough ci"), "{}", out);
    let (out, _) = run(&["ci", "retry", "9"]);
    assert!(out.contains("fake-passthrough ci retry 9"), "{}", out);

    let (out, _) = run(&["release"]);
    assert!(out.contains("fake-passthrough release"), "{}", out);
    let (out, _) = run(&["label", "list"]);
    assert!(out.contains("fake-passthrough label list"), "{}", out);
}
