use super::*;
use crate::core::{test_isolation, user_env};

const USER_RULES: &str = "# Copilot rules\n\nUse concise examples.\n";

type CopilotAction = fn(InitContext) -> Result<()>;
type UninstallAt = fn(&Path, InitContext) -> Result<Vec<String>>;

fn check_copilot_lifecycle(config_dir: &Path, install: CopilotAction, uninstall: CopilotAction) {
    let hooks_dir = config_dir.join(HOOKS_SUBDIR);
    let hook = hooks_dir.join(COPILOT_HOOK_FILE);
    let instructions = config_dir.join(COPILOT_INSTRUCTIONS_FILE);
    fs::create_dir_all(&hooks_dir).unwrap();
    fs::write(&instructions, USER_RULES).unwrap();

    uninstall(InitContext::default()).unwrap();
    assert!(!hook.exists());
    assert_eq!(fs::read_to_string(&instructions).unwrap(), USER_RULES);

    install(InitContext::default()).unwrap();
    let installed_hook = fs::read_to_string(&hook).unwrap();
    let installed_instructions = fs::read_to_string(&instructions).unwrap();
    assert_eq!(installed_hook, COPILOT_HOOK_JSON);
    assert!(installed_instructions.contains(USER_RULES.trim()));
    assert!(installed_instructions.contains(RTK_BLOCK_START));
    assert!(installed_instructions.contains(RTK_BLOCK_END));

    uninstall(InitContext {
        dry_run: true,
        ..InitContext::default()
    })
    .unwrap();
    assert_eq!(fs::read_to_string(&hook).unwrap(), installed_hook);
    assert_eq!(
        fs::read_to_string(&instructions).unwrap(),
        installed_instructions
    );

    uninstall(InitContext::default()).unwrap();
    assert!(!hook.exists());
    assert_eq!(
        fs::read_to_string(&instructions).unwrap().trim(),
        USER_RULES.trim()
    );

    let incomplete = format!("{USER_RULES}\n{RTK_BLOCK_START}\nUnfinished RTK block.\n");
    fs::write(&instructions, &incomplete).unwrap();
    uninstall(InitContext::default()).unwrap();
    assert_eq!(fs::read_to_string(&instructions).unwrap(), incomplete);
}

#[test]
fn project_copilot_lifecycle_uses_isolated_working_directory() {
    let temp = test_isolation::tempdir();
    test_isolation::with_root(temp.path(), || {
        let _working_dir = test_isolation::enter(temp.path());
        check_copilot_lifecycle(
            &temp.path().join(GITHUB_DIR),
            run_copilot,
            uninstall_copilot,
        );
    });
}

#[test]
fn global_copilot_lifecycle_uses_configured_copilot_home() {
    let temp = test_isolation::tempdir();
    let config_dir = temp.path().join("copilot-config");
    test_isolation::with_root(temp.path(), || {
        user_env::with_path(COPILOT_HOME_ENV, Some(&config_dir), || {
            check_copilot_lifecycle(&config_dir, run_copilot_global, uninstall_copilot_global);
        });
    });
}

fn check_copilot_filesystem_errors(
    base: &Path,
    config_dir: &Path,
    install: fn(&Path, InitContext) -> Result<()>,
    uninstall: UninstallAt,
) {
    let hooks_dir = config_dir.join(HOOKS_SUBDIR);
    let hook = hooks_dir.join(COPILOT_HOOK_FILE);
    let instructions = config_dir.join(COPILOT_INSTRUCTIONS_FILE);
    fs::create_dir_all(config_dir).unwrap();
    fs::write(&hooks_dir, "not a directory").unwrap();

    let error = install(base, InitContext::default()).unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("Failed to create {} directory", hooks_dir.display())
    );
    assert_eq!(fs::read_to_string(&hooks_dir).unwrap(), "not a directory");
    assert!(!instructions.exists());

    fs::remove_file(&hooks_dir).unwrap();
    fs::create_dir_all(&hook).unwrap();
    fs::write(&instructions, USER_RULES).unwrap();

    let error = uninstall(base, InitContext::default()).unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("Failed to remove hook: {}", hook.display())
    );
    assert!(hook.is_dir());
    assert_eq!(fs::read_to_string(&instructions).unwrap(), USER_RULES);

    fs::remove_dir(&hook).unwrap();
    let invalid_utf8 = [0xff_u8];
    fs::write(&instructions, invalid_utf8).unwrap();

    let error = uninstall(base, InitContext::default()).unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("Failed to read {}", instructions.display())
    );
    assert_eq!(fs::read(&instructions).unwrap(), invalid_utf8);

    #[cfg(unix)]
    check_copilot_uninstall_write_permissions(base, config_dir, &instructions, uninstall);
}

#[cfg(unix)]
fn check_copilot_uninstall_write_permissions(
    base: &Path,
    config_dir: &Path,
    instructions: &Path,
    uninstall: UninstallAt,
) {
    use std::io::ErrorKind;
    use std::os::unix::fs::PermissionsExt;

    let managed = format!("{USER_RULES}\n{COPILOT_INSTRUCTIONS}");
    fs::write(instructions, &managed).unwrap();
    let original_permissions = fs::metadata(config_dir).unwrap().permissions();
    fs::set_permissions(config_dir, fs::Permissions::from_mode(0o500)).unwrap();

    // Privileged runners may be able to write despite the directory's mode.
    let probe = fs::write(config_dir.join("write-probe"), b"probe");
    let result = uninstall(base, InitContext::default());
    fs::set_permissions(config_dir, original_permissions).unwrap();

    match probe {
        Err(probe_error) => {
            assert_eq!(probe_error.kind(), ErrorKind::PermissionDenied);
            assert_eq!(
                result.unwrap_err().to_string(),
                format!("Failed to write {}", instructions.display())
            );
            assert_eq!(fs::read_to_string(instructions).unwrap(), managed);
        }
        Ok(()) => {
            assert_eq!(
                result.unwrap(),
                [format!(
                    "{COPILOT_INSTRUCTIONS_FILE}: removed rtk-instructions block"
                )]
            );
            assert_eq!(
                fs::read_to_string(instructions).unwrap().trim(),
                USER_RULES.trim()
            );
        }
    }
}

#[test]
fn copilot_install_and_uninstall_report_filesystem_errors_in_both_scopes() {
    let temp = test_isolation::tempdir();
    check_copilot_filesystem_errors(
        temp.path(),
        &temp.path().join(GITHUB_DIR),
        run_copilot_at,
        uninstall_copilot_at,
    );

    let config_dir = temp.path().join("copilot-config");
    check_copilot_filesystem_errors(
        &config_dir,
        &config_dir,
        run_copilot_global_at,
        uninstall_copilot_global_at,
    );
}
