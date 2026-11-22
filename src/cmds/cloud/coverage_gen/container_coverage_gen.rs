use super::*;

#[test]
fn format_container_line_covers_ports_ids_and_short_rows() {
    let line = "abcdef1234567890\tweb\t Up 2 hours \tghcr.io/org/nginx:latest\t0.0.0.0:80->80/tcp, 0.0.0.0:443->443/tcp";
    assert_eq!(
        format_container_line(line, true).as_deref(),
        Some("  abcdef123456 web (nginx:latest) Up 2 hours [80, 443]\n")
    );
    assert_eq!(
        format_container_line(line, false).as_deref(),
        Some("  abcdef123456 web (nginx:latest) Up 2 hours\n")
    );

    // Short id, no ports column at all: no port suffix even with ports enabled.
    assert_eq!(
        format_container_line("abc\tdb\tExited\tpostgres", true).as_deref(),
        Some("  abc db (postgres) Exited\n")
    );

    // Empty ports column: compact_ports yields "-" which is suppressed.
    assert_eq!(
        format_container_line("abc\tdb\tExited\tlib/postgres\t", true).as_deref(),
        Some("  abc db (postgres) Exited\n")
    );

    // Too few columns.
    assert_eq!(format_container_line("a\tb\tc", true), None);
    assert_eq!(format_container_line("", false), None);
}

#[test]
fn format_container_line_from_parts_direct() {
    let parts = ["0123456789abcdef", "api", "Up 5 minutes", "node:20"];
    assert_eq!(
        format_container_line_from_parts(&parts, false).as_deref(),
        Some("  0123456789ab api (node:20) Up 5 minutes\n")
    );
    let with_port = ["id1", "svc", "Up", "img", "8080/tcp"];
    assert_eq!(
        format_container_line_from_parts(&with_port, true).as_deref(),
        Some("  id1 svc (img) Up [8080/tcp]\n")
    );
    assert_eq!(
        format_container_line_from_parts(&["x", "y", "z"], true),
        None
    );
}

#[cfg(unix)]
fn run_rtk_with_fake_docker(script: &str, args: &[&str]) -> std::process::Output {
    use std::os::unix::fs::PermissionsExt;
    let dir = crate::core::test_isolation::tempdir();
    let tool = dir.path().join("docker");
    std::fs::write(&tool, script).expect("write fake docker");
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755))
        .expect("chmod fake docker");
    let mut cmd = crate::core::test_isolation::rtk_command();
    let inherited: String = cmd
        .get_envs()
        .find(|(k, _)| *k == std::ffi::OsStr::new("PATH"))
        .and_then(|(_, v)| v)
        .map(|v| v.to_string_lossy().into_owned())
        .unwrap_or_default();
    let path = if inherited.is_empty() {
        dir.path().display().to_string()
    } else {
        format!("{}:{}", dir.path().display(), inherited)
    };
    cmd.env("PATH", path).args(args);
    cmd.output().expect("run rtk")
}

#[cfg(unix)]
const FAKE_DOCKER_PS: &str = r#"#!/bin/sh
case "$*" in
  *--format*)
    printf 'abcdef1234567890\tweb\tUp 2 hours\tghcr.io/org/nginx:latest\t0.0.0.0:80->80/tcp\n'
    printf '1111222233334444\tjob\tUp 1 hour\tbusybox\t\n'
    ;;
  *)
    echo "CONTAINER ID   IMAGE                              COMMAND                  CREATED         STATUS         PORTS                NAMES"
    echo "abcdef123456   ghcr.io/org/nginx:latest           \"/docker-entrypoint.…\"   3 hours ago     Up 2 hours     0.0.0.0:80->80/tcp   web"
    echo "111122223333   busybox                            \"sh -c 'while true; …\"   2 hours ago     Up 1 hour                           job"
    echo "PADDING-PADDING-PADDING-PADDING-PADDING-PADDING-PADDING-PADDING-PADDING-PADDING-PADDING-PADDING-PADDING"
    ;;
esac
"#;

#[cfg(unix)]
#[test]
fn docker_ps_formats_containers_via_binary() {
    if !crate::core::test_isolation::rtk_binary_is_built() {
        return;
    }
    let out = run_rtk_with_fake_docker(FAKE_DOCKER_PS, &["docker", "ps"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "stdout: {stdout}");
    assert!(
        stdout.contains("[docker] 2 containers:"),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains("abcdef123456 web (nginx:latest) Up 2 hours [80]"),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains("job (busybox) Up 1 hour"),
        "stdout: {stdout}"
    );
    assert!(!stdout.contains("PADDING"), "stdout: {stdout}");
}

#[cfg(unix)]
#[test]
fn docker_ps_falls_back_to_raw_when_format_fails() {
    if !crate::core::test_isolation::rtk_binary_is_built() {
        return;
    }
    let script = r#"#!/bin/sh
case "$*" in
  *--format*) exit 1 ;;
  *) echo "RAW-DOCKER-TABLE-MARKER" ;;
esac
"#;
    let out = run_rtk_with_fake_docker(script, &["docker", "ps"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "stdout: {stdout}");
    assert!(
        stdout.contains("RAW-DOCKER-TABLE-MARKER"),
        "stdout: {stdout}"
    );
    assert!(!stdout.contains("[docker]"), "stdout: {stdout}");
}

#[cfg(unix)]
#[test]
fn docker_ps_propagates_failure_exit_code() {
    if !crate::core::test_isolation::rtk_binary_is_built() {
        return;
    }
    let script = "#!/bin/sh\necho 'boom-from-fake-docker' >&2\nexit 3\n";
    let out = run_rtk_with_fake_docker(script, &["docker", "ps"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "stderr: {stderr}");
    assert!(stderr.contains("boom-from-fake-docker"), "stderr: {stderr}");
}
