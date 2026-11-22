use super::*;
use crate::core::test_isolation;

#[test]
fn bunx_without_a_tool_is_an_error() {
    let err = run_bunx_tool(&[], 0, false).unwrap_err();
    assert!(
        err.to_string().contains("bunx requires a command argument"),
        "unexpected error: {err}"
    );
}

#[test]
fn cat_error_text_matches_cat_for_permission_and_directory_errors() {
    let denied = anyhow::Error::new(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
        .context("Failed to read file: secret.txt");
    assert_eq!(cat_error_text(&denied), "Permission denied");

    let dir = anyhow::Error::new(std::io::Error::from(std::io::ErrorKind::IsADirectory))
        .context("Failed to read file: src");
    assert_eq!(cat_error_text(&dir), "Is a directory");
}

#[test]
fn k8s_logs_args_add_container_only_when_given() {
    assert_eq!(build_k8s_logs_args("web".to_string(), None), vec!["web"]);
    assert_eq!(
        build_k8s_logs_args("web".to_string(), Some("side".to_string())),
        vec!["web", "-c", "side"]
    );
}

#[test]
#[should_panic(expected = "expected a filtered run")]
fn raw_k8s_alias_is_not_a_filtered_run() {
    let _args = build_k8s_namespace_args(
        "pods",
        None,
        false,
        vec!["-o".to_string(), "wide".to_string()],
    )
    .filtered_args();
}

#[test]
fn empty_or_bare_negation_is_not_a_native_test_expression() {
    assert!(!is_native_test_expression(&[]));
    assert!(!is_native_test_expression(&["!".to_string()]));
    assert!(is_native_test_expression(&[
        "(".to_string(),
        "-f".to_string(),
        "x".to_string()
    ]));
}

#[test]
fn malformed_config_falls_back_to_default_awareness() {
    let dir = test_isolation::tempdir();
    let junk = "this is [not valid toml\n= = =\n";
    for rel in [
        "rtk/config.toml",
        ".config/rtk/config.toml",
        "config/rtk/config.toml",
        "Library/Application Support/rtk/config.toml",
        ".rtk/config.toml",
    ] {
        let path = dir.path().join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, junk).unwrap();
    }
    let level = std::cell::Cell::new(None);
    test_isolation::with_root(dir.path(), || {
        level.set(Some(configured_awareness_level()));
    });
    let level = level.into_inner().expect("closure ran");
    assert_eq!(
        std::mem::discriminant(&level),
        std::mem::discriminant(&crate::core::config::AwarenessLevel::default())
    );
}

#[cfg(unix)]
mod binary {
    use crate::core::test_isolation;
    use std::ffi::OsString;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::process::{Output, Stdio};

    fn fake_tool(dir: &Path, name: &str, body: &str) {
        let log = dir.join("calls.log");
        let script = format!(
            "#!/bin/sh\necho \"{name} $*\" >> '{}'\n{body}\n",
            log.display()
        );
        let path = dir.join(name);
        std::fs::write(&path, script).unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
    }

    fn reset_log(dir: &Path) {
        let _ = std::fs::remove_file(dir.join("calls.log"));
    }

    fn calls(dir: &Path) -> Vec<Vec<String>> {
        std::fs::read_to_string(dir.join("calls.log"))
            .unwrap_or_default()
            .lines()
            .map(|l| l.split_whitespace().map(str::to_string).collect())
            .collect()
    }

    fn called_with(dir: &Path, tools: &[&str], words: &[&str]) -> bool {
        calls(dir).iter().any(|call| {
            call.first().is_some_and(|t| tools.contains(&t.as_str()))
                && words.iter().all(|w| call.iter().skip(1).any(|x| x == w))
        })
    }

    fn rtk(dir: &Path, inherit_path: bool, args: &[&str]) -> Output {
        let mut cmd = test_isolation::rtk_command();
        let inherited: Option<OsString> = cmd
            .get_envs()
            .find(|(k, _)| *k == "PATH")
            .and_then(|(_, v)| v)
            .map(|v| v.to_os_string());
        let mut path = dir.as_os_str().to_os_string();
        if let Some(p) = inherited.filter(|_| inherit_path) {
            path.push(":");
            path.push(p);
        }
        cmd.env("PATH", path)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("rtk runs")
    }

    fn run_cases(dir: &Path, tools: &[&str], cases: &[(&[&str], &[&str])]) {
        for (args, words) in cases {
            reset_log(dir);
            let out = rtk(dir, true, args);
            assert!(
                called_with(dir, tools, words),
                "{args:?} did not reach {tools:?} with {words:?}; calls: {:?}; stderr: {}",
                calls(dir),
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }

    #[test]
    fn git_global_options_reach_the_passthrough() {
        if !test_isolation::rtk_binary_is_built() {
            return;
        }
        let dir = test_isolation::tempdir();
        fake_tool(dir.path(), "git", "exit 0");
        let argv = [
            "-C",
            "somewhere",
            "-c",
            "user.name=x",
            "--git-dir",
            "gd",
            "--work-tree",
            "wt",
            "--no-pager",
            "--no-optional-locks",
            "--bare",
            "--literal-pathspecs",
            "rev-parse",
            "HEAD",
        ];
        let mut args = vec!["git"];
        args.extend_from_slice(&argv);
        let out = rtk(dir.path(), true, &args);
        assert_eq!(
            out.status.code(),
            Some(0),
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            called_with(dir.path(), &["git"], &argv),
            "calls: {:?}",
            calls(dir.path())
        );
    }

    #[test]
    fn git_subcommands_reach_git() {
        if !test_isolation::rtk_binary_is_built() {
            return;
        }
        let dir = test_isolation::tempdir();
        fake_tool(dir.path(), "git", "exit 0");
        run_cases(
            dir.path(),
            &["git"],
            &[
                (&["git", "commit", "-m", "msg"], &["commit"]),
                (&["git", "push", "origin", "main"], &["push"]),
                (&["git", "pull"], &["pull"]),
                (&["git", "fetch"], &["fetch"]),
                (&["git", "worktree", "list"], &["worktree"]),
            ],
        );
    }

    #[test]
    fn pnpm_global_flags_and_filters_reach_pnpm() {
        if !test_isolation::rtk_binary_is_built() {
            return;
        }
        let dir = test_isolation::tempdir();
        fake_tool(dir.path(), "pnpm", "exit 0");
        run_cases(
            dir.path(),
            &["pnpm"],
            &[
                (&["pnpm", "-r", "list"], &["-r"]),
                (
                    &["pnpm", "--filter", "@a", "install"],
                    &["install", "--filter=@a"],
                ),
                (&["pnpm", "-w", "why", "lodash"], &["-w", "why", "lodash"]),
            ],
        );

        reset_log(dir.path());
        let out = rtk(
            dir.path(),
            true,
            &["pnpm", "-r", "--filter", "@a", "typecheck"],
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("--filter is not yet supported for pnpm tsc"),
            "stderr: {stderr}"
        );
        assert!(
            stderr.contains("pnpm tsc does not support -r"),
            "stderr: {stderr}"
        );
    }

    #[test]
    fn docker_subcommands_reach_docker() {
        if !test_isolation::rtk_binary_is_built() {
            return;
        }
        let dir = test_isolation::tempdir();
        fake_tool(dir.path(), "docker", "exit 0");
        fake_tool(dir.path(), "docker-compose", "exit 0");
        run_cases(
            dir.path(),
            &["docker", "docker-compose"],
            &[
                (&["docker", "ps"], &["ps"]),
                (&["docker", "ps", "-a"], &["ps"]),
                (&["docker", "logs", "web"], &["logs", "web"]),
                (&["docker", "compose", "ps"], &["ps"]),
                (
                    &["docker", "compose", "logs", "api", "--tail", "5"],
                    &["logs", "api"],
                ),
                (&["docker", "compose", "build", "api"], &["build", "api"]),
                (&["docker", "compose", "up", "-d"], &["up", "-d"]),
                (&["docker", "system", "df"], &["system", "df"]),
            ],
        );
    }

    #[test]
    fn kubectl_and_oc_subcommands_reach_their_tool() {
        if !test_isolation::rtk_binary_is_built() {
            return;
        }
        let dir = test_isolation::tempdir();
        fake_tool(dir.path(), "kubectl", "exit 0");
        fake_tool(dir.path(), "oc", "exit 0");
        run_cases(
            dir.path(),
            &["kubectl"],
            &[
                (&["kubectl", "services", "-n", "ns"], &["ns"]),
                (
                    &["kubectl", "services", "-o", "wide"],
                    &["services", "-o", "wide"],
                ),
                (
                    &["kubectl", "logs", "mypod", "-c", "side"],
                    &["mypod", "side"],
                ),
                (
                    &["kubectl", "describe", "pod", "x"],
                    &["describe", "pod", "x"],
                ),
            ],
        );
        run_cases(
            dir.path(),
            &["oc"],
            &[
                (&["oc", "get", "pods"], &["get", "pods"]),
                (&["oc", "pods", "-n", "ns"], &["ns"]),
                (&["oc", "pods", "-o", "wide"], &["pods", "-o", "wide"]),
                (&["oc", "services", "-n", "ns"], &["ns"]),
                (&["oc", "services", "-o", "wide"], &["services", "wide"]),
                (&["oc", "logs", "p1", "-c", "c1"], &["p1", "c1"]),
                (&["oc", "new-project", "t"], &["new-project", "t"]),
            ],
        );
    }

    #[test]
    fn psql_and_dotnet_reach_their_tool() {
        if !test_isolation::rtk_binary_is_built() {
            return;
        }
        let dir = test_isolation::tempdir();
        fake_tool(dir.path(), "psql", "exit 0");
        fake_tool(dir.path(), "dotnet", "exit 0");
        run_cases(
            dir.path(),
            &["psql"],
            &[(&["psql", "-c", "select 1"], &["select"])],
        );
        run_cases(
            dir.path(),
            &["dotnet"],
            &[
                (&["dotnet", "restore"], &["restore"]),
                (&["dotnet", "tool", "list"], &["tool", "list"]),
            ],
        );
    }

    #[test]
    fn local_views_succeed_on_real_files() {
        if !test_isolation::rtk_binary_is_built() {
            return;
        }
        let dir = test_isolation::tempdir();

        let json = dir.path().join("data.json");
        std::fs::write(&json, r#"{"name":"rtk","count":1}"#).unwrap();
        let out = rtk(dir.path(), true, &["json", json.to_str().unwrap()]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(String::from_utf8_lossy(&out.stdout).contains("name"));

        let log = dir.path().join("app.log");
        std::fs::write(&log, "ERROR boom\nINFO ok\nINFO ok\n").unwrap();
        let out = rtk(dir.path(), true, &["log", log.to_str().unwrap()]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );

        let out = rtk(dir.path(), true, &["log"]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );

        let out = rtk(dir.path(), true, &["env", "-f", "PATH"]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );

        let src = dir.path().join("lib.rs");
        std::fs::write(
            &src,
            "use std::fmt;\n\npub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n",
        )
        .unwrap();
        let out = rtk(dir.path(), true, &["smart", src.to_str().unwrap()]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn fallback_reports_an_unrunnable_program_with_127() {
        if !test_isolation::rtk_binary_is_built() {
            return;
        }
        let dir = test_isolation::tempdir();
        let out = rtk(dir.path(), true, &[".."]);
        assert_eq!(out.status.code(), Some(127));
        assert!(String::from_utf8_lossy(&out.stderr).contains("[rtk: "));
    }

    #[test]
    fn fallback_runs_a_filtered_tool_and_keeps_its_exit_code() {
        if !test_isolation::rtk_binary_is_built() {
            return;
        }
        // Missing tool: PATH holds only an empty directory.
        let empty = test_isolation::tempdir();
        let out = rtk(empty.path(), false, &["liquibase", "status"]);
        assert_eq!(out.status.code(), Some(127));
        assert!(String::from_utf8_lossy(&out.stderr).contains("[rtk: "));

        let dir = test_isolation::tempdir();
        fake_tool(
            dir.path(),
            "liquibase",
            "echo 'Liquibase Community banner' >&2\necho 'status ok'\nexit 0",
        );
        let out = rtk(dir.path(), true, &["liquibase", "status"]);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(called_with(dir.path(), &["liquibase"], &["status"]));

        let failing = test_isolation::tempdir();
        fake_tool(
            failing.path(),
            "liquibase",
            "echo 'Liquibase Community banner' >&2\necho 'update failed'\nexit 3",
        );
        let out = rtk(failing.path(), true, &["liquibase", "update"]);
        assert_eq!(out.status.code(), Some(3));
        assert!(called_with(failing.path(), &["liquibase"], &["update"]));
    }
}
