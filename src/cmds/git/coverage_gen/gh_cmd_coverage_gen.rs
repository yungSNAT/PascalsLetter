use super::*;
use serde_json::json;

type TakesValue = dyn Fn(TokenKind, &str) -> Option<ValueSpec>;

#[test]
fn markdown_tilde_fence_and_unclosed_fence_are_preserved() {
    // Only `~~~` fences: the fence kind must be detected as tildes.
    let input = "Intro\n~~~\ncode <!-- keep -->\n~~~\nAfter <!-- drop -->";
    let out = filter_markdown_body(input);
    assert!(out.contains("code <!-- keep -->"), "{out}");
    assert!(!out.contains("drop"), "{out}");
    assert!(out.contains("Intro"));
    assert!(out.contains("After"));

    // Unclosed code block: everything after the opening fence is kept verbatim.
    let unclosed = "Text <!-- gone -->\n```rust\nlet x = 1; <!-- kept -->\n---";
    let out = filter_markdown_body(unclosed);
    assert!(out.contains("<!-- kept -->"), "{out}");
    assert!(out.contains("---"), "{out}");
    assert!(!out.contains("gone"), "{out}");
}

#[test]
fn takes_value_tables_reject_non_flag_tokens_and_know_edit_shorts() {
    let tables: [&TakesValue; 6] = [
        &inherited_takes_value,
        &view_takes_value,
        &pr_checks_takes_value,
        &run_view_takes_value,
        &pr_comment_takes_value,
        &pr_edit_takes_value,
    ];
    for table in tables {
        assert!(table(TokenKind::Positional, "repo").is_none());
        assert!(table(TokenKind::DashDash, "R").is_none());
        assert!(table(TokenKind::Short, "R").is_some());
        assert!(table(TokenKind::Long, "repo").is_some());
    }
    for short in ["B", "b", "F", "m", "t"] {
        assert!(
            pr_edit_takes_value(TokenKind::Short, short).is_some(),
            "{short}"
        );
    }
    assert!(pr_edit_takes_value(TokenKind::Short, "x").is_none());
    assert!(pr_edit_takes_value(TokenKind::Long, "remove-milestone").is_none());

    // A short edit flag swallows its value, so it is not the PR number.
    let args: Vec<String> = vec!["-m".into(), "v1".into(), "12".into()];
    let (id, extra) = split_identifier(&args, &pr_edit_takes_value);
    assert_eq!(id.as_deref(), Some("12"));
    assert_eq!(extra, vec!["-m", "v1"]);
}

#[test]
fn split_gh_region_requires_a_leading_positional() {
    let flag_first: Vec<String> = vec!["--version".into()];
    assert!(split_gh_region(&flag_first).is_none());
    assert!(split_gh_region(&[]).is_none());

    let region: Vec<String> = vec!["pr".into(), "list".into(), "-R".into(), "o/r".into()];
    let (sub, rest) = split_gh_region(&region).expect("positional subcommand");
    assert_eq!(sub, "pr");
    assert_eq!(rest, &region[1..]);
}

#[test]
fn state_icon_maps_every_state() {
    assert_eq!(state_icon("OPEN"), "[open]");
    assert_eq!(state_icon("MERGED"), "[merged]");
    assert_eq!(state_icon("CLOSED"), "[closed]");
    assert_eq!(state_icon("DRAFT"), "[unknown]");
}

#[test]
fn format_pr_list_handles_shapes_and_caps_long_lists() {
    assert_eq!(format_pr_list(&json!({"not": "an array"})), "");
    assert_eq!(format_pr_list(&json!([])), "No Pull Requests\n");

    let small = json!([
        {"number": 1, "title": "First", "state": "OPEN", "author": {"login": "alice"}},
        {"number": 2, "title": "Second", "state": "MERGED", "author": {"login": "bob"}},
        {"state": "WEIRD"}
    ]);
    let out = format_pr_list(&small);
    assert!(out.starts_with("Pull Requests\n"), "{out}");
    assert!(out.contains("  [open] #1 First (alice)\n"), "{out}");
    assert!(out.contains("  [merged] #2 Second (bob)\n"), "{out}");
    assert!(out.contains("  [unknown] #0 ??? (???)\n"), "{out}");
    assert!(!out.contains("more"), "{out}");

    let extra = 3;
    let many: Vec<Value> = (0..CAP_LIST + extra)
        .map(|i| {
            json!({"number": i, "title": format!("pr {i}"), "state": "CLOSED", "author": {"login": "c"}})
        })
        .collect();
    let many = Value::Array(many);
    let dir = crate::core::test_isolation::tempdir();
    crate::core::test_isolation::with_root(dir.path(), || {
        let out = format_pr_list(&many);
        assert!(out.contains(&format!("  … +{} more\n", extra)), "{out}");
        assert!(out.contains("  [closed] #0 pr 0 (c)\n"), "{out}");
        let last = format!("#{} pr {} ", CAP_LIST + extra - 1, CAP_LIST + extra - 1);
        assert!(!out.contains(&last), "{out}");
    });
}

#[test]
fn format_issue_list_handles_shapes_and_caps_long_lists() {
    assert_eq!(format_issue_list(&json!("nope")), "");
    assert_eq!(format_issue_list(&json!([])), "No Issues\n");

    let small = json!([
        {"number": 5, "title": "Bug", "state": "OPEN"},
        {"number": 6, "title": "Old", "state": "CLOSED"}
    ]);
    let out = format_issue_list(&small);
    assert_eq!(out, "Issues\n  [open] #5 Bug\n  [closed] #6 Old\n");

    let extra = 2;
    let many: Vec<Value> = (0..CAP_LIST + extra)
        .map(|i| json!({"number": i, "title": format!("issue {i}"), "state": "OPEN"}))
        .collect();
    let many = Value::Array(many);
    let dir = crate::core::test_isolation::tempdir();
    crate::core::test_isolation::with_root(dir.path(), || {
        let out = format_issue_list(&many);
        assert!(out.starts_with("Issues\n  [open] #0 issue 0\n"), "{out}");
        assert!(out.contains(&format!("  … +{} more\n", extra)), "{out}");
    });
}

#[test]
fn format_pr_view_reports_conflicts_reviews_and_failed_checks() {
    let pr = json!({
        "number": 7,
        "title": "Big change",
        "state": "MERGED",
        "author": {"login": "octo"},
        "url": "https://github.com/o/r/pull/7",
        "mergeable": "CONFLICTING",
        "reviews": {"nodes": [
            {"state": "APPROVED"},
            {"state": "CHANGES_REQUESTED"},
            {"state": "COMMENTED"}
        ]},
        "statusCheckRollup": [
            {"conclusion": "SUCCESS"},
            {"state": "SUCCESS"},
            {"conclusion": "FAILURE"},
            {"state": "PENDING"}
        ]
    });
    let out = format_pr_view(&pr);
    assert!(out.starts_with("[merged] PR #7: Big change\n"), "{out}");
    assert!(out.contains("  MERGED | [x]\n"), "{out}");
    assert!(
        out.contains("  Reviews: 1 approved, 1 changes requested\n"),
        "{out}"
    );
    assert!(out.contains("  Checks: 2/4 passed\n"), "{out}");
    assert!(out.contains("  [warn] 1 checks failed\n"), "{out}");
    assert!(out.contains("  https://github.com/o/r/pull/7\n"), "{out}");

    let quiet = json!({
        "number": 8,
        "title": "Small",
        "state": "CLOSED",
        "mergeable": "UNKNOWN",
        "reviews": {"nodes": [{"state": "COMMENTED"}]},
        "statusCheckRollup": [{"conclusion": "SUCCESS"}]
    });
    let out = format_pr_view(&quiet);
    assert!(out.starts_with("[closed] PR #8: Small\n"), "{out}");
    assert!(out.contains("  ???\n"), "{out}");
    assert!(out.contains("  CLOSED | ?\n"), "{out}");
    assert!(!out.contains("Reviews:"), "{out}");
    assert!(out.contains("  Checks: 1/1 passed\n"), "{out}");
    assert!(!out.contains("[warn]"), "{out}");
}

#[test]
fn format_pr_checks_skips_unparseable_lines_and_counts_other_statuses() {
    let output = concat!(
        "Some checks were not successful\n",
        "\tpass\t1s\thttps://example.com/nameless\n",
        "ci\tpass\t1s\thttps://example.com/ci\n",
        "deploy\tskipping\t0\thttps://example.com/deploy\n",
        "build\t*\t0\thttps://example.com/build\n",
    );
    let out = format_pr_checks(output);
    assert!(out.starts_with("CI Checks Summary:\n"), "{out}");
    assert!(out.contains("  [ok] Passed: 1\n"), "{out}");
    assert!(out.contains("  [FAIL] Failed: 0\n"), "{out}");
    assert!(out.contains("  [pending] Pending: 1\n"), "{out}");
    assert!(out.contains("  [skip] Skipped/cancelled: 1\n"), "{out}");
    assert!(!out.contains("Failed checks:"), "{out}");
}

#[test]
fn format_pr_status_lists_created_prs_and_skips_nulls() {
    assert_eq!(format_pr_status_entry(&Value::Null), "");

    let json = json!({
        "currentBranch": null,
        "createdBy": [
            null,
            {"number": 1, "title": "a", "reviewDecision": "APPROVED",
             "statusCheckRollup": [{"conclusion": "SUCCESS"}]},
            {"number": 2, "title": "b", "statusCheckRollup": []},
            {"number": 3, "title": "c",
             "statusCheckRollup": [{"state": "FAILURE"}, {"state": "SUCCESS"}]},
            {"number": 4, "title": "d"},
            {"number": 6, "title": "e"}
        ]
    });
    let out = format_pr_status(&json);
    assert!(!out.contains("Current Branch"), "{out}");
    // The `null` entry is neither counted nor given a display slot.
    assert!(out.starts_with("Your PRs (5):\n"), "{out}");
    assert!(out.contains("  #1 a [APPROVED] checks 1/1\n"), "{out}");
    assert!(out.contains("  #2 b [PENDING]\n"), "{out}");
    assert!(
        out.contains("  #3 c [PENDING] checks 1/2 fail 1\n"),
        "{out}"
    );
    assert!(out.contains("  #4 d [PENDING]\n"), "{out}");
    assert!(out.contains("  #6 e [PENDING]\n"), "{out}");

    let with_branch = json!({
        "currentBranch": {"number": 9, "title": "mine", "reviewDecision": "REVIEW_REQUIRED"}
    });
    let out = format_pr_status(&with_branch);
    assert_eq!(out, "Current Branch\n  #9 mine [REVIEW_REQUIRED]\n\n");
}
