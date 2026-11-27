use super::*;
use crate::core::test_isolation;
use std::path::Path;

const IDENTITY: [&str; 6] = [
    "-c",
    "user.name=rtk",
    "-c",
    "user.email=rtk@example.com",
    "-c",
    "commit.gpgsign=false",
];

fn owned(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| a.to_string()).collect()
}

fn git_in(repo: &Path, args: &[&str]) {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(repo).args(IDENTITY).args(args);
    test_isolation::isolate_git(&mut cmd);
    let out = cmd.output().expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn globals(repo: &Path) -> Vec<String> {
    let mut g = vec!["-C".to_string(), repo.to_string_lossy().into_owned()];
    g.extend(IDENTITY.iter().map(|s| s.to_string()));
    g
}

#[test]
fn git_runners_report_gits_verdict_in_a_real_repo() {
    let repo = test_isolation::temp_git_repo();
    let path = repo.path().to_path_buf();
    let home = test_isolation::tempdir();
    test_isolation::with_root(home.path(), || {
        std::fs::write(path.join("a.txt"), "hello\n").expect("write a.txt");
        git_in(&path, &["add", "a.txt"]);
        git_in(&path, &["commit", "-q", "-m", "first"]);
        let g = globals(&path);

        // add: a staged file, then a pathspec git refuses.
        std::fs::write(path.join("b.txt"), "second\n").expect("write b.txt");
        assert_eq!(run_add(&owned(&["b.txt"]), 1, &g).expect("run_add"), 0);
        assert_ne!(
            run_add(&owned(&["no-such-file.txt"]), 0, &g).expect("run_add fails"),
            0
        );

        // commit: one that succeeds, then one with nothing staged.
        assert_eq!(
            run_commit(&owned(&["-m", "second"]), 1, &g).expect("run_commit"),
            0
        );
        assert_ne!(
            run_commit(&owned(&["-m", "empty"]), 0, &g).expect("run_commit empty"),
            0
        );

        // status with user args takes the minimal-filter path.
        assert_eq!(
            run_status(&owned(&["--porcelain"]), 1, &g).expect("run_status"),
            0
        );

        // diff: --no-compact passthrough, failures on both paths, and the capped compact path.
        assert_eq!(
            run_diff(&owned(&["--no-compact"]), None, 0, &g).expect("run_diff"),
            0
        );
        assert_ne!(
            run_diff(&owned(&["--stat", "no-such-ref-xyz"]), None, 0, &g).expect("diff stat"),
            0
        );
        assert_ne!(
            run_diff(&owned(&["no-such-ref-xyz"]), None, 0, &g).expect("diff bad ref"),
            0
        );
        assert_eq!(
            run_diff(&owned(&["-w"]), Some(50), 1, &g).expect("diff capped"),
            0
        );

        // show: stat failure, blob failure, commit-diff failure, then a verbose success.
        assert_ne!(
            run_show(&owned(&["--stat", "no-such-ref-xyz"]), None, 0, &g).expect("show stat"),
            0
        );
        assert_ne!(
            run_show(&owned(&["HEAD:a.txt", "--no-such-flag-xyz"]), None, 0, &g)
                .expect("show blob"),
            0
        );
        assert_ne!(
            run_show(&owned(&["no-such-ref-xyz"]), None, 0, &g).expect("show bad ref"),
            0
        );
        assert_eq!(run_show(&[], None, 1, &g).expect("show HEAD"), 0);

        // log: verbose filtered path.
        assert_eq!(run_log(&[], None, 1, &g).expect("run_log"), 0);

        // checkout of a branch that does not exist.
        assert_ne!(
            run_checkout(&owned(&["no-such-branch-xyz"]), 1, &g).expect("run_checkout"),
            0
        );

        // compact status on a detached HEAD while bisecting.
        git_in(&path, &["checkout", "-q", "--detach"]);
        git_in(&path, &["bisect", "start"]);
        assert_eq!(run_status(&[], 0, &g).expect("run_status compact"), 0);
    });
}

#[test]
fn git_runners_propagate_failure_outside_a_repository() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = test_isolation::tempdir();
    let g = vec!["-C".to_string(), dir.path().to_string_lossy().into_owned()];
    test_isolation::with_root(home.path(), || {
        for cmd in [
            GitCommand::Commit,
            GitCommand::Push,
            GitCommand::Pull,
            GitCommand::Fetch,
            GitCommand::Worktree,
        ] {
            let label = format!("{cmd:?}");
            let code = run(cmd, &[], None, 1, &g).expect("run");
            assert_ne!(code, 0, "{label} must propagate git's failure");
        }
        assert_ne!(
            run_status(&owned(&["--short"]), 0, &g).expect("status --short"),
            0
        );
        assert_ne!(run_status(&[], 0, &g).expect("status compact"), 0);
        assert_eq!(walk_exceeds_limit(&g, &[], &[], DEFAULT_LOG_LIMIT), None);
    });
}

#[test]
fn compact_diff_notes_truncation_at_every_boundary() {
    let mut diff = String::from("diff --git a/f b/f\n@@ -0,0 +1,110 @@\n");
    for i in 0..110 {
        diff.push_str(&format!("+a{i}\n"));
    }
    diff.push_str("@@ -5,0 +111,1 @@\n+z\n");
    diff.push_str("diff --git a/g b/g\n@@ -1,3 +1,108 @@\n");
    for i in 0..105 {
        diff.push_str(&format!("+b{i}\n"));
    }
    for i in 0..3 {
        diff.push_str(&format!(" ctx{i}\n"));
    }
    diff.push_str(
        "diff --git a/h b/h\n@@ -0,0 +0,0 @@\nstray\n@@ -1,2 +1,2 @@\n-x\n\\ No newline at end of file\n+y\n",
    );
    let out = compact_diff(&diff, 1000);

    assert!(out.contains("  ... (10 additions truncated)"), "{out}");
    assert!(out.contains("  ... (5 additions truncated)"), "{out}");
    assert!(out.lines().any(|l| l == "+z"), "{out}");
    assert!(!out.contains("+a100"), "{out}");
    assert!(!out.contains(" ctx0"), "{out}");
    assert!(!out.contains("stray"), "{out}");
    assert!(!out.contains("No newline"), "{out}");
    assert!(out.lines().any(|l| l == "-x"), "{out}");
    assert!(out.lines().any(|l| l == "+y"), "{out}");
    assert!(out.contains("  +111 -0"), "{out}");
    assert!(out.contains("  +105 -0"), "{out}");
    assert!(out.contains("  +1 -1"), "{out}");
    assert!(out.contains(NO_COMPACT_HINT), "{out}");

    let h = parse_hunk_header("@@ -1,2 junk +1,3 @@").expect("header");
    assert_eq!(h.parents, vec![2]);
    assert_eq!(h.new, 3);
}

#[test]
fn diff_header_path_handles_odd_headers_and_escapes() {
    assert_eq!(diff_header_path("diff --git"), "unknown");
    assert_eq!(diff_header_path("diff --git a/x c/y"), "a/x c/y");
    assert_eq!(diff_header_path("diff --git a/x b/yy"), "yy");
    assert_eq!(
        diff_header_path(r#"diff --cc "x\a\b\n\v\f\r\9""#),
        "x\u{7}\u{8}\n\u{b}\u{c}\r\\9"
    );
}

#[test]
fn checkout_output_formats_every_shape() {
    let fmt = |args: &[&str], raw: &str| format_checkout_output(&owned(args), raw, 0);
    assert_eq!(fmt(&[], "Already on 'main'"), "ok main");
    assert_eq!(fmt(&[], "HEAD is now at abc1234 msg"), "ok HEAD abc1234");
    assert_eq!(
        fmt(&[], "Updated 2 paths from the index"),
        "ok updated 2 paths from the index"
    );
    assert_eq!(fmt(&["-b", "feat"], ""), "ok feat (new)");
    assert_eq!(fmt(&["--orphan", "lonely"], ""), "ok lonely (new)");
    assert_eq!(fmt(&["-B", "feat"], ""), "ok feat");
    assert_eq!(fmt(&["main"], ""), "ok main");
    assert_eq!(fmt(&["--"], ""), "ok");
    assert_eq!(fmt(&[], ""), "ok");

    assert!(checkout_takes_value(TokenKind::Long, "conflict").is_some());
    assert!(checkout_takes_value(TokenKind::Positional, "x").is_none());

    let raw = "error: Your local changes to the following files would be overwritten by checkout:\n\ta.txt\nsomething\n\nPlease commit your changes or stash them before you switch branches.\nAborting\n";
    assert_eq!(
        format_checkout_output(&[], raw, 1),
        "error: Your local changes to the following files would be overwritten by checkout:\n\ta.txt\nAborting"
    );
    assert_eq!(filter_checkout_failure("  just text \n"), "just text");
}

#[test]
fn push_handler_skips_blanks_and_summarises_without_a_ref() {
    let mut h = GitPushLineHandler::default();
    assert!(h.should_skip(""));
    assert!(!h.should_skip("To origin"));
    h.observe_line("weird -> ");
    assert!(h.pushed_ref.is_none());
    assert_eq!(h.format_summary(0, ""), Some("ok\n".to_string()));
    assert_eq!(h.format_summary(1, ""), None);
}

#[test]
fn status_formatters_handle_empty_and_branchless_input() {
    assert_eq!(format_status_output(""), "Clean working tree");
    assert_eq!(
        format_status_output(" M a.rs\n?? b.rs\n"),
        " M a.rs\n?? b.rs"
    );
    assert_eq!(filter_status_with_args(""), "ok");
    assert_eq!(
        filter_status_with_args("  (use \"git add <file>...\" to include)\n"),
        "ok"
    );
}

#[test]
fn grammar_predicates_and_log_helpers() {
    assert!(git_global_takes_value(TokenKind::Positional, "x").is_none());
    assert!(git_global_takes_value(TokenKind::DashDash, "--").is_none());
    assert!(git_global_takes_value(TokenKind::Short, "C").is_some());
    assert!(log_takes_value(TokenKind::Positional, "x").is_none());
    assert!(diff_takes_value(TokenKind::Short, "n").is_some());
    assert!(diff_takes_value(TokenKind::Positional, "x").is_none());

    let two = owned(&["--min-parents=2"]);
    assert!(log_wants_merges(&tokenize_git_log_args(&two)));
    let one = owned(&["--min-parents=1"]);
    assert!(!log_wants_merges(&tokenize_git_log_args(&one)));
    let junk = owned(&["--min-parents=x"]);
    assert!(!log_wants_merges(&tokenize_git_log_args(&junk)));

    assert!(!format_certainly_prints(&[]));
    assert_eq!(at_most_commits("", &[]), None);
}
