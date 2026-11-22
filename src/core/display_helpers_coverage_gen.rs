use super::{PeriodStats, WeekStats, format_duration};

#[test]
fn format_duration_switches_units_at_one_second_and_one_minute() {
    for (ms, expected) in [
        (999, "999ms"),
        (1_000, "1.0s"),
        (1_260, "1.3s"),
        (59_999, "60.0s"),
        (60_000, "1m0s"),
        (125_999, "2m5s"),
    ] {
        assert_eq!(format_duration(ms), expected, "duration: {ms} ms");
    }
}

#[test]
fn weekly_period_preserves_short_dates_at_either_endpoint() {
    for (start, end) in [
        ("12-29", "01-04"),
        ("12-29", "2026-01-04"),
        ("2025-12-29", "01-04"),
    ] {
        let week = WeekStats {
            week_start: start.to_string(),
            week_end: end.to_string(),
            commands: 0,
            input_tokens: 0,
            output_tokens: 0,
            saved_tokens: 0,
            savings_pct: 0.0,
            total_time_ms: 0,
            avg_time_ms: 0,
            timed_commands: 0,
        };

        assert_eq!(week.period(), "12-29 → 01-04", "range: {start} to {end}");
    }
}
