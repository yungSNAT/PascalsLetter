use super::*;
use serde_json::json;

#[test]
fn hook_edits_reject_non_array_events_and_preserve_unowned_groups() {
    let original = json!({"hooks": {"PreToolUse": {"user": true}}, "keep": 42});
    let mut root = original.clone();
    let error = append_hook_entry(
        &mut root,
        PRE_TOOL_USE_KEY,
        json!({"command": CLAUDE_HOOK_COMMAND}),
    )
    .unwrap_err();
    assert_eq!(error.to_string(), "PreToolUse value is not an array");
    assert_eq!(root, original);

    let error =
        insert_hook_entry(&mut root, PRE_TOOL_USE_KEY, "Bash", CLAUDE_HOOK_COMMAND).unwrap_err();
    assert_eq!(error.to_string(), "PreToolUse value is not an array");
    assert_eq!(root, original);

    let mut root = json!(["not a settings object"]);
    insert_hook_entry(&mut root, PRE_TOOL_USE_KEY, "Bash", CLAUDE_HOOK_COMMAND).unwrap();
    assert_eq!(
        root,
        json!({"hooks": {"PreToolUse": [{
            "matcher": "Bash",
            "hooks": [{"type": "command", "command": CLAUDE_HOOK_COMMAND}]
        }]}})
    );

    let opaque_group = json!({"matcher": "Read", "hooks": {"custom": true}});
    let user_group = json!({"matcher": "Bash", "hooks": [{"command": "echo user"}]});
    for group in [&opaque_group, &user_group] {
        append_hook_entry(&mut root, PRE_TOOL_USE_KEY, group.clone()).unwrap();
    }
    assert!(remove_hook_entries(
        &mut root,
        PRE_TOOL_USE_KEY,
        HookEntries::Grouped,
        is_claude_hook_entry,
    ));
    assert_eq!(
        root,
        json!({"hooks": {"PreToolUse": [opaque_group, user_group]}})
    );
    assert!(!remove_hook_entries(
        &mut root,
        PRE_TOOL_USE_KEY,
        HookEntries::Grouped,
        is_claude_hook_entry,
    ));
}

#[test]
fn filter_templates_preview_report_path_errors_and_preserve_existing_files() {
    let scratch = test_isolation::tempdir();
    let _working_dir = test_isolation::enter(scratch.path());
    test_isolation::with_root(scratch.path(), || {
        let project = scratch.path().join(".rtk");
        let global = user_dirs::config().unwrap();
        assert!(global.starts_with(scratch.path()));
        let preview = InitContext {
            verbose: 1,
            dry_run: true,
            ..InitContext::default()
        };
        let apply = InitContext {
            dry_run: false,
            ..preview
        };

        for (directory, generate, template, is_global) in [
            (
                project,
                generate_project_filters_template as fn(InitContext) -> Result<()>,
                FILTERS_TEMPLATE,
                false,
            ),
            (
                global,
                generate_global_filters_template,
                FILTERS_GLOBAL_TEMPLATE,
                true,
            ),
        ] {
            let path = directory.join("filters.toml");
            generate(preview).unwrap();
            assert!(!path.exists(), "preview must not create a template");

            fs::create_dir_all(&path).unwrap();
            let candidates = crate::hooks::trust::gated_filter_paths();
            assert_eq!(candidates.get(usize::from(is_global)), Some(&path));
            finalize_filter_trust(is_global, false, FilterTrust::Trust).unwrap();
            assert!(path.is_dir(), "a directory is not a readable filter file");
            fs::remove_dir(&path).unwrap();
            fs::remove_dir(&directory).unwrap();

            fs::write(&directory, "parent is a file").unwrap();
            let error = generate(apply).unwrap_err();
            assert_eq!(
                error.to_string(),
                format!("Failed to create directory: {}", directory.display())
            );
            assert_eq!(fs::read_to_string(&directory).unwrap(), "parent is a file");
            fs::remove_file(&directory).unwrap();

            #[cfg(unix)]
            {
                fs::create_dir(&directory).unwrap();
                let destination = scratch.path().join("missing-parent").join("filters.toml");
                std::os::unix::fs::symlink(&destination, &path).unwrap();
                let error = generate(apply).unwrap_err();
                assert_eq!(
                    error.to_string(),
                    format!("Failed to write {}", path.display())
                );
                assert_eq!(fs::read_link(&path).unwrap(), destination);
                assert!(!destination.exists());
                fs::remove_file(&path).unwrap();
            }

            generate(apply).unwrap();
            assert_eq!(fs::read_to_string(&path).unwrap(), template);

            let custom = "schema_version = 1\n# my filters\n";
            fs::write(&path, custom).unwrap();
            generate(apply).unwrap();
            assert_eq!(fs::read_to_string(&path).unwrap(), custom);
        }
    });
}

#[test]
fn claude_uninstall_previews_then_removes_only_managed_artifacts() {
    let scratch = test_isolation::tempdir();
    let _working_dir = test_isolation::enter(scratch.path());
    with_claude_dir_override(&scratch, |claude_dir| {
        let opencode = scratch.path().join("opencode");
        crate::core::user_env::with_path("OPENCODE_CONFIG_DIR", Some(&opencode), || {
            assert!(resolve_opencode_dir().unwrap().starts_with(scratch.path()));
            assert!(resolve_cursor_dir().unwrap().starts_with(scratch.path()));
            let plugin = run_opencode_only_mode(InitContext::default())
                .unwrap()
                .expect("OpenCode plugin installed");
            assert!(plugin.is_file());

            let md = claude_dir.join(CLAUDE_MD);
            let original_md = format!("# User rules\n{RTK_MD_REF}\n");
            fs::write(&md, &original_md).unwrap();

            let settings = claude_dir.join(SETTINGS_JSON);
            let user_env = json!({"KEEP_ME": "yes"});
            let mut original_settings = json!({"env": user_env});
            insert_hook_entry(
                &mut original_settings,
                PRE_TOOL_USE_KEY,
                "Bash",
                CLAUDE_HOOK_COMMAND,
            )
            .unwrap();
            #[cfg(feature = "engine")]
            {
                original_settings["env"]["ANTHROPIC_BASE_URL"] =
                    json!(format!("http://{}", crate::llm_proxy::DEFAULT_BIND));
            }
            fs::write(&settings, original_settings.to_string()).unwrap();

            let uninstall_claude =
                |ctx| uninstall(true, false, false, false, false, false, ctx).unwrap();
            assert_eq!(
                uninstall_claude(InitContext {
                    dry_run: true,
                    verbose: 1,
                    ..InitContext::default()
                }),
                0
            );
            assert_eq!(fs::read_to_string(&md).unwrap(), original_md);
            assert_eq!(read_json_file(&settings).unwrap(), Some(original_settings));
            assert!(plugin.is_file());

            assert_eq!(uninstall_claude(InitContext::default()), 0);
            assert_eq!(fs::read_to_string(&md).unwrap().trim_end(), "# User rules");
            let cleaned = read_json_file(&settings).unwrap().unwrap();
            assert_eq!(cleaned["env"], user_env);
            assert!(!hook_already_present(
                &cleaned,
                PRE_TOOL_USE_KEY,
                CLAUDE_HOOK_COMMAND,
            ));
            assert!(!plugin.exists());

            fs::write(&md, format!("{RTK_MD_REF}\n")).unwrap();
            assert_eq!(uninstall_claude(InitContext::default()), 0);
            assert!(!md.exists(), "a managed-only CLAUDE.md should be removed");
        });
    });
}

#[cfg(windows)]
#[test]
fn claude_uninstall_reports_when_a_readable_document_cannot_be_changed() {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_SHARE_READ: u32 = 1;
    let scratch = test_isolation::tempdir();
    with_claude_dir_override(&scratch, |claude_dir| {
        let md = claude_dir.join(CLAUDE_MD);
        for (content, context) in [
            (
                format!("{RTK_MD_REF}\n"),
                "Failed to remove empty CLAUDE.md",
            ),
            (
                format!("Keep this\n{RTK_MD_REF}\n"),
                "Failed to write CLAUDE.md",
            ),
        ] {
            fs::write(&md, &content).unwrap();
            let reader = fs::OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ)
                .open(&md)
                .unwrap();
            let error = uninstall(
                true,
                false,
                false,
                false,
                false,
                false,
                InitContext::default(),
            )
            .unwrap_err();
            drop(reader);
            assert_eq!(error.to_string(), format!("{context}: {}", md.display()));
            assert_eq!(fs::read_to_string(&md).unwrap(), content);
        }
    });
}

#[test]
#[should_panic(expected = "fixture must exclude shell builtins from the prefix rule")]
fn prefix_rule_check_rejects_a_missing_builtin_exclusion() {
    tests::assert_prefix_rule_spares_builtins(
        "fixture",
        "Prefix external programs with `rtk`.\n\n",
    );
}

#[test]
#[should_panic(expected = "fixture: the exclusion paragraph must end with a blank line")]
fn prefix_rule_check_rejects_an_unterminated_exclusion_paragraph() {
    tests::assert_prefix_rule_spares_builtins(
        "fixture",
        "Prefix external programs with `rtk`.\n\nDo not prefix shell builtins or keywords.\n",
    );
}
