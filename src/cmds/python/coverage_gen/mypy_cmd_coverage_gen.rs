use super::filter_mypy_output;

#[test]
fn groups_delayed_notes_and_formats_errors_without_codes() {
    let input = concat!(
        "setup.py:2: note: Configuration context\n",
        "module.py:12:5: error: Types do not match\n",
        "Checking annotations\n",
        "module.py:12: note: Expected an integer\n",
        "other.py:6: note: Unrelated context\n",
        "module.py:18: error: Missing annotation  [no-untyped-def]\n",
        "Found 2 errors in 1 file\n",
    );

    assert_eq!(
        filter_mypy_output(input),
        concat!(
            "setup.py:2: note: Configuration context\n",
            "other.py:6: note: Unrelated context\n\n",
            "mypy: 2 errors in 1 files\n",
            "module.py (2 errors)\n",
            "  L12: Types do not match\n",
            "    Expected an integer\n",
            "  L18: [no-untyped-def] Missing annotation",
        ),
    );
}

#[test]
fn ignores_progress_but_retains_standalone_notes_without_errors() {
    assert_eq!(
        filter_mypy_output("Checking annotations\n\n"),
        "mypy: No issues found",
    );
    assert_eq!(
        filter_mypy_output("Checking annotations\nmodule.py:3: note: Inferred type is int\n"),
        "module.py:3: note: Inferred type is int",
    );
}

#[cfg(unix)]
#[test]
fn cli_filters_diagnostics_and_preserves_python_fallback_failure() {
    use crate::core::test_isolation::{rtk_binary_is_built, rtk_command, tempdir};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    if !rtk_binary_is_built() {
        return;
    }

    let tools = tempdir();
    let mypy = tools.path().join("mypy");
    let python = tools.path().join("python3");
    for (file, script) in [
        (
            &mypy,
            r#"#!/bin/sh
[ "$#" -eq 1 ] && [ "$1" = 'typed module.py' ] || exit 97
printf '\033[31mmodule.py:7: error: Types differ  [assignment]\033[0m\n'
exit 1
"#,
        ),
        (
            &python,
            r#"#!/bin/sh
[ "$#" -eq 3 ] && [ "$1" = '-m' ] && [ "$2" = 'mypy' ] && [ "$3" = 'typed module.py' ] || exit 97
printf '  \033[31mSynthetic startup failure\033[0m  \n\n'
exit 2
"#,
        ),
    ] {
        fs::write(file, script).unwrap();
        fs::set_permissions(file, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let mut cmd = rtk_command();
    let inherited_path = cmd
        .get_envs()
        .find(|(key, _)| *key == "PATH")
        .and_then(|(_, value)| value)
        .expect("isolated command should provide PATH");
    let path = std::env::join_paths(
        std::iter::once(tools.path().to_path_buf()).chain(std::env::split_paths(inherited_path)),
    )
    .unwrap();
    cmd.env("PATH", &path)
        .current_dir(tools.path())
        .stdin(std::process::Stdio::null())
        .args(["-v", "mypy", "typed module.py"]);

    let output = cmd.output().expect("run rtk with fake mypy");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stdout}\n{stderr}");
    assert!(
        stdout.contains(
            "mypy: 1 errors in 1 files\nmodule.py (1 errors)\n  L7: [assignment] Types differ"
        ),
        "{stdout}",
    );
    assert!(!stdout.contains('\x1b'), "{stdout:?}");
    assert!(stderr.contains("Running: mypy typed module.py"), "{stderr}");

    fs::remove_file(mypy).unwrap();
    // Do not exercise the fallback if removing the fixture exposes a real mypy.
    if which::which_in("mypy", Some(&path), tools.path()).is_ok() {
        return;
    }

    let output = cmd.output().expect("run rtk with fake python3");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stdout}\n{stderr}");
    assert!(
        stdout
            .lines()
            .any(|line| line == "Synthetic startup failure"),
        "{stdout:?}",
    );
    assert!(!stdout.contains("mypy: No issues found"), "{stdout}");
}
