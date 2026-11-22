use super::*;

#[test]
fn ledger_upsert_replaces_a_non_table_entry_and_removes_unset_keys() {
    let mut table = toml::Table::new();
    table.insert(
        "demo".to_string(),
        toml::Value::String("corrupt scalar".to_string()),
    );
    ledger_upsert(
        &mut table,
        "demo",
        &[
            ("source", Some("github:a/b".to_string())),
            ("sha", Some("abc".to_string())),
        ],
    );
    let entry = table.get("demo").unwrap().as_table().unwrap();
    assert_eq!(entry.get("source").unwrap().as_str(), Some("github:a/b"));
    assert_eq!(entry.get("sha").unwrap().as_str(), Some("abc"));

    ledger_upsert(&mut table, "demo", &[("sha", None)]);
    let entry = table.get("demo").unwrap().as_table().unwrap();
    assert!(entry.get("sha").is_none());
    assert_eq!(entry.get("source").unwrap().as_str(), Some("github:a/b"));
}

#[test]
fn ledger_lookups_ignore_non_table_entries() {
    let dir = crate::core::test_isolation::tempdir();
    crate::core::test_isolation::with_root(dir.path(), || {
        let path = pipeline_config::installs_ledger_path()
            .expect("ledger path resolves under the isolated root");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, "demo = \"scalar\"\n").unwrap();

        assert!(published_source("demo").is_none());
        assert!(install_origin("demo").is_none());
        assert!(published_source("missing").is_none());
    });
}

#[test]
fn ledger_write_failure_is_non_fatal() {
    let dir = crate::core::test_isolation::tempdir();
    crate::core::test_isolation::with_root(dir.path(), || {
        let path = pipeline_config::installs_ledger_path()
            .expect("ledger path resolves under the isolated root");
        // A directory where the ledger file should be makes the write fail.
        std::fs::create_dir_all(&path).unwrap();

        record_publish("demo", "github:me/rtk-pipeline-demo");

        assert!(
            path.is_dir(),
            "the failed write must not replace the directory"
        );
        assert!(published_source("demo").is_none());
        assert!(install_origin("demo").is_none());
    });
}

#[test]
fn line_diff_refuses_oversized_inputs() {
    let big: String = (0..501).map(|i| format!("line {i}\n")).collect();
    let diff = line_diff(&big, "small\n");
    assert_eq!(
        diff,
        "  (too large to diff inline — review the full source yourself)\n"
    );
    let diff = line_diff("small\n", &big);
    assert!(diff.contains("too large to diff inline"));
}

#[test]
fn repo_readme_without_body_or_headline() {
    let readme = repo_readme("# only a title", "bob/rtk-pipeline-solo", None);
    assert!(readme.starts_with("# only a title\n"));
    assert!(readme.contains("Numbers on this card are self-reported"));
    assert!(readme.contains("rtk pipeline add github:bob/rtk-pipeline-solo"));
    assert!(readme.ends_with("what you install.\n"));
}

#[test]
fn select_menu_with_no_items_returns_none() {
    let choice = select_menu(&[], 0).unwrap();
    assert_eq!(choice, None);
}
