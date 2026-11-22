use super::*;
use crate::core::user_dirs::home as isolated_home;
use crate::core::{test_isolation, user_env};
use serde_json::{Value, json};

fn with_isolated_claude(test: impl FnOnce(&Path)) {
    let tmp = test_isolation::tempdir();
    let _entered = test_isolation::enter(tmp.path());
    user_env::with_path(
        "RTK_CONFIG_DIR",
        Some(tmp.path().join("rtk-config")),
        || {
            with_claude_dir_override(&tmp, |_| test(&resolve_claude_dir().unwrap()));
        },
    );
}

fn verbose_context() -> InitContext {
    InitContext {
        verbose: 1,
        ..Default::default()
    }
}

fn hook_group(matcher: &str, command: &str) -> Value {
    json!({
        "matcher": matcher,
        "hooks": [{"type": "command", "command": command}]
    })
}

#[test]
fn verbose_markdown_migration_preserves_an_existing_reference() {
    with_isolated_claude(|dir| {
        let ctx = verbose_context();
        let dry = InitContext {
            dry_run: true,
            ..ctx
        };
        let path = dir.join(CLAUDE_MD);

        run_claude_md_mode(true, ctx).unwrap();
        let injected = fs::read_to_string(&path).unwrap();
        assert!(injected.contains(RTK_BLOCK_START));
        assert!(injected.contains("rtk cargo test"));
        assert!(injected.contains(RTK_BLOCK_END));

        let original = format!("# Existing notes\n\n@RTK.md\n\n{injected}");
        fs::write(&path, &original).unwrap();
        assert!(patch_claude_md(&path, dry).unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), original);

        assert!(patch_claude_md(&path, ctx).unwrap());
        let migrated = fs::read_to_string(&path).unwrap();
        assert_eq!(migrated.trim(), "# Existing notes\n\n@RTK.md");
        assert!(!patch_claude_md(&path, ctx).unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), migrated);

        fs::write(&path, "# Notes without a reference\n").unwrap();
        assert!(!patch_claude_md(&path, dry).unwrap());
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "# Notes without a reference\n"
        );
        assert!(!patch_claude_md(&path, ctx).unwrap());
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "# Notes without a reference\n\n@RTK.md\n"
        );
    });
}

#[test]
fn stale_matcher_is_reported_previewed_and_repaired_without_touching_user_hooks() {
    with_isolated_claude(|dir| {
        let ctx = verbose_context();
        let path = dir.join(SETTINGS_JSON);
        let backup = path.with_extension("json.bak");
        let events = required_claude_events();
        let [(event, matcher)] = events.as_slice() else {
            panic!("the default pipeline should require exactly one Claude hook");
        };
        assert_eq!(*event, PRE_TOOL_USE_KEY);

        let mut expected = json!({
            "hooks": {
                (PRE_TOOL_USE_KEY): [
                    hook_group("StaleMatcher", CLAUDE_HOOK_COMMAND),
                    hook_group("Read", "echo user-hook")
                ]
            },
            "userSetting": true
        });
        let original = format!("\u{feff}{expected}");
        fs::write(&path, &original).unwrap();
        assert_eq!(
            claude_hook_gaps(),
            vec![format!(
                "{PRE_TOOL_USE_KEY} does not cover {}",
                matcher.replace('|', ", ")
            )]
        );

        let patch = |ctx| {
            patch_settings_json_command(
                CLAUDE_HOOK_COMMAND,
                PatchMode::Auto,
                false,
                ctx,
                HookInstallScope::UserGlobal,
            )
            .unwrap()
        };
        assert_eq!(
            patch(InitContext {
                dry_run: true,
                ..ctx
            }),
            PatchResult::WouldPatch
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        assert!(!backup.exists());

        assert_eq!(patch(ctx), PatchResult::Patched);
        expected["hooks"][PRE_TOOL_USE_KEY][0]["matcher"] = json!(matcher);
        assert_eq!(read_json_file(&path).unwrap().unwrap(), expected);
        assert!(claude_hook_gaps().is_empty());
        assert_eq!(fs::read_to_string(&backup).unwrap(), original);

        let installed = fs::read_to_string(&path).unwrap();
        assert_eq!(patch(ctx), PatchResult::AlreadyPresent);
        assert_eq!(fs::read_to_string(&path).unwrap(), installed);
        assert_eq!(fs::read_to_string(&backup).unwrap(), original);

        let mut without_event = json!({"hooks": {"SessionStart": []}});
        let unchanged = without_event.clone();
        assert!(!refresh_hook_matcher(
            &mut without_event,
            PRE_TOOL_USE_KEY,
            matcher
        ));
        assert_eq!(without_event, unchanged);
    });
}

#[test]
fn hook_only_install_is_global_only_and_can_be_removed() {
    with_isolated_claude(|dir| {
        let ctx = verbose_context();
        let project = test_isolation::tempdir();
        let _entered = test_isolation::enter(project.path());
        let path = dir.join(SETTINGS_JSON);
        let backup = path.with_extension("json.bak");
        let plugin = dir.join("opencode-plugin.js");

        run_hook_only_mode(false, PatchMode::Auto, Some(&plugin), ctx).unwrap();
        assert!(!project.path().join(CLAUDE_DIR).exists());
        assert!(!project.path().join(CLAUDE_MD).exists());
        assert!(!path.exists());
        assert_eq!(
            remove_settings_entries(ctx).unwrap(),
            SettingsRemoval::default()
        );
        assert!(!path.exists());
        assert!(!backup.exists());

        run_hook_only_mode(true, PatchMode::Auto, Some(&plugin), ctx).unwrap();
        assert!(binary_hook_registered(dir));
        assert!(!dir.join(RTK_MD).exists());
        assert!(!dir.join(CLAUDE_MD).exists());
        let installed = fs::read_to_string(&path).unwrap();

        run_hook_only_mode(true, PatchMode::Auto, Some(&plugin), ctx).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), installed);
        assert!(!backup.exists());

        assert_eq!(
            remove_settings_entries(ctx).unwrap(),
            SettingsRemoval {
                hook: true,
                proxy_base_url: false,
            }
        );
        assert_eq!(read_json_file(&path).unwrap().unwrap(), json!({}));
        assert_eq!(fs::read_to_string(&backup).unwrap(), installed);
    });
}

#[test]
fn legacy_settings_cleanup_validates_input_and_requires_a_backup_before_writing() {
    with_isolated_claude(|dir| {
        let ctx = verbose_context();
        let path = dir.join(SETTINGS_JSON);
        let backup = path.with_extension("json.bak");

        remove_legacy_settings_entries(ctx).unwrap();
        assert!(!path.exists());
        assert!(!backup.exists());

        let invalid_utf8 = [0xff_u8];
        fs::write(&path, invalid_utf8).unwrap();
        let install_error = patch_settings_json_command(
            CLAUDE_HOOK_COMMAND,
            PatchMode::Auto,
            false,
            ctx,
            HookInstallScope::UserGlobal,
        )
        .unwrap_err();
        let cleanup_error = remove_legacy_settings_entries(ctx).unwrap_err();
        let read_context = format!("Failed to read {}", path.display());
        assert_eq!(install_error.to_string(), read_context);
        assert_eq!(cleanup_error.to_string(), read_context);
        assert_eq!(fs::read(&path).unwrap(), invalid_utf8);
        assert!(!backup.exists());

        let malformed = "{not valid json";
        fs::write(&path, malformed).unwrap();
        assert_eq!(
            remove_legacy_settings_entries(ctx).unwrap_err().to_string(),
            format!("Failed to parse {}", path.display())
        );
        assert!(!binary_hook_registered(dir));
        assert!(claude_hook_gaps().is_empty());
        assert_eq!(fs::read_to_string(&path).unwrap(), malformed);
        assert!(!backup.exists());

        for untouched in ["\u{feff} \t\n", "{ \"userSetting\": true }\n"] {
            fs::write(&path, untouched).unwrap();
            remove_legacy_settings_entries(ctx).unwrap();
            assert_eq!(fs::read_to_string(&path).unwrap(), untouched);
            assert!(!backup.exists());
        }

        let native = hook_group("Bash", CLAUDE_HOOK_COMMAND);
        let original = json!({
            "hooks": {
                (PRE_TOOL_USE_KEY): [
                    hook_group("Bash", "./hooks/rtk-rewrite.sh"),
                    native
                ]
            },
            "userSetting": true
        })
        .to_string();
        fs::write(&path, &original).unwrap();

        remove_legacy_settings_entries(InitContext {
            dry_run: true,
            ..ctx
        })
        .unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        assert!(!backup.exists());

        fs::create_dir(&backup).unwrap();
        assert_eq!(
            remove_legacy_settings_entries(ctx).unwrap_err().to_string(),
            format!("Failed to backup to {}", backup.display())
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        assert!(backup.is_dir());

        fs::remove_dir(&backup).unwrap();
        remove_legacy_settings_entries(ctx).unwrap();
        assert_eq!(fs::read_to_string(&backup).unwrap(), original);
        assert_eq!(
            read_json_file(&path).unwrap().unwrap(),
            json!({"hooks": {(PRE_TOOL_USE_KEY): [native]}, "userSetting": true})
        );
    });
}

#[test]
fn legacy_script_migration_cleans_up_auxiliary_files_and_preserves_settings_on_failure() {
    with_isolated_claude(|dir| {
        let ctx = verbose_context();
        let home = isolated_home().unwrap();
        let hook_dir = home.join(CLAUDE_DIR).join(HOOKS_SUBDIR);
        let old_hook = hook_dir.join(REWRITE_HOOK_FILE);
        let hash = hook_dir.join(".rtk-hook.sha256");
        let cursor_dir = home.join(CURSOR_DIR).join(HOOKS_SUBDIR);
        let cursor_hook = cursor_dir.join(REWRITE_HOOK_FILE);
        let settings = dir.join(SETTINGS_JSON);
        let backup = settings.with_extension("json.bak");

        let original = json!({
            "hooks": {
                (PRE_TOOL_USE_KEY): [
                    hook_group("Bash", "./hooks/rtk-rewrite.sh"),
                    hook_group("Bash", "echo user-hook")
                ]
            }
        })
        .to_string();
        fs::create_dir_all(&hook_dir).unwrap();
        fs::create_dir_all(&cursor_dir).unwrap();
        fs::write(&old_hook, "legacy Claude hook").unwrap();
        fs::write(&hash, "legacy hash").unwrap();
        fs::write(&cursor_hook, "legacy Cursor hook").unwrap();
        fs::write(&settings, &original).unwrap();

        migrate_old_hook_script(InitContext {
            dry_run: true,
            ..ctx
        });
        assert_eq!(fs::read_to_string(&old_hook).unwrap(), "legacy Claude hook");
        assert_eq!(fs::read_to_string(&hash).unwrap(), "legacy hash");
        assert_eq!(
            fs::read_to_string(&cursor_hook).unwrap(),
            "legacy Cursor hook"
        );
        assert_eq!(fs::read_to_string(&settings).unwrap(), original);
        assert!(!backup.exists());

        // A directory cannot be removed by remove_file, even with elevated permissions.
        fs::remove_file(&old_hook).unwrap();
        fs::create_dir(&old_hook).unwrap();
        migrate_old_hook_script(ctx);
        assert!(old_hook.is_dir());
        assert_eq!(fs::read_to_string(&settings).unwrap(), original);
        assert!(!backup.exists());
        assert!(!hash.exists());
        assert!(!cursor_hook.exists());

        fs::remove_dir(&old_hook).unwrap();
        fs::write(&old_hook, "legacy Claude hook").unwrap();
        migrate_old_hook_script(ctx);
        assert!(!old_hook.exists());
        assert_eq!(fs::read_to_string(&backup).unwrap(), original);
        assert_eq!(
            read_json_file(&settings).unwrap().unwrap(),
            json!({"hooks": {(PRE_TOOL_USE_KEY): [hook_group("Bash", "echo user-hook")]}})
        );

        fs::write(&old_hook, "another legacy Claude hook").unwrap();
        fs::write(&settings, "{not valid json").unwrap();
        migrate_old_hook_script(ctx);
        assert!(!old_hook.exists());
        assert_eq!(fs::read_to_string(&settings).unwrap(), "{not valid json");
        assert_eq!(fs::read_to_string(&backup).unwrap(), original);
    });
}

#[cfg(feature = "engine")]
#[test]
fn project_posthook_is_not_registered_again_when_global_settings_already_cover_it() {
    with_isolated_claude(|dir| {
        let project = test_isolation::tempdir();
        let project_pipeline = project.path().join(".rtk");
        fs::create_dir(&project_pipeline).unwrap();
        fs::write(
            project_pipeline.join("pipeline.toml"),
            "schema_version = 1\n[pipeline]\nextends = \"rtk:posthook\"\n",
        )
        .unwrap();
        let _entered = test_isolation::enter(project.path());

        user_env::with_vars(
            &[("CI", Some("1")), ("RTK_TRUST_PROJECT_FILTERS", Some("1"))],
            || {
                let events = required_claude_events_scoped(HookInstallScope::ProjectExtra);
                let [(event, matcher)] = events.as_slice() else {
                    panic!("the project pipeline should add exactly one Claude hook");
                };
                assert_eq!(*event, POST_TOOL_USE_KEY);

                let global = dir.join(SETTINGS_JSON);
                let global_content = format!(
                    "\u{feff}{}",
                    json!({
                        "hooks": {
                            (POST_TOOL_USE_KEY): [hook_group(matcher, CLAUDE_HOOK_COMMAND)]
                        }
                    })
                );
                fs::write(&global, &global_content).unwrap();

                let project_claude = project.path().join(CLAUDE_DIR);
                fs::create_dir(&project_claude).unwrap();
                let local = project_claude.join(SETTINGS_JSON);
                let local_content = "{ \"userSetting\": true }\n";
                fs::write(&local, local_content).unwrap();

                assert_eq!(
                    patch_settings_json_command(
                        CLAUDE_HOOK_COMMAND,
                        PatchMode::Auto,
                        false,
                        verbose_context(),
                        HookInstallScope::ProjectExtra,
                    )
                    .unwrap(),
                    PatchResult::AlreadyPresent
                );
                assert_eq!(fs::read_to_string(&local).unwrap(), local_content);
                assert_eq!(fs::read_to_string(&global).unwrap(), global_content);
                assert!(!local.with_extension("json.bak").exists());
                assert!(!global.with_extension("json.bak").exists());
            },
        );
    });
}
