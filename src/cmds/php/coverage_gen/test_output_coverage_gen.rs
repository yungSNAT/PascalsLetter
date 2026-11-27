use super::*;

#[test]
fn blank_lines_are_not_progress_and_noise_only_output_returns_ok() {
    assert!(!is_progress_line(" \t "));

    let output = "PHPUnit 12.2.0\nRuntime: PHP 8.4\n\n   \n.... 4 / 4 (100%)\n";
    assert_eq!(filter_test_runner_output(output), "ok");
}

#[test]
fn truncation_applies_only_above_120_retained_lines() {
    let diagnostics: Vec<_> = (1..=125)
        .map(|number| format!("Diagnostic {number}"))
        .collect();

    let at_limit = diagnostics[..120].join("\n");
    let output = format!("PHPUnit 12.2.0\n....\n{at_limit}\n");
    assert_eq!(filter_test_runner_output(&output), at_limit);

    let expected = format!(
        "{}\n... +5 more lines\n{}",
        diagnostics[..80].join("\n"),
        diagnostics[85..].join("\n"),
    );
    assert_eq!(filter_test_runner_output(&diagnostics.join("\n")), expected,);
}
