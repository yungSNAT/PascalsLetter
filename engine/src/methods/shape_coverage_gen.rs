use super::*;

#[test]
fn advertises_lossy_non_idempotent_shaping_with_fast_cost_and_recall() {
    let info = ShapeMethod.info();

    assert!(info.lossy);
    assert!(!info.idempotent);
    assert!(matches!(info.recovery, Recovery::Recall));
    assert!(matches!(info.cost, CostClass::Fast));
}
