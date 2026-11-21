//! Integration tests: `rtk gain`'s attribution views (`--by-method`,
//! `--by-rule`, `--by-pipeline`, `--by-layer`, `--shadow`) report the last 30
//! days of recorded savings, as JSON and as text.

use std::path::Path;

mod common;

/// Seed the tracking DB with attribution rows: one of each view's dimensions
/// today, and a 45-day-old row that every view must leave out.
fn seed(db: &Path) {
    let conn = rusqlite::Connection::open(db).expect("open tracking db");
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS engine_attribution (
            day TEXT NOT NULL,
            site TEXT NOT NULL,
            kind TEXT NOT NULL,
            name TEXT NOT NULL,
            calls INTEGER NOT NULL DEFAULT 0,
            tokens_in INTEGER NOT NULL DEFAULT 0,
            tokens_out INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (day, site, kind, name)
        );
        INSERT INTO engine_attribution VALUES
            (date('now'), 'hook', 'method', 'croft-dedupe', 3, 1000, 400),
            (date('now'), 'hook', 'method', 'croft-noop', 1, 100, 100),
            (date('now', '-45 days'), 'hook', 'method', 'croft-dedupe', 9, 9000, 0),
            (date('now'), 'hook', 'rule', 'croft-rule', 2, 500, 200),
            (date('now'), 'hook', 'pipeline', 'croft-pipe', 4, 2000, 500),
            (date('now'), 'shadow', 'rule', 'croft-shadow', 5, 800, 320),
            (date('now'), 'task', 'task', 'croft-summary', 1, 10000, 200);",
    )
    .expect("seed attribution rows");
}

fn gain(home: &Path, args: &[&str]) -> String {
    let output = common::rtk_command()
        .arg("gain")
        .args(args)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local").join("share"))
        .env("APPDATA", home.join("AppData").join("Roaming"))
        .env("LOCALAPPDATA", home.join("AppData").join("Local"))
        .env("RTK_DB_PATH", home.join("rtk.db"))
        .env("LC_ALL", "C")
        .output()
        .expect("spawn rtk gain");
    assert!(
        output.status.success(),
        "rtk gain {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn seeded_home() -> tempfile::TempDir {
    let home = common::tempdir();
    seed(&home.path().join("rtk.db"));
    home
}

#[test]
fn attribution_json_reports_each_view_over_30_days() {
    let home = seeded_home();
    let out = gain(
        home.path(),
        &[
            "--by-method",
            "--by-rule",
            "--by-pipeline",
            "--by-layer",
            "--shadow",
            "--format",
            "json",
        ],
    );
    let json: serde_json::Value = serde_json::from_str(&out).expect("gain prints JSON");
    let row = |name: &str, calls: i64, tokens_in: i64, tokens_out: i64| {
        serde_json::json!({
            "name": name, "calls": calls, "tokens_in": tokens_in,
            "tokens_out": tokens_out, "saved": tokens_in - tokens_out,
        })
    };
    assert_eq!(json["window_days"], 30);
    assert_eq!(
        json["by_method"]["hook"],
        serde_json::json!([
            row("croft-dedupe", 3, 1000, 400),
            row("croft-noop", 1, 100, 100)
        ]),
        "the 45-day-old row stays out and the larger saving comes first"
    );
    assert!(
        json["by_method"].get("proxy").is_none(),
        "no proxy section without a proxy DB: {out}"
    );
    assert_eq!(
        json["by_rule"],
        serde_json::json!([row("croft-rule", 2, 500, 200)])
    );
    assert_eq!(
        json["by_pipeline"],
        serde_json::json!([row("croft-pipe", 4, 2000, 500)])
    );
    assert_eq!(
        json["shadow"],
        serde_json::json!([row("croft-shadow", 5, 800, 320)])
    );
    assert_eq!(
        json["by_layer"],
        serde_json::json!([{"layer": "tool_result", "calls": 4, "saved": 600}]),
        "methods the engine does not know save on tool results"
    );
    assert_eq!(
        json["tasks"],
        serde_json::json!([row("croft-summary", 1, 10000, 200)])
    );
}

#[test]
fn attribution_json_includes_only_the_views_asked_for() {
    let home = seeded_home();
    let out = gain(home.path(), &["--by-rule", "--format", "json"]);
    let json: serde_json::Value = serde_json::from_str(&out).expect("gain prints JSON");
    let mut keys: Vec<_> = json
        .as_object()
        .expect("an object")
        .keys()
        .cloned()
        .collect();
    keys.sort();
    assert_eq!(keys, ["by_rule", "tasks", "window_days"]);
}

#[test]
fn attribution_text_lists_each_view_with_savings() {
    let home = seeded_home();
    let out = gain(
        home.path(),
        &[
            "--by-method",
            "--by-rule",
            "--by-pipeline",
            "--by-layer",
            "--shadow",
        ],
    );
    let line = |name: &str| {
        out.lines()
            .find(|l| l.trim_start().starts_with(name))
            .unwrap_or_else(|| panic!("no {name} line in:\n{out}"))
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };
    assert_eq!(line("croft-dedupe"), "croft-dedupe 3 calls 600 saved (60%)");
    assert_eq!(line("croft-noop"), "croft-noop 1 calls 0 saved (0%)");
    assert_eq!(line("croft-rule"), "croft-rule 2 calls 300 saved (60%)");
    assert_eq!(line("croft-pipe"), "croft-pipe 4 calls 1.5K saved (75%)");
    assert_eq!(line("croft-shadow"), "croft-shadow 5 calls 480 saved (60%)");
    assert_eq!(line("tool_result"), "tool_result 4 calls 600 saved");
    assert_eq!(
        line("croft-summary"),
        "croft-summary 1 calls 9.8K of 10.0K kept out (98%) est."
    );
    for heading in [
        "Shadow — recorded, nothing emitted (last 30d)",
        "By Layer (last 30d)",
        "Tasks — context kept out (last 30d)",
        "By Method — hook site (last 30d)",
        "By Rule — hook site (last 30d)",
        "By Pipeline — hook site (last 30d)",
    ] {
        assert!(out.contains(heading), "missing {heading:?} in:\n{out}");
    }
}

#[test]
fn attribution_text_says_when_a_view_has_no_data() {
    let home = common::tempdir();
    let out = gain(home.path(), &["--by-rule", "--by-layer"]);
    assert!(out.contains("(no data in the last 30 days)"), "{out}");
    assert!(out.contains("(no data)"), "{out}");
}
