use super::*;

fn usage(key: &str) -> CcusagePeriod {
    CcusagePeriod {
        key: key.to_string(),
        metrics: ccusage::CcusageMetrics {
            input_tokens: 1000,
            output_tokens: 500,
            cache_creation_tokens: 200,
            cache_read_tokens: 5000,
            total_tokens: 6700,
            total_cost: 42.5,
        },
    }
}

fn month_without_io(month: &str) -> MonthStats {
    MonthStats {
        month: month.to_string(),
        commands: 3,
        input_tokens: 0,
        output_tokens: 0,
        saved_tokens: 600,
        savings_pct: 99.0,
        total_time_ms: 0,
        avg_time_ms: 0,
        timed_commands: 3,
    }
}

#[test]
fn monthly_zero_io_savings_pct_and_totals() {
    let merged = merge_monthly(
        Some(vec![usage("2026-01")]),
        vec![month_without_io("2026-01")],
    );
    assert_eq!(merged.len(), 1);
    let p = &merged[0];
    assert_eq!(p.label, "2026-01");
    // input + output == 0 → savings pct falls back to 0.0, not the stored value.
    assert_eq!(p.rtk_savings_pct, Some(0.0));
    assert_eq!(p.rtk_commands, Some(3));
    assert_eq!(p.rtk_saved_tokens, Some(600));
    assert_eq!(p.weighted_input_cpt, Some(0.01));
    assert_eq!(p.savings_weighted, Some(6.0));
    assert_eq!(p.blended_cpt, Some(42.5 / 6700.0));
    assert_eq!(p.active_cpt, Some(42.5 / 1500.0));

    let totals = compute_totals(&merged);
    assert_eq!(totals.cc_cost, 42.5);
    assert_eq!(totals.cc_total_tokens, 6700);
    assert_eq!(totals.cc_active_tokens, 1500);
    assert_eq!(totals.cc_cache_create_tokens, 200);
    assert_eq!(totals.cc_cache_read_tokens, 5000);
    assert_eq!(totals.rtk_commands, 3);
    assert_eq!(totals.rtk_saved_tokens, 600);
    assert_eq!(totals.rtk_avg_savings_pct, 0.0);
    assert_eq!(totals.weighted_input_cpt, Some(0.01));
    assert_eq!(totals.savings_weighted, Some(6.0));
    assert_eq!(totals.blended_cpt, Some(42.5 / 6700.0));
    assert_eq!(totals.savings_blended, Some(600.0 * (42.5 / 6700.0)));
    assert_eq!(totals.active_cpt, Some(42.5 / 1500.0));
    assert_eq!(totals.savings_active, Some(600.0 * (42.5 / 1500.0)));
}

#[test]
fn merge_daily_ccusage_only_sorted_without_savings() {
    let daily = merge_daily(Some(vec![usage("2026-01-05"), usage("2026-01-02")]), vec![]);
    assert_eq!(
        daily.iter().map(|p| p.label.as_str()).collect::<Vec<_>>(),
        ["2026-01-02", "2026-01-05"]
    );
    for p in &daily {
        assert_eq!(p.cc_cost, Some(42.5));
        assert_eq!(p.cc_active_tokens, Some(1500));
        assert_eq!(p.cc_total_tokens, Some(6700));
        assert_eq!(p.rtk_commands, None);
        assert_eq!(p.rtk_saved_tokens, None);
        assert_eq!(p.weighted_input_cpt, None);
        assert_eq!(p.savings_weighted, None);
        assert_eq!(p.blended_cpt, None);
        assert_eq!(p.active_cpt, None);
    }

    // Non-verbose table with missing rtk columns must not panic or mutate data.
    print_period_table(&daily, 0);
    assert_eq!(daily.len(), 2);
    assert_eq!(daily[0].cc_cost, Some(42.5));

    let totals = compute_totals(&daily);
    assert_eq!(totals.cc_cost, 85.0);
    assert_eq!(totals.rtk_saved_tokens, 0);
    assert_eq!(totals.rtk_avg_savings_pct, 0.0);
    assert_eq!(totals.savings_weighted, Some(0.0));
}

#[test]
fn verbose_period_table_handles_full_and_empty_rows() {
    let mut periods = merge_monthly(
        Some(vec![usage("2026-01")]),
        vec![month_without_io("2026-01")],
    );
    periods.push(PeriodEconomics::new("2026-02"));

    print_period_table(&periods, 1);
    print_period_table(&periods, 0);

    assert_eq!(periods.len(), 2);
    assert_eq!(periods[0].label, "2026-01");
    assert_eq!(periods[0].savings_weighted, Some(6.0));
    assert_eq!(periods[0].savings_active, Some(600.0 * (42.5 / 1500.0)));
    assert_eq!(periods[0].savings_blended, Some(600.0 * (42.5 / 6700.0)));
    assert_eq!(periods[1].label, "2026-02");
    assert!(periods[1].cc_cost.is_none());
    assert!(periods[1].savings_weighted.is_none());
    assert!(periods[1].savings_active.is_none());
    assert!(periods[1].savings_blended.is_none());
    assert!(periods[1].rtk_commands.is_none());

    let empty_totals = compute_totals(&periods[1..]);
    assert_eq!(empty_totals.cc_cost, 0.0);
    assert!(empty_totals.weighted_input_cpt.is_none());
    assert!(empty_totals.blended_cpt.is_none());
    assert!(empty_totals.active_cpt.is_none());
}
