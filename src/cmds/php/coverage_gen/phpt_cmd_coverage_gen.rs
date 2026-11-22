use super::filter_phpt_output;

#[test]
fn scanned_warnings_and_leaks_keep_only_unexpected_leak_details() {
    let input = concat!(
        "PHP_VERSION : 8.4.20\n",
        "WARN Deprecated setting [warning.phpt]\n",
        "XLEAK Known allocation leak [known.phpt]\n",
        "========DIFF========\n",
        "001- no outstanding allocations\n",
        "001+ one outstanding allocation\n",
        "========DONE========\n",
        "LEAK Allocation survives cleanup [leak.phpt]\n",
    );

    assert_eq!(
        filter_phpt_output(input),
        concat!(
            "phpt: 0 passed, 1 warned, 1 leaked, 1 xleaked  (3 total)\n",
            "PHP 8.4.20\n",
            "\n",
            "FAILURES (1):\n",
            "  leak.phpt -- Allocation survives cleanup\n",
            "    001- no outstanding allocations\n",
            "    001+ one outstanding allocation",
        )
    );
}

#[cfg(unix)]
#[test]
fn phpt_requests_diffs_unless_the_caller_already_requested_them() {
    use crate::core::test_isolation::{rtk_binary_is_built, rtk_command, tempdir};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    const TEST_PATH: &str = "selected sample.phpt";

    if !rtk_binary_is_built() {
        return;
    }

    let dir = tempdir();
    let php = dir.path().join("php");

    // Put the received arguments in a diff so rtk's filtered stdout exposes
    // their order and boundaries, independently of its verbose command log.
    fs::write(
        &php,
        r#"#!/bin/sh
printf '%s\n' '========DIFF========'
printf '<%s>\n' "$@"
printf '%s\n' '========DONE========' 'FAIL Arguments received [args.phpt]'
"#,
    )
    .expect("write fake php");
    fs::set_permissions(&php, fs::Permissions::from_mode(0o700)).expect("make fake php executable");

    let cases: [(&str, &[&str]); 2] = [
        ("--show-out", &["--show-diff", "--show-out"]),
        ("--show-all", &["--show-all"]),
    ];

    for (flag, expected_flags) in cases {
        let mut cmd = rtk_command();
        let inherited_path = cmd
            .get_envs()
            .find_map(|(key, value)| if key == "PATH" { value } else { None })
            .expect("isolated rtk command supplies PATH");
        let path = std::env::join_paths(
            std::iter::once(dir.path().to_path_buf()).chain(std::env::split_paths(inherited_path)),
        )
        .expect("prepend fake php directory to PATH");

        let output = cmd
            .env("PATH", path)
            .current_dir(dir.path())
            .args(["-v", "phpt", "--", flag, TEST_PATH])
            .output()
            .expect("run isolated rtk phpt");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "rtk phpt with {flag} exited with {}: {stderr}",
            output.status
        );

        let mut expected_args = vec!["run-tests.php"];
        expected_args.extend_from_slice(expected_flags);
        expected_args.push(TEST_PATH);
        let diff = expected_args
            .iter()
            .map(|arg| format!("    <{arg}>"))
            .collect::<Vec<_>>()
            .join("\n");

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(
            stdout.trim_end(),
            format!(
                "phpt: 0 passed, 1 failed  (1 total)\n\nFAILURES (1):\n  args.phpt -- Arguments received\n{diff}"
            ),
            "incorrect arguments forwarded with {flag}"
        );

        let expected_log = format!("Running: php {}", expected_args.join(" "));
        assert!(
            stderr.lines().any(|line| line == expected_log),
            "missing verbose line {expected_log:?} in {stderr:?}"
        );
    }
}
