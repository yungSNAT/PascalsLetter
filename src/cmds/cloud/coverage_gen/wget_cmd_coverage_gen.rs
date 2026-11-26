use super::{extract_filename_from_output, format_size, get_file_size, parse_error};

#[test]
fn output_document_overrides_localized_saving_messages() {
    let url = "https://example.invalid/fallback.tar.gz?download=1";
    let french = "Saving to: ''\nSauvegarde en : « résumé.tar.gz »\n";

    assert_eq!(
        extract_filename_from_output(french, url, &["-Ochosen.tar.gz".to_owned()]),
        "chosen.tar.gz"
    );
    assert_eq!(
        extract_filename_from_output(french, url, &[]),
        "résumé.tar.gz"
    );
    assert_eq!(
        extract_filename_from_output("Saving to: 'report final.csv'\n", url, &[]),
        "report final.csv"
    );
    assert_eq!(
        extract_filename_from_output("Saving to: 'unclosed\n", url, &[]),
        "fallback.tar.gz"
    );
}

#[test]
fn file_size_reports_missing_files_and_formats_gigabytes() {
    let dir = crate::core::test_isolation::tempdir();
    let file = dir.path().join("download.bin");
    std::fs::write(&file, b"payload").unwrap();

    assert_eq!(get_file_size(file.to_str().unwrap()), 7);
    let missing = dir.path().join("missing.bin");
    assert_eq!(get_file_size(missing.to_str().unwrap()), 0);
    assert_eq!(format_size(1024 * 1024 * 1024), "1.0GB");
    assert_eq!(format_size(3 * 1024 * 1024 * 1024 / 2), "1.5GB");
}

#[test]
fn error_summary_classifies_failures_and_trims_unrecognized_diagnostics() {
    for (stderr, stdout, expected) in [
        ("", "HTTP status 403", "403 Forbidden"),
        ("HTTP status 401", "", "401 Unauthorized"),
        ("HTTP status 500", "", "500 Server Error"),
        ("connect: Connection refused", "", "Connection refused"),
        ("read timed out", "", "Connection timed out"),
        (
            "\n  -- request started --\n  Disk quota exceeded  \nignored\n",
            "",
            "Disk quota exceeded",
        ),
    ] {
        assert_eq!(parse_error(stderr, stdout), expected);
    }

    let long_line = "é".repeat(70);
    let stderr = format!("\n -- request started -- \n  {long_line}  \nignored");
    assert_eq!(parse_error(&stderr, ""), format!("{}...", "é".repeat(60)));
}

#[cfg(unix)]
mod cli {
    use crate::core::test_isolation::{rtk_binary_or_skip, rtk_command, tempdir};
    use std::fs::{self, Permissions};
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    fn install_fake_wget(dir: &Path) {
        let tool = dir.join("wget");
        fs::write(
            &tool,
            r#"#!/bin/sh
last=
forwarded=no
for arg do
    last=$arg
    if [ "$arg" = '--timeout=1' ]; then
        forwarded=yes
    fi
done
[ "$forwarded" = yes ] || exit 91

diagnostic='This is a simulated download diagnostic included in the captured response.'
case "$last" in
    https://example.invalid/archive.bin)
        printf '%s' 'payload' > "$WGET_COVERAGE_FILE" || exit 92
        printf "Saving to: '%s'\n%s\n" "$WGET_COVERAGE_FILE" "$diagnostic" >&2
        exit 0
        ;;
    https://example.invalid/forbidden.bin)
        printf '%s\n' 'HTTP response 403' "$diagnostic"
        exit 8
        ;;
esac

[ "$1" = '-q' ] && [ "$2" = '-O' ] && [ "$3" = '-' ] || exit 93
case "$last" in
    https://example.invalid/short.txt)
        printf '%s\n' first second
        ;;
    https://example.invalid/long.txt)
        chunk=xxxxxxxxxx
        half=$chunk$chunk$chunk$chunk$chunk$chunk
        index=1
        while [ "$index" -le 21 ]; do
            printf '%02d %s%s\n' "$index" "$half" "$half"
            index=$((index + 1))
        done
        ;;
    https://example.invalid/failed.txt)
        printf '%s\n' 'HTTP response 404'
        printf '%s\n' 'HTTP response 500' "$diagnostic" >&2
        exit 8
        ;;
    *)
        exit 94
        ;;
esac
"#,
        )
        .unwrap();
        fs::set_permissions(&tool, Permissions::from_mode(0o700)).unwrap();
    }

    fn invoke_wget(dir: &Path, url: &str, to_stdout: bool, expected_code: i32) -> String {
        let mut command = rtk_command();
        let inherited_path = command
            .get_envs()
            .find(|(key, _)| *key == "PATH")
            .and_then(|(_, value)| value)
            .expect("isolated rtk command has a PATH");
        let path = std::env::join_paths(
            std::iter::once(dir.to_path_buf()).chain(std::env::split_paths(inherited_path)),
        )
        .unwrap();

        command
            .env("PATH", path)
            .env("WGET_COVERAGE_FILE", dir.join("artifact.bin"))
            .args(["-v", "wget"]);
        if to_stdout {
            command.arg("-o");
        }
        let output = command.args([url, "--", "--timeout=1"]).output().unwrap();
        let stdout = String::from_utf8(output.stdout).unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(
            output.status.code(),
            Some(expected_code),
            "stdout: {stdout}\nstderr: {stderr}"
        );
        let suffix = if to_stdout { " -> stdout" } else { "" };
        let verbose_line = format!("wget: {url}{suffix}");
        assert!(stderr.lines().any(|line| line == verbose_line), "{stderr}");
        stdout
    }

    #[test]
    fn download_reports_the_written_file_or_the_http_failure() {
        if !rtk_binary_or_skip() {
            return;
        }

        let dir = tempdir();
        install_fake_wget(dir.path());
        let artifact = dir.path().join("artifact.bin");
        let output = invoke_wget(dir.path(), "https://example.invalid/archive.bin", false, 0);

        assert_eq!(
            output.trim_end(),
            format!(
                "example.invalid/archive.bin ok | {} | 7B",
                artifact.display()
            )
        );
        assert_eq!(fs::read_to_string(&artifact).unwrap(), "payload");

        let failure = invoke_wget(
            dir.path(),
            "https://example.invalid/forbidden.bin",
            false,
            8,
        );
        assert_eq!(
            failure.trim_end(),
            "example.invalid/forbidden.bin FAILED: 403 Forbidden"
        );
    }
}
