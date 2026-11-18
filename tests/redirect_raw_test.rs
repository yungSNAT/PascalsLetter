//! `rtk <command> > file` writes the command's own output to the file, not rtk's view of it.
//!
//! A file is read later, often whole and often by a program, so a filter's cut there loses
//! data with nobody watching: `rtk git log > log.txt` once kept 30 of 45,222 lines. A pipe or
//! a terminal is what an agent reads, and keeps the filter.

use std::fs::File;
use std::path::Path;
use std::process::{Command, Output, Stdio};

mod common;

fn git(repo: &Path, home: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(repo);
    cmd.args(["-c", "commit.gpgsign=false", "-c", "maintenance.auto=false"]);
    cmd.args(args);
    common::isolate_git_in(&mut cmd, home);
    let out = cmd.output().expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

/// A repository of `commits` commits, more than `rtk git log` shows.
fn repo(root: &Path, commits: usize) -> (std::path::PathBuf, std::path::PathBuf) {
    let path = root.join("repo");
    let home = root.join("home");
    std::fs::create_dir_all(&path).expect("mkdir repo");
    std::fs::create_dir_all(&home).expect("mkdir home");
    git(&path, &home, &["init", "-q", "-b", "main"]);
    for i in 0..commits {
        let message = format!("commit {i}");
        git(
            &path,
            &home,
            &["commit", "-q", "--allow-empty", "-m", &message],
        );
    }
    (path, home)
}

fn rtk_git_log(repo: &Path, home: &Path, args: &[&str], stdout: Stdio) -> Output {
    let mut cmd = common::rtk_command();
    cmd.arg("git")
        .args(args)
        .current_dir(repo)
        .stdout(stdout)
        .stderr(Stdio::piped());
    common::isolate_git_in(&mut cmd, home);
    cmd.output().expect("run rtk git log")
}

#[test]
fn a_command_redirected_to_a_file_writes_its_own_output() {
    let root = common::tempdir();
    let (repo, home) = repo(root.path(), 12);
    let target = root.path().join("log.txt");

    let out = rtk_git_log(
        &repo,
        &home,
        &["log", "-p"],
        Stdio::from(File::create(&target).expect("create log.txt")),
    );
    assert!(
        out.status.success(),
        "rtk git log failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let written = std::fs::read(&target).expect("read log.txt");
    let native = git(&repo, &home, &["log", "-p"]).stdout;
    assert_eq!(
        String::from_utf8_lossy(&written),
        String::from_utf8_lossy(&native),
        "the file must hold git's own output"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("[rtk]"),
        "nothing was cut, so nothing may be announced: {stderr:?}"
    );
}

/// `--skip-env` is rtk's own flag wherever it is written, so git never sees it.
#[test]
fn rtks_own_flag_after_the_command_stays_out_of_the_programs_arguments() {
    let root = common::tempdir();
    let (repo, home) = repo(root.path(), 3);
    let target = root.path().join("log.txt");

    let out = rtk_git_log(
        &repo,
        &home,
        &["log", "--skip-env", "-p"],
        Stdio::from(File::create(&target).expect("create log.txt")),
    );
    assert!(
        out.status.success(),
        "rtk git log --skip-env failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let written = std::fs::read(&target).expect("read log.txt");
    let native = git(&repo, &home, &["log", "-p"]).stdout;
    assert_eq!(
        String::from_utf8_lossy(&written),
        String::from_utf8_lossy(&native)
    );
}

/// After `--`, `--skip-env` is the program's: here a path git finds no commits for.
#[test]
fn rtks_flag_after_a_double_dash_is_the_programs() {
    let root = common::tempdir();
    let (repo, home) = repo(root.path(), 3);
    let target = root.path().join("log.txt");

    let args = ["log", "-p", "--", "--skip-env"];
    let out = rtk_git_log(
        &repo,
        &home,
        &args,
        Stdio::from(File::create(&target).expect("create log.txt")),
    );
    assert!(
        out.status.success(),
        "rtk git log -- --skip-env failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let written = std::fs::read(&target).expect("read log.txt");
    let native = git(&repo, &home, &args).stdout;
    assert!(
        native.is_empty(),
        "git found commits for the path --skip-env"
    );
    assert_eq!(
        String::from_utf8_lossy(&written),
        String::from_utf8_lossy(&native)
    );
}

#[test]
fn a_command_read_through_a_pipe_is_still_filtered() {
    let root = common::tempdir();
    let (repo, home) = repo(root.path(), 12);

    let out = rtk_git_log(&repo, &home, &["log", "-p"], Stdio::piped());
    assert!(out.status.success(), "rtk git log failed");
    let shown = String::from_utf8_lossy(&out.stdout);
    let commits = shown.lines().filter(|l| l.starts_with("commit ")).count();
    assert!(
        commits < 12,
        "a pipe keeps the filter's cap; it showed {commits} of 12 commits"
    );
}

/// A TOML filter is rtk's own view too: the built-in `make` filter caps output at 50 lines.
#[cfg(unix)]
#[test]
fn a_toml_filtered_command_redirected_to_a_file_writes_its_own_output() {
    if Command::new("make").arg("--version").output().is_err() {
        eprintln!("make is not installed; skipped");
        return;
    }
    let root = common::tempdir();
    let dir = sixty_line_makefile(root.path());
    let target = dir.join("make.txt");

    let mut cmd = common::rtk_command();
    cmd.args(["make", "-s"])
        .current_dir(dir)
        .stdout(Stdio::from(File::create(&target).expect("create make.txt")));
    let status = cmd.status().expect("run rtk make");
    assert!(status.success(), "rtk make failed");

    assert_sixty_lines(&target);
}

/// The file test above with no descriptor free to copy stdout: 3 to 7 taken, a limit of 8,
/// and 0 and 2 closed. Rust's runtime reopens 0 and 2 on `/dev/null` before `main`, so rtk
/// starts with none free and cannot start a filtered child either. Whatever rtk does, the
/// file holds make's whole output or nothing, never a filtered cut.
#[cfg(unix)]
#[test]
fn a_file_never_holds_filtered_output_when_no_descriptor_is_free() {
    if Command::new("make").arg("--version").output().is_err() {
        eprintln!("make is not installed; skipped");
        return;
    }
    let root = common::tempdir();
    let dir = sixty_line_makefile(root.path());
    let target = dir.join("make.txt");

    let rtk = common::rtk_command();
    let mut cmd = Command::new("sh");
    common::isolate_rtk(&mut cmd);
    // The limit comes after the redirections: bash copies descriptors to make them.
    cmd.arg("-c")
        .arg(
            "exec 3</dev/null 4</dev/null 5</dev/null 6</dev/null 7</dev/null 0<&- 2>&- \
             && ulimit -n 8 && exec \"$0\" make -s",
        )
        .arg(rtk.get_program())
        .current_dir(dir)
        .stdout(Stdio::from(File::create(&target).expect("create make.txt")));
    let status = cmd.status().expect("run sh");

    if status.success() {
        assert_sixty_lines(&target);
    } else {
        let written = std::fs::read_to_string(&target).expect("read make.txt");
        assert_eq!(written, "", "rtk failed, yet wrote to the file");
    }
}

#[cfg(unix)]
fn sixty_line_makefile(dir: &Path) -> &Path {
    std::fs::write(
        dir.join("Makefile"),
        "all:\n\t@i=1; while [ $$i -le 60 ]; do echo line $$i; i=$$((i+1)); done\n",
    )
    .expect("write Makefile");
    dir
}

#[cfg(unix)]
fn assert_sixty_lines(target: &Path) {
    let written = std::fs::read_to_string(target).expect("read make.txt");
    let expected: String = (1..=60).map(|i| format!("line {i}\n")).collect();
    assert_eq!(written, expected, "the file must hold make's own output");
}
