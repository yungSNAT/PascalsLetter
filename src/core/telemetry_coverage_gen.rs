use super::*;
use crate::core::{test_isolation, user_env};
use std::fs;
use std::path::Path;

fn in_sandbox(run: impl FnOnce(&Path)) {
    let root = test_isolation::tempdir();
    let _working_dir = test_isolation::enter(root.path());
    test_isolation::with_root(root.path(), || run(root.path()));
}

fn write_fixture(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn config_file() -> PathBuf {
    user_dirs::config()
        .unwrap()
        .join(crate::core::constants::CONFIG_TOML)
}

#[test]
fn marker_is_created_and_cleared_but_not_touched_without_a_collector() {
    in_sandbox(|_| {
        let marker = telemetry_marker_path();

        // Only exercise the ping entry points when this build cannot send a request.
        if endpoint_url().is_none() {
            write_fixture(
                &config_file(),
                "[telemetry]\nenabled = true\nconsent_given = true\n",
            );
            let cfg = config::Config::load().unwrap();
            assert!(cfg.telemetry.enabled);
            assert_eq!(cfg.telemetry.consent_given, Some(true));

            maybe_ping();
            assert!(!marker.exists(), "no ping should have been recorded");
            assert_eq!(send_ping().unwrap_err().to_string(), "no telemetry URL");
        }

        touch_marker(&marker);
        assert!(fs::read(&marker).unwrap().is_empty());

        fs::write(&marker, b"old contents").unwrap();
        touch_marker(&marker);
        assert!(fs::read(&marker).unwrap().is_empty());
    });
}

#[test]
fn recall_mode_label_follows_the_saved_configuration() {
    in_sandbox(|_| {
        let path = config_file();
        for mode in ["sqlite", "tee", "disabled"] {
            write_fixture(&path, &format!("[retriever]\nmode = \"{mode}\"\n"));
            assert_eq!(recall_mode_label(), mode);
        }
    });
}

#[test]
fn detects_hook_precedence_and_counts_toml_filters_in_both_scopes() {
    in_sandbox(|root| {
        let home = user_dirs::home().unwrap();
        let project = user_dirs::working_dir().unwrap();
        let claude_config = root.join("claude-user-settings");
        fs::create_dir(&claude_config).unwrap();

        user_env::with_path("CLAUDE_CONFIG_DIR", Some(&claude_config), || {
            assert_eq!(detect_hook_type(), "none");

            let copilot = project
                .join(".github")
                .join("hooks")
                .join("rtk-rewrite.json");
            write_fixture(&copilot, "{}\n");
            assert_eq!(detect_hook_type(), "copilot");

            let claude = project.join(".claude").join("hooks").join("rtk-rewrite.sh");
            write_fixture(&claude, "");
            assert_eq!(detect_hook_type(), "claude");

            let gemini = home.join(".gemini").join("hooks").join("rtk-hook.sh");
            write_fixture(&gemini, "");
            assert_eq!(detect_hook_type(), "gemini");
        });

        let local_filters = project.join(".rtk").join("filters");
        let global_filters = user_dirs::config().unwrap().join("filters");
        assert_eq!(count_custom_toml_filters(), 0);

        write_fixture(&local_filters.join("project.toml"), "");
        write_fixture(&local_filters.join("README.md"), "");
        assert_eq!(count_custom_toml_filters(), 1);

        write_fixture(&global_filters.join("first.toml"), "");
        write_fixture(&global_filters.join("second.toml"), "");
        write_fixture(&global_filters.join("disabled.toml.bak"), "");
        assert_eq!(count_custom_toml_filters(), 3);
    });
}

#[test]
fn meta_usage_counts_invocations_and_omits_unused_commands() {
    in_sandbox(|root| {
        let db = root.join("telemetry-test.db");
        user_env::with_path("RTK_DB_PATH", Some(&db), || {
            let tracker = tracking::Tracker::new().unwrap();
            assert_eq!(build_meta_usage(&tracker), serde_json::json!({}));

            for (original, filtered) in [
                ("rtk gain", "rtk gain"),
                ("rtk verify", "rtk verify"),
                ("rtk gain", "rtk gain"),
                ("git status", "rtk git status"),
            ] {
                tracker.record(original, filtered, 100, 40, 1).unwrap();
            }

            assert_eq!(
                build_meta_usage(&tracker),
                serde_json::json!({"gain": 2, "verify": 1})
            );
        });
    });
}
