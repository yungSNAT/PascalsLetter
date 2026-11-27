use super::*;

#[test]
fn gt_log_at_entry_limit_does_not_report_more_entries() {
    let entries = (0..MAX_LOG_ENTRIES)
        .map(|index| format!("◉ entry-{index}"))
        .collect::<Vec<_>>()
        .join("\n");
    let input = format!("{entries}\n│ detail after the last node\n~\n");

    assert_eq!(filter_gt_log_entries(&input), entries);
}

#[test]
fn gt_stack_filters_handle_output_without_branch_details() {
    assert_eq!(
        filter_gt_submit("Pushed to origin\n  \nAlready pushed to backup"),
        "pushed 2 branches"
    );
    assert_eq!(
        filter_gt_submit("  Nothing to submit today. \n"),
        "Nothing to submit today."
    );
    assert_eq!(
        filter_gt_sync("Synced with remote\n  \nDeleted stale tracking references"),
        "ok sync: 1 synced, 1 deleted"
    );
    assert_eq!(
        filter_gt_sync("Deleted branch 'old/topic'\n\nDeleted stale tracking references"),
        "ok sync: 2 deleted (old/topic)"
    );
    assert_eq!(filter_gt_sync("Everything is up to date."), "ok synced");
    assert_eq!(
        filter_gt_restack("No branches need restacking."),
        "ok restacked"
    );
}

#[test]
fn gt_other_rejects_an_empty_invocation() {
    let error = run_other(&[], 0).expect_err("an empty invocation must not run gt");
    assert_eq!(error.to_string(), "gt: no subcommand specified");
}

#[cfg(unix)]
#[cfg(feature = "engine")]
mod cli {
    use crate::core::test_isolation;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::process::{Command, Stdio};

    fn write_tool(root: &Path, name: &str, script: &str) {
        let bin = root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let tool = bin.join(name);
        fs::write(&tool, script).unwrap();
        fs::set_permissions(tool, fs::Permissions::from_mode(0o700)).unwrap();
    }

    fn isolated_command(root: &Path) -> Command {
        let mut command = test_isolation::rtk_command();
        command.current_dir(root).stdin(Stdio::null());
        command
    }

    fn command_with_tools(root: &Path) -> Command {
        let mut command = isolated_command(root);
        let inherited_path = command
            .get_envs()
            .find(|(key, _)| *key == "PATH")
            .and_then(|(_, value)| value)
            .expect("isolated rtk command must provide PATH");
        let path = std::env::join_paths(
            std::iter::once(root.join("bin")).chain(std::env::split_paths(inherited_path)),
        )
        .unwrap();
        command.env("PATH", path);
        command
    }

    #[test]
    fn gt_cli_filters_output_and_preserves_arguments_and_exit_status() {
        if !test_isolation::rtk_binary_ready() {
            return;
        }

        let scratch = test_isolation::tempdir();
        write_tool(
            scratch.path(),
            "gt",
            r#"#!/bin/sh
case "$#:$1:$2:$3" in
    '3:log:short:short topic')
        printf '\033[32m* short developer@example.test\033[0m\n'
        ;;
    '3:log:long:long topic')
        printf '\033[32m* long developer@example.test\033[0m\n'
        ;;
    '2:log:topic:')
        printf '\033[32m* regular developer@example.test\033[0m\n'
        ;;
    '2:submit:topic:')
        printf '%s\n' 'Preparing the stack for submission' 'Pushed branch topic' 'Created pull request #42 for topic'
        ;;
    '1:sync::')
        printf '%s\n' 'Checking every branch in the stack against its remote' 'Synced with remote' 'Deleted branch old-topic'
        ;;
    '1:restack::')
        printf '%s\n' 'Checking the stack for branches that need to be rebased' 'No branches need restacking.'
        ;;
    '2:create:topic:')
        printf '%s\n' 'Preparing a new branch from the current stack' 'Created branch topic'
        ;;
    '1:branch::')
        printf '\033[32mfixture current branch\033[0m\n'
        printf '%s\n' 'fixture branch diagnostic' >&2
        exit 7
        ;;
    '2:fixture-command:literal argument:')
        printf '%s\n' 'fixture passthrough output'
        exit 9
        ;;
    *)
        printf 'unexpected fixture arguments: <%s>\n' "$@" >&2
        exit 99
        ;;
esac
"#,
        );
        write_tool(scratch.path(), "git", "#!/bin/sh\nexit 98\n");

        let cases: &[(&[&str], i32, &str, &str)] = &[
            (
                &["gt", "log", "short", "short topic"],
                0,
                "* short developer@example.test",
                "",
            ),
            (&["gt", "log", "long", "long topic"], 0, "* long", ""),
            (
                &["-v", "gt", "log", "long", "long topic"],
                0,
                "* long developer@example.test",
                "Running: gt log long long topic",
            ),
            (&["gt", "log", "topic"], 0, "* regular", ""),
            (
                &["gt", "submit", "topic"],
                0,
                "pushed topic\ncreated PR #42 topic",
                "",
            ),
            (
                &["gt", "sync"],
                0,
                "ok sync: 1 synced, 1 deleted (old-topic)",
                "",
            ),
            (&["gt", "restack"], 0, "ok restacked", ""),
            (&["gt", "create", "topic"], 0, "ok created topic", ""),
            (
                &["gt", "branch"],
                7,
                "fixture current branch",
                "fixture branch diagnostic",
            ),
            (
                &["gt", "fixture-command", "literal argument"],
                9,
                "fixture passthrough output",
                "",
            ),
        ];

        for &(args, code, expected_stdout, expected_stderr) in cases {
            let output = command_with_tools(scratch.path())
                .args(args)
                .output()
                .unwrap();
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);

            assert_eq!(output.status.code(), Some(code), "{args:?}: {stderr}");
            assert!(
                stdout
                    .lines()
                    .zip(expected_stdout.lines())
                    .all(|(actual, expected)| actual == expected)
                    && stdout.lines().count() >= expected_stdout.lines().count(),
                "{args:?}: expected stdout to start with {expected_stdout:?}, got {stdout:?}"
            );
            assert!(!stdout.contains('\x1b'), "{args:?}: {stdout:?}");
            if !expected_stderr.is_empty() {
                assert!(
                    stderr.lines().any(|line| line == expected_stderr),
                    "{args:?}: expected {expected_stderr:?} in stderr, got {stderr:?}"
                );
            }
        }
    }

    #[test]
    fn gt_cli_routes_git_aliases_to_git_instead_of_graphite() {
        if !test_isolation::rtk_binary_ready() {
            return;
        }

        let scratch = test_isolation::tempdir();
        write_tool(scratch.path(), "gt", "#!/bin/sh\nexit 99\n");
        write_tool(
            scratch.path(),
            "git",
            r#"#!/bin/sh
printf ' <%s>' "$@" >> "$RTK_GT_TEST_TRACE"
printf '\n' >> "$RTK_GT_TEST_TRACE"
case "$1" in
    status) printf '%s\n' '## main' ;;
esac
exit 0
"#,
        );

        let cases: &[&[&str]] = &[
            &["status"],
            &["diff"],
            &["show"],
            &["add", "sample.txt"],
            &["push"],
            &["pull"],
            &["fetch"],
            &["stash", "list"],
            &["worktree", "list"],
        ];
        fs::write(scratch.path().join("sample.txt"), "sample\n").unwrap();

        for &args in cases {
            let subcommand = args[0];
            let trace = scratch.path().join(format!("{subcommand}.trace"));
            let output = command_with_tools(scratch.path())
                .env("RTK_GT_TEST_TRACE", &trace)
                .arg("gt")
                .args(args)
                .output()
                .unwrap();

            assert_eq!(
                output.status.code(),
                Some(0),
                "{args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let invocations = fs::read_to_string(trace)
                .unwrap_or_else(|error| panic!("gt {subcommand} did not invoke git: {error}"));
            let expected = match subcommand {
                "stash" => "<stash> <list>".to_owned(),
                "add" => "<add> <sample.txt>".to_owned(),
                _ => format!("<{subcommand}>"),
            };
            assert!(
                invocations.contains(&expected),
                "{args:?}: expected {expected:?} in git invocations, got {invocations:?}"
            );
        }
    }

    #[test]
    fn gt_cli_reports_context_when_graphite_cannot_be_started() {
        if !test_isolation::rtk_binary_ready() {
            return;
        }

        let scratch = test_isolation::tempdir();
        // Search only the empty scratch directory so a host installation cannot mask the error.
        let output = isolated_command(scratch.path())
            .env("PATH", scratch.path())
            .args(["gt", "log", "long"])
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(!output.status.success(), "{stderr}");
        assert!(
            stderr.contains("Failed to run gt log long. Is gt (Graphite) installed?"),
            "{stderr}"
        );
    }
}
