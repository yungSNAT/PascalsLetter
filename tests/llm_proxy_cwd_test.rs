// Unix only: the stand-in daemon is a shell script.
#![cfg(unix)]
//! `rtk llm-proxy start` starts the daemon in its state directory, not the
//! shell's: the daemon reads `./rtk-proxy.toml`, and one in a cloned project
//! could set the upstream that the relayed provider key is sent to.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

mod common;

#[test]
fn the_daemon_starts_in_the_state_directory_not_the_shells() {
    // rtk looks for the daemon next to its own executable, so run a copy beside
    // a stand-in that prints its working directory, which goes to the log,
    // and exits without ever answering the health check.
    let built = PathBuf::from(common::rtk_command().get_program());
    let dir = common::tempdir();
    let rtk = dir.path().join(built.file_name().unwrap());
    std::fs::copy(&built, &rtk).expect("copy rtk");
    let daemon = dir.path().join("rtk-proxy");
    std::fs::write(&daemon, "#!/bin/sh\npwd -P\n").expect("write rtk-proxy");
    std::fs::set_permissions(&daemon, std::fs::Permissions::from_mode(0o755))
        .expect("chmod rtk-proxy");

    let mut cmd = std::process::Command::new(&rtk);
    common::isolate_rtk(&mut cmd);
    let project = common::tempdir();
    std::fs::write(
        project.path().join("rtk-proxy.toml"),
        "anthropic_upstream = \"http://127.0.0.1:1\"\n",
    )
    .expect("write rtk-proxy.toml");
    let out = cmd
        .current_dir(project.path())
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
