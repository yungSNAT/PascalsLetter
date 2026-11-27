use super::*;
use crate::core::test_isolation;
use std::fs;

#[test]
fn event_dispatch_applies_the_length_limit_only_to_pre_tool_payloads() {
    let root = test_isolation::tempdir();
    test_isolation::with_root(root.path(), || {
        let _scope = test_isolation::enter(root.path());
        let pre = json!({
            "hook_event_name": PRE_TOOL_USE_KEY,
            "tool_name": "Bash",
            "tool_input": {
                "command": "printf fixture",
                "description": "Report a fixture value"
            }
        });

        assert!(!handle(&pre, STDIN_CAP).expect("accept a payload at the limit"));
        for payload in [&pre, &Value::Null] {
            let error = handle(payload, STDIN_CAP + 1).expect_err("reject oversized input");
            assert_eq!(
                error.to_string(),
                format!("hook stdin exceeds {STDIN_CAP} byte limit")
            );
        }

        for event in [POST_TOOL_USE_KEY, SESSION_START_KEY, "UnrecognizedEvent"] {
            let payload = json!({"hook_event_name": event});
            assert!(
                handle(&payload, STDIN_CAP + 1).expect("dispatch a non-pre-tool event"),
                "{event} must not fall through to command rewriting"
            );
        }
    });
}

fn handles_without_a_matching_rule(decision: ShellHookDecision) -> bool {
    match decision {
        ShellHookDecision::Rewrite => false,
        ShellHookDecision::NoRewrite => true,
        _ => panic!("a project without pipeline rules should not produce a rule action"),
    }
}

#[test]
fn gates_validate_their_inputs_and_preserve_the_pipeline_fallback() {
    let root = test_isolation::tempdir();
    test_isolation::with_root(root.path(), || {
        let _scope = test_isolation::enter(root.path());
        assert!(!pipeline_config::any_user_pipeline_files());

        let invalid = json!({"tool_name": 42, "tool_input": {"command": ""}});
        assert!(!read_gate(&invalid));
        assert!(!tool_gate(&invalid));
        assert!(!shell_gate(&invalid));
        assert!(!read_gate(&json!({
            "tool_name": "Read",
            "tool_input": {"file_path": 42}
        })));

        let path = root.path().join("small.txt");
        fs::write(&path, "first line\nsecond line\n").expect("write read fixture");
        for (tool, input) in [
            (
                "Read",
                json!({"file_path": path, "offset": null, "limit": null}),
            ),
            ("Read", json!({"file_path": path, "offset": 0})),
            ("NotebookRead", json!({"file_path": path, "limit": 1})),
        ] {
            assert!(!read_gate(&json!({"tool_name": tool, "tool_input": input})));
        }

        let tool = "Glob";
        let glob = json!({"tool_name": tool});
        let profile = rtk_engine::harness::profile("claude-code").expect("Claude Code profile");
        let subject = profile.map_tool(tool);
        let facts = pipeline_config::subject_facts(subject, tool, &Value::Null);
        assert!(!read_gate(&glob));
        assert_eq!(
            tool_gate(&glob),
            handles_without_a_matching_rule(pipeline_config::before_tool_decision(subject, &facts))
        );

        let command = "printf fixture";
        let shell = json!({"tool_name": "Bash", "tool_input": {"command": command}});
        assert!(!tool_gate(&shell), "Bash belongs to the shell gate");
        assert_eq!(
            shell_gate(&shell),
            handles_without_a_matching_rule(pipeline_config::before_shell_decision(command))
        );
    });
}

#[test]
fn a_stopped_shell_command_is_reported_as_handled() {
    let root = test_isolation::tempdir();
    test_isolation::with_root(root.path(), || {
        let _scope = test_isolation::enter(root.path());
        let command = "printf fixture";
        let payload = json!({
            "session_id": "coverage-session",
            "tool_name": "Bash",
            "tool_input": {"command": command}
        });
        assert!(
            stop(
                &payload,
                command,
                "Use the configured fixture tool instead",
                "coverage-shell-route",
                "route"
            ),
            "a pipeline stop must not fall through to command rewriting"
        );
    });
}
