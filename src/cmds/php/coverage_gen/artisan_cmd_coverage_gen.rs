use super::{PhpTestRunner, filter_artisan_test_output};

#[test]
fn unknown_runner_cleans_output_without_removing_runner_banner() {
    let output = "PHPUnit 12.2.0\n....\nOK (4 tests, 4 assertions)\n";

    assert_eq!(
        filter_artisan_test_output(output, PhpTestRunner::Unknown),
        "PHPUnit 12.2.0\n..\nOK (4 tests, 4 assertions)"
    );
}
