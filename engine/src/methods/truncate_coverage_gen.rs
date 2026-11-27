use super::{Method, Truncate};
use crate::method::Recovery;

#[test]
fn truncate_declares_lossy_non_idempotent_behavior_with_recall() {
    let info = Truncate.info();

    assert!(info.lossy);
    assert!(matches!(info.recovery, Recovery::Recall));
    assert!(!info.idempotent);
}
