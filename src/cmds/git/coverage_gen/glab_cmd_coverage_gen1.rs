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

#[cfg(unix)]
#[test]
fn ci_and_release_commands_forward_arguments_and_filter_only_supported_subcommands() {
    use std::os::unix::fs::PermissionsExt;

    if !test_isolation::rtk_binary_is_built() {
        return;
    }

    let bin = test_isolation::tempdir();
    let fake_glab = bin.path().join("glab");
    std::fs::write(
        &fake_glab,
        r#"#!/bin/sh
case "$#:$*" in
    '6:ci list -F json --page 2')
        printf '%s\n' '[{"id":37,"status":"success","ref":"main"}]'
        ;;
    '4:ci status --branch main')
        printf '%s\n' 'Pipeline #37' '' '  build passed  '
        ;;
    '3:ci trace 73')
        printf '%s\n' 'Running with gitlab-runner 17' '[0Ksection_start:1700000000:build[0K' \
            '[36;1m  Compiling widgets[0;m' 'WARNING: optional docs skipped'
        ;;
    '4:release list --page 2')
        printf 'Name\tTag\tCreated\nWidget release\tv2.1.0\tYesterday\n'
        ;;
    '3:release view v2.1.0')
        printf '%s\n' 'Release v2.1.0' '![preview](widget.png)' 'Added widget support'
        ;;
    '1:release')
        printf '%s\n' 'release-help-fixture'
        ;;
    '3:release download v2.1.0')
        printf '%s\n' 'release-download-fixture'
        ;;
    *)
        printf 'Unexpected fake glab arguments: %s\n' "$*" >&2
        exit 64
        ;;
esac
"#,
    )
    .expect("write fake glab");
    std::fs::set_permissions(&fake_glab, std::fs::Permissions::from_mode(0o700))
        .expect("make fake glab executable");

    let cases: &[(&[&str], &str)] = &[
        (
            &["ci", "list", "--page", "2"],
            "Pipelines\n  [ok] #37 success (main)",
        ),
        (
            &["ci", "status", "--branch", "main"],
            "  Pipeline #37\n[ok] build passed",
        ),
        (
            &["ci", "trace", "73"],
            "Compiling widgets\nWARNING: optional docs skipped",
        ),
        (
            &["release", "list", "--page", "2"],
            "Releases\n  Widget release [v2.1.0] (Yesterday)",
        ),
        (
            &["release", "view", "v2.1.0"],
            "Release v2.1.0\nAdded widget support",
        ),
        (&["release"], "release-help-fixture"),
        (
            &["release", "download", "v2.1.0"],
            "release-download-fixture",
        ),
    ];

    for &(args, expected) in cases {
        let mut cmd = test_isolation::rtk_command();
        let inherited_path = cmd
            .get_envs()
            .find(|(key, _)| *key == "PATH")
            .and_then(|(_, value)| value)
            .expect("isolated rtk command should provide PATH");
        let path = std::env::join_paths(
            std::iter::once(bin.path().to_path_buf()).chain(std::env::split_paths(inherited_path)),
        )
        .expect("prepend fake glab directory to PATH");
        let output = cmd
            .env("PATH", path)
            .arg("glab")
            .args(args)
            .output()
            .expect("run isolated rtk with fake glab");

        assert!(
            output.status.success(),
            "rtk glab {args:?} failed with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).expect("rtk stdout should be UTF-8");
        assert_eq!(stdout.trim_end(), expected, "rtk glab {args:?}");
    }
}
