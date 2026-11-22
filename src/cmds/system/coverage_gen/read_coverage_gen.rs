use super::*;
use crate::core::test_isolation;

#[test]
fn verbose_file_reads_report_language_counts_and_range() -> Result<()> {
    if !test_isolation::rtk_binary_is_built() {
        return Ok(());
    }

    let dir = test_isolation::tempdir();
    let first = dir.path().join("first.rs");
    let empty = dir.path().join("empty.rs");
    let last = dir.path().join("last.rs");
    fs::write(&first, "fn alpha() {}\nfn bravo() {}\n")?;
    fs::write(&empty, "")?;
    fs::write(&last, "fn charlie() {}\n")?;

    let output = test_isolation::rtk_command()
        .args(["-vv", "read", "-n"])
        .args([&first, &empty, &last])
        .output()?;
    let stderr = String::from_utf8(output.stderr)?;
    assert!(output.status.success(), "{stderr}");

    let stdout = String::from_utf8(output.stdout)?;
    let (_, after_first) = stdout
        .split_once("1\tfn alpha() {}\n2\tfn bravo() {}\n")
        .with_context(|| format!("first numbered file missing from stdout: {stdout:?}"))?;
    assert!(after_first.contains("1\tfn charlie() {}\n"), "{stdout}");

    for file in [&first, &empty, &last] {
        assert!(
            stderr.contains(&format!("Reading: {} (filter:", file.display())),
            "{stderr}"
        );
    }
    assert_eq!(
        stderr.matches("Detected language: Rust").count(),
        3,
        "{stderr}"
    );
    for counts in ["2 -> 2", "0 -> 0", "1 -> 1"] {
        assert!(
            stderr.contains(&format!("Lines: {counts} (0.0% reduction)")),
            "{stderr}"
        );
    }

    let output = test_isolation::rtk_command()
        .args(["-vv", "read", "-n", "--from", "2", "--lines", "1"])
        .arg(&first)
        .output()?;
    let stderr = String::from_utf8(output.stderr)?;
    assert!(output.status.success(), "{stderr}");
    assert_eq!(output.stdout, b"2\tfn bravo() {}\n");
    assert!(
        stderr.contains(&format!("Reading: {} (from 2, lines 1)", first.display())),
        "{stderr}"
    );
    Ok(())
}

#[test]
fn verbose_stdin_read_reports_counts_and_deduplicates_stdin() -> Result<()> {
    if !test_isolation::rtk_binary_is_built() {
        return Ok(());
    }

    let dir = test_isolation::tempdir();
    let input_file = dir.path().join("stdin.txt");
    for (input, expected, count) in [("", "", 0), ("alpha\nbravo\n", "1\talpha\n2\tbravo\n", 2)] {
        fs::write(&input_file, input)?;
        let output = test_isolation::rtk_command()
            .args(["-vv", "read", "-n", "-", "-"])
            .stdin(fs::File::open(&input_file)?)
            .output()?;
        let stderr = String::from_utf8(output.stderr)?;
        assert!(output.status.success(), "{stderr}");
        assert_eq!(output.stdout, expected.as_bytes());
        assert_eq!(
            stderr.matches("Reading from stdin (filter:").count(),
            1,
            "{stderr}"
        );
        assert!(
            stderr.contains("Language: Unknown (stdin has no extension)"),
            "{stderr}"
        );
        assert!(
            stderr.contains(&format!("Lines: {count} -> {count} (0.0% reduction)")),
            "{stderr}"
        );
        assert!(
            stderr.contains("stdin specified more than once"),
            "{stderr}"
        );
    }
    Ok(())
}

#[test]
fn copy_line_range_propagates_a_read_failure_after_partial_output() {
    struct FailingReader(Option<io::Error>);

    impl IoRead for FailingReader {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(self.0.take().expect("retried a non-interrupted read error"))
        }
    }

    let input = (&b"first\r\n"[..]).chain(FailingReader(Some(io::Error::new(
        io::ErrorKind::PermissionDenied,
        "injected read failure",
    ))));
    let mut output = Vec::new();
    let error = copy_line_range(input, &mut output, 0, 2)
        .expect_err("a partial range must not be reported as a successful read");

    assert_eq!(output, b"first\r\n");
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(error.to_string(), "injected read failure");
}

#[cfg(target_os = "linux")]
#[test]
fn read_head_adds_path_context_when_reading_an_open_directory_fails() {
    let dir = test_isolation::tempdir();
    let path = dir.path();
    // Linux allows opening a directory with File::open but rejects reading it.
    let error = read_head_lines(path, 1).expect_err("a directory is not a line source");

    assert_eq!(
        error.to_string(),
        format!("Failed to read file: {}", path.display())
    );
    assert_eq!(
        error.downcast_ref::<io::Error>().map(io::Error::kind),
        Some(io::ErrorKind::IsADirectory)
    );
}
