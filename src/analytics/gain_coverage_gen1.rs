use super::*;
use std::io::IsTerminal;

#[test]
fn pct_meter_and_pipeline_row_render_proportionally() {
    assert_eq!(pct_of(50, 200), 25.0);
    assert_eq!(pct_of(200, 200), 100.0);
    assert_eq!(pct_of(5, 0), 0.0);
    assert_eq!(pct_of(-3, -1), 0.0);

    let full = "█".repeat(10);
    let empty = "░".repeat(10);
    let empty_meter = pipeline_meter(0.0);
    assert!(empty_meter.contains(&empty), "got {empty_meter:?}");
    assert!(!empty_meter.contains('█'));

    let full_meter = pipeline_meter(100.0);
    assert!(full_meter.contains(&full), "got {full_meter:?}");
    assert!(!full_meter.contains('░'));

    // Over 100% is clamped to the full width.
    let over = pipeline_meter(250.0);
    assert!(over.contains(&full));
    assert!(!over.contains('░'));

    // 55% of 10 cells rounds to 6 filled, 4 empty.
    let mid = pipeline_meter(55.0);
    let expected = format!("{}{}", "█".repeat(6), "░".repeat(4));
    assert!(mid.contains(&expected), "got {mid:?}");

    let low = pipeline_meter(10.0);
    let expected_low = format!("{}{}", "█", "░".repeat(9));
    assert!(low.contains(&expected_low), "got {low:?}");

    // Rows print without panicking, including negative savings.
    print_pipeline_row("total", "3 calls".to_string(), 600, pct_of(600, 1000));
    print_pipeline_row("negative", "1 calls".to_string(), -50, pct_of(-50, 100));
}

#[test]
fn run_early_paths_work_against_an_isolated_root() {
    let dir = tempfile::tempdir().expect("tempdir");
    crate::core::test_isolation::with_root(dir.path(), || {
        // --recalls (without --reset) shows recall stats and returns.
        run(
            false, false, false, false, "pro", false, false, false, false, "text", false, true,
            false, false, false, false, 0,
        )
        .expect("recall stats view");

        // --failures on an empty database reports no failures.
        run(
            false, false, false, false, "pro", false, false, false, false, "text", true, false,
            false, false, false, false, 0,
        )
        .expect("failures view");

        // --watch without a terminal falls back to the (json) text view.
        if !std::io::stdout().is_terminal() {
            run(
                false, false, false, false, "pro", false, false, false, false, "json", false,
                false, false, false, true, false, 0,
            )
            .expect("watch fallback to json export");
        }

        // --reset --yes runs the reset and returns.
        run(
            false, false, false, false, "pro", false, false, false, false, "text", false, false,
            true, true, false, false, 0,
        )
        .expect("reset");

        let tracker = Tracker::new().expect("tracker");
        let summary = tracker.get_summary_filtered(None).expect("summary");
        assert_eq!(summary.total_commands, 0);
        let stats = crate::core::retriever::stats_snapshot().expect("stats");
        assert!(stats.is_empty());
    });
}

#[cfg(feature = "engine")]
#[test]
fn pipeline_detail_prints_hook_section_for_recorded_pipeline() {
    let dir = tempfile::tempdir().expect("tempdir");
    crate::core::test_isolation::with_root(dir.path(), || {
        let pipe = "cov-gen1-pipe";
        let tracker = Tracker::new().expect("tracker");
        tracker
            .record_attribution("hook", "pipeline", pipe, 2000, 800)
            .expect("record total");
        let key = crate::core::tracking::pipeline_slice_key(pipe, "shell");
        tracker
            .record_attribution("hook", "pipeline-slice", &key, 1000, 700)
            .expect("record slice");

        run_pipeline_detail(pipe).expect("pipeline detail");
        run_pipeline_detail("cov-gen1-unknown").expect("unknown pipeline detail");

        let reopened = Tracker::new().expect("tracker");
        let slices = hook_slices_for(&reopened, pipe, 30).expect("slices");
        assert_eq!(
            slices,
            vec![("shell output (bash)".to_string(), 1, 1000, 300)]
        );
    });
}
