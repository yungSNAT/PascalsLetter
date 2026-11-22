#![cfg(all(unix, feature = "engine"))]

use crate::core::test_isolation;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Stdio;

fn fake_gh(script: &str) -> tempfile::TempDir {
    let bin = test_isolation::tempdir();
    let gh = bin.path().join("gh");
    fs::write(&gh, script).expect("write fake gh");
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).expect("make fake gh executable");
    bin
}

fn run_gh(bin: &Path, args: &[&str]) -> String {
    let mut cmd = test_isolation::rtk_command();
    let inherited_path = cmd
        .get_envs()
        .find(|(key, _)| *key == "PATH")
        .and_then(|(_, value)| value)
        .expect("isolated rtk command has a PATH");
    let path = std::env::join_paths(
        std::iter::once(bin.to_path_buf()).chain(std::env::split_paths(inherited_path)),
    )
    .expect("prepend fake gh directory to PATH");

    let output = cmd
        .env("PATH", path)
        .stdin(Stdio::null())
        .arg("gh")
        .args(args)
        .output()
        .expect("run rtk with fake gh");
    assert!(
        output.status.success(),
        "rtk gh {args:?} exited with {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("rtk stdout is UTF-8")
}

#[test]
fn pr_mutations_confirm_the_target_but_leave_merge_output_intact() {
    if !test_isolation::rtk_binary_is_built() {
        return;
    }

    let bin = fake_gh(
        r#"#!/bin/sh
case "$#:$*" in
    '6:pr create --title Draft title --body Draft body')
        printf '  %s  \n' 'https://example.invalid/acme/demo/pull/42' ;;
    '5:pr comment --body Looks good 42')
        printf '%s\n' 'fake comment response' ;;
    '4:pr edit --add-label needs-review')
        printf '%s\n' 'fake edit response' ;;
    '4:pr merge 42 --squash')
        printf 'fake merge response\n  second line\n\n' ;;
    *)
        printf 'unexpected fake gh arguments: %s\n' "$*" >&2
        exit 97 ;;
esac
"#,
    );

    assert_eq!(
        run_gh(
            bin.path(),
            &[
                "pr",
                "create",
                "--title",
                "Draft title",
                "--body",
                "Draft body"
            ]
        )
        .trim_end(),
        "ok created #42 https://example.invalid/acme/demo/pull/42"
    );
    assert_eq!(
        run_gh(bin.path(), &["pr", "comment", "--body", "Looks good", "42"]).trim_end(),
        "ok commented #42"
    );
    assert_eq!(
        run_gh(bin.path(), &["pr", "edit", "--add-label", "needs-review"]).trim_end(),
        "ok edited"
    );
    assert_eq!(
        run_gh(bin.path(), &["pr", "merge", "42", "--squash"]),
        "fake merge response\n  second line\n\n"
    );
}

#[test]
fn pr_diff_compacts_unified_output_and_preserves_explicit_raw_formats() {
    if !test_isolation::rtk_binary_is_built() {
        return;
    }

    const DIFF: &str = concat!(
        "diff --git a/notes.txt b/notes.txt\n",
        "index 1234567..89abcde 100644\n",
        "--- a/notes.txt\n",
        "+++ b/notes.txt\n",
        "@@ -1,2 +1,2 @@\n",
        "-old note\n",
        "+updated note\n",
        " unchanged context\n",
    );
    let bin = fake_gh(&format!(
        r#"#!/bin/sh
case "$#:$*" in
    '3:pr diff 41') printf ' \t\n\n' ;;
    '3:pr diff 42') printf '%s' '{DIFF}' ;;
    '4:pr diff 42 --name-only') printf '%s\n' 'notes.txt' 'readme.md' ;;
    *)
        printf 'unexpected fake gh arguments: %s\n' "$*" >&2
        exit 97 ;;
esac
"#
    ));

    assert_eq!(
        run_gh(bin.path(), &["pr", "diff", "41"]).trim_end(),
        "No diff"
    );

    let compact = run_gh(bin.path(), &["pr", "diff", "42"]);
    assert!(compact.contains("notes.txt"), "{compact}");
    assert!(compact.contains("updated note"), "{compact}");
    assert!(!compact.contains("index 1234567..89abcde"), "{compact}");

    assert_eq!(
        run_gh(bin.path(), &["pr", "diff", "42", "--no-compact"]),
        DIFF
    );
    assert_eq!(
        run_gh(bin.path(), &["pr", "diff", "42", "--name-only"]),
        "notes.txt\nreadme.md\n"
    );
}
