use super::*;

#[test]
fn value_grammars_reject_non_option_tokens() {
    for (kind, text) in [
        (TokenKind::Positional, "config"),
        (TokenKind::DashDash, "--"),
    ] {
        assert!(global_takes_value(kind, text).is_none());
        assert!(run_takes_value(kind, text).is_none());
    }
}

#[test]
fn config_value_that_looks_like_an_output_flag_does_not_block_json_injection() {
    let mut invocation = RunInvocation {
        global_args: vec!["--color=never".into()],
        run_args: vec!["--config".into(), "--output.json.path".into()],
    };

    assert_eq!(
        build_filtered_args(&invocation, 2),
        [
            "--color=never",
            "run",
            "--output.json.path",
            "stdout",
            "--config",
            "--output.json.path",
        ]
    );

    invocation
        .run_args
        .push("--output.json.path=report.json".into());
    assert_eq!(
        build_filtered_args(&invocation, 2),
        [
            "--color=never",
            "run",
            "--config",
            "--output.json.path",
            "--output.json.path=report.json",
        ]
    );
}

#[test]
fn command_preview_separates_arguments_without_trailing_space() {
    assert_eq!(format_command("golangci-lint", &[]), "golangci-lint");
    assert_eq!(
        format_command(
            "golangci-lint",
            &["run".into(), "--output.json.path".into(), "stdout".into()],
        ),
        "golangci-lint run --output.json.path stdout"
    );
}

#[test]
fn json_summary_caps_file_groups_but_counts_all_issues() {
    let file_count = CAP_WARNINGS + 2;
    let issue_count = file_count + 1;
    let issues: Vec<_> = (0..file_count)
        .chain(std::iter::once(0))
        .map(|index| {
            serde_json::json!({
                "FromLinter": "errcheck",
                "Text": "unchecked error",
                "Pos": {"Filename": format!("file-{index}.go"), "Line": 1, "Column": 1}
            })
        })
        .collect();

    let report = serde_json::json!({"Issues": issues}).to_string();
    let summary = filter_golangci_json(&report, 2);
    let header = format!(
        "golangci-lint: {issue_count} issues in {file_count} files\n\
         Top linters:\n  errcheck ({issue_count}x)\n\n\
         Top files:\n  file-0.go (2 issues)\n    errcheck (2)\n"
    );

    assert!(summary.starts_with(&header), "{summary}");
    assert_eq!(
        summary
            .lines()
            .filter(|line| line.starts_with("  file-") && line.ends_with(" issues)"))
            .count(),
        CAP_WARNINGS
    );
    assert!(summary.ends_with("\n\n... +2 more files"), "{summary}");
}

#[cfg(unix)]
mod cli {
    use crate::core::test_isolation;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::process::Command;

    fn install_linter(dir: &Path, script: &str) {
        let tool = dir.join("golangci-lint");
        fs::write(&tool, script).unwrap();
        fs::set_permissions(tool, fs::Permissions::from_mode(0o700)).unwrap();
    }

    fn isolated_command(dir: &Path) -> Command {
        let mut cmd = test_isolation::rtk_command();
        let inherited_path = cmd
            .get_envs()
            .find(|(key, _)| *key == "PATH")
            .and_then(|(_, value)| value)
            .expect("isolated rtk command should provide PATH");
        let path = std::env::join_paths(
            std::iter::once(dir.to_path_buf()).chain(std::env::split_paths(inherited_path)),
        )
        .unwrap();
        cmd.env("PATH", path).current_dir(dir);
        cmd
    }

    #[test]
    fn cli_reads_v2_version_from_stderr_and_filters_only_the_json_line() {
        if !test_isolation::rtk_binary_is_built() {
            return;
        }

        let dir = test_isolation::tempdir();
        let issue = serde_json::json!({
            "FromLinter": "errcheck",
            "Text": "unchecked error",
            "Pos": {"Filename": "sample.go", "Line": 1, "Column": 1},
            "SourceLines": ["    defer f.Close()"]
        });
        let report = serde_json::json!({"Issues": vec![issue; 24]});
        install_linter(
            dir.path(),
            &format!(
                r#"#!/bin/sh
if [ "$#" -eq 1 ] && [ "$1" = "--version" ]; then
    printf ' \n'
    printf '%s\n' 'golangci-lint has version v2.13.2' >&2
    exit 0
fi
if [ "$#" -eq 4 ] && [ "$1" = run ] && [ "$2" = "--output.json.path" ] &&
   [ "$3" = stdout ] && [ "$4" = sample ]; then
    printf '%s\n' '{report}' 'fake trailing report text'
    exit 1
fi
printf 'unexpected linter arguments: %s\n' "$*" >&2
exit 99
"#
            ),
        );

        let output = isolated_command(dir.path())
            .args(["-v", "golangci-lint", "run", "sample"])
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert_eq!(output.status.code(), Some(0), "{stderr}");
        assert!(
            stderr
                .lines()
                .any(|line| line == "Running: golangci-lint run --output.json.path stdout sample"),
            "{stderr}"
        );
        assert!(
            stdout.contains("golangci-lint: 24 issues in 1 files"),
            "{stdout}"
        );
        assert!(stdout.contains("→ defer f.Close()"), "{stdout}");
        assert!(!stdout.contains("fake trailing report text"), "{stdout}");
    }

    #[test]
    fn cli_falls_back_to_v1_arguments_when_the_version_probe_cannot_spawn() {
        if !test_isolation::rtk_binary_is_built() {
            return;
        }

        let dir = test_isolation::tempdir();
        // PATH lookup succeeds, but the kernel cannot execute the interpreter.
        install_linter(dir.path(), "#!/dev/null\n");

        let output = isolated_command(dir.path())
            .args(["-v", "golangci-lint", "run", "sample"])
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(!output.status.success(), "{stderr}");
        assert!(
            stderr
                .lines()
                .any(|line| line == "Running: golangci-lint run --out-format=json sample"),
            "{stderr}"
        );
    }

    #[test]
    fn cli_passes_through_non_run_arguments_output_and_exit_code() {
        if !test_isolation::rtk_binary_is_built() {
            return;
        }

        let dir = test_isolation::tempdir();
        install_linter(
            dir.path(),
            r#"#!/bin/sh
if [ "$#" -eq 2 ] && [ "$1" = linters ] && [ "$2" = "--json" ]; then
    printf '%s\n' 'fake unfiltered linter list'
    exit 7
fi
printf 'unexpected linter arguments: %s\n' "$*" >&2
exit 99
"#,
        );

        let output = isolated_command(dir.path())
            .args(["golangci-lint", "linters", "--json"])
            .output()
            .unwrap();

        assert_eq!(
            output.status.code(),
            Some(7),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout.as_slice(), b"fake unfiltered linter list\n");
    }
}
