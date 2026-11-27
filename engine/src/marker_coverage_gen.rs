use super::carries_hook_stamp;

#[test]
fn detects_valid_stamps_without_legacy_hook_markers() {
    assert!(carries_hook_stamp(
        "output head\n[rtk:dedup 100→60 tokens]\noutput tail"
    ));
    assert!(!carries_hook_stamp(
        "output head\n[rtk:dedup 100→oops tokens]\noutput tail"
    ));
    assert!(!carries_hook_stamp("plain output without a stamp"));
}
