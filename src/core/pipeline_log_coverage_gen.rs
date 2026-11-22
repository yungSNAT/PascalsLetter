use super::*;
use crate::core::test_isolation;
use std::fs;

fn decision(rule: &str, subject: &str) -> LogEntry {
    LogEntry {
        ts: "2026-09-15T12:04:11Z".to_owned(),
        site: "hook".to_owned(),
        event: "after_tool".to_owned(),
        subject: subject.to_owned(),
        rule: rule.to_owned(),
        ..LogEntry::default()
    }
}

fn read_subjects(limit: usize, rule: Option<&str>) -> Vec<String> {
    read_last(limit, rule)
        .into_iter()
        .map(|entry| entry.subject)
        .collect()
}

#[test]
fn opt_in_log_appends_immediately_and_flushes_queued_entries_in_order() {
    let root = test_isolation::tempdir();
    test_isolation::with_root(root.path(), || {
        let data = user_dirs::data().unwrap();
        let path = log_path().unwrap();
        let marker = data.join("pipeline-log.enabled");
        assert_eq!(path, data.join("pipeline.log"));
        assert!(!enabled());

        set_enabled(false).unwrap();
        assert!(!marker.exists());
        append(&decision("keep", "disabled append"));
        queue(decision("keep", "disabled queue"));
        flush();
        assert!(!path.exists());
        assert!(read_last(10, None).is_empty());

        set_enabled(true).unwrap();
        assert!(enabled());
        assert!(fs::read(&marker).unwrap().is_empty());

        append(&decision("keep", "immediate"));
        let initial = fs::read_to_string(&path).unwrap();
        assert_eq!(initial.lines().count(), 1);
        assert!(initial.ends_with('\n'));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&initial).unwrap(),
            serde_json::json!({
                "ts": "2026-09-15T12:04:11Z",
                "site": "hook",
                "event": "after_tool",
                "subject": "immediate",
                "rule": "keep",
                "shadow": false
            })
        );

        queue(decision("keep", "first queued"));
        queue(decision("other", "second queued"));
        assert_eq!(fs::read_to_string(&path).unwrap(), initial);

        fs::write(&path, format!("{initial}not JSON\n")).unwrap();
        flush();
        assert_eq!(
            read_subjects(10, None),
            ["immediate", "first queued", "second queued"]
        );
        assert_eq!(read_subjects(2, None), ["first queued", "second queued"]);
        assert_eq!(read_subjects(1, Some("keep")), ["first queued"]);
        assert!(read_last(0, None).is_empty());

        let saved = fs::read_to_string(&path).unwrap();
        flush();
        assert_eq!(fs::read_to_string(&path).unwrap(), saved);

        set_enabled(false).unwrap();
        assert!(!enabled());
        assert!(!marker.exists());
        append(&decision("keep", "disabled again"));
        assert_eq!(fs::read_to_string(&path).unwrap(), saved);
    });
}

#[test]
fn marker_errors_are_reported_but_log_io_fails_open() {
    let root = test_isolation::tempdir();
    test_isolation::with_root(root.path(), || {
        let marker = marker_path().unwrap();
        let path = log_path().unwrap();

        fs::create_dir_all(&marker).unwrap();
        assert!(set_enabled(true).is_err());
        assert!(set_enabled(false).is_err());
        assert!(marker.is_dir());

        fs::remove_dir(&marker).unwrap();
        set_enabled(true).unwrap();
        fs::create_dir(&path).unwrap();

        append(&decision("keep", "blocked append"));
        queue(decision("keep", "blocked queue"));
        flush();
        assert!(path.is_dir());
        assert!(read_last(10, None).is_empty());

        fs::remove_dir(&path).unwrap();
        flush();
        assert!(!path.exists());
        append(&decision("keep", "written after recovery"));
        assert_eq!(read_subjects(10, None), ["written after recovery"]);
    });
}

#[test]
fn render_prefers_an_action_and_only_shows_complete_token_counts() {
    let entry = LogEntry {
        command: Some("cargo test".to_owned()),
        tool: Some("shell tool".to_owned()),
        action: Some("keep".to_owned()),
        methods: vec!["cap".to_owned()],
        tokens_before: Some(25),
        shadow: true,
        recovery: Some("recall abc".to_owned()),
        ..decision("trim", "shell")
    };
    assert_eq!(
        render(&entry),
        format!(
            "{}  {:<28} {:<24} {:<16} keep  (shadow)  recovery recall abc",
            entry.ts, "hook.after_tool.shell", "cargo test", "trim"
        )
    );

    let bare = LogEntry {
        tool: Some("shell tool".to_owned()),
        ..decision("trim", "shell")
    };
    assert_eq!(
        render(&bare),
        format!(
            "{}  {:<28} {:<24} {:<16} -",
            bare.ts, "hook.after_tool.shell", "shell tool", "trim"
        )
    );
}
