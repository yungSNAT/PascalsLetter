#[test]
fn tree_filter_skips_leading_whitespace_but_keeps_internal_spacing() {
    let raw = "\n \t\n.\n\n└── main.rs\n \t\n0 directories, 1 file\n";
    assert_eq!(super::filter_tree_output(raw), ".\n\n└── main.rs\n");
}

#[test]
fn tree_reports_install_options_when_unavailable() {
    use crate::core::test_isolation;

    if !test_isolation::rtk_binary_ready() {
        return;
    }

    // An empty PATH hides any installed tree.
    let empty = test_isolation::tempdir();
    let output = test_isolation::rtk_command()
        .env("PATH", empty.path())
        .arg("tree")
        .output()
        .expect("run the isolated rtk binary");
    assert!(!output.status.success(), "{output:?}");
    let message = String::from_utf8_lossy(&output.stderr);
    assert!(
        message.contains("tree command not found. Install it first:"),
        "{message}"
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
