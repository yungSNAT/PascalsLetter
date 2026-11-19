//! `rtk pipeline add github:…` and `rtk pipeline search` through the user's
//! own `gh`. A stub `gh` on PATH answers from files the test writes and
//! records its argv; a proxy on a closed loopback port makes every direct
//! GitHub request fail, so each read takes the `gh` fallback and none leaves
//! the machine.

#![cfg(all(unix, feature = "engine"))]

mod common;

use std::path::Path;
use std::process::Output;

const SHA_A: &str = "0123456789abcdef0123456789abcdef01234567";
const SHA_B: &str = "fedcba9876543210fedcba9876543210fedcba98";
const PIPELINE: &str = "[pipeline]\nname = \"shared\"\nextends = \"rtk:default\"\n";

/// A stub `gh` that prints `<root>/gh/<kind>` for the API call it recognises
/// (`commit`, `contents` or `search`) and fails when that file is absent.
fn install_stub(root: &Path) {
    let dir = root.join("gh");
    std::fs::create_dir_all(&dir).unwrap();
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let stub = bin.join("gh");
    std::fs::write(
        &stub,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{dir}/argv'\ncase \"$*\" in\n  *\"/commits/\"*) kind=commit ;;\n  *\"/contents/pipeline.toml\"*) kind=contents ;;\n  *\"search/repositories\"*) kind=search ;;\n  *) kind=unknown ;;\nesac\nif [ -f '{dir}/'\"$kind\" ]; then cat '{dir}/'\"$kind\"; exit 0; fi\necho \"gh: HTTP 404 ($kind)\" >&2\nexit 1\n",
            dir = dir.display()
        ),
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&stub).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    std::fs::set_permissions(&stub, permissions).unwrap();
}

fn answer(root: &Path, kind: &str, body: &str) {
    std::fs::write(root.join("gh").join(kind), body).unwrap();
}

fn forget(root: &Path, kind: &str) {
    let _ = std::fs::remove_file(root.join("gh").join(kind));
}

fn gh_calls(root: &Path) -> String {
    std::fs::read_to_string(root.join("gh").join("argv")).unwrap_or_default()
}

/// A loopback port nothing listens on: connecting to it is refused at once.
fn closed_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn rtk(root: &Path, args: &[&str]) -> Output {
    let proxy = format!("http://127.0.0.1:{}", closed_port());
    let path = format!(
        "{}:{}",
        root.join("bin").display(),
        std::env::var("PATH").unwrap_or_default()
    );
    common::rtk_command_in(root)
        .env("RTK_CONFIG_DIR", root.join("config"))
        .env("PATH", path)
        .env("ALL_PROXY", &proxy)
        .env("HTTPS_PROXY", &proxy)
        .args(args)
        .output()
        .expect("spawn rtk")
}

/// What `rtk pipeline add` installed. A file read through `gh` loses its
/// trailing newline, so compare with the served text's `trim_end`.
fn installed(root: &Path) -> Option<String> {
    std::fs::read_to_string(root.join("config/pipelines/shared.toml")).ok()
}

fn ledger_entry(root: &Path) -> Option<toml::Table> {
    let text = std::fs::read_to_string(root.join("rtk/pipeline-installs.toml")).ok()?;
    let table: toml::Table = text.parse().unwrap();
    table.get("shared")?.as_table().cloned()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn a_github_install_fetches_and_records_the_resolved_commit() {
    let root = common::tempdir();
    let root = root.path();
    install_stub(root);
    answer(root, "commit", &format!("{SHA_A}\n"));
    answer(root, "contents", PIPELINE);

    let output = rtk(root, &["pipeline", "add", "github:alice/shared", "--force"]);

    assert!(output.status.success(), "{output:?}");
    let calls = gh_calls(root);
    assert!(
        calls.contains("api repos/alice/shared/commits/HEAD --jq .sha"),
        "{calls}"
    );
    // The file comes from the commit just resolved, not from a branch that can move.
    assert!(
        calls.contains(&format!(
            "repos/alice/shared/contents/pipeline.toml?ref={SHA_A}"
        )),
        "{calls}"
    );
    let stdout = text(&output.stdout);
    assert!(
        stdout.contains("source: github:alice/shared · pinned at commit 0123456789ab"),
        "{stdout}"
    );
    assert_eq!(installed(root).as_deref(), Some(PIPELINE.trim_end()));
    let entry = ledger_entry(root).expect("ledger entry");
    assert_eq!(entry["source"].as_str(), Some("github:alice/shared"));
    assert_eq!(entry["sha"].as_str(), Some(SHA_A));
}

#[test]
fn a_full_commit_reference_is_used_without_asking_github() {
    let root = common::tempdir();
    let root = root.path();
    install_stub(root);
    answer(root, "contents", PIPELINE);

    let source = format!("github:alice/shared@{SHA_B}");
    let output = rtk(root, &["pipeline", "add", &source, "--force"]);

    assert!(output.status.success(), "{output:?}");
    let calls = gh_calls(root);
    assert!(!calls.contains("/commits/"), "{calls}");
    assert!(
        calls.contains(&format!("contents/pipeline.toml?ref={SHA_B}")),
        "{calls}"
    );
    assert_eq!(
        ledger_entry(root).expect("ledger entry")["sha"].as_str(),
        Some(SHA_B)
    );
}

#[test]
fn an_unresolvable_commit_installs_unpinned_from_the_named_reference() {
    let root = common::tempdir();
    let root = root.path();
    install_stub(root);
    answer(root, "contents", PIPELINE);

    let output = rtk(
        root,
        &["pipeline", "add", "github:alice/shared@main", "--force"],
    );

    assert!(output.status.success(), "{output:?}");
    let calls = gh_calls(root);
    assert!(calls.contains("repos/alice/shared/commits/main"), "{calls}");
    assert!(calls.contains("contents/pipeline.toml?ref=main"), "{calls}");
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("could not resolve a commit for github:alice/shared — installing UNPINNED"),
        "{stderr}"
    );
    let stdout = text(&output.stdout);
    assert!(
        stdout.contains("UNPINNED (commit could not be resolved)"),
        "{stdout}"
    );
    assert_eq!(installed(root).as_deref(), Some(PIPELINE.trim_end()));
    let entry = ledger_entry(root).expect("ledger entry");
    assert_eq!(entry["source"].as_str(), Some("github:alice/shared"));
    assert!(!entry.contains_key("sha"), "{entry:?}");
}

#[test]
fn a_repository_without_a_readable_pipeline_installs_nothing() {
    let root = common::tempdir();
    let root = root.path();
    install_stub(root);
    answer(root, "commit", &format!("{SHA_A}\n"));

    let output = rtk(root, &["pipeline", "add", "github:alice/shared", "--force"]);

    assert!(!output.status.success(), "{output:?}");
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("no readable pipeline.toml at the root of github:alice/shared"),
        "{stderr}"
    );
    assert_eq!(installed(root), None);
    assert!(ledger_entry(root).is_none());
}

#[test]
fn updating_from_github_shows_the_pin_change_and_the_diff() {
    let root = common::tempdir();
    let root = root.path();
    install_stub(root);
    answer(root, "commit", &format!("{SHA_A}\n"));
    answer(root, "contents", PIPELINE);
    let first = rtk(root, &["pipeline", "add", "github:alice/shared", "--force"]);
    assert!(first.status.success(), "{first:?}");

    let updated = format!("{PIPELINE}\n[[directive]]\nname = \"tone\"\ntext = \"be brief\"\n");
    forget(root, "commit");
    answer(root, "commit", &format!("{SHA_B}\n"));
    answer(root, "contents", &updated);

    let refused = rtk(root, &["pipeline", "add", "github:alice/shared"]);
    assert!(!refused.status.success(), "{refused:?}");
    assert!(
        text(&refused.stderr).contains("refusing to replace an installed pipeline"),
        "{refused:?}"
    );
    assert_eq!(installed(root).as_deref(), Some(PIPELINE.trim_end()));

    let output = rtk(root, &["pipeline", "add", "github:alice/shared", "--force"]);
    assert!(output.status.success(), "{output:?}");
    let stdout = text(&output.stdout);
    assert!(
        stdout.contains("pinned 0123456789ab -> fedcba987654"),
        "{stdout}"
    );
    assert!(stdout.contains("  + name = \"tone\""), "{stdout}");
    assert_eq!(installed(root).as_deref(), Some(updated.trim_end()));
    assert_eq!(
        ledger_entry(root).expect("ledger entry")["sha"].as_str(),
        Some(SHA_B)
    );
}

#[test]
fn search_lists_repositories_by_stars_with_terminal_controls_removed() {
    let root = common::tempdir();
    let root = root.path();
    install_stub(root);
    answer(
        root,
        "search",
        r#"{"items":[
            {"full_name":"alice/fast","stargazers_count":42,"description":"cuts \u001b[31mcargo\u001b[0m output\u0007"},
            {"full_name":"bob/plain","stargazers_count":3}
        ]}"#,
    );

    let output = rtk(root, &["pipeline", "search", "cargo & git"]);

    assert!(output.status.success(), "{output:?}");
    let calls = gh_calls(root);
    assert!(
        calls.contains(
            "api search/repositories?q=topic:rtk-pipeline%20cargo%20%26%20git&sort=stars"
        ),
        "{calls}"
    );
    let stdout = text(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines[0],
        format!("   42★  {:<42} cuts cargo output", "alice/fast")
    );
    assert_eq!(lines[1], format!("    3★  {:<42} ", "bob/plain"));
    assert!(
        !stdout.contains('\u{1b}') && !stdout.contains('\u{7}'),
        "{stdout:?}"
    );
}

#[test]
fn search_with_no_results_points_at_the_topic_page() {
    let root = common::tempdir();
    let root = root.path();
    install_stub(root);
    answer(root, "search", r#"{"items":[]}"#);

    let output = rtk(root, &["pipeline", "search"]);

    assert!(output.status.success(), "{output:?}");
    assert!(
        gh_calls(root).contains("api search/repositories?q=topic:rtk-pipeline&sort=stars"),
        "{}",
        gh_calls(root)
    );
    let stdout = text(&output.stdout);
    assert!(
        stdout.contains("no shared pipelines found for `topic:rtk-pipeline`"),
        "{stdout}"
    );
}

#[test]
fn search_reports_github_unreachable_when_gh_fails_too() {
    let root = common::tempdir();
    let root = root.path();
    install_stub(root);

    let output = rtk(root, &["pipeline", "search", "x"]);

    assert!(!output.status.success(), "{output:?}");
    assert!(
        text(&output.stderr).contains("GitHub search is unreachable"),
        "{output:?}"
    );
}

#[test]
fn publish_refuses_to_run_without_a_terminal() {
    let root = common::tempdir();
    let root = root.path();
    install_stub(root);

    let output = rtk(root, &["pipeline", "share", "publish"]);

    assert!(!output.status.success(), "{output:?}");
    assert!(
        text(&output.stderr).contains("`rtk pipeline share publish` is interactive"),
        "{output:?}"
    );
    assert_eq!(gh_calls(root), "");
}
