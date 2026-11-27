mod common;

use std::path::Path;
use std::process::Output;

fn seed_recall_db(path: &Path) {
    let body: String = (1..=30)
        .map(|i| match i {
            12 => "line 12: error[E0308]: mismatched types\n".to_string(),
            20 => "line 20: thread 'worker' panicked at src/lib.rs:9:1\n".to_string(),
            _ => format!("line {i}: normal output\n"),
        })
        .collect();
    common::recall_store(
        path,
        2,
        "CREATE TABLE recall (
             hash TEXT PRIMARY KEY, command TEXT NOT NULL, cwd TEXT, exit_code INTEGER,
             created_at INTEGER NOT NULL, total_lines INTEGER NOT NULL, shown_upto INTEGER NOT NULL,
             byte_size INTEGER NOT NULL, truncated INTEGER NOT NULL, codec TEXT NOT NULL,
             blob BLOB NOT NULL, recalled INTEGER NOT NULL DEFAULT 0
         );
         CREATE TABLE tee_reads (path TEXT PRIMARY KEY);
         CREATE TABLE recall_stats (
             slug TEXT NOT NULL, mode TEXT NOT NULL, elisions INTEGER NOT NULL DEFAULT 0,
             recalls INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (slug, mode)
         );",
    );
    let conn = rusqlite::Connection::open(path).expect("open recall db");
    conn.execute(
        "INSERT INTO recall
         (hash, command, cwd, exit_code, created_at, total_lines, shown_upto, byte_size, truncated, codec, blob)
         VALUES (?1, ?2, ?3, ?4, 1, 30, 11, ?5, 0, 'raw', ?6)",
        rusqlite::params![
            "a1b2c3d4e5f60708",
            "cargo-test",
            "/tmp/proj",
            101i64,
            body.len() as i64,
            body.as_bytes()
        ],
    )
    .expect("insert recall row");
}

fn run_rtk(recall_db: &Path, tracking_db: &Path, args: &[&str]) -> Output {
    common::rtk_command()
        .env("RTK_RECALL_DB", recall_db)
        .env("RTK_DB_PATH", tracking_db)
        .args(args)
        .output()
        .expect("run rtk")
}

#[test]
fn recall_ranged_to_and_tail_and_line_numbers_return_exact_lines() {
    let dir = common::tempdir();
    let (recall_db, tracking_db) = (dir.path().join("recall.db"), dir.path().join("tracking.db"));
    seed_recall_db(&recall_db);

    let to_only = run_rtk(
        &recall_db,
        &tracking_db,
        &["recall", "a1b2c3d4", "--to", "2"],
    );
    assert!(to_only.status.success());
    assert_eq!(
        String::from_utf8(to_only.stdout).expect("utf8"),
        "line 1: normal output\nline 2: normal output\n"
    );

    let inclusive = run_rtk(
        &recall_db,
        &tracking_db,
        &["recall", "a1b2c3d4", "--from", "11", "--to", "13"],
    );
    assert!(inclusive.status.success());
    assert_eq!(
        String::from_utf8(inclusive.stdout).expect("utf8"),
        "line 11: normal output\nline 12: error[E0308]: mismatched types\nline 13: normal output\n"
    );

    let tail = run_rtk(
        &recall_db,
        &tracking_db,
        &["recall", "a1b2c3d4", "--tail", "3", "-n"],
    );
    assert!(tail.status.success());
    assert_eq!(
        String::from_utf8(tail.stdout).expect("utf8"),
        "28:line 28: normal output\n29:line 29: normal output\n30:line 30: normal output\n"
    );
}

#[test]
fn recall_rejects_inverted_to_range_cli() {
    let dir = common::tempdir();
    let (recall_db, tracking_db) = (dir.path().join("recall.db"), dir.path().join("tracking.db"));
    seed_recall_db(&recall_db);
    let inverted = run_rtk(
        &recall_db,
        &tracking_db,
        &["recall", "a1b2c3d4", "--from", "15", "--to", "10"],
    );
    assert_eq!(inverted.status.code(), Some(2));
    assert!(inverted.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&inverted.stderr)
            .contains("--to (10) must be >= starting line (15)")
    );
}

#[test]
fn recall_grep_context_and_line_numbers_show_disjoint_windows() {
    let dir = common::tempdir();
    let (recall_db, tracking_db) = (dir.path().join("recall.db"), dir.path().join("tracking.db"));
    seed_recall_db(&recall_db);

    let out = run_rtk(
        &recall_db,
        &tracking_db,
        &[
            "recall",
            "a1b2c3d4",
            "--full",
            "--grep",
            "error|panicked",
            "-C",
            "1",
            "-n",
        ],
    );
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8(out.stdout).expect("utf8"),
        concat!(
            "11-line 11: normal output\n",
            "12:line 12: error[E0308]: mismatched types\n",
            "13-line 13: normal output\n",
            "--\n",
            "19-line 19: normal output\n",
            "20:line 20: thread 'worker' panicked at src/lib.rs:9:1\n",
            "21-line 21: normal output\n",
        )
    );
}

#[test]
fn recall_digest_summarises_handle_and_matches_drill_down_range() {
    let dir = common::tempdir();
    let (recall_db, tracking_db) = (dir.path().join("recall.db"), dir.path().join("tracking.db"));
    seed_recall_db(&recall_db);

    let out = run_rtk(
        &recall_db,
        &tracking_db,
        &["recall", "a1b2c3d4", "--digest"],
    );
    assert!(out.status.success());
    let digest = String::from_utf8(out.stdout).expect("utf8");
    for expected in [
        "[recall a1b2c3d4e5f60708 | cargo-test | 30 lines | ",
        "| exit 101 | hidden 11-30]",
        " 1: line 1: normal output\n",
        " 5: line 5: normal output\n",
        "... 20 lines omitted (lines 6-25); 2 diagnostic lines:\n",
        "12: line 12: error[E0308]: mismatched types\n",
        "20: line 20: thread 'worker' panicked at src/lib.rs:9:1\n",
        "26: line 26: normal output\n",
        "30: line 30: normal output\n",
        "[drill-down: rtk recall a1b2c3d4e5f60708 --from A --to B | --tail N | --grep PAT | --full]",
    ] {
        assert!(
            digest.contains(expected),
            "missing {expected:?} in:\n{digest}"
        );
    }

    let drill = run_rtk(
        &recall_db,
        &tracking_db,
        &["recall", "a1b2c3d4", "--from", "20", "--to", "20"],
    );
    assert!(drill.status.success());
    assert_eq!(
        String::from_utf8(drill.stdout).expect("utf8"),
        "line 20: thread 'worker' panicked at src/lib.rs:9:1\n"
    );
}

#[test]
fn recall_rejects_conflicting_range_and_digest_flags() {
    let dir = common::tempdir();
    let (recall_db, tracking_db) = (dir.path().join("recall.db"), dir.path().join("tracking.db"));

    for args in [
        &["recall", "a1b2c3d4", "--to", "5", "--lines", "2"][..],
        &["recall", "a1b2c3d4", "--tail", "5", "--from", "2"][..],
        &["recall", "a1b2c3d4", "--digest", "--full"][..],
        &["recall", "a1b2c3d4", "-C", "2"][..],
    ] {
        let out = run_rtk(&recall_db, &tracking_db, args);
        assert_eq!(
            out.status.code(),
            Some(2),
            "expected clap exit 2 for {args:?}"
        );
    }
}
