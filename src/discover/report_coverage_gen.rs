use super::*;

fn scanned_report() -> DiscoverReport {
    DiscoverReport {
        sessions_scanned: 1,
        scope: ScanScope::AllProjects,
        total_commands: 0,
        already_rtk: 0,
        already_rtk_estimated: 0,
        measured_since: None,
        since_days: 7,
        supported: vec![],
        unsupported: vec![],
        parse_errors: 0,
        rtk_disabled_count: 0,
        rtk_disabled_estimated: 0,
        rtk_disabled_examples: vec![],
        agent_status: AgentIntegrationStatus::default(),
    }
}

#[test]
fn report_limits_both_tables_without_limiting_savings_totals() {
    let report = DiscoverReport {
        total_commands: 35,
        supported: vec![
            SupportedEntry {
                command: "git status".to_string(),
                count: 10,
                rtk_equivalent: "rtk git status",
                category: "Git",
                estimated_savings_tokens: 51_000,
                estimated_savings_pct: 75.0,
                rtk_status: RtkStatus::Existing,
            },
            SupportedEntry {
                command: "cargo test".to_string(),
                count: 20,
                rtk_equivalent: "rtk cargo test",
                category: "Rust",
                estimated_savings_tokens: 1_000_000,
                estimated_savings_pct: 80.0,
                rtk_status: RtkStatus::Existing,
            },
        ],
        unsupported: vec![
            UnsupportedEntry {
                base_command: "unhandled-command-xé-extra".to_string(),
                count: 4,
                example: "unhandled-command-xé-extra --id café --verbose".to_string(),
            },
            UnsupportedEntry {
                base_command: "other-unhandled".to_string(),
                count: 1,
                example: "other-unhandled --help".to_string(),
            },
        ],
        ..scanned_report()
    };

    let output = format_text(&report, 1, false);

    let savings_row = format!(
        "{:<24} {:>5}    {:<18} {:<13} ~51.0K tokens",
        "git status", 10, "rtk git status", "existing"
    );
    assert!(output.lines().any(|line| line == savings_row), "{output}");
    assert!(!output.contains("cargo test"), "{output}");
    assert!(
        output.contains("Total: 30 commands -> ~1.1M tokens saveable\n"),
        "{output}"
    );

    let rule = "-".repeat(52);
    let unhandled_section = format!(
        "\nTOP UNHANDLED COMMANDS -- open an issue?\n\
         {rule}\n\
         {:<24} {:>5}    Example\n\
         {:<24} {:>5}    unhandled-command-xé-extra --id café..\n\
         {rule}\n\
         -> github.com/yungSNAT/PascalsLetter/issues\n",
        "Command", "Count", "unhandled-command-xé..", 4
    );
    assert!(output.contains(&unhandled_section), "{output}");
    assert!(!output.contains("other-unhandled"), "{output}");
}

#[test]
fn report_discloses_bypass_examples_and_skipped_parse_errors() {
    let report = DiscoverReport {
        total_commands: 2,
        parse_errors: 3,
        rtk_disabled_count: 2,
        rtk_disabled_estimated: 2,
        rtk_disabled_examples: vec!["git status".to_string(), "cargo test".to_string()],
        agent_status: AgentIntegrationStatus {
            cursor_hook_installed: true,
            ..AgentIntegrationStatus::default()
        },
        ..scanned_report()
    };

    let quiet = format_text(&report, 10, false);
    assert!(quiet.contains("RTK_DISABLED BYPASS -- 2 commands ran without filtering\n"));
    assert!(
        quiet.contains(
            "These commands used RTK_DISABLED=1 unnecessarily:\n  git status, cargo test\n"
        )
    );
    assert!(quiet.contains("  includes ~2 estimated from current hook/config state\n"));
    assert!(quiet.contains("-> Remove RTK_DISABLED=1 to recover token savings\n"));
    assert!(quiet.ends_with(
        "Note: Cursor sessions are tracked via `rtk gain` (discover scans Claude Code only)\n"
    ));
    assert!(!quiet.contains("Parse errors skipped"));

    let verbose = format_text(&report, 10, true);
    assert_eq!(verbose, format!("{quiet}Parse errors skipped: 3\n"));
}

#[test]
fn status_labels_distinguish_passthrough_from_unsupported_commands() {
    assert_eq!(RtkStatus::Passthrough.as_str(), "passthrough");
    assert_eq!(RtkStatus::NotSupported.as_str(), "not-supported");
}
