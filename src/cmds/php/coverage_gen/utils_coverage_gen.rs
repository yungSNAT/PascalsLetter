use super::*;
use crate::core::{test_isolation, user_env};
use std::fs;
use std::path::PathBuf;

/// Run `f` with Composer's bin directory set to an absolute directory in
/// `project`, so no fixture lands at a path relative to the process directory.
fn with_scratch_bin_dir<R>(project: &Path, f: impl FnOnce(&Path) -> R) -> R {
    let bin = project.join("composer-bin");
    user_env::with_path("COMPOSER_BIN_DIR", Some(&bin), || f(&bin))
}

fn scratch_composer_candidate(bin: &Path, tool: &str) -> PathBuf {
    let candidate = bin.join(tool);
    assert!(
        composer_tool_paths(tool).contains(&candidate),
        "COMPOSER_BIN_DIR is not searched"
    );
    candidate
}

fn write_tool_marker(path: &Path) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, b"").unwrap();
}

#[test]
fn php_tool_command_prefers_an_existing_composer_tool() {
    let project = test_isolation::tempdir();
    let _project_dir = test_isolation::enter(project.path());
    let tool = "rtk-coverage-php-tool";

    with_scratch_bin_dir(project.path(), |bin| {
        let fallback = resolved_command(tool);
        assert_eq!(php_tool_command(tool).get_program(), fallback.get_program());

        let local_tool = scratch_composer_candidate(bin, tool);
        write_tool_marker(&local_tool);

        let local_command = resolved_command(local_tool.to_string_lossy().as_ref());
        let selected = php_tool_command(tool);
        assert_eq!(selected.get_program(), local_command.get_program());
        assert_ne!(selected.get_program(), fallback.get_program());
    });
}

#[test]
fn php_test_runner_prefers_pest_when_both_composer_tools_exist() {
    let project = test_isolation::tempdir();
    let _project_dir = test_isolation::enter(project.path());

    with_scratch_bin_dir(project.path(), |bin| {
        // Cargo runs this test from the package root, which has no PHPUnit XML
        // markers. Only the Composer lookup uses the isolated project directory.
        assert_eq!(detect_php_test_runner(), PhpTestRunner::Unknown);

        write_tool_marker(&scratch_composer_candidate(bin, "phpunit"));
        assert_eq!(detect_php_test_runner(), PhpTestRunner::Phpunit);

        write_tool_marker(&scratch_composer_candidate(bin, "pest"));
        assert_eq!(detect_php_test_runner(), PhpTestRunner::Pest);
    });
}
