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
fn colorize_recall_rate_keeps_the_padded_text() {
    for (pct, padded) in [(0, "    0%"), (15, "   15%"), (45, "   45%")] {
        let out = colorize_recall_rate(pct, padded);
        // Coloured on a terminal, plain otherwise; the text survives either way.
        assert!(out.contains(padded), "{out:?} lost {padded:?}");
        assert!(
            stdout_is_terminal() || out == padded,
            "{out:?} is not plain"
        );
    }
    let verdict = colorize_recall_rate(50, recall_verdict(10, 5));
    assert!(verdict.contains("too aggressive"));
}

#[cfg(feature = "engine")]
#[test]
fn attribution_sites_keep_hook_and_shadow_rows_apart() {
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
    });
}
