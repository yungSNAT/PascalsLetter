use super::*;
use crate::core::test_isolation;
use serde_json::json;
use std::fs;
use std::io::Write;
use std::path::Path;

const PRACTICE: &str = "practice-project";
const QUIET: &str = "quiet-project";
const SESSION: &str = "609ef1a8-5dee-425a-b342-b79d5bc79a01.jsonl";

fn correction_commands() -> Vec<CommandExecution> {
    [
        (
            "git status --porcelian",
            true,
            "error: unknown option `porcelian'\nusage: git status [<options>]",
        ),
        ("git status --porcelain", false, " M README.md"),
        (
            "cargo test --libb",
            true,
            "error: unexpected argument '--libb' found\n\n  tip: a similar argument exists: '--lib'",
        ),
        ("cargo test --lib", false, "test result: ok. 1 passed"),
    ]
    .into_iter()
    .cycle()
    .take(8)
    .map(|(command, is_error, output)| CommandExecution {
        command: command.into(),
        is_error,
        output: output.into(),
    })
    .collect()
}

fn write_session(projects: &Path, project: &str, commands: &[CommandExecution]) {
    let directory = projects.join(project);
    fs::create_dir_all(&directory).unwrap();
    let mut file = fs::File::create(directory.join(SESSION)).unwrap();
    let timestamp = jiff::Timestamp::now().to_string();

    for (index, command) in commands.iter().enumerate() {
        let id = format!("toolu_coverage_{index}");
        let request = json!({
            "type": "assistant",
            "timestamp": timestamp,
            "message": {
                "role": "assistant",
                "content": [{
                    "type": "tool_use",
                    "name": "Bash",
                    "id": id,
                    "input": { "command": command.command }
                }]
            }
        });
        let result = json!({
            "type": "user",
            "timestamp": timestamp,
            "message": {
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": id,
                    "is_error": command.is_error,
                    "content": command.output
                }]
            }
        });
        writeln!(file, "{request}\n{result}").unwrap();
    }

    let unfinished = json!({
        "type": "assistant",
        "timestamp": timestamp,
        "message": {
            "role": "assistant",
            "content": [{
                "type": "tool_use",
                "name": "Bash",
                "id": "toolu_unfinished",
                "input": { "command": "git status" }
            }]
        }
    });
    writeln!(file, "{unfinished}").unwrap();
}

#[test]
fn learn_accepts_empty_incomplete_and_mixed_sessions() {
    let scratch = test_isolation::tempdir();
    test_isolation::with_root(scratch.path(), || {
        let _workdir = test_isolation::enter(scratch.path());
        let projects = user_dirs::home()
            .expect("isolated home")
            .join(".claude")
            .join("projects");

        assert!(run(None, false, 7, "json".into(), false, 0.0, 1).is_ok());

        write_session(&projects, QUIET, &[]);
        assert!(run(Some(QUIET.into()), false, 7, "json".into(), false, 0.0, 1).is_ok());

        let commands = correction_commands();
        write_session(&projects, PRACTICE, &commands);
        fs::write(
            projects
                .join(PRACTICE)
                .join("609ef1a8-5dee-425a-b342-b79d5bc79a02.jsonl"),
            [0xff, b'\n'],
        )
        .unwrap();

        assert!(
            run(
                Some(PRACTICE.into()),
                false,
                7,
                "json".into(),
                false,
                0.0,
                1
            )
            .is_ok()
        );
        assert!(run(None, true, 7, "text".into(), false, 0.0, 2).is_ok());
    });
}

#[cfg(all(unix, feature = "engine"))]
#[test]
fn learn_cli_reports_filters_and_writes_corrections() {
    if !test_isolation::rtk_binary_or_skip() {
        return;
    }

    let scratch = test_isolation::tempdir();
    let claude = scratch.path().join(".claude");
    let projects = claude.join("projects");
    let rules_path = claude.join("rules").join("cli-corrections.md");
    let commands = correction_commands();
    write_session(&projects, QUIET, &[]);
    write_session(&projects, PRACTICE, &commands);

    let corrections = find_corrections(&commands);
    let correction_count = corrections.len();
    let expected_rules = deduplicate_corrections(corrections);
    assert!(
        !expected_rules.is_empty(),
        "fixture must yield a correction"
    );

    let learn = |args: &[&str]| {
        let output = test_isolation::rtk_command()
            .env("HOME", scratch.path())
            .current_dir(scratch.path())
            .args(["learn", "--since", "7"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "learn {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };

    assert!(learn(&[]).contains("No Claude Code sessions found in the last 7 days."));
    assert!(learn(&["--project", QUIET]).contains("No CLI corrections detected in 1 sessions."));

    let report: serde_json::Value = serde_json::from_str(&learn(&[
        "--all",
        "--format",
        "json",
        "--min-confidence",
        "0",
        "--min-occurrences",
        "1",
        "--write-rules",
    ]))
    .unwrap();
    assert_eq!(report["sessions_scanned"], json!(2));
    assert_eq!(report["total_corrections"], json!(correction_count));
    let reported_rules = report["rules"].as_array().unwrap();
    assert_eq!(reported_rules.len(), expected_rules.len());
    for rule in &expected_rules {
        assert!(reported_rules.contains(&json!({
            "wrong": rule.wrong_pattern,
            "right": rule.right_pattern,
            "error_type": rule.error_type.as_str(),
            "occurrences": rule.occurrences,
            "base_command": rule.base_command
        })));
    }
    assert!(
        !rules_path.exists(),
        "JSON output must not write a rules file"
    );

    let filtered: serde_json::Value = serde_json::from_str(&learn(&[
        "--project",
        PRACTICE,
        "--format",
        "json",
        "--min-confidence",
        "0",
        "--min-occurrences",
        "100",
    ]))
    .unwrap();
    assert_eq!(filtered["sessions_scanned"], json!(1));
    assert_eq!(filtered["total_corrections"], json!(correction_count));
    assert_eq!(filtered["rules"], json!([]));

    let text = learn(&[
        "--project",
        PRACTICE,
        "--min-confidence",
        "0",
        "--min-occurrences",
        "1",
        "--write-rules",
    ]);
    assert!(text.contains("Written to:"));
    let markdown = fs::read_to_string(rules_path).unwrap();
    for rule in &expected_rules {
        assert!(text.contains(rule.right_pattern.as_str()));
        assert!(markdown.contains(rule.wrong_pattern.as_str()));
        assert!(markdown.contains(rule.right_pattern.as_str()));
    }
}
