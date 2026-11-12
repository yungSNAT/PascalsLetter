// Unix only: on Windows the home and configuration directories are known folders, which
// the environment cannot redirect.
#![cfg(unix)]
//! `rtk init -g` says the Claude Code hook is registered only when settings.json
//! holds it, and `rtk init --show` agrees with what init printed.

use std::path::Path;
use std::process::{Command, Stdio};

mod common;

fn rtk(project: &Path, home: &Path) -> Command {
    let mut cmd = common::rtk_command();
    cmd.current_dir(project)
        .env("HOME", home)
        .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
        .env("RTK_DB_PATH", home.join("rtk.db"))
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local").join("share"))
        .env("RTK_TELEMETRY_DISABLED", "1")
        .env("LC_ALL", "C")
        // No terminal: the settings.json prompt defaults to No.
        .stdin(Stdio::null());
    cmd
}

fn run(project: &Path, home: &Path, args: &[&str]) -> String {
    let out = rtk(project, home).args(args).output().expect("run rtk");
    assert_eq!(out.status.code(), Some(0), "rtk {args:?}: {out:?}");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn a_declined_settings_patch_is_not_reported_as_registered() {
    let project = common::tempdir();
    let home = common::tempdir();

    let init = run(project.path(), home.path(), &["init", "-g"]);
    assert!(!init.contains("RTK hook registered"), "{init}");
    assert!(init.contains("RTK hook not registered"), "{init}");
    assert!(!home.path().join(".claude/settings.json").exists());

    let show = run(project.path(), home.path(), &["init", "--show"]);
    assert!(show.contains("[--] Hook: not found"), "{show}");
}

#[test]
fn an_accepted_settings_patch_is_reported_as_registered() {
    let project = common::tempdir();
    let home = common::tempdir();

    let init = run(project.path(), home.path(), &["init", "-g", "--auto-patch"]);
    assert!(init.contains("RTK hook registered (global)."), "{init}");
    assert!(!init.contains("RTK hook not registered"), "{init}");

    let show = run(project.path(), home.path(), &["init", "--show"]);
    assert!(show.contains("[ok] Hook:"), "{show}");
}
