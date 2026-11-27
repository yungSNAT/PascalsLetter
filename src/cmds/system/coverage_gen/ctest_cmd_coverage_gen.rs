use super::*;
use std::fmt::Write as _;

#[test]
fn ansi_and_whitespace_without_ctest_output_are_discarded() {
    assert!(filter_ctest_output(" \t\x1b[31m\x1b[0m\r\n").is_empty());
}

fn assert_recovery_hint(line: Option<&str>, indent: &str) {
    let line = line.expect("expected a recovery hint after the truncation marker");
    let hint = line
        .strip_prefix(indent)
        .expect("recovery hint indentation");
    assert!(
        hint.starts_with("[see remaining:") || hint.starts_with("[+"),
        "unexpected recovery hint: {line}"
    );
}

#[test]
fn counts_passes_without_a_summary_and_adds_skipped_recovery_hint() {
    let root = crate::core::test_isolation::tempdir();
    let skipped = MAX_FAILED_LIST_LINES + 1;
    let total = skipped + 2;
    let mut input = format!(
        "Test project build\n\
         1/{total} Test #1: executed .... Passed 0.10 sec\n\
         2/{total} Test #2: disabled_case ....***Not Run (Disabled) 0.00 sec\n"
    );
    for index in 1..=skipped {
        let number = index + 2;
        writeln!(
            input,
            "{number}/{total} Test #{number}: skipped_case_{index} ....***Skipped 0.00 sec"
        )
        .unwrap();
    }

    let filtered =
        crate::core::test_isolation::with_root(root.path(), || filter_ctest_output(&input));
    let mut lines = filtered.lines();
    let summary = format!("ctest: 1/{total} passed, {skipped} skipped, 1 disabled");
    assert_eq!(lines.next(), Some(summary.as_str()));
    assert_eq!(lines.next(), Some("skipped:"));
    for index in 1..=MAX_FAILED_LIST_LINES {
        let entry = format!("  skipped_case_{index}");
        assert_eq!(lines.next(), Some(entry.as_str()));
    }
    assert_eq!(lines.next(), Some("  ... +1 more skipped"));
    assert_recovery_hint(lines.next(), "  ");
    assert_eq!(
        lines.collect::<Vec<_>>(),
        ["slowest:", "  executed 0.10 sec"]
    );
}

#[test]
fn adds_raw_failure_recovery_hint_without_duplicating_parsed_failure() {
    let root = crate::core::test_isolation::tempdir();
    let total = MAX_FAILED_LIST_LINES + 2;
    let mut input = format!(
        "Test project build\n\
         1/{total} Test #1: known_case ....***Failed 0.20 sec\n\
         0% tests passed, {total} tests failed out of {total}\n\
         The following tests FAILED:\n\t1 - known_case (Failed)\n"
    );
    for number in 2..=total {
        writeln!(input, "\t{number} - raw_case_{number} (Failed)").unwrap();
    }
    input.push_str("Errors while running CTest\n");

    let filtered =
        crate::core::test_isolation::with_root(root.path(), || filter_ctest_output(&input));
    let mut lines = filtered.lines();
    let summary = format!("ctest: 0/{total} passed, {total} failed");
    assert_eq!(lines.next(), Some(summary.as_str()));
    assert_eq!(lines.next(), Some("failed:"));
    assert_eq!(lines.next(), Some("  #1 known_case (Failed, 0.20 sec)"));
    assert_eq!(lines.next(), Some("failed (unparsed, raw):"));
    for number in 2..total {
        let entry = format!("    {number} - raw_case_{number} (Failed)");
        assert_eq!(lines.next(), Some(entry.as_str()));
    }
    assert_eq!(lines.next(), Some("    ... +1 more failed"));
    assert_recovery_hint(lines.next(), "    ");
    assert_eq!(
        lines.collect::<Vec<_>>(),
        ["", "Errors while running CTest"]
    );
}

#[cfg(unix)]
#[test]
fn ctest_cli_forwards_arguments_in_filtered_and_passthrough_modes() {
    use crate::core::test_isolation::{rtk_binary_ready, rtk_command, tempdir};
    use std::os::unix::fs::PermissionsExt;

    if !rtk_binary_ready() {
        return;
    }

    let tools = tempdir();
    let tool = tools.path().join("ctest");
    std::fs::write(
        &tool,
        r#"#!/bin/sh
case "$#:$1:$2" in
    '1:--show-only:') name=listed_case ;;
    '2:-R:sample case') name=sample_case ;;
    *) name=unexpected_arguments ;;
esac
printf 'Test project fake-build\n1/1 Test #1: %s .... Passed 0.25 sec\n' "$name"
"#,
    )
    .unwrap();
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o700)).unwrap();

    let invoke = |args: &[&str]| {
        let mut cmd = rtk_command();
        let inherited_path = cmd
            .get_envs()
            .find(|(key, _)| *key == "PATH")
            .and_then(|(_, value)| value)
            .expect("isolated rtk command should have a PATH");
        let path = std::env::join_paths(
            std::iter::once(tools.path().to_path_buf())
                .chain(std::env::split_paths(inherited_path)),
        )
        .unwrap();
        let output = cmd.env("PATH", path).args(args).output().unwrap();
        assert!(output.status.success(), "{output:?}");
        output
    };

    let filtered = invoke(&["-v", "ctest", "-R", "sample case"]);
    assert_eq!(
        String::from_utf8(filtered.stdout).unwrap().trim_end(),
        "ctest: 1/1 passed\nslowest:\n  sample_case 0.25 sec"
    );
    let stderr = String::from_utf8(filtered.stderr).unwrap();
    assert!(
        stderr
            .lines()
            .any(|line| line == "Running: ctest -R sample case")
    );

    let passthrough = invoke(&["ctest", "--show-only"]);
    assert_eq!(
        String::from_utf8(passthrough.stdout).unwrap(),
        "Test project fake-build\n1/1 Test #1: listed_case .... Passed 0.25 sec\n"
    );
}
