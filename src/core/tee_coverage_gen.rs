use super::*;
use crate::core::test_isolation;
#[cfg(feature = "engine")]
use crate::core::user_env;

#[test]
fn disabled_dispatch_does_not_archive_failures_or_hidden_lines() {
    let root = test_isolation::tempdir();
    let archive = root.path().join("archive");
    let database = root.path().join("recall.db");
    let cfg = RetrieverConfig {
        mode: RecoveryMode::Tee,
        tee_directory: Some(archive.clone()),
        database_path: Some(database.clone()),
        ..RetrieverConfig::default()
    };
    let raw = "visible diagnostic\nhidden detail\n".repeat(MIN_FAILURE_BYTES);

    test_isolation::with_root(root.path(), || {
        assert!(tee_and_hint_with(RecoveryMode::Disabled, &cfg, &raw, "build", 1).is_none());
        assert!(force_tee_tail_hint_with(RecoveryMode::Disabled, &cfg, &raw, "build", 2).is_none());
    });

    assert!(!archive.exists());
    assert!(!database.exists());
}

#[cfg(feature = "engine")]
#[test]
fn posthook_skips_empty_output_and_honors_the_recovery_kill_switch() {
    let root = test_isolation::tempdir();
    let archive = root.path().join("archive");
    let mut config = Config::default();
    config.retriever.mode = RecoveryMode::Tee;
    config.retriever.tee_directory = Some(archive.clone());

    test_isolation::with_root(root.path(), || {
        user_env::with_vars(&[("RTK_RECALL", None), ("RTK_TEE", None)], || {
            assert!(posthook_tee_hint(&config, "", "build").is_none());
        });
        user_env::with_vars(&[("RTK_RECALL", Some("0")), ("RTK_TEE", None)], || {
            assert!(posthook_tee_hint(&config, "diagnostic output\n", "build").is_none());
        });
    });

    assert!(!archive.join(POSTHOOK_SUBDIR).exists());
}
