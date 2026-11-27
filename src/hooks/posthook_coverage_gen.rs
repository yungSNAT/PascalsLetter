use super::*;
use rtk_engine::pipeline::{Scope, SourceFile, parse, resolve};

fn pipeline_of(text: &str) -> Resolved {
    let parsed = parse(text, "coverage.toml");
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let (resolved, diagnostics) = resolve(&[SourceFile {
        scope: Scope::User,
        label: "coverage.toml".to_string(),
        file: parsed.file.expect("parses"),
    }]);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
    resolved
}

fn posthook() -> Resolved {
    pipeline_of("[pipeline]\nextends = \"rtk:posthook\"\n")
}

fn quiet_config() -> Config {
    let mut config = Config::default();
    config.retriever.mode = crate::core::retriever::RecoveryMode::Disabled;
    config
}

fn tee_config(dir: &std::path::Path) -> Config {
    let mut config = Config::default();
    config.retriever.tee_directory = Some(dir.to_path_buf());
    config
}

fn repetitive_bash() -> Value {
    serde_json::from_str(include_str!(
        "../../tests/fixtures/posthook/bash_repetitive_lines.json"
    ))
    .expect("fixture parses")
}

fn web_pipeline() -> Resolved {
    pipeline_of(
        "[pipeline]\nname = \"web-pipe\"\n\
         [[hook.after_tool.web]]\nname = \"web-squeeze\"\nuse = [\"dedup\"]\n",
    )
}

fn websearch_event(noisy: &str) -> Value {
    json!({
        "tool_name": "WebSearch",
        "tool_input": {"query": "rust coverage"},
        "tool_response": {
            "query": "rust coverage",
            "results": [
                "Summary of results",
                {"tool_use_id": "abc", "content": [{"title": "t", "url": "https://example.com"}]},
                noisy
            ],
            "durationSeconds": 1.0
        },
        "duration_ms": 7
    })
}

fn shell_tokens(out: &str) -> usize {
    let v: Value = serde_json::from_str(out).expect("valid JSON");
    let response = &v["hookSpecificOutput"]["updatedToolOutput"];
    let stdout = response["stdout"].as_str().unwrap_or("");
    let stderr = response["stderr"].as_str().unwrap_or("");
    estimate_tokens(&combine_streams(stdout, stderr))
}

#[test]
fn kill_switches_stop_processing_before_any_load() {
    let event = repetitive_bash();
    let scratch = crate::core::test_isolation::tempdir();
    crate::core::test_isolation::with_root(scratch.path(), || {
        let pipeline =
            crate::core::pipeline_config::user_pipeline_path().expect("user config root");
        std::fs::create_dir_all(pipeline.parent().expect("pipeline dir"))
            .expect("create config dir");
        std::fs::write(&pipeline, "[pipeline]\nextends = \"rtk:posthook\"\n")
            .expect("write user pipeline");
        assert!(
            process(&event).is_some(),
            "premise: without a kill switch the event is filtered"
        );
        let disabled =
            crate::core::user_env::with_vars(&[("RTK_DISABLED", Some("1"))], || process(&event));
        assert!(disabled.is_none(), "RTK_DISABLED=1 must emit nothing");
        let posthook_off = crate::core::user_env::with_vars(
            &[("RTK_DISABLED", Some("0")), ("RTK_POSTHOOK", Some("0"))],
            || process(&event),
        );
        assert!(posthook_off.is_none(), "RTK_POSTHOOK=0 must emit nothing");
    });
}

#[test]
fn compact_grep_and_glob_shapes_are_tracked_passthroughs() {
    let listing = (0..1000)
        .map(|i| format!("src/module_{i}/file.rs"))
        .collect::<Vec<_>>()
        .join("\n");
    let grep = json!({
        "tool_name": "Grep",
        "tool_input": {"pattern": "fn", "path": "src"},
        "tool_response": {"mode": "files_with_matches", "content": listing.clone(), "numFiles": 1000},
        "duration_ms": 2
    });
    assert_eq!(source_of(&grep, "Grep"), "src");
    let (raw, format) = extract(&grep, "Grep", "src").expect("grep extracts");
    assert_eq!(raw, listing);
    assert!(format.is_none(), "files_with_matches is unsupported");
    assert!(process_with_config(&grep, &quiet_config(), &posthook()).is_none());

    let names: Vec<String> = (0..1000).map(|i| format!("src/m{i}/lib.rs")).collect();
    let glob = json!({
        "tool_name": "Glob",
        "tool_input": {"pattern": "**/*.rs"},
        "tool_response": {"filenames": names, "numFiles": 1000},
        "duration_ms": 1
    });
    let (joined, format) = extract(&glob, "Glob", "").expect("glob extracts");
    assert_eq!(joined.lines().count(), 1000);
    assert!(format.is_none());
    assert!(process_with_config(&glob, &quiet_config(), &posthook()).is_none());
}

#[test]
fn log_entry_falls_back_to_source_when_no_command() {
    let applied = Applied {
        text: "x".to_string(),
        methods: vec!["dedup"],
        rules: vec!["a".to_string(), "b".to_string()],
        pipeline: "p".to_string(),
        subject: Subject::Web,
        shadow: true,
    };
    let entry = log_entry_for(Subject::Web, None, "https://docs.rs/x", &applied, 10, 4);
    assert_eq!(entry.command.as_deref(), Some("https://docs.rs/x"));
    assert_eq!(entry.rule, "a+b");
    assert!(entry.shadow);
    assert_eq!(entry.tokens_before, Some(10));
    assert_eq!(entry.tokens_after, Some(4));
    let empty = log_entry_for(Subject::Web, None, "", &applied, 1, 1);
    assert!(empty.command.is_none());
}

#[test]
fn mcp_selection_runs_the_chain_and_reports_attribution_fields() {
    let pipeline = pipeline_of(
        "[pipeline]\nname = \"mcp-pipe\"\n\
         [[hook.after_tool.mcp]]\nname = \"squeeze\"\nuse = [\"dedup\", \"cap\"]\n",
    );
    let noisy = std::iter::repeat_n("repeated mcp line", 400)
        .collect::<Vec<_>>()
        .join("\n");
    let applied = run_selected(
        &pipeline,
        Subject::Mcp,
        "mcp__linear__get_issue",
        ContentFormat::Term,
        "",
        &noisy,
        5,
        None,
    )
    .expect("mcp rule applies");
    assert!(applied.text.contains("[x400]"), "{}", applied.text);
    assert!(applied.rules.iter().any(|r| r == "squeeze"));
    assert!(applied.methods.contains(&"dedup"));
    assert_eq!(applied.pipeline, "mcp-pipe");
    assert!(!applied.shadow);
}

#[test]
fn recall_commands_bypass_filtering() {
    let config = quiet_config();
    let pipeline = posthook();
    let event = repetitive_bash();
    assert!(
        process_with_config(&event, &config, &pipeline).is_some(),
        "premise: the plain command is filtered"
    );
    for cmd in ["echo hi; rtk recall 3f9a1c", "cat ./tee/posthook/notes.txt"] {
        let mut ev = event.clone();
        ev["tool_input"]["command"] = json!(cmd);
        assert!(
            process_with_config(&ev, &config, &pipeline).is_none(),
            "{cmd} must see raw bytes"
        );
    }
}

#[test]
fn unchanged_stderr_is_kept_verbatim_next_to_filtered_stdout() {
    let mut event = repetitive_bash();
    event["tool_response"]["stderr"] = json!("warning: deprecated flag");
    let out =
        process_with_config(&event, &quiet_config(), &posthook()).expect("stdout still filtered");
    let v: Value = serde_json::from_str(&out).expect("valid JSON");
    let response = &v["hookSpecificOutput"]["updatedToolOutput"];
    assert_eq!(response["stderr"], "warning: deprecated flag");
    assert!(response["stdout"].as_str().unwrap().contains("[x40]"));
}

#[test]
fn shell_output_with_recall_hint_never_grows() {
    let tee_dir = tempfile::tempdir().expect("tempdir");
    let config = tee_config(tee_dir.path());
    let pipeline = posthook();

    let event = repetitive_bash();
    let raw_stdout = event["tool_response"]["stdout"].as_str().unwrap_or("");
    let raw_stderr = event["tool_response"]["stderr"].as_str().unwrap_or("");
    let raw_tokens = estimate_tokens(&combine_streams(raw_stdout, raw_stderr));
    let out = process_with_config(&event, &config, &pipeline).expect("repetitive output filtered");
    assert!(shell_tokens(&out) <= raw_tokens);
    assert!(out.contains("[x40]"));

    let unique = (0..3000)
        .map(|i| format!("unique build line number {i} with details"))
        .collect::<Vec<_>>()
        .join("\n");
    let stdout = format!("\x1b[1m{unique}");
    let ev = json!({
        "tool_name": "Bash",
        "tool_input": {"command": "make build"},
        "tool_response": {"stdout": stdout, "stderr": "", "interrupted": false, "isImage": false},
        "duration_ms": 3
    });
    let raw_tokens = estimate_tokens(&stdout);
    let out = process_with_config(&ev, &config, &pipeline).expect("unique output still filtered");
    assert!(
        shell_tokens(&out) <= raw_tokens,
        "never emit a larger output"
    );
}

#[test]
fn mcp_output_with_recall_hint_never_grows() {
    let tee_dir = tempfile::tempdir().expect("tempdir");
    let config = tee_config(tee_dir.path());
    let pipeline = pipeline_of(
        "[pipeline]\nname = \"mcp-pipe\"\n\
         [[hook.after_tool.mcp]]\nname = \"squeeze\"\nuse = [\"dedup\"]\n",
    );
    let unique = (0..2000)
        .map(|i| format!("record {i} value {}", i * 7))
        .collect::<Vec<_>>()
        .join("\n");
    let barely = format!("{unique}\ndup line\ndup line");
    let noisy = std::iter::repeat_n("repeated mcp line", 400)
        .collect::<Vec<_>>()
        .join("\n");
    for (text, must_filter) in [(noisy, true), (barely, false)] {
        let event = json!({
            "tool_name": "mcp__linear__get_issue",
            "tool_input": {},
            "tool_response": {"result": text.clone()},
            "duration_ms": 5
        });
        let out = process_with_config(&event, &config, &pipeline);
        assert!(
            out.is_some() || !must_filter,
            "repeated lines must be filtered"
        );
        if let Some(out) = out {
            let v: Value = serde_json::from_str(&out).expect("valid JSON");
            let result = v["hookSpecificOutput"]["updatedToolOutput"]["result"]
                .as_str()
                .expect("result string");
            assert!(estimate_tokens(result) <= estimate_tokens(&text));
        }
    }
}

#[test]
fn websearch_string_results_are_filtered_in_place() {
    let noisy = std::iter::repeat_n("repeated web line", 300)
        .collect::<Vec<_>>()
        .join("\n");
    let event = websearch_event(&noisy);
    let out = process_with_config(&event, &quiet_config(), &web_pipeline())
        .expect("websearch results rewritten");
    let v: Value = serde_json::from_str(&out).expect("valid JSON");
    let hook = &v["hookSpecificOutput"];
    assert_eq!(hook["hookEventName"], POST_TOOL_USE_KEY);
    let updated = &hook["updatedToolOutput"];
    assert_eq!(updated["query"], "rust coverage");
    let results = updated["results"].as_array().expect("results array");
    assert_eq!(results.len(), 3);
    assert_eq!(results[0], "Summary of results");
    assert_eq!(
        results[1], event["tool_response"]["results"][1],
        "link objects untouched"
    );
    assert!(results[2].as_str().unwrap().contains("[x300]"));

    let tee_dir = tempfile::tempdir().expect("tempdir");
    let raw_concat = format!("Summary of results\n\n{noisy}");
    let out = process_with_config(&event, &tee_config(tee_dir.path()), &web_pipeline())
        .expect("websearch results rewritten with a recall hint");
    let v: Value = serde_json::from_str(&out).expect("valid JSON");
    let joined = v["hookSpecificOutput"]["updatedToolOutput"]["results"]
        .as_array()
        .expect("results")
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("\n\n");
    assert!(estimate_tokens(&joined) <= estimate_tokens(&raw_concat));
}

#[test]
fn tiny_websearch_is_below_min_size() {
    let event = json!({
        "tool_name": "WebSearch",
        "tool_input": {"query": "q"},
        "tool_response": {"results": ["hi"]}
    });
    assert!(process_with_config(&event, &quiet_config(), &web_pipeline()).is_none());
}
