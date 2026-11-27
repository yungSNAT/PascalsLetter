use super::{PhpTestRunner, filter_artisan_test_output};

#[test]
fn unknown_runner_cleans_output_without_removing_runner_banner() {
    let output = "PHPUnit 12.2.0\n....\nOK (4 tests, 4 assertions)\n";

    // The progress line is left unpinned: the dot-leader cleanup shortens it
    // to "..", which is a side effect rather than a decision.
    let filtered = filter_artisan_test_output(output, PhpTestRunner::Unknown);
    assert!(filtered.starts_with("PHPUnit 12.2.0\n"), "{filtered}");
    assert!(
        filtered.ends_with("\nOK (4 tests, 4 assertions)"),
        "{filtered}"
    );
}
