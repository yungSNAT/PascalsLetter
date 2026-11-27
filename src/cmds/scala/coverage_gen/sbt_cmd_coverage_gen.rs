use super::*;

const REPORT_RULE: &str = "═══════════════════════════════════════";

#[test]
fn scoped_tasks_are_distinguished_from_flags_and_bare_tasks() {
    for task in ["Test/test", "it:test", "exampleJVM/Compile/run"] {
        assert!(is_scoped_task(task), "{task}");
    }

    for arg in ["test", "-Dscala.home=local/cache", "--debug:tasks"] {
        assert!(!is_scoped_task(arg), "{arg}");
    }
}

#[test]
fn other_requires_a_subcommand() {
    assert_eq!(
        run_other(&[], 0).unwrap_err().to_string(),
        "sbt: no subcommand specified"
    );
}

#[test]
fn test_success_reports_pending_counts_and_timing_with_or_without_suites() {
    let input = concat!(
        "[info] Run completed in 1 second.\n",
        "[info] Tests: succeeded 8, failed 0, canceled 0, ignored 2, pending 1\n",
    );
    assert_eq!(
        filter_sbt_test(input),
        "sbt test: 8 passed, 2 ignored, 1 pending (1s)"
    );
    assert_eq!(
        filter_sbt_test(&format!("{input}[info] Suites: completed 3, aborted 0\n")),
        "sbt test: 8 passed, 2 ignored, 1 pending (3 suites, 1s)"
    );
}

#[test]
fn test_failure_keeps_useful_details_and_separates_suite_names_from_errors() {
    let input = concat!(
        "[info] - rejects stale data *** FAILED ***\n",
        "[info]     expected a cache miss\n",
        "[info]     at com.example.CacheSpec.run(CacheSpec.scala:41)\n",
        "[info]     -> at CacheSpec.scala:41\n",
        "[error] Failed tests:\n",
        "[info]     this is no longer part of the failure detail\n",
        "[error] com.example.CacheSpec\n",
        "[error] test fork stopped unexpectedly\n",
        "[error] (Test / test) sbt.TestsFailedException: Tests unsuccessful\n",
        "[info] Run completed in 4 seconds.\n",
        "[info] Tests: succeeded 9, failed 1, canceled 2, ignored 3, pending 0\n",
    );
    let expected = [
        "sbt test: 9 passed, 1 failed, 2 canceled, 3 ignored (4s)",
        REPORT_RULE,
        "  [FAIL] rejects stale data",
        "         expected a cache miss",
        "         -> at CacheSpec.scala:41",
        "",
        "  com.example.CacheSpec",
        "",
        "  test fork stopped unexpectedly",
    ]
    .join("\n");
    assert_eq!(filter_sbt_test(input), expected);
}

#[test]
fn test_output_without_a_summary_reports_errors_or_preserves_raw_text() {
    let input = concat!(
        "[info] loading project definition\n",
        "[error] could not load build definition\n",
        "[error] reference to undefined setting\n",
        "[error] Total time: 1 s\n",
    );
    assert_eq!(
        filter_sbt_test(input),
        format!(
            "sbt test: errors\n{REPORT_RULE}\n  could not load build definition\n  reference to undefined setting"
        )
    );

    let unrecognized = "\n[info] A custom test reporter is active.\n  No summary was emitted.\n";
    assert_eq!(filter_sbt_test(unrecognized), unrecognized);
}

#[test]
fn compile_error_report_caps_the_numbered_list_but_counts_every_error() {
    use std::fmt::Write as _;

    let mut input = String::from("[info] Compiling 3 Scala sources\n");
    for i in 1..=32 {
        writeln!(&mut input, "[error] missing value name{i}").unwrap();
    }

    let output = filter_sbt_compile(&input);
    let mut lines = output.lines();
    assert_eq!(lines.next(), Some("sbt compile: 32 errors"));
    assert_eq!(lines.next(), Some(REPORT_RULE));
    for i in 1..=30 {
        assert_eq!(
            lines.next(),
            Some(format!("{i}. missing value name{i}").as_str())
        );
    }
    assert_eq!(lines.next(), Some(""));
    assert_eq!(lines.next(), Some("... +2 more errors"));
    assert_eq!(lines.next(), None);
}

#[test]
fn compile_without_source_counts_distinguishes_timed_success_from_fallback() {
    assert_eq!(
        filter_sbt_compile("[success] Total time: 2 s, completed Jan 15, 2025\n"),
        "sbt compile: Success (2s)"
    );
    assert_eq!(
        filter_sbt_compile("[info] All classes are up to date.\n"),
        "sbt compile: Success"
    );
}

#[test]
fn run_keeps_program_output_and_diagnostics_but_removes_build_preamble() {
    let input = concat!(
        "\n \t\n",
        "[info] loading settings for project demo\n",
        "[info] Compiling 3 Scala sources\n",
        "[info] Running demo.Main\n",
        "[info] Starting import\n",
        "\n",
        "  raw program output  \n",
        "[error] Could not read optional settings\n",
        "[success] Import finished\n",
        "[success] Total time: 2 s\n",
    );
    assert_eq!(
        filter_sbt_run(input),
        "Starting import\n\nraw program output\nCould not read optional settings\nImport finished"
    );
}

#[cfg(unix)]
#[test]
fn cli_forwards_sbt_arguments_and_filters_only_supported_tasks() {
    use crate::core::test_isolation::{rtk_binary_ready, rtk_command, tempdir};
    use std::{fs, os::unix::fs::PermissionsExt, process::Stdio};

    if !rtk_binary_ready() {
        return;
    }

    let dir = tempdir();
    let sbt = dir.path().join("sbt");
    fs::write(
        &sbt,
        r#"#!/bin/sh
printf '%s\n' "$@" > sbt-argv.txt
printf '%s\n' \
    '[info] welcome to sbt 1.9.7' \
    '[info] loading settings for project demo from build.sbt ...' \
    '[info] loading project definition from the demo project directory' \
    '[info] set current project to demo (in build file for the demo project)'
case "$1" in
    test|it:test|testOnly)
        printf '%s\n' '[info] Tests: succeeded 7, failed 0, canceled 0, ignored 0, pending 0'
        ;;
    compile)
        printf '%s\n' '[info] Compiling 3 Scala sources' '[success] Total time: 2 s'
        ;;
    exampleJVM/run)
        printf '%s\n' '[info] running demo.Main' '[info] Hello from the program'
        ;;
    projects)
        printf '%s\n' '[info] Project list is unfiltered'
        ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&sbt, fs::Permissions::from_mode(0o700)).unwrap();

    let cases = [
        (&["test"][..], "test\n", "sbt test: 7 passed"),
        (&["compile"], "compile\n", "sbt compile: 3 sources (2s)"),
        (
            &["run", "exampleJVM/run", "two words"],
            "exampleJVM/run\ntwo words\n",
            "Hello from the program",
        ),
        (
            &["it:test", "clean"],
            "it:test\nclean\n",
            "sbt test: 7 passed",
        ),
        (
            &["testOnly", "demo.ExampleSpec"],
            "testOnly\ndemo.ExampleSpec\n",
            "sbt test: 7 passed",
        ),
        (
            &["projects"],
            "projects\n",
            "[info] Project list is unfiltered",
        ),
    ];

    for (args, expected_argv, expected_line) in cases {
        let mut command = rtk_command();
        let path = {
            let inherited = command
                .get_envs()
                .find_map(|(key, value)| if key == "PATH" { value } else { None })
                .expect("isolated rtk command should provide PATH");
            std::env::join_paths(
                std::iter::once(dir.path().to_path_buf()).chain(std::env::split_paths(inherited)),
            )
            .unwrap()
        };
        let output = command
            .env("PATH", path)
            .current_dir(dir.path())
            .stdin(Stdio::null())
            .args(["-v", "sbt"])
            .args(args)
            .output()
            .unwrap();

        let stdout = String::from_utf8(output.stdout).unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}\n{stderr}",
            output.status
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("sbt-argv.txt")).unwrap(),
            expected_argv,
            "{args:?}"
        );
        assert!(
            stdout.lines().any(|line| line == expected_line),
            "{args:?}: expected {expected_line:?} in {stdout:?}"
        );

        let passthrough = args[0] == "projects";
        assert_eq!(stdout.contains("[info] loading settings"), passthrough);
        if !passthrough {
            let task = expected_argv.lines().next().unwrap();
            assert!(
                stderr.contains(&format!("Running: sbt {task}")),
                "{args:?}: {stderr}"
            );
        }
    }
}
