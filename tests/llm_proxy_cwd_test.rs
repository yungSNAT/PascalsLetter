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

/// Runs `rtk llm-proxy start` with a stand-in that never answers the health
/// check, so start fails and names the log the stand-in's output went to.
/// Returns the log's path and text.
fn start_and_read_log(cmd: &mut Command) -> (PathBuf, String) {
    let out = cmd
        .args(["llm-proxy", "start", "--bind", "127.0.0.1:1"])
        .output()
        .expect("rtk llm-proxy start");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{out:?}");
    let log = PathBuf::from(
        stderr
            .rsplit_once("see ")
            .unwrap_or_else(|| panic!("no log path in {stderr}"))
            .1
            .trim(),
    );
    let text = std::fs::read_to_string(&log).expect("read the daemon's log");
    (log, text)
}

#[test]
fn the_daemon_starts_in_the_state_directory_not_the_shells() {
    // The stand-in prints its working directory to the log.
    let (mut cmd, _dir, _project) = rtk_beside_stand_in("#!/bin/sh\npwd -P\n");
    let (log, cwd) = start_and_read_log(&mut cmd);
    let state_dir = std::fs::canonicalize(log.parent().unwrap()).unwrap();
    assert_eq!(Path::new(cwd.trim()), state_dir);
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

/// The stand-in's working directory, then the file its `RTK_PROXY_CONFIG`
/// names from there.
const PRINT_CONFIG: &str = "#!/bin/sh\npwd -P\nprintf '%s\\n' \"$RTK_PROXY_CONFIG\"\n";

/// The file a stand-in running [`PRINT_CONFIG`] would read.
fn config_read(output: &str) -> PathBuf {
    let (cwd, config) = output.trim().split_once('\n').expect("2 lines");
    std::fs::canonicalize(Path::new(cwd).join(config))
        .unwrap_or_else(|e| panic!("{config} from {cwd}: {e}"))
}

/// `RTK_PROXY_CONFIG` is the user's own choice, so a relative one still names
/// a file in the shell's directory, for a forwarded subcommand and for start,
/// and an absolute one is passed on as it is.
#[test]
fn a_relative_rtk_proxy_config_still_names_a_file_in_the_shells_directory() {
    for (relative, start) in [(true, false), (false, false), (true, true)] {
        let (mut cmd, _dir, project) = rtk_beside_stand_in(PRINT_CONFIG);
        let custom = project.path().join("custom.toml");
        std::fs::write(&custom, "").expect("write custom.toml");
        let custom = std::fs::canonicalize(custom).unwrap();
        let config = if relative {
            "custom.toml"
        } else {
            custom.to_str().unwrap()
        };
        cmd.env("RTK_PROXY_CONFIG", config);
        let output = if start {
            start_and_read_log(&mut cmd).1
        } else {
            let out = cmd
                .args(["llm-proxy", "service", "install"])
                .output()
                .expect("rtk llm-proxy service install");
            assert!(out.status.success(), "{out:?}");
            String::from_utf8_lossy(&out.stdout).into_owned()
        };
        let command = if start { "start" } else { "service install" };
        assert_eq!(config_read(&output), custom, "{command} with {config}");
    }
}

/// rtk-proxy reads `RTK_PROXY_CONFIG` only as Unicode and treats anything else
/// as unset, so a relative one that would resolve to a path that is not is
/// refused rather than passed on to be ignored. Linux only: macOS refuses such
/// a folder name.
#[cfg(target_os = "linux")]
#[test]
fn a_config_path_rtk_proxy_cannot_read_is_refused() {
    use std::os::unix::ffi::OsStrExt;
    let (mut cmd, _dir, project) = rtk_beside_stand_in(PRINT_CONFIG);
    let odd = project
        .path()
        .join(std::ffi::OsStr::from_bytes(b"not-unicode-\xff"));
    std::fs::create_dir(&odd).expect("create a folder named with a non-UTF-8 byte");
    std::fs::write(odd.join("custom.toml"), "").expect("write custom.toml");
    let out = cmd
        .current_dir(&odd)
        .env("RTK_PROXY_CONFIG", "custom.toml")
        .args(["llm-proxy", "service", "install"])
        .output()
        .expect("rtk llm-proxy service install");
    assert!(!out.status.success(), "{out:?}");
    assert!(out.stdout.is_empty(), "rtk-proxy ran: {out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("not valid Unicode"),
        "{out:?}"
    );
}
