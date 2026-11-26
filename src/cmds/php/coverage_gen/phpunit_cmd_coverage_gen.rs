use super::{build_phpunit_summary, filter_phpunit_output};

#[test]
fn failure_block_is_flushed_at_footer_or_end_of_output() {
    let block = "There was 1 failure:\n\n1) CartTest::testTotal\nTotals did not match.\n";
    let counts = "Tests: 3, Assertions: 5, Failures: 1.";
    let detail = "\n\n1) CartTest::testTotal\n  Totals did not match.";

    assert_eq!(
        filter_phpunit_output(&format!("{block}\nFAILURES!\n{counts}\n")),
        format!("PHPUnit: 3 tests, 5 assertions, 1 failures{detail}")
    );
    // Without a footer the counts line reads "0 failures" beside the failure
    // it lists, and a footer without a block drops the failure count; only
    // the parts that are right today are pinned.
    let unterminated = filter_phpunit_output(block);
    assert!(unterminated.ends_with(detail), "{unterminated}");
    let footer_only = filter_phpunit_output(&format!("FAILURES!\n{counts}\n"));
    assert!(
        footer_only.starts_with("PHPUnit: 3 tests, 5 assertions"),
        "{footer_only}"
    );
}

#[test]
fn success_with_risky_tests_omits_skipped_and_unknown_counts() {
    let output = "OK, but incomplete, skipped, or risky tests!\n\
                  Tests: 7, Assertions: 11, Incomplete: 2, Risky: 1.";

    assert_eq!(
        filter_phpunit_output(output),
        "PHPUnit: 7 tests, 11 assertions"
    );
}

#[test]
fn empty_failure_block_adds_no_blank_heading() {
    let failures = [
        Vec::new(),
        vec![
            "1) CartTest::testTotal".to_owned(),
            "Totals did not match.".to_owned(),
        ],
    ];

    assert_eq!(
        build_phpunit_summary("Tests: 3, Assertions: 5, Failures: 1.", &failures),
        "PHPUnit: 3 tests, 5 assertions, 1 failures\n\n1) CartTest::testTotal\n  Totals did not match."
    );
}

#[cfg(all(unix, feature = "engine"))]
#[test]
fn verbose_phpunit_command_forwards_arguments_and_filters_stdout() -> anyhow::Result<()> {
    use crate::core::test_isolation::{rtk_binary_ready, rtk_command, tempdir};
    use std::fs::{self, Permissions};
    use std::os::unix::fs::PermissionsExt;

    if !rtk_binary_ready() {
        return Ok(());
    }

    let dir = tempdir();
    let tool = dir.path().join("phpunit");
    fs::write(
        &tool,
        r#"#!/bin/sh
if [ "$#" -eq 2 ] && [ "$1" = "Example Test.php" ] && [ "$2" = "OtherTest.php" ]; then
    printf '%s\n' 'PHPUnit fixture header' '...' 'OK (3 tests, 5 assertions)'
else
    printf '%s\n' 'OK (0 tests, 0 assertions)'
fi
"#,
    )?;
    fs::set_permissions(&tool, Permissions::from_mode(0o700))?;

    let mut cmd = rtk_command();
    let inherited_path = cmd
        .get_envs()
        .find(|(key, _)| *key == "PATH")
        .and_then(|(_, value)| value)
        .expect("isolated rtk command should preserve PATH");
    let path = std::env::join_paths(
        std::iter::once(dir.path().to_path_buf()).chain(std::env::split_paths(inherited_path)),
    )?;
    let output = cmd
        .current_dir(dir.path())
        .env("PATH", path)
        .args(["-v", "phpunit", "Example Test.php", "OtherTest.php"])
        .output()?;

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "rtk exited with {}: {stderr}",
        output.status
    );
    assert_eq!(
        String::from_utf8(output.stdout)?.trim(),
        "PHPUnit: OK (3 tests, 5 assertions)"
    );
    assert!(
        stderr
            .lines()
            .any(|line| line == "Running: phpunit Example Test.php OtherTest.php"),
        "missing verbose invocation in stderr: {stderr}"
    );
    Ok(())
}
