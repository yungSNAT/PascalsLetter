//! `rtk pipeline add` installs exactly one complete pipeline, and a share card
//! that cannot be installed that way does not offer an install line. `gh` is
//! stubbed on PATH, so the gist tests read what `gh gist create` received.

#![cfg(all(unix, feature = "engine"))]

mod common;

use std::path::Path;

const OUTER: &str = "[pipeline]\nname = \"outer\"\n\n[[directive]]\nname = \"examples\"\ntext = '''\n```toml\n[pipeline]\nname = \"inner\"\n```\n'''\n";

fn rtk(root: &Path, path: &str, args: &[&str]) -> std::process::Output {
    common::rtk_command_in(root)
        .env("RTK_CONFIG_DIR", root.join("config"))
        .env("PATH", path)
        .args(args)
        .output()
        .expect("spawn rtk")
}

fn installed(root: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root.join("config").join("pipelines")) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn system_path() -> String {
    std::env::var("PATH").unwrap_or_default()
}

#[test]
fn raw_toml_whose_strings_hold_a_fenced_example_installs_whole() {
    let root = common::tempdir();
    let file = root.path().join("outer.toml");
    std::fs::write(&file, OUTER).unwrap();

    let output = rtk(
        root.path(),
        &system_path(),
        &["pipeline", "add", file.to_str().unwrap(), "--force"],
    );

    assert!(output.status.success(), "{output:?}");
    assert_eq!(installed(root.path()), ["outer.toml"]);
    assert_eq!(
        std::fs::read_to_string(root.path().join("config/pipelines/outer.toml")).unwrap(),
        OUTER
    );
}

#[test]
fn a_card_with_more_than_one_toml_block_installs_nothing() {
    let root = common::tempdir();
    let card = root.path().join("card.md");
    std::fs::write(
        &card,
        "# rtk pipeline: merged\n\n```toml\n[pipeline]\nname = \"base\"\n```\n\n```toml\n[pipeline]\nname = \"overlay\"\n```\n",
    )
    .unwrap();

    let output = rtk(
        root.path(),
        &system_path(),
        &["pipeline", "add", card.to_str().unwrap(), "--force"],
    );

    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("multiple TOML blocks"), "{stderr}");
    assert!(installed(root.path()).is_empty());
}

/// Shares the active pipeline as a gist with `pipelines` installed. With
/// `gh` set, a stub records its argv and prints a gist URL; without it, PATH
/// has no `gh`. Returns stdout and the recorded argv.
fn share_gist(pipelines: &[&str], gh: bool) -> (String, String) {
    let root = common::tempdir();
    let dir = root.path().join("config").join("pipelines");
    std::fs::create_dir_all(&dir).unwrap();
    for name in pipelines {
        std::fs::write(
            dir.join(format!("{name}.toml")),
            format!("[pipeline]\nname = \"{name}\"\nextends = \"rtk:default\"\n"),
        )
        .unwrap();
    }
    let bin = root.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let argv = root.path().join("gh-argv.txt");
    if gh {
        let stub = bin.join("gh");
        std::fs::write(
            &stub,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" >> '{}'\necho https://gist.github.com/example/0123\n",
                argv.display()
            ),
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&stub).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
        std::fs::set_permissions(&stub, permissions).unwrap();
    }
    // Without the stub, PATH holds only the empty bin directory, so no `gh` is found.
    let path = if gh {
        format!("{}:{}", bin.display(), system_path())
    } else {
        bin.display().to_string()
    };

    let output = rtk(
        root.path(),
        &path,
        &["pipeline", "share", "--gist", "--force"],
    );

    assert!(output.status.success(), "{output:?}");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        std::fs::read_to_string(&argv).unwrap_or_default(),
    )
}

#[test]
fn a_gist_of_several_sources_offers_no_install_line() {
    let (stdout, argv) = share_gist(&["base", "overlay"], true);
    assert!(argv.contains("rtk configuration reference (multiple sources)"), "{argv}");
    assert!(!argv.contains("install: rtk pipeline add"), "{argv}");
    assert!(
        stdout.contains("installation requires one complete pipeline TOML file"),
        "{stdout}"
    );
    assert!(!stdout.contains("others install it with"), "{stdout}");

    let (stdout, _) = share_gist(&["base", "overlay"], false);
    assert!(stdout.contains("as a configuration reference"), "{stdout}");
    assert!(!stdout.contains("others install with"), "{stdout}");
}

#[test]
fn a_gist_of_one_source_keeps_its_install_line() {
    let (stdout, argv) = share_gist(&["solo"], true);
    assert!(argv.contains("install: rtk pipeline add <this url>"), "{argv}");
    assert!(
        stdout.contains("others install it with:  rtk pipeline add https://gist.github.com/example/0123"),
        "{stdout}"
    );

    let (stdout, _) = share_gist(&["solo"], false);
    assert!(
        stdout.contains("others install with: rtk pipeline add <gist-url>"),
        "{stdout}"
    );
}
