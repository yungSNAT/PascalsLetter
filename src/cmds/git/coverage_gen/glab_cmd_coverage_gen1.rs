use super::{
    CAP_WARNINGS, filter_release_view, format_ci_list, format_ci_status, format_release_list,
};
use crate::core::{test_isolation, user_env};
use serde_json::{Value, json};
use std::fmt::Write;

#[test]
fn ci_list_caps_visible_rows_and_counts_omitted_pipelines() {
    let root = test_isolation::tempdir();
    let tee_dir = root.path().join("tee");
    let pipelines = Value::Array(
        (1..=CAP_WARNINGS + 2)
            .map(|id| json!({"id": id, "status": "pending", "ref": "main"}))
            .collect(),
    );
    let rows: Vec<_> = (1..=CAP_WARNINGS + 2)
        .map(|id| format!("  [pend] #{id} pending (main)"))
        .collect();
    let expected_summary = format!(
        "Pipelines\n{}\n  … +2 more\n",
        rows[..CAP_WARNINGS].join("\n")
    );

    let output = test_isolation::with_root(root.path(), || {
        user_env::with_vars(&[("RTK_TEE", Some("1"))], || {
            user_env::with_path("RTK_TEE_DIR", Some(tee_dir.as_path()), || {
                format_ci_list(&pipelines)
            })
        })
    });

    // A tee hint is optional, but the omitted rows must not appear in the summary.
    let tail = output
        .strip_prefix(&expected_summary)
        .expect("summary should show the first pipelines in order and count the omitted rows");
    for row in &rows[CAP_WARNINGS..] {
        assert!(
            !tail.contains(row.as_str()),
            "omitted row was printed: {row}"
        );
    }
}

#[test]
fn ci_status_marks_jobs_without_discarding_context() {
    let raw = concat!(
        "  Pipeline #37  \n",
        " \t\n",
        "  build passed  \n",
        "  lint success\n",
        "  tests failed\n",
        "  docs running\n",
        "  deploy pending\n",
        "  cleanup canceled\n",
        "  publish cancelled\n",
    );
    assert_eq!(
        format_ci_status(raw),
        concat!(
            "  Pipeline #37\n",
            "[ok] build passed\n",
            "[ok] lint success\n",
            "[fail] tests failed\n",
            "[run] docs running\n",
            "[pend] deploy pending\n",
            "[cancel] cleanup canceled\n",
            "[cancel] publish cancelled\n",
        )
    );
}

#[test]
fn release_list_counts_only_valid_rows_toward_its_limit() {
    let mut raw = String::from(
        "Showing 21 releases\n\nNAME\tTAG\tCREATED\n  \nIncomplete\tv0\nMilestone\tv1\tToday\n",
    );
    let mut expected = String::from("Releases\n  Milestone [v1] (Today)\n");
    for version in 2..=20 {
        writeln!(raw, "v{version}\tv{version}\tToday").unwrap();
        writeln!(expected, "  v{version} (Today)").unwrap();
    }
    raw.push_str("Overflow\tv21\tToday\n");

    assert_eq!(
        format_release_list(&raw).as_deref(),
        Some(expected.as_str())
    );
}

#[test]
fn release_view_drops_standalone_markdown_images_but_keeps_inline_content() {
    let raw = concat!(
        "Release v2\n",
        "  ![overview](diagram.png)  \n",
        "Docs: [guide](guide.md)\n",
        "Status: ![badge](badge.png)\n",
    );
    assert_eq!(
        filter_release_view(raw),
        "Release v2\nDocs: [guide](guide.md)\nStatus: ![badge](badge.png)\n"
    );
}
