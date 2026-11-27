use super::*;

#[test]
fn lint_preserves_fatal_and_syntax_diagnostics_in_order() {
    let raw = "\n  PHP startup notice\n\
               \n  PHP Fatal error: Cannot redeclare example()\n\
               \n  parser: syntax error, unexpected token\n";

    assert_eq!(
        filter_php_lint_output(raw),
        "PHP Fatal error: Cannot redeclare example()\nparser: syntax error, unexpected token"
    );
}

#[test]
fn lint_without_recognized_lines_falls_back_to_trimmed_original_text() {
    assert_eq!(filter_php_lint_output(" \t\n\n"), "ok");
    assert_eq!(
        filter_php_lint_output("  \x1b[33mPHP startup notice\x1b[0m \n"),
        "\x1b[33mPHP startup notice\x1b[0m"
    );
}

#[test]
fn generic_php_removes_ansi_and_reports_ok_for_blank_output() {
    assert_eq!(
        filter_php_output("\n  \x1b[32mfirst line\x1b[0m\n  second line \n"),
        "first line\n  second line"
    );
    assert_eq!(filter_php_output("\x1b[32m \x1b[0m\n"), "ok");
}

#[cfg(unix)]
#[test]
fn php_cli_forwards_arguments_and_selects_the_output_filter() {
    use crate::core::test_isolation::{rtk_binary_ready, rtk_command, tempdir};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    if !rtk_binary_ready() {
        return;
    }

    let scratch = tempdir();
    let php = scratch.path().join("php");
    let lint = "PHP startup notice\nNo syntax errors detected in example.php\n";
    let artisan = "\n   INFO  Clearing cached bootstrap files.\n\n  config .... 1.00ms DONE\n";
    let artisan_test = concat!(
        "\n   PASS  Tests\\Feature\\ExampleTest\n",
        "  ✓ it works\n\n",
        "  Tests:    1 passed (1 assertions)\n",
        "  Duration: 0.06s\n",
    );
    let cases: [(&[&str], &str, String); 5] = [
        (&["-l", "example.php"], lint, "ok example.php".to_owned()),
        (
            &["--syntax-check", "example.php"],
            lint,
            "ok example.php".to_owned(),
        ),
        (
            &["artisan", "optimize:clear"],
            artisan,
            filter_artisan_output(artisan),
        ),
        (
            &["artisan", "test"],
            artisan_test,
            filter_artisan_test_output(artisan_test, PhpTestRunner::Unknown),
        ),
        (
            &["example.php"],
            "  \x1b[32mhello from PHP\x1b[0m\n",
            "hello from PHP".to_owned(),
        ),
    ];

    for (args, raw, expected) in cases {
        let arguments = args.join(" ");
        fs::write(
            &php,
            format!(
                "#!/bin/sh\n\
                 if [ \"$#\" -eq {} ] && [ \"$*\" = '{arguments}' ]; then\n\
                   printf '%s' '{raw}'\n\
                 else\n\
                   printf '%s\\n' 'unexpected PHP arguments'\n\
                 fi\n",
                args.len()
            ),
        )
        .unwrap();
        fs::set_permissions(&php, fs::Permissions::from_mode(0o700)).unwrap();

        let mut cmd = rtk_command();
        let mut paths = vec![scratch.path().to_path_buf()];
        if let Some(path) = cmd
            .get_envs()
            .find(|(name, _)| *name == "PATH")
            .and_then(|(_, value)| value)
        {
            paths.extend(std::env::split_paths(path));
        }

        let output = cmd
            .env("PATH", std::env::join_paths(paths).unwrap())
            .current_dir(scratch.path())
            .args(["-v", "php"])
            .args(args)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "php {arguments} exited with {}: {stderr}",
            output.status
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            expected.trim(),
            "php {arguments}: {stderr}"
        );
        assert!(
            stderr
                .lines()
                .any(|line| line == format!("Running: php {arguments}")),
            "{stderr}"
        );
        assert_eq!(
            stderr.contains("Detected artisan test runner:"),
            args.starts_with(&["artisan", "test"]),
            "{stderr}"
        );
    }
}
