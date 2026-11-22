use super::*;
use crate::core::test_isolation;
use std::fs;
use std::path::PathBuf;

fn scratch_composer_candidate(project: &Path, tool: &str) -> Option<PathBuf> {
    // Never install a fixture at a path relative to the process directory.
    composer_tool_paths(tool)
        .into_iter()
        .find(|candidate| candidate.starts_with(project))
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

    let fallback = resolved_command(tool);
    assert_eq!(php_tool_command(tool).get_program(), fallback.get_program());

    let Some(local_tool) = scratch_composer_candidate(project.path(), tool) else {
        return;
    };
    write_tool_marker(&local_tool);

    let local_command = resolved_command(local_tool.to_string_lossy().as_ref());
    let selected = php_tool_command(tool);
    assert_eq!(selected.get_program(), local_command.get_program());
    assert_ne!(selected.get_program(), fallback.get_program());
}

#[test]
fn php_test_runner_prefers_pest_when_both_composer_tools_exist() {
    let project = test_isolation::tempdir();
    let _project_dir = test_isolation::enter(project.path());

    // Cargo runs this test from the package root, which has no PHPUnit XML
    // markers. Only the Composer lookup uses the isolated project directory.
    assert_eq!(detect_php_test_runner(), PhpTestRunner::Unknown);

    let (Some(phpunit), Some(pest)) = (
        scratch_composer_candidate(project.path(), "phpunit"),
        scratch_composer_candidate(project.path(), "pest"),
    ) else {
        return;
    };

    write_tool_marker(&phpunit);
    assert_eq!(detect_php_test_runner(), PhpTestRunner::Phpunit);

    write_tool_marker(&pest);
    assert_eq!(detect_php_test_runner(), PhpTestRunner::Pest);
}
