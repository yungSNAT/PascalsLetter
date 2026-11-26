use super::{TELEMETRY_DISABLED_ENV, TelemetrySubcommand, run, send_erasure_request};
use crate::core::{
    config::Config, constants::HISTORY_DB, telemetry, test_isolation, user_dirs, user_env,
};
use crate::hooks::init::save_telemetry_consent;
use std::{fs, path::Path};

const TEST_SALT: &[u8] = b"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn with_telemetry_root(test: impl FnOnce()) {
    let root = test_isolation::tempdir();
    test_isolation::with_root(root.path(), || {
        let _working_dir = test_isolation::enter(root.path());
        user_env::with_vars(&[(TELEMETRY_DISABLED_ENV, None)], test);
    });
}

fn write_fixture(path: &Path, contents: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn assert_opted_out() {
    let config = Config::load().unwrap();
    assert_eq!(config.telemetry.consent_given, Some(false));
    assert!(!config.telemetry.enabled);
}

#[test]
fn status_is_read_only_and_disable_preserves_local_data() {
    with_telemetry_root(|| {
        let salt = telemetry::salt_file_path();
        run(&TelemetrySubcommand::Status).unwrap();
        assert!(!salt.exists(), "status must not create a device identity");
        assert_eq!(
            Config::load().unwrap_or_default().telemetry.consent_given,
            None
        );

        save_telemetry_consent(true).unwrap();
        write_fixture(&salt, TEST_SALT);
        user_env::with_vars(&[(TELEMETRY_DISABLED_ENV, Some("1"))], || {
            run(&TelemetrySubcommand::Status).unwrap();
        });

        let config = Config::load().unwrap();
        assert_eq!(config.telemetry.consent_given, Some(true));
        assert!(config.telemetry.enabled);
        assert!(config.telemetry.consent_date.is_some());
        assert_eq!(fs::read(&salt).unwrap(), TEST_SALT);

        let marker = telemetry::telemetry_marker_path();
        let history = user_dirs::data_under(".").join(HISTORY_DB);
        write_fixture(&marker, b"last ping");
        write_fixture(&history, b"local history");

        run(&TelemetrySubcommand::Disable).unwrap();
        run(&TelemetrySubcommand::Status).unwrap();
        assert_opted_out();
        assert_eq!(fs::read(&salt).unwrap(), TEST_SALT);
        assert_eq!(fs::read(&marker).unwrap(), b"last ping");
        assert_eq!(fs::read(&history).unwrap(), b"local history");
    });
}

#[cfg(unix)]
#[test]
fn status_reports_unasked_consent_and_the_environment_override() {
    if !test_isolation::rtk_binary_or_skip() {
        return;
    }

    let root = test_isolation::tempdir();
    let output = test_isolation::rtk_command()
        .current_dir(root.path())
        .env("HOME", root.path())
        .env("XDG_CONFIG_HOME", root.path().join("config"))
        .env("XDG_DATA_HOME", root.path().join("data"))
        .env("XDG_CACHE_HOME", root.path().join("cache"))
        .env(TELEMETRY_DISABLED_ENV, "1")
        .args(["telemetry", "status"])
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    for expected in [
        "Telemetry status:",
        "  consent:       never asked",
        "  env override:  RTK_TELEMETRY_DISABLED=1 (blocked)",
    ] {
        assert!(stdout.lines().any(|line| line == expected), "{stdout}");
    }
    // Only the label is pinned: the named controller is the upstream project's.
    assert!(
        stdout.lines().any(|line| line.starts_with("Data controller: ")),
        "{stdout}"
    );
    assert!(
        stdout.lines().any(|line| matches!(
            line,
            "  device hash:   (not applicable; this build has no telemetry endpoint)"
                | "  device hash:   (blocked by RTK_TELEMETRY_DISABLED)"
        )),
        "{stdout}"
    );
    assert!(!stdout.contains("consent date:"), "{stdout}");
}

#[test]
fn enable_rejects_nonterminal_stdin_without_recording_consent() {
    use std::io::IsTerminal;

    // Only exercise the terminal check, never the prompt or read.
    if std::io::stdin().is_terminal() {
        return;
    }

    with_telemetry_root(|| {
        let error = run(&TelemetrySubcommand::Enable).unwrap_err();
        assert_eq!(
            error.to_string(),
            "consent requires interactive terminal — cannot enable telemetry in piped mode"
        );
        assert_eq!(
            Config::load().unwrap_or_default().telemetry.consent_given,
            None
        );
    });
}

#[test]
fn forget_purges_local_data_without_a_salt_and_tolerates_cleanup_errors() {
    with_telemetry_root(|| {
        save_telemetry_consent(true).unwrap();
        let salt = telemetry::salt_file_path();
        let marker = telemetry::telemetry_marker_path();
        let history = user_dirs::data_under(".").join(HISTORY_DB);
        write_fixture(&marker, b"last ping");
        write_fixture(&history, b"local history");
        assert!(!salt.exists());

        run(&TelemetrySubcommand::Forget).unwrap();
        assert_opted_out();
        for path in [&salt, &marker, &history] {
            assert!(!path.exists(), "unexpected artifact: {}", path.display());
        }

        // Nonempty directories make remove_file fail on every supported OS.
        let marker_child = marker.join("keep");
        let history_child = history.join("keep");
        write_fixture(&marker_child, b"marker directory");
        write_fixture(&history_child, b"history directory");
        save_telemetry_consent(true).unwrap();

        run(&TelemetrySubcommand::Forget).unwrap();
        assert_opted_out();
        assert_eq!(fs::read(marker_child).unwrap(), b"marker directory");
        assert_eq!(fs::read(history_child).unwrap(), b"history directory");
        assert!(!salt.exists());
    });
}

#[test]
fn forget_requires_salt_removal_but_not_a_configured_endpoint() {
    with_telemetry_root(|| {
        // Forget with an existing salt can send a request even when opted out.
        if telemetry::endpoint_url().is_some() {
            return;
        }

        let error = send_erasure_request("test-device-hash").unwrap_err();
        assert_eq!(error.to_string(), "no telemetry endpoint configured");

        save_telemetry_consent(true).unwrap();
        let salt = telemetry::salt_file_path();
        let salt_child = salt.join("keep");
        let marker = telemetry::telemetry_marker_path();
        let history = user_dirs::data_under(".").join(HISTORY_DB);
        write_fixture(&salt_child, b"not a salt file");
        write_fixture(&marker, b"last ping");
        write_fixture(&history, b"local history");

        let error = run(&TelemetrySubcommand::Forget).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("Failed to delete {}", salt.display())
        );
        assert_opted_out();
        assert_eq!(fs::read(&salt_child).unwrap(), b"not a salt file");
        assert_eq!(fs::read(&marker).unwrap(), b"last ping");
        assert_eq!(fs::read(&history).unwrap(), b"local history");

        fs::remove_file(salt_child).unwrap();
        fs::remove_dir(&salt).unwrap();
        write_fixture(&salt, TEST_SALT);
        run(&TelemetrySubcommand::Forget).unwrap();
        assert_opted_out();
        for path in [&salt, &marker, &history] {
            assert!(!path.exists(), "unexpected artifact: {}", path.display());
        }
    });
}
