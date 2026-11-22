use super::*;
use crate::core::test_isolation;
use jiff::SignedDuration;

fn assert_sqlite_failure<T: std::fmt::Debug>(result: Result<T>, detail: &str) {
    let err = result.expect_err("expected a SQLite failure");
    let sqlite_err = err
        .downcast_ref::<rusqlite::Error>()
        .unwrap_or_else(|| panic!("expected a SQLite failure, got {err:#}"));
    // SQLite errors reported during statement preparation can have a different
    // variant from errors reported during execution.
    assert!(
        sqlite_err.to_string().contains(detail),
        "expected a SQLite failure involving {detail:?}, got {err:#}"
    );
}

fn assert_blob_decode_error<T: std::fmt::Debug>(result: Result<T>) {
    let err = result.expect_err("a BLOB must not decode as a string");
    assert!(
        matches!(
            err.downcast_ref::<rusqlite::Error>(),
            Some(rusqlite::Error::InvalidColumnType(
                _,
                _,
                rusqlite::types::Type::Blob
            ))
        ),
        "expected a BLOB column error, got {err:#}"
    );
}

#[test]
fn hook_log_round_trips_known_outcomes_and_keeps_writes_when_cleanup_fails() {
    let tracker = Tracker::new_in_memory().unwrap();
    tracker
        .conn
        .execute_batch(
            "INSERT INTO hook_decisions
                (timestamp, session_id, tool_use_id, raw_cmd, decision, rtk_version)
             VALUES
                ('2000-01-01T00:00:00Z', 's', 'stale', 'ls', 'allow', 'fixture'),
                (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), 's', 'unknown', 'ls',
                 'future-outcome', 'fixture');
             CREATE TRIGGER block_hook_cleanup BEFORE DELETE ON hook_decisions
             BEGIN
                 SELECT RAISE(ABORT, 'fixture blocked retention');
             END;",
        )
        .unwrap();

    let sampled = (0..100_000)
        .map(|i| format!("sampled-tool-{i}"))
        .find(|id| should_sample_cleanup(id, 500))
        .expect("one of the tool IDs should trigger cleanup");

    tracker
        .record_hook_decision(
            "s",
            &sampled,
            "",
            "ls",
            HookOutcome::Ask,
            Some("rtk ls"),
            "fixture",
        )
        .expect("a failed retention sweep must not turn a committed insert into an error");
    let log = tracker.hook_decisions_since(Timestamp::MIN).unwrap();
    assert_eq!(log[&sampled].decision, HookOutcome::Ask);
    assert_eq!(log["stale"].decision, HookOutcome::Allow);
    assert!(!log.contains_key("unknown"));

    tracker
        .conn
        .execute_batch("DROP TRIGGER block_hook_cleanup")
        .unwrap();
    tracker.maybe_cleanup_hook_decisions(&sampled).unwrap();
    let log = tracker.hook_decisions_since(Timestamp::MIN).unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log[&sampled].decision, HookOutcome::Ask);

    let outcomes = [
        (HookOutcome::Allow, "allow", true),
        (HookOutcome::Ask, "ask", true),
        (HookOutcome::Deny, "deny", false),
        (HookOutcome::Defer, "defer", false),
    ];
    for (outcome, spelling, covered) in outcomes {
        assert_eq!(outcome.to_string(), spelling);
        assert_eq!(spelling.parse::<HookOutcome>(), Ok(outcome));
        assert_eq!(outcome.is_covered(), covered);
        tracker
            .record_hook_decision(
                "s",
                spelling,
                "",
                "ls",
                outcome,
                covered.then_some("rtk ls"),
                "fixture",
            )
            .unwrap();
    }
    let log = tracker.hook_decisions_since(Timestamp::MIN).unwrap();
    assert_eq!(log.len(), 5);
    for (outcome, spelling, _) in outcomes {
        assert_eq!(log[spelling].decision, outcome);
    }
    assert_eq!("future-outcome".parse::<HookOutcome>(), Err(()));
}

#[test]
fn migration_normalizes_null_paths_without_changing_period_totals() {
    let tracker = Tracker::new_in_memory().unwrap();
    tracker
        .conn
        .execute_batch(
            "INSERT INTO commands
                (timestamp, original_cmd, rtk_cmd, input_tokens, output_tokens,
                 saved_tokens, savings_pct, exec_time_ms, project_path)
             VALUES
                ('2024-02-29T12:00:00Z', 'git status', 'rtk git status', 100, 25, 75, 75, 19, NULL),
                ('2024-02-29T13:00:00Z', 'git tag', 'rtk:passthrough git tag', 0, 0, 0, 0, 600, 'demo');",
        )
        .unwrap();

    run_schema_migrations(&tracker.conn).unwrap();

    let unscoped = tracker.get_recent_filtered(10, Some("")).unwrap();
    assert_eq!(unscoped.len(), 1);
    assert_eq!(unscoped[0].rtk_cmd, "rtk git status");
    let scoped = tracker.get_recent_filtered(10, Some("demo")).unwrap();
    assert_eq!(scoped.len(), 1);
    assert_eq!(scoped[0].rtk_cmd, "rtk:passthrough git tag");

    let days = tracker.get_all_days().unwrap();
    let weeks = tracker.get_by_week().unwrap();
    let months = tracker.get_by_month().unwrap();
    assert_eq!((days.len(), weeks.len(), months.len()), (1, 1, 1));
    assert_eq!(days[0].date, "2024-02-29");
    assert_eq!(weeks[0].week_start, "2024-02-26");
    assert_eq!(weeks[0].week_end, "2024-03-03");
    assert_eq!(months[0].month, "2024-02");
    for stats in [
        (days[0].commands, days[0].saved_tokens, days[0].avg_time_ms),
        (
            weeks[0].commands,
            weeks[0].saved_tokens,
            weeks[0].avg_time_ms,
        ),
        (
            months[0].commands,
            months[0].saved_tokens,
            months[0].avg_time_ms,
        ),
    ] {
        assert_eq!(stats, (2, 75, 19));
    }
}

#[test]
fn legacy_rows_handle_offsetless_dates_and_noncanonical_shell_spacing() {
    assert_eq!(
        stored_words("rtk 'local tool' --flag"),
        ["rtk", "local tool", "--flag"]
    );
    assert_eq!(
        stored_words("rtk\t'local tool' --flag"),
        ["rtk", "'local", "tool'", "--flag"]
    );

    let tracker = Tracker::new_in_memory().unwrap();
    tracker
        .conn
        .execute(
            "INSERT INTO commands
                (timestamp, original_cmd, rtk_cmd, input_tokens, output_tokens, saved_tokens, savings_pct)
             VALUES ('invalid timestamp', 'wc', 'rtk wc', 20, 30, -10, -50)",
            [],
        )
        .unwrap();

    let before = Timestamp::now();
    let recent = tracker.get_recent(1).unwrap();
    let after = Timestamp::now();
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].rtk_cmd, "rtk wc");
    assert_eq!(recent[0].saved_tokens, 0);
    assert_eq!(recent[0].savings_pct, -50.0);
    assert!((before..=after).contains(&recent[0].timestamp));
    assert_eq!(tracker.first_seen_days().unwrap(), 0);

    let old = before - SignedDuration::from_hours(10 * 24 + 12);
    for format in ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M:%S"] {
        tracker
            .conn
            .execute(
                "UPDATE commands SET timestamp = ?1",
                [old.strftime(format).to_string()],
            )
            .unwrap();
        assert_eq!(tracker.first_seen_days().unwrap(), 10, "{format}");
    }
}

#[test]
fn telemetry_distinguishes_recent_savings_from_lifetime_activity() {
    let tracker = Tracker::new_in_memory().unwrap();
    assert_eq!(tracker.overall_savings_pct().unwrap(), 0.0);
    assert!(tracker.ecosystem_mix().unwrap().is_empty());

    let now = Timestamp::now();
    for (days, cmd, project, input, output) in [
        (0, "rtk docker ps", "demo", 100, 50),
        (0, "rtk kubectl get", "demo", 100, 50),
        (4, "rtk rspec", "demo-tests", 100, 50),
        (5, "rtk ctest", "demo-tests", 100, 75),
        (5, "rtk bypass", "", 0, 0),
        (40, "rtk dotnet build", "", 100, 75),
        (100, "rtk ctest", "", 100, 75),
    ] {
        let timestamp = rfc3339(now - SignedDuration::from_hours(days * 24));
        let saved = input - output;
        tracker
            .conn
            .execute(
                "INSERT INTO commands
                    (timestamp, original_cmd, rtk_cmd, project_path,
                     input_tokens, output_tokens, saved_tokens, savings_pct)
                 VALUES (?1, ?2, ?2, ?3, ?4, ?5, ?6, COALESCE(100.0 * ?6 / NULLIF(?4, 0), 0))",
                params![timestamp, cmd, project, input, output, saved],
            )
            .unwrap();
    }

    let yesterday = now - SignedDuration::from_hours(24);
    assert_eq!(tracker.count_commands_since(yesterday).unwrap(), 2);
    assert_eq!(tracker.tokens_saved_24h(yesterday).unwrap(), 100);
    assert_eq!(tracker.active_days_30d().unwrap(), 3);
    assert_eq!(tracker.tokens_saved_30d().unwrap(), 175);
    assert_eq!(tracker.total_tokens_saved().unwrap(), 225);
    assert_eq!(tracker.overall_savings_pct().unwrap(), 37.5);
    assert_eq!(tracker.avg_savings_per_command().unwrap(), 40.0);
    assert_eq!(tracker.count_meta_command("ctest").unwrap(), 2);
    assert_eq!(tracker.projects_count().unwrap(), 2);

    let mix = tracker.ecosystem_mix().unwrap();
    assert_eq!(
        mix.first().map(|(name, pct)| (name.as_str(), *pct)),
        Some(("cloud", 40.0))
    );
    let by_category: HashMap<_, _> = mix.into_iter().collect();
    assert_eq!(
        by_category,
        HashMap::from([
            ("cloud".to_owned(), 40.0),
            ("cpp".to_owned(), 20.0),
            ("dotnet".to_owned(), 20.0),
            ("ruby".to_owned(), 20.0),
        ])
    );
}

#[test]
fn damaged_tracking_tables_return_errors_instead_of_reporting_empty_history() {
    let tracker = Tracker::new_in_memory().unwrap();
    tracker
        .conn
        .execute_batch(
            "CREATE TRIGGER block_record BEFORE INSERT ON commands
             BEGIN
                 SELECT RAISE(ABORT, 'fixture blocked insert');
             END;",
        )
        .unwrap();
    assert_sqlite_failure(
        tracker.record("ls", "rtk ls", 8, 2, 1),
        "fixture blocked insert",
    );
    assert_eq!(tracker.commands_total().unwrap(), 0);

    tracker
        .conn
        .execute_batch("DROP TABLE commands; DROP TABLE parse_failures; DROP TABLE hook_decisions;")
        .unwrap();
    assert_sqlite_failure(tracker.record("ls", "rtk ls", 8, 2, 1), "commands");
    assert_sqlite_failure(
        tracker.record_parse_failure("ls", "fixture error", false),
        "parse_failures",
    );
    assert_sqlite_failure(
        tracker.record_hook_decision("s", "u", "", "ls", HookOutcome::Deny, None, "fixture"),
        "hook_decisions",
    );
    let since = Timestamp::now() - SignedDuration::from_hours(24);
    for result in [
        tracker.count_commands_since(since),
        tracker.total_tokens_saved(),
        tracker.tokens_saved_24h(since),
        tracker.count_meta_command("ls"),
        tracker.active_days_30d(),
        tracker.tokens_saved_30d(),
        tracker.projects_count(),
    ] {
        assert_sqlite_failure(result, "commands");
    }
    assert_sqlite_failure(tracker.overall_savings_pct(), "commands");
    assert_sqlite_failure(tracker.avg_savings_per_command(), "commands");
    assert_sqlite_failure(tracker.ecosystem_mix(), "commands");
    assert_sqlite_failure(tracker.parse_failures_since(since), "parse_failures");
    let err = tracker
        .first_seen_days()
        .expect_err("commands table is missing");
    assert!(
        err.to_string()
            .starts_with("Failed to query first seen timestamp:")
    );
}

#[test]
fn parse_failure_summary_propagates_bad_columns_and_rows() {
    let tracker = Tracker::new_in_memory().unwrap();
    tracker
        .record_parse_failure("ls", "fixture error", true)
        .unwrap();

    tracker
        .conn
        .execute_batch("UPDATE parse_failures SET raw_command = X'FF';")
        .unwrap();
    assert_blob_decode_error(tracker.get_parse_failure_summary());

    tracker
        .conn
        .execute_batch("UPDATE parse_failures SET raw_command = 'ls', error_message = X'FF';")
        .unwrap();
    assert_blob_decode_error(tracker.get_parse_failure_summary());

    tracker
        .conn
        .execute_batch("ALTER TABLE parse_failures DROP COLUMN fallback_succeeded;")
        .unwrap();
    assert_sqlite_failure(tracker.get_parse_failure_summary(), "fallback_succeeded");
}

#[test]
fn tracking_helpers_write_to_the_configured_database_and_tolerate_an_open_error() {
    let root = test_isolation::tempdir();
    let db_path = root.path().join("history").join("tracking.db");

    test_isolation::with_root(root.path(), || {
        let _working_dir = test_isolation::enter(root.path());
        let config_dir = user_dirs::config().expect("isolated config directory");
        std::fs::create_dir_all(&config_dir).unwrap();
        let database_path = toml::Value::String(db_path.to_string_lossy().into_owned());
        std::fs::write(
            config_dir.join("config.toml"),
            format!(
                "[tracking]\nenabled = true\nhistory_days = 7\ndatabase_path = {database_path}\n"
            ),
        )
        .unwrap();

        user_env::with_vars(&[("RTK_DB_PATH", None)], || {
            assert_eq!(get_db_path().unwrap(), db_path);
            let since = Timestamp::now() - SignedDuration::from_hours(1);
            let timer = TimedExecution::start();
            timer.track("read", "rtk read", "abcdefghij", "abcd");
            timer.track_byte_counts("grep", "rtk grep", 17, 5);
            timer.track_passthrough("git tag", "rtk git tag");
            timer.track_passthrough("custom", "legacy custom");
            record_parse_failure_silent("read", "recovered fixture", true);
            record_parse_failure_silent("read", "unrecovered fixture", false);

            let bad_path = db_path.with_file_name("x".repeat(1024));
            user_env::with_path("RTK_DB_PATH", Some(&bad_path), || {
                let Err(err) = Tracker::new() else {
                    panic!("an overlong database file name should not open");
                };
                assert_eq!(
                    err.to_string(),
                    format!(
                        "Failed to pre-create private DB file: {}",
                        bad_path.display()
                    )
                );
                timer.track("ignored", "rtk ignored", "data", "");
                timer.track_byte_counts("ignored", "rtk ignored", 8, 1);
                timer.track_passthrough("ignored", "rtk ignored");
                record_parse_failure_silent("ignored", "database unavailable", false);
            });

            assert!(db_path.is_file());
            let tracker = Tracker::new().unwrap();
            let mut stmt = tracker
                .conn
                .prepare("SELECT rtk_cmd, input_tokens, output_tokens FROM commands ORDER BY id")
                .unwrap();
            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            assert_eq!(
                rows,
                [
                    ("rtk read".to_owned(), 3, 1),
                    ("rtk grep".to_owned(), 5, 2),
                    ("rtk:passthrough git tag".to_owned(), 0, 0),
                    ("legacy custom".to_owned(), 0, 0),
                ]
            );

            assert_eq!(tracker.parse_failures_since(since).unwrap(), 2);
            let failures = tracker.get_parse_failure_summary().unwrap();
            assert_eq!(failures.total, 2);
            assert_eq!(failures.recovery_rate, 50.0);
            assert_eq!(failures.top_commands, [("read".to_owned(), 2)]);
            assert_eq!(failures.recent.len(), 2);
            assert!(failures.recent.iter().any(|row| {
                row.raw_command == "read"
                    && row.error_message == "recovered fixture"
                    && row.fallback_succeeded
            }));
            assert!(failures.recent.iter().any(|row| {
                row.raw_command == "read"
                    && row.error_message == "unrecovered fixture"
                    && !row.fallback_succeeded
            }));
        });
    });
}

#[cfg(feature = "engine")]
#[test]
fn engine_tracking_replaces_cached_answers_and_surfaces_damaged_attribution() {
    let tracker = Tracker::new_in_memory().unwrap();
    assert_eq!(tracker.classifier_cache_get("example").unwrap(), None);
    tracker
        .classifier_cache_put("example", "first answer")
        .unwrap();
    tracker
        .classifier_cache_put("other", "independent answer")
        .unwrap();
    tracker
        .classifier_cache_put("example", "replacement")
        .unwrap();
    assert_eq!(
        tracker.classifier_cache_get("example").unwrap().as_deref(),
        Some("replacement")
    );
    assert_eq!(
        tracker.classifier_cache_get("other").unwrap().as_deref(),
        Some("independent answer")
    );

    tracker
        .record_attribution("hook", "pipeline", "compact", 100, 25)
        .unwrap();
    let rows = tracker.attribution_site("hook", "pipeline", 7).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "compact");
    assert_eq!(rows[0].saved(), 75);
    tracker
        .conn
        .execute_batch("UPDATE engine_attribution SET name = X'FF';")
        .unwrap();
    assert_blob_decode_error(tracker.attribution("pipeline", 7));

    tracker.reset_all().unwrap();
    assert_eq!(tracker.classifier_cache_get("example").unwrap(), None);
    assert!(tracker.attribution("pipeline", 7).unwrap().is_empty());

    tracker
        .conn
        .execute_batch("DROP TABLE engine_attribution;")
        .unwrap();
    assert_sqlite_failure(
        tracker.record_attribution("hook", "pipeline", "compact", 100, 25),
        "engine_attribution",
    );
}
