#[test]
fn tree_filter_skips_leading_whitespace_but_keeps_internal_spacing() {
    let raw = "\n \t\n.\n\n└── main.rs\n \t\n0 directories, 1 file\n";
    assert_eq!(super::filter_tree_output(raw), ".\n\n└── main.rs\n");
}

#[test]
fn tree_reports_install_options_when_unavailable() {
    if super::tool_exists("tree") {
        return;
    }

    let message = super::run(&[], 0).unwrap_err().to_string();
    assert_eq!(
        message.lines().next(),
        Some("tree command not found. Install it first:")
    );
    for hint in [
        "brew install tree",
        "sudo apt install tree",
        "sudo dnf install tree",
        "sudo pacman -S tree",
    ] {
        assert!(message.contains(hint), "missing {hint:?} in {message:?}");
    }
}
