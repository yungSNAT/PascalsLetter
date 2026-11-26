use super::*;
use crate::core::test_isolation;
use std::fs;
use std::io::Seek;
use std::path::Path;
use std::process::Output;

#[test]
fn rewrite_responses_preserve_non_object_arguments() {
    user_env::with_vars(&[("RTK_HOOK_AUDIT", None)], || {
        let args = json!(["cargo test", {"timeout": 30_000}]);
        let cli = copilot_cli_response_from_decision(
            &args,
            HookDecision::AskRewrite("rtk cargo test".into(), AskOwner::Host),
            "cargo test",
        );
        assert_eq!(
            cli,
            Some(json!({
                "permissionDecisionReason": "RTK auto-rewrite",
                "modifiedArgs": args
            }))
        );

        let pre_tool_use =
            pre_tool_use_rewrite_output(&json!({"tool_input": args}), "rtk cargo test", None);
        assert_eq!(
            pre_tool_use,
            json!({
                "hookSpecificOutput": {
                    "hookEventName": PRE_TOOL_USE_KEY,
                    "permissionDecisionReason": "RTK auto-rewrite",
                    "updatedInput": args
                }
            })
        );
    });
}

#[test]
fn gemini_distinguishes_missing_shell_work_from_a_denied_command() {
    for payload in [
        json!({"tool_name": "read_file", "tool_input": {"command": "cargo test"}}),
        json!({"tool_name": "run_shell_command", "tool_input": {"command": ""}}),
    ] {
        let output = run_gemini_inner_impl(&payload.to_string(), |_| {
            panic!("a non-shell tool or empty command must not reach the decision callback")
        })
        .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&output).unwrap(),
            json!({"decision": "allow"})
        );
    }

    let shell = json!({
        "tool_name": "run_shell_command",
        "tool_input": {"command": "cargo test"}
    });
    let output =
        run_gemini_inner_with_rules(&shell.to_string(), &["cargo test".to_string()], &[], &[])
            .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&output).unwrap(),
        json!({"decision": "deny", "reason": "Blocked by RTK permission rule"})
    );
}

#[test]
fn antigravity_accepts_cmd_as_the_last_fallback() {
    let root = test_isolation::tempdir();
    test_isolation::with_root(root.path(), || {
        let _entered = test_isolation::enter(root.path());
        user_env::with_vars(&[("RTK_HOOK_AUDIT", None)], || {
            let run = |args: Value| {
                run_antigravity_inner(
                    &json!({"toolCall": {"name": "run_command", "args": args}}).to_string(),
                )
            };

            let fallback = run(json!({"cmd": "cargo test"}));
            assert_eq!(fallback["decision"], "allow");
            assert_eq!(fallback["overwrite"]["CommandLine"], "rtk cargo test");

            for args in [
                json!({"cwd": "."}),
                json!({"cmd": ""}),
                json!({"CommandLine": "", "cmd": "cargo test"}),
            ] {
                assert_eq!(run(args), json!({"decision": "allow"}));
            }
        });
    });
}

#[test]
fn cursor_and_droid_handle_payloads_without_a_command() {
    for input in [
        r#"{"tool_name":"Bash"}"#,
        r#"{"tool_input":{"command":""}}"#,
        r#"{"tool_input":{"command":42}}"#,
    ] {
        assert_eq!(run_cursor_inner(input), "{}", "{input}");
    }

    assert_eq!(droid_payload("\u{feff} \t\n").unwrap(), None);
    assert!(droid_payload("\u{feff}{").is_err());
}

fn invoke_hook(project: &Path, hook: &str, input: &str) -> Output {
    let mut payload = tempfile::tempfile_in(project).unwrap();
    payload.write_all(input.as_bytes()).unwrap();
    payload.rewind().unwrap();

    let output = test_isolation::rtk_command()
        .current_dir(project)
        .args(["hook", hook])
        .stdin(payload)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "hook {hook} returned {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[test]
fn native_hooks_keep_their_protocol_on_blank_malformed_and_commandless_input() {
    if !test_isolation::rtk_binary_ready() {
        return;
    }

    let project = test_isolation::tempdir();
    for hook in ["claude", "trae", "codex", "cursor", "droid"] {
        for (input, malformed) in [("\u{feff} \n\t", false), ("{", true), ("{}", false)] {
            let output = invoke_hook(project.path(), hook, input);
            if hook == "cursor" {
                assert_eq!(
                    serde_json::from_slice::<Value>(&output.stdout).unwrap(),
                    json!({}),
                    "{hook} on {input:?}"
                );
            } else {
                assert!(output.stdout.is_empty(), "{hook} on {input:?}");
            }

            let stderr = String::from_utf8_lossy(&output.stderr);
            assert_eq!(
                stderr.contains("[rtk hook] Failed to parse JSON input:"),
                malformed && hook != "cursor",
                "{hook} on {input:?}: {stderr}"
            );
        }
    }
}

#[test]
fn codex_accepts_one_mib_but_reports_oversized_input_without_a_reply() {
    if !test_isolation::rtk_binary_ready() {
        return;
    }

    let project = test_isolation::tempdir();
    let mut input = format!("{{}}{}", " ".repeat(1_048_576 - 2));
    let at_limit = invoke_hook(project.path(), "codex", &input);
    assert!(at_limit.stdout.is_empty());
    assert!(at_limit.stderr.is_empty());

    input.push(' ');
    let over_limit = invoke_hook(project.path(), "codex", &input);
    assert!(over_limit.stdout.is_empty());
    let diagnostic = String::from_utf8_lossy(&over_limit.stderr);
    assert!(
        diagnostic.contains(
            "[rtk hook] Failed to read JSON input: hook stdin exceeds 1048576 byte limit"
        ),
        "{diagnostic}"
    );
}

#[test]
fn copilot_heals_a_project_registration_without_removing_custom_hooks() {
    use crate::hooks::constants::{COPILOT_HOOK_FILE, GITHUB_DIR, HOOKS_SUBDIR};

    if !test_isolation::rtk_binary_ready() {
        return;
    }

    let project = test_isolation::tempdir();
    let hooks_dir = project.path().join(GITHUB_DIR).join(HOOKS_SUBDIR);
    fs::create_dir_all(&hooks_dir).unwrap();
    let config_path = hooks_dir.join(COPILOT_HOOK_FILE);
    let expected = json!({
        "version": 1,
        "hooks": {
            "PreToolUse": [
                {"type": "command", "command": "rtk hook copilot"},
                {"type": "command", "command": "echo custom"}
            ],
            "postToolUse": [{"type": "command", "bash": "echo done"}]
        }
    });
    let mut legacy = expected.clone();
    legacy["hooks"]["preToolUse"] = json!([{
        "type": "command",
        "bash": "rtk hook copilot",
        "powershell": "rtk hook copilot",
        "cwd": ".",
        "timeoutSec": 5
    }]);
    fs::write(&config_path, legacy.to_string()).unwrap();

    let input = json!({
        "toolName": "bash",
        "toolArgs": json!({"command": "rtk cargo test"}).to_string()
    });
    let output = invoke_hook(project.path(), "copilot", &input.to_string());
    assert!(
        output.stdout.is_empty(),
        "an already rewritten command is silent"
    );
    let healed: Value = serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
    assert_eq!(healed, expected);
}

#[test]
fn hook_decision_logging_recovers_after_insert_and_open_failures() {
    let root = test_isolation::tempdir();
    let preferred_db = root.path().join("history.db");
    test_isolation::with_root(root.path(), || {
        user_env::with_path("RTK_DB_PATH", Some(&preferred_db), || {
            let record = |id: &str| {
                log_hook_decision(
                    &json!({"session_id": "coverage-session", "tool_use_id": id}),
                    "cargo test",
                    HookOutcome::Ask,
                    Some("rtk cargo test"),
                );
            };

            record("first");
            let history = walkdir::WalkDir::new(root.path())
                .into_iter()
                .map(|entry| entry.expect("read the isolated tracking directory"))
                .find(|entry| entry.file_type().is_file() && entry.file_name() == "history.db")
                .expect("the first decision should create the isolated history database")
                .into_path();

            {
                let db = rusqlite::Connection::open(&history).unwrap();
                let count = |id: &str| -> i64 {
                    db.query_row(
                        "SELECT COUNT(*) FROM hook_decisions \
                         WHERE session_id = ?1 AND tool_use_id = ?2",
                        ["coverage-session", id],
                        |row| row.get(0),
                    )
                    .unwrap()
                };
                assert_eq!(count("first"), 1);

                db.execute_batch(
                    "CREATE TRIGGER coverage_reject_hook BEFORE INSERT ON hook_decisions
                     WHEN NEW.tool_use_id = 'rejected'
                     BEGIN SELECT RAISE(ABORT, 'fixture rejects this decision'); END;",
                )
                .unwrap();
                record("rejected");
                assert_eq!(count("rejected"), 0);
                assert_eq!(count("first"), 1);

                db.execute_batch("DROP TRIGGER coverage_reject_hook;")
                    .unwrap();
                record("rejected");
                assert_eq!(count("rejected"), 1);
            }

            let backup = history.with_extension("backup");
            fs::rename(&history, &backup).unwrap();
            fs::create_dir(&history).unwrap();
            assert!(crate::core::tracking::Tracker::new().is_err());
            record("blocked");
            assert!(history.is_dir(), "logging must not replace the obstruction");

            fs::remove_dir(&history).unwrap();
            fs::rename(&backup, &history).unwrap();
            record("recovered");
            let db = rusqlite::Connection::open(&history).unwrap();
            let ids = db
                .prepare(
                    "SELECT tool_use_id FROM hook_decisions
                     WHERE session_id = 'coverage-session' ORDER BY tool_use_id",
                )
                .unwrap()
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            assert_eq!(ids, ["first", "recovered", "rejected"]);
        });
    });
}
