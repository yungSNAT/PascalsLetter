use super::is_shell_wrapper_candidate;

#[test]
fn shell_wrapper_candidate_requires_command_mode() {
    for command in ["bash \t", "bash --norc -eu"] {
        assert!(
            !is_shell_wrapper_candidate(command),
            "options without command-string mode must not form a wrapper: {command:?}"
        );
    }

    assert!(is_shell_wrapper_candidate("bash --norc -eu -c 'printf ok'"));
}
