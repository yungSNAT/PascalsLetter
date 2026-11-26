#![cfg(feature = "engine")]

use super::*;
use crate::core::test_isolation;
use std::fs;

#[test]
fn scaffold_rejects_invalid_names_and_reports_a_blocked_directory() {
    let root = test_isolation::tempdir();
    test_isolation::with_root(root.path(), || {
        let plugins = plugins_dir().unwrap();
        for name in ["", "9plugin", "_plugin", "two words", "éclair"] {
            let error = run_new(name).unwrap_err();
            assert_eq!(
                error.to_string(),
                format!(
                    "plugin name must be alphanumeric with - or _, starting with a letter: `{name}`"
                )
            );
        }
        assert!(!plugins.exists());

        fs::create_dir_all(plugins.parent().unwrap()).unwrap();
        fs::write(&plugins, "blocking file").unwrap();

        let error = run_new("valid-plugin").unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("create {}", plugins.join("valid-plugin").display())
        );
        assert!(error.downcast_ref::<std::io::Error>().is_some());
        assert_eq!(fs::read_to_string(plugins).unwrap(), "blocking file");
    });
}

#[test]
fn scaffold_pins_the_upstream_release_tag_and_preserves_existing_files() {
    let root = test_isolation::tempdir();
    test_isolation::with_root(root.path(), || {
        user_env::with_vars(&[("RTK_PLUGIN_SOURCE", None)], || {
            let name = "release--preview_2";
            assert_eq!(run_new(name).unwrap(), 0);

            let dir = plugins_dir().unwrap().join(name);
            let manifest_path = dir.join("Cargo.toml");
            let manifest_text = fs::read_to_string(&manifest_path).unwrap();
            let manifest: toml::Value = toml::from_str(&manifest_text).unwrap();
            let dependency = &manifest["dependencies"]["rtk-plugin"];
            let version = env!("CARGO_PKG_VERSION");
            let release = version
                .split_once('+')
                .map_or(version, |(release, _)| release);
            let tag = format!("v{release}");
            assert_eq!(dependency["git"].as_str(), Some(GIT_URL));
            assert_eq!(dependency["tag"].as_str(), Some(tag.as_str()));
            assert!(dependency.get("path").is_none());

            let source = dir.join("src").join("lib.rs");
            assert!(
                fs::read_to_string(&source)
                    .unwrap()
                    .contains("rtk_plugin!(ReleasePreview2);")
            );
            fs::write(&source, "// user's edits\n").unwrap();

            let error = run_new(name).unwrap_err();
            assert_eq!(
                error.to_string(),
                format!("plugin `{name}` already exists at {}", dir.display())
            );
            assert_eq!(fs::read_to_string(source).unwrap(), "// user's edits\n");
            assert_eq!(fs::read_to_string(manifest_path).unwrap(), manifest_text);
        });
    });
}

#[test]
fn discovery_handles_absence_and_returns_only_visible_crates_in_order() {
    let root = test_isolation::tempdir();
    test_isolation::with_root(root.path(), || {
        let plugins = plugins_dir().unwrap();
        assert!(!plugins.exists());
        assert!(discover_crates().unwrap().is_empty());
        assert_eq!(run_list().unwrap(), 0);
        assert!(!plugins.exists());

        let host = plugins.join(".host");
        fs::create_dir_all(&host).unwrap();
        // Prevent a real Cargo invocation even if the empty-build guard regresses.
        fs::write(host.join("src"), "not a directory").unwrap();
        assert_eq!(
            run_build().unwrap_err().to_string(),
            "no plugins to build — scaffold one with `rtk plugins new <name>`"
        );

        for name in ["zeta", ".host", "alpha"] {
            let dir = plugins.join(name);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("Cargo.toml"), "[package]\n").unwrap();
        }
        fs::create_dir_all(plugins.join("incomplete").join("Cargo.toml")).unwrap();
        fs::write(plugins.join("notes.txt"), "not a crate").unwrap();

        assert_eq!(
            discover_crates().unwrap(),
            vec![
                ("alpha".to_string(), plugins.join("alpha")),
                ("zeta".to_string(), plugins.join("zeta")),
            ]
        );
        assert_eq!(run_list().unwrap(), 0);
    });
}

#[cfg(unix)]
#[test]
fn plugin_cli_lists_scaffolds_and_distinguishes_cargo_failures() {
    use std::os::unix::fs::PermissionsExt;

    if !test_isolation::rtk_binary_ready() {
        return;
    }

    let root = test_isolation::tempdir();
    let home = root.path().join("home");
    let tools = root.path().join("bin");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&tools).unwrap();
    let cargo = tools.join("cargo");
    fs::write(
        &cargo,
        "#!/bin/sh\n\
         printf '%s\\n' \"$@\" >> \"$RTK_TEST_CARGO_LOG\" || exit 2\n\
         [ \"$1\" != \"$RTK_TEST_FAIL_STEP\" ]\n",
    )
    .unwrap();
    fs::set_permissions(cargo, fs::Permissions::from_mode(0o700)).unwrap();

    let command = || {
        let mut cmd = test_isolation::rtk_command();
        let inherited_path = cmd
            .get_envs()
            .find(|(key, _)| *key == "PATH")
            .and_then(|(_, value)| value)
            .unwrap();
        let path = std::env::join_paths(
            std::iter::once(tools.clone()).chain(std::env::split_paths(inherited_path)),
        )
        .unwrap();
        cmd.env("PATH", path)
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("CARGO_HOME", home.join("cargo"))
            .env("RUSTUP_HOME", home.join("rustup"))
            .env("CARGO_NET_OFFLINE", "true")
            .env("RTK_PLUGIN_SOURCE", root.path().join("no-upstream"));
        cmd
    };

    let empty = command().args(["plugins", "list"]).output().unwrap();
    assert!(
        empty.status.success(),
        "{}",
        String::from_utf8_lossy(&empty.stderr)
    );
    let stdout = String::from_utf8_lossy(&empty.stdout);
    assert!(stdout.contains("no plugins — scaffold one with `rtk plugins new <name>`"));
    #[cfg(not(feature = "example-plugins"))]
    assert!(!stdout.contains("plugin methods linked into this binary:"));

    let name = "cover-build";
    let scaffold = command().args(["plugins", "new", name]).output().unwrap();
    assert!(
        scaffold.status.success(),
        "{}",
        String::from_utf8_lossy(&scaffold.stderr)
    );
    let prefix = format!("created plugin `{name}` at ");
    let dir = String::from_utf8_lossy(&scaffold.stdout)
        .lines()
        .find_map(|line| line.strip_prefix(&prefix))
        .map(PathBuf::from)
        .unwrap();
    let plugins = dir.parent().unwrap();
    let list = command().args(["plugins", "list"]).output().unwrap();
    assert!(
        list.status.success(),
        "{}",
        String::from_utf8_lossy(&list.stderr)
    );
    let stdout = String::from_utf8_lossy(&list.stdout);
    let heading = format!("plugin crates in {}:", plugins.display());
    let row = format!("  {name:<24} {}", dir.display());
    assert!(stdout.lines().any(|line| line == heading), "{stdout}");
    assert!(stdout.lines().any(|line| line == row), "{stdout}");
    assert!(!stdout.contains("no plugins"));

    let binary = plugins
        .join(".host")
        .join("target")
        .join("release")
        .join("rtk");
    for (stage, expected_args, diagnostic) in [
        (
            "build",
            "build\n--release\n",
            "plugin build failed".to_string(),
        ),
        (
            "install",
            "build\n--release\ninstall\n--path\n.\n--force\n",
            format!(
                "plugin install failed — the binary is at {}",
                binary.display()
            ),
        ),
    ] {
        let log = root.path().join(format!("cargo-{stage}.log"));
        let result = command()
            .env("RTK_TEST_CARGO_LOG", &log)
            .env("RTK_TEST_FAIL_STEP", stage)
            .args(["plugins", "build"])
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(!result.status.success(), "{stage} unexpectedly succeeded");
        assert!(stderr.contains(&diagnostic), "{stage}: {stderr}");
        let stdout = String::from_utf8_lossy(&result.stdout);
        assert!(
            stdout.contains("building a custom rtk with 1 plugin(s)..."),
            "{stdout}"
        );
        assert_eq!(
            stdout.contains("installing the custom rtk"),
            stage == "install"
        );
        assert_eq!(fs::read_to_string(log).unwrap(), expected_args);
    }
}
