use super::*;
use crate::core::{test_isolation, user_env};

#[test]
fn dry_run_leaves_hermes_files_unchanged_and_reports_pending_removals() {
    let scratch = test_isolation::tempdir();
    let hermes_home = scratch.path().join("hermes");
    fs::create_dir(&hermes_home).unwrap();
    let plugin_dir = hermes_plugin_dir(&hermes_home);
    let init_path = plugin_dir.join(HERMES_PLUGIN_INIT_FILE);
    let manifest_path = plugin_dir.join(HERMES_PLUGIN_MANIFEST_FILE);
    let config_path = hermes_home.join("config.yaml");
    let original = concat!(
        "plugins:\n",
        "  # Plugin settings\n",
        "  enabled: # Loaded at startup\n",
        "    # Keep this note\n",
        "session: local\n",
    );
    let installed = concat!(
        "plugins:\n",
        "  # Plugin settings\n",
        "  enabled: # Loaded at startup\n",
        "    # Keep this note\n",
        "    - rtk-rewrite\n",
        "session: local\n",
    );
    let uninstalled = concat!(
        "plugins:\n",
        "  # Plugin settings\n",
        "  enabled: [] # Loaded at startup\n",
        "    # Keep this note\n",
        "session: local\n",
    );
    fs::write(&config_path, original).unwrap();

    let preview = InitContext {
        dry_run: true,
        verbose: 1,
        ..InitContext::default()
    };
    let apply = InitContext {
        verbose: 1,
        ..InitContext::default()
    };
    let expected_removals = vec![
        format!("Hermes plugin: {}", plugin_dir.display()),
        "Hermes config: removed RTK plugin entry".to_string(),
    ];

    test_isolation::with_root(scratch.path(), || {
        user_env::with_path("HERMES_HOME", Some(hermes_home.as_path()), || {
            run_hermes_mode(preview).unwrap();
            assert!(!plugin_dir.exists());
            assert_eq!(fs::read_to_string(&config_path).unwrap(), original);

            run_hermes_mode(apply).unwrap();
            assert_eq!(fs::read_to_string(&config_path).unwrap(), installed);
            assert!(is_configured());

            assert_eq!(
                uninstall_hermes_at(&hermes_home, preview).unwrap(),
                expected_removals
            );
            uninstall_hermes(preview).unwrap();
            assert_eq!(fs::read_to_string(&init_path).unwrap(), HERMES_PLUGIN_INIT);
            assert_eq!(
                fs::read_to_string(&manifest_path).unwrap(),
                HERMES_PLUGIN_YAML
            );
            assert_eq!(fs::read_to_string(&config_path).unwrap(), installed);

            assert_eq!(
                uninstall_hermes_at(&hermes_home, apply).unwrap(),
                expected_removals
            );
            assert!(!plugin_dir.exists());
            assert_eq!(fs::read_to_string(&config_path).unwrap(), uninstalled);
            assert!(!is_configured());

            uninstall_hermes(apply).unwrap();
            assert!(!plugin_dir.exists());
            assert_eq!(fs::read_to_string(&config_path).unwrap(), uninstalled);
        });
    });
}

#[test]
fn hermes_io_errors_identify_the_failed_operation_and_path() {
    let scratch = test_isolation::tempdir();
    let hermes_home = scratch.path();
    let plugin_dir = hermes_plugin_dir(hermes_home);
    let config_path = hermes_home.join("config.yaml");
    let ctx = InitContext::default();

    fs::create_dir_all(plugin_dir.parent().unwrap()).unwrap();
    fs::write(&plugin_dir, "not a directory").unwrap();
    assert_eq!(
        run_hermes_mode_at(hermes_home, ctx)
            .unwrap_err()
            .to_string(),
        format!(
            "Failed to create Hermes plugin directory: {}",
            plugin_dir.display()
        )
    );
    assert_eq!(
        uninstall_hermes_at(hermes_home, ctx)
            .unwrap_err()
            .to_string(),
        format!(
            "Failed to remove Hermes plugin directory: {}",
            plugin_dir.display()
        )
    );
    assert_eq!(fs::read_to_string(&plugin_dir).unwrap(), "not a directory");
    assert!(!config_path.exists());
    fs::remove_file(&plugin_dir).unwrap();

    let invalid_utf8 = b"theme: \xff\n";
    fs::write(&config_path, invalid_utf8).unwrap();
    let expected_error = format!("Failed to read Hermes config: {}", config_path.display());
    assert_eq!(
        uninstall_hermes_at(hermes_home, ctx)
            .unwrap_err()
            .to_string(),
        expected_error
    );
    assert_eq!(
        run_hermes_mode_at(hermes_home, ctx)
            .unwrap_err()
            .to_string(),
        expected_error
    );
    assert_eq!(fs::read(&config_path).unwrap(), invalid_utf8);
}

#[cfg(unix)]
#[test]
fn uninstall_preserves_config_when_atomic_replacement_is_denied() {
    use std::os::unix::fs::PermissionsExt;

    let scratch = test_isolation::tempdir();
    let hermes_home = scratch.path();
    let config_path = hermes_home.join("config.yaml");
    let installed = "plugins:\n  enabled:\n    - rtk-rewrite\n";
    fs::write(&config_path, installed).unwrap();

    let original_permissions = fs::metadata(hermes_home).unwrap().permissions();
    let mut restricted_permissions = original_permissions.clone();
    restricted_permissions.set_mode(0o500);
    fs::set_permissions(hermes_home, restricted_permissions).unwrap();

    // Privileged runners can still create the temporary replacement file.
    let can_create_tempfile = tempfile::NamedTempFile::new_in(hermes_home).is_ok();
    let result = uninstall_hermes_at(hermes_home, InitContext::default());
    fs::set_permissions(hermes_home, original_permissions).unwrap();

    if can_create_tempfile {
        assert_eq!(result.unwrap(), ["Hermes config: removed RTK plugin entry"]);
        assert_eq!(
            fs::read_to_string(&config_path).unwrap(),
            "plugins:\n  enabled: []\n"
        );
    } else {
        assert_eq!(
            result.unwrap_err().to_string(),
            format!("Failed to write Hermes config: {}", config_path.display())
        );
        assert_eq!(fs::read_to_string(&config_path).unwrap(), installed);
    }
}

#[test]
fn patch_appends_a_missing_plugins_section_without_changing_existing_settings() {
    let existing = "# plugins: example only\n\ntheme: dark";
    let terminated = format!("{existing}\n");
    let expected = concat!(
        "# plugins: example only\n",
        "\n",
        "theme: dark\n",
        "plugins:\n",
        "  enabled:\n",
        "    - rtk-rewrite\n",
    );

    assert_eq!(patch_hermes_config(existing), expected);
    assert_eq!(patch_hermes_config(&terminated), expected);
    assert_eq!(unpatch_hermes_config(existing), existing);
    assert_eq!(unpatch_hermes_config(" \t\n\n"), "");
    assert!(split_yaml_lines("").is_empty());
}

#[test]
fn patch_uses_child_indentation_and_leaves_top_level_enabled_alone() {
    let existing = concat!(
        "plugins:\n",
        "  # This comment does not determine the indentation\n",
        "\n",
        "    search_path: extensions\n",
        "enabled: [global]\n",
    );
    assert_eq!(
        patch_hermes_config(existing),
        concat!(
            "plugins:\n",
            "  # This comment does not determine the indentation\n",
            "\n",
            "    search_path: extensions\n",
            "    enabled:\n",
            "      - rtk-rewrite\n",
            "enabled: [global]\n",
        )
    );

    let lines = split_yaml_lines(existing);
    assert_eq!(
        find_yaml_key_line(&lines, "enabled", 1, Some((lines.len(), 0))),
        None
    );
}

#[test]
fn inline_enabled_round_trip_preserves_crlf_and_the_trailing_comment() {
    let existing = "plugins:\r\n  enabled: ['alpha'] # keep\r\n  search_path: extensions\r\n";
    let patched = patch_hermes_config(existing);
    assert_eq!(
        patched,
        "plugins:\r\n  enabled: ['alpha', rtk-rewrite] # keep\r\n  search_path: extensions\r\n"
    );
    assert_eq!(unpatch_hermes_config(&patched), existing);
}

#[test]
fn enabled_rewriters_handle_missing_delimiters() {
    let unclosed = "plugins:\n  enabled: [alpha, rtk-rewrite\nsession: local\n";
    assert_eq!(patch_hermes_config(unclosed), unclosed);
    assert_eq!(unpatch_hermes_config(unclosed), unclosed);

    let mut block_header = split_yaml_lines("  enabled: # keep\n");
    rewrite_inline_hermes_enabled(&mut block_header, 0, true);
    assert_eq!(block_header.concat(), "  enabled: # keep\n");

    let mut missing_colon =
        split_yaml_lines("plugins:\n  enabled\n    - rtk-rewrite\nsession: local\n");
    rewrite_block_hermes_enabled(&mut missing_colon, 1, false);
    assert_eq!(
        missing_colon.concat(),
        "plugins:\n  enabled: []\nsession: local\n"
    );
}
