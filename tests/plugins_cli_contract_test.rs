//! Exercise plugin scaffolding and Cargo orchestration inside an owned Linux home.
#![cfg(all(target_os = "linux", feature = "engine"))]

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Child, Output, Stdio};
use std::time::{Duration, Instant};

struct OwnedChild(Child);

impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Fixture {
    root: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let root = common::tempdir();
        for name in [
            "home",
            "config",
            "data",
            "project",
            "cargo",
            "bin",
            "log",
            "sdk/plugin",
        ] {
            fs::create_dir_all(root.path().join(name)).unwrap();
        }
        let cargo = root.path().join("bin/cargo");
        // Only shell builtins run here. PATH contains this directory alone, so
        // a fixture failure cannot fall back to a real Cargo installation.
        fs::write(
            &cargo,
            "#!/bin/sh\nset -eu\ncase \"${1-}\" in\n  build|install) phase=$1 ;;\n  *) exit 97 ;;\nesac\nprintf '%s\\0' \"$@\" > \"$PUBLIC_CARGO_LOG/$phase.args\"\nprintf '%s\\n' \"$PWD\" > \"$PUBLIC_CARGO_LOG/$phase.cwd\"\nprintf '%s\\n' \"$phase\" >> \"$PUBLIC_CARGO_LOG/calls\"\ncase \"$phase\" in\n  build) exit \"$PUBLIC_CARGO_BUILD_STATUS\" ;;\n  install) exit \"$PUBLIC_CARGO_INSTALL_STATUS\" ;;\nesac\n",
        )
        .unwrap();
        fs::set_permissions(cargo, fs::Permissions::from_mode(0o700)).unwrap();
        Self { root }
    }

    fn plugins(&self) -> PathBuf {
        self.root.path().join("config/rtk/plugins")
    }

    fn run(&self, args: &[&str], build_status: u8, install_status: u8) -> Output {
        let capture = common::tempdir();
        let stdout = capture.path().join("stdout");
        let stderr = capture.path().join("stderr");
        let mut command = common::rtk_command();
        let root = self.root.path();
        let mut child = OwnedChild(
            command
                .arg("plugins")
                .args(args)
                .current_dir(root.join("project"))
                .env("HOME", root.join("home"))
                .env("XDG_CONFIG_HOME", root.join("config"))
                .env("XDG_DATA_HOME", root.join("data"))
                .env("CARGO_HOME", root.join("cargo"))
                .env("CARGO_NET_OFFLINE", "true")
                .env("PATH", root.join("bin"))
                .env("RTK_PLUGIN_SOURCE", root.join("sdk"))
                .env("PUBLIC_CARGO_LOG", root.join("log"))
                .env("PUBLIC_CARGO_BUILD_STATUS", build_status.to_string())
                .env("PUBLIC_CARGO_INSTALL_STATUS", install_status.to_string())
                .stdin(Stdio::null())
                .stdout(Stdio::from(fs::File::create(&stdout).unwrap()))
                .stderr(Stdio::from(fs::File::create(&stderr).unwrap()))
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                return Output {
                    status,
                    stdout: fs::read(&stdout).unwrap(),
                    stderr: fs::read(&stderr).unwrap(),
                };
            }
            assert!(
                Instant::now() < deadline,
                "Owned plugin CLI exceeded its deadline"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn scaffold(&self, name: &str) {
        let result = self.run(&["new", name], 0, 0);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }

    fn calls(&self) -> Vec<String> {
        let log = self.root.path().join("log/calls");
        if !log.exists() {
            return Vec::new();
        }
        fs::read_to_string(log)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn assert_cargo_call(&self, phase: &str, expected: &[&str]) {
        let log = self.root.path().join("log");
        let args = fs::read(log.join(format!("{phase}.args"))).unwrap();
        let expected: Vec<u8> = expected
            .iter()
            .flat_map(|arg| arg.bytes().chain([0]))
            .collect();
        assert_eq!(args, expected);
        assert_eq!(
            fs::read_to_string(log.join(format!("{phase}.cwd"))).unwrap(),
            format!("{}\n", self.plugins().join(".host").display())
        );
    }
}

#[test]
fn host_manifest_keeps_plugin_packages_distinct_from_sdk_dependencies() {
    let fixture = Fixture::new();
    for name in ["std", "rtk-plugin", "rtk"] {
        fixture.scaffold(name);
    }
    let result = fixture.run(&["build"], 0, 0);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let host = fixture.plugins().join(".host");
    let manifest = fs::read_to_string(host.join("Cargo.toml")).unwrap();
    let manifest: toml::Value =
        toml::from_str(&manifest).expect("generated host must be valid TOML");
    let dependencies = manifest["dependencies"].as_table().unwrap();
    assert_eq!(dependencies.len(), 5);
    assert_eq!(
        dependencies["rtk"]["path"].as_str().unwrap(),
        fixture.root.path().join("sdk/.").to_str().unwrap()
    );
    assert_eq!(
        dependencies["rtk-plugin"]["path"].as_str().unwrap(),
        fixture.root.path().join("sdk/plugin").to_str().unwrap()
    );
    let main = fs::read_to_string(host.join("src/main.rs")).unwrap();
    for (index, package) in ["rtk", "rtk-plugin", "std"].iter().enumerate() {
        let alias = format!("plugin_{index}");
        assert_eq!(dependencies[&alias]["package"].as_str().unwrap(), *package);
        assert_eq!(
            dependencies[&alias]["path"].as_str().unwrap(),
            fixture.plugins().join(package).to_str().unwrap()
        );
        assert!(main.contains(&format!("{alias}::__rtk_plugin_methods()")));
        assert!(main.contains(&format!("{alias}::__rtk_plugin_kinds()")));
    }
    assert!(main.contains("std::process::exit(rtk::entry())"));
    assert!(
        String::from_utf8(result.stdout)
            .unwrap()
            .contains("rtk, rtk-plugin, std")
    );
    assert_eq!(fixture.calls(), ["build", "install"]);
    fixture.assert_cargo_call("build", &["build", "--release"]);
    fixture.assert_cargo_call("install", &["install", "--path", ".", "--force"]);
}

#[test]
fn invalid_discovered_crate_is_rejected_before_host_creation() {
    let fixture = Fixture::new();
    let plugin = fixture.plugins().join("self");
    fs::create_dir_all(&plugin).unwrap();
    fs::write(
        plugin.join("Cargo.toml"),
        "[package]\nname='self'\nversion='0.1.0'\n",
    )
    .unwrap();
    let result = fixture.run(&["build"], 0, 0);
    assert!(!result.status.success());
    assert!(
        String::from_utf8(result.stderr)
            .unwrap()
            .contains("reserved Rust keyword")
    );
    assert!(!fixture.plugins().join(".host").exists());
    assert!(fixture.calls().is_empty());
}

#[test]
fn legacy_plugin_generating_struct_self_is_refused_before_host_creation() {
    // Before reserved names were refused, `rtk plugins new self_` wrote this crate.
    for name in ["self_", "self-", "Self_", "self-_"] {
        let fixture = Fixture::new();
        let plugin = fixture.plugins().join(name);
        fs::create_dir_all(plugin.join("src")).unwrap();
        let manifest = format!("[package]\nname='{name}'\nversion='0.1.0'\n");
        let lib = "struct Self;\n";
        fs::write(plugin.join("Cargo.toml"), &manifest).unwrap();
        fs::write(plugin.join("src/lib.rs"), lib).unwrap();

        let result = fixture.run(&["build"], 0, 0);
        assert!(!result.status.success(), "{name} reached cargo");
        let stderr = String::from_utf8(result.stderr).unwrap();
        assert!(stderr.contains(&plugin.display().to_string()), "{stderr}");
        assert!(stderr.contains("reserved Rust type `Self`"), "{stderr}");
        assert!(stderr.contains("rtk plugins new"), "{stderr}");
        assert!(!fixture.plugins().join(".host").exists());
        assert!(fixture.calls().is_empty());
        assert_eq!(fs::read_to_string(plugin.join("Cargo.toml")).unwrap(), manifest);
        assert_eq!(fs::read_to_string(plugin.join("src/lib.rs")).unwrap(), lib);
    }
}
