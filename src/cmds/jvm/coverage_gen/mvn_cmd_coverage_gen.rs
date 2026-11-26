use super::*;

#[test]
fn surefire_flushes_interrupted_class_before_committing_next_failure() {
    let raw = concat!(
        "[INFO] Running demo.InterruptedTest\n",
        "last message from the interrupted class\n",
        "[INFO] Running demo.FailingTest\n",
        "java.lang.AssertionError: unexpected count\n",
        "\tat org.junit.Assert.fail(Assert.java:42)\n",
        "\tat demo.FailingTest.count(FailingTest.java:9)\n",
        "[ERROR] Tests run: 1, Failures: 1, Errors: 0, Skipped: 0, Time elapsed: 0.01 s <<< FAILURE! -- in demo.FailingTest\n",
        "\n",
        "[INFO] BUILD FAILURE\n",
    );
    let expected = concat!(
        "[INFO] Running demo.InterruptedTest\n",
        "last message from the interrupted class\n",
        "[INFO] Running demo.FailingTest\n",
        "java.lang.AssertionError: unexpected count\n",
        "\tat demo.FailingTest.count(FailingTest.java:9)\n",
        "[ERROR] Tests run: 1, Failures: 1, Errors: 0, Skipped: 0, Time elapsed: 0.01 s <<< FAILURE! -- in demo.FailingTest\n",
        "\n",
        "[INFO] BUILD FAILURE\n",
    );

    assert_eq!(filter_surefire(raw, false), expected);
}

#[test]
fn compile_and_package_preserve_localized_output_minus_ansi() {
    let raw = "\x1b[36m[INFO]\x1b[0m Compiling 1 source file\r\n[INFO] BUILD ÉCHEC\r\n";
    let expected = "[INFO] Compiling 1 source file\r\n[INFO] BUILD ÉCHEC\r\n";

    assert_eq!(filter_compile(raw, false), expected);
    assert_eq!(filter_package(raw, false), expected);
}

#[test]
fn compile_module_boundaries_clear_pending_diagnostic_continuations() {
    let diagnostic = "[app] [ERROR] /Main.java:[7,9] cannot find symbol";
    let continuation = "  symbol: variable missing";
    let footer = "[INFO] BUILD FAILURE\n";

    for (boundary, kept) in [
        ("[app] [INFO] -----< demo:app >-----", true),
        ("[app] [INFO] Building app 1.0 [1/1]", true),
        ("[app] [INFO] Building the widget pipeline", false),
        ("[app] [ERROR] -> [Help 1]", false),
    ] {
        let raw = format!("{diagnostic}\n{continuation}\n{boundary}\n\torphaned detail\n{footer}");
        let mut expected = format!("{diagnostic}\n{continuation}\n");
        if kept {
            expected.push_str(boundary);
            expected.push('\n');
        }
        expected.push_str(footer);

        assert_eq!(filter_compile(&raw, true), expected, "boundary: {boundary}");
    }
}

#[test]
fn package_deduplicates_reactor_warnings_without_extending_an_error_block() {
    let raw = concat!(
        "[alpha] [INFO] -----< demo:alpha >-----\n",
        "[alpha] [WARNING] /Alpha.java:[4,2] uses deprecated API\n",
        "[beta] [ERROR] /Beta.java:[7,9] cannot find symbol\n",
        "  symbol: variable missing\n",
        "[beta] [WARNING] /Beta.java:[12,3] uses deprecated API\n",
        "\torphaned detail after warning\n",
        "[INFO] BUILD FAILURE\n",
    );
    let expected = concat!(
        "[alpha] [INFO] -----< demo:alpha >-----\n",
        "[alpha] [WARNING] /Alpha.java:[4,2] uses deprecated API\n",
        "[beta] [ERROR] /Beta.java:[7,9] cannot find symbol\n",
        "  symbol: variable missing\n",
        "[INFO] BUILD FAILURE\n",
    );

    assert_eq!(filter_package(raw, true), expected);
}

#[test]
fn quiet_module_tag_stripping_preserves_nonmodule_prefixes() {
    for line in [
        "\tat demo.ExampleTest.check(ExampleTest.java:7)",
        "[trace]",
        "[] [ERROR] empty tag",
        "[1, 2] != [1, 3]",
        "[ERROR] unprefixed diagnostic",
    ] {
        assert_eq!(strip_module_tag(line, true), line);
    }

    let tagged = "[app] [ERROR] Failed to execute goal";
    assert_eq!(
        strip_module_tag(tagged, true),
        "[ERROR] Failed to execute goal"
    );
    assert_eq!(strip_module_tag(tagged, false), tagged);
}

#[cfg(unix)]
#[test]
fn cli_filters_each_maven_route_using_isolated_tools() {
    use crate::core::test_isolation::{rtk_binary_ready, rtk_command, tempdir};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Stdio;

    if !rtk_binary_ready() {
        return;
    }

    let scratch = tempdir();
    let project = scratch.path().join("project");
    let bin = scratch.path().join("bin");
    fs::create_dir(&project).expect("create project directory");
    fs::create_dir(&bin).expect("create tool directory");

    for (path, script) in [
        (
            project.join("mvnw"),
            r#"#!/bin/sh
case "$#:$*" in
    1:test|1:compile|1:package)
        printf '%s\n' \
            "[INFO] Scanning for projects (fixture wrapper: $1)" \
            '[INFO] Running demo.PassingTest' \
            'passing test stdout' \
            '[INFO] Tests run: 1, Failures: 0, Errors: 0, Skipped: 0, Time elapsed: 0.01 s -- in demo.PassingTest' \
            '[INFO] Results:' \
            '[WARNING] fixture compiler warning' \
            '[INFO] Installing demo.jar' \
            '[INFO] BUILD SUCCESS'
        ;;
    *) exit 64 ;;
esac
"#,
        ),
        // A wrapper-selection regression must not invoke an installed Maven.
        (bin.join("mvn"), "#!/bin/sh\nexit 65\n"),
        (
            bin.join("mvnd"),
            r#"#!/bin/sh
case "$#:$*" in
    '2:test -q')
        printf '%s\n' \
            '[app] fixture daemon diagnostic' \
            '[app] [ERROR] Re-run Maven with the debug flag'
        ;;
    *) exit 64 ;;
esac
"#,
        ),
    ] {
        fs::write(&path, script).expect("write fake Maven tool");
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .expect("make fake Maven tool executable");
    }

    let cases: [(&str, &[&str], &str, &str); 4] = [
        (
            "mvn",
            &["test"],
            "[INFO] Results:",
            "fixture compiler warning",
        ),
        (
            "mvn",
            &["compile"],
            "[WARNING] fixture compiler warning",
            "[INFO] Installing",
        ),
        (
            "mvn",
            &["package"],
            "[WARNING] fixture compiler warning\n[INFO] Installing demo.jar",
            "demo.PassingTest",
        ),
        (
            "mvnd",
            &["test", "-q"],
            "[app] fixture daemon diagnostic",
            "Re-run Maven",
        ),
    ];

    for (tool, args, kept, dropped) in cases {
        let mut cmd = rtk_command();
        let inherited_path = cmd
            .get_envs()
            .find_map(|(key, value)| if key == "PATH" { value } else { None })
            .expect("isolated command should supply PATH");
        let path = std::env::join_paths(
            std::iter::once(bin.clone()).chain(std::env::split_paths(inherited_path)),
        )
        .expect("prepend fake tools to PATH");
        let output = cmd
            .env("PATH", path)
            .current_dir(&project)
            .stdin(Stdio::null())
            .arg(tool)
            .args(args)
            .output()
            .expect("run isolated rtk command");

        assert_eq!(
            output.status.code(),
            Some(0),
            "{tool} {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).expect("UTF-8 output from rtk");
        assert!(stdout.contains(kept), "{tool} {args:?}: {stdout}");
        assert!(!stdout.contains(dropped), "{tool} {args:?}: {stdout}");
        if tool == "mvn" {
            assert!(stdout.contains(&format!("(fixture wrapper: {})", args[0])));
            assert!(stdout.contains("[INFO] BUILD SUCCESS"));
        }
    }
}
