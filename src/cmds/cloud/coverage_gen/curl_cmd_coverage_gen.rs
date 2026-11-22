use super::{Cow, MAX_RESPONSE_SIZE, filter_curl_output};

#[test]
fn long_terminal_response_stays_intact_when_tee_is_disabled() {
    let root = crate::core::test_isolation::tempdir();
    let body = format!("{} and the original tail", "x".repeat(MAX_RESPONSE_SIZE));
    let raw = format!(" \t{body}\n ");

    crate::core::test_isolation::with_root(root.path(), || {
        crate::core::user_env::with_vars(&[("RTK_TEE", Some("0"))], || {
            let result = filter_curl_output(&raw, true);
            assert_eq!(result.content.as_ref(), body.as_str());
            assert!(matches!(result.content, Cow::Borrowed(_)));
            assert!(result.tee_hint.is_none());
        });
    });
}

#[cfg(unix)]
#[test]
fn curl_cli_reports_failures_and_preserves_binary_output() {
    use crate::core::test_isolation::{rtk_binary_is_built, rtk_command, tempdir};
    use std::os::unix::fs::PermissionsExt;
    use std::process::Stdio;

    if !rtk_binary_is_built() {
        return;
    }

    let tools = tempdir();
    let curl = tools.path().join("curl");
    std::fs::write(
        &curl,
        r#"#!/bin/sh
[ "$#" -eq 2 ] && [ "$1" = "-s" ] && [ "$2" = "file://" ] || exit 99
case "$RTK_TEST_CURL_CASE" in
    diagnostic)
        printf 'discarded response body\n'
        printf ' \tfixture diagnostic \n' >&2
        exit 22
        ;;
    body)
        printf ' \tfixture body \377 \n'
        printf ' \t\n' >&2
        exit 23
        ;;
    binary)
        printf '\037\213\010\000\377'
        ;;
    *) exit 99 ;;
esac
"#,
    )
    .expect("write fake curl");
    std::fs::set_permissions(&curl, std::fs::Permissions::from_mode(0o755))
        .expect("make fake curl executable");

    let mut cmd = rtk_command();
    let inherited_path = cmd
        .get_envs()
        .find(|(name, _)| *name == "PATH")
        .and_then(|(_, value)| value)
        .expect("isolated command has a PATH");
    let path = std::env::join_paths(
        std::iter::once(tools.path().to_path_buf()).chain(std::env::split_paths(inherited_path)),
    )
    .expect("prepend fake tool directory to PATH");
    // Use a file URL so even an unexpected fallback to real curl cannot use the network.
    cmd.env("PATH", path)
        .stdin(Stdio::null())
        .args(["-v", "curl", "file://"]);

    for (case, exit_code, message) in [
        ("diagnostic", 22, "fixture diagnostic"),
        ("body", 23, "fixture body \u{fffd}"),
    ] {
        let output = cmd
            .env("RTK_TEST_CURL_CASE", case)
            .output()
            .expect("run rtk with failing fake curl");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(exit_code), "{stderr}");
        assert!(output.stdout.is_empty(), "unexpected stdout for {case}");
        assert!(
            stderr
                .lines()
                .any(|line| line == "Running: curl -s file://"),
            "{stderr}"
        );
        let expected = format!("FAILED: curl {message}");
        assert!(stderr.lines().any(|line| line == expected), "{stderr}");
        assert!(!stderr.contains("discarded response body"), "{stderr}");
    }

    let output = cmd
        .env("RTK_TEST_CURL_CASE", "binary")
        .output()
        .expect("run rtk with binary fake curl output");
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, [0x1f, 0x8b, 0x08, 0x00, 0xff]);
}
