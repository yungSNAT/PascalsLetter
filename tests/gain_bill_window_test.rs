//! Integration tests: the proxy bill in `rtk gain`'s "Pipeline efficiency —
//! last 30 days" panel prices only that window and signs the change.
//! The panel is part of the engine build.
#![cfg(all(feature = "engine", any(target_os = "linux", target_os = "macos")))]

use std::path::{Path, PathBuf};

mod common;

/// Where the binary looks for the proxy's usage DB under a test home.
fn proxy_db(home: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    let config = home.join("Library").join("Application Support");
    #[cfg(not(target_os = "macos"))]
    let config = home.join(".config");
    config.join("rtk").join("llm-proxy").join("rtk-proxy.db")
}

fn gain(home: &Path) -> String {
    let output = common::rtk_command()
        .arg("gain")
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local").join("share"))
        .env("RTK_DB_PATH", home.join("rtk.db"))
        .env("LC_ALL", "C")
        .output()
        .expect("spawn rtk gain");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn proxy_bill_prices_only_the_last_30_days_and_signs_increases() {
    let home = common::tempdir();
    let db_path = proxy_db(home.path());
    std::fs::create_dir_all(db_path.parent().expect("db folder")).expect("create db folder");
    let db = rusqlite::Connection::open(&db_path).expect("open proxy db");
    db.execute_batch(
        "CREATE TABLE usage_records (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            ts TEXT NOT NULL,
            tokens_in INTEGER NOT NULL DEFAULT 0,
            tokens_out INTEGER NOT NULL DEFAULT 0,
            tokens_saved INTEGER NOT NULL DEFAULT 0,
            cost_usd REAL NOT NULL DEFAULT 0,
            cost_without REAL NOT NULL DEFAULT 0,
            routed_from TEXT,
            status INTEGER NOT NULL DEFAULT 0,
            session TEXT,
            cache_read_tokens INTEGER NOT NULL DEFAULT 0,
            cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
            ccr_retrieve_calls INTEGER NOT NULL DEFAULT 0,
            ctx_total INTEGER NOT NULL DEFAULT 0,
            ctx_tool_result INTEGER NOT NULL DEFAULT 0,
            ctx_tool_use INTEGER NOT NULL DEFAULT 0,
            ctx_duplicate INTEGER NOT NULL DEFAULT 0,
            ctx_image INTEGER NOT NULL DEFAULT 0,
            saved_ingest_filter INTEGER NOT NULL DEFAULT 0,
            saved_pixel_compress INTEGER NOT NULL DEFAULT 0,
            saved_ccr INTEGER NOT NULL DEFAULT 0,
            saved_normalize INTEGER NOT NULL DEFAULT 0
        );
        INSERT INTO usage_records (ts, tokens_in, cost_usd, cost_without, status) VALUES
            (strftime('%Y-%m-%dT%H:%M:%S+00:00', 'now'), 100, 2, 4, 200),
            (strftime('%Y-%m-%dT%H:%M:%S+00:00', 'now', '-45 days'), 700, 80, 100, 200);",
    )
    .expect("seed a recent and a 45-day-old request");

    let output = gain(home.path());
    assert!(
        output.contains("$2.00 actual vs ~$4.00 without pipeline (\u{2212}50%)"),
        "the 45-day-old request must stay out of the 30-day bill:\n{output}"
    );

    db.execute(
        "UPDATE usage_records SET cost_usd = 6 WHERE tokens_in = 100",
        [],
    )
    .expect("make the recent request cost more than without the pipeline");
    let output = gain(home.path());
    assert!(
        output.contains("$6.00 actual vs ~$4.00 without pipeline (+50%)"),
        "a cost increase must read as one:\n{output}"
    );
}

/// The hook's repeat pointers and hunked re-reads each get their own line in
/// the panel.
#[test]
fn repeats_and_hunks_have_their_own_panel_lines() {
    let home = common::tempdir();
    // The first run creates the tracking database.
    gain(home.path());
    let db = rusqlite::Connection::open(home.path().join("rtk.db")).expect("open rtk db");
    db.execute_batch(
        "INSERT INTO engine_attribution (day, site, kind, name, calls, tokens_in, tokens_out) VALUES
            (date('now'), 'hook', 'repeat', 'Bash', 1, 4000, 20),
            (date('now'), 'hook', 'hunks', 'Read', 2, 6000, 200);",
    )
    .expect("book a repeat and 2 hunked re-reads");

    let output = gain(home.path());
    let line = |label: &str| {
        output
            .lines()
            .find(|l| l.starts_with(label))
            .map(|l| l[label.len()..].trim().to_string())
    };
    assert_eq!(
        line("Repeats:").as_deref(),
        Some("1 repeated output sent as a pointer: 4.0K \u{2192} 20 tokens   est."),
        "{output}"
    );
    assert_eq!(
        line("Hunks:").as_deref(),
        Some("2 changed re-reads sent as hunks: 6.0K \u{2192} 200 tokens   est."),
        "{output}"
    );
}
