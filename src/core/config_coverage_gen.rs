use super::*;
use crate::core::{retriever::RecoveryMode, test_isolation};
use std::fs;

#[test]
fn save_round_trips_display_settings_from_an_older_config() {
    let root = test_isolation::tempdir();
    test_isolation::with_root(root.path(), || {
        let path = get_config_path().unwrap();
        assert!(!path.exists());

        let config =
            Config::from_toml("[display]\ncolors = false\nemoji = false\nmax_width = 73\n")
                .unwrap();
        assert!(config.display.pro_hint, "older display tables get the hint");

        config.save().unwrap();
        let saved: toml::Value = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["display"]["pro_hint"].as_bool(), Some(true));

        let display = Config::load().unwrap().display;
        assert!(!display.colors);
        assert!(!display.emoji);
        assert_eq!(display.max_width, 73);
    });
}

#[test]
fn set_recall_mode_creates_and_migrates_config_but_preserves_invalid_toml() {
    let root = test_isolation::tempdir();
    test_isolation::with_root(root.path(), || {
        let path = get_config_path().unwrap();
        assert!(!path.exists());
        assert_eq!(set_recall_mode(RecoveryMode::Disabled).unwrap(), path);
        assert_eq!(
            Config::load().unwrap().retriever.mode,
            RecoveryMode::Disabled
        );

        let legacy = "\
# Keep this comment
[retriever]
tee_max_files = 5

[tee]
mode = \"always\"
max_files = 7
max_file_size = 4096
";
        fs::write(&path, legacy).unwrap();
        assert_eq!(set_recall_mode(RecoveryMode::Tee).unwrap(), path);
        let updated = fs::read_to_string(&path).unwrap();
        assert!(updated.contains("# Keep this comment"));
        let document: toml::Value = toml::from_str(&updated).unwrap();
        assert!(document.get("tee").is_none());

        let retriever = Config::load().unwrap().retriever;
        assert_eq!(retriever.mode, RecoveryMode::Tee);
        assert_eq!(retriever.tee_max_files, 5);
        assert_eq!(retriever.tee_max_file_size, 4096);
        assert!(retriever.tee_on_success);

        let invalid = "[retriever\nmode = \"tee\"\n";
        fs::write(&path, invalid).unwrap();
        let error = set_recall_mode(RecoveryMode::Sqlite).unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("config.toml is not valid TOML:")
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), invalid);
    });
}

#[cfg(target_os = "linux")]
#[test]
fn create_default_propagates_a_file_open_error() {
    let root = test_isolation::tempdir();
    let path = test_isolation::with_root(root.path(), || get_config_path().unwrap());

    // Leave room for the parent path and its terminating NUL, but not the
    // filename. This makes open fail even when the test runs as root.
    let mut remaining = libc::PATH_MAX as usize - 1 - path.parent().unwrap().as_os_str().len();
    let mut nested_root = root.path().to_path_buf();
    while remaining > 1 {
        let component_len = (remaining - 1).min(100);
        nested_root.push("d".repeat(component_len));
        remaining -= component_len + 1;
    }
    fs::create_dir_all(&nested_root).unwrap();

    test_isolation::with_root(&nested_root, || {
        let path = get_config_path().unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();

        let error = Config::create_default().unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<std::io::Error>()
                .and_then(std::io::Error::raw_os_error),
            Some(libc::ENAMETOOLONG)
        );
    });
}

#[test]
fn show_commands_leave_config_unchanged_and_handle_parse_errors() {
    let root = test_isolation::tempdir();
    test_isolation::with_root(root.path(), || {
        let path = get_config_path().unwrap();
        show_config().unwrap();
        user_env::with_vars(&[("RTK_RECALL", None), ("RTK_TEE", None)], show_recall_mode).unwrap();
        assert!(!path.exists(), "showing defaults must not create a config");

        assert_eq!(Config::create_default().unwrap(), path);
        for (content, tee_override) in [
            ("[tee]\nenabled = true\n", Some("0")),
            ("[retriever]\nmode = \"disabled\"\n", None),
        ] {
            fs::write(&path, content).unwrap();
            show_config().unwrap();
            user_env::with_vars(
                &[("RTK_RECALL", None), ("RTK_TEE", tee_override)],
                show_recall_mode,
            )
            .unwrap();
            assert_eq!(fs::read_to_string(&path).unwrap(), content);
        }

        let invalid = "[display\n";
        fs::write(&path, invalid).unwrap();
        assert!(show_config().unwrap_err().is::<toml::de::Error>());
        user_env::with_vars(&[("RTK_RECALL", None), ("RTK_TEE", None)], show_recall_mode).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), invalid);
    });
}

#[cfg(unix)]
#[test]
fn config_cli_reports_defaults_and_explains_legacy_recall() {
    if !test_isolation::rtk_binary_is_built() {
        return;
    }

    let root = test_isolation::tempdir();
    let show = |args: &[&str], recall_override: &str| {
        let output = test_isolation::rtk_command()
            .current_dir(root.path())
            .env("HOME", root.path())
            .env("XDG_CONFIG_HOME", root.path().join("xdg"))
            .env("RTK_RECALL", recall_override)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "rtk {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };

    let defaults = show(&["config"], "1");
    assert!(defaults.contains("(default config, file not created)"));
    let path = std::path::Path::new(
        defaults
            .lines()
            .find_map(|line| line.strip_prefix("Config: "))
            .expect("config path in stdout"),
    );
    assert!(path.starts_with(root.path()));
    assert!(!path.exists());

    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        "[display]\ncolors = false\nemoji = false\nmax_width = 73\n\n[tee]\nenabled = true\n",
    )
    .unwrap();
    let configured = show(&["config"], "1");
    assert!(!configured.contains("(default config, file not created)"));
    assert!(configured.lines().any(|line| line == "max_width = 73"));

    let recall = show(&["config", "recall"], "0");
    for expected in [
        "recall mode: tee",
        "source: legacy [tee] section (auto-migrated at load)",
        "note: RTK_RECALL=0/RTK_TEE=0 is set — recovery disabled for this environment",
        "change with: rtk config recall <sqlite|tee|disabled>",
    ] {
        assert!(recall.lines().any(|line| line == expected), "{recall}");
    }
}
