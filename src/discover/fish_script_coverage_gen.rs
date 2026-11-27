use std::cell::Cell;

#[test]
fn platform_gate_controls_fish_wrapping_and_probe_calls() {
    let script = "begin; echo hello; end";
    assert!(super::is_unambiguous_fish(script));

    let config_checked = Cell::new(false);
    let fish_checked = Cell::new(false);
    let wrapped = super::try_wrap_with_probes(
        script,
        || {
            config_checked.set(true);
            true
        },
        || {
            assert!(config_checked.get(), "configuration must be checked first");
            fish_checked.set(true);
            true
        },
    );

    if cfg!(windows) {
        assert!(wrapped.is_none());
        assert!(!config_checked.get());
        assert!(!fish_checked.get());
    } else {
        assert_eq!(
            wrapped.as_deref(),
            Some("rtk run --shell fish -c 'begin; echo hello; end'")
        );
        assert!(config_checked.get());
        assert!(fish_checked.get());
    }
}
