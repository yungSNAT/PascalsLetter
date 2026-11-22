use super::filter_npm_output;

#[test]
fn strips_progress_without_dropping_longer_lines_with_ellipses() {
    let output = "⸨ fetching\nfetching ⸩\nwait...\n1234567...\nbuild still running...\ndone\n";
    assert_eq!(
        filter_npm_output(output),
        "1234567...\nbuild still running...\ndone"
    );
}

#[cfg(unix)]
mod cli {
    use crate::core::test_isolation;
    use std::fs;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::process::{Command, Output, Stdio};

    fn install_tool(dir: &Path, name: &str, script: &str) {
        let path = dir.join(name);
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    }

    fn command_with_tools(dir: &Path) -> Command {
        let mut cmd = test_isolation::rtk_command();
        let inherited_path = cmd
            .get_envs()
            .find(|(key, _)| *key == "PATH")
            .and_then(|(_, value)| value)
            .expect("isolated rtk command should have a PATH");
        let path = std::env::join_paths(
            std::iter::once(dir.to_path_buf()).chain(std::env::split_paths(inherited_path)),
        )
        .unwrap();
        cmd.env("PATH", path)
            .env_remove("SKIP_ENV_VALIDATION")
            .current_dir(dir);
        cmd
    }

    fn assert_stdout(output: &Output, expected: &str) {
        assert!(
            output.status.success(),
            "rtk exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim_end(), expected);
    }

    #[test]
    fn routes_npm_scripts_and_package_runners_through_the_filter() {
        if !test_isolation::rtk_binary_is_built() {
            return;
        }

        let scratch = test_isolation::tempdir();
        let dir = scratch.path();
        install_tool(
            dir,
            "npm",
            r#"#!/bin/sh
printf '%s\n' 'npm notice fixture boilerplate'
printf 'npm argument=<%s>\n' "$@"
printf 'skip-env=<%s>\n' "${SKIP_ENV_VALIDATION-unset}"
"#,
        );
        install_tool(
            dir,
            "npx",
            r#"#!/bin/sh
printf '%s\n' 'npm WARN fixture boilerplate'
printf 'npx argument=<%s>\n' "$@"
"#,
        );
        install_tool(
            dir,
            "bunx",
            r#"#!/bin/sh
IFS= read -r payload || payload=missing
printf '%s\n' 'npm notice fixture boilerplate'
printf 'bunx argument=<%s>\n' "$@"
printf 'stdin=<%s>\n' "$payload"
"#,
        );

        let npm = command_with_tools(dir)
            .args(["-v", "npm", "--skip-env", "coverage-build", "two words"])
            .output()
            .unwrap();
        assert_stdout(
            &npm,
            "npm argument=<run>\nnpm argument=<coverage-build>\nnpm argument=<two words>\nskip-env=<1>",
        );
        let stderr = String::from_utf8_lossy(&npm.stderr);
        assert!(
            stderr
                .lines()
                .any(|line| line.starts_with("Running: npm run coverage-build")),
            "missing verbose npm invocation in {stderr:?}"
        );

        let npx = command_with_tools(dir)
            .args(["npx", "cowsay", "hello"])
            .output()
            .unwrap();
        assert_stdout(&npx, "npx argument=<cowsay>\nnpx argument=<hello>");

        let mut bunx = command_with_tools(dir)
            .args(["bunx", "cowsay", "hello"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        bunx.stdin
            .take()
            .unwrap()
            .write_all(b"piped fixture input\n")
            .unwrap();
        let bunx = bunx.wait_with_output().unwrap();
        assert_stdout(
            &bunx,
            "bunx argument=<cowsay>\nbunx argument=<hello>\nstdin=<piped fixture input>",
        );
    }
}
