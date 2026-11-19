//! The install lifecycle of a pipeline from a local file: the refusal of a
//! pipeline that runs credential commands, the non-interactive opt-ins, the
//! shadow slot, `rtk pipeline promote` and `rtk pipeline eval`.

#![cfg(all(unix, feature = "engine"))]

mod common;

use std::path::{Path, PathBuf};
use std::process::Output;

const V1: &str = "[pipeline]\nname = \"tidy\"\nextends = \"rtk:default\"\n";
const V2: &str = "[pipeline]\nname = \"tidy\"\nextends = \"rtk:default\"\n\n[[directive]]\nname = \"tone\"\ntext = \"be brief\"\n";

fn rtk(root: &Path, args: &[&str]) -> Output {
    common::rtk_command_in(root)
        .env("RTK_CONFIG_DIR", root.join("config"))
        .args(args)
        .output()
        .expect("spawn rtk")
}

fn source(root: &Path, text: &str) -> String {
    let file = root.join("tidy-source.toml");
    std::fs::write(&file, text).unwrap();
    file.to_str().unwrap().to_string()
}

fn live(root: &Path) -> PathBuf {
    root.join("config/pipelines/tidy.toml")
}

fn shadow(root: &Path) -> PathBuf {
    root.join("config/pipelines/shadow/tidy.toml")
}

fn read(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// rtk's data directory in the child: `dirs::data_local_dir()` follows
/// `XDG_DATA_HOME` (the root) on Linux and is `~/Library/Application Support`
/// on macOS.
fn data_dir(root: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        root.join("Library/Application Support/rtk")
    } else {
        root.join("rtk")
    }
}

/// The pipeline `rtk pipeline eval` is collecting for, if any.
fn candidate(root: &Path) -> Option<String> {
    let text = read(&data_dir(root).join("pipeline-eval.json"))?;
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    v["candidate"].as_str().map(str::to_string)
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn ok(output: &Output) -> String {
    assert!(output.status.success(), "{output:?}");
    text(&output.stdout)
}

fn refused(output: &Output) -> String {
    assert!(!output.status.success(), "{output:?}");
    text(&output.stderr)
}

#[test]
fn a_pipeline_that_runs_a_credential_command_is_never_installed() {
    let root = common::tempdir();
    let root = root.path();
    let file = source(
        root,
        "[pipeline]\nname = \"tidy\"\n\n[upstream.w]\nprovider = \"anthropic\"\nmodel = \"m\"\napi_key_command = \"cat ~/.key\\u001b[2J\"\n",
    );

    for flags in [&["--force"][..], &["--shadow"][..]] {
        let mut args = vec!["pipeline", "add", file.as_str()];
        args.extend_from_slice(flags);
        let output = rtk(root, &args);

        assert_eq!(output.status.code(), Some(1), "{output:?}");
        let stderr = text(&output.stderr);
        assert!(stderr.contains("Refused: api_key_command"), "{stderr}");
        assert!(stderr.contains("upstream `w`: cat ~/.key"), "{stderr}");
        assert!(
            !stderr.contains('\u{1b}') && !stderr.contains("[2J"),
            "{stderr:?}"
        );
    }
    assert_eq!(read(&live(root)), None);
    assert_eq!(read(&shadow(root)), None);
    assert_eq!(candidate(root), None);
}

#[test]
fn installing_without_a_terminal_needs_force_or_shadow() {
    let root = common::tempdir();
    let root = root.path();
    let file = source(root, V1);

    let stderr = refused(&rtk(root, &["pipeline", "add", &file]));
    assert!(
        stderr.contains("installing non-interactively needs an explicit opt-in"),
        "{stderr}"
    );
    assert_eq!(read(&live(root)), None);

    let stdout = ok(&rtk(root, &["pipeline", "add", &file, "--force"]));
    assert!(stdout.contains("installed: "), "{stdout}");
    assert_eq!(read(&live(root)).as_deref(), Some(V1));

    let stdout = ok(&rtk(root, &["pipeline", "add", &file]));
    assert!(
        stdout.contains("`tidy` is already installed and identical — nothing to change"),
        "{stdout}"
    );
}

#[test]
fn a_shadow_install_leaves_the_live_version_alone_until_discarded() {
    let root = common::tempdir();
    let root = root.path();
    ok(&rtk(
        root,
        &["pipeline", "add", &source(root, V1), "--force"],
    ));

    let stdout = ok(&rtk(
        root,
        &["pipeline", "add", &source(root, V2), "--shadow"],
    ));
    assert!(stdout.contains("trying `tidy` silently"), "{stdout}");
    assert!(
        stdout.contains("your currently installed `tidy` stays live"),
        "{stdout}"
    );
    assert_eq!(read(&live(root)).as_deref(), Some(V1));
    assert_eq!(read(&shadow(root)).as_deref(), Some(V2));
    assert_eq!(candidate(root).as_deref(), Some("tidy"));

    let stdout = ok(&rtk(root, &["pipeline", "promote", "tidy", "--discard"]));
    assert!(
        stdout.contains("discarded the shadow install of `tidy`"),
        "{stdout}"
    );
    assert_eq!(read(&shadow(root)), None);
    assert_eq!(candidate(root), None);
    assert_eq!(read(&live(root)).as_deref(), Some(V1));
}

#[test]
fn promoting_a_shadow_install_replaces_the_live_version_only_when_forced() {
    let root = common::tempdir();
    let root = root.path();
    ok(&rtk(
        root,
        &["pipeline", "add", &source(root, V1), "--force"],
    ));
    ok(&rtk(
        root,
        &["pipeline", "add", &source(root, V2), "--shadow"],
    ));

    let stderr = refused(&rtk(root, &["pipeline", "promote", "tidy"]));
    assert!(
        stderr.contains("refusing to promote non-interactively"),
        "{stderr}"
    );
    assert_eq!(read(&live(root)).as_deref(), Some(V1));
    assert_eq!(read(&shadow(root)).as_deref(), Some(V2));

    let stdout = ok(&rtk(root, &["pipeline", "promote", "tidy", "--force"]));
    assert!(
        stdout.contains("promoting replaces the live `tidy` — the change:"),
        "{stdout}"
    );
    assert!(stdout.contains("  + name = \"tone\""), "{stdout}");
    assert!(stdout.contains("`tidy` is live"), "{stdout}");
    assert_eq!(read(&live(root)).as_deref(), Some(V2));
    assert_eq!(read(&shadow(root)), None);
    assert_eq!(candidate(root), None);
}

#[test]
fn promote_names_what_is_missing() {
    let root = common::tempdir();
    let root = root.path();

    let stderr = refused(&rtk(root, &["pipeline", "promote", "tidy", "--force"]));
    assert!(stderr.contains("no shadow install of `tidy`"), "{stderr}");

    let stderr = refused(&rtk(root, &["pipeline", "promote", "../tidy", "--force"]));
    assert!(stderr.contains("must be alphanumeric"), "{stderr}");
    assert_eq!(read(&live(root)), None);
}

#[test]
fn eval_start_and_stop_arm_and_clear_the_candidate() {
    let root = common::tempdir();
    let root = root.path();

    let stderr = refused(&rtk(root, &["pipeline", "eval", "tidy", "--start"]));
    assert!(stderr.contains("no installed pipeline `tidy`"), "{stderr}");
    assert_eq!(candidate(root), None);

    ok(&rtk(
        root,
        &["pipeline", "add", &source(root, V1), "--force"],
    ));
    let stdout = ok(&rtk(root, &["pipeline", "eval", "tidy"]));
    assert!(
        stdout.contains("candidate      no measurements yet"),
        "{stdout}"
    );
    assert!(
        stdout.contains("start with: rtk pipeline eval tidy --start"),
        "{stdout}"
    );

    let stdout = ok(&rtk(root, &["pipeline", "eval", "tidy", "--start"]));
    assert!(stdout.contains("shadow-evaluating `tidy`"), "{stdout}");
    assert_eq!(candidate(root).as_deref(), Some("tidy"));
    let stdout = ok(&rtk(root, &["pipeline", "eval", "tidy"]));
    assert!(!stdout.contains("start with:"), "{stdout}");

    let stdout = ok(&rtk(root, &["pipeline", "eval", "tidy", "--stop"]));
    assert!(
        stdout.contains("stopped shadow-evaluating `tidy`"),
        "{stdout}"
    );
    assert_eq!(candidate(root), None);
}
