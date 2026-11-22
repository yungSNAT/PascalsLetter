#[cfg(unix)]
#[test]
fn paratest_forwards_arguments_and_filters_output() {
    use crate::core::test_isolation;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Stdio;

    if !test_isolation::rtk_binary_is_built() {
        return;
    }

    let scratch = test_isolation::tempdir();
    let tool = scratch.path().join("paratest");
    let recorded_args = scratch.path().join("paratest-args.txt");
    fs::write(
        &tool,
        r#"#!/bin/sh
printf '%s\n' "$@" > paratest-args.txt
printf '%s\n' \
    'ParaTest v7.3.0 upon PHPUnit 10.5.0 by Sebastian Bergmann and contributors.' \
    'Random Seed:   1234567890' \
    '..........                                        10 / 10 (100%)' \
    '' \
    'OK (10 tests, 25 assertions)'
"#,
    )
    .unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o700)).unwrap();

    for (cli_args, expected_args, expected_trace) in [
        (
            &["-v", "paratest", "--", "--filter", "Example test"][..],
            "--no-progress\n--filter\nExample test\n",
            Some("Running: paratest --filter Example test"),
        ),
        (
            &[
                "paratest",
                "--",
                "--filter",
                "Example test",
                "--no-progress",
            ][..],
            "--filter\nExample test\n--no-progress\n",
            None,
        ),
    ] {
        let mut cmd = test_isolation::rtk_command();
        let inherited_path = cmd
            .get_envs()
            .find(|(key, _)| *key == "PATH")
            .and_then(|(_, value)| value)
            .expect("the isolated rtk command should provide PATH");
        let path = std::env::join_paths(
            std::iter::once(scratch.path().to_path_buf())
                .chain(std::env::split_paths(inherited_path)),
        )
        .unwrap();

        let output = cmd
            .env("PATH", path)
            .current_dir(scratch.path())
            .stdin(Stdio::null())
            .args(cli_args)
            .output()
            .unwrap();

        let stdout = String::from_utf8(output.stdout).unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(
            output.status.code(),
            Some(0),
            "stdout: {stdout}\nstderr: {stderr}"
        );
        assert_eq!(fs::read_to_string(&recorded_args).unwrap(), expected_args);
        assert!(stdout.contains("OK (10 tests, 25 assertions)"), "{stdout}");
        for noise in ["ParaTest v7.3.0", "Random Seed:", "10 / 10 (100%)"] {
            assert!(!stdout.contains(noise), "found {noise:?} in {stdout}");
        }
        assert_eq!(
            stderr
                .lines()
                .find(|line| line.starts_with("Running: paratest ")),
            expected_trace,
            "{stderr}"
        );
    }
}
