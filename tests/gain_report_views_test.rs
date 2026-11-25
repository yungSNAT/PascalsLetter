//! Integration tests: every `rtk gain` text, export and detail view over a
//! seeded home — the tracking database, the proxy's usage database, agent
//! settings and session logs, recall statistics and pipeline status — read off
//! a pipe and, on Linux, off a terminal, where the views add colour.
#![cfg(all(feature = "engine", target_os = "linux"))]

mod common;

use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// A home of the test's own: rtk keeps its tracking database under `rtk/`,
/// its configuration under `.config/rtk/` and starts in `project/`.
struct Home {
    root: tempfile::TempDir,
}

/// What a run printed, with its exit status.
struct Run {
    stdout: String,
    stderr: String,
    success: bool,
}

impl Home {
    /// A home whose tracking database exists, created by a first `rtk gain`.
    fn new() -> Self {
        let home = Self {
            root: common::tempdir(),
        };
        let first = home.gain(&["--format", "json"]);
        assert!(
            first.success,
            "the first run creates the database: {}",
            first.stderr
        );
        home
    }

    fn path(&self) -> &Path {
        self.root.path()
    }

    fn config(&self) -> PathBuf {
        self.path().join(".config").join("rtk")
    }

    fn write(&self, relative: &str, content: &str) -> PathBuf {
        let path = self.path().join(relative);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("create parent");
        std::fs::write(&path, content).expect("write fixture");
        path
    }

    fn tracking(&self) -> Connection {
        Connection::open(self.path().join("rtk").join("history.db")).expect("open tracking db")
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = common::rtk_command_in(self.path());
        cmd.arg("gain").args(args).env("LC_ALL", "C");
        cmd
    }

    fn run(mut cmd: Command) -> Run {
        let output: Output = cmd.stdin(Stdio::null()).output().expect("spawn rtk gain");
        Run {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            success: output.status.success(),
        }
    }

    fn gain(&self, args: &[&str]) -> Run {
        Self::run(self.command(args))
    }

    /// `rtk gain <args>` that must succeed; its stdout.
    fn ok(&self, args: &[&str]) -> String {
        let run = self.gain(args);
        assert!(run.success, "rtk gain {args:?}: {}", run.stderr);
        run.stdout
    }

    /// Recorded commands: `(rtk_cmd, input, output, project_path)`, all now.
    fn commands(&self, rows: &[(&str, i64, i64, &str)]) {
        let conn = self.tracking();
        for (cmd, input, output, project) in rows {
            let saved = input - output;
            let pct = if *input > 0 {
                saved as f64 / *input as f64 * 100.0
            } else {
                0.0
            };
            conn.execute(
                "INSERT INTO commands (timestamp, original_cmd, rtk_cmd, project_path,
                     input_tokens, output_tokens, saved_tokens, savings_pct, exec_time_ms)
                 VALUES (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), ?1, ?1, ?2, ?3, ?4, ?5, ?6, 12)",
                rusqlite::params![cmd, project, input, output, saved, pct],
            )
            .expect("record a command");
        }
    }

    /// Attribution rows for today: `(site, kind, name, calls, in, out)`.
    fn attribution(&self, rows: &[(&str, &str, &str, i64, i64, i64)]) {
        let conn = self.tracking();
        for (site, kind, name, calls, tokens_in, tokens_out) in rows {
            conn.execute(
                "INSERT INTO engine_attribution (day, site, kind, name, calls, tokens_in, tokens_out)
                 VALUES (date('now'), ?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![site, kind, name, calls, tokens_in, tokens_out],
            )
            .expect("record attribution");
        }
    }

    /// The proxy's usage database with the current schema, filled by `rows`.
    fn proxy(&self, rows: &str) {
        let dir = self.config().join("llm-proxy");
        std::fs::create_dir_all(&dir).expect("create proxy dir");
        Connection::open(dir.join("rtk-proxy.db"))
            .expect("open proxy db")
            .execute_batch(&format!(
                "CREATE TABLE usage_records (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    ts TEXT NOT NULL,
                    tokens_in INTEGER NOT NULL DEFAULT 0,
                    tokens_out INTEGER NOT NULL DEFAULT 0,
                    tokens_saved INTEGER NOT NULL DEFAULT 0,
                    cost_usd REAL NOT NULL DEFAULT 0,
                    status INTEGER NOT NULL DEFAULT 200,
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
                    saved_normalize INTEGER NOT NULL DEFAULT 0,
                    session TEXT,
                    pipeline TEXT,
                    routed_from TEXT,
                    cost_without REAL NOT NULL DEFAULT 0,
                    saved_engine_methods INTEGER NOT NULL DEFAULT 0,
                    saved_rules TEXT
                );
                {rows}"
            ))
            .expect("seed proxy usage");
    }

    /// A Claude Code session log under `~/.claude/projects/<project>/`.
    fn session(&self, project: &str, name: &str, tool_uses: &[(&str, &str)]) {
        let lines: Vec<String> = tool_uses
            .iter()
            .enumerate()
            .map(|(i, (tool, command))| {
                serde_json::json!({
                    "type": "assistant",
                    "message": {"role": "assistant", "content": [{
                        "type": "tool_use", "id": format!("toolu_{i}"), "name": tool,
                        "input": {"command": command, "file_path": command},
                    }]},
                })
                .to_string()
            })
            .collect();
        self.write(
            &format!(".claude/projects/{project}/{name}.jsonl"),
            &(lines.join("\n") + "\n"),
        );
    }
}

/// Seconds-precise UTC timestamp `offset` seconds from now, as the proxy writes.
fn ts(offset: i64) -> String {
    format!("strftime('%Y-%m-%dT%H:%M:%S+00:00', 'now', '{offset} seconds')")
}

/// The home most views read: commands in every savings band and one past the
/// agent cap, hook attribution of every kind, a proxy database with each
/// lever, a hook that misses an event, a session that bypasses rtk, an
/// untrusted filter, legacy tee logs, a directive and a pipeline that fell back.
fn rich_home() -> Home {
    let home = Home::new();
    home.commands(&[
        ("rtk git log", 1000, 100, ""),
        ("rtk cargo test", 1000, 500, ""),
        ("rtk ls", 1000, 900, ""),
        ("rtk grep", 9000, 100, ""),
    ]);
    let slice = |subject: &str| format!("croft-pipe\u{1f}{subject}");
    let (shell, read, web) = (slice("shell"), slice("read"), slice("web"));
    home.attribution(&[
        ("hook", "method", "croft-dedupe", 3, 1000, 400),
        ("hook", "method", "strip-line-numbers", 2, 300, 100),
        ("hook", "rule", "croft-rule", 2, 500, 200),
        ("hook", "rule", "croft-zero", 1, 0, 0),
        ("hook", "pipeline", "croft-pipe", 4, 2000, 500),
        ("hook", "pipeline-slice", &shell, 2, 800, 300),
        ("hook", "pipeline-slice", &read, 1, 400, 200),
        ("hook", "pipeline-slice", &web, 1, 1000, 900),
        ("hook", "llm-spend", "classifier", 1, 0, 250),
        ("hook", "recovery", "croft-recall", 3, 0, 0),
        ("shadow", "pipeline", "croft-shadow", 5, 1000, 400),
        ("task", "task", "croft-summary", 1, 10000, 200),
        ("task", "task", "croft-empty", 1, 0, 0),
    ]);
    let rules = r#"{"rule-a":40,"rule-b":30,"rule-c":10,"rule-d":8,"rule-e":7,"rule-f":5}"#;
    home.proxy(&format!(
        "INSERT INTO usage_records (ts, session, pipeline, tokens_in, tokens_saved, saved_ccr,
             saved_normalize, saved_engine_methods, saved_rules, cache_read_tokens,
             cache_creation_tokens, ccr_retrieve_calls, cost_usd, cost_without)
         VALUES ({}, 's1', 'croft-pipe', 1000, 600, 300, 200, 100, '{rules}', 5000, 1000, 2, 1.0, 3.0);
         INSERT INTO usage_records (ts, pipeline) VALUES ({}, 'idle-pipe');
         INSERT INTO usage_records (ts, session, cache_creation_tokens) VALUES ({}, 's1', 5000);
         INSERT INTO usage_records (ts, status, cost_usd, cost_without) VALUES ({}, 500, 9.0, 9.0);
         INSERT INTO usage_records (ts, tokens_in, routed_from, cost_usd, cost_without)
         VALUES ({}, 400, 'big-model', 0.5, 2.0);",
        ts(-120),
        ts(-100),
        ts(-60),
        ts(-30),
        ts(-10),
    ));
    home.write(
        ".claude/settings.json",
        r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"rtk hook claude"}]}]}}"#,
    );
    home.session(
        "croft",
        "bypass",
        &[("Bash", "RTK_DISABLED=1 git status"), ("Bash", "git diff")],
    );
    std::os::unix::fs::symlink(
        home.path().join("missing"),
        home.path().join(".claude/projects/croft/dangling.jsonl"),
    )
    .expect("a session link to nothing");
    home.write(
        "project/.rtk/filters.toml",
        r#"schema_version = 1

[filters.croft-probe]
description = "croft probe"
match_command = "^croft$"
match_output = [{ pattern = "x", message = "y" }]
"#,
    );
    home.write("rtk/tee/1700000000_git_log.log", "legacy tee output\n");
    // Repeat dedup needs PostToolUse, which the settings above do not hook.
    home.write(".config/rtk/config.toml", "[hooks]\ndedup_repeats = true\n");
    home.write(
        ".config/rtk/pipeline.toml",
        "[pipeline]\nname = \"croft\"\nextends = \"rtk:default\"\n\n[[directive]]\nname = \"tone\"\ntext = \"answer briefly and plainly in every single reply\"\n",
    );
    home.write(
        "rtk/pipeline-status.json",
        r#"{"fell_back": true, "diagnostics": ["bad rule"]}"#,
    );
    home
}

/// A home with one command that saved nothing, a proxy that only routed, a
/// shadow rule that measured nothing and a pipeline loaded with warnings.
fn quiet_home() -> Home {
    let home = Home::new();
    home.commands(&[("rtk cat", 400, 400, "")]);
    home.attribution(&[
        ("hook", "pipeline", "quiet-pipe", 1, 100, 50),
        (
            "hook",
            "pipeline-slice",
            "quiet-pipe\u{1f}shell",
            1,
            100,
            50,
        ),
        ("hook", "pipeline-slice", "orphan\u{1f}read", 1, 10, 5),
        ("shadow", "pipeline", "quiet-shadow", 2, 0, 0),
    ]);
    home.proxy(&format!(
        "INSERT INTO usage_records (ts, pipeline, tokens_in, routed_from)
         VALUES ({}, 'quiet-proxy', 100, 'big-model');",
        ts(-5)
    ));
    home.session("quiet", "plain", &[("Bash", "ls")]);
    home.write(
        "rtk/pipeline-status.json",
        r#"{"fell_back": false, "diagnostics": ["one", "two"]}"#,
    );
    home
}

#[test]
fn summary_reports_the_proxy_levers_the_ledger_and_every_warning() {
    let home = rich_home();
    let run = home.gain(&["--history", "--graph", "--quota", "--tier", "pro"]);
    assert!(run.success, "{}", run.stderr);
    let out = &run.stdout;
    for expected in [
        "RTK Token Savings (Global Scope)",
        "Capped:            1 outputs past the ~30k-char agent cap, counted at the cap",
        "Hook needed:       PostToolUse hook missing",
        "Requests:          5",
        "Failed:            1 (excluded from cost)",
        "removed across the 1 req it touched (per-mechanism ratios below)",
        "1 req · 400 in ran on the task model, not the session model · $0.5000 spent · ~$1.50 (-75%) vs the session model est.",
        "  via ccr:         300",
        "  via normalize:   200",
        "  via rules:       100",
        "    rule-a:        40",
        "    rule-e:        7",
        "    …:             1 more · rtk gain --by-rule",
        "Recovery calls:    2",
        "Cache hit:         45.5%",
        "Cache effect:      input billed at ~76% of raw (-24%) est.",
        "  fractures:       1 recent",
        "Filter+compress:   2.0K → 500 tokens (75% removed)   est.",
        "LLM-spend:         −250 tokens (classifier/digest calls)   measured",
        "Recalled:          3 compressed outputs pulled back in full",
        "Directives:        +12 tokens standing, every turn   est.",
        "Bill (proxy):      $1.50 actual vs ~$5.00 without pipeline (−70%)",
        "Routed:            1 requests (400 tokens in) to a cheaper model — ~$1.50 saved est.",
        "Shadow rules:      would remove 60% across 5 outputs (changed nothing)",
        "Daily Savings (last 30 days)",
        "Recent Commands",
        "Monthly Quota Analysis",
        "Subscription tier: Pro ($20/mo)",
    ] {
        assert!(out.contains(expected), "missing {expected:?} in:\n{out}");
    }
    assert!(
        !out.contains("rule-f"),
        "only five rules fit the panel:\n{out}"
    );
    let history: Vec<&str> = out.lines().filter(|line| line.contains(" rtk ")).collect();
    for (command, sign) in [
        ("rtk git log", "▲"),
        ("rtk cargo test", "■"),
        ("rtk ls", "•"),
    ] {
        assert!(
            history
                .iter()
                .any(|line| line.contains(&format!("{sign} {command}"))),
            "{command} is not marked {sign} in:\n{out}"
        );
    }
    let graph_row = out
        .lines()
        .find(|line| line.contains('█') && line.trim_end().ends_with("8.9K"))
        .unwrap_or_else(|| panic!("a graph row for today's 8.9K saved:\n{out}"));
    assert_eq!(graph_row.matches('█').count(), 40, "{graph_row}");

    let err = &run.stderr;
    for expected in [
        "[rtk] Recovery hints moved to a sqlite store",
        "[warn] 1 commands (50%) used RTK_DISABLED=1 unnecessarily",
        "[rtk] 1 untrusted custom filter(s) not applied — run `rtk trust`",
        "[rtk] pipeline.toml: 1 issue(s), using the built-in default",
    ] {
        assert!(err.contains(expected), "missing {expected:?} in:\n{err}");
    }
}

#[test]
fn quiet_summary_reports_routing_without_prices_and_pipeline_warnings() {
    let home = quiet_home();
    let run = home.gain(&["--graph"]);
    assert!(run.success, "{}", run.stderr);
    let out = &run.stdout;
    for expected in [
        "Trimmed in flight: 0 (transforms only)",
        "Routed:            1 req · 100 in ran on the task model, not the session model (cheaper model, not fewer tokens)",
        "Shadow rules:      would remove 0% across 2 outputs (changed nothing)",
        "~ no proxy cost data — size estimates only, not your bill",
    ] {
        assert!(out.contains(expected), "missing {expected:?} in:\n{out}");
    }
    assert!(!out.contains("Cache hit"), "no cache data:\n{out}");
    let graph_row = out
        .lines()
        .find(|line| line.contains(" │"))
        .unwrap_or_else(|| panic!("a graph row for a day that saved nothing:\n{out}"));
    assert!(
        graph_row.ends_with(&format!(" │{} 0", " ".repeat(40))),
        "nothing saved, no bar: {graph_row:?}"
    );
    assert!(
        run.stderr
            .contains("[rtk] pipeline.toml: 2 issue(s), loaded with warnings"),
        "{}",
        run.stderr
    );
    assert!(
        !run.stderr.contains("RTK_DISABLED"),
        "one plain command is no bypass: {}",
        run.stderr
    );
}

#[test]
fn quota_names_each_subscription_tier() {
    let home = quiet_home();
    for (tier, name, quota) in [
        ("pro", "Pro ($20/mo)", "6.0M"),
        ("5x", "Max 5x ($100/mo)", "30.0M"),
        ("20x", "Max 20x ($200/mo)", "120.0M"),
        ("team", "Pro ($20/mo)", "6.0M"),
    ] {
        let out = home.ok(&["--quota", "--tier", tier]);
        assert!(
            out.contains(&format!("Subscription tier: {name}")),
            "{tier}:\n{out}"
        );
        assert!(
            out.contains(&format!("Estimated monthly quota: {quota}")),
            "{tier}:\n{out}"
        );
        assert!(out.contains("Quota preserved:   0.0%"), "{tier}:\n{out}");
    }
}

#[test]
fn exports_carry_every_period_as_json_and_csv() {
    let home = rich_home();
    let json: serde_json::Value =
        serde_json::from_str(&home.ok(&["--format", "json", "--all"])).expect("JSON export");
    for period in ["daily", "weekly", "monthly"] {
        let rows = json[period].as_array().expect("a period array");
        assert_eq!(rows.len(), 1, "{period}: {json}");
        assert_eq!(rows[0]["commands"], 4, "{period}: {json}");
    }
    assert_eq!(json["summary"]["oversized_capped"], 1);

    let csv = home.ok(&["--format", "csv", "--all"]);
    for (title, header) in [
        (
            "# Daily Data",
            "date,commands,input_tokens,output_tokens,saved_tokens,savings_pct,total_time_ms,avg_time_ms",
        ),
        (
            "# Weekly Data",
            "week_start,week_end,commands,input_tokens,output_tokens,saved_tokens,savings_pct,total_time_ms,avg_time_ms",
        ),
        (
            "# Monthly Data",
            "month,commands,input_tokens,output_tokens,saved_tokens,savings_pct,total_time_ms,avg_time_ms",
        ),
    ] {
        let mut lines = csv.lines().skip_while(|line| *line != title).skip(1);
        assert_eq!(lines.next(), Some(header), "{csv}");
        let row = lines.next().expect("one data row");
        let commands_column = if title == "# Weekly Data" { 2 } else { 1 };
        assert_eq!(
            row.split(',').nth(commands_column),
            Some("4"),
            "{title}: {row}"
        );
    }
}

#[test]
fn attribution_views_add_the_proxy_site_and_recovery_reads() {
    let home = rich_home();
    let out = home.ok(&[
        "--by-method",
        "--by-rule",
        "--by-pipeline",
        "--by-layer",
        "--shadow",
    ]);
    for expected in [
        "  croft-zero                        1 calls           0 saved  (0%)",
        "  croft-empty                       1 calls           0 of         0 kept out (0%) est.",
        "By Method — proxy site (all time)",
        "  ccr                                 300 saved",
        "  normalize                           200 saved",
        "  rules                               100 saved",
        "  ccr (rtk_retrieve)                    2 recovery calls",
        "By Rule — proxy site (all time)",
        "  rule-a                               40 saved",
        "  rule-f                                5 saved",
        "By Pipeline — proxy site (all time)",
        "  croft-pipe                        1 req           600 saved  (38%)",
        "  (unknown)                         3 req             0 saved  (0%)",
        "Recovery reads — hook site (last 30d)",
        "  croft-recall                      3 recall reads",
        "  history             0 calls         500 saved",
        "  idle-pipe                         1 req             0 saved  (0%)",
    ] {
        assert!(out.contains(expected), "missing {expected:?} in:\n{out}");
    }
    let read = format!("  {:<14} {:>6} calls  {:>10} saved", "read", 2, 200);
    assert!(
        out.contains(&read),
        "a method the engine knows lands on its layer:\n{out}"
    );

    let json: serde_json::Value =
        serde_json::from_str(&home.ok(&["--by-method", "--by-rule", "--format", "json"]))
            .expect("JSON attribution");
    assert_eq!(
        json["by_method"]["proxy"],
        serde_json::json!([
            {"name": "ccr", "saved": 300},
            {"name": "normalize", "saved": 200},
            {"name": "rules", "saved": 100},
        ])
    );
    assert_eq!(json["by_method"]["retrieve_calls"], 2);
    assert_eq!(
        json["by_rule_proxy"][0],
        serde_json::json!({"name": "rule-a", "saved": 40})
    );
    assert_eq!(json["by_rule_proxy"].as_array().map(Vec::len), Some(6));

    let json: serde_json::Value =
        serde_json::from_str(&home.ok(&["--by-layer", "--format", "json"])).expect("JSON layers");
    assert_eq!(
        json["by_layer"],
        serde_json::json!([
            {"layer": "read", "calls": 2, "saved": 200},
            {"layer": "tool_result", "calls": 3, "saved": 600},
        ]),
        "a method the engine knows lands on its own layer"
    );
}

#[test]
fn pipeline_detail_slices_hooks_tasks_and_the_proxy() {
    let home = rich_home();
    let out = home.ok(&["--pipeline", "croft-pipe"]);
    for expected in [
        "Pipeline: croft-pipe",
        "Agent hooks — tool output trimmed before it re-enters context (last 30d)",
        "  shell output (bash)     2 calls         500 saved  ██████░░░░  62%",
        "  file reads              1 calls         200 saved  █████░░░░░  50%",
        "  web content             1 calls         100 saved  █░░░░░░░░░  10%",
        "  total                   4 calls        1.5K saved  ████████░░  75%",
        "(rows recorded before per-slice tracking count only in the total)",
        "Delegation — context a worker read for you, kept out of your window",
        "  croft-summary           1 reads        9.8K kept out of context",
        "LLM proxy — billed input tokens compressed before forwarding (all time)",
        "  input tokens              1 req         600 saved  ████░░░░░░  38%",
        "    ccr                           300 saved",
        "    rules                         100 saved",
        "  prompt cache:    5.0K read · 1.0K written  (hit 83%)",
        "  cache effect:    input billed at ~39% of raw (-61%) est.",
        "Output tokens are not compressed by pipelines",
    ] {
        assert!(out.contains(expected), "missing {expected:?} in:\n{out}");
    }
}

#[test]
fn pipeline_detail_without_data_lists_the_pipelines_that_have_some() {
    let home = quiet_home();
    let out = home.ok(&["--pipeline", "nope"]);
    assert!(
        out.contains("no recorded savings for pipeline `nope`"),
        "{out}"
    );
    assert!(
        out.contains("pipelines with data: quiet-pipe, quiet-proxy"),
        "{out}"
    );
}

#[test]
fn project_scope_shortens_a_deep_working_directory() {
    let home = Home::new();
    let deep = home.path().join("project").join("a").join("b").join("c");
    std::fs::create_dir_all(&deep).expect("create a deep project");
    let canonical = deep.canonicalize().expect("canonical project");
    home.commands(&[("rtk git status", 1000, 500, canonical.to_str().unwrap())]);
    home.commands(&[("rtk git log", 1000, 0, "/elsewhere")]);
    home.session("deep", "reads", &[("Read", "/a/file")]);
    let mut cmd = home.command(&["--project"]);
    cmd.current_dir(&deep);
    let run = Home::run(cmd);
    assert!(run.success, "{}", run.stderr);
    assert!(
        run.stdout.contains("RTK Token Savings (Project Scope)"),
        "{}",
        run.stdout
    );
    assert!(run.stdout.contains("Scope: /.../b/c"), "{}", run.stdout);
    assert!(
        run.stdout.contains("Total commands:    1"),
        "{}",
        run.stdout
    );
    assert!(!run.stderr.contains("RTK_DISABLED"), "{}", run.stderr);
}

#[test]
fn failures_list_the_top_and_recent_parse_failures() {
    let home = Home::new();
    home.tracking()
        .execute_batch(
            "INSERT INTO parse_failures (timestamp, raw_command, error_message, fallback_succeeded) VALUES
                ('2026-11-20T10:11:12.000Z', 'git frobnicate', 'unknown', 1),
                ('2026-11-21T10:11:12.000Z', 'git frobnicate', 'unknown', 1),
                ('2026-11-22T10:11:12.000Z', 'cargo weird', 'unknown', 0);",
        )
        .expect("record parse failures");
    let out = home.ok(&["--failures"]);
    for expected in [
        "RTK Parse Failures",
        "Total failures:    3",
        "Recovery rate:     66.7%",
        "Top Commands (by frequency)",
        "     2x  git frobnicate",
        "Recent Failures (last 10)",
        "  2026-11-22T10:11 [FAIL] cargo weird",
        "  2026-11-21T10:11 [ok] git frobnicate",
    ] {
        assert!(out.contains(expected), "missing {expected:?} in:\n{out}");
    }
}

/// A recall store with statistics, through the first run that migrates it.
fn recall_home(mode: &str) -> Home {
    let home = Home::new();
    common::recall_store(&home.path().join("rtk").join("recall.db"), 0, "");
    let first = home.gain(&["--recalls"]);
    assert!(first.success, "{}", first.stderr);
    assert!(
        first.stdout.contains("No recall activity recorded yet."),
        "{}",
        first.stdout
    );
    Connection::open(home.path().join("rtk").join("recall.db"))
        .expect("open recall store")
        .execute_batch(
            "INSERT INTO recall_stats (slug, mode, elisions, recalls) VALUES
                ('alpha', 'sqlite', 10, 5),
                ('beta', 'sqlite', 10, 2),
                ('gamma', 'sqlite', 10, 0),
                ('delta', 'tee', 0, 0);",
        )
        .expect("record recall stats");
    home.write(
        ".config/rtk/config.toml",
        &format!("[retriever]\nmode = \"{mode}\"\n"),
    );
    home
}

#[test]
fn recall_stats_name_the_mode_and_judge_each_filter() {
    for mode in ["tee", "disabled"] {
        let home = recall_home(mode);
        let out = home.ok(&["--recalls"]);
        assert!(out.contains(&format!("Mode: {mode}")), "{out}");
        for expected in [
            "alpha         10         5     50%  too aggressive",
            "beta          10         2     20%  watch",
            "gamma         10         0      0%  ok",
            "delta          0         0       -  -",
        ] {
            assert!(out.contains(expected), "missing {expected:?} in:\n{out}");
        }
    }
}

#[test]
fn reset_off_a_terminal_without_yes_aborts_and_keeps_the_history() {
    let home = quiet_home();
    let run = home.gain(&["--reset"]);
    assert!(run.success, "{}", run.stderr);
    assert!(run.stdout.contains("Aborted."), "{}", run.stdout);
    assert!(
        run.stderr
            .contains("(non-interactive mode, defaulting to N)"),
        "{}",
        run.stderr
    );
    let kept: i64 = home
        .tracking()
        .query_row("SELECT COUNT(*) FROM commands", [], |r| r.get(0))
        .expect("count commands");
    assert_eq!(kept, 1);
}

#[test]
fn watch_off_a_terminal_falls_back_to_the_text_view() {
    let home = quiet_home();
    let run = home.gain(&["--watch"]);
    assert!(run.success, "{}", run.stderr);
    assert!(
        run.stderr
            .contains("rtk: --watch needs a terminal; showing the text view instead"),
        "{}",
        run.stderr
    );
    assert!(run.stdout.contains("RTK Token Savings"), "{}", run.stdout);
}

/// A loopback server that answers `/health` as the rtk proxy does, for as long
/// as the test runs.
fn fake_proxy() -> String {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let addr = listener.local_addr().expect("address").to_string();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            let body = r#"{"service":"rtk-proxy"}"#;
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    addr
}

#[test]
fn an_empty_history_says_what_a_running_proxy_still_needs() {
    let home = Home::new();
    let mut cmd = home.command(&[]);
    cmd.env("RTK_PROXY_BIND", fake_proxy());
    let run = Home::run(cmd);
    assert!(run.success, "{}", run.stderr);
    assert!(
        run.stdout.contains("No tracking data yet."),
        "{}",
        run.stdout
    );
    let hint = run
        .stdout
        .lines()
        .find(|line| line.starts_with("LLM proxy: "))
        .unwrap_or_else(|| panic!("a proxy hint:\n{}", run.stdout));
    assert!(
        !hint.contains("llm-proxy start"),
        "the proxy is running, so it needs no start: {hint}"
    );
}

/// Runs a command with its standard input and output on a pseudo-terminal,
/// typing `input` first, so the views take their terminal paths.
mod tty {
    use std::fs::File;
    use std::io::{Read, Write};
    use std::os::fd::FromRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    use std::process::{Command, Stdio};

    pub struct Run {
        pub screen: String,
        pub stderr: String,
        pub success: bool,
    }

    #[allow(unsafe_code)]
    fn open() -> (File, File) {
        // SAFETY: plain libc calls on a descriptor this function owns; the
        // name buffer is large enough and NUL-terminated by `ptsname_r`.
        // nosemgrep: unsafe-block
        unsafe {
            let fd = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
            assert!(fd >= 0, "posix_openpt");
            let master = File::from_raw_fd(fd);
            assert_eq!(libc::grantpt(fd), 0, "grantpt");
            assert_eq!(libc::unlockpt(fd), 0, "unlockpt");
            let size = libc::winsize {
                ws_row: 40,
                ws_col: 160,
                ws_xpixel: 0,
                ws_ypixel: 0,
            };
            assert_eq!(libc::ioctl(fd, libc::TIOCSWINSZ, &size), 0, "set size");
            let mut name = [0 as libc::c_char; 128];
            assert_eq!(libc::ptsname_r(fd, name.as_mut_ptr(), name.len()), 0);
            let path = std::ffi::CStr::from_ptr(name.as_ptr())
                .to_str()
                .expect("a UTF-8 terminal name")
                .to_owned();
            let slave = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc::O_NOCTTY)
                .open(path)
                .expect("open the terminal");
            (master, slave)
        }
    }

    pub fn run(cmd: Command, input: &[u8]) -> Run {
        run_answering(cmd, input, None)
    }

    /// [`run`], typing `answer.1` once the screen shows `answer.0`.
    pub fn run_answering(mut cmd: Command, input: &[u8], answer: Option<(&str, &[u8])>) -> Run {
        let (mut master, slave) = open();
        cmd.stdin(Stdio::from(slave.try_clone().expect("clone terminal")))
            .stdout(Stdio::from(slave))
            .stderr(Stdio::piped())
            .env("CLICOLOR_FORCE", "1");
        let mut child = cmd.spawn().expect("spawn on a terminal");
        // The command holds this process's copies of the terminal; with them
        // closed, reading the master ends once the child exits.
        drop(cmd);
        master.write_all(input).expect("type the input");
        let mut answer = answer.map(|(seen, keys)| (seen.as_bytes().to_vec(), keys.to_vec()));
        let mut typist = master.try_clone().expect("clone the terminal");
        let reader = std::thread::spawn(move || {
            let mut screen = Vec::new();
            let mut buf = [0u8; 8192];
            while let Ok(n) = master.read(&mut buf) {
                if n == 0 {
                    break;
                }
                screen.extend_from_slice(&buf[..n]);
                if let Some((seen, keys)) = &answer
                    && screen.windows(seen.len()).any(|w| w == seen.as_slice())
                {
                    typist.write_all(keys).expect("type the answer");
                    answer = None;
                }
            }
            screen
        });
        let mut stderr = String::new();
        child
            .stderr
            .take()
            .expect("stderr")
            .read_to_string(&mut stderr)
            .expect("read stderr");
        let status = child.wait().expect("wait for rtk");
        let screen = reader.join().expect("terminal reader");
        Run {
            screen: String::from_utf8_lossy(&screen).replace("\r\n", "\n"),
            stderr,
            success: status.success(),
        }
    }
}

/// `text` as `colored` paints it with the SGR `codes`.
fn painted(codes: &str, text: &str) -> String {
    format!("\x1b[{codes}m{text}\x1b[0m")
}

#[test]
fn a_terminal_summary_colours_each_band_and_offers_sharing() {
    let home = rich_home();
    // A hint shown a moment ago is not due again; this fork never shows one.
    home.write("rtk/pro-hint-shown", "");
    let run = tty::run(home.command(&[]), b"");
    assert!(run.success, "{}", run.stderr);
    let screen = &run.screen;
    assert!(!screen.contains("pro.rtk-ai.app"), "{screen}");
    for expected in [
        painted("1;32", "RTK Token Savings (Global Scope)"),
        painted("1;96", "rtk git log             "),
        painted("1;32", " 90.0%"),
        painted("1;33", " 50.0%"),
        painted("1;31", " 10.0%"),
        painted("1;32", "84.8%"),
        painted("32", "▶ share these numbers:"),
    ] {
        assert!(
            screen.contains(&expected),
            "missing {expected:?} in:\n{screen}"
        );
    }
    assert!(screen.contains("\x1b[36m"), "a cyan impact bar:\n{screen}");

    let quiet = tty::run(quiet_home().command(&[]), b"");
    assert!(
        quiet.screen.contains(&painted("1;31", "0.0%")),
        "{}",
        quiet.screen
    );

    let middle = Home::new();
    middle.commands(&[("rtk git diff", 1000, 500, "")]);
    let run = tty::run(middle.command(&[]), b"");
    assert!(
        run.screen.contains(&painted("1;33", "50.0%")),
        "{}",
        run.screen
    );
}

#[test]
fn a_terminal_pipeline_detail_colours_its_meters_by_band() {
    let home = rich_home();
    let run = tty::run(home.command(&["--pipeline", "croft-pipe"]), b"");
    assert!(run.success, "{}", run.stderr);
    for expected in [
        painted("32", "██████░░░░"),
        painted("33", "█████░░░░░"),
        painted("2", "█░░░░░░░░░"),
    ] {
        assert!(
            run.screen.contains(&expected),
            "missing {expected:?} in:\n{}",
            run.screen
        );
    }
}

#[test]
fn a_terminal_recall_table_colours_rates_by_band() {
    let home = recall_home("sqlite");
    let run = tty::run(home.command(&["--recalls"]), b"");
    assert!(run.success, "{}", run.stderr);
    for expected in [
        painted("1;31", "too aggressive"),
        painted("1;33", "watch"),
        painted("1;32", "ok"),
    ] {
        assert!(
            run.screen.contains(&expected),
            "missing {expected:?} in:\n{}",
            run.screen
        );
    }
}

#[test]
fn a_terminal_reset_asks_and_resets_on_yes() {
    let home = quiet_home();
    let run = tty::run(home.command(&["--reset"]), b"y\n");
    assert!(run.success, "{}", run.stderr);
    assert!(
        run.stderr.contains("Continue? [y/N]"),
        "the question goes to stderr: {}",
        run.stderr
    );
    assert!(
        run.screen
            .contains("Token savings and recall stats reset to zero."),
        "{}",
        run.screen
    );
    let left: i64 = home
        .tracking()
        .query_row("SELECT COUNT(*) FROM commands", [], |r| r.get(0))
        .expect("count commands");
    assert_eq!(left, 0);
}

#[test]
fn a_terminal_watch_opens_the_dashboard_and_quits_on_q() {
    let home = rich_home();
    // The dashboard takes the alternate screen once it starts drawing.
    let run = tty::run_answering(home.command(&["--watch"]), b"", Some(("\x1b[?1049h", b"q")));
    assert!(run.success, "{}", run.stderr);
    assert!(run.screen.contains("\x1b[?1049h"), "{}", run.screen);
    assert!(
        !run.stderr.contains("needs a terminal"),
        "a terminal gets the dashboard: {}",
        run.stderr
    );
}

#[test]
fn pipeline_detail_shows_only_the_sections_with_data() {
    let home = quiet_home();
    let whole = home.ok(&["--pipeline", "quiet-pipe"]);
    assert!(whole.contains("  shell output (bash)"), "{whole}");
    assert!(whole.contains("  total "), "{whole}");
    for absent in [
        "(rows recorded before per-slice tracking",
        "Delegation",
        "LLM proxy",
    ] {
        assert!(!whole.contains(absent), "{absent:?} in:\n{whole}");
    }

    let orphan = home.ok(&["--pipeline", "orphan"]);
    assert!(orphan.contains("  file reads"), "{orphan}");
    assert!(
        !orphan.contains("  total "),
        "no hook total recorded:\n{orphan}"
    );

    let proxy_only = home.ok(&["--pipeline", "quiet-proxy"]);
    assert!(proxy_only.contains("  input tokens"), "{proxy_only}");
    for absent in ["Agent hooks", "prompt cache", "cache effect"] {
        assert!(!proxy_only.contains(absent), "{absent:?} in:\n{proxy_only}");
    }
}

#[test]
fn attribution_without_tasks_or_proxy_methods_prints_only_what_exists() {
    let home = quiet_home();
    let json: serde_json::Value =
        serde_json::from_str(&home.ok(&["--by-method", "--format", "json"]))
            .expect("JSON attribution");
    let mut keys: Vec<_> = json
        .as_object()
        .expect("an object")
        .keys()
        .cloned()
        .collect();
    keys.sort();
    assert_eq!(keys, ["by_method", "window_days"], "{json}");
    assert_eq!(json["by_method"]["proxy"], serde_json::json!([]));

    let out = home.ok(&["--by-method"]);
    assert!(out.contains("By Method — hook site"), "{out}");
    assert!(
        !out.contains("proxy site"),
        "a proxy that saved nothing by method prints no section:\n{out}"
    );
}

/// No commands of its own, a proxy with one rule, a prompt cache over input the
/// proxy logged as negative, routing that saved nothing and a clean pipeline.
fn proxy_only_home() -> Home {
    let home = Home::new();
    home.proxy(&format!(
        "INSERT INTO usage_records (ts, pipeline, tokens_in, tokens_saved, saved_ccr,
             saved_engine_methods, saved_rules, cache_read_tokens, routed_from, cost_usd,
             cost_without)
         VALUES ({}, 'neg-pipe', -10000, 70, 20, 50, '{{\"only-rule\":50}}', 100, 'big-model',
             1.0, 1.0);",
        ts(-5)
    ));
    home.write(
        "rtk/pipeline-status.json",
        r#"{"fell_back": false, "diagnostics": []}"#,
    );
    home
}

#[test]
fn a_proxy_only_summary_skips_the_lines_it_has_no_data_for() {
    let home = proxy_only_home();
    let run = home.gain(&["--history"]);
    assert!(run.success, "{}", run.stderr);
    let out = &run.stdout;
    for expected in [
        "Total commands:    0",
        "  via ccr:         20",
        "  via rules:       50",
        "    only-rule:     50",
        "Cache hit:         100.0%",
        "Bill (proxy):      $1.00 actual vs ~$1.00 without pipeline (0%)",
        "Routed:            1 requests (0 tokens in) to a cheaper model",
    ] {
        assert!(out.contains(expected), "missing {expected:?} in:\n{out}");
    }
    for absent in [
        "more · rtk gain --by-rule",
        "Cache effect",
        "fractures",
        "Recent Commands",
    ] {
        assert!(!out.contains(absent), "{absent:?} in:\n{out}");
    }
    let routed = out
        .lines()
        .find(|line| line.starts_with("Routed:            1 requests"))
        .expect("the ledger's routed line");
    assert!(
        !routed.contains("saved est."),
        "routing saved nothing: {routed}"
    );
    assert!(!run.stderr.contains("pipeline.toml"), "{}", run.stderr);

    let methods = home.ok(&["--by-method"]);
    assert!(
        methods.contains("By Method — proxy site (all time)"),
        "{methods}"
    );
    assert!(!methods.contains("recovery calls"), "{methods}");

    let detail = home.ok(&["--pipeline", "neg-pipe"]);
    assert!(
        detail.contains("  prompt cache:    100 read · 0 written  (hit 100%)"),
        "{detail}"
    );
    assert!(
        !detail.contains("cache effect"),
        "no positive raw input, no effect to show:\n{detail}"
    );
}
