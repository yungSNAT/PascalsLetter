use super::*;

#[test]
fn detects_word_byte_and_character_modes() {
    for (flag, expected) in [
        ("-w", WcMode::Words),
        ("-c", WcMode::Bytes),
        ("-m", WcMode::Chars),
        ("-cm", WcMode::Mixed),
        ("--", WcMode::Full),
    ] {
        let args = [flag.to_owned(), "sample.txt".to_owned()];
        assert_eq!(detect_mode(&args), expected, "flag: {flag}");
    }
}

#[test]
fn formats_single_rows_with_and_without_filenames() {
    assert_eq!(format_single_line("  7    21  ", &WcMode::Full), "7    21");
    assert_eq!(
        format_single_line("  7    21    sample.txt  ", &WcMode::Mixed),
        "7 21"
    );
    assert_eq!(format_single_line("  7    21  ", &WcMode::Mixed), "7 21");
    assert_eq!(format_single_line("  7  ", &WcMode::Mixed), "7");
}

#[test]
fn formats_table_rows_and_preserves_incomplete_counts() {
    assert_eq!(
        filter_wc_output(
            "   3    8 data/first.txt\n   4    9 data/second.txt\n   7    17 total\n",
            &WcMode::Mixed,
        ),
        "3 8 first.txt\n4 9 second.txt\nΣ 7 17"
    );
    assert_eq!(
        filter_wc_output(
            "   3    8 first.txt\n \t \n   5    9\n   12  \n",
            &WcMode::Mixed
        ),
        "3 8 first.txt\n5 9\n12"
    );
    assert_eq!(
        filter_wc_output("   6    19    42\n   6 19 42 total\n", &WcMode::Full),
        "6    19    42\nΣ 6L 19W 42B"
    );
}

#[test]
fn prefix_search_uses_only_shared_directory_segments() {
    let prefix = find_common_prefix(&["project/src/bin/one.rs", "project/tests/two.rs"]);
    assert_eq!(prefix.strip_suffix('/'), Some("project"));
    assert!(find_common_prefix(&["project-one/one.rs", "project-two/two.rs"]).is_empty());
    assert!(find_common_prefix(&["project/one.rs"]).is_empty());
}

#[cfg(unix)]
#[test]
fn verbose_wc_reports_the_command_and_filters_its_output() {
    use crate::core::test_isolation::{rtk_binary_is_built, rtk_command};
    use std::os::unix::fs::PermissionsExt;

    if !rtk_binary_is_built() {
        return;
    }

    let tools = tempfile::tempdir().expect("create fake tool directory");
    let wc = tools.path().join("wc");
    std::fs::write(
        &wc,
        "#!/bin/sh\nprintf '%s\\n' '      7     11     63 sample.txt'\n",
    )
    .expect("write fake wc");
    std::fs::set_permissions(&wc, std::fs::Permissions::from_mode(0o700))
        .expect("make fake wc executable");

    let mut cmd = rtk_command();
    let inherited_path = cmd
        .get_envs()
        .find(|(key, _)| *key == "PATH")
        .and_then(|(_, value)| value);
    let path = std::env::join_paths(
        std::iter::once(tools.path().to_path_buf())
            .chain(inherited_path.into_iter().flat_map(std::env::split_paths)),
    )
    .expect("prepend fake tool directory to PATH");

    let output = cmd
        .env("PATH", path)
        .args(["-v", "wc", "sample.txt"])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run rtk wc");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "rtk wc failed: {stderr}");
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "7L 11W 63B");
    assert!(
        stderr.lines().any(|line| line == "Running: wc sample.txt"),
        "missing verbose command in stderr: {stderr}"
    );
}
