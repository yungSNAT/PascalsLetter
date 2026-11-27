use super::*;
use crate::core::{test_isolation, user_env};

fn with_claude_fixture(root: &Path, test: impl FnOnce(&Path)) {
    let claude_dir = root.join("claude");
    fs::create_dir(&claude_dir).unwrap();
    test_isolation::with_root(root, || {
        user_env::with_path("CLAUDE_CONFIG_DIR", Some(&claude_dir), || test(&claude_dir));
    });
}

#[test]
fn store_and_remove_reject_a_directory_at_the_hash_path() {
    let temp = test_isolation::tempdir();
    let hook = temp.path().join(REWRITE_HOOK_FILE);
    let sidecar = temp.path().join(HASH_FILENAME);
    fs::write(&hook, b"original hook").unwrap();
    fs::create_dir(&sidecar).unwrap();

    assert_eq!(
        store_hash(&hook).unwrap_err().to_string(),
        format!("Failed to write hash to {}", sidecar.display())
    );
    assert_eq!(
        remove_hash(&hook).unwrap_err().to_string(),
        format!("Failed to remove hash file: {}", sidecar.display())
    );
    assert!(sidecar.is_dir());
    assert_eq!(fs::read(&hook).unwrap(), b"original hook");
    fs::remove_dir(&sidecar).unwrap();
}

#[test]
fn legacy_verifier_distinguishes_missing_empty_and_unreadable_baselines() {
    let temp = test_isolation::tempdir();
    with_claude_fixture(temp.path(), |claude_dir| {
        let hooks = claude_dir.join(HOOKS_SUBDIR);
        fs::create_dir(&hooks).unwrap();
        fs::write(hooks.join(REWRITE_HOOK_FILE), b"legacy hook").unwrap();
        let sidecar = hooks.join(HASH_FILENAME);

        assert_eq!(verify_hook().unwrap(), IntegrityStatus::NoBaseline);

        for (bytes, context) in [
            (&b""[..], "Empty hash file"),
            (&b"\xff"[..], "Failed to read hash file"),
        ] {
            fs::write(&sidecar, bytes).unwrap();
            assert_eq!(
                verify_hook().unwrap_err().to_string(),
                format!("{context}: {}", sidecar.display())
            );
        }
    });
}

fn assert_verify_report(
    claude_dir: &Path,
    db_path: &Path,
    private_data_dir: bool,
    expected_stdout: &[&str],
    expected_stderr: &[&str],
) {
    user_env::with_path("RTK_DB_PATH", Some(db_path), || run_verify(1).unwrap());

    if !test_isolation::rtk_binary_ready() {
        return;
    }

    let output = test_isolation::rtk_command()
        .args(["-v", "verify"])
        .env("CLAUDE_CONFIG_DIR", claude_dir)
        .env("RTK_DB_PATH", db_path)
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    // `verify` also runs the built-in filter self-tests, and on Windows 15 of
    // them fail (ledger line), so Windows may exit 1, but no other failure.
    assert!(
        output.status.success() || (cfg!(windows) && output.status.code() == Some(1)),
        "{stdout}\n{stderr}"
    );

    for expected in expected_stdout {
        assert!(stdout.contains(expected), "missing {expected:?}: {stdout}");
    }
    for expected in expected_stderr {
        assert!(stderr.contains(expected), "missing {expected:?}: {stderr}");
    }

    let hooks = claude_dir.join(HOOKS_SUBDIR);
    assert!(stderr.contains(&format!(
        "Hook:  {}",
        hooks.join(REWRITE_HOOK_FILE).display()
    )));
    assert!(stderr.contains(&format!("Hash:  {}", hooks.join(HASH_FILENAME).display())));
    assert_eq!(
        stdout.contains("PASS  data directory is owner-only"),
        cfg!(unix) && private_data_dir,
        "{stdout}"
    );
    assert!(!stderr.contains("WARN  data directory is reachable by other local users"));
}

#[test]
fn verify_reports_native_and_legacy_hook_states() {
    let temp = test_isolation::tempdir();
    with_claude_fixture(temp.path(), |claude_dir| {
        let data = temp.path().join("data");
        fs::create_dir(&data).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            fs::set_permissions(&data, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let db_path = data.join("history.db");

        let blocked = temp.path().join("not-a-directory");
        fs::write(&blocked, b"ordinary file").unwrap();
        let unavailable_db = blocked.join("data").join("history.db");

        let settings = claude_dir.join(super::super::constants::SETTINGS_JSON);
        let malformed = r#"{"hooks":"#;
        fs::write(&settings, malformed).unwrap();
        assert!(!settings_has_claude_hook(malformed));
        assert_verify_report(
            claude_dir,
            &unavailable_db,
            false,
            &["SKIP  RTK hook not installed"],
            &[],
        );
        assert_eq!(fs::read(&blocked).unwrap(), b"ordinary file");

        let native = r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[
            {"type":"command","command":"rtk","args":["hook","claude"]}
        ]}]}}"#;
        fs::write(&settings, native).unwrap();
        assert!(settings_has_claude_hook(native));
        assert_verify_report(
            claude_dir,
            &db_path,
            true,
            &[
                "PASS  native binary hook registered in settings.json",
                "command: rtk hook claude",
            ],
            &[],
        );

        let hooks = claude_dir.join(HOOKS_SUBDIR);
        fs::create_dir(&hooks).unwrap();
        let hook = hooks.join(REWRITE_HOOK_FILE);
        let script = b"legacy hook contents";
        fs::write(&hook, script).unwrap();
        assert_verify_report(
            claude_dir,
            &db_path,
            true,
            &["WARN  no baseline hash found", "to establish baseline"],
            &[],
        );

        store_hash(&hook).unwrap();
        let digest = format!("sha256:{}", compute_hash_bytes(script));
        assert_verify_report(
            claude_dir,
            &db_path,
            true,
            &["PASS  hook integrity verified", &digest],
            &[],
        );

        fs::remove_file(&hook).unwrap();
        assert_verify_report(
            claude_dir,
            &db_path,
            true,
            &[],
            &["WARN  hash file exists but hook is missing", "to reinstall"],
        );
        assert!(remove_hash(&hook).unwrap());
    });
}
