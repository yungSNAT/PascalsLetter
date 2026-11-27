#[cfg(unix)]
#[test]
fn pest_forwards_arguments_and_filters_progress_output() {
    use crate::core::test_isolation::{rtk_binary_ready, rtk_command, tempdir};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Stdio;

    if !rtk_binary_ready() {
        return;
    }

    let dir = tempdir();
    let pest = dir.path().join("pest");
    let arg_log = dir.path().join("pest-args.txt");

    fs::write(
        &pest,
        r#"#!/bin/sh
set -eu
printf '%s\n' "$@" > pest-args.txt
printf '%s\n' 'Pest 5.0.0' '.....' 'PASS  Tests\Unit\ExampleTest'
"#,
    )
    .expect("write fake Pest executable");
    fs::set_permissions(&pest, fs::Permissions::from_mode(0o700))
        .expect("make fake Pest executable");

    for (verbose, args, expected_args) in [
        (
            true,
            ["Example Test.php", "--stop-on-failure"],
            "--no-progress\nExample Test.php\n--stop-on-failure\n",
        ),
        (
            false,
            ["Example Test.php", "--no-progress"],
            "Example Test.php\n--no-progress\n",
        ),
    ] {
        let mut cmd = rtk_command();
        let inherited_path = cmd
            .get_envs()
            .find(|(key, _)| *key == "PATH")
            .and_then(|(_, value)| value)
            .expect("isolated rtk command has a PATH");
        let path = std::env::join_paths(
            std::iter::once(dir.path().to_path_buf()).chain(std::env::split_paths(inherited_path)),
        )
        .expect("prepend fake Pest directory to PATH");
        cmd.env("PATH", path)
            .current_dir(dir.path())
            .stdin(Stdio::null());
        if verbose {
            cmd.arg("-v");
        }

        let output = cmd
            .arg("pest")
            .args(args)
            .output()
            .expect("run rtk pest against the fake executable");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "rtk pest failed: {stderr}");
        assert_eq!(
            fs::read_to_string(&arg_log).expect("read arguments received by Pest"),
            expected_args
        );
        fs::remove_file(&arg_log).expect("clear the argument log before the next run");

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains(r"PASS  Tests\Unit\ExampleTest"), "{stdout}");
        assert!(!stdout.contains("Pest 5.0.0"), "{stdout}");
        assert!(!stdout.contains("....."), "{stdout}");

        if verbose {
            assert!(
                stderr
                    .lines()
                    .any(|line| line == "Running: pest Example Test.php --stop-on-failure"),
                "{stderr}"
            );
        } else {
            assert!(!stderr.contains("Running: pest"), "{stderr}");
        }
    }
}
