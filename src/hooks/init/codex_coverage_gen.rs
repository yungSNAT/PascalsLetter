use super::*;
use crate::core::{test_isolation, user_env};

#[test]
fn link_site_normalizes_paths_without_a_final_file_name() {
    let path = Path::new(".").join("..").join("unused").join("..");
    assert_eq!(link_site(&path), Path::new(".."));
}

#[cfg(unix)]
#[test]
fn hook_destination_requires_a_complete_component_walk() {
    use std::os::unix::fs::symlink;

    let temp = test_isolation::tempdir();
    let root = fs::canonicalize(temp.path()).unwrap();
    let via = root.join("via");
    let hook = Path::new(CODEX_DIR).join(HOOKS_JSON);
    let site = root.join(&hook);
    symlink("via", &via).unwrap();
    symlink(via.join("nested"), root.join(CODEX_DIR)).unwrap();

    let error = ensure_inside_root(&root, &hook).unwrap_err().to_string();
    assert!(
        error.contains("symlink RTK cannot follow to an end"),
        "{error}"
    );
    assert!(error.contains(&hook.display().to_string()), "{error}");

    fs::remove_file(&via).unwrap();
    let nested = root.join("real").join("nested");
    fs::create_dir_all(&nested).unwrap();
    symlink("real", &via).unwrap();
    let destination = nested.join(HOOKS_JSON);

    assert!(matches!(
        resolve_symlink_components_within(&site, 1),
        Resolution::Unresolved(path) if path == destination
    ));
    assert!(matches!(
        resolve_symlink_components_within(&site, 2),
        Resolution::Fully(path) if path == destination
    ));
    ensure_inside_root(&root, &hook).unwrap();
    assert!(
        !destination.exists(),
        "the containment check must not write"
    );
}

#[cfg(unix)]
#[test]
fn moving_foreign_rtk_md_skips_an_occupied_directory_and_reports_rename_failure() {
    use std::os::unix::fs::symlink;

    let temp = test_isolation::tempdir();
    let source = temp.path().join(RTK_MD);
    let occupied = temp.path().join("RTK.md.bak");
    let next = temp.path().join("RTK.md.bak.1");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("notes"), "keep these notes").unwrap();
    fs::create_dir(&occupied).unwrap();
    fs::write(occupied.join("notes"), "keep this older backup").unwrap();

    // The probe sees a missing target, but a directory cannot be renamed over the link.
    let missing = temp.path().join("missing");
    symlink(&missing, &next).unwrap();
    let move_aside =
        || back_up_foreign_rtk_md(&source, RtkMdScope::ProjectRoot, InitContext::default());
    assert_eq!(
        move_aside().unwrap_err().to_string(),
        format!(
            "Failed to move {} aside to {}",
            source.display(),
            next.display()
        )
    );
    assert_eq!(fs::read_link(&next).unwrap(), missing);
    assert_eq!(
        fs::read_to_string(source.join("notes")).unwrap(),
        "keep these notes"
    );

    fs::remove_file(&next).unwrap();
    assert_eq!(move_aside().unwrap(), Some(next.clone()));
    assert!(!source.exists());
    assert_eq!(
        fs::read_to_string(next.join("notes")).unwrap(),
        "keep these notes"
    );
    assert_eq!(
        fs::read_to_string(occupied.join("notes")).unwrap(),
        "keep this older backup"
    );
}

#[cfg(unix)]
#[test]
fn codex_install_reports_an_uncreatable_config_directory() {
    use std::os::unix::fs::symlink;

    let temp = test_isolation::tempdir();
    let config = temp.path().join(CODEX_DIR);
    let missing = temp.path().join("missing");
    symlink(&missing, &config).unwrap();
    let hooks = config.join(HOOKS_JSON);
    let expected = format!(
        "Failed to create Codex config directory: {}",
        config.display()
    );

    let install_error = run_codex_mode_with_paths(
        config.join(AGENTS_MD),
        config.join(RTK_MD),
        hooks.clone(),
        true,
        InitContext::default(),
    )
    .unwrap_err();
    assert_eq!(install_error.to_string(), expected);

    let patch_error = patch_codex_hooks_json(&hooks, InitContext::default()).unwrap_err();
    assert_eq!(patch_error.to_string(), expected);
    assert_eq!(fs::read_link(&config).unwrap(), missing);
    assert!(!missing.exists());
}

#[cfg(target_os = "linux")]
#[test]
fn codex_reinstall_is_idempotent_even_when_tracking_guidance_cannot_be_rendered() {
    use std::os::unix::ffi::OsStrExt;

    let temp = test_isolation::tempdir();
    let user_root = temp.path().join(std::ffi::OsStr::from_bytes(b"user-\xff"));
    fs::create_dir(&user_root).unwrap();
    let config = temp.path().join(CODEX_DIR);
    let agents = config.join(AGENTS_MD);
    let rtk = config.join(RTK_MD);
    let hooks = config.join(HOOKS_JSON);

    test_isolation::with_root(&user_root, || {
        user_env::with_path("RTK_DB_PATH", Some(&user_root.join("history.db")), || {
            assert!(
                crate::core::tracking::get_db_path()
                    .and_then(|path| codex_tracking_config(&path))
                    .is_err(),
                "the fixture must make tracking guidance unavailable"
            );

            let install = || {
                run_codex_mode_with_paths(
                    agents.clone(),
                    rtk.clone(),
                    hooks.clone(),
                    true,
                    InitContext::default(),
                )
                .unwrap();
            };
            install();
            let original_hooks = fs::read_to_string(&hooks).unwrap();
            let parsed: serde_json::Value = serde_json::from_str(&original_hooks).unwrap();
            assert!(codex_hook_already_present(&parsed));

            install();
            assert_eq!(fs::read_to_string(&hooks).unwrap(), original_hooks);
            assert_eq!(
                fs::read_to_string(&agents).unwrap(),
                format!("@{}\n", rtk.display())
            );
            assert_eq!(
                fs::read_to_string(&rtk).unwrap(),
                codex_rtk_md_content(AwarenessLevel::Default)
            );
            assert!(!backup_path_for(&hooks).exists());
        });
    });
}

#[test]
fn status_and_uninstall_report_unreadable_codex_files_in_context() {
    let temp = test_isolation::tempdir();
    let global = temp.path().join("global");
    let project = temp.path().join("project");
    fs::create_dir(&global).unwrap();
    fs::create_dir(&project).unwrap();
    let global_hooks = global.join(HOOKS_JSON);
    let global_agents = global.join(AGENTS_MD);
    let local_agents = project.join(AGENTS_MD);

    test_isolation::with_root(temp.path(), || {
        let _entered = test_isolation::enter(&project);
        user_env::with_path("CODEX_HOME", Some(&global), || {
            fs::create_dir(&global_hooks).unwrap();
            let error = show_codex_config().unwrap_err();
            assert!(
                error.downcast_ref::<std::io::Error>().is_some(),
                "{error:#}"
            );
            fs::remove_dir(&global_hooks).unwrap();

            fs::write(&global_agents, [0xff]).unwrap();
            assert_eq!(
                show_codex_config().unwrap_err().to_string(),
                format!(
                    "Failed to read global Codex instructions: {}",
                    global_agents.display()
                )
            );

            fs::write(&global_agents, "# Global team instructions\n").unwrap();
            fs::write(&local_agents, [0xff]).unwrap();
            assert_eq!(
                show_codex_config().unwrap_err().to_string(),
                format!(
                    "Failed to read local Codex instructions: {}",
                    local_agents.display()
                )
            );
            assert_eq!(
                uninstall_codex(false, InitContext::default())
                    .unwrap_err()
                    .to_string(),
                format!("Failed to read AGENTS.md: {}", local_agents.display())
            );
            assert_eq!(fs::read(&local_agents).unwrap(), [0xff]);
        });
    });
}

#[cfg(unix)]
#[test]
fn codex_status_distinguishes_references_legacy_blocks_and_user_notes() {
    if !test_isolation::rtk_binary_or_skip() {
        return;
    }

    let temp = test_isolation::tempdir();
    let global = temp.path().join("global");
    let project = temp.path().join("project");
    fs::create_dir(&global).unwrap();
    fs::create_dir(&project).unwrap();
    fs::write(global.join(RTK_MD), "Installed global instructions\n").unwrap();
    let absolute_ref = format!("@{}\n", global.join(RTK_MD).display());
    let legacy_block = format!("{RTK_BLOCK_START} v2 -->\nOld instructions\n{RTK_BLOCK_END}\n");

    for (global_content, local_content, owned, expected_agents) in [
        (
            absolute_ref.as_str(),
            RTK_MD_REF,
            true,
            [
                "[ok] Global AGENTS.md: RTK.md reference",
                "[ok] Local AGENTS.md: @RTK.md reference",
            ],
        ),
        (
            legacy_block.as_str(),
            legacy_block.as_str(),
            false,
            [
                "[!!] Global AGENTS.md: old inline RTK block",
                "[!!] Local AGENTS.md: old inline RTK block",
            ],
        ),
        (
            "# Global team rules\n",
            "# Project team rules\n",
            false,
            [
                "[--] Global AGENTS.md: exists but rtk not configured",
                "[--] Local AGENTS.md: exists but rtk not configured",
            ],
        ),
    ] {
        fs::write(global.join(AGENTS_MD), global_content).unwrap();
        fs::write(project.join(AGENTS_MD), format!("{local_content}\n")).unwrap();
        let local_rtk = if owned {
            codex_rtk_md_content(AwarenessLevel::Default)
        } else {
            "# Notes I wrote myself\n".to_string()
        };
        fs::write(project.join(RTK_MD), local_rtk).unwrap();

        let output = test_isolation::rtk_command()
            .current_dir(&project)
            .env("CODEX_HOME", &global)
            .args(["init", "--codex", "--show"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(stdout.contains("[ok] Global RTK.md:"), "{stdout}");
        for expected in expected_agents {
            assert!(stdout.lines().any(|line| line == expected), "{stdout}");
        }
        if owned {
            assert!(stdout.contains("[ok] Local RTK.md:"), "{stdout}");
        } else {
            assert!(
                stdout.lines().any(|line| {
                    line.starts_with("[--] Local RTK.md:")
                        && line.ends_with("exists but RTK did not write it")
                }),
                "{stdout}"
            );
        }
    }
}

#[test]
fn uninstall_previews_owned_instructions_and_leaves_unrelated_hooks_untouched() {
    let temp = test_isolation::tempdir();
    let rtk = temp.path().join(RTK_MD);
    let hooks = temp.path().join(HOOKS_JSON);
    let content = codex_rtk_md_content(AwarenessLevel::Default);
    let unrelated = r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo done"}]}]}}"#;
    fs::write(&rtk, &content).unwrap();
    fs::write(&hooks, unrelated).unwrap();

    let expected = vec![format!("RTK.md: {}", rtk.display())];
    let preview = uninstall_codex_at(
        temp.path(),
        InitContext {
            dry_run: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(preview, expected);
    assert_eq!(fs::read_to_string(&rtk).unwrap(), content);

    let removed = uninstall_codex_at(
        temp.path(),
        InitContext {
            verbose: 1,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(removed, expected);
    assert!(!rtk.exists());
    assert_eq!(fs::read_to_string(&hooks).unwrap(), unrelated);
    assert!(!backup_path_for(&hooks).exists());

    fs::create_dir(&rtk).unwrap();
    fs::write(rtk.join("notes"), "not an RTK file").unwrap();
    assert_eq!(
        uninstall_codex_at(temp.path(), InitContext::default())
            .unwrap_err()
            .to_string(),
        format!("Failed to remove RTK.md: {}", rtk.display())
    );
    assert_eq!(
        fs::read_to_string(rtk.join("notes")).unwrap(),
        "not an RTK file"
    );
}

#[cfg(unix)]
#[test]
fn uninstall_reports_a_failed_legacy_block_rewrite_without_losing_instructions() {
    use std::os::unix::fs::PermissionsExt;

    let temp = test_isolation::tempdir();
    let agents = temp.path().join(AGENTS_MD);
    let original =
        format!("# Team rules\n\n{RTK_BLOCK_START} v2 -->\nOld instructions\n{RTK_BLOCK_END}\n");
    fs::write(&agents, &original).unwrap();
    let permissions = fs::metadata(temp.path()).unwrap().permissions();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o500)).unwrap();

    // Privileged test users can still write here, so verify normal cleanup in that case.
    let probe = fs::write(temp.path().join("write-probe"), "probe");
    let result = uninstall_codex_at(temp.path(), InitContext::default());
    fs::set_permissions(temp.path(), permissions).unwrap();

    match probe {
        Ok(()) => {
            assert_eq!(
                result.unwrap(),
                vec!["AGENTS.md: removed rtk-instructions block".to_string()]
            );
            assert_eq!(
                fs::read_to_string(&agents).unwrap().trim_end(),
                "# Team rules"
            );
        }
        Err(error) => {
            assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
            assert_eq!(
                result.unwrap_err().to_string(),
                format!("Failed to write AGENTS.md: {}", agents.display())
            );
            assert_eq!(fs::read_to_string(&agents).unwrap(), original);
        }
    }
}
