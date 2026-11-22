use super::*;

#[test]
fn json_reports_global_errors_before_diagnostics_for_short_paths() {
    let input = r#"{
        "totals": {"errors": 2, "file_errors": 2},
        "errors": ["Unknown configuration key", "Could not load bootstrap file"],
        "files": {
            "src\\Service.php": {
                "errors": 1,
                "messages": [{"line": 17, "message": "Missing return type"}]
            },
            "Bootstrap.php": {
                "errors": 1,
                "messages": [{"line": 3, "message": "Undefined constant"}]
            }
        }
    }"#;

    assert_eq!(
        filter_phpstan_json(input),
        concat!(
            "phpstan: 2 errors in 2 files\n\n",
            "Global errors:\n",
            "  Unknown configuration key\n",
            "  Could not load bootstrap file\n\n\n",
            "Bootstrap.php (1 errors)\n",
            "  :3 Undefined constant\n\n",
            "Service.php (1 errors)\n",
            "  :17 Missing return type",
        )
    );
}

#[test]
fn text_prioritizes_shell_failures_and_limits_them_to_twenty_lines() {
    let short = " \x1b[31mphpstan: command not found\x1b[0m\n[OK] No errors\n";
    assert_eq!(
        filter_phpstan_text(short),
        "PHPStan error:\nphpstan: command not found\n[OK] No errors"
    );

    let failure = "sh: phpstan unavailable";
    let details: Vec<_> = (1..=22).map(|line| format!("detail {line:02}")).collect();
    let input = format!("{failure}\n{}", details.join("\n"));
    assert_eq!(
        filter_phpstan_text(&input),
        format!(
            "PHPStan error:\n{failure}\n{}\n... (3 more lines)",
            details[..19].join("\n")
        )
    );
}

#[test]
fn text_prefers_the_latest_summary_and_keeps_the_tail_without_one() {
    let summarized = "[OK] No errors\n  \x1b[31m[ErRoR] 2 errors IN 1 file\x1b[0m\n";
    assert_eq!(
        filter_phpstan_text(summarized),
        "PHPStan: [ErRoR] 2 errors IN 1 file"
    );

    let lines: Vec<_> = (1..=25)
        .map(|line| format!("diagnostic {line:02}"))
        .collect();
    let output = filter_phpstan_text(&lines.join("\n"));
    assert!(output.contains(&lines[5..].join("\n")), "{output:?}");
    for omitted in &lines[..5] {
        assert!(!output.contains(omitted), "{output:?}");
    }
}

#[cfg(unix)]
#[test]
fn cli_passes_through_utilities_and_selects_the_requested_analysis_format() {
    use crate::core::test_isolation::{rtk_binary_is_built, rtk_command, tempdir};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Stdio;

    if !rtk_binary_is_built() {
        return;
    }

    let dir = tempdir();
    let tool = dir.path().join("phpstan");
    fs::write(
        &tool,
        r#"#!/bin/sh
case "$#:$*" in
    "1:list")
        printf '%s\n' 'fixture utility output'
        exit 7
        ;;
    "4:analyse src --error-format json"|"3:analyze src --error-format=json")
        printf '%s\n' '{"totals":{"errors":0,"file_errors":0},"files":{}}'
        ;;
    "3:analyse src --error-format=table")
        printf '%s\n' 'fixture table detail' '[ERROR] Found 2 errors'
        exit 1
        ;;
    *)
        printf 'Unexpected fixture arguments: %s\n' "$*" >&2
        exit 99
        ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o700)).unwrap();

    let cases: &[(&[&str], i32, &str, &str)] = &[
        (
            &["list"],
            7,
            "fixture utility output",
            "Running: phpstan list (passthrough)",
        ),
        (
            &["analyse", "src"],
            0,
            "phpstan: ok",
            "Running: phpstan analyse src",
        ),
        (
            &["analyze", "src", "--error-format=json"],
            0,
            "phpstan: ok",
            "Running: phpstan analyze src --error-format=json",
        ),
        (
            &["analyse", "src", "--error-format=table"],
            1,
            "PHPStan: [ERROR] Found 2 errors",
            "Running: phpstan analyse src --error-format=table",
        ),
    ];

    for &(args, code, expected_stdout, expected_log) in cases {
        let mut cmd = rtk_command();
        let inherited_path = cmd
            .get_envs()
            .find(|(key, _)| *key == "PATH")
            .and_then(|(_, value)| value)
            .expect("isolated command should provide PATH");
        let path = std::env::join_paths(
            std::iter::once(dir.path().to_path_buf()).chain(std::env::split_paths(inherited_path)),
        )
        .unwrap();

        let output = cmd
            .env("PATH", path)
            .current_dir(dir.path())
            .stdin(Stdio::null())
            .args(["-v", "phpstan"])
            .args(args)
            .output()
            .unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        let stdout = String::from_utf8(output.stdout).unwrap();

        assert_eq!(output.status.code(), Some(code), "{args:?}: {stderr}");
        assert_eq!(
            stdout.lines().next(),
            Some(expected_stdout),
            "{args:?}: {stdout:?}"
        );
        assert!(!stdout.contains("fixture table detail"), "{stdout:?}");
        assert!(
            stderr.lines().any(|line| line == expected_log),
            "{args:?}: {stderr:?}"
        );
    }
}
