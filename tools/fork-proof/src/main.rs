//! Proves each row of README's 'What PascalsLetter changes' table against upstream
//! RTK: its tests pass here and fail on upstream. A test that passes upstream means
//! upstream has fixed the bug, so the patch and its row can go.
//!
//! Adapted from kylehgc/rtk scripts/fork-proof.sh at commit
//! 6fb87aa9591f03815fe8932b7f241820e2bc9374 (Apache-2.0), by way of this
//! repository's own shell version. Changed here: the manifest maps each README row
//! to its proving tests and their platform; a row without a manifest line, or a
//! line without a row, fails; only the manifest's Linux test files, with
//! tests/common and tests/fixtures, are lifted onto upstream, including a test of
//! the proxy/ crate; rows for another platform are marked, never proved; skip
//! guards panic under RTK_PROOF; README's Upstream column must match the result;
//! the table is printed, not written to a page.
//!
//! Linux only. Run from the repository root, with upstream's develop fetched into
//! `upstream/develop` (or the ref in `UPSTREAM_REF`):
//! `cargo run --locked --manifest-path tools/fork-proof/Cargo.toml`.

use anyhow::{Context, Result, ensure};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

const CLAIMS: &str = ".github/fork-claims.tsv";
const README: &str = "README.md";
const HEADING: &str = "## What PascalsLetter changes";
const UPSTREAM: &str = "target/fork-proof/upstream";

/// One manifest line: a README row's area and a test that proves it.
#[derive(Debug, PartialEq)]
struct Claim {
    area: String,
    platform: String,
    target: String,
    test: String,
}

/// README's rows under [`HEADING`], as (area, Upstream cell), in table order.
fn readme_rows(readme: &str) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    let mut inside = false;
    for line in readme.lines() {
        if line.starts_with("## ") {
            inside = line == HEADING;
        }
        let Some(rest) = line.strip_prefix("| ") else {
            continue;
        };
        if !inside || rest.starts_with("Area ") || rest.starts_with("--- ") {
            continue;
        }
        let mut cells = rest.split(" | ");
        let area = cells.next().unwrap_or_default().to_string();
        let cell = cells.next().unwrap_or_default().to_string();
        rows.push((area, cell));
    }
    rows
}

/// The manifest's lines, skipping blank lines and `#` comments; a line's fields
/// after the fourth (such as a `# reason`) are ignored.
fn claims(manifest: &str) -> Vec<Claim> {
    manifest
        .lines()
        .filter_map(|line| {
            // Tabs separate the fields and runs of them count as one, as `read` reads them.
            let mut fields = line.split('\t').filter(|field| !field.is_empty());
            let area = fields.next()?;
            if area.starts_with('#') {
                return None;
            }
            let mut next = || fields.next().unwrap_or_default().to_string();
            Some(Claim {
                area: area.to_string(),
                platform: next(),
                target: next(),
                test: next(),
            })
        })
        .collect()
}

/// A manifest out of step with README: areas claimed but not in README, and rows
/// with no claim.
fn mismatches(rows: &[(String, String)], claims: &[Claim]) -> Vec<String> {
    let areas: HashSet<&str> = rows.iter().map(|(area, _)| area.as_str()).collect();
    let claimed: HashSet<&str> = claims.iter().map(|c| c.area.as_str()).collect();
    let mut problems: Vec<String> = claims
        .iter()
        .filter(|c| !areas.contains(c.area.as_str()))
        .map(|c| format!("{}: in {CLAIMS}, but not a README row", c.area))
        .collect();
    problems.extend(
        rows.iter()
            .filter(|(area, _)| !claimed.contains(area.as_str()))
            .map(|(area, _)| format!("{area}: claim without a test (no line in {CLAIMS})")),
    );
    problems
}

/// The test result lines of a cargo test run's stdout, as `test name` to `ok` or `FAILED`.
fn results(stdout: &str) -> Vec<(String, String)> {
    stdout
        .lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("test ")?;
            let (name, result) = rest.split_once(" ... ")?;
            (!name.is_empty() && !name.contains(' ') && matches!(result, "ok" | "FAILED"))
                .then(|| (name.to_string(), result.to_string()))
        })
        .collect()
}

/// The first result for `test` in `target`, or `MISSING` when it did not compile or run.
fn result<'a>(found: &'a [(String, String, String)], target: &str, test: &str) -> &'a str {
    found
        .iter()
        .find(|(t, name, _)| t == target && name == test)
        .map_or("MISSING", |(_, _, result)| result)
}

/// Runs each test target with cargo in `dir`, forwarding cargo's output to stderr so a
/// compile error stays visible, and returns (target, test, result) for every test.
fn run_tests(
    dir: &Path,
    targets: &BTreeSet<&str>,
    extra: &[&str],
    target_dir: Option<&Path>,
) -> Result<Vec<(String, String, String)>> {
    let mut found = Vec::new();
    for target in targets {
        let krate = target.split_once("/tests/").map_or(".", |(krate, _)| krate);
        let name = Path::new(target)
            .file_stem()
            .with_context(|| format!("test target {target}"))?
            .to_string_lossy();
        let mut cargo = Command::new("cargo");
        cargo
            .arg("test")
            .args(extra)
            .args(["--manifest-path", &format!("{krate}/Cargo.toml")])
            .args(["--test", &name])
            // A skipped test prints ok; under RTK_PROOF the proving tests' skip guards
            // panic instead.
            .env("RTK_PROOF", "1")
            .current_dir(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        if let Some(target_dir) = target_dir {
            cargo.env("CARGO_TARGET_DIR", target_dir);
        }
        let mut child = cargo.spawn().context("running cargo test")?;
        let mut stdout = String::new();
        for line in BufReader::new(child.stdout.take().context("cargo's stdout")?).lines() {
            let line = line?;
            eprintln!("{line}");
            stdout.push_str(&line);
            stdout.push('\n');
        }
        // A failing test exits non-zero; its result lines say which.
        child.wait()?;
        found.extend(
            results(&stdout)
                .into_iter()
                .map(|(test, result)| (target.to_string(), test, result)),
        );
    }
    Ok(found)
}

fn git(args: &[&str]) -> Result<std::process::Output> {
    Command::new("git")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .context("running git")
}

/// Copies `from` (a file or a directory) into `to`, merging into what is there, as
/// `cp -R` does.
fn copy_into(from: &Path, to: &Path) -> Result<()> {
    if from.is_dir() {
        fs::create_dir_all(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            copy_into(&entry.path(), &to.join(entry.file_name()))?;
        }
    } else {
        fs::copy(from, to).with_context(|| format!("copying {}", from.display()))?;
    }
    Ok(())
}

/// Upstream's worktree, removed when dropped, whatever ended the run.
struct Worktree;

impl Worktree {
    fn remove() {
        let removed =
            git(&["worktree", "remove", "--force", UPSTREAM]).is_ok_and(|out| out.status.success());
        if !removed {
            let _ = fs::remove_dir_all(UPSTREAM);
        }
        let _ = git(&["worktree", "prune"]);
    }
}

impl Drop for Worktree {
    fn drop(&mut self) {
        Worktree::remove();
    }
}

/// The proof table and the problems found, from both runs' results.
fn proof(
    rows: &[(String, String)],
    claims: &[Claim],
    here: &[(String, String, String)],
    there: &[(String, String, String)],
) -> (Vec<String>, Vec<String>) {
    let mut status: HashMap<&str, &str> = HashMap::new();
    let mut table = Vec::new();
    let mut problems = Vec::new();
    for c in claims {
        let mark = match c.platform.as_str() {
            "linux" => None,
            "windows" => Some("Not proved: Windows only"),
            "macos" => Some("Not proved: macOS only"),
            "none" => Some("Not proved: no test"),
            other => {
                problems.push(format!("{}: unknown platform '{other}'", c.area));
                continue;
            }
        };
        if let Some(mark) = mark {
            status.entry(&c.area).or_insert(mark);
            table.push((&c.area, &c.test, "not run", "not run"));
            continue;
        }
        let fork = result(here, &c.target, &c.test);
        let up = result(there, &c.target, &c.test);
        table.push((&c.area, &c.test, fork, up));
        let area = &c.area;
        let test = &c.test;
        if fork != "ok" {
            problems.push(format!("{area}: {test} does not pass on the fork ({fork})"));
        } else if up == "ok" {
            problems.push(format!(
                "{area}: {test} passes upstream: upstream has fixed this: drop the patch and the row"
            ));
        } else if up == "MISSING" {
            problems.push(format!("{area}: {test} did not compile or run upstream"));
        } else {
            status.insert(area, "Not in upstream");
        }
    }
    for (area, cell) in rows {
        if let Some(status) = status.get(area.as_str())
            && cell != status
        {
            problems.push(format!(
                "{area}: README says '{cell}', the proof says '{status}'"
            ));
        }
    }
    let lines = table
        .into_iter()
        .map(|(area, test, fork, up)| {
            let test = if test == "-" {
                test.clone()
            } else {
                format!("`{test}`")
            };
            let status = status.get(area.as_str()).copied().unwrap_or("not proved");
            format!("| {area} | {status} | {test} | {fork} | {up} |")
        })
        .collect();
    (lines, problems)
}

/// Prints the problems to stderr; true when there were any.
fn report(problems: &[String]) -> bool {
    if problems.is_empty() {
        return false;
    }
    eprintln!();
    for problem in problems {
        eprintln!("fork-proof: {problem}");
    }
    true
}

fn run(upstream_ref: &str) -> Result<bool> {
    let verified = git(&[
        "rev-parse",
        "--verify",
        "--quiet",
        &format!("{upstream_ref}^{{commit}}"),
    ])?;
    ensure!(
        verified.status.success(),
        "{upstream_ref} not found: fetch rtk-ai/rtk develop into it"
    );
    ensure!(Path::new(CLAIMS).is_file(), "{CLAIMS} not found");
    let rows = readme_rows(&fs::read_to_string(README).context("reading README.md")?);
    ensure!(
        !rows.is_empty(),
        "no table under 'What PascalsLetter changes' in {README}"
    );
    let claims = claims(&fs::read_to_string(CLAIMS)?);
    // A manifest out of step with README fails before anything is built.
    if report(&mismatches(&rows, &claims)) {
        return Ok(false);
    }
    let targets: BTreeSet<&str> = claims
        .iter()
        .filter(|c| c.platform == "linux")
        .map(|c| c.target.as_str())
        .collect();

    eprintln!("==> fork tests");
    let here = run_tests(Path::new("."), &targets, &["--locked"], None)?;

    // Under target/, which git ignores. Upstream builds in a target directory of its
    // own beside the worktree, never over the fork's binaries, and kept for the next run.
    Worktree::remove();
    let worktree = Worktree;
    let added = git(&[
        "worktree",
        "add",
        "--quiet",
        "--detach",
        UPSTREAM,
        upstream_ref,
    ])?;
    ensure!(
        added.status.success(),
        "git worktree add failed: {}",
        String::from_utf8_lossy(&added.stderr)
    );
    for path in targets
        .iter()
        .copied()
        .chain(["tests/common", "tests/fixtures"])
    {
        let path = Path::new(path);
        let to = Path::new(UPSTREAM).join(path);
        fs::create_dir_all(to.parent().context("a path with a parent")?)?;
        copy_into(path, &to)?;
    }
    eprintln!("==> the same tests on {upstream_ref}");
    // Not --locked: a lockfile upstream left behind its manifest (proxy/Cargo.lock on
    // 12 November 2026) would stop the proof compiling there, which reads as a
    // missing test rather than a result.
    let target_dir: PathBuf = std::env::current_dir()?.join("target/fork-proof/target");
    let there = run_tests(Path::new(UPSTREAM), &targets, &[], Some(&target_dir))?;
    drop(worktree);

    let (table, problems) = proof(&rows, &claims, &here, &there);
    let short = git(&["rev-parse", "--short", upstream_ref])?;
    let mut out = std::io::stdout().lock();
    writeln!(
        out,
        "Proof against `{upstream_ref}` ({}):",
        String::from_utf8_lossy(&short.stdout).trim()
    )?;
    writeln!(out)?;
    writeln!(out, "| Area | Status | Test | Fork | Upstream |")?;
    writeln!(out, "| --- | --- | --- | --- | --- |")?;
    for line in table {
        writeln!(out, "{line}")?;
    }
    out.flush()?;
    Ok(!report(&problems))
}

fn main() -> ExitCode {
    if std::env::args().len() > 1 {
        eprintln!("error: usage: fork-proof");
        return ExitCode::FAILURE;
    }
    let upstream_ref =
        std::env::var("UPSTREAM_REF").unwrap_or_else(|_| "upstream/develop".to_string());
    match run(&upstream_ref) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const README_TEXT: &str = "# x\n\n## What PascalsLetter changes\n\nIntro.\n\n\
        | Area | Upstream | Change |\n| --- | --- | --- |\n\
        | `grep` | Not in upstream | Caps. |\n| Windows | Not proved: Windows only | Quotes. |\n\
        \n## Inherited\n\n| Area | Upstream | Change |\n| --- | --- | --- |\n| Other | x | y |\n";

    fn claim(area: &str, platform: &str, target: &str, test: &str) -> Claim {
        Claim {
            area: area.into(),
            platform: platform.into(),
            target: target.into(),
            test: test.into(),
        }
    }

    #[test]
    fn rows_come_only_from_the_changes_table() {
        assert_eq!(
            readme_rows(README_TEXT),
            [
                ("`grep`".to_string(), "Not in upstream".to_string()),
                ("Windows".into(), "Not proved: Windows only".into()),
            ]
        );
    }

    #[test]
    fn claims_skip_comments_and_blank_lines_and_ignore_a_trailing_reason() {
        let manifest =
            "# header\n\n`grep`\tlinux\ttests/g.rs\tcaps\nWindows\twindows\t-\t-\t# Windows only\n";
        assert_eq!(
            claims(manifest),
            [
                claim("`grep`", "linux", "tests/g.rs", "caps"),
                claim("Windows", "windows", "-", "-"),
            ]
        );
    }

    #[test]
    fn a_manifest_out_of_step_with_readme_is_refused() {
        let rows = readme_rows(README_TEXT);
        let claims = [
            claim("`grep`", "linux", "tests/g.rs", "caps"),
            claim("Gone", "linux", "tests/g.rs", "old"),
        ];
        assert_eq!(
            mismatches(&rows, &claims),
            [
                format!("Gone: in {CLAIMS}, but not a README row"),
                format!("Windows: claim without a test (no line in {CLAIMS})"),
            ]
        );
    }

    #[test]
    fn only_libtest_result_lines_are_read() {
        let stdout = "running 2 tests\ntest a ... ok\ntest b ... FAILED\ntest c ... ignored\n\
                      test result: FAILED. 1 passed\ntest d e ... ok\n";
        assert_eq!(
            results(stdout),
            [
                ("a".to_string(), "ok".to_string()),
                ("b".into(), "FAILED".into())
            ]
        );
    }

    fn found(rows: &[(&str, &str, &str)]) -> Vec<(String, String, String)> {
        rows.iter()
            .map(|(t, n, r)| (t.to_string(), n.to_string(), r.to_string()))
            .collect()
    }

    #[test]
    fn a_result_is_matched_by_target_and_whole_test_name() {
        let here = found(&[
            ("tests/g.rs", "caps_long", "ok"),
            ("tests/h.rs", "caps", "ok"),
        ]);
        assert_eq!(result(&here, "tests/g.rs", "caps"), "MISSING");
        assert_eq!(result(&here, "tests/h.rs", "caps"), "ok");
    }

    #[test]
    fn each_outcome_gives_its_status_and_problem() {
        let rows = readme_rows(README_TEXT);
        let claims = [
            claim("`grep`", "linux", "tests/g.rs", "proved"),
            claim("`grep`", "linux", "tests/g.rs", "fixed_upstream"),
            claim("`grep`", "linux", "tests/g.rs", "broken_here"),
            claim("`grep`", "linux", "tests/g.rs", "absent_upstream"),
            claim("Windows", "windows", "-", "-"),
            claim("Odd", "solaris", "-", "-"),
        ];
        let here = found(&[
            ("tests/g.rs", "proved", "ok"),
            ("tests/g.rs", "fixed_upstream", "ok"),
            ("tests/g.rs", "broken_here", "FAILED"),
            ("tests/g.rs", "absent_upstream", "ok"),
        ]);
        let there = found(&[
            ("tests/g.rs", "proved", "FAILED"),
            ("tests/g.rs", "fixed_upstream", "ok"),
            ("tests/g.rs", "broken_here", "FAILED"),
        ]);
        let (table, problems) = proof(&rows, &claims, &here, &there);
        assert_eq!(
            table,
            [
                "| `grep` | Not in upstream | `proved` | ok | FAILED |",
                "| `grep` | Not in upstream | `fixed_upstream` | ok | ok |",
                "| `grep` | Not in upstream | `broken_here` | FAILED | FAILED |",
                "| `grep` | Not in upstream | `absent_upstream` | ok | MISSING |",
                "| Windows | Not proved: Windows only | - | not run | not run |",
            ]
        );
        assert_eq!(
            problems,
            [
                "`grep`: fixed_upstream passes upstream: upstream has fixed this: drop the patch and the row",
                "`grep`: broken_here does not pass on the fork (FAILED)",
                "`grep`: absent_upstream did not compile or run upstream",
                "Odd: unknown platform 'solaris'",
            ]
        );
    }

    #[test]
    fn readmes_upstream_column_must_match_the_proof() {
        let rows = vec![("A".to_string(), "Not proved: no test".to_string())];
        let claims = [claim("A", "linux", "tests/a.rs", "t")];
        let here = found(&[("tests/a.rs", "t", "ok")]);
        let there = found(&[("tests/a.rs", "t", "FAILED")]);
        let (_, problems) = proof(&rows, &claims, &here, &there);
        assert_eq!(
            problems,
            ["A: README says 'Not proved: no test', the proof says 'Not in upstream'"]
        );
        // A row with no proved test has no status, so its cell is not checked.
        let (table, problems) = proof(&rows, &claims, &here, &here);
        assert_eq!(table, ["| A | not proved | `t` | ok | ok |"]);
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn copying_merges_into_an_existing_directory() {
        let root = std::env::temp_dir().join(format!("fork-proof-copy-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("from/sub")).unwrap();
        fs::create_dir_all(root.join("to/sub")).unwrap();
        fs::write(root.join("from/sub/new.txt"), "new").unwrap();
        fs::write(root.join("from/sub/both.txt"), "fork").unwrap();
        fs::write(root.join("to/sub/both.txt"), "upstream").unwrap();
        fs::write(root.join("to/sub/kept.txt"), "kept").unwrap();
        copy_into(&root.join("from"), &root.join("to")).unwrap();
        let read = |p: &str| fs::read_to_string(root.join("to/sub").join(p)).unwrap();
        assert_eq!(
            (read("new.txt"), read("both.txt"), read("kept.txt")),
            ("new".into(), "fork".into(), "kept".into())
        );
        fs::remove_dir_all(&root).unwrap();
    }
}
