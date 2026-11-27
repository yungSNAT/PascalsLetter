use super::{
    PRO_HINT_MARKER, Tracker, check_rtk_disabled_bypass, print_pro_hint_if_due, show_failures,
};
use crate::core::{test_isolation, tracking, user_dirs, user_env};
use rusqlite::{Connection, params};
use std::fs;

#[test]
fn bypass_warning_requires_more_than_ten_percent() {
    let root = test_isolation::tempdir();
    test_isolation::with_root(root.path(), || {
        let claude = user_dirs::home().expect("isolated home").join(".claude");
        let project = claude.join("projects").join("-fixture-project");
        fs::create_dir_all(&project).expect("Claude project directory");
        let session = project.join("be76ad1e-dad8-494f-a7d7-ecb5a4ab0711.jsonl");
        let timestamp = jiff::Timestamp::now().to_string();

        user_env::with_path("CLAUDE_CONFIG_DIR", Some(&claude), || {
            for (bypassed, expected) in [
                (1, None),
                (
                    2,
                    Some(
                        "[warn] 2 commands (20%) used RTK_DISABLED=1 unnecessarily — run `rtk discover` for details",
                    ),
                ),
            ] {
                let mut transcript = String::new();
                for i in 0..10 {
                    let command = if i < bypassed {
                        "RTK_DISABLED=1 git status"
                    } else {
                        "git status"
                    };
                    let entry = serde_json::json!({
                        "type": "assistant",
                        "timestamp": timestamp,
                        "cwd": root.path().to_string_lossy(),
                        "message": {
                            "id": format!("msg_fixture_{i}"),
                            "role": "assistant",
                            "content": [{
                                "type": "tool_use",
                                "id": format!("toolu_fixture_{i}"),
                                "name": "Bash",
                                "input": { "command": command }
                            }]
                        }
                    });
                    transcript.push_str(&entry.to_string());
                    transcript.push('\n');
                }
                fs::write(&session, transcript).expect("write synthetic session");
                assert_eq!(check_rtk_disabled_bypass().as_deref(), expected);
            }
        });
    });
}

fn seed_parse_failures(db: &Connection, timestamp: &str, split_timestamp: &str) {
    // Supply required metadata that the report itself doesn't care about.
    let mut metadata = db
        .prepare(
            "SELECT name FROM pragma_table_info('parse_failures')
             WHERE \"notnull\" = 1 AND dflt_value IS NULL AND pk = 0
               AND name NOT IN ('timestamp', 'raw_command', 'fallback_succeeded')",
        )
        .expect("inspect failure table");
    let mut columns = String::from("timestamp, raw_command, fallback_succeeded");
    let mut values = String::from("?1, ?2, ?3");
    let names = metadata
        .query_map([], |row| row.get::<_, String>(0))
        .expect("read required column names");
    for name in names {
        columns.push_str(", \"");
        columns.push_str(&name.expect("column name").replace('"', "\"\""));
        columns.push('"');
        values.push_str(", 0");
    }

    let mut insert = db
        .prepare(&format!(
            "INSERT INTO parse_failures ({columns}) VALUES ({values})"
        ))
        .expect("prepare failure fixture");
    for (time, command, recovered) in [
        (split_timestamp, "git status --short", true),
        (timestamp, "git log --oneline", false),
    ] {
        insert
            .execute(params![time, command, recovered])
            .expect("insert parse failure");
    }
}

#[cfg(unix)]
fn failure_report_stdout(db_path: &std::path::Path, check: impl FnOnce(&str)) {
    let ready = test_isolation::rtk_binary_ready();
    let true = ready else { return };
    let output = test_isolation::rtk_command()
        .args(["gain", "--failures"])
        .env("RTK_DB_PATH", db_path)
        .env(
            "XDG_DATA_HOME",
            user_dirs::data().expect("isolated data directory"),
        )
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run gain --failures");
    assert!(
        output.status.success(),
        "gain --failures failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    check(&String::from_utf8(output.stdout).expect("UTF-8 failure report"));
}

#[test]
fn failure_report_handles_empty_data_utf8_timestamps_and_query_errors() {
    let root = test_isolation::tempdir();
    test_isolation::with_root(root.path(), || {
        let tracker = Tracker::new().expect("isolated tracker");
        let db_path = tracking::get_db_path().expect("tracking database path");
        show_failures(&tracker).expect("report an empty failure history");

        #[cfg(unix)]
        failure_report_stdout(&db_path, |stdout| {
            assert!(stdout.contains("No parse failures recorded."), "{stdout}");
        });

        let db = Connection::open(&db_path).expect("open fixture database");
        let timestamp = jiff::Timestamp::now().to_string();
        let prefix = &timestamp[..15];
        // Byte 16 is inside 'é', so byte slicing without flooring would panic.
        let split_timestamp = format!("{prefix}é-tail");
        seed_parse_failures(&db, &timestamp, &split_timestamp);
        show_failures(&tracker).expect("render both failures without splitting UTF-8");

        #[cfg(unix)]
        failure_report_stdout(&db_path, |stdout| {
            assert!(stdout.contains("RTK Parse Failures"), "{stdout}");
            assert!(stdout.contains("Top Commands (by frequency)"), "{stdout}");
            assert!(stdout.contains("Recent Failures (last 10)"), "{stdout}");
            let total = stdout
                .lines()
                .find_map(|line| line.strip_prefix("Total failures:"))
                .expect("total failures KPI");
            assert_eq!(total.trim(), "2");
            let rate = stdout
                .lines()
                .find_map(|line| line.strip_prefix("Recovery rate:"))
                .expect("recovery rate KPI");
            assert_eq!(rate.trim(), "50.0%");
            assert!(
                stdout.contains(&format!("{prefix} [ok] git status --short")),
                "{stdout}"
            );
            assert!(
                stdout.contains(&format!("{} [FAIL] git log --oneline", &timestamp[..16])),
                "{stdout}"
            );
        });

        db.execute_batch("DROP TABLE parse_failures")
            .expect("remove failure table from the fixture");
        let error = show_failures(&tracker).expect_err("missing failure table must be reported");
        assert_eq!(error.to_string(), "Failed to load parse failure data");
    });
}

#[cfg(unix)]
#[test]
fn reset_without_a_terminal_declines_without_reading_input() {
    let ready = test_isolation::rtk_binary_ready();
    let true = ready else { return };
    let output = test_isolation::rtk_command()
        .args(["gain", "--reset"])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run gain --reset");
    let stderr = String::from_utf8(output.stderr).expect("UTF-8 reset prompt");
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 reset result");
    assert!(output.status.success(), "{stderr}");
    assert!(stderr.contains("Continue? [y/N]"), "{stderr}");
    assert!(
        stderr.contains("non-interactive mode, defaulting to N"),
        "{stderr}"
    );
    assert!(stdout.lines().any(|line| line == "Aborted."), "{stdout}");
    assert!(!stdout.contains("reset to zero"), "{stdout}");
}

#[test]
fn suppressed_pro_hint_does_not_create_or_overwrite_its_marker() {
    let root = test_isolation::tempdir();
    test_isolation::with_root(root.path(), || {
        user_env::with_vars(&[("RTK_PRO_HINT", Some("1"))], || {
            let tracker = Tracker::new().expect("isolated tracker");
            let marker = tracking::get_db_path()
                .expect("tracking database path")
                .with_file_name(PRO_HINT_MARKER);

            print_pro_hint_if_due(&tracker);
            assert!(!marker.try_exists().expect("check for hint marker"));

            let previous = b"existing marker";
            fs::write(&marker, previous).expect("seed old hint marker");
            print_pro_hint_if_due(&tracker);
            assert_eq!(fs::read(&marker).expect("read original marker"), previous);
        });
    });
}
