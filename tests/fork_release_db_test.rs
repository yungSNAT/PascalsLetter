//! Databases written by the fork's last release (`fork-v1.2.0-1`) still open.
//!
//! `tests/fixtures/fork-v1.2.0-1/` holds the tracking and recall databases that
//! release's `rtk` wrote for `ls`, `git status`, `grep`, `find` and two failing
//! commands (`rtk err`, `rtk test`), with the project path replaced. The tracking
//! file is stamped 2 and the recall file 0, neither tagged with an application id.

mod common;

use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::process::Output;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/fork-v1.2.0-1");
/// A stored `rtk err` failure in the fixture's recall database.
const ERR_HASH: &str = "c2dc5dd379ca56fd";

fn copy_release_dbs(dir: &Path) -> (PathBuf, PathBuf) {
    let tracking = dir.join("history.db");
    let recall = dir.join("recall.db");
    std::fs::copy(Path::new(FIXTURES).join("history.db"), &tracking).expect("copy history.db");
    std::fs::copy(Path::new(FIXTURES).join("recall.db"), &recall).expect("copy recall.db");
    (tracking, recall)
}

fn rtk(tracking: &Path, recall: &Path, args: &[&str]) -> Output {
    common::rtk_command()
        .args(args)
        .env("RTK_DB_PATH", tracking)
        .env("RTK_RECALL_DB", recall)
        .output()
        .expect("spawn rtk")
}

fn pragma(path: &Path, name: &str) -> i64 {
    Connection::open(path)
        .expect("open should succeed")
        .query_row(&format!("PRAGMA {name}"), [], |row| row.get(0))
        .expect("pragma should be readable")
}

fn count(path: &Path, table: &str) -> i64 {
    Connection::open(path)
        .expect("open should succeed")
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .expect("count should succeed")
}

#[test]
fn the_release_databases_are_as_described() {
    let dir = common::tempdir();
    let (tracking, recall) = copy_release_dbs(dir.path());
    assert_eq!(pragma(&tracking, "user_version"), 2);
    assert_eq!(pragma(&tracking, "application_id"), 0);
    assert_eq!(pragma(&recall, "user_version"), 0);
    assert_eq!(pragma(&recall, "application_id"), 0);
    assert_eq!(count(&tracking, "commands"), 6);
    assert_eq!(count(&recall, "recall"), 2);
}

#[test]
fn a_release_tracking_database_migrates_and_keeps_its_history() {
    let dir = common::tempdir();
    let (tracking, recall) = copy_release_dbs(dir.path());

    let output = rtk(&tracking, &recall, &["gain", "--format", "json"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stderr}");
    assert!(!stderr.contains("not upgraded"), "{stderr}");
    assert!(stdout.contains("\"total_commands\": 6"), "{stdout}");
    assert!(stdout.contains("\"total_saved\": 3166"), "{stdout}");

    assert!(
        pragma(&tracking, "user_version") > 2,
        "the release's stamp is migrated forward"
    );
    assert_ne!(pragma(&tracking, "application_id"), 0, "the file is tagged");
    assert_eq!(count(&tracking, "commands"), 6);
}

#[test]
fn a_release_recall_store_migrates_and_still_recalls() {
    let dir = common::tempdir();
    let (tracking, recall) = copy_release_dbs(dir.path());

    let output = rtk(&tracking, &recall, &["recall", ERR_HASH]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stderr}");
    assert!(!stderr.contains("not upgraded"), "{stderr}");
    assert!(stdout.contains("failure line 400"), "{stdout}");

    assert!(
        pragma(&recall, "user_version") > 0,
        "the release's store is stamped"
    );
    assert_ne!(pragma(&recall, "application_id"), 0, "the file is tagged");
    assert_eq!(count(&recall, "recall"), 2);
}

/// A step that cannot run (a table holds the name of an index the step creates)
/// leaves the release's stamp, tag, rows and integrity as they were. The open
/// still switches the file to WAL, as the release itself does.
#[test]
fn a_failed_migration_leaves_the_release_tracking_database_usable() {
    let dir = common::tempdir();
    let (tracking, recall) = copy_release_dbs(dir.path());
    Connection::open(&tracking)
        .expect("open should succeed")
        .execute_batch(
            "DROP INDEX idx_project_path_timestamp;
             CREATE TABLE idx_project_path_timestamp (x);",
        )
        .expect("the database should be set up");

    let output = rtk(&tracking, &recall, &["gain", "--format", "json"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stderr}");
    assert!(
        stderr.contains("tracking database was not upgraded"),
        "{stderr}"
    );
    assert!(stdout.contains("\"total_commands\": 6"), "{stdout}");

    assert_eq!(
        pragma(&tracking, "user_version"),
        2,
        "no stamp for a failed step"
    );
    assert_eq!(pragma(&tracking, "application_id"), 0, "no tag either");
    assert_eq!(count(&tracking, "commands"), 6);
    assert_eq!(integrity(&tracking), "ok");
}

/// A step that fails at its last statement takes back what its earlier
/// statements created: `idx_timestamp`, dropped here, stays absent.
#[test]
fn a_step_failing_at_its_end_rolls_back_its_earlier_statements() {
    let dir = common::tempdir();
    let (tracking, recall) = copy_release_dbs(dir.path());
    Connection::open(&tracking)
        .expect("open should succeed")
        .execute_batch(
            "DROP INDEX idx_timestamp;
             DROP INDEX idx_hook_decisions_timestamp;
             CREATE TABLE idx_hook_decisions_timestamp (x);",
        )
        .expect("the database should be set up");

    let output = rtk(&tracking, &recall, &["gain", "--format", "json"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(
        stderr.contains("tracking database was not upgraded"),
        "{stderr}"
    );

    assert_eq!(pragma(&tracking, "user_version"), 2);
    assert_eq!(pragma(&tracking, "application_id"), 0);
    assert!(!has_index(&tracking, "idx_timestamp"), "rolled back");
    assert_eq!(count(&tracking, "commands"), 6);
    assert_eq!(integrity(&tracking), "ok");
}

/// A recall step that cannot run still lets `rtk recall` serve the release's
/// rows; `rtk gain --recalls` reports the store was not upgraded.
#[test]
fn a_failed_migration_leaves_the_release_recall_store_usable() {
    let dir = common::tempdir();
    let (tracking, recall) = copy_release_dbs(dir.path());
    Connection::open(&recall)
        .expect("open should succeed")
        .execute_batch(
            "DROP INDEX idx_recall_created;
             CREATE TABLE idx_recall_created (x);",
        )
        .expect("the store should be set up");

    let output = rtk(&tracking, &recall, &["recall", ERR_HASH]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stderr}");
    assert!(stdout.contains("failure line 400"), "{stdout}");

    let output = rtk(&tracking, &recall, &["gain", "--recalls"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("recall database was not upgraded"),
        "{stderr}"
    );

    assert_eq!(pragma(&recall, "user_version"), 0);
    assert_eq!(pragma(&recall, "application_id"), 0);
    assert_eq!(count(&recall, "recall"), 2);
    assert_eq!(integrity(&recall), "ok");
}

fn integrity(path: &Path) -> String {
    Connection::open(path)
        .expect("open should succeed")
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .expect("integrity_check should run")
}

fn has_index(path: &Path, name: &str) -> bool {
    Connection::open(path)
        .expect("open should succeed")
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = ?1",
            [name],
            |row| row.get::<_, i64>(0),
        )
        .expect("sqlite_master should be readable")
        > 0
}
