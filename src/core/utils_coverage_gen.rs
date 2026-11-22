use super::*;

#[test]
fn decode_backslashes_preserves_backspace_form_feed_and_vertical_tab() {
    for ansi_c in [false, true] {
        assert_eq!(
            decode_backslashes(r"one\btwo\fthree\vfour", ansi_c).as_deref(),
            Some("one\x08two\x0cthree\x0bfour")
        );
    }
}

#[test]
fn format_usd_displays_nonfinite_amounts_as_zero() {
    for amount in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(format_usd(amount), "$0.00", "amount: {amount}");
    }
}

#[cfg(unix)]
#[test]
fn exit_code_helpers_handle_normal_signaled_and_stopped_statuses() {
    use std::os::unix::process::ExitStatusExt;
    use std::process::{ExitStatus, Output};

    // A stopped wait status has neither an exit code nor a terminating signal.
    let stopped = (libc::SIGSTOP << 8) | 0x7f;
    for (raw, expected) in [
        (42 << 8, 42),
        (libc::SIGTERM, 128 + libc::SIGTERM),
        (stopped, 1),
    ] {
        let output = Output {
            status: ExitStatus::from_raw(raw),
            stdout: Vec::new(),
            stderr: Vec::new(),
        };
        assert_eq!(exit_code_from_output(&output, "fixture"), expected);
        assert_eq!(exit_code_from_status(&output.status, "fixture"), expected);
    }
}

#[test]
fn package_runner_uses_lockfile_precedence_but_respects_explicit_choices() {
    let project = test_isolation::tempdir();
    let _scope = test_isolation::enter(project.path());
    let pnpm_lock = project.path().join("pnpm-lock.yaml");
    fs::write(&pnpm_lock, "").expect("write pnpm lockfile");
    fs::write(project.path().join("yarn.lock"), "").expect("write Yarn lockfile");
    fs::write(project.path().join("bun.lock"), "").expect("write Bun lockfile");

    assert_eq!(detect_package_manager_in(project.path()), "pnpm");
    assert_eq!(exec_runner(None, MissingTool::Fail), "pnpm");

    let tool = "project-tool";
    let cases: [(_, _, &[_]); 4] = [
        ("pnpm", "pnpm", &["exec", "--", tool]),
        ("yarn", "yarn", &["exec", "--", tool]),
        ("bun", "bunx", &[tool]),
        ("bunx", "bunx", &[tool]),
    ];
    for (runner, program, args) in cases {
        let command = tool_exec(Some(runner), tool, MissingTool::Fail);
        let expected_program = resolve_binary(program).unwrap_or_else(|_| PathBuf::from(program));
        let expected_args: Vec<_> = args.iter().map(OsStr::new).collect();
        assert_eq!(
            command.get_program(),
            expected_program.as_os_str(),
            "runner: {runner}"
        );
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            expected_args,
            "runner: {runner}"
        );
    }

    fs::remove_file(&pnpm_lock).expect("remove pnpm lockfile");
    assert_eq!(detect_package_manager_in(project.path()), "yarn");
    assert_eq!(exec_runner(None, MissingTool::Fail), "yarn");
}

#[test]
fn msys_rewrite_detection_distinguishes_special_and_literal_arguments() {
    for arg in [
        "",
        "a b",
        "a\tb",
        "a\u{2003}b",
        r"a\b",
        r#"a"b"#,
        "a'b",
        "*.rs",
        "a?b",
        "a[b",
        "a]b",
        "a{b",
        "a}b",
        "~",
        "@args",
    ] {
        assert!(msys_would_rewrite(arg), "should quote {arg:?}");
    }

    for arg in ["--help", "résumé", "%PATH%", "$HOME", "a;b"] {
        assert!(!msys_would_rewrite(arg), "should leave {arg:?} alone");
    }
}

#[test]
fn resolved_command_keeps_the_requested_program_when_resolution_fails() {
    let project = test_isolation::tempdir();
    let missing = project.path().join("missing-tool");
    let name = missing.to_str().expect("UTF-8 fixture path");

    assert!(resolve_binary(name).is_err());
    assert_eq!(resolved_command(name).get_program(), missing.as_os_str());
}

#[test]
fn composer_tool_paths_honor_overrides_and_keep_one_default() {
    let project = test_isolation::tempdir();
    let _scope = test_isolation::enter(project.path());
    let config = project.path().join("composer.json");
    let tool = "phpunit";
    let default = PathBuf::from("vendor").join("bin").join(tool);

    fs::write(&config, r#"{"config":{"bin-dir":"project-tools"}}"#).expect("write Composer config");

    user_env::with_vars(&[("COMPOSER_BIN_DIR", Some("  env-tools  "))], || {
        assert_eq!(
            composer_tool_paths(tool),
            [PathBuf::from("env-tools").join(tool), default.clone()]
        );
    });

    user_env::with_vars(&[("COMPOSER_BIN_DIR", None)], || {
        assert_eq!(
            composer_tool_paths(tool),
            [PathBuf::from("project-tools").join(tool), default.clone()]
        );

        for contents in [
            r#"{"config":{"bin-dir":"   "}}"#,
            r#"{"config":{"bin-dir":"vendor/bin"}}"#,
        ] {
            fs::write(&config, contents).expect("update Composer config");
            assert_eq!(
                composer_tool_paths(tool).as_slice(),
                std::slice::from_ref(&default),
                "config: {contents}"
            );
        }
    });
}

#[test]
fn undefined_oem_byte_falls_back_to_lossy_utf8_for_only_its_line() {
    // CP857 leaves 0xD5 undefined but decodes 0x87 as ç.
    assert_eq!(
        decode_mixed(b"caf\xc3\xa9\xd5\r\n\x87\n", Some(857)),
        "café\u{fffd}\r\nç\n"
    );
}
