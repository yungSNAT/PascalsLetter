use super::*;
use crate::core::test_isolation as isolation;

fn write_legacy_rules(workspace: &Path, content: &str) -> PathBuf {
    let path = workspace.join(LEGACY_RULES_REL_PATH);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, content).unwrap();
    path
}

#[test]
fn verbose_workspace_install_previews_then_installs_and_removes_legacy_rules() {
    let workspace = isolation::tempdir();
    let plugin_dir = workspace.path().join(".agents/plugins/rtk");
    let legacy_path = write_legacy_rules(workspace.path(), LEGACY_ANTIGRAVITY_RULES_V036);
    let ctx = InitContext {
        verbose: 1,
        awareness: AwarenessLevel::High,
        ..InitContext::default()
    };

    run_antigravity_mode_at(
        workspace.path(),
        false,
        InitContext {
            dry_run: true,
            ..ctx
        },
    )
    .unwrap();
    assert!(!plugin_dir.exists());
    assert_eq!(
        fs::read_to_string(&legacy_path).unwrap(),
        LEGACY_ANTIGRAVITY_RULES_V036
    );

    run_antigravity_mode_at(workspace.path(), false, ctx).unwrap();
    for (path, expected) in [
        (plugin_dir.join("plugin.json"), ANTIGRAVITY_PLUGIN_JSON),
        (plugin_dir.join("hooks.json"), ANTIGRAVITY_HOOKS_JSON),
        (
            plugin_dir.join("rules").join(ANTIGRAVITY_RULES_FILE),
            RTK_AWARENESS_HIGH,
        ),
    ] {
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            expected,
            "{}",
            path.display()
        );
    }
    assert!(!legacy_path.exists());
}

#[test]
fn verbose_global_uninstall_removes_the_plugin_but_keeps_user_rules() {
    let workspace = isolation::tempdir();
    let user_root = isolation::tempdir();
    isolation::with_root(user_root.path(), || {
        let _guard = isolation::enter(workspace.path());
        let ctx = InitContext {
            verbose: 1,
            ..InitContext::default()
        };

        assert!(!is_configured());
        run_antigravity_mode(true, ctx).unwrap();
        assert!(is_configured());

        let user_rules = "# Team rules\r\n\r\nKeep examples runnable.";
        let legacy_suffix = LEGACY_ANTIGRAVITY_RULES_V049.replace('\n', "\r\n");
        let combined_rules = format!("{user_rules}\r\n\r\n{legacy_suffix}");
        let legacy_path = write_legacy_rules(workspace.path(), &combined_rules);

        uninstall_antigravity_mode(
            true,
            InitContext {
                dry_run: true,
                ..ctx
            },
        )
        .unwrap();
        assert!(is_configured());
        assert_eq!(fs::read_to_string(&legacy_path).unwrap(), combined_rules);

        uninstall_antigravity_mode(true, ctx).unwrap();
        assert!(!is_configured());
        assert_eq!(fs::read_to_string(&legacy_path).unwrap(), user_rules);
    });
}

#[test]
fn uninstall_reports_a_plugin_path_occupied_by_a_regular_file() {
    let workspace = isolation::tempdir();
    let plugin_path = workspace.path().join(".agents/plugins/rtk");
    fs::create_dir_all(plugin_path.parent().unwrap()).unwrap();
    fs::write(&plugin_path, "not a plugin directory").unwrap();
    let legacy_path = write_legacy_rules(workspace.path(), LEGACY_ANTIGRAVITY_RULES_V036);

    let error =
        uninstall_antigravity_mode_at(workspace.path(), false, InitContext::default()).unwrap_err();

    assert_eq!(
        error.to_string(),
        format!(
            "Failed to remove Antigravity plugin directory: {}",
            plugin_path.display()
        )
    );
    assert_eq!(
        fs::read_to_string(plugin_path).unwrap(),
        "not a plugin directory"
    );
    assert_eq!(
        fs::read_to_string(legacy_path).unwrap(),
        LEGACY_ANTIGRAVITY_RULES_V036
    );
}

#[cfg(unix)]
#[test]
fn migration_leaves_an_unreadable_regular_file_untouched() {
    use std::os::unix::fs::PermissionsExt as _;

    let workspace = isolation::tempdir();
    let legacy_path = write_legacy_rules(workspace.path(), LEGACY_ANTIGRAVITY_RULES_V036);
    let original_permissions = fs::metadata(&legacy_path).unwrap().permissions();
    fs::set_permissions(&legacy_path, fs::Permissions::from_mode(0o000)).unwrap();

    // Privileged runners may be able to read the file despite its mode.
    let Err(error) = fs::File::open(&legacy_path) else {
        fs::set_permissions(&legacy_path, original_permissions).unwrap();
        return;
    };
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert!(legacy_path.is_file());

    let action = migrate_legacy_rules_at(workspace.path(), InitContext::default());

    fs::set_permissions(&legacy_path, original_permissions).unwrap();
    assert_eq!(action, None);
    assert_eq!(
        fs::read_to_string(legacy_path).unwrap(),
        LEGACY_ANTIGRAVITY_RULES_V036
    );
}
