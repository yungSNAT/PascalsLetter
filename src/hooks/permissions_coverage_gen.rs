use super::*;
use crate::core::{test_isolation, user_dirs as isolated_dirs};
use std::fs;
use std::path::Path;

fn with_settings_project(test: impl FnOnce(&Path, &Path)) {
    let scratch = test_isolation::tempdir();
    let project = scratch.path().join("project");
    fs::create_dir_all(project.join(CLAUDE_DIR)).expect("create project marker");
    test_isolation::with_root(scratch.path(), || {
        let _guard = test_isolation::enter(&project);
        test(scratch.path(), &project);
    });
}

fn write_settings(dir: &Path, file: &str, content: &str) {
    fs::create_dir_all(dir).expect("create settings directory");
    fs::write(dir.join(file), content).expect("write settings");
}

#[test]
fn claude_skips_settings_without_permissions_and_reads_local_rules() {
    with_settings_project(|root, project| {
        let settings = project.join(CLAUDE_DIR);
        write_settings(&settings, SETTINGS_JSON, r#"{"model":"example"}"#);
        write_settings(
            &settings,
            SETTINGS_LOCAL_JSON,
            r#"{"permissions":{"ask":["Bash(git status)"],"allow":["Bash(git *)"]}}"#,
        );

        let profile = root.join("claude-profile");
        fs::create_dir(&profile).expect("create isolated Claude profile");
        user_env::with_path("CLAUDE_CONFIG_DIR", Some(&profile), || {
            let (deny, ask, allow) = load_rules_for(Host::Claude);
            assert!(deny.is_empty());
            assert_eq!(ask, ["git status"]);
            assert_eq!(allow, ["git *"]);

            // OpenCode uses its own matcher, not the generic Bash rule loader.
            let (deny, ask, allow) = load_rules_for(Host::OpenCode);
            assert!(deny.is_empty() && ask.is_empty() && allow.is_empty());

            #[cfg(feature = "engine")]
            {
                assert!(ask_rule_applies("git status --short", Host::Claude, &[]));
                assert!(!ask_rule_applies("git log", Host::Claude, &[]));
                assert!(!ask_rule_applies("bash -c 'git status'", Host::Claude, &[]));
            }
        });
    });
}

#[test]
fn gemini_falls_back_to_global_rules_and_cursor_requires_its_own_section() {
    with_settings_project(|_, project| {
        let home = isolated_dirs::home().expect("isolated home");
        let global = r#"{
            "security":{"folderTrust":{"enabled":true}},
            "tools":{
                "allowed":["ShellTool(git status)"],
                "confirmationRequired":["run_shell_command(npm test)"]
            }
        }"#;
        write_settings(&home.join(GEMINI_DIR), SETTINGS_JSON, global);
        write_settings(
            &project.join(GEMINI_DIR),
            SETTINGS_JSON,
            r#"{"tools":{"allowed":["ShellTool(cargo test)"]}}"#,
        );

        user_env::with_vars(&[("GEMINI_CLI_TRUST_WORKSPACE", None)], || {
            let (deny, ask, allow) = load_rules_for(Host::Gemini);
            assert!(deny.is_empty());
            assert_eq!(ask, ["npm test *"]);
            assert_eq!(allow, ["git status *"]);
        });
        user_env::with_vars(&[("GEMINI_CLI_TRUST_WORKSPACE", Some("true"))], || {
            let (deny, ask, allow) = load_rules_for(Host::Gemini);
            assert!(deny.is_empty() && ask.is_empty());
            assert_eq!(allow, ["cargo test *"]);
        });

        let cursor = home.join(CURSOR_DIR);
        write_settings(&cursor, "cli-config.json", global);
        let (deny, ask, allow) = load_rules_for(Host::Cursor);
        assert!(deny.is_empty() && ask.is_empty() && allow.is_empty());

        write_settings(
            &cursor,
            "cli-config.json",
            r#"{"permissions":{"allow":["Shell(git status)"]}}"#,
        );
        let (deny, ask, allow) = load_rules_for(Host::Cursor);
        assert!(deny.is_empty() && ask.is_empty());
        assert_eq!(allow, ["git status *"]);
    });
}

#[test]
fn droid_combines_override_and_project_denials_in_both_settings_files() {
    with_settings_project(|root, project| {
        let profile = root.join("droid-profile");
        let user_settings = profile.join(DROID_DIR);
        let project_settings = project.join(DROID_DIR);
        for (dir, file, content) in [
            (
                &user_settings,
                DROID_SETTINGS_FILE,
                r#"{"commandDenylist":["git status"]}"#,
            ),
            (
                &user_settings,
                SETTINGS_LOCAL_JSON,
                r#"{"commandBlocklist":["git log"]}"#,
            ),
            (
                &project_settings,
                DROID_SETTINGS_FILE,
                r#"{"commandDenylist":["cargo check"]}"#,
            ),
            (
                &project_settings,
                SETTINGS_LOCAL_JSON,
                r#"{"commandBlocklist":["cargo test"]}"#,
            ),
        ] {
            write_settings(dir, file, content);
        }

        user_env::with_path(DROID_HOME_ENV, Some(&profile), || {
            let (deny, ask, allow) = load_rules_for(Host::Droid);
            assert_eq!(deny, ["git status", "git log", "cargo check", "cargo test"]);
            assert!(ask.is_empty() && allow.is_empty());
        });
    });
}

#[test]
fn blank_segments_do_not_affect_allow_but_at_least_one_command_is_required() {
    let verdict = |segments: &[&str], allow: &[String]| {
        gate_verdict(segments, &[], false, &[], &[], allow, &[])
    };

    let exact = ["git status".to_string()];
    assert_eq!(
        verdict(&[" \t", "git status", ""], &exact),
        PermissionVerdict::Allow
    );

    let all_stars = ["***".to_string()];
    assert_eq!(
        verdict(&[" \t", ""], &all_stars),
        PermissionVerdict::Default
    );
    assert_eq!(
        verdict(&[" \t", "git status", "cargo test"], &all_stars),
        PermissionVerdict::Allow
    );
}
