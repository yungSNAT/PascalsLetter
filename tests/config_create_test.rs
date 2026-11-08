#![cfg(target_os = "linux")]
//! `rtk config --create` writes the default config.toml and refuses to replace
//! one that exists. Linux only: the test gives rtk a config directory of its
//! own through `XDG_CONFIG_HOME`, which `dirs` reads only there.

mod common;

#[test]
fn config_create_keeps_an_existing_config() {
    let dir = common::tempdir();
    let create = || {
        common::rtk_command()
            .env("XDG_CONFIG_HOME", dir.path())
            .args(["config", "--create"])
            .output()
            .expect("rtk config --create")
    };
    let path = dir.path().join("rtk").join("config.toml");

    let first = create();
    assert!(first.status.success(), "first run: {first:?}");
    assert!(path.is_file(), "first run creates {}", path.display());

    let own = "[hooks]\nexclude_commands = [\"git\"]\n";
    std::fs::write(&path, own).expect("write the user's config");
    let second = create();
    assert_eq!(second.status.code(), Some(1), "second run: {second:?}");
    assert!(
        String::from_utf8_lossy(&second.stderr).contains("already exists"),
        "second run: {second:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&path).expect("read config"),
        own,
        "the user's config was replaced"
    );
}
