use super::*;
use tempfile::TempDir;

fn uninstall_claude(ctx: InitContext) -> Result<i32> {
    uninstall_with_patch_mode(true, false, false, false, false, false, PatchMode::Ask, ctx)
}

fn dry() -> InitContext {
    InitContext {
        dry_run: true,
        ..InitContext::default()
    }
}

#[test]
fn uninstall_rejects_missing_global_flag() {
    let cursor_err = uninstall(
        false,
        false,
        false,
        true,
        false,
        false,
        InitContext::default(),
    )
    .expect_err("cursor uninstall needs --global");
    assert!(format!("{cursor_err:#}").contains("Cursor uninstall only works with --global flag"));

    let err = uninstall_with_patch_mode(
        false,
        false,
        false,
        false,
        false,
        false,
        PatchMode::Auto,
        InitContext::default(),
    )
    .expect_err("plain uninstall needs --global");
    assert!(format!("{err:#}").contains("Uninstall only works with --global flag"));
}

#[test]
fn uninstall_cursor_dry_run_then_real_removes_hook() {
    // Nothing installed: reports nothing to remove, in dry-run and for real.
    let empty = TempDir::new().unwrap();
    with_claude_dir_override(&empty, |_| {
        assert_eq!(
            uninstall(true, false, false, true, false, false, dry()).unwrap(),
            0
        );
        assert_eq!(
            uninstall(
                true,
                false,
                false,
                true,
                false,
                false,
                InitContext::default()
            )
            .unwrap(),
            0
        );
    });

    let temp = TempDir::new().unwrap();
    with_claude_dir_override(&temp, |_| {
        let cursor_dir = resolve_cursor_dir().expect("cursor dir under isolated home");
        fs::create_dir_all(&cursor_dir).unwrap();
        let hooks_json = cursor_dir.join(HOOKS_JSON);
        let root = serde_json::json!({
            "version": 1,
            "hooks": {
                "preToolUse": [
                    { "command": crate::hooks::constants::CURSOR_HOOK_COMMAND },
                    { "command": "echo user" }
                ]
            }
        });
        fs::write(&hooks_json, serde_json::to_string_pretty(&root).unwrap()).unwrap();
        let before = fs::read_to_string(&hooks_json).unwrap();

        assert_eq!(
            uninstall(true, false, false, true, false, false, dry()).unwrap(),
            0
        );
        assert_eq!(
            fs::read_to_string(&hooks_json).unwrap(),
            before,
            "dry-run must not touch hooks.json"
        );

        assert_eq!(
            uninstall(
                true,
                false,
                false,
                true,
                false,
                false,
                InitContext::default()
            )
            .unwrap(),
            0
        );
        if hooks_json.exists() {
            let after: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&hooks_json).unwrap()).unwrap();
            assert!(!cursor_hook_already_present(&after));
        }
    });
}

#[test]
fn uninstall_gemini_with_nothing_installed_reports_clean() {
    let temp = TempDir::new().unwrap();
    with_claude_dir_override(&temp, |claude_dir| {
        let entries_before = fs::read_dir(claude_dir).unwrap().count();
        assert_eq!(
            uninstall(true, true, false, false, false, false, dry()).unwrap(),
            0
        );
        assert_eq!(
            uninstall(
                true,
                true,
                false,
                false,
                false,
                false,
                InitContext::default()
            )
            .unwrap(),
            0
        );
        assert_eq!(fs::read_dir(claude_dir).unwrap().count(), entries_before);
    });
}

#[test]
fn uninstall_removes_legacy_hook_sidecar_and_rtk_md() {
    let temp = TempDir::new().unwrap();
    with_claude_dir_override(&temp, |claude_dir| {
        let hooks_dir = claude_dir.join(HOOKS_SUBDIR);
        fs::create_dir_all(&hooks_dir).unwrap();
        let hook = hooks_dir.join(REWRITE_HOOK_FILE);
        fs::write(&hook, "#!/bin/sh\nrtk rewrite\n").unwrap();
        let sidecar = integrity::hash_path_for(&hook);
        fs::write(&sidecar, "deadbeef  rtk-rewrite.sh\n").unwrap();
        let rtk_md = claude_dir.join(RTK_MD);
        fs::write(&rtk_md, "rtk notes").unwrap();

        assert_eq!(uninstall_claude(dry()).unwrap(), 0);
        assert!(hook.exists(), "dry-run keeps the hook script");
        assert!(sidecar.exists(), "dry-run keeps the hash sidecar");
        assert!(rtk_md.exists(), "dry-run keeps RTK.md");

        assert_eq!(uninstall_claude(InitContext::default()).unwrap(), 0);
        assert!(!hook.exists(), "hook script removed");
        assert!(!sidecar.exists(), "hash sidecar removed");
        assert!(!rtk_md.exists(), "RTK.md removed");

        print_manual_instructions(CLAUDE_HOOK_COMMAND, true);
        assert!(!claude_dir.join(SETTINGS_JSON).exists());
    });
}

#[test]
fn uninstall_keeps_unterminated_block_but_drops_reference() {
    let temp = TempDir::new().unwrap();
    with_claude_dir_override(&temp, |claude_dir| {
        let claude_md = claude_dir.join(CLAUDE_MD);
        let content = format!("# Mine\n\n@RTK.md\n\n{RTK_BLOCK_START} v2 -->\nunterminated\n");
        fs::write(&claude_md, content).unwrap();

        assert_eq!(uninstall_claude(InitContext::default()).unwrap(), 0);

        let cleaned = fs::read_to_string(&claude_md).unwrap();
        assert!(!cleaned.lines().any(|line| line.trim() == RTK_MD_REF));
        assert!(cleaned.contains("# Mine"));
        assert!(cleaned.contains(RTK_BLOCK_START));
        assert!(cleaned.contains("unterminated"));
    });
}

#[test]
fn uninstall_reports_unreadable_artifacts() {
    let temp = TempDir::new().unwrap();
    with_claude_dir_override(&temp, |claude_dir| {
        fs::create_dir(claude_dir.join(RTK_MD)).unwrap();
        let err = uninstall_claude(InitContext::default()).expect_err("RTK.md is a directory");
        assert!(format!("{err:#}").contains("Failed to remove RTK.md"));
    });

    let temp = TempDir::new().unwrap();
    with_claude_dir_override(&temp, |claude_dir| {
        fs::create_dir(claude_dir.join(CLAUDE_MD)).unwrap();
        let err = uninstall_claude(InitContext::default()).expect_err("CLAUDE.md is a directory");
        assert!(format!("{err:#}").contains("Failed to read CLAUDE.md"));
    });
}

#[test]
fn telemetry_consent_is_saved_and_respected() {
    let tmp = crate::core::test_isolation::tempdir();
    crate::core::test_isolation::with_root(tmp.path(), || {
        save_telemetry_consent(true).unwrap();
        let config = crate::core::config::Config::load().unwrap();
        assert_eq!(config.telemetry.consent_given, Some(true));
        assert!(config.telemetry.enabled);
        assert!(config.telemetry.consent_date.is_some());
        prompt_telemetry_consent().unwrap();
        assert_eq!(
            crate::core::config::Config::load()
                .unwrap()
                .telemetry
                .consent_given,
            Some(true)
        );

        save_telemetry_consent(false).unwrap();
        let config = crate::core::config::Config::load().unwrap();
        assert_eq!(config.telemetry.consent_given, Some(false));
        assert!(!config.telemetry.enabled);
        prompt_telemetry_consent().unwrap();
        assert_eq!(
            crate::core::config::Config::load()
                .unwrap()
                .telemetry
                .consent_given,
            Some(false)
        );
    });

    let fresh = crate::core::test_isolation::tempdir();
    crate::core::test_isolation::with_root(fresh.path(), || {
        crate::core::user_env::with_vars(&[("RTK_TELEMETRY_DISABLED", Some("1"))], || {
            prompt_telemetry_consent().unwrap();
        });
        assert_eq!(
            crate::core::config::Config::load()
                .unwrap()
                .telemetry
                .consent_given,
            None,
            "opt-out env must not record a consent answer"
        );
    });
}

#[cfg(feature = "engine")]
#[test]
fn ensure_pipeline_extends_creates_fresh_file_then_defers() {
    let tmp = crate::core::test_isolation::tempdir();
    let _cwd = crate::core::test_isolation::enter(tmp.path());
    crate::core::test_isolation::with_root(tmp.path(), || {
        let path = crate::core::pipeline_config::user_pipeline_path()
            .expect("user pipeline path under isolated root");
        match ensure_pipeline_extends("rtk:proxy", "# extra\n") {
            PipelineChange::Created(shown) => assert_eq!(shown, path.display().to_string()),
            _ => panic!("expected a fresh pipeline.toml to be created"),
        }
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.starts_with("schema_version = 1\n"));
        assert!(content.contains("extends = \"rtk:proxy\""));
        assert!(content.ends_with("# extra\n"));

        assert!(!matches!(
            ensure_pipeline_extends("rtk:proxy", ""),
            PipelineChange::Created(_)
        ));
        assert!(!matches!(
            ensure_pipeline_extends("rtk:delegate-reads", ""),
            PipelineChange::Created(_)
        ));
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            content,
            "an existing pipeline.toml is never edited"
        );
    });
}

#[cfg(feature = "engine")]
#[test]
fn delegation_onramp_stays_silent_without_prompting() {
    let tmp = crate::core::test_isolation::tempdir();
    let _cwd = crate::core::test_isolation::enter(tmp.path());
    crate::core::test_isolation::with_root(tmp.path(), || {
        assert!(offer_delegation_onramp(PatchMode::Auto, InitContext::default()).is_ok());
        assert!(offer_delegation_onramp(PatchMode::Skip, InitContext::default()).is_ok());
        assert!(offer_delegation_onramp(PatchMode::Ask, dry()).is_ok());
        crate::core::user_env::with_vars(&[("RTK_ONRAMP_DISABLED", Some("1"))], || {
            assert!(offer_delegation_onramp(PatchMode::Ask, InitContext::default()).is_ok());
        });
        let path = crate::core::pipeline_config::user_pipeline_path()
            .expect("user pipeline path under isolated root");
        assert!(!path.exists(), "no pipeline.toml written without consent");
    });
}
