use super::*;

#[test]
fn go_without_a_subcommand_returns_a_usage_error() {
    let error = run_other(&[], 0).unwrap_err();
    assert_eq!(error.to_string(), "go: no subcommand specified");
}

#[test]
fn go_test_json_ignores_incomplete_records_but_keeps_package_diagnostics() {
    let input = r#"
not a JSON event
{"Action":"build-output","Output":"unattributed diagnostic"}
{"Action":"build-output","ImportPath":"example.test/compiler"}
{"Action":"build-output","ImportPath":"example.test/compiler","Output":" \t\n"}
{"Action":"build-output","ImportPath":"example.test/compiler","Output":"  main.go:8:2: undefined: Widget  \n"}
{"Action":"fail","Package":"example.test/compiler","FailedBuild":"example.test/compiler"}
{"Action":"output","Package":"example.test/worker"}
{"Action":"output","Package":"example.test/worker","Output":" \t\n"}
{"Action":"output","Package":"example.test/worker","Output":"  test process timed out  \n"}
{"Action":"fail","Package":"example.test/worker"}
"#;

    assert_eq!(
        filter_go_test_json(input),
        concat!(
            "Go test: 0 passed, 2 failed in 2 packages\n",
            "\nworker [FAIL]\n",
            "  test process timed out\n",
            "\ncompiler [build failed]\n",
            "  main.go:8:2: undefined: Widget",
        )
    );
}

#[test]
fn go_test_failure_details_cap_relevant_lines_and_fall_back_to_first_plain_line() {
    let detailed = [
        "=== RUN TestWidget",
        "  widget_test.go:17:",
        "    normalized values differ",
        "  error: first check",
        "  error: second check",
        "  error: third check",
        "  error: beyond the display limit",
    ]
    .map(str::to_owned);
    assert_eq!(
        select_go_test_failure_lines(&detailed),
        [
            "widget_test.go:17:",
            "normalized values differ",
            "error: first check",
            "error: second check",
            "error: third check",
        ]
    );

    let plain = [
        " \t",
        "=== RUN TestWidget",
        "--- FAIL: TestWidget (0.00s)",
        "--- PASS: TestSetup (0.00s)",
        "  worker disconnected  ",
        "another diagnostic",
    ]
    .map(str::to_owned);
    assert_eq!(
        select_go_test_failure_lines(&plain),
        ["worker disconnected"]
    );
    assert!(select_go_test_failure_lines(&plain[..4]).is_empty());
}

#[test]
fn go_build_nonzero_exit_handles_blank_and_unclassified_output() {
    assert_eq!(
        filter_go_build_with_exit(" \n\t\n", 7),
        "Go build: failed (exit 7)"
    );

    let input = (1..=CAP_ERRORS + 2)
        .map(|number| format!("  subprocess message {number}  "))
        .collect::<Vec<_>>()
        .join("\n\n");
    let summary = filter_go_build_with_exit(&input, 7);
    assert_eq!(summary.lines().next(), Some("Go build: failed (exit 7)"));
    assert!(
        summary
            .lines()
            .any(|line| line == "1. subprocess message 1")
    );
    let last_visible = format!("{CAP_ERRORS}. subprocess message {CAP_ERRORS}");
    assert!(summary.lines().any(|line| line == last_visible));
    let first_omitted = format!("subprocess message {}", CAP_ERRORS + 1);
    assert!(!summary.contains(&first_omitted));
    assert!(summary.ends_with("\n… +2 more output lines"));
}

#[test]
fn go_build_and_vet_cap_diagnostics_and_show_a_recall_hint() {
    use crate::core::{test_isolation, user_env};

    let scratch = test_isolation::tempdir();
    let tee_dir = scratch.path().join("tee");
    std::fs::create_dir(&tee_dir).unwrap();

    test_isolation::with_root(scratch.path(), || {
        user_env::with_vars(
            &[("RTK_TEE", Some("1")), ("RTK_TEE_MODE", Some("always"))],
            || {
                user_env::with_path("RTK_TEE_DIR", Some(tee_dir.as_path()), || {
                    for (tool, noun, filter) in [
                        ("build", "errors", filter_go_build as fn(&str) -> String),
                        ("vet", "issues", filter_go_vet),
                    ] {
                        let diagnostics: Vec<_> = (1..=CAP_ERRORS + 2)
                            .map(|number| format!("{tool}.go:{number}:2: fixture diagnostic"))
                            .collect();
                        let raw = diagnostics.join("\n");
                        let summary = filter(&format!("\n{raw}\n"));
                        let mut lines = summary.lines();

                        let heading = format!("Go {tool}: {} {noun}", diagnostics.len());
                        assert_eq!(lines.next(), Some(heading.as_str()));
                        for (index, diagnostic) in diagnostics.iter().take(CAP_ERRORS).enumerate() {
                            let numbered = format!("{}. {diagnostic}", index + 1);
                            assert_eq!(lines.next(), Some(numbered.as_str()));
                        }
                        assert_eq!(lines.next(), Some(""));
                        let omitted = format!("… +2 more {noun}");
                        assert_eq!(lines.next(), Some(omitted.as_str()));
                        for diagnostic in &diagnostics[CAP_ERRORS..] {
                            assert!(!summary.contains(diagnostic));
                        }

                        let recall_id = lines
                            .next()
                            .and_then(|line| line.strip_prefix("  [+2 hidden: rtk recall "))
                            .and_then(|line| line.strip_suffix(']'))
                            .unwrap_or_else(|| panic!("missing {tool} recall hint: {summary}"));
                        assert!(
                            !recall_id.is_empty()
                                && recall_id.bytes().all(|byte| byte.is_ascii_hexdigit()),
                            "invalid {tool} recall identifier: {recall_id:?}"
                        );
                        assert_eq!(lines.next(), None);
                    }
                });
            },
        );
    });
}

#[cfg(all(unix, feature = "engine"))]
mod cli {
    use crate::core::test_isolation;
    use crate::golangci_cmd;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::process::Command;

    fn fake_go(script: &str) -> tempfile::TempDir {
        let dir = test_isolation::tempdir();
        let executable = dir.path().join("go");
        std::fs::write(&executable, format!("#!/bin/sh\n{script}")).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        dir
    }

    fn go_command(fake_bin: &Path) -> Command {
        let mut command = test_isolation::rtk_command();
        let inherited_path = command
            .get_envs()
            .find(|(key, _)| *key == "PATH")
            .and_then(|(_, value)| value)
            .expect("isolated rtk command should supply PATH");
        let path = std::env::join_paths(
            std::iter::once(fake_bin.to_path_buf()).chain(std::env::split_paths(inherited_path)),
        )
        .unwrap();
        command.env("PATH", path);
        command
    }

    fn run_go(mut command: Command, args: &[&str]) -> (String, String) {
        let output = command.args(["-v", "go"]).args(args).output().unwrap();
        let stdout = String::from_utf8(output.stdout).unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(
            output.status.success(),
            "go {args:?} returned {}:\nstdout: {stdout}\nstderr: {stderr}",
            output.status
        );
        (stdout, stderr)
    }

    #[test]
    fn go_wrappers_select_the_filter_and_forward_arguments() {
        if !test_isolation::rtk_binary_ready() {
            return;
        }

        let go = fake_go(
            r#"
case "$*" in
    'test -json example.test/auto'|'test -json example.test/explicit')
        printf '%s\n' '{"Action":"pass","Package":"example.test/pkg","Test":"TestWidget"}'
        ;;
    'test -bench=Widget example.test/bench')
        printf '%s\n' 'BenchmarkWidget-4  100  10 ns/op'
        ;;
    'build example.test/pkg')
        printf '%s\n' 'go: downloading example.test/dependency v1.0.0'
        ;;
    'build -h')
        printf '%s\n' 'build help fixture'
        ;;
    'vet example.test/pkg')
        printf '%s\n' '# example.test/pkg' \
            'widget.go:9:2: fixture diagnostic' \
            'unclassified output that should not appear in the vet summary' >&2
        ;;
    'env GOPATH')
        printf '%s\n' 'fixture workspace'
        printf '%s\n' 'fixture notice' >&2
        ;;
    *)
        printf 'unexpected fake go arguments: %s\n' "$*" >&2
        exit 91
        ;;
esac
"#,
        );

        let (stdout, stderr) = run_go(go_command(go.path()), &["test", "example.test/auto"]);
        assert!(stdout.contains("Go test: 1 passed in 1 packages"));
        assert!(stderr.contains("Running: go test -json example.test/auto"));

        let (stdout, _) = run_go(
            go_command(go.path()),
            &["test", "-json", "example.test/explicit"],
        );
        assert!(
            stdout
                .contains(r#"{"Action":"pass","Package":"example.test/pkg","Test":"TestWidget"}"#)
        );
        assert!(!stdout.contains("Go test:"));

        let (stdout, stderr) = run_go(
            go_command(go.path()),
            &["test", "-bench=Widget", "example.test/bench"],
        );
        assert!(stdout.contains("BenchmarkWidget-4  100  10 ns/op"));
        assert!(stderr.contains("Running: go test -bench=Widget example.test/bench"));

        let (stdout, stderr) = run_go(go_command(go.path()), &["build", "example.test/pkg"]);
        assert!(stdout.contains("Go build: Success"));
        assert!(stderr.contains("Running: go build example.test/pkg"));

        let (stdout, _) = run_go(go_command(go.path()), &["build", "-h"]);
        assert!(stdout.contains("build help fixture"));

        let (stdout, stderr) = run_go(go_command(go.path()), &["vet", "example.test/pkg"]);
        assert!(stdout.contains("Go vet: 1 issues\n1. widget.go:9:2: fixture diagnostic"));
        assert!(stderr.contains("Running: go vet example.test/pkg"));

        let (stdout, stderr) = run_go(go_command(go.path()), &["env", "GOPATH"]);
        assert_eq!(stdout.trim(), "fixture workspace");
        assert!(stderr.contains("Running: go env ..."));
        assert!(stderr.lines().any(|line| line == "fixture notice"));
    }

    #[test]
    fn go_tool_golangci_uses_the_detected_version_and_respects_explicit_format() {
        if !test_isolation::rtk_binary_ready() {
            return;
        }

        let go = fake_go(
            r#"
case "$COVERAGE_LINT_MAJOR:$*" in
    '1:tool golangci-lint --version')
        printf '%s\n' 'golangci-lint has version 1.64.8 built with go1.24.0'
        ;;
    '2:tool golangci-lint --version')
        printf '%s\n' 'golangci-lint has version 2.1.6 built with go1.24.2' >&2
        ;;
    '1:tool golangci-lint run --out-format=json example.test/pkg')
        printf '%s\n' '{' '  "Issues": [],' '  "Report": {"Linters": []}' '}'
        ;;
    '2:tool golangci-lint run --output.json.path stdout example.test/pkg')
        printf '%s\n' '{"Issues":[],"Report":{"Linters":[]}}' 'trailing fixture text'
        printf '%s\n' '  lint fixture notice  ' >&2
        ;;
    *)
        printf 'unexpected fake go arguments: %s\n' "$*" >&2
        exit 91
        ;;
esac
"#,
        );

        let report = r#"{"Issues":[],"Report":{"Linters":[]}}"#;
        let mut v1 = go_command(go.path());
        v1.env("COVERAGE_LINT_MAJOR", "1");
        let (stdout, stderr) = run_go(v1, &["tool", "golangci-lint", "example.test/pkg"]);
        assert_eq!(
            stdout.trim(),
            golangci_cmd::filter_golangci_json(report, 1).trim()
        );
        assert!(stderr.contains("Running: go tool golangci-lint run --out-format=json"));

        let mut v2 = go_command(go.path());
        v2.env("COVERAGE_LINT_MAJOR", "2");
        let (stdout, stderr) = run_go(
            v2,
            &[
                "tool",
                "golangci-lint",
                "--output.json.path",
                "stdout",
                "example.test/pkg",
            ],
        );
        assert_eq!(
            stdout.trim(),
            golangci_cmd::filter_golangci_json(report, 2).trim()
        );
        assert!(stderr.contains("Running: go tool golangci-lint run --output.json.path stdout"));
        assert!(stderr.lines().any(|line| line == "lint fixture notice"));
    }
}
