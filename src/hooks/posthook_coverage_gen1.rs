use super::*;

#[test]
fn shell_emits_ansi_cleanup_only_when_the_recall_hint_fits() {
    let scratch = crate::core::test_isolation::tempdir();
    crate::core::test_isolation::with_root(scratch.path(), || {
        let label = "shell-ansi-test.toml";
        let parsed = rtk_engine::pipeline::parse(
            r#"
[guards]
min_size = 1

[[hook.after_tool.shell]]
name = "strip-color"
use = ["ansi"]
"#,
            label,
        );
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let (pipeline, diagnostics) =
            rtk_engine::pipeline::resolve(&[rtk_engine::pipeline::SourceFile {
                scope: rtk_engine::pipeline::Scope::User,
                label: label.to_string(),
                file: parsed.file.expect("valid shell pipeline"),
            }]);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");

        let clean = (0..160)
            .map(|i| format!("Build step {i}: checked module dependencies"))
            .collect::<Vec<_>>()
            .join("\n");
        let stderr = "One optional component was skipped.";
        let event = json!({
            "tool_name": "Bash",
            "tool_input": {"command": "make build"},
            "tool_response": {
                "stdout": format!("\x1b[32m{clean}\x1b[0m"),
                "stderr": stderr,
                "interrupted": false,
                "isImage": false
            }
        });

        let mut no_recall = Config::default();
        no_recall.retriever.mode = crate::core::retriever::RecoveryMode::Disabled;
        let output = process_shell(&event, &no_recall, &pipeline, 17)
            .expect("stripping ANSI without a recall hint should fit");
        let output: Value = serde_json::from_str(&output).expect("valid hook response");
        assert_eq!(
            output,
            json!({
                "hookSpecificOutput": {
                    "hookEventName": "PostToolUse",
                    "updatedToolOutput": {
                        "stdout": clean,
                        "stderr": stderr,
                        "interrupted": false,
                        "isImage": false
                    }
                }
            })
        );

        let mut with_recall = Config::default();
        with_recall.retriever.tee_directory = Some(scratch.path().join("recall"));
        assert!(
            process_shell(&event, &with_recall, &pipeline, 17).is_none(),
            "the recall hint costs more than removing one pair of ANSI escapes saves"
        );
    });
}

#[test]
fn extract_classifies_json_in_both_tool_families_and_rejects_nontext_grep() {
    let raw = " \n{\"count\": 2}\n ";
    for (tool, response) in [
        ("mcp__example__list", json!({"result": raw})),
        ("Agent", json!({"content": [{"type": "text", "text": raw}]})),
    ] {
        let event = json!({"tool_response": response});
        let (text, format) = extract(&event, tool, "").expect("extractable JSON text");
        assert_eq!(text, raw, "{tool}: extraction preserves the original text");
        assert_eq!(format.map(|f| f.as_str()), Some("json"), "{tool}");
    }

    for response in [
        json!({"mode": "content"}),
        json!({"mode": "content", "content": ["not a string payload"]}),
    ] {
        let event = json!({"tool_response": response});
        assert!(extract(&event, "Grep", "").is_none());
    }
}

#[test]
fn rebuild_replaces_only_the_webfetch_result_and_refuses_mutation_tools() {
    let event = json!({
        "tool_response": {
            "result": "original page text",
            "status": 200,
            "cached": true
        }
    });
    let replacement = "shorter page text";
    assert_eq!(
        rebuild(&event, "WebFetch", replacement),
        Some((
            json!({"result": replacement, "status": 200, "cached": true}),
            false
        ))
    );

    let malformed = json!({"tool_response": "original page text"});
    assert!(rebuild(&malformed, "WebFetch", replacement).is_none());

    for tool in ["Write", "Edit", "MultiEdit", "NotebookEdit"] {
        assert!(
            rebuild(&event, tool, replacement).is_none(),
            "{tool} must not be rebuilt even when its response looks like replaceable text"
        );
    }
}
