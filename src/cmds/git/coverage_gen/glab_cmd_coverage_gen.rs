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
