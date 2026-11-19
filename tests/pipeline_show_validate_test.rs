//! `rtk pipeline show` and `rtk pipeline validate` on a user pipeline, and
//! `rtk pipeline add` from stdin and from a URL.

#![cfg(all(unix, feature = "engine"))]

mod common;

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};

const MINE: &str = r#"[pipeline]
name = "mine"
extends = "rtk:default"

[upstream.u]
provider = "anthropic"
model = "m"
api_key_env = "RTK_TEST_PIPELINE_KEY"

[task.digest-it]
kind = "digest"
upstream = "u"

[[hook.after_tool.shell]]
name = "gentle-term"
when = { is_rtk_command = false }
use = ["ansi", "dedup"]
shadow = true

[[hook.before_tool.shell]]
name = "swap-git"
when = { command_matches = "git status" }
action = "route"
task = "digest-it"
mode = "swap"

[[directive]]
name = "tone"
text = "be brief"
enabled = false
"#;

fn rtk(root: &Path, args: &[&str]) -> std::process::Command {
    let mut cmd = common::rtk_command_in(root);
    cmd.env("RTK_CONFIG_DIR", root.join("config")).args(args);
    cmd
}

fn with_pipeline(text: &str) -> tempfile::TempDir {
    let root = common::tempdir();
    let config = root.path().join("config");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("pipeline.toml"), text).unwrap();
    root
}

fn out(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// The first line of `text` that contains `needle`.
fn line_with<'a>(text: &'a str, needle: &str) -> &'a str {
    text.lines()
        .find(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no line with {needle:?} in:\n{text}"))
}

#[test]
fn show_lists_the_rules_directives_and_tasks_of_the_user_pipeline() {
    let root = with_pipeline(MINE);

    let output = rtk(root.path(), &["pipeline", "show"]).output().unwrap();

    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Pipeline: mine\n"), "{stdout}");
    assert!(
        line_with(&stdout, "Inherits: ").contains("rtk:default"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("no pipeline file present"),
        "a user file is present: {stdout}"
    );

    let term = line_with(&stdout, "gentle-term");
    assert!(
        term.contains("when is_rtk_command=false -> use=[ansi, dedup] (shadow)"),
        "{term}"
    );
    let swap = line_with(&stdout, "swap-git");
    assert!(swap.contains("command_matches="), "{swap}");
    assert!(swap.contains("-> action=route task=digest-it"), "{swap}");

    let tone = line_with(&stdout, "  tone ");
    assert!(tone.ends_with("(disabled)"), "{tone}");
    assert!(stdout.contains("\n      be brief\n"), "{stdout}");

    let task = line_with(&stdout, "  digest-it ");
    assert!(task.contains("kind=digest upstream=u"), "{task}");
    assert!(stdout.contains("Guards: never_worse="), "{stdout}");
}

#[test]
fn show_of_a_built_in_prints_it_and_an_unknown_name_lists_the_built_ins() {
    let root = common::tempdir();

    let output = rtk(root.path(), &["pipeline", "show", "rtk:default"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("[pipeline]"),
        "{output:?}"
    );

    let output = rtk(root.path(), &["pipeline", "show", "rtk:nope"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("unknown built-in `rtk:nope` — built-ins: ")
            && stdout.contains("rtk:default"),
        "{stdout}"
    );
}

#[test]
fn validate_passes_a_good_pipeline_and_names_a_missing_credential() {
    let root = with_pipeline(MINE);

    let output = rtk(root.path(), &["pipeline", "validate"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("pipeline: ok (mine)"), "{stdout}");
    assert!(
        line_with(&stdout, "auth needed: ")
            .starts_with("auth needed: export RTK_TEST_PIPELINE_KEY=...   (upstream `u` — "),
        "{stdout}"
    );

    let output = rtk(root.path(), &["pipeline", "validate"])
        .env("RTK_TEST_PIPELINE_KEY", "set")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(!out(&output).contains("auth needed"), "{output:?}");
}

#[test]
fn validate_fails_a_pipeline_with_an_error() {
    let root = with_pipeline(
        "[pipeline]\nname = \"broken\"\n\n[[hook.during_tool.shell]]\nname = \"x\"\nuse = [\"ansi\"]\n",
    );

    let output = rtk(root.path(), &["pipeline", "validate"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let all = out(&output);
    assert!(all.contains("E001"), "{all}");
    assert!(!all.contains("pipeline: ok"), "{all}");
}

fn installed(root: &Path, name: &str) -> Option<String> {
    std::fs::read_to_string(root.join(format!("config/pipelines/{name}.toml"))).ok()
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

#[test]
fn add_reads_a_pipeline_from_stdin() {
    let root = common::tempdir();
    let text = "[pipeline]\nname = \"piped\"\nextends = \"rtk:default\"\n";

    let mut child = rtk(root.path(), &["pipeline", "add", "-", "--force"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();

    assert!(output.status.success(), "{output:?}");
    assert_eq!(installed(root.path(), "piped").as_deref(), Some(text));
    // Only a URL or a GitHub source has an origin worth recording.
    assert!(
        !data_dir(root.path())
            .join("pipeline-installs.toml")
            .exists(),
        "{output:?}"
    );
}

#[test]
fn add_fetches_a_pipeline_from_a_url_and_records_where_it_came_from() {
    let root = common::tempdir();
    let text = "[pipeline]\nname = \"fetched\"\nextends = \"rtk:default\"\n";
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!(
        "http://127.0.0.1:{}/fetched.toml",
        listener.local_addr().unwrap().port()
    );
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0u8; 4096];
        let n = stream.read(&mut request).unwrap();
        let request_line = String::from_utf8_lossy(&request[..n])
            .lines()
            .next()
            .unwrap_or_default()
            .to_string();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
            text.len()
        )
        .unwrap();
        request_line
    });

    let output = rtk(root.path(), &["pipeline", "add", &url, "--force"])
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert_eq!(server.join().unwrap(), "GET /fetched.toml HTTP/1.1");
    assert_eq!(installed(root.path(), "fetched").as_deref(), Some(text));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(&format!("source: {url} · UNPINNED")),
        "{stdout}"
    );
    let ledger: toml::Table =
        std::fs::read_to_string(data_dir(root.path()).join("pipeline-installs.toml"))
            .unwrap()
            .parse()
            .unwrap();
    let entry = ledger["fetched"].as_table().unwrap();
    assert_eq!(entry["source"].as_str(), Some(url.as_str()));
    assert!(!entry.contains_key("sha"), "{entry:?}");
}
