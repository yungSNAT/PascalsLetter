//! Smoke suite: runs each rtk command against this repository and checks that
//! it succeeds, prints what it should, or fails where it should.
//!
//! `rtk-dev smoke` needs the installed `rtk` on PATH and runs from the
//! repository root. It prints a line per check and exits with the number of
//! failures (at most 255). Checks that need an optional tool (tree, gh, docker,
//! pytest and so on) are skipped when it is missing.

use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

const RED: &str = "\x1b[0;31m";
const GREEN: &str = "\x1b[0;32m";
const YELLOW: &str = "\x1b[0;33m";
const CYAN: &str = "\x1b[0;36m";
const BOLD: &str = "\x1b[1m";
const NC: &str = "\x1b[0m";
const RULE: &str = "══════════════════════════════════════";

/// Writes to stdout; a closed stdout ends the run, as it ended the script.
fn out(bytes: &[u8]) {
    if io::stdout().write_all(bytes).is_err() {
        std::process::exit(1);
    }
}

/// The first executable `name` on PATH, as `command -v` finds it.
fn on_path(name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    let names: &[String] = &if cfg!(windows) {
        vec![format!("{name}.exe"), name.to_string()]
    } else {
        vec![name.to_string()]
    };
    env::split_paths(&path)
        .flat_map(|dir| names.iter().map(move |n| dir.join(n)))
        .find(|p| is_executable(p))
}

#[cfg(unix)]
fn is_executable(p: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(p: &std::path::Path) -> bool {
    p.is_file()
}

/// Runs `rtk <args>` with stdout and stderr in one stream (`2>&1`): whether it
/// succeeded, its exit code and the output.
fn capture(args: &[&str], input: Option<&[u8]>) -> (bool, Option<i32>, Vec<u8>) {
    let spawned = (|| {
        let (mut reader, writer) = io::pipe()?;
        let mut child = {
            let mut cmd = Command::new("rtk");
            cmd.args(args)
                .stdout(writer.try_clone()?)
                .stderr(writer)
                .stdin(if input.is_some() {
                    Stdio::piped()
                } else {
                    Stdio::inherit()
                });
            cmd.spawn()?
            // `cmd` drops here with its copies of the pipe, so the read ends.
        };
        if let (Some(bytes), Some(mut stdin)) = (input, child.stdin.take()) {
            // A command that exits without reading its input is judged by its status.
            let _ = stdin.write_all(bytes);
        }
        let mut output = Vec::new();
        reader.read_to_end(&mut output)?;
        let status = child.wait()?;
        Ok::<_, io::Error>((status.success(), status.code(), output))
    })();
    spawned.unwrap_or_else(|e| {
        (
            false,
            None,
            format!("rtk-dev smoke: rtk: {e}\n").into_bytes(),
        )
    })
}

/// Whether `rtk <args>` succeeds, output discarded.
fn succeeds(program: &str, args: &[&str]) -> bool {
    let mut cmd = match program {
        "gh" => Command::new("gh"),
        "docker" => Command::new("docker"),
        _ => Command::new("rtk"),
    };
    cmd.args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Output as `$(...)` keeps it: trailing newlines removed.
fn trim_newlines(mut s: &[u8]) -> &[u8] {
    while let [rest @ .., b'\n'] = s {
        s = rest;
    }
    s
}

/// The first 3 lines, as `$(echo "$output" | head -3)` gives them.
fn head3(output: &[u8]) -> &[u8] {
    let s = trim_newlines(output);
    let end = s
        .iter()
        .enumerate()
        .filter(|&(_, &b)| b == b'\n')
        .nth(2)
        .map_or(s.len(), |(i, _)| i);
    trim_newlines(&s[..end])
}

/// `grep -q NEEDLE` for the needles this suite uses: a leading `^` anchors to
/// the line start and `.` matches any character; the rest is literal.
fn grep(output: &[u8], needle: &str) -> bool {
    let (anchored, pat) = match needle.strip_prefix('^') {
        Some(p) => (true, p.as_bytes()),
        None => (false, needle.as_bytes()),
    };
    let matches_at = |line: &[u8], i: usize| {
        line.len() - i >= pat.len()
            && pat
                .iter()
                .zip(&line[i..])
                .all(|(&p, &c)| p == b'.' || p == c)
    };
    trim_newlines(output).split(|&b| b == b'\n').any(|line| {
        if anchored {
            matches_at(line, 0)
        } else {
            (0..=line.len()).any(|i| matches_at(line, i))
        }
    })
}

/// `YYYY-MM-DD HH:MM UTC` for a Unix time (days to civil date by Howard
/// Hinnant's algorithm).
fn utc(secs: u64) -> String {
    let z = (secs / 86_400) as i64 + 719_468;
    let rem = secs % 86_400;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        rem / 3600,
        rem % 3600 / 60
    )
}

#[derive(Default)]
struct Suite {
    pass: u32,
    fail: u32,
    skip: u32,
    failures: Vec<String>,
}

impl Suite {
    fn passed(&mut self, name: &str) {
        self.pass += 1;
        out(format!("  {GREEN}PASS{NC}  {name}\n").as_bytes());
    }

    fn failed(&mut self, entry: String, line: &str, detail: &[&[u8]]) {
        self.fail += 1;
        self.failures.push(entry);
        out(format!("  {RED}FAIL{NC}  {line}\n").as_bytes());
        out(&detail.concat());
    }

    fn section(&self, title: &str) {
        out(format!("\n{BOLD}{CYAN}── {title} ──{NC}\n").as_bytes());
    }

    fn skip(&mut self, name: &str, reason: &str) {
        self.skip += 1;
        out(format!("  {YELLOW}SKIP{NC}  {name} ({reason})\n").as_bytes());
    }

    /// `rtk <args>` succeeds.
    fn ok(&mut self, name: &str, args: &[&str]) {
        let (ok, _, output) = capture(args, None);
        self.judge_ok(name, ok, &output, &format!("rtk {}", args.join(" ")));
    }

    fn judge_ok(&mut self, name: &str, ok: bool, output: &[u8], cmd: &str) {
        if ok {
            self.passed(name);
        } else {
            let cmd = format!("        cmd: {cmd}\n");
            self.failed(
                name.to_string(),
                name,
                &[cmd.as_bytes(), b"        out: ", head3(output), b"\n"],
            );
        }
    }

    /// `rtk <args>` succeeds and prints a line matching `needle`.
    fn contains(&mut self, name: &str, needle: &str, args: &[&str]) {
        let (ok, _, output) = capture(args, None);
        self.judge_contains(name, needle, ok && grep(&output, needle), &output);
    }

    fn judge_contains(&mut self, name: &str, needle: &str, ok: bool, output: &[u8]) {
        if ok {
            self.passed(name);
        } else {
            let expected = format!("        expected: '{needle}'\n");
            self.failed(
                name.to_string(),
                name,
                &[expected.as_bytes(), b"        got: ", head3(output), b"\n"],
            );
        }
    }

    /// `rtk <args>` fails.
    fn fails(&mut self, name: &str, args: &[&str]) {
        if succeeds("rtk", args) {
            self.failed(
                format!("{name} (expected failure, got success)"),
                &format!("{name} (expected failure)"),
                &[],
            );
        } else {
            self.passed(name);
        }
    }

    /// `rtk <args> --help` prints `Usage:`.
    fn help(&mut self, name: &str, args: &[&str]) {
        let args = [args, &["--help"]].concat();
        self.contains(&format!("{name} --help"), "Usage:", &args);
    }

    /// The help check when `tool` is on PATH, else a skip.
    fn help_if(&mut self, tool: &str, name: &str, args: &[&str]) {
        if on_path(tool).is_some() {
            self.help(name, args);
        } else {
            self.skip(name, &format!("{tool} not installed"));
        }
    }
}

/// A temporary file, removed when dropped. Failing to write one ends the run
/// with exit 1, as `set -e` ended the script.
struct TempFile(PathBuf);

impl TempFile {
    fn new(suffix: &str, body: &str) -> Self {
        let path = env::temp_dir().join(format!("rtk-smoke-{}{suffix}", std::process::id()));
        if let Err(e) = fs::write(&path, body) {
            eprintln!("rtk-dev smoke: {}: {e}", path.display());
            std::process::exit(1);
        }
        TempFile(path)
    }

    fn arg(&self) -> String {
        self.0.display().to_string()
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

pub fn run() -> ExitCode {
    let Some(rtk) = on_path("rtk") else {
        out(b"rtk not found in PATH. Run: cargo install --path .\n");
        return ExitCode::from(1);
    };
    let version = Command::new("rtk")
        .arg("--version")
        .stderr(Stdio::inherit())
        .output()
        .map(|o| o.stdout)
        .unwrap_or_default();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    out(format!(
        "{BOLD}RTK Smoke Test Suite{NC}\nBinary: {}\nVersion: ",
        rtk.display()
    )
    .as_bytes());
    out(trim_newlines(&version));
    out(format!("\nDate: {}\n", utc(now)).as_bytes());

    let in_repo = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !in_repo {
        out(b"Must run from inside a git repository.\n");
        return ExitCode::from(1);
    }

    let mut s = Suite::default();
    checks(&mut s);
    out(format!(
        "\n{BOLD}{RULE}{NC}\n{BOLD}Results: {GREEN}{} passed{NC}, {RED}{} failed{NC}, {YELLOW}{} skipped{NC}\n",
        s.pass, s.fail, s.skip
    )
    .as_bytes());
    if !s.failures.is_empty() {
        out(format!("\n{RED}Failures:{NC}\n").as_bytes());
        for f in &s.failures {
            out(format!("  - {f}\n").as_bytes());
        }
    }
    out(format!("{BOLD}{RULE}{NC}\n").as_bytes());
    ExitCode::from(u8::try_from(s.fail).unwrap_or(u8::MAX))
}

fn checks(s: &mut Suite) {
    s.section("Version & Help");
    s.contains("rtk --version", "rtk", &["--version"]);
    s.contains("rtk --help", "Usage:", &["--help"]);

    s.section("Ls");
    s.ok("rtk ls .", &["ls", "."]);
    s.ok("rtk ls -la .", &["ls", "-la", "."]);
    s.ok("rtk ls -lh .", &["ls", "-lh", "."]);
    s.ok("rtk ls -l src/", &["ls", "-l", "src/"]);
    s.ok("rtk ls src/ -l (flag after)", &["ls", "src/", "-l"]);
    s.ok("rtk ls multi paths", &["ls", "src/", "scripts/"]);
    s.contains("rtk ls -a shows hidden", ".git", &["ls", "-a", "."]);
    s.contains("rtk ls shows sizes", "K", &["ls", "src/"]);
    s.contains("rtk ls shows dirs with /", "/", &["ls", "."]);

    s.section("Tree");
    if on_path("tree").is_some() {
        s.ok("rtk tree .", &["tree", "."]);
        s.ok("rtk tree -L 2 .", &["tree", "-L", "2", "."]);
        s.ok("rtk tree -d -L 1 .", &["tree", "-d", "-L", "1", "."]);
        s.contains("rtk tree shows src/", "src", &["tree", "-L", "1", "."]);
    } else {
        s.skip("rtk tree", "tree not installed");
    }

    s.section("Read");
    s.ok("rtk read Cargo.toml", &["read", "Cargo.toml"]);
    s.ok(
        "rtk read --level none Cargo.toml",
        &["read", "--level", "none", "Cargo.toml"],
    );
    s.ok(
        "rtk read --level aggressive Cargo.toml",
        &["read", "--level", "aggressive", "Cargo.toml"],
    );
    s.ok("rtk read -n Cargo.toml", &["read", "-n", "Cargo.toml"]);
    s.ok(
        "rtk read --max-lines 5 Cargo.toml",
        &["read", "--max-lines", "5", "Cargo.toml"],
    );

    s.section("Read (stdin support)");
    let (ok, _, output) = capture(&["read", "-"], Some(b"fn main() {}\n"));
    s.judge_ok(
        "rtk read stdin pipe",
        ok,
        &output,
        "bash -c echo \"fn main() {}\" | rtk read -",
    );

    s.section("Git (existing)");
    s.ok("rtk git status", &["git", "status"]);
    s.ok("rtk git status --short", &["git", "status", "--short"]);
    s.ok("rtk git status -s", &["git", "status", "-s"]);
    s.ok(
        "rtk git status --porcelain",
        &["git", "status", "--porcelain"],
    );
    s.ok("rtk git log", &["git", "log"]);
    s.ok("rtk git log -5", &["git", "log", "--", "-5"]);
    s.ok("rtk git diff", &["git", "diff"]);
    s.ok("rtk git diff --stat", &["git", "diff", "--stat"]);

    s.section("Git (new: branch, fetch, stash, worktree)");
    s.ok("rtk git branch", &["git", "branch"]);
    s.ok("rtk git fetch", &["git", "fetch"]);
    s.ok("rtk git stash list", &["git", "stash", "list"]);
    s.ok("rtk git worktree", &["git", "worktree"]);

    s.section("Git (passthrough: unsupported subcommands)");
    s.ok("rtk git tag --list", &["git", "tag", "--list"]);
    s.ok("rtk git remote -v", &["git", "remote", "-v"]);
    s.ok("rtk git rev-parse HEAD", &["git", "rev-parse", "HEAD"]);

    s.section("GitHub CLI");
    if on_path("gh").is_some() && succeeds("gh", &["auth", "status"]) {
        s.ok("rtk gh pr list", &["gh", "pr", "list"]);
        s.ok("rtk gh run list", &["gh", "run", "list"]);
        s.ok("rtk gh issue list", &["gh", "issue", "list"]);
        // pr create, merge, diff, comment and edit write, so only their help.
        s.help("rtk gh", &["gh"]);
    } else {
        s.skip("gh commands", "gh not authenticated");
    }

    s.section("Cargo (new)");
    s.ok("rtk cargo build", &["cargo", "build"]);
    s.ok("rtk cargo clippy", &["cargo", "clippy"]);
    // Judged by its output, not its exit code: a failing test still reports.
    let (_, _, output) = capture(&["cargo", "test"], None);
    if ["FAILURES", "test result:", "passed"]
        .iter()
        .any(|n| grep(&output, n))
    {
        s.passed("rtk cargo test");
    } else {
        s.failed(
            "rtk cargo test".to_string(),
            "rtk cargo test",
            &[b"        got: ", head3(&output), b"\n"],
        );
    }
    s.help("rtk cargo", &["cargo"]);

    s.section("Curl (new)");
    s.contains(
        "rtk curl JSON detect",
        "string",
        &["curl", "https://mockhttp.org/json"],
    );
    s.ok(
        "rtk curl plain text",
        &["curl", "https://mockhttp.org/robots.txt"],
    );
    s.help("rtk curl", &["curl"]);

    s.section("Npm / Npx (new)");
    s.help("rtk npm", &["npm"]);
    s.help("rtk npx", &["npx"]);

    s.section("Pnpm");
    s.help("rtk pnpm", &["pnpm"]);
    s.help("rtk pnpm build", &["pnpm", "build"]);
    s.help("rtk pnpm typecheck", &["pnpm", "typecheck"]);
    if on_path("pnpm").is_some() {
        s.ok("rtk pnpm help", &["pnpm", "help"]);
    }

    s.section("Grep");
    // `grep PATTERN <dir>` needs -r, or it exits 2 ("Is a directory").
    s.ok("rtk grep pattern", &["grep", "-r", "pub fn", "src/"]);
    s.contains(
        "rtk grep finds results",
        "pub fn",
        &["grep", "-r", "pub fn", "src/"],
    );
    // `-t` is rg-only. The pattern matches, the path is one file and `-t` comes
    // first, so `-t` is the only reason to fail.
    s.fails(
        "rtk grep -t rejected by grep",
        &["grep", "-t", "rust", "fn main", "src/main.rs"],
    );
    if on_path("rg").is_some() {
        s.ok(
            "rtk rg with file type",
            &["rg", "pub fn", "src/", "-t", "rust"],
        );
    } else {
        s.skip("rtk rg with file type", "rg not installed");
    }

    s.section("Grep (extra args passthrough)");
    s.ok(
        "rtk grep -i case insensitive",
        &["grep", "fn", "src/", "-i"],
    );
    s.ok(
        "rtk grep -A context lines",
        &["grep", "fn run", "src/", "-A", "2"],
    );

    s.section("Find");
    s.ok("rtk find *.rs", &["find", "*.rs", "src/"]);
    s.contains("rtk find shows files", ".rs", &["find", "*.rs", "src/"]);

    s.section("Json");
    let json = TempFile::new(
        ".json",
        "{\"name\":\"test\",\"count\":42,\"items\":[1,2,3]}\n",
    );
    s.ok("rtk json file", &["json", &json.arg()]);
    s.contains("rtk json shows schema", "string", &["json", &json.arg()]);
    drop(json);

    s.section("Deps");
    s.ok("rtk deps .", &["deps", "."]);
    s.contains("rtk deps shows Cargo", "Cargo", &["deps", "."]);

    s.section("Env");
    s.ok("rtk env", &["env"]);
    s.ok("rtk env --filter PATH", &["env", "--filter", "PATH"]);

    s.section("Log");
    let log = "[2025-01-01 12:00:00] INFO: repeated message\n".repeat(20)
        + "[2025-01-01 12:00:01] ERROR: something failed\n";
    let log = TempFile::new(".log", &log);
    s.ok("rtk log file", &["log", &log.arg()]);
    drop(log);

    s.section("Summary");
    s.ok("rtk summary echo hello", &["summary", "echo", "hello"]);

    s.section("Err");
    s.ok("rtk err echo ok", &["err", "echo", "ok"]);

    s.section("Test runner");
    s.ok("rtk test echo ok", &["test", "echo", "ok"]);

    s.section("Gain");
    s.ok("rtk gain", &["gain"]);
    s.ok("rtk gain --history", &["gain", "--history"]);

    s.section("Config & Init");
    s.ok("rtk config", &["config"]);
    s.ok("rtk init --show", &["init", "--show"]);

    s.section("Wget");
    if on_path("wget").is_some() {
        s.ok(
            "rtk wget stdout",
            &["wget", "https://mockhttp.org/robots.txt", "-O"],
        );
    } else {
        s.skip("rtk wget", "wget not installed");
    }

    s.section("JS Tooling (help only, no project context)");
    for tool in ["tsc", "lint", "prettier", "next", "playwright"] {
        s.help(&format!("rtk {tool}"), &[tool]);
    }

    s.section("Prisma (help only)");
    s.help("rtk prisma", &["prisma"]);

    s.section("Vitest (help only)");
    s.help("rtk vitest", &["vitest"]);

    s.section("Docker / Kubectl (help only)");
    s.help("rtk docker", &["docker"]);
    s.help("rtk kubectl", &["kubectl"]);

    // The script passed `--help` itself here, so these run `--help --help`.
    s.section("Python (conditional)");
    for tool in ["pytest", "ruff", "pip"] {
        s.help_if(tool, &format!("rtk {tool}"), &[tool, "--help"]);
    }

    s.section("Go (conditional)");
    if on_path("go").is_some() {
        s.help("rtk go", &["go", "--help"]);
        s.help("rtk go test", &["go", "test", "-h"]);
        s.help("rtk go build", &["go", "build", "-h"]);
        s.help("rtk go vet", &["go", "vet", "-h"]);
    } else {
        s.skip("rtk go", "go not installed");
    }
    s.help_if(
        "golangci-lint",
        "rtk golangci-lint",
        &["golangci-lint", "--help"],
    );

    s.section("Graphite (conditional)");
    if on_path("gt").is_some() {
        s.help("rtk gt", &["gt", "--help"]);
        s.ok("rtk gt log short", &["gt", "log", "short"]);
    } else {
        s.skip("rtk gt", "gt not installed");
    }

    s.section("Ruby (conditional)");
    for tool in ["rspec", "rubocop", "rake"] {
        s.help_if(tool, &format!("rtk {tool}"), &[tool, "--help"]);
    }

    s.section("Global flags");
    s.ok("rtk -u ls .", &["-u", "ls", "."]);
    s.ok(
        "rtk --skip-env npm --help",
        &["--skip-env", "npm", "--help"],
    );

    s.section("CcEconomics");
    s.ok("rtk cc-economics", &["cc-economics"]);

    s.section("Learn");
    s.ok("rtk learn --help", &["learn", "--help"]);
    s.ok("rtk learn (no sessions)", &["learn", "--since", "0"]);

    s.section("Rewrite");
    s.contains(
        "rewrite git status",
        "rtk git status",
        &["rewrite", "git status"],
    );
    s.contains(
        "rewrite cargo test",
        "rtk cargo test",
        &["rewrite", "cargo test"],
    );
    s.contains(
        "rewrite compound &&",
        "rtk git status",
        &["rewrite", "git status && cargo test"],
    );
    s.contains(
        "rewrite pipe preserves",
        "| head",
        &["rewrite", "git log | head"],
    );

    s.section("Rewrite (#345: RTK_DISABLED skip)");
    s.fails(
        "rewrite RTK_DISABLED=1 skip",
        &["rewrite", "RTK_DISABLED=1 git status"],
    );
    s.fails(
        "rewrite env RTK_DISABLED skip",
        &["rewrite", "FOO=1 RTK_DISABLED=1 cargo test"],
    );

    s.section("Rewrite (#346: 2>&1 preserved)");
    s.contains(
        "rewrite 2>&1 preserved",
        "2>&1",
        &["rewrite", "cargo test 2>&1 | head"],
    );

    s.section("Rewrite (#196: gh --json skip)");
    s.fails(
        "rewrite gh --json skip",
        &["rewrite", "gh pr list --json number"],
    );
    s.fails(
        "rewrite gh --jq skip",
        &["rewrite", "gh api /repos --jq .name"],
    );
    s.fails(
        "rewrite gh --template skip",
        &["rewrite", "gh pr view 1 --template '{{.title}}'"],
    );
    s.contains(
        "rewrite gh normal works",
        "rtk gh pr list",
        &["rewrite", "gh pr list"],
    );

    s.section("Verify");
    s.ok("rtk verify", &["verify"]);

    s.section("Proxy");
    s.ok("rtk bypass echo hello", &["bypass", "echo", "hello"]);
    s.contains(
        "rtk bypass passthrough",
        "hello",
        &["bypass", "echo", "hello"],
    );

    s.section("Discover");
    s.ok("rtk discover", &["discover"]);

    s.section("Diff");
    s.ok(
        "rtk diff identical files",
        &["diff", "Cargo.toml", "Cargo.toml"],
    );
    s.fails(
        "rtk diff differing files",
        &["diff", "Cargo.toml", "LICENSE"],
    );
    // Differing files exit 1 and print the classic `<` lines.
    let (_, code, output) = capture(&["diff", "Cargo.toml", "LICENSE"], None);
    s.judge_contains(
        "rtk diff shows classic changes",
        "^< ",
        code == Some(1) && grep(&output, "^< "),
        &output,
    );

    s.section("Wc");
    s.ok("rtk wc Cargo.toml", &["wc", "Cargo.toml"]);

    s.section("Smart");
    s.ok("rtk smart src/main.rs", &["smart", "src/main.rs"]);

    s.section("Json (edge cases)");
    s.fails("rtk json on TOML (#347)", &["json", "Cargo.toml"]);

    s.section("Docker (conditional)");
    if on_path("docker").is_some() && succeeds("docker", &["info"]) {
        s.ok("rtk docker ps", &["docker", "ps"]);
        s.ok("rtk docker images", &["docker", "images"]);
    } else {
        s.skip("rtk docker", "docker not running");
    }

    s.section("Hook check (#344)");
    s.contains(
        "rtk init --show hook version",
        "version",
        &["init", "--show"],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grep_anchors_and_wildcards_like_basic_regex() {
        assert!(grep(b"xgit\n", ".git"));
        assert!(!grep(b"git\n", ".git"));
        // `.` takes the last byte of a multibyte character where grep takes the character.
        assert!(grep("égit\n".as_bytes(), ".git"));
        assert!(grep(b"a\n< b\n", "^< "));
        assert!(!grep(b"a < b\n", "^< "));
        assert!(grep(b"x | head\n", "| head"));
        assert!(!grep(b"\n\n", "K"));
    }

    #[test]
    fn head3_keeps_three_lines_as_command_substitution_does() {
        assert_eq!(head3(b"a\nb\nc\nd\n"), b"a\nb\nc");
        assert_eq!(head3(b"a\n\n\nd\n"), b"a");
        assert_eq!(head3(b"\n\n"), b"");
        assert_eq!(head3(b"a\n\nc"), b"a\n\nc");
    }

    #[test]
    fn utc_formats_known_times() {
        assert_eq!(utc(0), "1970-01-01 00:00 UTC");
        assert_eq!(utc(951_782_400), "2000-02-29 00:00 UTC");
        assert_eq!(utc(1_795_997_100), "2026-11-30 00:05 UTC");
    }
}
