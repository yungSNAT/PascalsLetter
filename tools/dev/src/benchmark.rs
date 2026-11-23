//! Token savings of each rtk filter against the raw command it replaces.
//!
//! Run from the repository root. `RTK_BIN` names the binary to measure;
//! otherwise `./target/release/rtk`, then `rtk` on PATH. Each row runs the raw
//! command and the rtk command through `bash -c 'eval "$1" 2>&1'`, so the rows
//! keep their shell syntax, and counts tokens as (characters + 3) / 4. Rows for a
//! tool that is not installed are skipped. Without `CI` set, each row's outputs
//! go to `scripts/benchmark/{unix,rtk,diff}/`. Exits 1 when a filter returns
//! more tokens than the raw output or nothing at all.
//!
//! Unix only: it needs bash and the measured tools.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::thread::sleep;
use std::time::Duration;

// cargo prints no colour into a pipe, which is how agents read it.
const CARGO_COLOUR: &str = "never";
const DOUBLE_RULE: &str = "═";
const SINGLE_RULE: &str = "─";
const RULE_WIDTH: usize = 139;

const BENCH_JSON: &str = r#"{
  "name": "rtk",
  "version": "0.2.1",
  "config": {
    "debug": false,
    "max_depth": 10,
    "filters": ["node_modules", "target", ".git"]
  },
  "dependencies": {
    "serde": "1.0",
    "clap": "4.0",
    "anyhow": "1.0"
  }
}
"#;

const SAMPLE_LOG: &str = "2024-01-15 10:00:01 INFO  Application started
2024-01-15 10:00:02 INFO  Loading configuration
2024-01-15 10:00:03 ERROR Connection failed: timeout
2024-01-15 10:00:04 ERROR Connection failed: timeout
2024-01-15 10:00:05 ERROR Connection failed: timeout
2024-01-15 10:00:06 ERROR Connection failed: timeout
2024-01-15 10:00:07 ERROR Connection failed: timeout
2024-01-15 10:00:08 WARN  Retrying connection
2024-01-15 10:00:09 INFO  Connection established
2024-01-15 10:00:10 INFO  Processing request
2024-01-15 10:00:11 INFO  Processing request
2024-01-15 10:00:12 INFO  Processing request
2024-01-15 10:00:13 INFO  Request completed
";

// Pretty-printed on purpose, so rtk has JSON to minify.
const NET_DATA_JSON: &str = r#"{
    "message": "Hello from RTK benchmark",
    "status": "success",
    "code": 200,
    "items": [
        { "id": 1, "name": "first" },
        { "id": 2, "name": "second" }
    ]
}
"#;

const NET_ROBOTS: &str = "User-agent: *
Disallow: /private/
Allow: /
Sitemap: https://example.com/sitemap.xml
";

const PY_PROJECT: &str = "[project]
name = \"rtk-bench\"
version = \"0.1.0\"

[tool.ruff]
line-length = 88
";

const PY_SAMPLE: &str = "import os
import sys
import json


def process_data(x):
    if x == None:  # E711: comparison to None
        return []
    result = []
    for i in range(len(x)):  # C416: unnecessary list comprehension
        result.append(x[i] * 2)
    return result

def unused_function():  # F841: local variable assigned but never used
    temp = 42
    return None
";

const PY_TEST: &str = "from sample import process_data

def test_process_data():
    assert process_data([1, 2, 3]) == [2, 4, 6]

def test_process_data_none():
    assert process_data(None) == []
";

const GO_MOD: &str = "module bench

go 1.21
";

// The ignored error returns are deliberate: the golangci-lint row only measures
// the filter if errcheck has findings to compress.
const GO_MAIN: &str = r#"package main

import (
    "fmt"
    "os"
)

func Add(a, b int) int {
    return a + b
}

func Multiply(a, b int) int {
    return a * b
}

func readConfig() {
    f, _ := os.Open("config.yml")
    defer f.Close()
    fmt.Println(f != nil)
}

func writeCache() {
    os.Remove("cache.tmp")
    os.Chmod("cache.tmp", 0o600)
}

func exportEnv() {
    os.Setenv("BENCH_MODE", "on")
    os.Unsetenv("BENCH_DEBUG")
}

func reportStatus() {
    fmt.Fprintf(os.Stderr, "status: %s\n", "ok")
    fmt.Fprintln(os.Stderr, "done")
}

func rotateLogs() {
    os.Truncate("bench.log", 0)
    os.Rename("bench.log", "bench.log.1")
}

func main() {
    fmt.Println(Add(2, 3))
    fmt.Println(Multiply(4, 5))
    readConfig()
    writeCache()
    exportEnv()
    reportStatus()
    rotateLogs()
}
"#;

const GO_TEST: &str = r#"package main

import "testing"

func TestAdd(t *testing.T) {
    result := Add(2, 3)
    if result != 5 {
        t.Errorf("Add(2, 3) = %d; want 5", result)
    }
}

func TestMultiply(t *testing.T) {
    result := Multiply(4, 5)
    if result != 20 {
        t.Errorf("Multiply(4, 5) = %d; want 20", result)
    }
}
"#;

#[derive(Clone, Copy, PartialEq)]
enum Tag {
    Good,
    Fail,
    Neg,
    Warn,
    Skip,
}

struct Bench {
    rtk: String,
    ci: bool,
    bench_dir: PathBuf,
    total_unix: i64,
    total_rtk: i64,
    tests: i64,
    good: i64,
    fail: i64,
    warn: i64,
    negative: i64,
}

pub fn run() -> ExitCode {
    let root = logical_cwd();
    let Some(rtk) = find_rtk(&root) else {
        return ExitCode::from(1);
    };
    let ci = std::env::var_os("CI").is_some_and(|v| !v.is_empty());
    let mut b = Bench {
        rtk,
        ci,
        bench_dir: root.join("scripts/benchmark"),
        total_unix: 0,
        total_rtk: 0,
        tests: 0,
        good: 0,
        fail: 0,
        warn: 0,
        negative: 0,
    };

    // Only the gitignored output subdirectories are wiped between local runs.
    if !b.ci {
        for sub in ["unix", "rtk", "diff"] {
            let dir = b.bench_dir.join(sub);
            let _ = fs::remove_dir_all(&dir);
            if let Err(e) = fs::create_dir_all(&dir) {
                eprintln!("rtk-dev benchmark: {}: {e}", dir.display());
                return ExitCode::from(1);
            }
        }
    }

    println!("RTK Benchmark");
    println!("{}", DOUBLE_RULE.repeat(RULE_WIDTH));
    println!(
        "   {:<24} │ {:<40} │ {:<40} │ TOKENS",
        "TEST", "SHELL", "RTK"
    );
    println!("{}", SINGLE_RULE.repeat(RULE_WIDTH));

    // Like the script under `set -e`, a fixture or output file that cannot be
    // written stops the run.
    let mut net = NetFixtures::default();
    let rows = b.rows(&root, &mut net);
    net.cleanup();
    if let Err(e) = rows {
        eprintln!("rtk-dev benchmark: {e}");
        return ExitCode::from(1);
    }
    b.summary()
}

impl Bench {
    /// A row whose rtk side is `$RTK <rtk_args>`.
    fn b(&mut self, name: &str, unix_cmd: &str, rtk_args: &str) -> io::Result<()> {
        let rtk_cmd = self.r(rtk_args);
        self.bench(name, unix_cmd, &rtk_cmd)?;
        Ok(())
    }

    fn r(&self, args: &str) -> String {
        format!("{} {args}", self.rtk)
    }

    fn rows(&mut self, root: &Path, net: &mut NetFixtures) -> io::Result<()> {
        section("ls");
        self.b("ls", "ls -la", "ls")?;
        self.b("ls src/", "ls -la src/", "ls src/")?;
        self.b("ls -l src/", "ls -l src/", "ls -l src/")?;
        self.b("ls -la src/", "ls -la src/", "ls -la src/")?;
        self.b("ls -lh src/", "ls -lh src/", "ls -lh src/")?;
        self.b("ls src/ -l", "ls -l src/", "ls src/ -l")?;
        self.b("ls -a", "ls -la", "ls -a")?;
        self.b("ls multi", "ls -la src/ scripts/", "ls src/ scripts/")?;

        if have("tree") {
            section("tree");
            self.b("tree", "tree -L 2 -I target", "tree -L 2")?;
            self.b("tree src/", "tree src/ -L 2", "tree src/ -L 2")?;
        } else {
            skipped("\n⏭️  tree (not installed, skipped)");
        }

        section("read");
        // The CLI lives in src/lib.rs; src/main.rs is a 3-line shim.
        self.b("read", "cat src/lib.rs", "read src/lib.rs")?;
        self.b(
            "read -l minimal",
            "cat src/lib.rs",
            "read src/lib.rs -l minimal",
        )?;
        self.b(
            "read -l aggressive",
            "cat src/lib.rs",
            "read src/lib.rs -l aggressive",
        )?;
        self.b("read -n", "cat -n src/lib.rs", "read src/lib.rs -n")?;

        section("find");
        let find_all = "find . -not -path './target/*' -not -path './.git/*' -type f";
        self.b("find *", find_all, "find '*'")?;
        self.b(
            "find *.rs",
            "find . -not -path './target/*' -not -path './.git/*' -name '*.rs' -type f",
            "find '*.rs'",
        )?;
        // `rtk find --max N` caps the names displayed but still scans the whole
        // tree, so the honest baseline is the full `find` a user would otherwise
        // read. One row carries the savings: the baseline is the same for every N.
        self.b("find --max 100", find_all, "find '*' --max 100")?;
        // That baseline cannot detect a --max that stops limiting, so assert the cap.
        self.find_max(10)?;
        self.find_max(100)?;

        section("git");
        self.b("git status", "git status", "git status")?;
        self.b("git log -n 10", "git log -10", "git log -n 10")?;
        self.b("git log -n 5", "git log -5", "git log -n 5")?;
        self.b(
            "git diff",
            "git diff HEAD~1 2>/dev/null || echo ''",
            "git diff HEAD~1",
        )?;
        self.b(
            "git show",
            "git show HEAD --stat 2>/dev/null || true",
            "git show HEAD --stat",
        )?;

        section("grep");
        self.b(
            "grep fn",
            "grep -rn 'fn ' src/ || true",
            "grep -rn 'fn ' src/",
        )?;
        self.b(
            "grep struct",
            "grep -rn 'struct ' src/ || true",
            "grep -rn 'struct ' src/",
        )?;
        self.b(
            "grep --max-len 40",
            "grep -rn 'fn ' src/ || true",
            "grep --max-len 40 -rn 'fn ' src/",
        )?;
        self.b(
            "grep -c",
            "grep -ron 'fn ' src/ || true",
            "grep -rc 'fn ' src/",
        )?;

        // Native ripgrep: recursive by default, same output filter.
        if have("rg") {
            section("rg");
            self.b("rg fn", "rg -n 'fn ' src/ || true", "rg 'fn ' src/")?;
            self.b(
                "rg struct",
                "rg -n 'struct ' src/ || true",
                "rg 'struct ' src/",
            )?;
            self.b(
                "rg -l files",
                "rg -l 'fn ' src/ || true",
                "rg -l 'fn ' src/",
            )?;
            self.b(
                "rg -c count",
                "rg -c 'fn ' src/ || true",
                "rg -c 'fn ' src/",
            )?;
        } else {
            skipped("\n⏭️  rg (not installed, skipped)");
        }

        section("json");
        let json = "/tmp/rtk_bench.json";
        write(json, BENCH_JSON)?;
        self.bench(
            "json",
            &format!("cat {json}"),
            &self.r(&format!("json {json}")),
        )?;
        self.bench(
            "json -d 2",
            &format!("cat {json}"),
            &self.r(&format!("json {json} -d 2")),
        )?;
        let _ = fs::remove_file(json);

        section("deps");
        self.b("deps", "cat Cargo.toml", "deps")?;

        section("env");
        self.b("env", "env", "env")?;
        // `-f` keeps every variable whose name holds the text in any case, such as
        // the runners' PSModulePath; a plain `grep PATH` would miss those.
        self.b("env -f PATH", "env | grep -i '^[^=]*path'", "env -f PATH")?;

        section("err");
        if have("cargo") {
            precompile(&["cargo", "build"]);
            self.b(
                "err cargo build",
                "cargo build 2>&1 || true",
                "err cargo build 2>&1",
            )?;
        } else {
            skipped("⏭️  err cargo build (cargo not in PATH, skipped)");
        }

        section("test");
        if have("cargo") {
            precompile(&["cargo", "test", "--lib", "--no-run"]);
            self.b(
                "test cargo test",
                "cargo test --lib 2>&1 || true",
                "test cargo test --lib 2>&1",
            )?;
        } else {
            skipped("⏭️  test cargo test (cargo not in PATH, skipped)");
        }

        section("log");
        let log = "/tmp/rtk_bench_sample.log";
        write(log, SAMPLE_LOG)?;
        self.bench("log", &format!("cat {log}"), &self.r(&format!("log {log}")))?;
        let _ = fs::remove_file(log);

        section("summary");
        if have("cargo") {
            self.b(
                "summary cargo --help",
                "cargo --help",
                "summary cargo --help",
            )?;
        } else {
            skipped("⏭️  summary cargo --help (cargo not in PATH, skipped)");
        }
        if have("rustc") {
            self.b(
                "summary rustc --help",
                "rustc --help 2>/dev/null || echo 'rustc not found'",
                "summary rustc --help",
            )?;
        } else {
            skipped("⏭️  summary rustc --help (rustc not in PATH, skipped)");
        }

        section("cargo");
        if have("cargo") {
            precompile(&["cargo", "build"]);
            self.b(
                "cargo build",
                "cargo build 2>&1 || true",
                "cargo build 2>&1",
            )?;
            precompile(&["cargo", "test", "--lib", "--no-run"]);
            self.b(
                "cargo test",
                "cargo test --lib 2>&1 || true",
                "cargo test --lib 2>&1",
            )?;
            precompile(&["cargo", "clippy"]);
            self.b(
                "cargo clippy",
                "cargo clippy 2>&1 || true",
                "cargo clippy 2>&1",
            )?;
            precompile(&["cargo", "check"]);
            self.b(
                "cargo check",
                "cargo check 2>&1 || true",
                "cargo check 2>&1",
            )?;
        } else {
            skipped("⏭️  cargo build/test/clippy/check (cargo not in PATH, skipped)");
        }

        section("smart");
        self.b("smart main.rs", "cat src/main.rs", "smart src/main.rs")?;

        section("wc");
        self.b(
            "wc",
            "wc Cargo.toml src/main.rs",
            "wc Cargo.toml src/main.rs",
        )?;

        self.net_rows(net)?;
        self.js_rows()?;
        self.service_rows()?;
        self.python_rows(root)?;
        self.go_rows(root)?;
        self.misc_rows()?;
        self.rewrite_rows()?;
        Ok(())
    }

    // curl and wget, fully offline: fixed fixtures served from a loopback server,
    // so the responses carry real headers and the JSON gets minified.
    fn net_rows(&mut self, net: &mut NetFixtures) -> io::Result<()> {
        net.setup()?;
        let url = net.url.clone();

        section("curl");
        if have("curl") {
            if let Some(url) = &url {
                self.bench(
                    "curl json",
                    &format!("curl -s {url}/data.json"),
                    &self.r(&format!("curl {url}/data.json")),
                )?;
                self.bench(
                    "curl text",
                    &format!("curl -s {url}/robots.txt"),
                    &self.r(&format!("curl {url}/robots.txt")),
                )?;
            } else {
                // No python3 to host a server: file:// carries no headers, so no
                // JSON minification, but it stays offline and deterministic.
                let dir = net.dir.display().to_string();
                self.bench(
                    "curl json",
                    &format!("curl -s file://{dir}/data.json"),
                    &self.r(&format!("curl file://{dir}/data.json")),
                )?;
                self.bench(
                    "curl text",
                    &format!("curl -s file://{dir}/robots.txt"),
                    &self.r(&format!("curl file://{dir}/robots.txt")),
                )?;
            }
        }

        // wget rejects file://, so without the server there is nothing to fetch.
        if have("wget") {
            section("wget");
            if let Some(url) = &url {
                self.bench(
                    "wget",
                    &format!("wget -qO- {url}/data.json"),
                    &self.r(&format!("wget {url}/data.json")),
                )?;
            } else {
                skipped("⏭️  wget (no local HTTP server available, skipped)");
            }
        }

        // npm needs no package.json.
        if have("npm") {
            section("npm");
            self.b(
                "npm list",
                "npm list -g --depth 0 2>&1 || true",
                "npm list -g --depth 0",
            )?;
        }
        Ok(())
    }

    fn js_rows(&mut self) -> io::Result<()> {
        if !is_file("package.json") {
            return Ok(());
        }
        section("modern JS stack");
        let js = |tool: &str| have(tool) || is_file(&format!("node_modules/.bin/{tool}"));
        if js("tsc") {
            self.b("tsc", "tsc --noEmit 2>&1 || true", "tsc --noEmit 2>&1")?;
        }
        if js("prettier") {
            self.b(
                "prettier --check",
                "prettier --check . 2>&1 || true",
                "prettier --check .",
            )?;
        }
        if js("eslint") {
            self.b("lint", "eslint . 2>&1 || true", "lint .")?;
        }
        if ["next.config.js", "next.config.mjs", "next.config.ts"]
            .iter()
            .any(|f| is_file(f))
            && js("next")
        {
            self.b("next build", "next build 2>&1 || true", "next build")?;
        }
        if ["playwright.config.ts", "playwright.config.js"]
            .iter()
            .any(|f| is_file(f))
            && js("playwright")
        {
            self.b(
                "playwright test",
                "playwright test 2>&1 || true",
                "playwright test",
            )?;
        }
        if is_file("prisma/schema.prisma") && js("prisma") {
            self.b(
                "prisma generate",
                "prisma generate 2>&1 || true",
                "prisma generate",
            )?;
        }
        if js("vitest") {
            self.b(
                "vitest",
                "vitest run --reporter=json 2>&1 || true",
                "vitest",
            )?;
        }
        if have("pnpm") {
            self.b(
                "pnpm list",
                "pnpm list --depth 0 2>&1 || true",
                "pnpm list --depth 0",
            )?;
            self.b(
                "pnpm outdated",
                "pnpm outdated 2>&1 || true",
                "pnpm outdated",
            )?;
        }
        Ok(())
    }

    fn service_rows(&mut self) -> io::Result<()> {
        if shell_ok("command -v gh && git rev-parse --git-dir && gh auth status") {
            section("gh");
            self.b("gh pr list", "gh pr list 2>&1 || true", "gh pr list")?;
            self.b("gh run list", "gh run list 2>&1 || true", "gh run list")?;
        }
        if have("glab") {
            section("glab");
            self.b("glab mr list", "glab mr list 2>&1 || true", "glab mr list")?;
            self.b(
                "glab issue list",
                "glab issue list 2>&1 || true",
                "glab issue list",
            )?;
        }
        if have("gt") {
            section("gt");
            self.b("gt log", "gt log 2>&1 || true", "gt log")?;
        }
        if have("docker") {
            section("docker");
            self.b("docker ps", "docker ps 2>/dev/null || true", "docker ps")?;
            self.b(
                "docker images",
                "docker images 2>/dev/null || true",
                "docker images",
            )?;
        }
        if have("kubectl") {
            section("kubectl");
            self.b(
                "kubectl pods",
                "kubectl get pods 2>/dev/null || true",
                "kubectl pods",
            )?;
            self.b(
                "kubectl services",
                "kubectl get services 2>/dev/null || true",
                "kubectl services",
            )?;
        }
        Ok(())
    }

    fn python_rows(&mut self, root: &Path) -> io::Result<()> {
        if !(have("python3") && have("ruff") && have("pytest")) {
            return Ok(());
        }
        section("python");
        let dir = mktemp(true)?;
        cd(&dir)?;
        write("pyproject.toml", PY_PROJECT)?;
        write("sample.py", PY_SAMPLE)?;
        write("test_sample.py", PY_TEST)?;
        self.b("ruff check", "ruff check . 2>&1 || true", "ruff check .")?;
        self.b("pytest", "pytest -v 2>&1 || true", "pytest -v")?;
        if have("pip") {
            self.b("pip list", "pip list 2>&1 || true", "pip list")?;
        }
        if have("mypy") {
            self.b("mypy", "mypy sample.py 2>&1 || true", "mypy sample.py")?;
        }
        cd(root)?;
        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    fn go_rows(&mut self, root: &Path) -> io::Result<()> {
        if !(have("go") && have("golangci-lint")) {
            return Ok(());
        }
        section("go");
        let dir = mktemp(true)?;
        cd(&dir)?;
        write("go.mod", GO_MOD)?;
        write("main.go", GO_MAIN)?;
        write("main_test.go", GO_TEST)?;
        self.b(
            "golangci-lint",
            "golangci-lint run 2>&1 || true",
            "golangci-lint run",
        )?;
        self.b("go test", "go test -v 2>&1 || true", "go test -v")?;
        self.b("go build", "go build ./... 2>&1 || true", "go build ./...")?;
        self.b("go vet", "go vet ./... 2>&1 || true", "go vet ./...")?;
        cd(root)?;
        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    fn misc_rows(&mut self) -> io::Result<()> {
        if have("ruby") {
            section("ruby");
            if have("rake") {
                self.b("rake -T", "rake -T 2>&1 || true", "rake -T")?;
            }
            if have("rubocop") {
                self.b(
                    "rubocop",
                    "rubocop --format simple 2>&1 || true",
                    "rubocop --format simple",
                )?;
            }
            if have("rspec") {
                self.b(
                    "rspec --dry-run",
                    "rspec --dry-run 2>&1 || true",
                    "rspec --dry-run",
                )?;
            }
        }
        if have("dotnet") {
            section("dotnet");
            self.b(
                "dotnet --info",
                "dotnet --info 2>&1 || true",
                "dotnet --info",
            )?;
        }
        if have("aws") {
            section("aws");
            self.b(
                "aws --version",
                "aws --version 2>&1 || true",
                "aws --version",
            )?;
        }
        if have("psql") {
            section("psql");
            self.b(
                "psql --version",
                "psql --version 2>&1 || true",
                "psql --version",
            )?;
        }
        Ok(())
    }

    // `rtk rewrite`, with and without quotes.
    fn rewrite_rows(&mut self) -> io::Result<()> {
        section("rewrite");
        for (name, args, expected) in [
            ("rewrite quoted", "rewrite 'git status'", "rtk git status"),
            ("rewrite unquoted", "rewrite git status", "rtk git status"),
            ("rewrite ls -al", "rewrite ls -al", "rtk ls -al"),
            ("rewrite npm exec", "rewrite npm exec", "rtk npm exec"),
            ("rewrite cargo test", "rewrite cargo test", "rtk cargo test"),
            (
                "rewrite compound",
                "rewrite 'cargo test && git push'",
                "rtk cargo test && rtk git push",
            ),
        ] {
            let cmd = self.r(args);
            let result = text(&capture(&cmd, "2>&1"));
            self.tests += 1;
            let (name, cmd) = (pad(name, 24), pad(&cmd, 40));
            if result == expected {
                println!("✅ {name} │ {cmd} │ {result}");
                self.good += 1;
            } else {
                println!("❌ {name} │ {cmd} │ got: {result} (expected: {expected})");
                self.fail += 1;
            }
        }
        Ok(())
    }

    fn bench(&mut self, name: &str, unix_cmd: &str, rtk_cmd: &str) -> io::Result<()> {
        // rtk forwards the stderr it does not filter, so both streams are measured.
        let unix_out = capture(unix_cmd, "2>&1");
        let rtk_out = capture(rtk_cmd, "2>&1");
        let unix_tokens = count_tokens(&unix_out);
        let rtk_tokens = count_tokens(&rtk_out);
        self.tests += 1;

        let tag = if rtk_out.is_empty() && !unix_out.is_empty() {
            self.fail += 1;
            self.total_unix += unix_tokens;
            self.total_rtk += unix_tokens;
            Tag::Fail
        } else if unix_tokens > 0 {
            self.total_unix += unix_tokens;
            self.total_rtk += rtk_tokens;
            if rtk_tokens > unix_tokens {
                self.negative += 1;
                Tag::Neg
            } else if rtk_tokens == unix_tokens
                || (unix_tokens - rtk_tokens) * 100 / unix_tokens < 60
            {
                self.warn += 1;
                Tag::Warn
            } else {
                self.good += 1;
                Tag::Good
            }
        } else {
            self.warn += 1;
            Tag::Skip
        };

        let icon = match tag {
            Tag::Good => "✅",
            Tag::Fail => "❌",
            Tag::Neg => "🔴",
            Tag::Warn => "⚠️",
            Tag::Skip => "⏭️",
        };
        let row = format!(
            "{} │ {} │ {}",
            pad(name, 24),
            pad(unix_cmd, 40),
            pad(rtk_cmd, 40)
        );
        if tag == Tag::Fail {
            println!("{icon} {row} │ {unix_tokens:>6} → {:>6} (--)", "-");
        } else {
            let pct = if unix_tokens > 0 {
                (unix_tokens - rtk_tokens) * 100 / unix_tokens
            } else {
                0
            };
            println!("{icon} {row} │ {unix_tokens:>6} → {rtk_tokens:>6} ({pct:+}%)");
        }

        if !self.ci {
            self.write_outputs(
                name,
                tag,
                unix_cmd,
                rtk_cmd,
                &unix_out,
                &rtk_out,
                unix_tokens,
                rtk_tokens,
            )?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn write_outputs(
        &self,
        name: &str,
        tag: Tag,
        unix_cmd: &str,
        rtk_cmd: &str,
        unix_out: &[u8],
        rtk_out: &[u8],
        unix_tokens: i64,
        rtk_tokens: i64,
    ) -> io::Result<()> {
        let file = safe_name(name);
        let prefix = match tag {
            Tag::Good => "GOOD",
            Tag::Fail => "FAIL",
            Tag::Neg => "NEG",
            Tag::Warn => "WARN",
            Tag::Skip => "SKIP",
        };
        let ts = timestamp();
        let page = |cmd: &str, out: &[u8]| {
            let mut page =
                format!("# {name}\n> {ts}\n\n```bash\n$ {cmd}\n```\n\n```\n").into_bytes();
            page.extend_from_slice(out);
            page.extend_from_slice(b"\n```\n");
            page
        };
        write(
            self.bench_dir.join(format!("unix/{file}.md")),
            page(unix_cmd, unix_out),
        )?;
        write(
            self.bench_dir.join(format!("rtk/{file}.md")),
            page(rtk_cmd, rtk_out),
        )?;

        let mut diff = format!(
            "# Diff: {name}\n> {ts}\n\n| Metric | Unix | RTK |\n|--------|------|-----|\n| Tokens | {unix_tokens} | {rtk_tokens} |\n\n## Unix\n```\n"
        )
        .into_bytes();
        diff.extend_from_slice(unix_out);
        diff.extend_from_slice(b"\n```\n\n## RTK\n```\n");
        diff.extend_from_slice(rtk_out);
        diff.extend_from_slice(b"\n```\n");
        write(
            self.bench_dir.join(format!("diff/{prefix}-{file}.md")),
            diff,
        )?;
        Ok(())
    }

    fn find_max(&mut self, max: usize) -> io::Result<()> {
        let out = text(&capture(
            &self.r(&format!("find '*' --max {max}")),
            "2>/dev/null",
        ));
        let (displayed, total) = count_find(&out);
        let expected = max.min(total);
        self.tests += 1;
        let label = pad(&format!("find --max {max} cap"), 24);
        let cmd = pad(&format!("rtk find '*' --max {max}"), 40);
        if displayed == expected && expected > 0 {
            println!("✅ {label} │ {cmd} │ {displayed}/{total} names displayed");
            self.good += 1;
        } else {
            println!(
                "❌ {label} │ {cmd} │ {displayed} names displayed (expected {expected} of {total})"
            );
            self.fail += 1;
        }
        Ok(())
    }

    fn summary(&self) -> ExitCode {
        println!();
        println!("{}", DOUBLE_RULE.repeat(RULE_WIDTH));
        if self.tests == 0 {
            return ExitCode::SUCCESS;
        }
        let good_pct = self.good * 100 / self.tests;
        let save_pct = if self.total_unix > 0 {
            (self.total_unix - self.total_rtk) * 100 / self.total_unix
        } else {
            0
        };
        println!();
        println!(
            "  ✅ {} good  ⚠️ {} warn  🔴 {} negative  ❌ {} fail    {}/{} ({good_pct}%)",
            self.good, self.warn, self.negative, self.fail, self.good, self.tests
        );
        println!(
            "  Tokens: {} → {}  (-{save_pct}%)",
            self.total_unix, self.total_rtk
        );
        println!();
        if !self.ci {
            println!("  Debug: {}/{{unix,rtk,diff}}/", self.bench_dir.display());
        }
        println!();
        let mut failed = false;
        if self.negative > 0 {
            println!(
                "  BENCHMARK FAILED: {} filter(s) produced more tokens than raw output",
                self.negative
            );
            failed = true;
        }
        if self.fail > 0 {
            println!(
                "  BENCHMARK FAILED: {} filter(s) returned empty output",
                self.fail
            );
            failed = true;
        }
        if good_pct < 60 && !failed {
            println!("  WARNING: {good_pct}% good (target 60%)");
        }
        if failed {
            ExitCode::from(1)
        } else {
            ExitCode::SUCCESS
        }
    }
}

/// The loopback server and fixtures for the curl and wget rows; removed at exit.
#[derive(Default)]
struct NetFixtures {
    dir: PathBuf,
    log: PathBuf,
    server: Option<Child>,
    url: Option<String>,
}

impl NetFixtures {
    fn setup(&mut self) -> io::Result<()> {
        self.dir = mktemp(true)?;
        // Outside the served folder, so it is never served itself.
        self.log = mktemp(false)?;
        write(self.dir.join("data.json"), NET_DATA_JSON)?;
        write(self.dir.join("robots.txt"), NET_ROBOTS)?;
        if !have("python3") {
            return Ok(());
        }
        // Port 0 asks the kernel for a free port. The "Serving HTTP on ... port N"
        // line is printed once the socket listens; -u gets it to the log at once.
        let log = fs::File::create(&self.log)?;
        let err = log.try_clone()?;
        self.server = Command::new("python3")
            .args(["-u", "-m", "http.server", "0", "--bind", "127.0.0.1"])
            .current_dir(&self.dir)
            .stdout(log)
            .stderr(err)
            .spawn()
            .ok();
        for _ in 0..25 {
            let log = fs::read_to_string(&self.log).unwrap_or_default();
            if let Some(port) = log.lines().find_map(last_port) {
                self.url = Some(format!("http://127.0.0.1:{port}"));
                break;
            }
            sleep(Duration::from_millis(200));
        }
        Ok(())
    }

    fn cleanup(&mut self) {
        if let Some(server) = &mut self.server {
            let _ = server.kill();
            let _ = server.wait();
        }
        if !self.dir.as_os_str().is_empty() {
            let _ = fs::remove_dir_all(&self.dir);
        }
        if !self.log.as_os_str().is_empty() {
            let _ = fs::remove_file(&self.log);
        }
        // `rtk wget <url>/data.json` saves ./data.json; remove it and numbered copies.
        if let Ok(entries) = fs::read_dir(".") {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name == "data.json" || name.starts_with("data.json.") {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
    }
}

/// The port in the last "port N" of a line, as `sed 's/.*port \([0-9][0-9]*\).*/\1/p'`.
fn last_port(line: &str) -> Option<&str> {
    let indices: Vec<usize> = line.match_indices("port ").map(|(i, _)| i).collect();
    indices.into_iter().rev().find_map(|i| {
        let rest = &line[i + 5..];
        let end = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        (end > 0).then(|| &rest[..end])
    })
}

/// Runs `cmd` through `eval` in bash with `redirect` applied, and returns stdout as
/// command substitution would: trailing newlines and NUL bytes removed.
fn capture(cmd: &str, redirect: &str) -> Vec<u8> {
    let out = Command::new("bash")
        .args(["-c", &format!("eval \"$1\" {redirect}"), "bash", cmd])
        .env("CARGO_TERM_COLOR", CARGO_COLOUR)
        .stdin(Stdio::inherit())
        .stderr(Stdio::inherit())
        .output()
        .map(|o| o.stdout)
        .unwrap_or_default();
    if out.contains(&0) {
        eprintln!("rtk-dev benchmark: warning: command substitution: ignored null byte in input");
    }
    let mut out: Vec<u8> = out.into_iter().filter(|&b| b != 0).collect();
    while out.last() == Some(&b'\n') {
        out.pop();
    }
    out
}

fn text(out: &[u8]) -> String {
    String::from_utf8_lossy(out).into_owned()
}

/// Characters, as bash's `${#var}` counts them in a UTF-8 locale (an invalid byte
/// counts as one), rounded up to tokens of 4.
fn count_tokens(out: &[u8]) -> i64 {
    let chars: usize = out
        .utf8_chunks()
        .map(|c| c.valid().chars().count() + c.invalid().len())
        .sum();
    (chars as i64 + 3) / 4
}

/// Names displayed by `rtk find`, and the tree size it reports: the "NNNF ND:"
/// header when it groups, otherwise the names shown plus any "+N more".
fn count_find(out: &str) -> (usize, usize) {
    let (mut shown, mut total, mut more) = (0, 0, 0);
    for line in out.split('\n') {
        let fields: Vec<&str> = line.split([' ', '\t']).filter(|f| !f.is_empty()).collect();
        let Some(first) = fields.first() else {
            continue;
        };
        if let Some(n) = find_header(line) {
            total = n;
            continue;
        }
        if let Some(n) = more_marker(line) {
            more = n;
            continue;
        }
        if ["ext: ", "... (", "[see remaining: ", "[full output: "]
            .iter()
            .any(|p| line.starts_with(p))
            || hidden_marker(line)
        {
            continue;
        }
        shown += fields.len() - usize::from(first.ends_with('/'));
    }
    (shown, if total > 0 { total } else { shown + more })
}

fn digits(s: &str) -> Option<usize> {
    (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        .then(|| s.parse().unwrap_or(usize::MAX))
}

/// `^[0-9]+F [0-9]+D:$`
fn find_header(line: &str) -> Option<usize> {
    let (files, dirs) = line.split_once("F ")?;
    let n = digits(files)?;
    digits(dirs.strip_suffix("D:")?)?;
    Some(n)
}

/// `^\+[0-9]+ more$`
fn more_marker(line: &str) -> Option<usize> {
    digits(line.strip_prefix('+')?.strip_suffix(" more")?)
}

/// `^\[\+[0-9]+ hidden[ :(]`: the recall hint in each of its forms,
/// `[+N hidden: ...]`, `[+N hidden (storage limit): ...]` and
/// `[+N hidden (lines A-B): ...]`. The shell script matched the first only.
fn hidden_marker(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("[+") else {
        return false;
    };
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    end > 0
        && rest[end..]
            .strip_prefix(" hidden")
            .and_then(|tail| tail.chars().next())
            .is_some_and(|c| matches!(c, ' ' | ':' | '('))
}

/// Left-aligned to `width` bytes, as bash's printf pads `%-Ns`.
fn pad(s: &str, width: usize) -> String {
    format!("{s}{}", " ".repeat(width.saturating_sub(s.len())))
}

fn section(name: &str) {
    println!("\n── {name} ──");
}

fn skipped(line: &str) {
    println!("{line}");
}

fn shell_ok(script: &str) -> bool {
    Command::new("bash")
        .args(["-c", script])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn have(tool: &str) -> bool {
    shell_ok(&format!("command -v {tool}"))
}

fn is_file(path: &str) -> bool {
    Path::new(path).is_file()
}

/// Builds before a row, so a first build's compile output does not land on the
/// raw side alone and read as savings.
fn precompile(args: &[&str]) {
    let ok = Command::new(args[0])
        .args(&args[1..])
        .env("CARGO_TERM_COLOR", CARGO_COLOUR)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        eprintln!("⚠️  precompile failed: {}", args.join(" "));
    }
}

fn timestamp() -> String {
    Command::new("date")
        .arg("+%d/%m/%Y %H:%M:%S")
        .output()
        .map(|o| text(&o.stdout).trim_end_matches('\n').to_string())
        .unwrap_or_default()
}

/// `tr ' /' '_-' | tr -cd 'a-zA-Z0-9_-'`
fn safe_name(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            ' ' => '_',
            '/' => '-',
            c => c,
        })
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect()
}

/// A new temporary folder or file from `mktemp`, as the script made them.
fn mktemp(dir: bool) -> io::Result<PathBuf> {
    let mut cmd = Command::new("mktemp");
    if dir {
        cmd.arg("-d");
    }
    let out = cmd.stderr(Stdio::inherit()).output()?;
    if !out.status.success() {
        return Err(io::Error::other("mktemp failed"));
    }
    Ok(PathBuf::from(text(&out.stdout).trim_end_matches('\n')))
}

/// `fs::write`, naming the path in the error.
fn write(path: impl AsRef<Path>, bytes: impl AsRef<[u8]>) -> io::Result<()> {
    let path = path.as_ref();
    fs::write(path, bytes).map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))
}

/// `cd`, naming the path in the error.
fn cd(path: impl AsRef<Path>) -> io::Result<()> {
    let path = path.as_ref();
    std::env::set_current_dir(path)
        .map_err(|e| io::Error::new(e.kind(), format!("cd {}: {e}", path.display())))
}

/// The working directory as the shell's `pwd` gives it: `$PWD` when it names it.
fn logical_cwd() -> PathBuf {
    let physical = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    match std::env::var_os("PWD").map(PathBuf::from) {
        Some(pwd) if pwd.is_absolute() && same_file(&pwd, &physical) => pwd,
        _ => physical,
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    fs::canonicalize(a)
        .ok()
        .zip(fs::canonicalize(b).ok())
        .is_some_and(|(a, b)| a == b)
}

fn find_rtk(root: &Path) -> Option<String> {
    let rtk = match std::env::var_os("RTK_BIN") {
        Some(bin) if !bin.is_empty() => {
            let bin = bin.to_string_lossy().into_owned();
            if !is_executable_file(Path::new(&bin)) {
                println!("Error: RTK_BIN={bin} is not a path to an executable file.");
                return None;
            }
            bin
        }
        _ if is_file("./target/release/rtk") => "./target/release/rtk".to_string(),
        _ => {
            let found = Command::new("bash")
                .args(["-c", "command -v rtk"])
                .stderr(Stdio::null())
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| text(&o.stdout).trim_end_matches('\n').to_string());
            match found {
                Some(path) => path,
                None => {
                    println!(
                        "Error: rtk not found. Run 'cargo build --release', install rtk, or set RTK_BIN."
                    );
                    return None;
                }
            }
        }
    };
    // Absolute, since some rows change directory.
    if !rtk.contains('/') {
        return Some(rtk);
    }
    let path = Path::new(&rtk);
    let dir = path.parent().unwrap_or(Path::new(""));
    let base = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    Some(format!("{}/{base}", normalise(&root.join(dir)).display()))
}

/// Lexical `.` and `..` removal, as `cd` without `-P` resolves them.
fn normalise(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_counts_follow_the_awk_rules() {
        let out =
            "356F 63D:\n./ a b\nsrc/ c\n+256 more\next: .rs 3\n... (8 filtered)\n[+2 hidden: x]\n";
        assert_eq!(count_find(out), (3, 356));
        assert_eq!(count_find("a\nb c\n+5 more\n"), (3, 8));
    }

    #[test]
    fn find_skips_every_form_of_the_recall_hint() {
        for hint in [
            "[+2 hidden: rtk recall ab12]",
            "[+2 hidden (storage limit): rtk recall ab12]",
            "[+2 hidden (lines 3-4): rtk recall ab12]",
        ] {
            assert_eq!(count_find(&format!("a b\n{hint}\n")), (2, 2), "{hint}");
        }
        // Not a hint: counted as names.
        assert_eq!(count_find("a b\n[+2 hiddenx]\n"), (4, 4));
    }

    #[test]
    fn tokens_count_characters_not_bytes() {
        assert_eq!(count_tokens("éééé".as_bytes()), 1);
        assert_eq!(count_tokens(b"abcde"), 2);
        assert_eq!(count_tokens(b""), 0);
    }

    #[test]
    fn pad_counts_bytes_like_bash_printf() {
        assert_eq!(pad("é", 5), "é   ");
        assert_eq!(pad("abcdef", 3), "abcdef");
    }

    #[test]
    fn safe_name_matches_tr() {
        assert_eq!(safe_name("ls src/ -l"), "ls_src-_-l");
        assert_eq!(safe_name("find *.rs"), "find_rs");
    }

    #[test]
    fn last_port_takes_the_last_match() {
        assert_eq!(
            last_port("Serving HTTP on 127.0.0.1 port 40404 (http://127.0.0.1:40404/) ..."),
            Some("40404")
        );
        assert_eq!(last_port("port x"), None);
    }
}
