// Unix only: the stand-in rtk is a shell script.
#![cfg(unix)]
//! The Hermes adapter rewrites a terminal command only on the backends
//! `RTK_HERMES_BACKENDS` names, `local` by default: a Docker or SSH backend may
//! have no `rtk`, and there the rewritten command would fail.

use std::os::unix::fs::PermissionsExt;

mod common;

const PLUGIN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/hooks/hermes/rtk-rewrite/__init__.py"
);

/// Loads the plugin as Hermes does, with the stand-in `rtk`'s directory first on
/// PATH, and prints the command it leaves for `git status`.
const HOST: &str = r#"
import importlib.util, os, sys
os.environ["PATH"] = sys.argv[2] + os.pathsep + os.environ.get("PATH", "")
spec = importlib.util.spec_from_file_location("rtk_rewrite", sys.argv[1])
plugin = importlib.util.module_from_spec(spec)
spec.loader.exec_module(plugin)
hooks = {}
class Context:
    def register_hook(self, name, callback):
        hooks[name] = callback
plugin.register(Context())
args = {"command": "git status"}
hooks["pre_tool_call"](tool_name="terminal", args=args)
print(args["command"])
"#;

/// The command Hermes would run for `git status` with these variables, with
/// a stand-in `rtk` that rewrites it to `rtk git status`.
fn command_run(vars: &[(&str, &str)]) -> String {
    let bin = common::tempdir();
    let rtk = bin.path().join("rtk");
    std::fs::write(&rtk, "#!/bin/sh\necho 'rtk git status'\n").expect("write rtk");
    std::fs::set_permissions(&rtk, std::fs::Permissions::from_mode(0o755)).expect("chmod rtk");
    let out = common::native_command("python3")
        .args(["-I", "-c", HOST, PLUGIN])
        .arg(bin.path())
        .envs(vars.iter().copied())
        .output()
        .expect("run python3");
    assert!(out.status.success(), "{out:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn a_command_on_a_remote_backend_runs_as_written() {
    for backend in ["docker", "ssh", "modal"] {
        assert_eq!(
            command_run(&[("TERMINAL_ENV", backend)]),
            "git status",
            "{backend}"
        );
    }
}

#[test]
fn a_local_command_is_rewritten() {
    assert_eq!(command_run(&[]), "rtk git status");
    assert_eq!(command_run(&[("TERMINAL_ENV", "local")]), "rtk git status");
}

#[test]
fn a_backend_named_in_rtk_hermes_backends_is_rewritten() {
    let vars = [
        ("TERMINAL_ENV", "docker"),
        ("RTK_HERMES_BACKENDS", "local, docker"),
    ];
    assert_eq!(command_run(&vars), "rtk git status");
}
