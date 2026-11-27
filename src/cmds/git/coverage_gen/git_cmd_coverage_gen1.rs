use super::*;
use crate::core::test_isolation;
use std::path::Path;

fn owned(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| a.to_string()).collect()
}

/// Runs an isolated git command in `dir` and insists it succeeds.
fn git_in(dir: &Path, args: &[&str]) -> std::process::Output {
    let mut cmd = std::process::Command::new("git");
    cmd.arg("-C").arg(dir).args([
        "-c",
        "user.name=rtk",
        "-c",
        "user.email=rtk@example.com",
        "-c",
        "commit.gpgsign=false",
    ]);
    cmd.args(args);
    test_isolation::isolate_git(&mut cmd);
    let out = cmd.output().expect("git should run");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn git_ok(dir: &Path, args: &[&str]) -> bool {
    let mut cmd = std::process::Command::new("git");
    cmd.arg("-C").arg(dir).args(args);
    test_isolation::isolate_git(&mut cmd);
    cmd.output().expect("git should run").status.success()
}

#[test]
fn filter_branch_output_caps_remote_only_branches_and_skips_noise() {
    let cap = crate::core::truncate::CAP_WARNINGS;
    let mut output = String::from("\n* main\n\n  dev\n  remotes/origin\n");
    output.push_str("  remotes/origin/HEAD -> origin/main\n  remotes/origin/main\n");
    for i in 0..cap + 3 {
        output.push_str(&format!("  remotes/origin/r{i}\n"));
    }
    let result = filter_branch_output(&output);
    let lines: Vec<&str> = result.lines().collect();
    assert_eq!(lines[0], "* main");
    assert_eq!(lines[1], "  dev");
    assert!(
        result.contains(&format!("  remote-only ({}):", cap + 3)),
        "{result}"
    );
    assert!(result.contains("    ... +3 more"), "{result}");
    let shown = lines.iter().filter(|l| l.starts_with("    r")).count();
    assert_eq!(shown, cap, "{result}");
    assert!(!result.contains("HEAD"), "{result}");
    assert!(!result.lines().any(|l| l.trim().is_empty()), "{result}");
}

#[test]
fn git_write_commands_propagate_failures_from_a_missing_repository() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let missing = tmp.path().join("missing-git-dir");
    let global = vec![
        "--git-dir".to_string(),
        missing.to_string_lossy().into_owned(),
    ];

    let code = run_pull(&owned(&["--ff-only"]), 1, &global).expect("run_pull");
    assert_ne!(code, 0, "pull failure must propagate");

    let code = run_fetch(&owned(&["origin"]), 1, &global).expect("run_fetch");
    assert_ne!(code, 0, "fetch failure must propagate");

    let code = run_branch(&owned(&["--show-current"]), 1, &global).expect("run_branch");
    assert_ne!(code, 0, "branch --show-current failure must propagate");

    let code = run_branch(&[], 0, &global).expect("run_branch list");
    assert_ne!(code, 0, "branch list failure must propagate");

    let code = run_branch(&owned(&["-d", "nope"]), 0, &global).expect("run_branch -d");
    assert_ne!(code, 0, "branch delete failure must propagate");
}

#[test]
fn push_pull_fetch_and_branch_against_a_local_clone() {
    let root = tempfile::tempdir().expect("tempdir");
    let origin = root.path().join("origin");
    let clone = root.path().join("clone");
    std::fs::create_dir_all(&origin).expect("mkdir origin");

    git_in(&origin, &["init", "-q"]);
    std::fs::write(origin.join("a.txt"), "one\n").expect("write");
    git_in(&origin, &["add", "a.txt"]);
    git_in(&origin, &["commit", "-q", "-m", "init"]);
    git_in(
        root.path(),
        &[
            "clone",
            "-q",
            &origin.to_string_lossy(),
            &clone.to_string_lossy(),
        ],
    );

    // A new upstream commit that modifies a line: one insertion, one deletion.
    std::fs::write(origin.join("a.txt"), "two\n").expect("write");
    git_in(&origin, &["commit", "-q", "-am", "second"]);

    let global = vec!["-C".to_string(), clone.to_string_lossy().into_owned()];

    let code = run_fetch(&owned(&["origin"]), 1, &global).expect("run_fetch");
    assert_eq!(code, 0);

    let code = run_pull(&owned(&["--ff-only"]), 1, &global).expect("run_pull");
    assert_eq!(code, 0);
    let content = std::fs::read_to_string(clone.join("a.txt")).expect("read");
    assert_eq!(
        content.trim_end(),
        "two",
        "pull must fast-forward the clone"
    );

    // Nothing left to pull.
    let code = run_pull(&owned(&["--ff-only"]), 0, &global).expect("run_pull again");
    assert_eq!(code, 0);

    // Push a new branch into the (non-bare) origin, away from its checked-out branch.
    let code =
        run_push(&owned(&["origin", "HEAD:refs/heads/pushed"]), 1, &global).expect("run_push");
    assert_eq!(code, 0);
    assert!(
        git_ok(&origin, &["rev-parse", "--verify", "refs/heads/pushed"]),
        "push must create the branch in origin"
    );

    let code = run_branch(&owned(&["--show-current"]), 1, &global).expect("show-current");
    assert_eq!(code, 0);

    let code = run_branch(&owned(&["feature-x"]), 0, &global).expect("create branch");
    assert_eq!(code, 0);
    assert!(
        git_ok(&clone, &["rev-parse", "--verify", "refs/heads/feature-x"]),
        "branch creation must not be swallowed"
    );

    let code = run_branch(&owned(&["-d", "does-not-exist"]), 0, &global).expect("delete");
    assert_ne!(code, 0, "deleting a missing branch must fail");

    let code = run_branch(&[], 0, &global).expect("list");
    assert_eq!(code, 0);

    let code =
        run_branch(&owned(&["-a", "--contains", "HEAD"]), 0, &global).expect("list with flags");
    assert_eq!(code, 0);
}
