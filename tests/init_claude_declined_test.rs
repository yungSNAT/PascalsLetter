// Unix only: on Windows the home and configuration directories are known folders, which
// the environment cannot redirect.
#![cfg(unix)]
//! `rtk init -g` says the Claude Code hook is registered only when settings.json
//! holds it, and `rtk init --show` agrees with what init printed.

use std::path::Path;
use std::process::Stdio;

mod common;

/// rtk's stdout for `args`, with `home` as its home and no terminal, so the
/// settings.json prompt defaults to No.
fn run(home: &Path, args: &[&str]) -> String {
    let out = common::rtk_command_in(home)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("run rtk");
    assert_eq!(out.status.code(), Some(0), "rtk {args:?}: {out:?}");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn a_declined_settings_patch_is_not_reported_as_registered() {
    let home = common::tempdir();

    let init = run(home.path(), &["init", "-g"]);
    assert!(!init.contains("RTK hook registered"), "{init}");
    assert!(init.contains("RTK hook not registered"), "{init}");
    assert!(!home.path().join(".claude/settings.json").exists());

    let show = run(home.path(), &["init", "--show"]);
    assert!(show.contains("[--] Hook: not found"), "{show}");
}

#[test]
fn an_accepted_settings_patch_is_reported_as_registered() {
    let home = common::tempdir();

    let init = run(home.path(), &["init", "-g", "--auto-patch"]);
    assert!(init.contains("RTK hook registered (global)."), "{init}");
    assert!(!init.contains("RTK hook not registered"), "{init}");

    let show = run(home.path(), &["init", "--show"]);
    assert!(show.contains("[ok] Hook:"), "{show}");
}

/// A pipeline that needs no hook event leaves settings.json untouched, and init
/// says so rather than that the hook is registered.
#[cfg(feature = "engine")]
#[test]
fn a_pipeline_without_hook_events_is_not_reported_as_registered() {
    let home = common::tempdir();
    // dirs::config_dir() resolves differently per platform; cover both.
    for dir in [".config/rtk", "Library/Application Support/rtk"] {
        let dir = home.path().join(dir);
        std::fs::create_dir_all(&dir).expect("mkdir config");
        std::fs::write(
            dir.join("pipeline.toml"),
            "schema_version = 1\n[pipeline]\nname = \"no-hook\"\n[override.rtk-rewrite]\nenabled = false\n",
        )
        .expect("write pipeline");
    }

    let init = run(home.path(), &["init", "-g", "--auto-patch"]);
    assert!(init.contains("requires no hook events"), "{init}");
    assert!(!init.contains("RTK hook registered"), "{init}");
    assert!(!init.contains("hook already present"), "{init}");
    assert!(init.contains("RTK hook not registered"), "{init}");
    assert!(!home.path().join(".claude/settings.json").exists());
}
