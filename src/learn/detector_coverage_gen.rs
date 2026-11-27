use super::*;

fn execution(command: &str, is_error: bool, output: &str) -> CommandExecution {
    CommandExecution {
        command: command.to_owned(),
        is_error,
        output: output.to_owned(),
    }
}

#[test]
fn error_types_have_distinct_labels_and_preserve_custom_text() {
    for (error_type, expected) in [
        (ErrorType::UnknownFlag, "Unknown Flag"),
        (ErrorType::CommandNotFound, "Command Not Found"),
        (ErrorType::WrongSyntax, "Wrong Syntax"),
        (ErrorType::WrongPath, "Wrong Path"),
        (ErrorType::MissingArg, "Missing Argument"),
        (ErrorType::PermissionDenied, "Permission Denied"),
        (
            ErrorType::Other("Custom diagnostic".to_owned()),
            "Custom diagnostic",
        ),
    ] {
        assert_eq!(error_type.as_str(), expected);
    }
}

#[test]
fn blank_commands_have_no_base() {
    assert!(extract_base_command("").is_empty());
    assert!(extract_base_command(" \t\r\n").is_empty());
}

#[test]
fn test_failure_diagnostics_are_not_cli_corrections() {
    for (output, expected_count) in [
        ("test result: FAILED\nfixture setup: permission denied", 0),
        ("error: 2 tests failed", 0),
        ("error: suite FAILED", 0),
        ("error: unknown option --brief (status: FAILED)", 1),
    ] {
        let commands = [
            execution("runner test --brief", true, output),
            execution("runner test --verbose", false, "done"),
        ];

        assert_eq!(
            find_corrections(&commands).len(),
            expected_count,
            "{output}"
        );
    }
}

#[test]
fn path_exploration_and_identical_retries_do_not_hide_a_correction() {
    let original = "tool scan -a -b -c -d -e -f -g -h -i missing.txt";
    // Nine shared arguments put this filename swap above the path similarity threshold.
    let alternate = "tool scan -a -b -c -d -e -f -g -h -i alternate.txt";
    let fixed = "tool scan present.txt";

    assert!(!differs_only_by_path(original, "tool list missing.txt"));

    let commands = [
        execution(original, true, "error: file not found: missing.txt"),
        execution(alternate, false, "done"),
        execution(original, false, "done"),
        execution(fixed, false, "done"),
    ];

    let corrections = find_corrections(&commands);
    assert_eq!(corrections.len(), 1);
    assert_eq!(corrections[0].wrong_command, original);
    assert_eq!(corrections[0].right_command, fixed);
    assert_eq!(corrections[0].error_type, ErrorType::WrongPath);
}

#[test]
fn success_boost_makes_a_low_similarity_candidate_qualify() {
    let candidate = "tool scan --recursive";
    let mut commands = [
        execution("tool scan --recurse", true, "error: unknown flag --recurse"),
        execution(candidate, true, "error: permission denied"),
    ];

    assert!(find_corrections(&commands).is_empty());

    commands[1] = execution(candidate, false, "done");
    let corrections = find_corrections(&commands);
    assert_eq!(corrections.len(), 1);
    assert_eq!(corrections[0].right_command, candidate);
    assert!((corrections[0].confidence - 0.7).abs() < f64::EPSILON);
}

#[test]
fn diff_token_describes_one_sided_changes_and_ignores_order_and_duplicates() {
    for (wrong, right, expected) in [
        ("tool scan --obsolete", "tool scan", "removed --obsolete"),
        ("tool scan", "tool scan --recursive", "added --recursive"),
        (
            "tool scan one.txt one.txt two.txt",
            "tool scan two.txt one.txt",
            "unknown",
        ),
    ] {
        assert_eq!(extract_diff_token(wrong, right), expected);
    }
}
