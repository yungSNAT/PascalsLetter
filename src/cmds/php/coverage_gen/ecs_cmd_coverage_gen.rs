use super::filter_ecs_output;

#[test]
fn keeps_ecs_findings_and_summaries_in_report_order() {
    let report = concat!(
        "Scanning project\n",
        "   \n",
        "  FAIL: style violations remain  \n",
        "  Fixed 2 issues\n",
        "  5 rules checked\n",
        "  3 files need attention\n",
        "Done\n",
    );

    assert_eq!(
        filter_ecs_output(report),
        "FAIL: style violations remain\nFixed 2 issues\n5 rules checked\n3 files need attention"
    );
}

#[test]
fn falls_back_to_the_cleaned_message_or_ok_when_blank() {
    assert_eq!(filter_ecs_output("  \n\t\n"), "ok");
    assert_eq!(
        filter_ecs_output(
            "\n  \x1b[33mConfiguration unavailable\x1b[0m\n  Supply a configuration.\n"
        ),
        "Configuration unavailable\n  Supply a configuration."
    );
}

#[cfg(all(unix, feature = "engine"))]
#[test]
fn ecs_cli_forwards_arguments_and_filters_the_tool_output() {
    use crate::core::test_isolation;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    if !test_isolation::rtk_binary_is_built() {
        return;
    }

    let dir = test_isolation::tempdir();
    let tool = dir.path().join("ecs");
    fs::write(
        &tool,
        r#"#!/bin/sh
if [ "$#" -eq 2 ] && [ "$1" = check ] && [ "$2" = 'source files' ]; then
    printf '%s\n' '[OK] No errors found. Great job!'
else
    printf '%s\n' 'ERROR: unexpected ECS arguments'
fi
"#,
    )
    .unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();

    let mut cmd = test_isolation::rtk_command();
    let inherited_path = cmd
        .get_envs()
        .find_map(|(name, value)| if name == "PATH" { value } else { None });
    let path = std::env::join_paths(
        std::iter::once(dir.path().to_path_buf())
            .chain(inherited_path.into_iter().flat_map(std::env::split_paths)),
    )
    .unwrap();

    let output = cmd
        .current_dir(dir.path())
        .env("PATH", path)
        .args(["-v", "ecs", "check", "source files"])
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "rtk failed: {stderr}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "✓ ecs: no issues"
    );
    assert!(
        stderr.contains("Running: ecs check source files"),
        "missing verbose command message: {stderr}"
    );
}
