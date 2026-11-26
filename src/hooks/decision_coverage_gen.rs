use super::*;
use crate::core::{test_isolation, user_dirs};

#[test]
fn claude_hook_verdict_uses_configured_permission_rules() {
    let root = test_isolation::tempdir();
    let project = test_isolation::tempdir();
    test_isolation::with_root(root.path(), || {
        let _working_dir = test_isolation::enter(project.path());
        let settings_dir = user_dirs::home()
            .expect("isolated home directory")
            .join(".claude");
        std::fs::create_dir_all(&settings_dir).expect("create Claude settings directory");
        std::fs::write(
            settings_dir.join("settings.json"),
            r#"{"permissions":{"allow":["Bash(git status)"],"deny":["Bash(git log)"]}}"#,
        )
        .expect("write Claude permission rules");

        user_env::with_path("CLAUDE_CONFIG_DIR", Some(&settings_dir), || {
            let path = AgentPath::ClaudeHook;
            assert_eq!(path.verdict("git status"), PermissionVerdict::Allow);
            assert_eq!(path.verdict("git log"), PermissionVerdict::Deny);
            assert_eq!(path.verdict("git diff"), PermissionVerdict::Default);
        });
    });
}
