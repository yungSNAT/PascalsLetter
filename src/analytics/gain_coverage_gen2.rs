use super::*;

fn stdout_is_terminal() -> bool {
    std::io::IsTerminal::is_terminal(&std::io::stdout())
}

#[test]
fn truncate_for_column_pads_cuts_and_elides() {
    assert_eq!(truncate_for_column("anything", 0), "");
    assert_eq!(truncate_for_column("hi", 5), "hi   ");
    assert_eq!(truncate_for_column("hello", 3), "hel");
    assert_eq!(truncate_for_column("abcd", 2), "ab");
    assert_eq!(truncate_for_column("hello world", 8), "hello...");
    let elided = truncate_for_column("éééééééééé", 6);
    assert_eq!(elided, "ééé...");
    assert_eq!(elided.chars().count(), 6);
}

#[test]
fn colorize_recall_rate_keeps_the_padded_text_in_every_band() {
    for (pct, padded) in [(0, "    0%"), (15, "   15%"), (45, "   45%")] {
        let out = colorize_recall_rate(pct, padded);
        if stdout_is_terminal() {
            assert!(out.contains(padded), "{out:?} lost {padded:?}");
        } else {
            assert_eq!(out, padded);
        }
    }
    let verdict = colorize_recall_rate(50, recall_verdict(10, 5));
    assert!(verdict.contains("too aggressive"));
}

#[test]
fn show_recall_stats_runs_on_an_empty_isolated_home() {
    let dir = crate::core::test_isolation::tempdir();
    crate::core::test_isolation::with_root(dir.path(), || {
        let result = show_recall_stats();
        assert!(result.is_ok(), "show_recall_stats failed: {result:?}");
    });
}

#[cfg(feature = "engine")]
#[test]
fn efficiency_ledger_reports_every_hook_kind_and_shadow_rules() {
    let dir = crate::core::test_isolation::tempdir();
    crate::core::test_isolation::with_root(dir.path(), || {
        let tracker = Tracker::new_in_memory().expect("tracker");
        tracker
            .record_attribution("hook", "pipeline", "my-pipe", 1000, 400)
            .expect("record pipeline");
        tracker
            .record_attribution("hook", "pipeline", "other-pipe", 500, 100)
            .expect("record pipeline");
        tracker
            .record_attribution("hook", "llm-spend", "classifier", 200, 150)
            .expect("record llm-spend");
        tracker
            .record_attribution("hook", "recovery", "git-log", 10, 10)
            .expect("record recovery");
        tracker
            .record_attribution("shadow", "pipeline", "my-pipe", 800, 200)
            .expect("record shadow");

        let pipeline = tracker
            .attribution_site("hook", "pipeline", 30)
            .expect("pipeline rows");
        let (calls, tin, tout) = pipeline.iter().fold((0, 0, 0), |(c, i, o), r| {
            (c + r.calls, i + r.tokens_in, o + r.tokens_out)
        });
        assert_eq!(calls, 2);
        assert_eq!(tin, 1500);
        assert_eq!(tout, 500);

        let shadow = tracker
            .attribution_site("shadow", "pipeline", 30)
            .expect("shadow rows");
        assert_eq!(shadow.len(), 1);
        assert_eq!(shadow[0].saved(), 600);

        // Exercises the filter+compress, LLM-spend, recall, no-proxy and
        // shadow branches of the panel.
        print_efficiency_ledger(&tracker);

        // Recording is read-only for the ledger: the rows are unchanged.
        let after = tracker
            .attribution_site("hook", "pipeline", 30)
            .expect("pipeline rows");
        assert_eq!(after.len(), pipeline.len());
    });
}

#[cfg(feature = "engine")]
#[test]
fn efficiency_ledger_handles_shadow_rows_without_input_tokens() {
    let dir = crate::core::test_isolation::tempdir();
    crate::core::test_isolation::with_root(dir.path(), || {
        let tracker = Tracker::new_in_memory().expect("tracker");
        tracker
            .record_attribution("shadow", "pipeline", "empty-pipe", 0, 0)
            .expect("record shadow");
        let rows = tracker
            .attribution_site("shadow", "pipeline", 30)
            .expect("shadow rows");
        assert!(rows.iter().all(|r| r.tokens_in == 0));
        print_efficiency_ledger(&tracker);
        assert_eq!(
            tracker
                .attribution_site("hook", "pipeline", 30)
                .expect("hook rows")
                .len(),
            0
        );
    });
}
