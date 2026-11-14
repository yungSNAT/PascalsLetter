// Unix only: on Windows the home and configuration directories are known folders, which
// the environment cannot redirect.
#![cfg(unix)]
//! `rtk init -g` says the Claude Code hook is registered only when settings.json
//! holds it, and `rtk init --show` agrees with what init printed.

use std::path::Path;
use std::process::{Command, Stdio};

mod common;

/// rtk's stdout for `args`, with `home` as its home and no terminal, so the
/// settings.json prompt defaults to No.
fn run(home: &Path, args: &[&str]) -> String {
    run_with(common::rtk_command_in(home), args)
}

/// [`run`], by `rtk`.
fn run_with(mut rtk: Command, args: &[&str]) -> String {
    let out = rtk
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

/// `rtk init -g --auto-patch` under a user pipeline that needs no hook event,
/// with `settings` as an existing settings.json: the home and init's stdout.
#[cfg(feature = "engine")]
fn init_without_hook_events(settings: Option<&str>) -> (tempfile::TempDir, String) {
    let home = common::tempdir();
    let config = home.path().join("config");
    std::fs::create_dir_all(&config).expect("mkdir config");
    std::fs::write(
        config.join("pipeline.toml"),
        "schema_version = 1\n[pipeline]\nname = \"no-hook\"\n[override.rtk-rewrite]\nenabled = false\n",
    )
    .expect("write pipeline");
    if let Some(settings) = settings {
        std::fs::create_dir_all(home.path().join(".claude")).expect("mkdir .claude");
        std::fs::write(home.path().join(".claude/settings.json"), settings)
            .expect("write settings");
    }
    let mut rtk = common::rtk_command_in(home.path());
    rtk.env("RTK_CONFIG_DIR", &config);
    let init = run_with(rtk, &["init", "-g", "--auto-patch"]);
    (home, init)
}

/// A pipeline that needs no hook event leaves settings.json untouched, and init
/// says the hook is not needed rather than that it is registered.
#[cfg(feature = "engine")]
#[test]
fn a_pipeline_without_hook_events_is_reported_as_not_needing_the_hook() {
    let (home, init) = init_without_hook_events(None);
    assert!(init.contains("requires no hook events"), "{init}");
    assert!(
        init.contains("RTK hook not needed: the active pipeline requires no hook events."),
        "{init}"
    );
    assert!(!init.contains("RTK hook registered"), "{init}");
    assert!(!init.contains("hook already present"), "{init}");
    assert!(!init.contains("RTK hook not registered"), "{init}");
    assert!(!home.path().join(".claude/settings.json").exists());
}

/// A hook registered before the pipeline stopped needing it stays, and init
/// does not call it unregistered.
#[cfg(feature = "engine")]
#[test]
fn a_hook_the_pipeline_no_longer_needs_is_not_reported_as_unregistered() {
    let settings = r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"rtk hook claude"}]}]}}"#;
    let (home, init) = init_without_hook_events(Some(settings));
    assert!(
        init.contains("PreToolUse no longer required by the pipeline"),
        "{init}"
    );
    assert!(init.contains("RTK hook not needed"), "{init}");
    assert!(!init.contains("RTK hook not registered"), "{init}");
    assert!(!init.contains("RTK hook registered"), "{init}");
    assert_eq!(
        std::fs::read_to_string(home.path().join(".claude/settings.json")).expect("settings"),
        settings
    );
}
