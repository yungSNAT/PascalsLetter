#![cfg(unix)]

use crate::core::test_isolation;
use std::fmt::Write as _;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Stdio;

fn run_fake_deno(args: &[&str], raw_stdout: &str, verbose: bool) -> (String, String) {
    let scratch = test_isolation::tempdir();
    let tool = scratch.path().join("deno");
    let argv_log = scratch.path().join("deno-argv.txt");
    fs::write(scratch.path().join("deno-stdout.txt"), raw_stdout).unwrap();
    fs::write(
        &tool,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > deno-argv.txt && cat deno-stdout.txt\n",
    )
    .unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o700)).unwrap();

    let mut cmd = test_isolation::rtk_command();
    let inherited_path = cmd
        .get_envs()
        .find_map(|(key, value)| if key == "PATH" { value } else { None })
        .expect("isolated rtk command should have a PATH");
    let path = std::env::join_paths(
        std::iter::once(scratch.path().to_path_buf()).chain(std::env::split_paths(inherited_path)),
    )
    .unwrap();

    if verbose {
        cmd.arg("-v");
    }
    let output = cmd
        .env("PATH", path)
        .current_dir(scratch.path())
        .stdin(Stdio::null())
        .arg("deno")
        .args(args)
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        output.status.success(),
        "deno {args:?} returned {}:\nstdout: {stdout}\nstderr: {stderr}",
        output.status
    );
    assert_eq!(
        fs::read_to_string(argv_log).unwrap(),
        format!("{}\n", args.join("\n")),
        "forwarded argv for deno {args:?}"
    );
    (stdout, stderr)
}

fn passing_test_output() -> String {
    let mut raw = String::from("running 32 tests from ./fixture_test.ts\n");
    for index in 0..32 {
        writeln!(raw, "passing_fixture_case_{index} ... ok (1ms)").unwrap();
    }
    raw.push_str("\nok | 32 passed | 0 failed (40ms)\n");
    raw
}

#[test]
fn lint_and_check_strip_download_noise_and_log_the_invocation() {
    if !test_isolation::rtk_binary_is_built() {
        return;
    }

    let downloads = "Download https://example.invalid/fixture.ts\n".repeat(24);
    for (subcmd, message) in [("lint", "Checked 1 file"), ("check", "Check fixture.ts")] {
        let raw = format!("{downloads}\n\x1b[32m{message}\x1b[0m\n");
        let args = [subcmd, "source file.ts"];
        let (stdout, stderr) = run_fake_deno(&args, &raw, true);

        assert!(stdout.lines().any(|line| line == message), "{stdout:?}");
        assert!(!stdout.contains("Download "), "{stdout:?}");
        assert!(!stdout.contains('\x1b'), "{stdout:?}");
        let invocation = format!("Running: deno {subcmd} source file.ts");
        assert!(stderr.contains(&invocation), "{stderr:?}");
    }
}

#[test]
fn default_test_output_omits_individual_passing_cases() {
    if !test_isolation::rtk_binary_is_built() {
        return;
    }

    let raw = passing_test_output();
    let (stdout, _) = run_fake_deno(&["test", "fixture_test.ts"], &raw, false);
    assert!(
        !stdout.contains("passing_fixture_case_"),
        "passing test details were not filtered: {stdout:?}"
    );
}

#[test]
fn watch_reporter_compile_and_task_preserve_stdout() {
    if !test_isolation::rtk_binary_is_built() {
        return;
    }

    let watch_lint = format!(
        "{}\nChecked 1 file\n",
        "Download https://example.invalid/fixture.ts\n".repeat(24)
    );
    let watch_test = passing_test_output();
    let junit = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                 <testsuite name=\"fixture\" tests=\"1\"><testcase name=\"smoke\"/></testsuite>\n";
    let cases: &[(&[&str], &str)] = &[
        (&["lint", "--watch", "source file.ts"], &watch_lint),
        (&["test", "--watch", "fixture_test.ts"], &watch_test),
        (&["test", "--reporter=junit", "fixture_test.ts"], junit),
        (
            &["compile", "--output", "build artifact", "source file.ts"],
            "Compile source file.ts\nEmit build artifact (64 KB)\n",
        ),
        (
            &["task", "smoke", "two words"],
            "Download fixture\n\nTask complete\n",
        ),
    ];

    for &(args, raw) in cases {
        let (stdout, _) = run_fake_deno(args, raw, false);
        assert_eq!(stdout, raw, "passthrough stdout for deno {args:?}");
    }
}
