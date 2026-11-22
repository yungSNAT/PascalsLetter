use super::{default_log_path, run};
use crate::core::{test_isolation, user_env};
use std::fs;

#[test]
fn audit_reports_read_errors_at_the_configured_path() {
    let dir = test_isolation::tempdir();
    let log_path = dir.path().join("hook-audit.log");
    fs::write(&log_path, b"\xff").unwrap();

    user_env::with_path("RTK_AUDIT_DIR", Some(dir.path()), || {
        assert_eq!(default_log_path(), log_path);
        let error = run(0, 0).expect_err("invalid UTF-8 is not an empty audit log");
        assert_eq!(
            error.to_string(),
            format!("Failed to read {}", log_path.display())
        );
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::InvalidData
        );
    });
}
