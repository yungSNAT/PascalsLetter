//! `rtk git add` keeps what git says: `-v`'s list of the files it added, and a
//! warning git prints while a call succeeds, such as a line-ending conversion.

use std::path::Path;
use std::process::{Command, Output};

mod common;

fn git(repo: &Path) -> Command {
    let mut cmd = common::native_command("git");
    common::isolate_git(&mut cmd);
    cmd.current_dir(repo);
    cmd
}

fn rtk_git_add(repo: &Path, args: &[&str]) -> Output {
    let out = common::rtk_command()
        .current_dir(repo)
        .env("LC_ALL", "C")
        .args(["git", "add"])
        .args(args)
        .output()
        .expect("rtk git add");
    assert!(out.status.success(), "rtk git add {args:?}: {out:?}");
    out
}

#[test]
fn git_add_verbose_keeps_gits_list_of_added_files() {
    let repo = common::temp_git_repo();
    std::fs::write(repo.path().join("new.txt"), "new\n").expect("write");

    let out = rtk_git_add(repo.path(), &["-v", "new.txt"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("add 'new.txt'"), "{out:?}");
}

#[test]
fn a_successful_git_add_keeps_gits_warning() {
    let repo = common::temp_git_repo();
    let ok = git(repo.path())
        .args(["config", "core.autocrlf", "true"])
        .status();
    assert!(ok.expect("git config").success());
    std::fs::write(repo.path().join("lf.txt"), "line\n").expect("write");

    let out = rtk_git_add(repo.path(), &["lf.txt"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stdout.starts_with("ok 1 file changed"), "{out:?}");
    assert!(stderr.contains("LF will be replaced by CRLF"), "{out:?}");
}
