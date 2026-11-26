use super::*;

#[test]
fn gradle_command_preserves_argument_boundaries_and_tool_selection() {
    use std::ffi::OsStr;
    use std::path::Path;

    let args = ["assembleDebug", "-Pflavor=demo build", "--offline"];
    let command = new_gradle_command(&args.map(str::to_owned));
    assert_eq!(command.get_args().collect::<Vec<_>>(), args.map(OsStr::new));

    // Check that both helpers make the same choice without assuming a wrapper exists.
    match gradlew_binary() {
        "gradle" => {
            assert_eq!(
                command.get_program(),
                resolved_command("gradle").get_program()
            );
        }
        wrapper => {
            let filename = if cfg!(windows) {
                "gradlew.bat"
            } else {
                "gradlew"
            };
            assert_eq!(Path::new(wrapper).file_name(), Some(OsStr::new(filename)));
            assert_eq!(command.get_program(), OsStr::new(wrapper));
        }
    }
}

#[test]
fn build_stream_keeps_diagnostics_and_blank_lines_without_buffering() {
    let mut filter = BuildLineFilter;

    assert_eq!(filter.feed_line("> Task :compileJava"), None);
    assert_eq!(
        filter.feed_line("error: missing symbol").as_deref(),
        Some("error: missing symbol\n")
    );
    assert_eq!(filter.feed_line("").as_deref(), Some("\n"));
    assert_eq!(
        filter.feed_line("BUILD FAILED in 1s").as_deref(),
        Some("BUILD FAILED in 1s\n")
    );
    assert!(filter.flush().is_empty());
}

#[test]
fn filters_distinguish_quiet_success_from_unrecognized_diagnostics() {
    let success = "> Task :app:check\n[quiet] BUILD SUCCESSFUL in 2s\n";
    assert_eq!(
        filter_test(success),
        "ok ✓ (no test output — add testLogging to build.gradle for details)"
    );
    assert_eq!(filter_lint(success), "ok ✓ lint passed");
    assert_eq!(filter_dependencies(success), "ok ✓ no dependencies");
    assert_eq!(
        filter_connected(
            "Starting 1 test on emulator\n\
             INSTRUMENTATION_STATUS: numtests=1\n\
             INSTRUMENTATION_CODE: -1\n"
        ),
        "ok ✓ (connected tests passed)"
    );

    let diagnostic = "Gradle could not read settings.gradle";
    let unrecognized = format!("\n  {diagnostic}\n\t");
    assert_eq!(filter_test(&unrecognized), diagnostic);
    assert_eq!(filter_lint(&unrecognized), diagnostic);
    assert_eq!(filter_dependencies(&unrecognized), diagnostic);
}

#[test]
fn dependency_list_caps_details_but_counts_hidden_entries() {
    use std::fmt::Write as _;

    let total = CAP_LIST + 2;
    let mut input = String::from("runtimeClasspath - Runtime dependencies\n");
    for index in 0..total {
        writeln!(input, "+--- example:library-{index}:1.0").unwrap();
    }

    let output = filter_dependencies(&input);
    let mut lines = output.lines();
    assert_eq!(
        lines.next(),
        Some(format!("{total} top-level dependencies across 1 configurations").as_str())
    );
    assert_eq!(lines.next(), Some(""));
    assert_eq!(
        lines.next(),
        Some(format!("runtimeClasspath ({total}):").as_str())
    );
    for index in 0..CAP_LIST {
        assert_eq!(
            lines.next(),
            Some(format!("  example:library-{index}:1.0").as_str())
        );
    }
    assert_eq!(lines.next(), Some("  ... +2 more"));
    assert_eq!(lines.next(), None);
}

#[cfg(all(unix, feature = "engine"))]
#[test]
fn gradlew_cli_prefers_the_wrapper_and_routes_each_task() {
    use crate::core::test_isolation;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    fn write_tool(path: &Path, script: &str) {
        fs::write(path, script).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }

    if !test_isolation::rtk_binary_ready() {
        return;
    }

    let project = test_isolation::tempdir();
    let invoke = |args: &[&str]| {
        let mut cmd = test_isolation::rtk_command();
        let inherited_path = cmd
            .get_envs()
            .find(|(key, _)| *key == "PATH")
            .and_then(|(_, value)| value)
            .expect("isolated command should provide PATH");
        let path = std::env::join_paths(
            std::iter::once(project.path().to_path_buf())
                .chain(std::env::split_paths(inherited_path)),
        )
        .unwrap();
        let output = cmd
            .current_dir(project.path())
            .env("PATH", path)
            .arg("gradlew")
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "rtk gradlew {args:?} exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };

    write_tool(
        &project.path().join("gradle"),
        "#!/bin/sh\nprintf '%s\\n' '> Task :fallback' 'warning: system gradle selected'\n",
    );
    let fallback = invoke(&["assembleDebug"]);
    assert!(
        fallback.contains("warning: system gradle selected"),
        "{fallback}"
    );
    assert!(!fallback.contains("> Task :fallback"), "{fallback}");

    write_tool(
        &project.path().join("gradlew"),
        r#"#!/bin/sh
printf '%s\n' '> Task :coverageFixture'
case "$1" in
    assembleDebug)
        printf '%s\n' 'Calculating task graph' 'warning: project wrapper selected' 'BUILD SUCCESSFUL in 1s'
        if [ "$#" -gt 1 ]; then
            printf 'fixture flag: <%s>\n' "$2"
        fi
        ;;
    testDebugUnitTest)
        printf '%s\n' 'ExampleTest > passes PASSED' '1 test completed'
        ;;
    connectedDebugAndroidTest)
        printf '%s\n' 'Starting 1 test on emulator' 'INSTRUMENTATION_STATUS: numtests=1'
        ;;
    lintDebug)
        printf '%s\n' 'Wrote HTML report to fixture.html' 'Example.kt:12: Warning: Sample warning [SampleIssue]'
        ;;
    dependencies)
        printf '%s\n' 'runtimeClasspath - Runtime dependencies' '+--- example:direct:1.0' '|    +--- example:transitive:2.0'
        ;;
    signingReport)
        printf 'fixture property: <%s>\n' "$2"
        ;;
esac
"#,
    );

    for (task, kept, removed) in [
        (
            "assembleDebug",
            "warning: project wrapper selected",
            "Calculating task graph",
        ),
        ("testDebugUnitTest", "1 test completed", "passes PASSED"),
        (
            "connectedDebugAndroidTest",
            "ok ✓ (connected tests passed)",
            "INSTRUMENTATION_STATUS",
        ),
        ("lintDebug", "[SampleIssue]", "Wrote HTML report"),
        (
            "dependencies",
            "runtimeClasspath (1):",
            "example:transitive",
        ),
    ] {
        let stdout = invoke(&[task]);
        assert!(stdout.contains(kept), "{task}: {stdout}");
        assert!(!stdout.contains(removed), "{task}: {stdout}");
        assert!(
            !stdout.contains("> Task :coverageFixture"),
            "{task}: {stdout}"
        );
    }

    for (args, marker) in [
        (
            ["assembleDebug", "--full-stacktrace"],
            "fixture flag: <--full-stacktrace>",
        ),
        (
            ["signingReport", "-Pname=two words"],
            "fixture property: <-Pname=two words>",
        ),
    ] {
        let stdout = invoke(&args);
        assert!(
            stdout.contains("> Task :coverageFixture"),
            "{args:?}: {stdout}"
        );
        assert!(stdout.contains(marker), "{args:?}: {stdout}");
    }
}
