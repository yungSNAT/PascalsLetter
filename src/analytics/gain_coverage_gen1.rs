use super::*;

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
}

#[test]
fn reset_with_yes_clears_recorded_commands() {
    let dir = crate::core::test_isolation::tempdir();
    crate::core::test_isolation::with_root(dir.path(), || {
        let tracker = Tracker::new().expect("tracker");
        tracker
            .record("git status", "rtk git status", 100, 20, 5)
            .expect("record a command");
        let summary = tracker.get_summary_filtered(None).expect("summary");
        assert_eq!(summary.total_commands, 1);

        run(
            false, false, false, false, "pro", false, false, false, false, "text", false, false,
            true, true, false, false, 0,
        )
        .expect("reset");

        let summary = Tracker::new()
            .expect("tracker")
            .get_summary_filtered(None)
            .expect("summary");
        assert_eq!(summary.total_commands, 0);
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

        #[cfg(unix)]
        pipeline_detail_stdout(pipe, "cov-gen1-unknown", |known, unknown| {
            assert!(known.contains("Pipeline: cov-gen1-pipe"), "{known}");
            assert!(known.contains("Agent hooks"), "{known}");
            assert!(known.contains("shell output (bash)"), "{known}");
            assert!(known.contains("total"), "{known}");
            assert!(
                unknown.contains("no recorded savings for pipeline `cov-gen1-unknown`"),
                "{unknown}"
            );
            assert!(
                unknown.contains("pipelines with data: cov-gen1-pipe"),
                "{unknown}"
            );
        });

        let reopened = Tracker::new().expect("tracker");
        let slices = hook_slices_for(&reopened, pipe, 30).expect("slices");
        assert_eq!(
            slices,
            vec![("shell output (bash)".to_string(), 1, 1000, 300)]
        );
    });
}

/// Runs `rtk gain --pipeline` for a recorded and an unknown pipeline against the
/// isolated tracking database and hands both outputs to `check`.
#[cfg(all(unix, feature = "engine"))]
fn pipeline_detail_stdout(known: &str, unknown: &str, check: impl FnOnce(&str, &str)) {
    use crate::core::{test_isolation, tracking, user_dirs};
    let ready = test_isolation::rtk_binary_ready();
    let true = ready else { return };
    let db_path = tracking::get_db_path().expect("tracking database path");
    let data = user_dirs::data().expect("isolated data directory");
    let detail = |name: &str| {
        let output = test_isolation::rtk_command()
            .args(["gain", "--pipeline", name])
            .env("RTK_DB_PATH", &db_path)
            .env("XDG_DATA_HOME", &data)
            .stdin(std::process::Stdio::null())
            .output()
            .expect("run gain --pipeline");
        assert!(
            output.status.success(),
            "gain --pipeline {name} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("UTF-8 pipeline detail")
    };
    check(&detail(known), &detail(unknown));
}
