#[test]
fn command_labels_handle_missing_and_present_subcommands() {
    for (command, subcommand, label) in [
        (" \t ", "", ""),
        ("  ls  ", "", "ls"),
        ("\tgit\tstatus --short ", "status", "git status"),
    ] {
        assert_eq!(super::extract_subcmd(command), subcommand);
        assert_eq!(super::truncate_command(command), label);
    }
}

#[cfg(unix)]
mod cli {
    use super::super::*;
    use crate::core::{test_isolation, user_env};
    use serde_json::{Value, json};
    use std::fs;
    use std::io::Write;
    use std::path::PathBuf;
    use std::process::Output;

    struct Fixture {
        root: tempfile::TempDir,
        work: PathBuf,
        claude: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let root = test_isolation::tempdir();
            let work = root.path().join("workspace");
            let claude = root.path().join(".claude");
            fs::create_dir_all(&work).unwrap();
            fs::create_dir_all(claude.join("projects")).unwrap();
            Self {
                root,
                work: work.canonicalize().unwrap(),
                claude,
            }
        }

        fn session(&self, project: &str, name: &str, commands: &[(&str, Option<&str>)]) -> PathBuf {
            let project_dir = self.claude.join("projects").join(project);
            fs::create_dir_all(&project_dir).unwrap();
            let path = project_dir.join(name);
            let mut file = fs::File::create(&path).unwrap();
            let timestamp = Timestamp::now().to_string();
            for (index, &(command, result)) in commands.iter().enumerate() {
                let id = format!("toolu_coverage_{index}");
                let invocation = json!({
                    "type": "assistant",
                    "timestamp": timestamp,
                    "message": {
                        "role": "assistant",
                        "content": [{
                            "type": "tool_use", "name": "Bash", "id": id,
                            "input": { "command": command }
                        }]
                    }
                });
                writeln!(file, "{invocation}").unwrap();
                if let Some(content) = result {
                    let response = json!({
                        "type": "user",
                        "timestamp": timestamp,
                        "message": {
                            "role": "user",
                            "content": [{
                                "type": "tool_result", "tool_use_id": id, "content": content
                            }]
                        }
                    });
                    writeln!(file, "{response}").unwrap();
                }
            }
            path
        }

        fn discover(&self, project: Option<&str>, format: &str) -> Output {
            let mut cmd = test_isolation::rtk_command();
            cmd.current_dir(&self.work)
                .env("HOME", self.root.path())
                .env("CLAUDE_CONFIG_DIR", &self.claude)
                .env("XDG_CONFIG_HOME", self.root.path().join("config"))
                .env("XDG_DATA_HOME", self.root.path().join("data"))
                .env("RTK_DB_PATH", self.root.path().join("coverage.db"))
                .env("NO_COLOR", "1")
                .args(["discover", "--since", "7", "--verbose", "--format", format]);
            if let Some(project) = project {
                cmd.args(["--project", project]);
            }
            let output = cmd.output().unwrap();
            assert!(
                output.status.success(),
                "discover failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            output
        }

        fn tracking_database(&self) -> PathBuf {
            walkdir::WalkDir::new(self.root.path())
                .into_iter()
                .filter_map(|entry| entry.ok())
                .filter(|entry| entry.file_type().is_file())
                .map(walkdir::DirEntry::into_path)
                .filter(|path| path.extension().is_some_and(|ext| ext == "db"))
                .find(|path| {
                    rusqlite::Connection::open_with_flags(
                        path,
                        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                    )
                    .unwrap()
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name = 'hook_decisions')",
                        [],
                        |row| row.get::<_, bool>(0),
                    )
                    .unwrap()
                })
                .expect("discover should initialize a tracking database in the scratch root")
        }

        fn empty_report(&self, scope: report::ScanScope) -> DiscoverReport {
            let agent_status = test_isolation::with_root(self.root.path(), || {
                let _working_dir = test_isolation::enter(&self.work);
                user_env::with_vars(&[("CLAUDE_CONFIG_DIR", None), ("CODEX_HOME", None)], || {
                    report::AgentIntegrationStatus::detect()
                })
            });
            DiscoverReport {
                sessions_scanned: 0,
                scope,
                total_commands: 0,
                already_rtk: 0,
                already_rtk_estimated: 0,
                measured_since: None,
                since_days: 7,
                supported: vec![],
                unsupported: vec![],
                parse_errors: 0,
                rtk_disabled_count: 0,
                rtk_disabled_estimated: 0,
                rtk_disabled_examples: vec![],
                agent_status,
            }
        }
    }

    fn assert_report(stdout: &[u8], expected: DiscoverReport) {
        let actual: Value = serde_json::from_slice(stdout).unwrap();
        let wanted: Value = serde_json::from_str(&report::format_json(&expected)).unwrap();
        // Compare the fields driven by the scan without depending on how the
        // formatter names them or on the ambient agent-integration diagnostics.
        let control: Value = serde_json::from_str(&report::format_json(&DiscoverReport {
            sessions_scanned: 20,
            scope: report::ScanScope::AllProjects,
            total_commands: 100,
            already_rtk: 5,
            already_rtk_estimated: 4,
            measured_since: Some("2000-01-01".to_string()),
            since_days: 1,
            supported: vec![],
            unsupported: vec![],
            parse_errors: 3,
            rtk_disabled_count: 2,
            rtk_disabled_estimated: 1,
            rtk_disabled_examples: vec!["control".to_string()],
            ..expected
        }))
        .unwrap();
        for key in wanted
            .as_object()
            .unwrap()
            .keys()
            .chain(control.as_object().unwrap().keys())
        {
            if wanted.get(key) != control.get(key) {
                assert_eq!(actual.get(key), wanted.get(key), "report field {key}");
            }
        }
    }

    fn missed_entry(command: &str, subcommand: &str, output_len: Option<usize>) -> SupportedEntry {
        let Classification::Supported {
            rtk_equivalent,
            category,
            estimated_savings_pct,
            status,
        } = classify_command(command)
        else {
            panic!("expected a supported fixture command: {command}");
        };
        let raw_tokens =
            output_len.map_or_else(|| category_avg_tokens(category, subcommand), |len| len / 4);
        let saved = (raw_tokens as f64 * estimated_savings_pct / 100.0) as usize;
        let pct = if raw_tokens == 0 {
            0.0
        } else {
            100.0 * saved as f64 / raw_tokens as f64
        };
        SupportedEntry {
            command: command
                .split_whitespace()
                .take(2)
                .collect::<Vec<_>>()
                .join(" "),
            count: 1,
            rtk_equivalent,
            category,
            estimated_savings_tokens: saved,
            estimated_savings_pct: pct,
            rtk_status: match format!("{status:?}").as_str() {
                "Passthrough" => report::RtkStatus::Passthrough,
                "NotSupported" => report::RtkStatus::NotSupported,
                _ => report::RtkStatus::Existing,
            },
        }
    }

    #[test]
    fn filtered_scan_counts_coverage_and_bypasses_even_when_log_reads_fail() {
        if !test_isolation::rtk_binary_is_built() {
            return;
        }
        let fixture = Fixture::new();
        let hook = fixture.claude.join("hooks").join("rtk-rewrite.sh");
        fs::create_dir_all(hook.parent().unwrap()).unwrap();
        fs::write(&hook, "#!/bin/sh\nexec rtk hook claude\n").unwrap();
        fs::write(
            fixture.claude.join("settings.json"),
            json!({
                "hooks": {
                    "PreToolUse": [{
                        "matcher": "Bash",
                        "hooks": [
                            { "type": "command", "command": "rtk hook claude" },
                            { "type": "command", "command": hook }
                        ]
                    }]
                }
            })
            .to_string(),
        )
        .unwrap();

        let (unsupported, base_command) = [
            "frobnicate first",
            "terraform plan",
            "brew info",
            "sudo whoami",
        ]
        .into_iter()
        .find_map(|command| match classify_command(command) {
            Classification::Unsupported { base_command } => Some((command, base_command)),
            _ => None,
        })
        .expect("an unsupported fixture command");
        let project = "coverage-selected";
        fixture.session(
            project,
            "selected.jsonl",
            &[
                ("git status && rtk git diff", None),
                ("rtk bypass git diff", None),
                ("RTK_DISABLED=1 docker ps", None),
                ("RTK_DISABLED='1 2' docker ps --all", None),
                ("RTK_DISABLED=1 cargo test", None),
                ("RTK_DISABLED=1 frobnicate x", None),
                (unsupported, None),
                (unsupported, None),
            ],
        );
        fixture.session("coverage-other", "other.jsonl", &[("rtk ls", None)]);
        let broken = fixture.session(project, "broken.jsonl", &[]);
        fs::write(&broken, [0xff, b'\n']).unwrap();
        let parse_errors = usize::from(ClaudeProvider.extract_commands(&broken).is_err());

        let healthy = fixture.discover(Some(project), "json");
        let stderr = String::from_utf8_lossy(&healthy.stderr);
        assert!(stderr.contains("Scanning 2 session files..."), "{stderr}");
        assert!(stderr.contains("selected.jsonl"), "{stderr}");
        assert!(!stderr.contains("other.jsonl"), "{stderr}");
        assert_eq!(stderr.contains("Warning: skipping "), parse_errors != 0);
        assert_report(
            &healthy.stdout,
            DiscoverReport {
                sessions_scanned: 2,
                total_commands: 9,
                already_rtk: 2,
                already_rtk_estimated: 1,
                unsupported: vec![UnsupportedEntry {
                    base_command,
                    count: 2,
                    example: unsupported.to_string(),
                }],
                parse_errors,
                rtk_disabled_count: 3,
                rtk_disabled_estimated: 3,
                rtk_disabled_examples: vec![
                    "docker ps (2x)".to_string(),
                    "cargo test (1x)".to_string(),
                ],
                ..fixture.empty_report(report::ScanScope::ProjectFilter(project.to_string()))
            },
        );

        // Leave the table and its indexes in place so initialization still
        // works, but make the queries' original column names invalid.
        {
            let db = rusqlite::Connection::open(fixture.tracking_database()).unwrap();
            let mut stmt = db.prepare("PRAGMA table_info(hook_decisions)").unwrap();
            let columns = stmt
                .query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            assert!(!columns.is_empty());
            for (index, column) in columns.iter().enumerate() {
                db.execute_batch(&format!(
                    "ALTER TABLE hook_decisions RENAME COLUMN \"{}\" TO coverage_hidden_{index}",
                    column.replace('"', "\"\"")
                ))
                .unwrap();
            }
        }
        let degraded = fixture.discover(Some(project), "json");
        let stderr = String::from_utf8_lossy(&degraded.stderr);
        assert!(
            stderr.contains("failed to read hook_decisions log ("),
            "{stderr}"
        );
        assert!(
            stderr.contains("failed to read hook_decisions log timestamp ("),
            "{stderr}"
        );
        assert_eq!(degraded.stdout, healthy.stdout);
    }

    #[test]
    fn current_project_scan_reports_misses_without_a_hook_or_database() {
        if !test_isolation::rtk_binary_is_built() {
            return;
        }
        let fixture = Fixture::new();
        let project = ClaudeProvider::encode_project_path(&fixture.work.to_string_lossy());
        let tiny_output_cmd = [
            "git merge topic",
            "git tag sample",
            "git add README.md",
            "git push",
            "docker run sample",
        ]
        .into_iter()
        .find(|command| {
            matches!(
                classify_command(command),
                Classification::Supported { status, .. }
                    if matches!(format!("{status:?}").as_str(), "Passthrough" | "NotSupported")
            ) && registry::rewrite_command_precompiled(command, &[], &[]).is_some()
        })
        .unwrap_or("git status");
        fixture.session(
            &project,
            "current.jsonl",
            &[
                ("RTK_DISABLED=1 docker ps", None),
                (tiny_output_cmd, Some("x")),
            ],
        );
        fixture.session("coverage-neighbor", "neighbor.jsonl", &[("rtk ls", None)]);

        let initial = fixture.discover(None, "json");
        assert_report(
            &initial.stdout,
            DiscoverReport {
                sessions_scanned: 1,
                total_commands: 2,
                supported: vec![
                    missed_entry("docker ps", "ps", None),
                    missed_entry(tiny_output_cmd, extract_subcmd(tiny_output_cmd), Some(1)),
                ],
                ..fixture.empty_report(report::ScanScope::CurrentProject(project))
            },
        );

        let database = fixture.tracking_database();
        fs::remove_file(&database).unwrap();
        fs::create_dir(&database).unwrap();
        let text = fixture.discover(None, "text");
        let stderr = String::from_utf8_lossy(&text.stderr);
        assert!(
            stderr.contains("failed to open tracking database"),
            "{stderr}"
        );
        assert!(stderr.contains("Scanning 1 session files..."), "{stderr}");
        assert!(stderr.contains("current.jsonl"), "{stderr}");
        assert!(!stderr.contains("neighbor.jsonl"), "{stderr}");
        let stdout = String::from_utf8(text.stdout).unwrap();
        assert!(stdout.contains("docker ps"), "{stdout}");
        let fallback = fixture.discover(None, "json");
        assert_eq!(fallback.stdout, initial.stdout);
    }
}
