// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: Apache-2.0

//! Rewrites a hook would run in place of the command the agent wrote. Each case
//! goes through the binary, so `tools/fork-proof` can run it against
//! upstream RTK too.

mod common;

/// What `rtk rewrite` prints for `command`: empty when it leaves it alone.
fn rewrite(command: &str) -> String {
    let out = common::rtk_command()
        .args(["rewrite", command])
        .output()
        .expect("run rtk rewrite");
    String::from_utf8_lossy(&out.stdout).trim_end().to_string()
}

#[test]
fn yadm_goes_through_rtk_yadm_not_rtk_git() {
    // `rtk git` would run git on the current project, not yadm's dotfiles repository.
    assert_eq!(rewrite("yadm commit -m x"), "rtk yadm commit -m x");
    assert_eq!(rewrite("yadm add ."), "rtk yadm add .");
}

#[test]
fn a_php_tool_path_the_shell_expands_keeps_the_path() {
    for (command, path) in [
        (
            "$ROOT/vendor/bin/phpunit tests/",
            "$ROOT/vendor/bin/phpunit",
        ),
        ("~/x/vendor/bin/pint --test", "~/x/vendor/bin/pint"),
    ] {
        let out = rewrite(command);
        assert!(
            out.is_empty() || out.contains(path),
            "{command:?} -> {out:?}"
        );
    }
}

#[test]
fn whitespace_bash_does_not_split_at_is_not_rewritten() {
    // A vertical tab taken as a word break turned `>&2` into a redirect of descriptor 7.
    for command in ["cat \u{b}7>&2", "cat a\u{a0}b"] {
        assert_eq!(rewrite(command), "", "{command:?}");
    }
}

#[test]
fn a_line_continuation_inside_a_word_joins_it_as_bash_does() {
    // bash runs `git status`; splitting at the continuation ran `git sta tus`.
    assert_eq!(rewrite("git sta\\\ntus"), "rtk git status");
}

#[test]
fn a_heredoc_with_a_descriptor_keeps_its_body() {
    // The body is the data `cat` reads, not commands to rewrite.
    for command in [
        "cat 3<<EOF\ngit status && git log\nEOF",
        "cat 0<<-EOF\ngit status\nEOF",
    ] {
        assert_eq!(rewrite(command), "", "{command:?}");
    }
}
