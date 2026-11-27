use super::{PipelineSafety, RtkRule};

#[test]
fn default_rule_has_no_rewrite_and_uses_baseline_savings() {
    let rule = RtkRule::default();

    assert!(rule.pattern.is_empty());
    assert!(rule.rtk_cmd.is_empty());
    assert!(rule.rewrite_prefixes.is_empty());
    assert_eq!(rule.pipeline_safety, PipelineSafety::None);
    assert!(rule.category.is_empty());
    assert_eq!(rule.savings_pct.to_bits(), 60.0_f64.to_bits());
    assert!(rule.subcmd_savings.is_empty());
    assert!(rule.subcmd_status.is_empty());
}
