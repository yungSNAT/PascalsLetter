use super::*;

#[test]
fn unparsed_failure_omits_blank_lines_without_eliding_diagnostics() {
    let handler = TscHandler::new();
    let header = "TypeScript: compiler exited with code 2, but RTK parsed no diagnostics\n";
    let short = "\x1b[31mCould not load project\x1b[0m\n\t \n\x1b[0m\nConfiguration was not read\n";

    assert_eq!(
        handler.format_summary(2, short).unwrap(),
        format!("{header}Could not load project\nConfiguration was not read\n")
    );

    // Cross the tee threshold without exceeding the cap on non-empty lines.
    let details: String = (1..=MAX_UNPARSED_LINES)
        .map(|i| format!("compiler detail {i}\n"))
        .collect();
    let padding = " ".repeat(crate::core::tee::MIN_TEE_SIZE);
    let raw = format!("{padding}\n{details}");

    assert_eq!(
        handler.format_summary(2, &raw).unwrap(),
        format!("{header}{details}")
    );
}

#[test]
fn buffered_output_without_diagnostics_or_a_zero_error_marker_reports_completion() {
    for raw in ["", "Starting compilation in watch mode...\n"] {
        assert_eq!(filter_tsc_output(raw), "TypeScript compilation completed");
    }
}

#[cfg(unix)]
#[test]
fn verbose_tsc_forwards_arguments_to_the_direct_tool_or_package_runner() {
    use crate::core::test_isolation::{rtk_binary_ready, rtk_command, tempdir};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    if !rtk_binary_ready() {
        return;
    }

    let scratch = tempdir();
    let bin = scratch.path().join("bin");
    fs::create_dir(&bin).unwrap();

    let npx_prefix = r#"
while [ "$#" -gt 0 ]; do
    case "$1" in
        --yes|-y|--) shift ;;
        *) break ;;
    esac
done
[ "$1" = tsc ] || exit 0
shift
"#;
    let compiler_body = r#"
if [ "$#" -eq 3 ] && [ "$1" = '--noEmit' ] && [ "$2" = '--project' ] && [ "$3" = 'project with spaces.json' ]; then
    printf '%s\n' "coverage.ts(4,2): error TS2322: invoked ${0##*/}" 'Found 1 error.'
fi
"#;

    for (tool, via, prefix) in [("npx", "npx tsc", npx_prefix), ("tsc", "tsc", "")] {
        let script = bin.join(tool);
        fs::write(&script, format!("#!/bin/sh\n{prefix}{compiler_body}")).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();

        let mut cmd = rtk_command();
        let inherited = cmd
            .get_envs()
            .find_map(|(key, value)| if key == "PATH" { value } else { None })
            .expect("isolated rtk command should supply PATH");
        // Keep the fallback deterministic even on machines with TypeScript installed.
        let inherited_dirs = std::env::split_paths(inherited).filter(|dir| {
            dir.is_absolute()
                && !["tsc", "npx", "bunx", "bun", "npm", "pnpm", "pnpx", "yarn"]
                    .iter()
                    .any(|name| dir.join(name).exists())
        });
        let path =
            std::env::join_paths(std::iter::once(bin.clone()).chain(inherited_dirs)).unwrap();

        let output = cmd
            .current_dir(scratch.path())
            .env("PATH", path)
            .env("RTK_TEE", "0")
            .args([
                "-v",
                "tsc",
                "--",
                "--noEmit",
                "--project",
                "project with spaces.json",
            ])
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(0), "{stdout}\n{stderr}");

        let invocation = format!("Running: {via} --noEmit --project project with spaces.json");
        assert!(stderr.lines().any(|line| line == invocation), "{stderr}");
        let diagnostic = format!("coverage.ts(4,2): error TS2322: invoked {tool}");
        assert!(stdout.contains(&diagnostic), "{stdout}");
        assert!(
            stdout.contains("TypeScript: 1 errors in 1 files"),
            "{stdout}"
        );
        assert!(!stdout.contains("Found 1 error."), "{stdout}");
    }
}
