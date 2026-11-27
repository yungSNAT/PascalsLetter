use super::*;
use crate::core::test_isolation::{tempdir, with_root};
use serde_json::{Value, json};

fn in_isolated_gemini(test: impl FnOnce(&Path)) {
    let scratch = tempdir();
    with_root(scratch.path(), || {
        let gemini_dir = resolve_gemini_dir().unwrap();
        assert!(gemini_dir.starts_with(scratch.path()));
        test(&gemini_dir);
    });
}

fn verbose_context(dry_run: bool) -> InitContext {
    InitContext {
        dry_run,
        verbose: 1,
        ..InitContext::default()
    }
}

#[test]
fn install_rejects_local_mode_and_reports_blocked_directories() {
    in_isolated_gemini(|gemini_dir| {
        let ctx = verbose_context(false);
        let error = run_gemini(false, false, PatchMode::Auto, ctx).unwrap_err();
        assert!(error.to_string().contains("Gemini support is global-only"));
        assert!(!gemini_dir.exists());

        fs::create_dir_all(gemini_dir.parent().unwrap()).unwrap();
        fs::write(gemini_dir, "config directory blocker").unwrap();
        let error = run_gemini(true, false, PatchMode::Auto, ctx).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!(
                "Failed to create Gemini config dir: {}",
                gemini_dir.display()
            )
        );
        assert_eq!(
            fs::read_to_string(gemini_dir).unwrap(),
            "config directory blocker"
        );

        fs::remove_file(gemini_dir).unwrap();
        fs::create_dir(gemini_dir).unwrap();
        let hook_dir = gemini_dir.join(HOOKS_SUBDIR);
        fs::write(&hook_dir, "hook directory blocker").unwrap();
        let error = run_gemini(true, false, PatchMode::Auto, ctx).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("Failed to create hook dir: {}", hook_dir.display())
        );
        assert_eq!(
            fs::read_to_string(&hook_dir).unwrap(),
            "hook directory blocker"
        );
        assert!(!gemini_dir.join(SETTINGS_JSON).exists());
    });
}

#[test]
fn dry_runs_preserve_state_and_install_uninstall_keep_unrelated_hooks() {
    in_isolated_gemini(|gemini_dir| {
        let preview = verbose_context(true);
        let apply = verbose_context(false);
        run_gemini(true, false, PatchMode::Ask, preview).unwrap();
        assert!(!gemini_dir.exists());

        fs::create_dir_all(gemini_dir).unwrap();
        let settings_path = gemini_dir.join(SETTINGS_JSON);
        let hook_path = gemini_dir.join(HOOKS_SUBDIR).join(GEMINI_HOOK_FILE);
        let awareness_path = gemini_dir.join(GEMINI_MD);

        fs::write(&settings_path, r#"{"model":"example"}"#).unwrap();
        assert!(!is_configured());

        let original = json!({
            "model": "example",
            "hooks": {
                BEFORE_TOOL_KEY: [{
                    "matcher": "run_shell_command",
                    "hooks": [{"type": "command", "command": "audit-tool"}]
                }]
            }
        });
        fs::write(&settings_path, original.to_string()).unwrap();

        run_gemini(true, false, PatchMode::Auto, apply).unwrap();
        assert_eq!(fs::read_to_string(&hook_path).unwrap(), GEMINI_HOOK_SCRIPT);
        let awareness = fs::read_to_string(&awareness_path).unwrap();
        assert_eq!(awareness, awareness_content(apply.awareness));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            assert_eq!(
                fs::metadata(&hook_path).unwrap().permissions().mode() & 0o777,
                0o755
            );
        }

        let installed_settings = fs::read_to_string(&settings_path).unwrap();
        let mut expected = original.clone();
        expected["hooks"][BEFORE_TOOL_KEY]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "matcher": "run_shell_command",
                "hooks": [{
                    "type": "command",
                    "command": hook_path.to_string_lossy()
                }]
            }));
        assert_eq!(
            serde_json::from_str::<Value>(&installed_settings).unwrap(),
            expected
        );
        assert!(is_configured());

        assert!(!patch_gemini_settings(gemini_dir, &hook_path, PatchMode::Auto, apply).unwrap());
        assert_eq!(
            fs::read_to_string(&settings_path).unwrap(),
            installed_settings
        );

        let expected_removals = vec![
            format!("Gemini hook: {}", hook_path.display()),
            format!("GEMINI.md: {}", awareness_path.display()),
            "Gemini settings.json: removed RTK hook entry".to_owned(),
        ];
        assert_eq!(uninstall_gemini(preview).unwrap(), expected_removals);
        assert_eq!(fs::read_to_string(&hook_path).unwrap(), GEMINI_HOOK_SCRIPT);
        assert_eq!(fs::read_to_string(&awareness_path).unwrap(), awareness);
        assert_eq!(
            fs::read_to_string(&settings_path).unwrap(),
            installed_settings
        );
        assert!(is_configured());

        assert_eq!(uninstall_gemini(apply).unwrap(), expected_removals);
        assert!(!hook_path.exists());
        assert!(!awareness_path.exists());
        assert_eq!(
            serde_json::from_str::<Value>(&fs::read_to_string(&settings_path).unwrap()).unwrap(),
            original
        );
        assert!(!is_configured());
        assert!(uninstall_gemini(apply).unwrap().is_empty());
    });
}

#[test]
fn malformed_settings_survive_an_otherwise_successful_install() {
    in_isolated_gemini(|gemini_dir| {
        fs::create_dir_all(gemini_dir).unwrap();
        let settings_path = gemini_dir.join(SETTINGS_JSON);
        let malformed = r#"{"model":"example","#;
        fs::write(&settings_path, malformed).unwrap();

        let ctx = verbose_context(false);
        run_gemini(true, false, PatchMode::Auto, ctx).unwrap();

        assert_eq!(fs::read_to_string(&settings_path).unwrap(), malformed);
        assert_eq!(
            fs::read_to_string(gemini_dir.join(HOOKS_SUBDIR).join(GEMINI_HOOK_FILE)).unwrap(),
            GEMINI_HOOK_SCRIPT
        );
        assert_eq!(
            fs::read_to_string(gemini_dir.join(GEMINI_MD)).unwrap(),
            awareness_content(ctx.awareness)
        );
        assert!(!is_configured());
    });
}

#[test]
fn patch_skips_or_initializes_blank_settings_but_reports_read_errors() {
    let scratch = tempdir();
    let gemini_dir = scratch.path();
    let settings_path = gemini_dir.join(SETTINGS_JSON);
    let hook_path = gemini_dir.join(GEMINI_HOOK_FILE);
    let ctx = verbose_context(false);
    let blank = "\u{feff} \t\n";
    fs::write(&settings_path, blank).unwrap();

    assert!(!patch_gemini_settings(gemini_dir, &hook_path, PatchMode::Skip, ctx).unwrap());
    assert_eq!(fs::read_to_string(&settings_path).unwrap(), blank);

    assert!(!patch_gemini_settings(gemini_dir, &hook_path, PatchMode::Auto, ctx).unwrap());
    let patched: Value =
        serde_json::from_str(&fs::read_to_string(&settings_path).unwrap()).unwrap();
    assert_eq!(
        patched,
        json!({
            "hooks": {
                BEFORE_TOOL_KEY: [{
                    "matcher": "run_shell_command",
                    "hooks": [{"type": "command", "command": hook_path.to_string_lossy()}]
                }]
            }
        })
    );

    let invalid_utf8 = [0xff];
    fs::write(&settings_path, invalid_utf8).unwrap();
    let error = patch_gemini_settings(gemini_dir, &hook_path, PatchMode::Auto, ctx).unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("Failed to read {}", settings_path.display())
    );
    assert_eq!(fs::read(&settings_path).unwrap(), invalid_utf8);
}

#[test]
fn uninstall_reports_when_an_artifact_path_is_a_directory() {
    in_isolated_gemini(|gemini_dir| {
        let hook_path = gemini_dir.join(HOOKS_SUBDIR).join(GEMINI_HOOK_FILE);
        let awareness_path = gemini_dir.join(GEMINI_MD);
        let ctx = verbose_context(false);

        fs::create_dir_all(&hook_path).unwrap();
        let error = uninstall_gemini(ctx).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("Failed to remove {}", hook_path.display())
        );
        assert!(hook_path.is_dir());

        fs::remove_dir(&hook_path).unwrap();
        fs::create_dir(&awareness_path).unwrap();
        let error = uninstall_gemini(ctx).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("Failed to remove {}", awareness_path.display())
        );
        assert!(awareness_path.is_dir());
    });
}

#[cfg(windows)]
#[test]
fn patch_reports_when_existing_settings_cannot_be_replaced() {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_SHARE_READ: u32 = 1;

    let scratch = tempdir();
    let gemini_dir = scratch.path();
    let settings_path = gemini_dir.join(SETTINGS_JSON);
    let hook_path = gemini_dir.join(GEMINI_HOOK_FILE);
    let original = "{ }\n";
    fs::write(&settings_path, original).unwrap();

    // Allow the initial read, but deny the delete access required for replacement.
    let _settings_handle = fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(&settings_path)
        .unwrap();

    let error = patch_gemini_settings(
        gemini_dir,
        &hook_path,
        PatchMode::Auto,
        verbose_context(false),
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("Failed to write {}", settings_path.display())
    );
    assert_eq!(fs::read_to_string(&settings_path).unwrap(), original);
}
