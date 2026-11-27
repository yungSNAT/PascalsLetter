use super::*;
use crate::core::test_isolation as isolation;
use std::path::Path;

fn with_recall_path(test: impl FnOnce(&Path)) {
    let dir = isolation::tempdir();
    let path = dir.path().join("nested").join("recall.db");
    isolation::with_root(dir.path(), || {
        let _cwd_guard = isolation::enter(dir.path());
        user_env::with_vars(&[("RTK_RECALL", None), ("RTK_TEE", None)], || {
            user_env::with_path("RTK_RECALL_DB", Some(&path), || test(&path));
        });
        // Release the cached SQLite handle before removing the scratch directory.
        CONN_CACHE.with(|cache| *cache.borrow_mut() = None);
    });
}

fn recall_args(hash: Option<&str>, list: bool) -> RecallArgs<'_> {
    RecallArgs {
        hash,
        full: false,
        from: None,
        lines: None,
        grep: None,
        list,
    }
}

#[test]
fn grep_uses_a_literal_for_invalid_patterns_and_preserves_input_if_it_cannot_compile() {
    let input = b"plain line\n[bracketed]\nlast line";
    assert_eq!(grep_bytes(input, "["), b"[bracketed]\n");

    // The unmatched ')' rejects the regex immediately. Escaping it still
    // leaves a literal large enough to exceed the regex compiler's size limit.
    let oversized = format!("){}", "😀".repeat(1 << 18));
    assert_eq!(grep_bytes(input, &oversized), input);
}

#[test]
fn recall_distinguishes_usage_missing_store_and_unavailable_store() {
    with_recall_path(|path| {
        assert_eq!(run_recall(recall_args(None, false)).unwrap(), 2);
        assert_eq!(run_recall(recall_args(None, true)).unwrap(), 0);
        assert_eq!(run_recall(recall_args(Some("deadbeef"), false)).unwrap(), 1);
        assert!(!path.exists(), "recall must not create a missing store");

        // SQLite cannot use a directory as its database file.
        std::fs::create_dir_all(path).unwrap();
        assert_eq!(run_recall(recall_args(None, true)).unwrap(), 1);
    });
}

#[test]
fn recall_lists_entries_and_counts_only_successful_reads_of_a_truncated_tail() {
    with_recall_path(|path| {
        let cfg = RetrieverConfig {
            max_entry_bytes: 12,
            max_entries: 0,
            ..RetrieverConfig::default()
        };
        assert!(matches!(store(&cfg, b"", "empty", None, 1), Stored::Empty));
        assert!(!path.exists(), "empty output must not create the store");

        let conn = open(&cfg).unwrap();
        assert!(path.is_file());
        assert_eq!(run_recall(recall_args(None, true)).unwrap(), 0);

        let Stored::Saved(saved) = store(
            &cfg,
            b"already shown\n[keep]\ntail\n",
            "coverage-long-command-for-recall-list",
            Some(7),
            2,
        ) else {
            panic!("the hidden tail should fit in the store");
        };
        assert!(saved.truncated);
        assert_eq!(saved.hidden_lines, 2);
        assert_eq!(
            recalled_content(&saved.hash).as_deref(),
            Some("[keep]\ntail\n")
        );

        let short = store_inner(&cfg, b"ok\n", "short", None, 1).unwrap();
        let prefix = &saved.hash[..6];
        // Give the second row a valid legacy-length key sharing the first
        // row's prefix, without relying on a chance hash collision.
        conn.execute(
            "UPDATE recall SET hash = ?1 WHERE hash = ?2",
            params![format!("{prefix}000000"), short.hash],
        )
        .unwrap();

        assert_eq!(run_recall(recall_args(None, true)).unwrap(), 0);
        assert_eq!(run_recall(recall_args(Some(prefix), false)).unwrap(), 1);
        assert_eq!(
            run_recall(RecallArgs {
                from: Some(3),
                ..recall_args(Some(&saved.hash), false)
            })
            .unwrap(),
            1
        );
        assert_eq!(
            run_recall(RecallArgs {
                lines: Some(0),
                ..recall_args(Some(&saved.hash), false)
            })
            .unwrap(),
            1
        );

        let recalled_count = || {
            conn.query_row(
                "SELECT COUNT(*) FROM recall WHERE recalled = 1",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap()
        };
        assert_eq!(recalled_count(), 0, "listing and failed reads do not count");

        // The literal '[' matches no line starting at line 2. An empty grep
        // result is still a successful recall, not a lost-lines error.
        assert_eq!(
            run_recall(RecallArgs {
                from: Some(2),
                grep: Some("["),
                ..recall_args(Some(&saved.hash), false)
            })
            .unwrap(),
            0
        );
        assert_eq!(recalled_count(), 1);
    });
}

#[test]
fn tee_stats_use_the_existing_store_and_public_reset_clears_read_deduplication() {
    with_recall_path(|path| {
        let cfg = RetrieverConfig {
            mode: RecoveryMode::Tee,
            ..RetrieverConfig::default()
        };
        let log = "capture.log";
        reset_stats().unwrap();
        record_tee_recall_with(&cfg, "grep", log);
        assert!(stats_snapshot().unwrap().is_empty());
        assert!(
            !path.exists(),
            "stats and tee reads must not create the store"
        );

        let conn = open(&cfg).unwrap();
        let disabled = RetrieverConfig {
            mode: RecoveryMode::Disabled,
            ..cfg.clone()
        };
        record_tee_elision(&disabled, "grep");
        bump_stat(&conn, "grep", "tee", "unknown");
        assert!(stats_snapshot().unwrap().is_empty());

        record_tee_elision(&cfg, "grep");
        record_tee_recall_with(&cfg, "grep", log);
        record_tee_recall_with(&cfg, "grep", log);
        let snapshot = || {
            stats_snapshot()
                .unwrap()
                .into_iter()
                .map(|row| (row.slug, row.mode, row.elisions, row.recalls))
                .collect::<Vec<_>>()
        };
        assert_eq!(snapshot(), vec![("grep".into(), "tee".into(), 1, 1)]);

        reset_stats().unwrap();
        assert!(snapshot().is_empty());
        record_tee_recall_with(&cfg, "grep", log);
        assert_eq!(snapshot(), vec![("grep".into(), "tee".into(), 0, 1)]);
    });
}
