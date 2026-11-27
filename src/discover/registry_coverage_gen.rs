use super::*;
use crate::core::{test_isolation, user_env};

fn with_scratch_project(test: impl FnOnce()) {
    let scratch = test_isolation::tempdir();
    test_isolation::with_root(scratch.path(), || {
        let _working_dir = test_isolation::enter(scratch.path());
        test();
    });
}

#[test]
fn average_token_estimates_use_the_category_and_subcommand() {
    for (category, subcommand, expected) in [
        ("Git", "log", 200),
        ("Git", "diff", 200),
        ("Git", "show", 200),
        ("Git", "status", 40),
        ("Cargo", "test", 500),
        ("Cargo", "check", 150),
        ("Tests", "run", 800),
        ("Files", "list", 100),
        ("Build", "build", 300),
        ("Infra", "plan", 120),
        ("Network", "get", 150),
        ("GitHub", "pr", 200),
        ("GitLab", "mr", 200),
        ("PackageManager", "install", 150),
        ("Unknown", "test", 150),
    ] {
        assert_eq!(
            category_avg_tokens(category, subcommand),
            expected,
            "{category}/{subcommand}"
        );
    }
}

#[test]
fn inputs_without_a_command_word_do_not_produce_a_route() {
    let blank = " \t\n ";
    assert_eq!(classify_command(blank), Classification::Ignored);
    assert!(split_command_chain(blank).is_empty());
    assert_eq!(strip_trailing_redirects(blank), (blank, ""));

    with_scratch_project(|| {
        // Basename normalization leaves a tab followed by a flag, not a tool name.
        assert_eq!(
            classify_command("tools/\t--registry-coverage-flag"),
            Classification::Ignored
        );
    });
}

#[test]
fn golangci_short_config_option_consumes_its_value_but_double_dash_stops_parsing() {
    let command = "golangci-lint -c 'lint settings.yml' run --verbose";
    let parts = parse_golangci_run_parts(command).expect("short config option before run");
    assert_eq!(parts.global_segment, "-c 'lint settings.yml'");
    assert_eq!(parts.run_segment, "run --verbose");

    let command = "golangci-lint -c 'lint settings.yml' -- run --verbose";
    assert!(parse_golangci_run_parts(command).is_none());
    assert_eq!(strip_golangci_global_opts(command), command);
}

#[test]
fn analytics_does_not_attribute_a_later_clause_to_a_disabled_prefix() {
    with_scratch_project(|| {
        assert_eq!(
            strip_disabled_prefix_for_analytics("sudo RTK_DISABLED=1 git status"),
            ("sudo RTK_DISABLED=1 ", "git status")
        );

        let command = "sudo RTK_DISABLED=1 && git status";
        assert_eq!(strip_disabled_prefix_for_analytics(command), ("", command));
    });
}

#[test]
fn multiline_rewrite_preserves_a_dangling_continuation_operator() {
    with_scratch_project(|| {
        assert_eq!(
            rewrite_command("git status\n  git log &&", &[], &[]).as_deref(),
            Some("rtk git status\n  rtk git log &&")
        );
    });
}

#[test]
fn git_output_is_only_marked_as_folded_when_arguments_request_it() {
    assert!(producer_output_is_line_faithful("rtk git", "git"));
    assert!(!producer_output_is_line_faithful(
        "rtk git",
        "git ls-tree -r --name-only HEAD"
    ));
    assert!(producer_output_is_line_faithful(
        "rtk git",
        "git ls-tree -r -z --name-only HEAD"
    ));
}

#[test]
fn malformed_anchored_exclusion_matches_only_its_literal_prefix() {
    let compiled = compile_exclude_patterns(&["  ^git status[  ".to_owned()]);
    assert!(matches!(
        compiled.as_slice(),
        [ExcludePattern::Prefix(prefix)] if prefix == "^git status["
    ));
    assert!(is_excluded("^git status[ --short", &compiled));
    assert!(!is_excluded("git status --short", &compiled));
    assert!(!is_excluded("echo ^git status[", &compiled));
}

#[test]
fn shell_script_opt_out_inspection_stops_at_the_depth_limit() {
    let excluded = compile_exclude_patterns(&["pytest".to_owned()]);
    let literal = "bash -c 'timeout 5 pytest -q'";
    assert!(shell_wrapper_script_opts_out(
        literal,
        &excluded,
        MAX_PREFIX_DEPTH - 1
    ));
    assert!(!shell_wrapper_script_opts_out(
        literal,
        &excluded,
        MAX_PREFIX_DEPTH
    ));

    for command in ["bash -c ''", r#"bash -c "$TEST_RUNNER""#] {
        assert!(
            !shell_wrapper_script_opts_out(command, &excluded, 0),
            "{command:?} does not expose an excluded command"
        );
    }
}

#[test]
fn wrapper_positions_stop_when_there_is_no_inner_command() {
    let prefixes = ["wrap".to_owned(), "uv".to_owned()];
    assert!(shell_wrapper_positions(" \t ", &prefixes).is_empty());

    for command in ["uv run", "wrap"] {
        assert_eq!(shell_wrapper_positions(command, &prefixes), [command]);
        assert_eq!(rewrite_command(command, &[], &prefixes), None, "{command}");
    }
}

#[test]
fn disabling_toml_filters_leaves_native_rewrites_enabled() {
    with_scratch_project(|| {
        user_env::with_vars(&[("RTK_NO_TOML", None)], || {
            assert_eq!(
                rewrite_command("jj log", &[], &[]).as_deref(),
                Some("rtk jj log")
            );
        });
        user_env::with_vars(&[("RTK_NO_TOML", Some("1"))], || {
            assert_eq!(rewrite_command("jj log", &[], &[]), None);
            assert_eq!(
                rewrite_command("git status", &[], &[]).as_deref(),
                Some("rtk git status")
            );
        });
    });
}
