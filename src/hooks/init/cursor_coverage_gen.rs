use super::*;
use crate::core::test_isolation;
use serde_json::{Value, json};

fn with_isolated_cursor_dir(test: impl FnOnce(&Path)) {
    let temp = test_isolation::tempdir();
    test_isolation::with_root(temp.path(), || {
        let cursor_dir = resolve_cursor_dir().unwrap();
        assert!(cursor_dir.starts_with(temp.path()));
        test(&cursor_dir);
    });
}

#[test]
fn cursor_migration_previews_then_replaces_the_legacy_hook() {
    with_isolated_cursor_dir(|cursor_dir| {
        let script = cursor_dir.join(HOOKS_SUBDIR).join(REWRITE_HOOK_FILE);
        let hooks_json = cursor_dir.join(HOOKS_JSON);
        fs::create_dir_all(script.parent().unwrap()).unwrap();
        fs::write(&script, "legacy hook").unwrap();

        let user_hook = json!({"command": "echo user", "matcher": "Shell"});
        let original = serde_json::to_string_pretty(&json!({
            "version": 1,
            "hooks": {
                "preToolUse": [
                    user_hook,
                    {"command": "./hooks/rtk-rewrite.sh", "matcher": "Shell"}
                ],
                "afterFileEdit": [{"command": "echo edited"}]
            }
        }))
        .unwrap();
        fs::write(&hooks_json, &original).unwrap();

        let verbose = InitContext {
            verbose: 1,
            ..InitContext::default()
        };
        install_cursor_hooks(InitContext {
            dry_run: true,
            ..verbose
        })
        .unwrap();
        assert_eq!(fs::read_to_string(&script).unwrap(), "legacy hook");
        assert_eq!(fs::read_to_string(&hooks_json).unwrap(), original);

        install_cursor_hooks(verbose).unwrap();
        assert!(!script.exists());
        let installed = fs::read_to_string(&hooks_json).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&installed).unwrap(),
            json!({
                "version": 1,
                "hooks": {
                    "preToolUse": [
                        user_hook,
                        {"command": CURSOR_HOOK_COMMAND, "matcher": "Shell"}
                    ],
                    "afterFileEdit": [{"command": "echo edited"}]
                }
            })
        );
        assert!(!patch_cursor_hooks_json(&hooks_json, verbose).unwrap());
        assert_eq!(fs::read_to_string(&hooks_json).unwrap(), installed);
    });
}

#[test]
fn cursor_legacy_cleanup_leaves_missing_and_hookless_files_alone() {
    let temp = test_isolation::tempdir();
    let path = temp.path().join(HOOKS_JSON);
    let ctx = InitContext::default();

    remove_legacy_cursor_hooks_json_entries(&path, ctx).unwrap();
    assert!(!path.exists());

    let original = "[\n  42\n]\n";
    fs::write(&path, original).unwrap();
    remove_legacy_cursor_hooks_json_entries(&path, ctx).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), original);

    assert!(patch_cursor_hooks_json(&path, ctx).unwrap());
    let registered: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        registered,
        json!({
            "version": 1,
            "hooks": {
                "preToolUse": [{"command": CURSOR_HOOK_COMMAND, "matcher": "Shell"}]
            }
        })
    );
}

#[test]
fn cursor_install_reports_when_a_file_blocks_directory_creation() {
    with_isolated_cursor_dir(|cursor_dir| {
        fs::create_dir_all(cursor_dir.parent().unwrap()).unwrap();
        fs::write(cursor_dir, "keep this file").unwrap();

        let error = install_cursor_hooks(InitContext::default()).unwrap_err();

        assert_eq!(
            error.to_string(),
            format!("Failed to create {}", cursor_dir.display())
        );
        assert_eq!(fs::read_to_string(cursor_dir).unwrap(), "keep this file");
    });
}

#[test]
fn cursor_install_propagates_registration_error_after_legacy_script_removal() {
    with_isolated_cursor_dir(|cursor_dir| {
        let script = cursor_dir.join(HOOKS_SUBDIR).join(REWRITE_HOOK_FILE);
        let hooks_json = cursor_dir.join(HOOKS_JSON);
        fs::create_dir_all(script.parent().unwrap()).unwrap();
        fs::write(&script, "legacy hook").unwrap();
        fs::create_dir(&hooks_json).unwrap();

        let error = install_cursor_hooks(InitContext {
            verbose: 1,
            ..InitContext::default()
        })
        .unwrap_err();

        assert!(format!("{error:#}").contains(&hooks_json.display().to_string()));
        assert!(hooks_json.is_dir());
        assert!(!script.exists());
    });
}

#[test]
fn cursor_uninstall_previews_both_changes_but_reports_a_blocked_script_removal() {
    let temp = test_isolation::tempdir();
    let cursor_dir = temp.path().join(CURSOR_DIR);
    let script = cursor_dir.join(HOOKS_SUBDIR).join(REWRITE_HOOK_FILE);
    let hooks_json = cursor_dir.join(HOOKS_JSON);
    fs::create_dir_all(&script).unwrap();
    let payload = script.join("keep.txt");
    fs::write(&payload, "not a hook script").unwrap();
    let original = json!({
        "version": 1,
        "hooks": {
            "preToolUse": [{"command": CURSOR_HOOK_COMMAND, "matcher": "Shell"}]
        }
    })
    .to_string();
    fs::write(&hooks_json, &original).unwrap();

    let preview = remove_cursor_hooks_at(
        &cursor_dir,
        InitContext {
            dry_run: true,
            ..InitContext::default()
        },
    )
    .unwrap();
    assert_eq!(
        preview,
        [
            format!("Cursor hook: {}", script.display()),
            "Cursor hooks.json: removed RTK entry".to_string(),
        ]
    );
    assert_eq!(fs::read_to_string(&payload).unwrap(), "not a hook script");
    assert_eq!(fs::read_to_string(&hooks_json).unwrap(), original);

    let error = remove_cursor_hooks_at(&cursor_dir, InitContext::default()).unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("Failed to remove Cursor hook: {}", script.display())
    );
    assert_eq!(fs::read_to_string(&payload).unwrap(), "not a hook script");
    assert_eq!(fs::read_to_string(&hooks_json).unwrap(), original);
}
