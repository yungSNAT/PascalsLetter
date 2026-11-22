#[cfg(all(unix, feature = "engine"))]
#[test]
fn pint_injects_json_only_for_default_formatting() {
    use crate::core::test_isolation::{rtk_binary_is_built, rtk_command, tempdir};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    if !rtk_binary_is_built() {
        return;
    }

    const JSON: &str = r#"{"files":[{"name":"job.php","appliedFixers":["concat_space"]}]}"#;
    const SUMMARY: &str = "pint: 1 changes in 1 files";

    let dir = tempdir();
    let tool = dir.path().join("pint");
    // Return identical output in every mode so the assertions distinguish
    // Pint's JSON summary from the passthrough filter.
    fs::write(
        &tool,
        format!(
            r#"#!/bin/sh
case "$#:$1:$2:$3" in
    '2:--format=json:source file.php:'|'3:source file.php:--format:json'|'1:--version::')
        printf '%s\n' '{JSON}'
        ;;
    *)
        printf 'unexpected Pint arguments (%s): %s\n' "$#" "$*" >&2
        exit 90
        ;;
esac
"#
        ),
    )
    .unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();

    for (args, summarize) in [
        (&["source file.php"][..], true),
        (&["--format=json", "source file.php"][..], false),
        (&["source file.php", "--format", "json"][..], false),
        (&["--version"][..], false),
    ] {
        let mut cmd = rtk_command();
        let inherited_path = cmd
            .get_envs()
            .find(|(key, _)| *key == "PATH")
            .and_then(|(_, value)| value)
            .expect("isolated rtk command should provide PATH");
        let path = std::env::join_paths(
            std::iter::once(dir.path().to_path_buf()).chain(std::env::split_paths(inherited_path)),
        )
        .unwrap();

        cmd.current_dir(dir.path()).env("PATH", path);
        if summarize {
            cmd.arg("-v");
        }
        let output = cmd.arg("pint").arg("--").args(args).output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "pint {args:?} returned {}:\nstdout: {stdout}\nstderr: {stderr}",
            output.status
        );

        if summarize {
            assert!(stdout.contains(SUMMARY), "{stdout}");
            assert!(stdout.contains("job.php (1)\n  - concat_space"), "{stdout}");
            assert!(!stdout.contains(JSON), "{stdout}");
            assert!(stderr.contains("Running: pint source file.php"), "{stderr}");
        } else {
            assert!(stdout.contains(JSON), "pint {args:?}: {stdout}");
            assert!(!stdout.contains(SUMMARY), "pint {args:?}: {stdout}");
        }
    }
}
