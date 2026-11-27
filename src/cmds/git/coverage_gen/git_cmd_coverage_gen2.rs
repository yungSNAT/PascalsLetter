use super::*;
use crate::core::test_isolation;
use std::path::Path;

fn run_git(dir: &Path, args: &[&str]) -> String {
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
    let out = cmd.output().expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn init_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    run_git(dir.path(), &["init", "-q"]);
    std::fs::write(dir.path().join("a.txt"), "one\n").expect("write a.txt");
    run_git(dir.path(), &["add", "a.txt"]);
    run_git(dir.path(), &["commit", "-q", "-m", "init"]);
    dir
}

fn globals(dir: &Path) -> Vec<String> {
    vec![
        "-C".to_string(),
        dir.to_string_lossy().into_owned(),
        "-c".to_string(),
        "user.name=rtk".to_string(),
        "-c".to_string(),
        "user.email=rtk@example.com".to_string(),
        "-c".to_string(),
        "commit.gpgsign=false".to_string(),
    ]
}

fn owned(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| a.to_string()).collect()
}

fn stash_count(dir: &Path) -> usize {
    run_git(dir, &["stash", "list"])
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count()
}

fn a_txt(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("a.txt"))
        .expect("read a.txt")
        .trim_end()
        .to_string()
}

#[test]
fn stash_message_reports_noop_and_subcommands() {
    let result = |stdout: &str, stderr: &str| CaptureResult {
        stdout: stdout.to_string(),
        stderr: stderr.to_string(),
        exit_code: 0,
    };
    assert_eq!(
        format_stash_message(None, &result("Saved working directory", "")),
        "ok stashed"
    );
    assert_eq!(
        format_stash_message(Some("push"), &result("", "No local changes to save")),
        "No local changes to save"
    );
    assert_eq!(
        format_stash_message(Some("save"), &result("No local changes to save\n", "")),
        "No local changes to save"
    );
    assert_eq!(
        format_stash_message(Some("pop"), &result("anything", "")),
        "ok stash pop"
    );
}

#[test]
fn stash_list_keeps_messages_without_branch_and_odd_lines() {
    let out = filter_stash_list("stash@{0}:  custom message \ngarbage line\n");
    assert_eq!(out, "stash@{0}: custom message\ngarbage line");
}

#[test]
fn compact_stash_stat_caps_files_and_keeps_summary() {
    let total = CAP_LIST + 5;
    let mut raw = String::from(" empty.txt | 0\n");
    for i in 0..total {
        raw.push_str(&format!(" f{i:03}.rs | 2 ++\n"));
    }
    raw.push_str(&format!(
        " {} files changed, {} insertions(+)\n   \n",
        total + 1,
        total * 2
    ));
    let root = tempfile::tempdir().expect("tempdir");
    let mut out = String::new();
    test_isolation::with_root(root.path(), || {
        out = compact_stash_stat(&raw);
    });
    assert!(out.contains("empty.txt 0"), "{out}");
    assert!(!out.contains("empty.txt 0 +"), "{out}");
    assert!(out.contains("f000.rs 2 +"), "{out}");
    let last = format!("f{:03}.rs", total - 1);
    assert!(!out.contains(&last), "{out}");
    let summary = format!("\n{} changed {} +", total + 1, total * 2);
    assert!(out.ends_with(&summary), "{out}");
}

#[test]
fn run_stash_covers_every_subcommand_family() {
    let repo = init_repo();
    let dir = repo.path();
    let g = globals(dir);

    std::fs::write(dir.join("a.txt"), "two-changed\n").expect("modify");
    assert_eq!(run_stash(None, &[], 1, &g).expect("push"), 0);
    assert_eq!(stash_count(dir), 1);
    assert_eq!(a_txt(dir), "one");

    assert_eq!(run_stash(Some("list"), &[], 0, &g).expect("list"), 0);
    assert_eq!(run_stash(Some("show"), &[], 0, &g).expect("show"), 0);
    assert_eq!(
        run_stash(Some("show"), &owned(&["-p"]), 0, &g).expect("show -p"),
        0
    );
    assert_eq!(
        run_stash(Some("show"), &owned(&["-p", "--word-diff"]), 0, &g).expect("word"),
        0
    );
    assert_eq!(
        run_stash(Some("show"), &owned(&["--stat", "stash@{0}"]), 0, &g).expect("stat"),
        0
    );

    assert_eq!(run_stash(Some("apply"), &[], 0, &g).expect("apply"), 0);
    assert_eq!(a_txt(dir), "two-changed");
    assert_eq!(stash_count(dir), 1);

    assert_eq!(
        run_stash(Some("save"), &owned(&["second"]), 0, &g).expect("save"),
        0
    );
    assert_eq!(stash_count(dir), 2);
    assert_eq!(a_txt(dir), "one");

    // Nothing to stash: still a success, nothing added.
    assert_eq!(run_stash(Some("push"), &[], 0, &g).expect("noop"), 0);
    assert_eq!(stash_count(dir), 2);

    assert_eq!(run_stash(Some("pop"), &[], 0, &g).expect("pop"), 0);
    assert_eq!(a_txt(dir), "two-changed");
    assert_eq!(stash_count(dir), 1);

    // A distinctly sized change, so git cannot mistake the file for unchanged.
    std::fs::write(dir.join("a.txt"), "third version of the file\n").expect("modify again");
    // An unknown subcommand is a pathspec for `stash push`.
    assert_eq!(run_stash(Some("a.txt"), &[], 0, &g).expect("pathspec"), 0);
    assert_eq!(stash_count(dir), 2);
    assert_eq!(a_txt(dir), "one");

    assert_ne!(
        run_stash(Some("missing-file.txt"), &[], 0, &g).expect("bad pathspec"),
        0
    );
    assert_eq!(stash_count(dir), 2);

    assert_eq!(run_stash(Some("drop"), &[], 0, &g).expect("drop"), 0);
    assert_eq!(run_stash(Some("drop"), &[], 0, &g).expect("drop"), 0);
    assert_eq!(stash_count(dir), 0);
    assert_ne!(run_stash(Some("drop"), &[], 0, &g).expect("empty drop"), 0);
    assert_eq!(run_stash(Some("clear"), &[], 0, &g).expect("clear"), 0);
}

#[test]
fn run_fetch_reports_success_and_failure() {
    let origin = init_repo();
    let clone = tempfile::tempdir().expect("tempdir");
    run_git(clone.path(), &["init", "-q"]);
    run_git(
        clone.path(),
        &["remote", "add", "origin", &origin.path().to_string_lossy()],
    );
    let g = globals(clone.path());

    assert_eq!(run_fetch(&[], 1, &g).expect("fetch"), 0);
    let refs = run_git(clone.path(), &["for-each-ref", "refs/remotes"]);
    assert!(!refs.trim().is_empty(), "fetch must create remote refs");
    // Nothing new the second time.
    assert_eq!(run_fetch(&[], 0, &g).expect("refetch"), 0);

    assert_ne!(
        run_fetch(&owned(&["./no-such-remote"]), 0, &g).expect("bad remote"),
        0
    );
}

#[test]
fn run_worktree_write_actions_and_passthrough() {
    let repo = init_repo();
    let dir = repo.path();
    let g = globals(dir);
    let parent = tempfile::tempdir().expect("tempdir");
    let wt = parent.path().join("wt");
    let wt_str = wt.to_string_lossy().into_owned();

    assert_eq!(
        run_worktree(&owned(&["add", "-b", "topic", &wt_str]), 1, &g).expect("add"),
        0
    );
    assert!(wt.join("a.txt").exists());

    std::fs::remove_dir_all(&wt).expect("remove worktree dir");
    assert_eq!(
        run_worktree(&owned(&["prune", "-n"]), 0, &g).expect("prune -n"),
        0
    );
    assert!(run_git(dir, &["worktree", "list", "--porcelain"]).contains("topic"));
    assert_eq!(run_worktree(&owned(&["prune"]), 0, &g).expect("prune"), 0);
    assert!(!run_git(dir, &["worktree", "list", "--porcelain"]).contains("topic"));

    assert_ne!(
        run_worktree(&owned(&["remove", "no-such-worktree"]), 0, &g).expect("remove"),
        0
    );

    assert_eq!(
        run_worktree(&owned(&["list", "--porcelain"]), 0, &g).expect("porcelain"),
        0
    );
    assert_ne!(
        run_worktree(&owned(&["frobnicate"]), 0, &g).expect("unknown"),
        0
    );
}
