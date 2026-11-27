use super::*;
use crate::core::test_isolation::tempdir;

#[test]
fn discovery_skips_top_level_files_and_filters_stale_sessions() {
    let projects = tempdir();
    let subagents = projects.path().join("project").join("subagents");
    fs::create_dir_all(&subagents).unwrap();
    fs::write(projects.path().join("not-a-project.jsonl"), "").unwrap();

    let recent = subagents.join("recent.jsonl");
    let stale = subagents.join("stale.jsonl");
    fs::write(&recent, "").unwrap();
    fs::File::create(&stale)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(30 * 86_400))
        .unwrap();

    let mut unfiltered =
        ClaudeProvider::discover_sessions_in_projects_dir(projects.path(), None, None).unwrap();
    unfiltered.sort_unstable();
    assert_eq!(unfiltered, [recent.clone(), stale]);

    let filtered =
        ClaudeProvider::discover_sessions_in_projects_dir(projects.path(), None, Some(7)).unwrap();
    assert_eq!(filtered, [recent]);
}

#[cfg(unix)]
#[test]
fn discovery_reports_a_projects_symlink_loop_as_an_access_error() {
    let root = tempdir();
    let projects = root.path().join("projects");
    std::os::unix::fs::symlink("projects", &projects).unwrap();

    let error =
        ClaudeProvider::discover_sessions_in_projects_dir(&projects, None, None).unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("failed to access {}", projects.display())
    );
}

#[test]
fn transcript_readers_keep_valid_tool_data_among_malformed_records() {
    let root = tempdir();
    let path = root.path().join("mixed.jsonl");
    let lines = [
        "plain text without a tool marker",
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash""#,
        r#"{"type":"assistant","message":{"content":{"type":"tool_use","name":"Bash","id":"not-array","input":{"command":"ignored"}}}}"#,
        r#"{"type":"summary","message":{"content":[{"type":"tool_use","name":"Bash","id":"summary","input":{"command":"ignored"}}]}}"#,
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"missing id"}},{"type":"tool_use","name":"Bash","id":"bash","input":{"command":"printf hello"}},{"type":"tool_use","name":"Read","id":"read"}]}}"#,
        r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"missing id"},{"type":"tool_result","tool_use_id":"bash","content":"café","is_error":true},{"type":"tool_result","tool_use_id":"read","content":"ok"},{"type":"tool_result","tool_use_id":"opaque","content":{"text":"not a text block array"}}]}}"#,
        r#"{"type":"user","message":{"content":{"type":"tool_result","tool_use_id":"bash","content":"not an array"}}}"#,
    ];
    let mut transcript = b"\xff\n".to_vec();
    transcript.extend_from_slice(lines.join("\n").as_bytes());
    fs::write(&path, transcript).unwrap();

    let provider = ClaudeProvider;
    let commands = provider.extract_commands(&path).unwrap();
    assert_eq!(commands.len(), 1);
    let command = &commands[0];
    assert_eq!(command.command, "printf hello");
    assert_eq!(command.tool_use_id, "bash");
    assert_eq!(command.sequence_index, 0);
    assert_eq!(command.output_content.as_deref(), Some("café"));
    assert_eq!(command.output_len, Some(5));
    assert!(command.is_error);

    assert_eq!(
        provider.tool_output_bytes(&path).unwrap(),
        ToolOutputBytes {
            bash: 5,
            all: 7,
            unparsed_lines: 2,
        }
    );
}

#[cfg(unix)]
#[test]
fn tool_output_bytes_counts_a_directory_read_failure_once() {
    // Opening a directory can succeed on Unix, but reading it as a stream fails.
    let directory = tempdir();
    assert_eq!(
        ClaudeProvider.tool_output_bytes(directory.path()).unwrap(),
        ToolOutputBytes {
            unparsed_lines: 1,
            ..ToolOutputBytes::default()
        }
    );
}
