use super::*;
use crate::core::test_isolation;
use serde_json::json;

#[test]
fn invalid_hook_shapes_abort_install_before_any_target_is_written() {
    let temp = test_isolation::tempdir();
    let first = temp.path().join("first.json");
    let invalid = temp.path().join("invalid.json");
    let paths = [first.clone(), invalid.clone()];
    let original = "{ \"version\": 1, \"custom\": true }\n";
    fs::write(&first, original).unwrap();

    for (root, expected_error) in [
        (
            json!({ "version": 1, "hooks": [] }),
            "Trae hooks value is not an object",
        ),
        (
            json!({ "version": 1, "hooks": { PRE_TOOL_USE_KEY: {} } }),
            "Trae PreToolUse value is not an array",
        ),
    ] {
        let malformed = root.to_string();
        fs::write(&invalid, &malformed).unwrap();

        let error = patch_trae_hooks_json_paths(&paths, InitContext::default()).unwrap_err();
        assert_eq!(error.to_string(), expected_error);
        assert_eq!(fs::read_to_string(&first).unwrap(), original);
        assert_eq!(fs::read_to_string(&invalid).unwrap(), malformed);
        assert!(!first.with_extension("json.bak").exists());
        assert!(!invalid.with_extension("json.bak").exists());
    }
}

#[test]
fn project_dry_runs_preserve_config_and_backup_through_install_and_uninstall() {
    let temp = test_isolation::tempdir();
    test_isolation::with_root(temp.path(), || {
        let _workspace = test_isolation::enter(temp.path());
        let directory = temp.path().join(TRAE_DIR);
        let path = directory.join(HOOKS_JSON);
        let backup = path.with_extension("json.bak");
        let paths = std::slice::from_ref(&path);
        let verbose = InitContext {
            verbose: 1,
            ..InitContext::default()
        };
        let preview = InitContext {
            dry_run: true,
            ..verbose
        };

        assert!(!is_configured());
        uninstall_trae_mode(false, InitContext::default()).unwrap();
        assert!(!directory.exists());

        let original = "{ \"version\": 1, \"custom\": true }\n";
        fs::create_dir(&directory).unwrap();
        fs::write(&path, original).unwrap();

        assert_eq!(
            patch_trae_hooks_json_paths(paths, preview).unwrap(),
            [PatchResult::WouldPatch]
        );
        run_trae_mode(false, preview).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        assert!(!backup.exists());
        assert!(!is_configured());

        run_trae_mode(false, verbose).unwrap();
        let installed = fs::read_to_string(&path).unwrap();
        assert!(is_configured());
        assert_eq!(fs::read_to_string(&backup).unwrap(), original);

        run_trae_mode(false, verbose).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), installed);
        assert_eq!(fs::read_to_string(&backup).unwrap(), original);

        uninstall_trae_mode(false, preview).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), installed);
        assert_eq!(fs::read_to_string(&backup).unwrap(), original);
        assert!(is_configured());

        uninstall_trae_mode(false, InitContext::default()).unwrap();
        assert!(!is_configured());
        assert_eq!(fs::read_to_string(backup).unwrap(), installed);
        let removed = read_json_file(&path).unwrap().unwrap();
        assert_eq!(removed["custom"], true);
        assert_eq!(removed["hooks"][PRE_TOOL_USE_KEY], json!([]));
    });
}

#[test]
fn backup_failures_report_only_targets_already_in_the_requested_state() {
    let temp = test_isolation::tempdir();
    let path = temp.path().join(HOOKS_JSON);
    let backup = path.with_extension("json.bak");
    let paths = std::slice::from_ref(&path);
    let original = r#"{"version":1}"#;
    let update_error = |updated: &str| {
        format!(
            "Failed to update Trae hooks file {}. Already updated: {updated}",
            path.display()
        )
    };
    let backup_error = format!("Failed to backup to {}", backup.display());

    // A directory blocks replacement of the backup, regardless of permissions.
    fs::write(&path, original).unwrap();
    fs::create_dir(&backup).unwrap();
    let error = patch_trae_hooks_json_paths(paths, InitContext::default()).unwrap_err();
    assert_eq!(error.to_string(), update_error("none"));
    assert_eq!(error.chain().nth(1).unwrap().to_string(), backup_error);
    assert_eq!(fs::read_to_string(&path).unwrap(), original);

    let mut root = json!({ "version": 1 });
    insert_trae_hook_entry(&mut root).unwrap();
    let installed = root.to_string();
    fs::write(&path, &installed).unwrap();

    let error = remove_trae_hooks_json_paths(paths, InitContext::default()).unwrap_err();
    assert_eq!(error.to_string(), update_error("none"));
    assert_eq!(error.chain().nth(1).unwrap().to_string(), backup_error);
    assert_eq!(fs::read_to_string(&path).unwrap(), installed);

    let missing = temp.path().join("missing.json");
    let clean = temp.path().join("clean.json");
    fs::write(&clean, original).unwrap();
    let error = remove_trae_hooks_json_paths(
        &[missing.clone(), clean.clone(), path.clone()],
        InitContext::default(),
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        update_error(&clean.display().to_string())
    );
    assert!(!missing.exists());
    assert_eq!(fs::read_to_string(&clean).unwrap(), original);
    assert!(!clean.with_extension("json.bak").exists());
    assert_eq!(fs::read_to_string(path).unwrap(), installed);
    assert!(backup.is_dir());
}

#[cfg(unix)]
#[test]
fn install_reports_invalid_parent_paths_before_attempting_a_write() {
    // On Unix, an empty pathname is missing and has no parent.
    let error = patch_trae_hooks_json_paths(&[PathBuf::new()], InitContext::default()).unwrap_err();
    assert_eq!(
        error.chain().nth(1).unwrap().to_string(),
        "Cannot write Trae hooks file : path has no parent directory"
    );

    let temp = test_isolation::tempdir();
    let parent = temp.path().join("dangling");
    let target = temp.path().join("absent");
    // The hook looks missing during preflight, but mkdir cannot replace the link.
    std::os::unix::fs::symlink(&target, &parent).unwrap();
    let path = parent.join(HOOKS_JSON);
    let error = patch_trae_hooks_json_paths(&[path], InitContext::default()).unwrap_err();
    assert_eq!(
        error.chain().nth(1).unwrap().to_string(),
        format!("Failed to create Trae directory {}", parent.display())
    );
    assert_eq!(fs::read_link(parent).unwrap(), target);
    assert!(!target.exists());
}
