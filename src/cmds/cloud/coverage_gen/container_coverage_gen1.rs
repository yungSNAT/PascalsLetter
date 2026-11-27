use super::*;

fn pods_fixture() -> Value {
    serde_json::json!({
        "items": [
            {
                "metadata": {"namespace": "ns", "name": "p1"},
                "status": {
                    "phase": "Running",
                    "containerStatuses": [{"restartCount": 2}]
                }
            },
            {
                "metadata": {"namespace": "ns", "name": "p2"},
                "status": {"phase": "Pending"}
            },
            {
                "metadata": {"namespace": "ns", "name": "p3"},
                "status": {"phase": "Failed"}
            },
            {
                "metadata": {"namespace": "ns", "name": "p4"},
                "status": {
                    "phase": "Unknown",
                    "containerStatuses": [
                        {"restartCount": 0, "state": {"waiting": {"reason": "CrashLoopBackOff"}}},
                        {"restartCount": 0, "state": {"waiting": {"reason": "ContainerCreating"}}}
                    ]
                }
            },
            {
                "status": {"phase": "Succeeded"}
            }
        ]
    })
}

#[test]
fn kubectl_pods_summary_counts_phases_and_issues() {
    let out = format_kubectl_pods(&pods_fixture());
    // The running count is printed without a label today, so the header is
    // checked piece by piece rather than pinned.
    let header = out.lines().next().unwrap_or_default();
    assert!(header.starts_with("5 pods: "), "unexpected header: {out}");
    for part in ["1 pending", "2 [x]", "2 restarts"] {
        assert!(header.contains(part), "unexpected header: {out}");
    }
    assert!(out.contains("[warn] Issues:\n"), "{out}");
    assert!(out.contains("  ns/p2 Pending\n"), "{out}");
    assert!(out.contains("  ns/p3 Failed\n"), "{out}");
    assert!(out.contains("  ns/p4 CrashLoopBackOff\n"), "{out}");
    assert!(!out.contains("ContainerCreating"), "{out}");
    assert!(!out.contains("more"), "{out}");
}

#[test]
fn kubectl_pods_empty_items() {
    assert_eq!(
        format_kubectl_pods(&serde_json::json!({"items": []})),
        "No pods found\n"
    );
    assert_eq!(
        format_kubectl_pods(&serde_json::json!({})),
        "No pods found\n"
    );
}

#[test]
fn kubectl_pods_many_issues_are_truncated() {
    let n = CAP_WARNINGS + 2;
    let items: Vec<Value> = (0..n)
        .map(|i| {
            serde_json::json!({
                "metadata": {"namespace": "default", "name": format!("pod-{i}")},
                "status": {"phase": "Pending"}
            })
        })
        .collect();
    let json = serde_json::json!({ "items": items });
    let dir = crate::core::test_isolation::tempdir();
    crate::core::test_isolation::with_root(dir.path(), || {
        let out = format_kubectl_pods(&json);
        assert!(
            out.starts_with(&format!("{n} pods: {n} pending\n")),
            "{out}"
        );
        assert!(out.contains("  default/pod-0 Pending\n"), "{out}");
        assert!(out.contains("  … +2 more"), "{out}");
        assert!(
            !out.contains(&format!("default/pod-{} Pending\n", n - 1)),
            "{out}"
        );
    });
}

#[test]
fn kubectl_services_formats_ports() {
    let json = serde_json::json!({
        "items": [
            {
                "metadata": {"namespace": "default", "name": "web"},
                "spec": {
                    "type": "ClusterIP",
                    "ports": [
                        {"port": 80, "targetPort": 8080},
                        {"port": 443, "targetPort": "443"},
                        {"port": 9000, "targetPort": "http"}
                    ]
                }
            },
            {
                "spec": {}
            }
        ]
    });
    let out = format_kubectl_services(&json);
    assert_eq!(
        out,
        "2 services:\n  default/web ClusterIP [80→8080,443,9000]\n  -/- - []\n"
    );
}

#[test]
fn kubectl_services_empty_items() {
    assert_eq!(
        format_kubectl_services(&serde_json::json!({"items": []})),
        "No services found\n"
    );
}

#[test]
fn kubectl_services_many_are_truncated() {
    let n = CAP_LIST + 3;
    let items: Vec<Value> = (0..n)
        .map(|i| {
            serde_json::json!({
                "metadata": {"namespace": "ns", "name": format!("svc-{i}")},
                "spec": {"type": "NodePort", "ports": [{"port": 80}]}
            })
        })
        .collect();
    let json = serde_json::json!({ "items": items });
    let dir = crate::core::test_isolation::tempdir();
    crate::core::test_isolation::with_root(dir.path(), || {
        let out = format_kubectl_services(&json);
        assert!(out.starts_with(&format!("{n} services:\n")), "{out}");
        assert!(out.contains("  ns/svc-0 NodePort [80]\n"), "{out}");
        assert!(out.contains("  … +3 more"), "{out}");
        assert!(out.ends_with('\n'), "{out}");
        assert!(!out.contains(&format!("ns/svc-{} ", n - 1)), "{out}");
    });
}

#[cfg(unix)]
fn fake_docker_command(dir: &std::path::Path) -> std::process::Command {
    use std::os::unix::fs::PermissionsExt;
    let script = r#"#!/bin/sh
case "$1" in
images)
  if [ "$2" = "--format" ]; then
    if [ "$FAKE_MODE" = "nofmt" ]; then exit 3; fi
    printf 'nginx:latest\t1.5GB\nredis:7\t300MB\n'
  else
    if [ "$FAKE_MODE" = "fail" ]; then printf 'partial-out\n'; echo boom >&2; exit 2; fi
    printf 'REPOSITORY                     TAG       IMAGE ID       CREATED        SIZE\n'
    printf 'nginx                          latest    0123456789ab   2 weeks ago    1.5GB\n'
    printf 'redis                          7         ba9876543210   3 weeks ago    300MB\n'
  fi
  ;;
logs)
  i=0
  while [ $i -lt 200 ]; do
    echo "ERROR connection refused to database host db.internal port 5432"
    i=$((i+1))
  done
  ;;
esac
exit 0
"#;
    let path = dir.join("docker");
    std::fs::write(&path, script).expect("write fake docker");
    let mut perms = std::fs::metadata(&path).expect("metadata").permissions();
    perms.set_mode(0o700);
    std::fs::set_permissions(&path, perms).expect("chmod");

    let mut cmd = crate::core::test_isolation::rtk_command();
    let inherited = cmd
        .get_envs()
        .find(|(k, _)| k.to_str() == Some("PATH"))
        .and_then(|(_, v)| v.map(|v| v.to_os_string()))
        .unwrap_or_default();
    let mut new_path = dir.as_os_str().to_os_string();
    if !inherited.is_empty() {
        new_path.push(":");
        new_path.push(&inherited);
    }
    cmd.env("PATH", new_path);
    cmd
}

#[cfg(unix)]
#[test]
fn docker_images_via_rtk_binary() {
    if !crate::core::test_isolation::rtk_binary_ready() {
        return;
    }
    let dir = crate::core::test_isolation::tempdir();

    // Success path: compact summary with total size.
    let out = fake_docker_command(dir.path())
        .args(["docker", "images"])
        .output()
        .expect("run rtk");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "stdout: {stdout}");
    assert!(stdout.contains("[docker] 2 images (1.8GB)"), "{stdout}");
    assert!(stdout.contains("  nginx:latest [1.5GB]"), "{stdout}");
    assert!(stdout.contains("  redis:7 [300MB]"), "{stdout}");

    // --format call fails: raw output is passed through.
    let out = fake_docker_command(dir.path())
        .env("FAKE_MODE", "nofmt")
        .args(["docker", "images"])
        .output()
        .expect("run rtk");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "stdout: {stdout}");
    assert!(stdout.contains("REPOSITORY"), "{stdout}");
    assert!(!stdout.contains("[docker]"), "{stdout}");

    // Base call fails: exit code propagated, stdout passed through.
    let out = fake_docker_command(dir.path())
        .env("FAKE_MODE", "fail")
        .args(["docker", "images"])
        .output()
        .expect("run rtk");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!out.status.success(), "stdout: {stdout}");
    assert!(stdout.contains("partial-out"), "{stdout}");
}

#[cfg(unix)]
#[test]
fn docker_logs_via_rtk_binary() {
    if !crate::core::test_isolation::rtk_binary_ready() {
        return;
    }
    let dir = crate::core::test_isolation::tempdir();
    let out = fake_docker_command(dir.path())
        .args(["docker", "logs", "web-1"])
        .output()
        .expect("run rtk");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "stdout: {stdout}");
    assert!(stdout.contains("Logs for web-1"), "{stdout}");
    let repeats = stdout.matches("connection refused").count();
    assert!((1..200).contains(&repeats), "{stdout}");
}
