use super::{EXIT_COMMAND_NOT_EXECUTABLE, command_from_args, spawn_failure};
use std::io::{self, ErrorKind};

#[test]
fn spawn_failure_reports_a_directory_as_not_executable() {
    let error =
        anyhow::Error::new(io::Error::from(ErrorKind::IsADirectory)).context("failed to spawn");
    let outcome = spawn_failure("tool-dir", &error).expect("directory errors should be mapped");

    assert_eq!(outcome.code, EXIT_COMMAND_NOT_EXECUTABLE);
    assert_eq!(outcome.message, "rtk: tool-dir: Is a directory\n");
}

#[test]
fn explicit_shell_without_a_script_reports_missing_command() {
    let Err(error) = command_from_args(&[], Some("unused-shell")) else {
        panic!("explicit shell mode should reject an empty argument list");
    };

    assert_eq!(
        error.to_string(),
        "command is required when --shell is used"
    );
}
