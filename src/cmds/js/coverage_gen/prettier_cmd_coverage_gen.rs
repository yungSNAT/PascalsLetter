use super::*;

#[test]
fn check_mode_lists_supported_extensions_and_counts_already_formatted_files() {
    let output = "\
Checking formatting...
  app.js
widget.jsx
settings.json
README.md
theme.css
layout.scss
[warn] ignored.js
[error] ignored.css
logo.svg
10 All matched files use Prettier code style!
";

    assert_eq!(
        filter_prettier_output(output),
        "\
Prettier: 6 files need formatting
1. app.js
2. widget.jsx
3. settings.json
4. README.md
5. theme.css
6. layout.scss

4 files already formatted"
    );
}

#[test]
fn check_mode_accepts_no_listed_files_without_an_explicit_summary() {
    assert_eq!(
        filter_prettier_output("Checking formatting...\n"),
        "Prettier: All files formatted correctly"
    );
}

#[test]
fn write_mode_reports_the_count_without_listing_files() {
    for status in ["modified", "formatted"] {
        let output = format!("app.js\nlayout.scss\n2 files {status}\n");
        assert_eq!(
            filter_prettier_output(&output),
            "Prettier: 2 files formatted"
        );
    }
}

#[cfg(unix)]
#[test]
fn verbose_cli_reports_the_invocation_and_filters_tool_output() {
    use crate::core::test_isolation;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    if !test_isolation::rtk_binary_ready() {
        return;
    }

    let scratch = test_isolation::tempdir();
    let script = r#"#!/bin/sh
for arg in "$@"; do
    if [ "$arg" = fixture.js ]; then
        printf '%s\n' 'Checking formatting...' 'All matched files use Prettier code style!'
        exit 0
    fi
done
exit 2
"#;
    for name in [
        "prettier", "npx", "npm", "pnpm", "pnpx", "yarn", "bun", "bunx",
    ] {
        let tool = scratch.path().join(name);
        fs::write(&tool, script).expect("write fake Prettier launcher");
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o700))
            .expect("make fake Prettier launcher executable");
    }

    let mut cmd = test_isolation::rtk_command();
    let inherited_path = cmd
        .get_envs()
        .find(|(name, _)| *name == "PATH")
        .and_then(|(_, value)| value)
        .expect("isolated rtk command should provide PATH");
    let path = std::env::join_paths(
        std::iter::once(scratch.path().to_path_buf()).chain(std::env::split_paths(inherited_path)),
    )
    .expect("prepend fake tools to PATH");

    let output = cmd
        .current_dir(scratch.path())
        .env("PATH", path)
        .args(["-v", "prettier", "fixture.js"])
        .output()
        .expect("run isolated rtk prettier");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{}: {stderr}", output.status);
    assert!(
        stderr
            .lines()
            .any(|line| line == "Running: prettier fixture.js"),
        "missing verbose invocation in stderr: {stderr}"
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "Prettier: All files formatted correctly"
    );
}
