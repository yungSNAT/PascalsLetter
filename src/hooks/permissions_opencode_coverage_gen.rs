use super::*;
use crate::core::{test_isolation, user_env};
use std::fs;

fn shell_rule(pattern: &str, action: Action) -> Rule {
    Rule {
        permission: "bash".to_string(),
        pattern: pattern.to_string(),
        action,
    }
}

#[test]
fn blank_commands_do_not_inherit_a_catch_all_allow() {
    let rules = [shell_rule("*", Action::Allow)];

    for command in ["", " \t "] {
        assert_eq!(
            check_command_with_opencode_rules(command, &rules),
            PermissionVerdict::Default,
            "blank command {command:?} should have no verdict from the rules"
        );
    }
    assert_eq!(
        check_command_with_opencode_rules("echo hello", &rules),
        PermissionVerdict::Allow
    );
}

#[test]
fn malformed_legacy_permissions_leave_valid_rules_intact() {
    let configs = [
        serde_json::json!({ "permission": { "bash": "ask" } }),
        serde_json::json!({ "permission": "allow" }),
        serde_json::json!({ "permission": { "bash": "maybe", "*": true } }),
    ];

    assert_eq!(
        rules_from_configs(&configs, None),
        vec![shell_rule("*", Action::Ask)]
    );
}

#[test]
fn malformed_config_falls_through_only_during_directory_lookup() {
    let tmp = test_isolation::tempdir();
    let home = tmp.path().join("home");
    let global = home.join(CONFIG_DIR).join(OPENCODE_SUBDIR);
    let project = tmp.path().join("project");
    fs::create_dir_all(&global).expect("create global config directory");
    fs::create_dir_all(&project).expect("create project directory");

    let malformed = global.join("opencode.json");
    fs::write(&malformed, r#"{"permission":"#).expect("write malformed config");
    fs::write(
        global.join("opencode.jsonc"),
        r#"{ "permission": { "bash": { "git *": "deny" } } }"#,
    )
    .expect("write fallback config");
    fs::write(
        project.join("opencode.json"),
        r#"{ "permission": { "bash": { "echo *": "ask" } } }"#,
    )
    .expect("write project config");

    test_isolation::with_root(&home, || {
        let _entered = test_isolation::enter(&project);
        user_env::with_vars(&[("OPENCODE_CONFIG", None)], || {
            assert_eq!(
                load_opencode_rules(None),
                vec![
                    shell_rule("git *", Action::Deny),
                    shell_rule("echo *", Action::Ask),
                ],
                "directory lookup should try opencode.jsonc after malformed opencode.json"
            );
        });
        user_env::with_path("OPENCODE_CONFIG", Some(&malformed), || {
            assert_eq!(
                load_opencode_rules(None),
                vec![shell_rule("echo *", Action::Ask)],
                "a malformed explicit override should skip global fallback but keep project rules"
            );
        });
    });
}
