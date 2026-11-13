// Unix only: the stand-in daemon is a shell script.
#![cfg(unix)]
//! rtk runs `rtk-proxy` in its state directory, not the shell's: `rtk-proxy`
//! reads `./rtk-proxy.toml`, and one in a cloned project could set the upstream
//! that the relayed provider key is sent to. That holds for `rtk llm-proxy
//! start` and for every subcommand rtk passes on, such as `service install`,
//! which writes the upstream it read into the service's configuration.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

/// A copy of rtk beside a stand-in `rtk-proxy` running `script` (rtk looks for
/// the daemon next to its own executable), isolated and started from a project
/// holding a `rtk-proxy.toml`. Keep the 2 directories alive.
fn rtk_beside_stand_in(script: &str) -> (Command, tempfile::TempDir, tempfile::TempDir) {
    let built = PathBuf::from(common::rtk_command().get_program());
    let dir = common::tempdir();
    let rtk = dir.path().join(built.file_name().unwrap());
    std::fs::copy(&built, &rtk).expect("copy rtk");
    let daemon = dir.path().join("rtk-proxy");
    std::fs::write(&daemon, script).expect("write rtk-proxy");
    std::fs::set_permissions(&daemon, std::fs::Permissions::from_mode(0o755))
        .expect("chmod rtk-proxy");

    let mut cmd = Command::new(&rtk);
    common::isolate_rtk(&mut cmd);
    let project = common::tempdir();
    std::fs::write(
        project.path().join("rtk-proxy.toml"),
        "anthropic_upstream = \"http://127.0.0.1:1\"\n",
    )
    .expect("write rtk-proxy.toml");
    cmd.current_dir(project.path());
    (cmd, dir, project)
}

#[test]
fn the_daemon_starts_in_the_state_directory_not_the_shells() {
    // The stand-in prints its working directory, which goes to the log, and
    // exits without ever answering the health check.
    let (mut cmd, _dir, _project) = rtk_beside_stand_in("#!/bin/sh\npwd -P\n");
    let out = cmd
        .args(["llm-proxy", "start", "--bind", "127.0.0.1:1"])
        .output()
        .expect("rtk llm-proxy start");

    // The stand-in never becomes healthy, so start fails and names the log.
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{out:?}");
    let log = Path::new(
        stderr
            .rsplit_once("see ")
            .unwrap_or_else(|| panic!("no log path in {stderr}"))
            .1
            .trim(),
    );
    let cwd = std::fs::read_to_string(log).expect("read the daemon's log");
    let state_dir = std::fs::canonicalize(log.parent().unwrap()).unwrap();
    assert_eq!(Path::new(cwd.trim()), state_dir, "{stderr}");
}

#[test]
fn forwarded_subcommands_run_rtk_proxy_outside_the_shells_directory() {
    let scratch = std::fs::canonicalize(common::scratch_dir()).unwrap();
    for args in [
        &["service", "install"][..],
        &["config", "get"],
        &["stop"],
        &["status"],
        &["stats"],
    ] {
        let (mut cmd, _dir, project) = rtk_beside_stand_in("#!/bin/sh\npwd -P\n");
        let out = cmd
            .arg("llm-proxy")
            .args(args)
            .output()
            .expect("run rtk llm-proxy");
        assert!(out.status.success(), "{args:?}: {out:?}");
        let cwd = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
        assert_ne!(
            cwd,
            std::fs::canonicalize(project.path()).unwrap(),
            "{args:?} ran rtk-proxy in the shell's directory"
        );
        assert!(
            cwd.starts_with(&scratch) && cwd.ends_with("llm-proxy"),
            "{args:?} ran rtk-proxy in {}",
            cwd.display()
        );
    }
}

/// A relative `RTK_PROXY_CONFIG` is the user's own choice, so it still names a
/// file in the shell's directory.
#[test]
fn a_relative_rtk_proxy_config_still_names_a_file_in_the_shells_directory() {
    let (mut cmd, _dir, project) =
        rtk_beside_stand_in("#!/bin/sh\npwd -P\nprintf '%s\\n' \"$RTK_PROXY_CONFIG\"\n");
    std::fs::write(project.path().join("custom.toml"), "").expect("write custom.toml");
    let out = cmd
        .env("RTK_PROXY_CONFIG", "custom.toml")
        .args(["llm-proxy", "service", "install"])
        .output()
        .expect("rtk llm-proxy service install");
    assert!(out.status.success(), "{out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let (cwd, config) = stdout.trim().split_once('\n').expect("2 lines");
    assert_eq!(
        std::fs::canonicalize(Path::new(cwd).join(config)).unwrap(),
        std::fs::canonicalize(project.path().join("custom.toml")).unwrap(),
        "{stdout}"
    );
}
