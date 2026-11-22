#[test]
fn tree_filter_skips_leading_whitespace_but_keeps_internal_spacing() {
    let raw = "\n \t\n.\n\n└── main.rs\n \t\n0 directories, 1 file\n";
    assert_eq!(super::filter_tree_output(raw), ".\n\n└── main.rs\n");
}

#[test]
fn tree_reports_install_options_when_unavailable() {
    if super::tool_exists("tree") {
        return;
    }

    let message = super::run(&[], 0).unwrap_err().to_string();
    assert_eq!(
        message.lines().next(),
        Some("tree command not found. Install it first:")
    );
    for hint in [
        "brew install tree",
        "sudo apt install tree",
        "sudo dnf install tree",
        "sudo pacman -S tree",
    ] {
        assert!(message.contains(hint), "missing {hint:?} in {message:?}");
    }
}

#[cfg(unix)]
#[test]
fn verbose_tree_reports_reduction_and_respects_show_all() {
    use crate::core::test_isolation::{rtk_binary_is_built, rtk_command, tempdir};
    use std::os::unix::fs::PermissionsExt;
    use std::process::Stdio;

    if !rtk_binary_is_built() {
        return;
    }

    let dir = tempdir();
    let tool = dir.path().join("tree");
    std::fs::write(
        &tool,
        r#"#!/bin/sh
if [ "$#" -ne 1 ] || [ "$1" != "-a" ]; then
    printf '%s\n' "$@"
    printf '\n0 directories, 0 files\n'
fi
"#,
    )
    .unwrap();
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();

    let check_tree = |args: &[&str], expected_stdout: &str, expected_report: &str| {
        let mut cmd = rtk_command();
        let inherited_path = cmd
            .get_envs()
            .find(|(name, _)| *name == "PATH")
            .and_then(|(_, value)| value);
        let path = std::env::join_paths(
            std::iter::once(dir.path().to_path_buf())
                .chain(inherited_path.into_iter().flat_map(std::env::split_paths)),
        )
        .unwrap();
        let output = cmd
            .env("PATH", path)
            .args(["-v", "tree"])
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap();

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{}: {stderr}", output.status);
        assert_eq!(String::from_utf8(output.stdout).unwrap(), expected_stdout);
        assert!(
            stderr.lines().any(|line| line == expected_report),
            "expected {expected_report:?} in {stderr:?}"
        );
    };

    check_tree(
        &[],
        &format!("-I\n{}\n", super::NOISE_DIRS.join("|")),
        "Lines: 4 → 2 (50% reduction)",
    );
    check_tree(&["-a"], "\n", "Lines: 0 → 1 (0% reduction)");
}
