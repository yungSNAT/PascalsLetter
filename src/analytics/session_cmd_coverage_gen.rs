use super::{ClaudeProvider, SessionProvider, count_rtk_commands, run};
use crate::core::{test_isolation, user_dirs, user_env};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

fn with_project(test: impl FnOnce(&Path, &Path)) {
    let scratch = test_isolation::tempdir();
    test_isolation::with_root(scratch.path(), || {
        let home = user_dirs::home().expect("resolve isolated home");
        let config = home.join(".claude");
        let project = config.join("projects").join("-fixture");
        fs::create_dir_all(&project).expect("create Claude project fixture");
        user_env::with_path("CLAUDE_CONFIG_DIR", Some(&config), || test(&home, &project));
    });
}

fn run_session(home: &Path) -> Option<String> {
    // Run in-process for coverage even when the CLI binary is unavailable.
    run(0).expect("generate session report");

    #[cfg(unix)]
    if test_isolation::rtk_binary_is_built() {
        let output = test_isolation::rtk_command()
            .env("HOME", home)
            .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
            .arg("session")
            .stdin(std::process::Stdio::null())
            .output()
            .expect("run isolated session command");
        assert!(
            output.status.success(),
            "session command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        return Some(String::from_utf8(output.stdout).expect("decode session report"));
    }

    #[cfg(not(unix))]
    let _ = home;
    None
}

fn write_bash_session(project: &Path, id: &str, command: &str) -> PathBuf {
    let call = serde_json::json!({
        "type": "assistant",
        "message": {"role": "assistant", "content": [{
            "type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": command}
        }]}
    });
    let result = serde_json::json!({
        "type": "user",
        "message": {"role": "user", "content": [{
            "type": "tool_result", "tool_use_id": "t1", "content": "ok"
        }]}
    });
    let path = project.join(format!("{id}.jsonl"));
    fs::write(&path, format!("{call}\n{result}\n")).expect("write Bash session");
    path
}

fn assert_row(stdout: &str, expected_columns: &[&str]) {
    let row = stdout
        .lines()
        .find(|line| line.split_whitespace().next() == Some(expected_columns[0]))
        .unwrap_or_else(|| panic!("missing session row for {}\n{stdout}", expected_columns[0]));
    let columns: Vec<_> = row
        .split_whitespace()
        .take(expected_columns.len())
        .collect();
    assert_eq!(columns.as_slice(), expected_columns);
}

#[test]
fn distinguishes_no_recent_logs_from_logs_without_bash() {
    with_project(|home, project| {
        let provider = ClaudeProvider;
        assert!(
            provider
                .discover_sessions(None, Some(30))
                .expect("discover empty project")
                .is_empty()
        );
        if let Some(stdout) = run_session(home) {
            assert!(stdout.contains("No Claude Code sessions found in the last 30 days."));
            assert!(stdout.contains("Make sure Claude Code has been used at least once."));
            assert!(!stdout.contains("No sessions with Bash commands found."));
        }

        let notes = project.join("11111111-1111-4111-8111-111111111111.jsonl");
        fs::write(
            &notes,
            r#"{"type":"user","message":{"role":"user","content":"Hello"}}"#,
        )
        .expect("write a session without tools");

        let broken = project.join("22222222-2222-4222-8222-222222222222.jsonl");
        fs::write(&broken, [0xff, b'\n']).expect("write a corrupt session log");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // Discovery can stat the log, but an unprivileged reader cannot open it.
            fs::set_permissions(&broken, fs::Permissions::from_mode(0o000))
                .expect("make the corrupt session log unreadable");
        }

        let discovered = provider
            .discover_sessions(None, Some(30))
            .expect("discover both session logs");
        assert_eq!(discovered.len(), 2);
        assert!(discovered.contains(&notes));
        assert!(discovered.contains(&broken));
        assert!(
            provider
                .extract_commands(&notes)
                .expect("read notes log")
                .is_empty()
        );
        if let Some(stdout) = run_session(home) {
            assert!(stdout.contains("No sessions with Bash commands found."));
            assert!(!stdout.contains("No Claude Code sessions found"));
            assert!(!stdout.contains("RTK Session Overview"));
        }
    });
}

#[test]
fn labels_yesterday_and_older_sessions_by_log_modification_time() {
    with_project(|home, project| {
        let now = SystemTime::now();
        // Set both ages halfway between day boundaries.
        for (id, command, hours) in [
            ("33333333-3333-4333-8333-333333333333", "rtk git status", 36),
            ("44444444-4444-4444-8444-444444444444", "echo done", 84),
        ] {
            let path = write_bash_session(project, id, command);
            fs::File::options()
                .write(true)
                .open(path)
                .expect("open session log to set its age")
                .set_modified(now - Duration::from_secs(hours * 3600))
                .expect("set session log age");
        }

        assert_eq!(
            ClaudeProvider
                .discover_sessions(None, Some(30))
                .expect("discover dated logs")
                .len(),
            2
        );
        if let Some(stdout) = run_session(home) {
            assert_row(&stdout, &["33333333", "Yesterday", "1", "1", "100%"]);
            assert_row(&stdout, &["44444444", "3d", "ago", "1", "0", "0%"]);
            assert!(stdout.contains("Average adoption: 50%"));
        }
    });
}

#[test]
fn reports_zero_adoption_when_a_bash_entry_has_only_chain_separators() {
    with_project(|home, project| {
        let path = write_bash_session(project, "55555555-5555-4555-8555-555555555555", " ; ; ");
        let commands = ClaudeProvider
            .extract_commands(&path)
            .expect("read separator-only Bash entry");
        assert_eq!(commands.len(), 1);
        let (total, adopted, _) = count_rtk_commands(&commands);
        assert_eq!((total, adopted), (0, 0));

        if let Some(stdout) = run_session(home) {
            assert_row(&stdout, &["55555555", "Today", "0", "0", "0%", "....."]);
            assert!(stdout.contains("Average adoption: 0%"));
            assert!(!stdout.contains("No sessions with Bash commands found."));
        }
    });
}
