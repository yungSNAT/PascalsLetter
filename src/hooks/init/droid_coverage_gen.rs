use super::*;
use crate::core::test_isolation;
use serde_json::{Value, json};

#[cfg(unix)]
#[test]
fn install_reports_a_blocked_config_directory() {
    let temp = test_isolation::tempdir();
    let missing = temp.path().join("missing");
    let droid_dir = temp.path().join(DROID_DIR);
    std::os::unix::fs::symlink(&missing, &droid_dir).unwrap();

    let err = run_droid_mode_at(&droid_dir, false, InitContext::default()).unwrap_err();

    assert_eq!(
        err.to_string(),
        format!(
            "Failed to create Droid config directory: {}",
            droid_dir.display()
        )
    );
    assert_eq!(fs::read_link(&droid_dir).unwrap(), missing);
    assert!(!missing.exists());
}

#[test]
fn install_preserves_user_hooks_despite_a_corrupt_shadowed_file() {
    let temp = test_isolation::tempdir();
    let root_path = temp.path().join(DROID_HOOKS_FILE);
    let legacy_dir = temp.path().join(DROID_HOOKS_SUBDIR);
    fs::create_dir(&legacy_dir).unwrap();
    let legacy_path = legacy_dir.join(DROID_HOOKS_FILE);
    let corrupt = "{";
    fs::write(&legacy_path, corrupt).unwrap();

    let mut expected = json!({
        "PreToolUse": [{
            "matcher": DROID_EXECUTE_MATCHER,
            "hooks": [{ "type": "command", "command": "echo user-hook" }]
        }]
    });
    let original = expected.to_string();
    fs::write(&root_path, &original).unwrap();

    let ctx = InitContext {
        verbose: 1,
        ..Default::default()
    };
    let preview = InitContext {
        dry_run: true,
        ..ctx
    };
    run_droid_mode_at(temp.path(), false, preview).unwrap();
    assert_eq!(fs::read_to_string(&root_path).unwrap(), original);
    assert!(!root_path.with_extension("json.bak").exists());

    run_droid_mode_at(temp.path(), false, ctx).unwrap();
    expected[PRE_TOOL_USE_KEY][0]["hooks"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "type": "command", "command": DROID_HOOK_COMMAND }));
    let installed = fs::read_to_string(&root_path).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&installed).unwrap(), expected);
    assert_eq!(fs::read_to_string(&legacy_path).unwrap(), corrupt);

    let target = DroidHookFile {
        path: root_path,
        layout: DroidLayout::Root,
    };
    assert!(!patch_droid_hook_file(&target, ctx).unwrap());
    assert_eq!(fs::read_to_string(&target.path).unwrap(), installed);
}

#[test]
fn insert_replaces_a_non_object_settings_document() {
    let mut root = json!(["not a settings object"]);

    insert_droid_hook_entry(&mut root, DroidLayout::Nested).unwrap();

    assert_eq!(
        root,
        json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": DROID_EXECUTE_MATCHER,
                    "hooks": [{ "type": "command", "command": DROID_HOOK_COMMAND }]
                }]
            }
        })
    );
}

#[test]
fn project_uninstall_previews_then_removes_only_the_rtk_command() {
    let project = test_isolation::tempdir();
    let _working_dir = test_isolation::enter(project.path());
    let droid_dir = project.path().join(DROID_DIR);
    let legacy_dir = droid_dir.join(DROID_HOOKS_SUBDIR);
    fs::create_dir_all(&legacy_dir).unwrap();
    let root_path = droid_dir.join(DROID_HOOKS_FILE);
    let legacy_path = legacy_dir.join(DROID_HOOKS_FILE);
    let settings_path = droid_dir.join(DROID_SETTINGS_FILE);
    let untouched = [
        (&legacy_path, r#"{"PreToolUse":false}"#),
        (&settings_path, r#"{"model":"custom","hooks":null}"#),
    ];
    for (path, content) in untouched {
        fs::write(path, content).unwrap();
    }

    let remaining = json!({
        "PreToolUse": [
            {
                "matcher": DROID_EXECUTE_MATCHER,
                "hooks": [{ "type": "command", "command": "echo user-hook" }]
            },
            { "matcher": "Edit", "hooks": null }
        ],
        "PostToolUse": []
    });
    let mut installed = remaining.clone();
    installed[PRE_TOOL_USE_KEY][0]["hooks"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "type": "command", "command": DROID_HOOK_COMMAND }));
    let original = installed.to_string();
    fs::write(&root_path, &original).unwrap();

    let ctx = InitContext::default();
    let preview = InitContext {
        dry_run: true,
        ..ctx
    };
    assert_eq!(
        uninstall_droid_at(&droid_dir, preview).unwrap(),
        [format!("Droid hook file: {}", root_path.display())]
    );
    uninstall_droid(false, preview).unwrap();
    let backup = root_path.with_extension("json.bak");
    assert_eq!(fs::read_to_string(&root_path).unwrap(), original);
    assert!(!backup.exists());

    uninstall_droid(false, ctx).unwrap();
    let uninstalled = fs::read_to_string(&root_path).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&uninstalled).unwrap(),
        remaining
    );

    uninstall_droid(false, ctx).unwrap();
    assert_eq!(fs::read_to_string(&root_path).unwrap(), uninstalled);
    assert_eq!(fs::read_to_string(backup).unwrap(), original);
    for (path, content) in untouched {
        assert_eq!(fs::read_to_string(path).unwrap(), content);
        assert!(!path.with_extension("json.bak").exists());
    }
}
