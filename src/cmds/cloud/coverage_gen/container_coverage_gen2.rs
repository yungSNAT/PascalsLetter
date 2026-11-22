use super::*;
use crate::core::{test_isolation, user_env};
use std::fmt::Write as _;

fn assert_overflow_recall_hint(hint: &str) {
    let id = hint
        .strip_prefix("[+8 hidden: rtk recall ")
        .and_then(|rest| rest.strip_suffix(']'))
        .expect("overflow hint should give a recall command for the eight hidden rows");
    assert!(
        !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "invalid recall ID in {hint:?}"
    );
}

#[test]
fn capped_service_lists_report_hidden_rows_with_recall_hints() {
    let root = test_isolation::tempdir();
    let tee_dir = root.path().join("overflow");
    std::fs::create_dir(&tee_dir).unwrap();
    let _scope = test_isolation::enter(root.path());

    let count = CAP_LIST + 8;
    let services: Vec<_> = (0..count)
        .map(|index| {
            serde_json::json!({
                "metadata": {"namespace": "demo", "name": format!("svc-{index}")},
                "spec": {"type": "ClusterIP", "ports": [{"port": 80, "targetPort": "8080"}]}
            })
        })
        .collect();

    let mut compose_input = String::from("malformed row\n\n");
    for index in 0..count {
        writeln!(
            compose_input,
            "worker-{index}\tregistry.example/demo/worker:v1\tExited (0)\t-"
        )
        .unwrap();
    }

    let (kube, compose) = test_isolation::with_root(root.path(), || {
        user_env::with_vars(&[("RTK_TEE", Some("1"))], || {
            user_env::with_path("RTK_TEE_DIR", Some(tee_dir.as_path()), || {
                (
                    format_kubectl_services(&serde_json::json!({"items": services})),
                    format_compose_ps(&compose_input),
                )
            })
        })
    });

    let mut kube_lines = kube.lines();
    let mut compose_lines = compose.lines();
    assert_eq!(kube_lines.next().unwrap(), format!("{count} services:"));
    assert_eq!(
        compose_lines.next().unwrap(),
        format!("[compose] {} services:", count + 1)
    );
    for index in 0..CAP_LIST {
        assert_eq!(
            kube_lines.next().unwrap(),
            format!("  demo/svc-{index} ClusterIP [80→8080]")
        );
        assert_eq!(
            compose_lines.next().unwrap(),
            format!("  worker-{index} (worker:v1) Exited (0)")
        );
    }

    let kube_hint = kube_lines
        .next()
        .unwrap()
        .strip_prefix("  … +8 more ")
        .expect("kubectl puts the recall hint beside the omitted count");
    assert_eq!(compose_lines.next(), Some("  … +8 more"));
    let compose_hint = compose_lines
        .next()
        .and_then(|line| line.strip_prefix("  "))
        .expect("compose indents the recall hint on its own line");

    assert_overflow_recall_hint(kube_hint);
    assert_overflow_recall_hint(compose_hint);
    assert_eq!(kube_lines.next(), None);
    assert_eq!(compose_lines.next(), None);
    assert!(kube.ends_with('\n'));
    assert!(!compose.ends_with('\n'));
}

#[test]
fn compose_build_uses_the_first_finished_summary_or_falls_back() {
    for (raw, expected) in [
        (
            "[+] Building 0.1s (0/3)\n\
             => [web 1/2] load metadata\n\
             => [web 2/2] copy sources\n\
             => [api 1/1] copy sources\n\
             [+] Building 1.2s (3/3) FINISHED\n\
             [+] Building 9.9s (3/3) FINISHED\n",
            "[compose] [+] Building 1.2s (3/3) FINISHED\n  Services: web, api\n  Steps: 3",
        ),
        (
            "[+] Building 0.4s (1/2)\n => [web 1/2] load metadata\n",
            "[compose] [+] Building 0.4s (1/2)\n  Services: web\n  Steps: 1",
        ),
        ("failed to read build context\n", "[compose] Build:"),
    ] {
        assert_eq!(format_compose_build(raw), expected);
    }
}

#[test]
fn kubectl_logs_without_a_pod_returns_usage_success() {
    assert_eq!(k8s_logs("kubectl", &[], 0).unwrap(), 0);
}

#[cfg(unix)]
mod cli {
    use crate::core::test_isolation;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::process::Stdio;

    fn fake_tools(scripts: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = test_isolation::tempdir();
        for &(name, script) in scripts {
            let path = dir.path().join(name);
            fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        dir
    }

    fn invoke(tools: &Path, args: &[&str]) -> (String, String) {
        let mut cmd = test_isolation::rtk_command();
        let mut path = vec![tools.to_path_buf()];
        if let Some(inherited) = cmd
            .get_envs()
            .find(|(key, _)| *key == "PATH")
            .and_then(|(_, value)| value)
        {
            path.extend(std::env::split_paths(inherited));
        }
        let output = cmd
            .env("PATH", std::env::join_paths(path).unwrap())
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        let stdout = String::from_utf8(output.stdout).unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(
            output.status.success(),
            "{args:?} returned {}: stdout={stdout:?}, stderr={stderr:?}",
            output.status
        );
        (stdout, stderr)
    }

    #[test]
    fn docker_compose_filters_output_and_docker_passthrough_preserves_arguments() {
        if !test_isolation::rtk_binary_is_built() {
            return;
        }
        let tools = fake_tools(&[(
            "docker",
            r#"
template=$(printf '{{.Name}}\t{{.Image}}\t{{.Status}}\t{{.Ports}}')
case "$*" in
    'compose ps'|'compose ps -a')
        printf '%s\n' 'NAME IMAGE COMMAND SERVICE CREATED STATUS PORTS'
        printf '%s\n' 'web-1 registry.example/demo/nginx:stable "/entrypoint nginx" web 2 hours ago Up 2 hours 0.0.0.0:8080->80/tcp'
        if [ "$#" -eq 3 ]; then
            printf '%s\n' 'worker-1 registry.example/demo/job:v1 "/entrypoint run-worker" worker 2 hours ago Exited (0) 1 hour ago'
        fi
        ;;
    "compose ps --format $template"|"compose ps -a --format $template")
        printf 'web-1\tregistry.example/demo/nginx:stable\tUp 2 hours\t0.0.0.0:8080->80/tcp\n'
        if [ "$3" = '-a' ]; then
            printf 'worker-1\tregistry.example/demo/job:v1\tExited (0)\t\n'
        fi
        ;;
    'compose logs --tail 7 web')
        i=0
        while [ "$i" -lt 120 ]; do
            printf '%s\n' 'web-1  | ERROR fixture connection closed'
            i=$((i + 1))
        done
        ;;
    'compose build web')
        printf '%s\n' '[+] Building 1.2s (2/2) FINISHED' \
            ' => [web internal] load metadata for registry.example/demo/runtime:stable' \
            ' => [web 1/1] COPY application and configuration files into the runtime image'
        ;;
    'version'|'compose version')
        printf 'forwarded by docker: %s\n' "$*"
        ;;
    *)
        printf 'unexpected docker arguments: %s\n' "$*" >&2
        exit 97
        ;;
esac
"#,
        )]);

        let (stdout, stderr) = invoke(tools.path(), &["docker", "compose", "ps"]);
        assert_eq!(
            stdout.trim_end(),
            "[compose] 1 services:\n  web-1 (nginx:stable) Up 2 hours [8080]"
        );
        assert!(!stderr.contains("raw docker compose ps:"));

        let (stdout, stderr) = invoke(tools.path(), &["-v", "docker", "compose", "ps", "-a"]);
        assert_eq!(
            stdout.trim_end(),
            "[compose] 2 services:\n  web-1 (nginx:stable) Up 2 hours [8080]\n  worker-1 (job:v1) Exited (0)"
        );
        assert!(stderr.contains("raw docker compose ps:\nNAME IMAGE"));
        assert!(stderr.contains("worker-1 registry.example/demo/job:v1"));

        let (stdout, stderr) = invoke(
            tools.path(),
            &["-v", "docker", "compose", "logs", "--tail", "7", "web"],
        );
        assert!(stdout.starts_with("[compose] Logs:\n"), "{stdout}");
        assert!(stdout.contains("fixture connection closed"), "{stdout}");
        assert_eq!(stdout.matches("fixture connection closed").count(), 1);
        assert!(stderr.contains("raw docker compose logs:\nweb-1  | ERROR"));

        let (stdout, stderr) = invoke(tools.path(), &["-v", "docker", "compose", "build", "web"]);
        assert_eq!(
            stdout.trim_end(),
            "[compose] [+] Building 1.2s (2/2) FINISHED\n  Services: web\n  Steps: 2"
        );
        assert!(stderr.contains("raw docker compose build:\n[+] Building"));

        for (args, expected) in [
            (&["docker", "version"][..], "forwarded by docker: version"),
            (
                &["docker", "compose", "version"][..],
                "forwarded by docker: compose version",
            ),
        ] {
            let (stdout, _) = invoke(tools.path(), args);
            assert_eq!(stdout.trim_end(), expected);
        }
    }

    #[test]
    fn kubectl_and_oc_route_get_aliases_logs_and_passthrough() {
        if !test_isolation::rtk_binary_is_built() {
            return;
        }
        let script = r#"
case "$*" in
    'get pods -o json -n demo')
        printf '%s\n' '{"apiVersion":"v1","kind":"List","metadata":{"resourceVersion":"42"},"items":[{"metadata":{"name":"worker","namespace":"demo"},"status":{"phase":"Pending"}}]}'
        ;;
    'get services -o json -n demo')
        printf '%s\n' '{"apiVersion":"v1","kind":"List","metadata":{"resourceVersion":"42"},"items":[{"metadata":{"name":"web","namespace":"demo"},"spec":{"type":"ClusterIP","ports":[{"port":80,"targetPort":8080}]}}]}'
        ;;
    'logs --tail 100 worker')
        i=0
        while [ "$i" -lt 120 ]; do
            printf '%s\n' 'ERROR fixture worker disconnected'
            i=$((i + 1))
        done
        ;;
    'version')
        printf 'forwarded by %s: %s\n' "${0##*/}" "$*"
        ;;
    *)
        printf 'unexpected Kubernetes arguments: %s\n' "$*" >&2
        exit 97
        ;;
esac
"#;
        let tools = fake_tools(&[("kubectl", script), ("oc", script)]);

        let (stdout, _) = invoke(tools.path(), &["oc", "get", "po", "-n", "demo"]);
        assert_eq!(
            stdout.trim_end(),
            "1 pods: 1 pending\n[warn] Issues:\n  demo/worker Pending"
        );
        let (stdout, _) = invoke(tools.path(), &["kubectl", "get", "svc", "-n", "demo"]);
        assert_eq!(
            stdout.trim_end(),
            "1 services:\n  demo/web ClusterIP [80→8080]"
        );

        let (stdout, _) = invoke(tools.path(), &["kubectl", "logs", "worker"]);
        assert!(stdout.starts_with("Logs for worker:\n"), "{stdout}");
        assert!(stdout.contains("fixture worker disconnected"), "{stdout}");

        for tool in ["kubectl", "oc"] {
            let (stdout, _) = invoke(tools.path(), &[tool, "version"]);
            assert_eq!(stdout.trim_end(), format!("forwarded by {tool}: version"));
        }
    }
}
