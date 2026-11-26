use super::*;
use crate::core::test_isolation;

fn verbose_context(dry_run: bool) -> InitContext {
    InitContext {
        dry_run,
        verbose: 1,
        ..InitContext::default()
    }
}

#[test]
fn global_vibe_lifecycle_leaves_artifacts_untouched_in_dry_run() {
    let root = test_isolation::tempdir();
    test_isolation::with_root(root.path(), || {
        let vibe_dir = resolve_vibe_dir().unwrap();
        let hooks_path = vibe_dir.join(VIBE_HOOKS_FILE);
        let prompt_path = vibe_dir.join(VIBE_PROMPTS_SUBDIR).join(VIBE_PROMPT_FILE);

        let error =
            run_vibe_mode(false, false, PatchMode::Auto, verbose_context(false)).unwrap_err();
        assert!(error.to_string().contains("Vibe support is global-only"));
        assert!(!vibe_dir.exists());

        run_vibe_mode(true, false, PatchMode::Ask, verbose_context(true)).unwrap();
        assert!(!vibe_dir.exists());
        assert!(!is_configured());

        run_vibe_mode(true, false, PatchMode::Auto, verbose_context(false)).unwrap();
        assert!(is_configured());
        let hooks = fs::read_to_string(&hooks_path).unwrap();
        let prompt = fs::read_to_string(&prompt_path).unwrap();
        assert_eq!(hooks, vibe_hook_entry());
        assert_eq!(prompt, awareness_content(InitContext::default().awareness));

        uninstall_vibe(verbose_context(true)).unwrap();
        assert!(is_configured());
        assert_eq!(fs::read_to_string(&hooks_path).unwrap(), hooks);
        assert_eq!(fs::read_to_string(&prompt_path).unwrap(), prompt);

        uninstall_vibe(verbose_context(false)).unwrap();
        assert!(!is_configured());
        assert!(!hooks_path.exists());
        assert!(!prompt_path.exists());

        uninstall_vibe(verbose_context(false)).unwrap();
        assert!(fs::read_dir(&vibe_dir).unwrap().all(|entry| {
            entry
                .map(|entry| entry.file_name() == VIBE_PROMPTS_SUBDIR)
                .unwrap_or(false)
        }));
    });
}

#[test]
fn registry_patch_preserves_user_text_and_uninstall_keeps_other_sections() {
    let root = test_isolation::tempdir();
    let hooks_path = root.path().join(VIBE_HOOKS_FILE);
    let user_hook = "[[hooks]]\nname = \"user-audit\"\ncommand = \"audit.py\"";
    let user_text = format!("# Keep this comment\n\n\n\n{user_hook}");
    let entry = vibe_hook_entry();
    let ctx = verbose_context(false);

    for (trailing, separator) in [("", "\n\n"), ("\n\n", "")] {
        let original = format!("{user_text}{trailing}");
        fs::write(&hooks_path, &original).unwrap();

        let skipped = patch_vibe_hooks_toml(&hooks_path, PatchMode::Skip, ctx).unwrap();
        assert_eq!(skipped, VibeHookPatchOutcome::Skipped);
        assert_eq!(skipped.summary_verb(), None);
        assert_eq!(fs::read_to_string(&hooks_path).unwrap(), original);

        let expected = format!("{original}{separator}{entry}");
        let installed = patch_vibe_hooks_toml(&hooks_path, PatchMode::Auto, ctx).unwrap();
        assert_eq!(installed, VibeHookPatchOutcome::Installed);
        assert_eq!(fs::read_to_string(&hooks_path).unwrap(), expected);

        let present = patch_vibe_hooks_toml(&hooks_path, PatchMode::Auto, ctx).unwrap();
        assert_eq!(present, VibeHookPatchOutcome::AlreadyPresent);
        assert_eq!(fs::read_to_string(&hooks_path).unwrap(), expected);

        fs::write(
            &hooks_path,
            format!("{expected}\n[settings]\nenabled = true"),
        )
        .unwrap();
        let removed = uninstall_vibe_at(root.path(), ctx).unwrap();
        assert_eq!(
            removed,
            vec![format!(
                "Vibe hooks.toml: removed RTK entry ({})",
                hooks_path.display()
            )]
        );
        assert_eq!(
            fs::read_to_string(&hooks_path).unwrap(),
            format!("# Keep this comment\n\n{user_hook}\n\n[settings]\nenabled = true\n")
        );
    }
}

#[test]
fn vite_install_reports_blocked_config_registry_and_prompt_paths() {
    let root = test_isolation::tempdir();
    let blocker = root.path().join("not-a-directory");
    fs::write(&blocker, "keep this file").unwrap();
    let ctx = InitContext::default();

    let blocked_vibe_dir = blocker.join(VIBE_DIR);
    let error = run_vibe_mode_at(&blocked_vibe_dir, true, PatchMode::Auto, ctx).unwrap_err();
    assert_eq!(
        error.to_string(),
        format!(
            "Failed to create Vibe config dir: {}",
            blocked_vibe_dir.display()
        )
    );

    let blocked_hooks_path = blocked_vibe_dir.join(VIBE_HOOKS_FILE);
    let error = patch_vibe_hooks_toml(&blocked_hooks_path, PatchMode::Auto, ctx).unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("Failed to write {}", blocked_hooks_path.display())
    );
    assert_eq!(fs::read_to_string(&blocker).unwrap(), "keep this file");

    let vibe_dir = root.path().join(VIBE_DIR);
    fs::create_dir_all(&vibe_dir).unwrap();
    let prompts_dir = vibe_dir.join(VIBE_PROMPTS_SUBDIR);
    fs::write(&prompts_dir, "user-owned file").unwrap();
    let error = run_vibe_mode_at(&vibe_dir, false, PatchMode::Auto, ctx).unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("Failed to create prompts dir: {}", prompts_dir.display())
    );
    assert_eq!(fs::read_to_string(&prompts_dir).unwrap(), "user-owned file");
    assert_eq!(
        fs::read_to_string(vibe_dir.join(VIBE_HOOKS_FILE)).unwrap(),
        vibe_hook_entry()
    );
}

#[test]
fn vibe_io_errors_leave_unreadable_registry_and_blocked_prompt_in_place() {
    let root = test_isolation::tempdir();
    let hooks_path = root.path().join(VIBE_HOOKS_FILE);
    let invalid_utf8 = [0xff, 0xfe];
    fs::write(&hooks_path, invalid_utf8).unwrap();
    let ctx = InitContext::default();
    let expected = format!("Failed to read {}", hooks_path.display());

    let error = patch_vibe_hooks_toml(&hooks_path, PatchMode::Auto, ctx).unwrap_err();
    assert_eq!(error.to_string(), expected);
    let error = uninstall_vibe_at(root.path(), ctx).unwrap_err();
    assert_eq!(error.to_string(), expected);
    assert_eq!(fs::read(&hooks_path).unwrap(), invalid_utf8);

    let entry = vibe_hook_entry();
    fs::write(&hooks_path, &entry).unwrap();
    let prompt_path = root.path().join(VIBE_PROMPTS_SUBDIR).join(VIBE_PROMPT_FILE);
    fs::create_dir_all(&prompt_path).unwrap();

    let error = uninstall_vibe_at(root.path(), ctx).unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("Failed to remove {}", prompt_path.display())
    );
    assert!(prompt_path.is_dir());
    assert_eq!(fs::read_to_string(&hooks_path).unwrap(), entry);
}

#[cfg(unix)]
#[test]
fn vibe_cli_treats_end_of_input_as_declining_the_patch() {
    if !test_isolation::rtk_binary_or_skip() {
        return;
    }

    let root = test_isolation::tempdir();
    let output = test_isolation::rtk_command()
        .args(["init", "-g", "--agent", "vibe"])
        .env("HOME", root.path())
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("with RTK hook? [y/N]"), "{stdout}");
    assert!(
        stdout.contains("Skipped. Re-run with --auto-patch"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("Mistral Vibe CLI hook installed"),
        "{stdout}"
    );
    assert!(!root.path().join(VIBE_DIR).join(VIBE_HOOKS_FILE).exists());
}
