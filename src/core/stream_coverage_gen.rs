use super::*;

#[test]
#[should_panic(expected = "RegexBlockFilter: bad pattern '['")]
fn invalid_block_regex_reports_the_pattern() {
    let _filter = RegexBlockFilter::new("test tool", "[");
}

#[cfg(unix)]
#[test]
fn stopped_status_without_an_exit_code_or_terminating_signal_falls_back_to_one() {
    use std::os::unix::process::ExitStatusExt;

    // A stopped wait status is neither a normal exit nor a signal termination.
    let status = std::process::ExitStatus::from_raw((libc::SIGSTOP << 8) | 0x7f);
    assert_eq!(status.stopped_signal(), Some(libc::SIGSTOP));
    assert_eq!(status_to_exit_code(status), 1);
}

#[test]
fn help_request_for_missing_program_returns_the_spawn_error() {
    let scratch = crate::core::test_isolation::tempdir();
    let program = scratch.path().join("missing-help-tool.exe");
    let mut cmd = Command::new(&program);
    cmd.env_clear().arg("--help");

    // The missing executable makes the help path return before it can exit.
    let Err(error) = exec_capture(&mut cmd) else {
        panic!("a missing help executable should return an error");
    };
    assert_eq!(
        error.to_string(),
        format!("Failed to execute {}", program.display())
    );
    assert_eq!(
        error.downcast_ref::<io::Error>().map(io::Error::kind),
        Some(io::ErrorKind::NotFound)
    );
}

#[cfg(unix)]
const FIRST_STDOUT: &str = "out 0\rout 1\n";

#[cfg(unix)]
const FIRST_STDERR: &str = "err 0\rerr 1\n";

#[cfg(unix)]
fn capture_cap_fixture(scratch: &std::path::Path) -> Command {
    // On each stream, the first two lines reach RAW_CAP exactly before the
    // second line's newline. A short line follows to test capture after overflow.
    let padded_line = format!("%{}s\\n", RAW_CAP - FIRST_STDOUT.len());
    // nosemgrep: interpreter-execution
    let mut cmd = Command::new("/bin/sh");
    cmd.env_clear()
        .current_dir(scratch)
        .args([
            "-c",
            r#"set -e
printf 'out 0\rout 1\n'
printf "$1" x
printf 'out last\n'
printf 'err 0\rerr 1\n' >&2
printf "$1" x >&2
printf 'err last\n' >&2
exit 23"#,
            "stream-cap-fixture",
        ])
        .arg(padded_line);
    cmd
}

#[cfg(unix)]
#[test]
fn streaming_caps_each_raw_stream_but_keeps_filtering() {
    struct CompactLines;

    impl StreamFilter for CompactLines {
        fn feed_line(&mut self, line: &str) -> Option<String> {
            if line.len() > RAW_CAP / 2 {
                assert_eq!(line.len(), RAW_CAP - FIRST_STDOUT.len());
                Some("oversized line\n".to_owned())
            } else {
                Some(format!("{line}\n"))
            }
        }

        fn flush(&mut self) -> String {
            "flushed\n".to_owned()
        }

        fn on_exit(&mut self, exit_code: i32, raw: &str) -> Option<String> {
            Some(format!(
                "exit={exit_code}, captured={}\n",
                raw.lines().count()
            ))
        }
    }

    let scratch = crate::core::test_isolation::tempdir();
    let result = run_streaming(
        &mut capture_cap_fixture(scratch.path()),
        StdinMode::Null,
        FilterMode::Streaming(Box::new(CompactLines)),
    )
    .expect("streaming fixture should complete");

    assert_eq!(result.exit_code, 23);
    assert_eq!(result.raw_stdout.len(), FIRST_STDOUT.len());
    assert_eq!(result.raw_stderr.len(), FIRST_STDERR.len());
    assert_eq!(result.raw_stdout, FIRST_STDOUT);
    assert_eq!(result.raw_stderr, FIRST_STDERR);
    assert_eq!(result.raw, "out 0\rout 1\nerr 0\rerr 1\n");

    let body = result
        .filtered
        .strip_suffix("flushed\nexit=23, captured=2\n")
        .expect("flush output should precede the exit summary");
    let mut lines: Vec<_> = body.lines().collect();
    // The two reader threads can deliver their lines in either order.
    lines.sort_unstable();
    assert_eq!(
        lines,
        [
            "err 1",
            "err last",
            "out 1",
            "out last",
            "oversized line",
            "oversized line",
        ]
    );
}

#[cfg(unix)]
#[test]
fn buffered_filter_panic_falls_back_to_raw_output_after_skipping_oversized_line() {
    let scratch = crate::core::test_isolation::tempdir();
    let seen = std::cell::RefCell::new(String::new());
    let result = run_streaming(
        &mut capture_cap_fixture(scratch.path()),
        StdinMode::Null,
        FilterMode::Buffered(Box::new(|input| {
            *seen.borrow_mut() = input.to_owned();
            panic!("intentional buffered filter failure");
        })),
    )
    .expect("a buffered filter panic should not discard captured output");

    assert_eq!(result.exit_code, 23);
    assert_eq!(seen.into_inner(), "out 1\nout last\n");
    let expected_stdout = "out 0\rout 1\nout last\n";
    let expected_stderr = "err 0\rerr 1\nerr last\n";
    assert_eq!(result.raw_stdout.len(), expected_stdout.len());
    assert_eq!(result.raw_stderr.len(), expected_stderr.len());
    assert_eq!(result.raw_stdout, expected_stdout);
    assert_eq!(result.raw_stderr, expected_stderr);
    assert_eq!(result.filtered, expected_stdout);
}
