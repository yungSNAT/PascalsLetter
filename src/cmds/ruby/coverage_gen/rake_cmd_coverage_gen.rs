use super::{MAX_RAKE_FAILURES, filter_minitest_output};

#[test]
fn adjacent_failure_blocks_are_kept_up_to_the_display_limit() {
    let failures = MAX_RAKE_FAILURES + 2;
    let details = (1..=failures)
        .map(|n| {
            format!(
                "  {n}) Failure:\nWidgetTest#test_case_{n}\nExpected: 1\n  Actual: 2\n\
                 Assertion message\nBacktrace frame"
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let input = format!(
        "# Running:\nFinished in 0.01s\n{details}\n\
         {failures} runs, {failures} assertions, {failures} failures, 0 errors, 0 skips"
    );

    // Each block's header line is left unpinned: it is numbered twice today
    // ("1. 1) Failure:").
    let output = filter_minitest_output(&input);
    assert!(
        output.starts_with(&format!(
            "rake test: {failures} runs, {failures} failures, 0 errors\n\n"
        )),
        "{output}"
    );
    for n in 1..=MAX_RAKE_FAILURES {
        let details = format!(
            "\n   WidgetTest#test_case_{n}\n   Expected: 1\n   Actual: 2\n   Assertion message"
        );
        assert!(output.contains(&details), "{output}");
    }
    for n in MAX_RAKE_FAILURES + 1..=failures {
        assert!(!output.contains(&format!("test_case_{n}\n")), "{output}");
    }
    assert!(!output.contains("Backtrace frame"), "{output}");
    // The omission count includes the final block, which has no trailing blank line.
    assert!(output.ends_with("\n\n... +2 more failures"), "{output}");
}

#[test]
fn summary_without_failure_details_keeps_skips_and_ignores_extra_metrics() {
    let input = "\
Started with run options --seed 42
Finished in 0.02s
7 tests, 4 assertions, 1 failures, 1 errors, 2 skips, 99 retries";

    assert_eq!(
        filter_minitest_output(input),
        "rake test: 7 runs, 1 failures, 1 errors, 2 skips"
    );
}

#[cfg(unix)]
#[test]
fn verbose_rake_forwards_a_positional_file_to_rails_and_filters_output() {
    use crate::core::test_isolation::{rtk_binary_ready, rtk_command, tempdir};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    if !rtk_binary_ready() {
        return;
    }

    let dir = tempdir();
    let script = r#"#!/bin/sh
if [ "${0##*/}" = rails ] && [ "$#" -eq 2 ] && [ "$1" = test ] && [ "$2" = "widget test.rb:15" ]; then
    printf '%s\n' '# Running:' '...' 'Finished in 0.01s' '3 runs, 4 assertions, 0 failures, 0 errors, 0 skips'
fi
"#;
    for name in ["rake", "rails"] {
        let tool = dir.path().join(name);
        fs::write(&tool, script).unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o700)).unwrap();
    }

    let mut cmd = rtk_command();
    let inherited_path = cmd
        .get_envs()
        .find(|(key, _)| *key == "PATH")
        .and_then(|(_, value)| value)
        .expect("isolated rtk command should have a PATH");
    let path = std::env::join_paths(
        std::iter::once(dir.path().to_path_buf()).chain(std::env::split_paths(inherited_path)),
    )
    .unwrap();

    let output = cmd
        .current_dir(dir.path())
        .env("PATH", path)
        .args(["-v", "rake", "test", "widget test.rb:15"])
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "rtk failed: {stderr}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "ok rake test: 3 runs, 0 failures"
    );
    assert!(
        stderr
            .lines()
            .any(|line| line.starts_with("Running: ") && line.ends_with(" test widget test.rb:15")),
        "missing verbose invocation: {stderr}"
    );
}
