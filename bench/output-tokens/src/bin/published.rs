//! Repeats the published measurement: downloads each tool's release and checks it
//! against its published checksum, clones Chisle at its tag, installs the plugins
//! from their committed lockfiles, builds this checkout's rtk, then runs
//! output-tokens over corpus.toml. Linux x86_64; needs git, curl, tar, sha256sum,
//! cargo, node, npm, uv and rg. From the repository root:
//! `cargo run --release --locked --manifest-path bench/output-tokens/Cargo.toml --bin published -- <work directory>`

use anyhow::{Context, Result, ensure};
use clap::Parser;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const RTK: &str = "https://github.com/rtk-ai/rtk/releases/download/v1.2.0";
const PL: &str = "https://github.com/yungSNAT/PascalsLetter/releases/download/fork-v1.2.0-1";
const TOKENADE: &str = "https://github.com/pi-infected/tokenade/releases/download/v2.0.9";
const SNIP: &str = "https://github.com/edouard-claude/snip/releases/download/v0.26.0";
/// Chisle publishes no archive: its v3.12.1 tag, checked by commit and by a clean
/// tree, so a cached or moved copy cannot run under its label.
const CHISLE: &str = "84e8f5a392aa8706604bf143822009a7e5c48c44";
/// The Claude Code plugins: each one's folder (its lockfile under `plugins/`, its
/// install under the work directory's `tools/`), npm package and version.
const PLUGINS: [(&str, &str, &str); 2] = [
    ("context-mode", "context-mode", "2.2.0"),
    ("token-optimizer", "@ooples/token-optimizer-mcp", "7.11.0"),
];

#[derive(Parser)]
#[command(about = "Repeats the published output-token measurement (Linux x86_64)")]
struct Args {
    /// Holds the downloads, builds and corpus clones, reused by the next run, and
    /// each run's `run-<UTC time>` folder.
    work: PathBuf,
}

/// Runs `cmd`, failing unless it exits 0.
fn run(cmd: &mut Command) -> Result<()> {
    let status = cmd
        .status()
        .with_context(|| format!("running {:?}", cmd.get_program()))?;
    ensure!(status.success(), "{cmd:?} failed: {status}");
    Ok(())
}

/// The standard output of `cmd`, failing unless it exits 0.
fn stdout(cmd: &mut Command) -> Result<String> {
    let out = cmd
        .stderr(Stdio::inherit())
        .output()
        .with_context(|| format!("running {:?}", cmd.get_program()))?;
    ensure!(out.status.success(), "{cmd:?} failed: {}", out.status);
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Downloads `url` to `file` unless a non-empty copy is there; a reused copy is
/// checked like a new one.
fn fetch(url: &str, file: &Path) -> Result<()> {
    if fs::metadata(file).is_ok_and(|m| m.len() > 0) {
        return Ok(());
    }
    // nosemgrep: interpreter-execution -- a benchmark, not rtk: downloads the published releases it measures
    run(Command::new("curl")
        .arg("-fsSL")
        .arg("-o")
        .arg(file)
        .arg(url))
}

/// The hashes a sha256sum-style list gives for `file` (as `<hash>  <file>` or
/// `<hash> *<file>`).
fn listed(list: &str, file: &str) -> Vec<String> {
    list.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let hash = fields.next()?;
            let name = fields.next()?;
            (name == file || name.strip_prefix('*') == Some(file)).then(|| hash.to_string())
        })
        .collect()
}

/// Fails unless `expected` names at least one hash and every one is `actual`.
fn verify(expected: &[String], file: &str, actual: &str) -> Result<()> {
    ensure!(!expected.is_empty(), "no checksum published for {file}");
    for hash in expected {
        ensure!(
            hash.eq_ignore_ascii_case(actual),
            "{file}: SHA-256 {actual}, published {hash}"
        );
    }
    Ok(())
}

/// The hashes a checksum file gives for `archive`. `per_file` marks a file that
/// holds only the archive's hash, rather than a list: each line's first field then
/// counts, so a second, different hash fails `verify` rather than going unread.
fn expected(sums: &str, archive: &str, per_file: bool) -> Vec<String> {
    if per_file {
        sums.lines()
            .filter_map(|line| line.split_whitespace().next())
            .map(str::to_string)
            .collect()
    } else {
        listed(sums, archive)
    }
}

/// Fetches `archive` and its checksum file `sums` from `base` into `dir`, checks the
/// archive and unpacks it. `per_file` as for `expected`.
fn release(dir: &Path, base: &str, sums: &str, archive: &str, per_file: bool) -> Result<()> {
    fs::create_dir_all(dir)?;
    fetch(&format!("{base}/{sums}"), &dir.join(sums))?;
    fetch(&format!("{base}/{archive}"), &dir.join(archive))?;
    let list = fs::read_to_string(dir.join(sums)).with_context(|| format!("reading {sums}"))?;
    let expected = expected(&list, archive, per_file);
    let sum = stdout(
        Command::new("sha256sum")
            .arg("-b")
            .arg(archive)
            .current_dir(dir),
    )?;
    let actual = sum.split_whitespace().next().unwrap_or_default();
    verify(&expected, archive, actual)?;
    run(Command::new("tar")
        .arg("-xzf")
        .arg(archive)
        .current_dir(dir))
}

/// Clones Chisle's tag once and refuses a copy at another commit or with any change.
fn chisle(dir: &Path) -> Result<()> {
    if !dir.is_dir() {
        run(Command::new("git")
            .args(["clone", "--quiet", "--depth", "1", "--branch", "v3.12.1"])
            .arg("https://github.com/JayPokale/Chisle")
            .arg(dir))?;
    }
    let git = |args: &[&str]| stdout(Command::new("git").arg("-C").arg(dir).args(args)).ok();
    let clean = git(&["rev-parse", "HEAD"]).is_some_and(|head| head.trim() == CHISLE)
        && git(&["status", "--porcelain", "--ignored"]).is_some_and(|s| s.is_empty());
    ensure!(
        clean,
        "{} is not a clean checkout of {CHISLE}",
        dir.display()
    );
    Ok(())
}

/// Installs a Claude Code plugin into `prefix` with `npm ci` from the lockfile
/// committed in `locked`, without install scripts: every package at its locked
/// version, checked against its locked hash, replacing what an earlier run left.
fn npm_plugin(locked: &Path, prefix: &Path, package: &str, version: &str) -> Result<()> {
    fs::create_dir_all(prefix)?;
    for file in ["package.json", "package-lock.json"] {
        fs::copy(locked.join(file), prefix.join(file))
            .with_context(|| format!("copy {}", locked.join(file).display()))?;
    }
    run(Command::new("npm")
        .args([
            "ci",
            "--ignore-scripts",
            "--no-audit",
            "--no-fund",
            "--prefix",
        ])
        .arg(prefix)
        .stdout(Stdio::null()))?;
    holds(prefix, package, version)
}

/// Fails unless `prefix` holds `package` at `version`, the version its label names.
fn holds(prefix: &Path, package: &str, version: &str) -> Result<()> {
    let installed = prefix.join("node_modules").join(package);
    let found = fs::read_to_string(installed.join("package.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|json| json["version"].as_str().map(str::to_string));
    ensure!(
        found.as_deref() == Some(version),
        "{} does not hold {package} {version}",
        prefix.display()
    );
    Ok(())
}

/// output-tokens' arguments: the corpus and each side, in the published order.
fn sides(here: &Path, work: &Path, out: &Path) -> Vec<OsString> {
    let t = work.join("tools");
    let t = t.display();
    let rtk = work.join("target-rtk/release/rtk");
    let pairs: [(&str, String); 13] = [
        ("--corpus", here.join("corpus.toml").display().to_string()),
        ("--work", work.join("repos").display().to_string()),
        ("--out", out.display().to_string()),
        ("--rtk", format!("rtk 1.2.0={t}/rtk/rtk")),
        ("--rtk", format!("rtk-oss 1.2.0={t}/rtk-oss/rtk")),
        (
            "--rtk",
            format!("PascalsLetter fork-v1.2.0-1 oss={t}/pl-oss/rtk"),
        ),
        (
            "--rtk",
            format!("PascalsLetter (this checkout)={}", rtk.display()),
        ),
        (
            "--wrap",
            format!(
                "Tokenade 2.0.9=env TOKENADE_NO_AUTO_UPDATE=1 TOKENADE_NO_STAR_INVITE=1 {t}/tokenade/tokenade wrap {{qcmd}}"
            ),
        ),
        ("--wrap", format!("snip 0.26.0={t}/snip/snip {{cmd}}")),
        ("--wrap", "raw, run again={cmd}".to_string()),
        (
            "--chisle",
            format!("Chisle 3.12.1={t}/chisle/hooks/chisle-compress-output.js"),
        ),
        (
            "--claude-plugin",
            format!("context-mode 2.2.0={t}/context-mode/node_modules/context-mode"),
        ),
        (
            "--claude-plugin",
            format!(
                "token-optimizer-mcp 7.11.0={t}/token-optimizer/node_modules/@ooples/token-optimizer-mcp/src/plugin"
            ),
        ),
    ];
    pairs
        .into_iter()
        .flat_map(|(flag, value)| [flag.into(), value.into()])
        .collect()
}

fn main() -> Result<()> {
    let args = Args::parse();
    fs::create_dir_all(&args.work)?;
    let work = fs::canonicalize(&args.work)?;
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = fs::canonicalize(here.join("../.."))?;
    let t = work.join("tools");

    let full = "rtk-x86_64-unknown-linux-musl.tar.gz";
    let oss = "rtk-oss-x86_64-unknown-linux-musl.tar.gz";
    release(&t.join("rtk"), RTK, "checksums.txt", full, false)?;
    release(&t.join("rtk-oss"), RTK, "checksums.txt", oss, false)?;
    release(&t.join("pl-oss"), PL, "checksums.txt", oss, false)?;
    let tokenade = "tokenade-2.0.9-x86_64-unknown-linux-musl.tar.gz";
    let sums = format!("{tokenade}.sha256");
    release(&t.join("tokenade"), TOKENADE, &sums, tokenade, true)?;
    let snip = "snip_0.26.0_linux_amd64.tar.gz";
    release(&t.join("snip"), SNIP, "checksums.txt", snip, false)?;
    chisle(&t.join("chisle"))?;
    for (dir, package, version) in PLUGINS {
        npm_plugin(
            &here.join("plugins").join(dir),
            &t.join(dir),
            package,
            version,
        )?;
    }

    // This checkout's full edition, as `cargo install --git` builds it.
    for (target, manifest) in [
        ("target-rtk", root.join("Cargo.toml")),
        ("target-bench", here.join("Cargo.toml")),
    ] {
        run(Command::new("cargo")
            .args(["build", "--release", "--locked", "--manifest-path"])
            .arg(manifest)
            .env("CARGO_TARGET_DIR", work.join(target)))?;
    }

    let stamp = stdout(Command::new("date").arg("-u").arg("+%Y%m%dT%H%M%SZ"))?;
    let out = work.join(format!("run-{}", stamp.trim()));
    run(
        // nosemgrep: dynamic-command-execution -- a benchmark, not rtk: runs the measurement binary it just built
        Command::new(work.join("target-bench/release/output-tokens"))
            .args(sides(here, &work, &out)),
    )?;

    let python = work.join("repos/click/.venv/bin/python");
    let mut versions = String::new();
    for probe in [
        &["cargo", "--version"][..],
        &["cargo", "clippy", "--version"],
        &["node", "--version"],
        &["npm", "--version"],
        &["uv", "--version"],
    ] {
        // nosemgrep: dynamic-command-execution -- a benchmark, not rtk: records the version of each tool the run used
        versions += &stdout(Command::new(probe[0]).args(&probe[1..]))?;
    }
    // nosemgrep: dynamic-command-execution -- a benchmark, not rtk: records the version of the corpus's Python
    versions += &stdout(Command::new(&python).arg("--version"))?;
    versions += &stdout(Command::new("git").arg("--version"))?;
    let rg = stdout(Command::new("rg").arg("--version"))?;
    versions += rg.lines().next().unwrap_or_default();
    versions.push('\n');
    fs::write(out.join("toolchain.txt"), versions)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    #[test]
    fn a_list_gives_the_lines_for_the_file_alone() {
        let list = format!("{EMPTY}  a.tar.gz\n{EMPTY} *b.tar.gz\nffff  a.tar.gz.sig\n\n");
        assert_eq!(listed(&list, "a.tar.gz"), [EMPTY]);
        assert_eq!(listed(&list, "b.tar.gz"), [EMPTY]);
        assert!(listed(&list, "c.tar.gz").is_empty());
    }

    #[test]
    fn every_listed_hash_must_match() {
        assert!(verify(&[EMPTY.to_uppercase()], "a", EMPTY).is_ok());
        assert!(verify(&[], "a", EMPTY).is_err());
        assert!(verify(&[EMPTY.into()], "a", "ffff").is_err());
        assert!(verify(&[EMPTY.into(), "00".into()], "a", EMPTY).is_err());
        // A failed hash gives no text, which matches nothing.
        assert!(verify(&[EMPTY.into()], "a", "").is_err());
    }

    #[test]
    fn a_per_file_checksum_gives_every_line_hash() {
        let sums = format!("{EMPTY}  a.tar.gz\nffff  a.tar.gz\n");
        assert_eq!(expected(&sums, "a.tar.gz", true), [EMPTY, "ffff"]);
        assert!(verify(&expected(&sums, "a.tar.gz", true), "a.tar.gz", EMPTY).is_err());
        assert_eq!(expected(&format!("{EMPTY}\n"), "a.tar.gz", true), [EMPTY]);
    }

    /// A cached archive is checked again, and one that no longer matches is never
    /// unpacked. Needs sha256sum, as the runner does.
    #[cfg(unix)]
    #[test]
    fn a_cached_archive_that_does_not_match_is_refused_before_tar() {
        let dir = std::env::temp_dir().join(format!("published-cache-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.tar.gz"), "not the published archive").unwrap();
        fs::write(dir.join("checksums.txt"), format!("{EMPTY}  a.tar.gz\n")).unwrap();
        // Both files are cached, so nothing is fetched from this address.
        let error = release(
            &dir,
            "http://127.0.0.1:9",
            "checksums.txt",
            "a.tar.gz",
            false,
        )
        .unwrap_err()
        .to_string();
        assert!(error.starts_with("a.tar.gz: SHA-256 "), "{error}");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_plugin_at_another_version_is_refused() {
        let root = std::env::temp_dir().join(format!("published-npm-{}", std::process::id()));
        let package = root.join("node_modules/@scope/plugin");
        fs::create_dir_all(&package).unwrap();
        fs::write(package.join("package.json"), r#"{"version": "1.0.0"}"#).unwrap();
        assert!(holds(&root, "@scope/plugin", "1.0.0").is_ok());
        let error = holds(&root, "@scope/plugin", "2.0.0").unwrap_err();
        assert!(
            error
                .to_string()
                .ends_with("does not hold @scope/plugin 2.0.0")
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn each_plugin_lock_pins_its_labelled_version_and_every_package_by_hash() {
        let plugins = Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins");
        for (dir, package, version) in PLUGINS {
            let read = |file: &str| -> serde_json::Value {
                let text = fs::read_to_string(plugins.join(dir).join(file)).unwrap();
                serde_json::from_str(&text).unwrap()
            };
            assert_eq!(read("package.json")["dependencies"][package], version);
            let lock = read("package-lock.json");
            let packages = lock["packages"].as_object().unwrap();
            assert_eq!(
                packages[&format!("node_modules/{package}")]["version"],
                version
            );
            for (path, entry) in packages.iter().filter(|(path, _)| !path.is_empty()) {
                assert!(
                    entry["integrity"]
                        .as_str()
                        .is_some_and(|hash| hash.starts_with("sha512-")),
                    "{dir}: {path} is not pinned by hash"
                );
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_sides_keep_their_published_labels_and_order() {
        let args = sides(Path::new("/b"), Path::new("/w"), Path::new("/w/run-x"));
        let args: Vec<_> = args.iter().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(
            args,
            [
                "--corpus",
                "/b/corpus.toml",
                "--work",
                "/w/repos",
                "--out",
                "/w/run-x",
                "--rtk",
                "rtk 1.2.0=/w/tools/rtk/rtk",
                "--rtk",
                "rtk-oss 1.2.0=/w/tools/rtk-oss/rtk",
                "--rtk",
                "PascalsLetter fork-v1.2.0-1 oss=/w/tools/pl-oss/rtk",
                "--rtk",
                "PascalsLetter (this checkout)=/w/target-rtk/release/rtk",
                "--wrap",
                "Tokenade 2.0.9=env TOKENADE_NO_AUTO_UPDATE=1 TOKENADE_NO_STAR_INVITE=1 /w/tools/tokenade/tokenade wrap {qcmd}",
                "--wrap",
                "snip 0.26.0=/w/tools/snip/snip {cmd}",
                "--wrap",
                "raw, run again={cmd}",
                "--chisle",
                "Chisle 3.12.1=/w/tools/chisle/hooks/chisle-compress-output.js",
                "--claude-plugin",
                "context-mode 2.2.0=/w/tools/context-mode/node_modules/context-mode",
                "--claude-plugin",
                "token-optimizer-mcp 7.11.0=/w/tools/token-optimizer/node_modules/@ooples/token-optimizer-mcp/src/plugin",
            ]
        );
    }
}
