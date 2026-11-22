use super::*;
use crate::core::test_isolation;
use std::fs;

#[test]
fn legacy_migration_requires_a_valid_sqlite_config_even_with_recent_logs() {
    let root = test_isolation::tempdir();
    test_isolation::with_root(root.path(), || {
        let config_dir = user_dirs::config().expect("isolated config directory");
        fs::create_dir_all(&config_dir).unwrap();
        let config_path = config_dir.join("config.toml");

        let tee_dir = get_tee_dir(&RetrieverConfig::default()).expect("isolated tee directory");
        fs::create_dir_all(&tee_dir).unwrap();
        fs::write(tee_dir.join("recent.log"), "previous command output\n").unwrap();

        fs::write(&config_path, "[retriever]\nmode = \"sqlite\"\n").unwrap();
        assert!(legacy_tee_migration_pending());

        fs::write(&config_path, "[retriever]\nmode = \"tee\"\n").unwrap();
        assert!(Config::load().is_ok(), "tee mode should load successfully");
        assert!(!legacy_tee_migration_pending());

        fs::write(&config_path, "[retriever\n").unwrap();
        assert!(
            Config::load().is_err(),
            "malformed TOML should fail to load"
        );
        assert!(!legacy_tee_migration_pending());
    });
}

#[cfg(unix)]
#[test]
fn create_tee_dir_preserves_the_creation_result_for_a_parentless_path() {
    // An empty Unix path has no parent and cannot name a filesystem entry.
    let path = Path::new("");
    let expected = crate::core::utils::create_private_dir(path).ok();
    assert_eq!(create_tee_dir(path), expected);
}
