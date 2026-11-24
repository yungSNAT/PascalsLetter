//! `rtk-dev smoke` against cases recorded from the shell script it replaced
//! (scripts/test-all.sh at 0009cab3). Each case runs in a fixed folder under
//! /tmp with a stub `rtk` and stub optional tools, and compares stdout, stderr,
//! the exit code and every file left behind. The `Date:` line is masked: the
//! script printed local time, the port prints UTC.
//!
//! To record the cases again from a shell script:
//! `RTK_DEV_RECORD_SMOKE=/abs/path/test-all.sh cargo test --test smoke_parity`.
#![cfg(target_os = "linux")]

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Real tools the script needs; the port needs none of them.
const REAL_TOOLS: &[&str] = &["bash", "cat", "grep", "head", "rm", "seq"];

/// `good`: every check passes. `bad`: wrong exit codes, missing needles,
/// output on both streams, near-misses for the `^` and `.` needles.
const RTK_STUB: &str = r#"#!/bin/sh
if [ "$1" = --version ]; then echo "rtk 1.2.0-stub"; exit 0; fi
if [ "$RTK_STUB_MODE" = good ]; then
  case "$*" in
    "rewrite RTK_DISABLED=1 git status"|"rewrite FOO=1 RTK_DISABLED=1 cargo test"|\
    "rewrite gh pr list --json number"|"rewrite gh api /repos --jq .name"|\
    "rewrite gh pr view 1 --template '{{.title}}'"|"grep -t rust fn main src/main.rs"|\
    "json Cargo.toml") exit 1 ;;
    "diff Cargo.toml LICENSE") printf '1c1\n< a\n---\n> b\n'; exit 1 ;;
    "read -") cat; exit 0 ;;
  esac
  echo "rtk Usage: .git 1.2K src/ string pub fn x.rs Cargo version rtk git status"
  echo "rtk cargo test | head 2>&1 rtk gh pr list hello test result: ok"
  exit 0
fi
case "$*" in
  "ls -a .") echo "xgit"; exit 0 ;;
  "ls .") echo "out 1"; echo "err 1" >&2; echo "out 2"; exit 1 ;;
  "diff Cargo.toml LICENSE") echo "x < y"; exit 1 ;;
  "cargo test") echo "nothing ran"; exit 101 ;;
  "read -") cat; exit 4 ;;
  json\ /*|log\ /*) echo "string"; exit 0 ;;
  "rewrite git status") echo "git status"; exit 0 ;;
  *--help) printf 'no usage here\n\n\n'; exit 0 ;;
  rewrite*|"grep -t rust fn main src/main.rs") exit 0 ;;
  "env") exit 0 ;;
esac
printf 'line 1\n\nline 3\nline 4\n\n\n'
exit 3
"#;

const GIT_STUB: &str = r#"#!/bin/sh
[ -n "$GIT_STUB_FAIL" ] && exit 128
case "$*" in
  "rev-parse --is-inside-work-tree") echo true ;;
  "rev-parse --show-toplevel") pwd ;;
  *) exit 1 ;;
esac
"#;

const DATE_STUB: &str = "#!/bin/sh\necho '2026-11-24 10:00'\n";
const OK_STUB: &str = "#!/bin/sh\nexit 0\n";
const FAIL_STUB: &str = "#!/bin/sh\nexit 1\n";

/// Every optional tool the suite looks for, present and working.
const ALL_TOOLS: &[(&str, &str)] = &[
    ("tree", OK_STUB),
    ("gh", OK_STUB),
    ("pnpm", OK_STUB),
    ("rg", OK_STUB),
    ("wget", OK_STUB),
    ("pytest", OK_STUB),
    ("ruff", OK_STUB),
    ("pip", OK_STUB),
    ("go", OK_STUB),
    ("golangci-lint", OK_STUB),
    ("gt", OK_STUB),
    ("rspec", OK_STUB),
    ("rubocop", OK_STUB),
    ("rake", OK_STUB),
    ("docker", OK_STUB),
];

struct Case {
    name: &'static str,
    mode: &'static str,
    rtk_on_path: bool,
    in_repo: bool,
    /// Optional tools on PATH, with the stub each gets.
    tools: &'static [(&'static str, &'static str)],
}

const CASES: &[Case] = &[
    Case {
        name: "no-rtk",
        mode: "good",
        rtk_on_path: false,
        in_repo: true,
        tools: &[],
    },
    Case {
        name: "not-a-repo",
        mode: "good",
        rtk_on_path: true,
        in_repo: false,
        tools: &[],
    },
    Case {
        name: "all-pass",
        mode: "good",
        rtk_on_path: true,
        in_repo: true,
        tools: ALL_TOOLS,
    },
    Case {
        name: "failures",
        mode: "bad",
        rtk_on_path: true,
        in_repo: true,
        // gh not signed in, docker not running.
        tools: &[("gh", FAIL_STUB), ("docker", FAIL_STUB), ("rg", OK_STUB)],
    },
];

fn write_exe(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn real_tool(name: &str) -> PathBuf {
    let path = std::env::var_os("PATH").expect("PATH");
    std::env::split_paths(&path)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
        .unwrap_or_else(|| panic!("{name} not on PATH"))
}

/// The case folder: `root` (the repository stand-in), `bin` (the stubs and the
/// real tools the script needs), `rtkbin` (the rtk stub), `tmp`.
fn fixture(case: &Case) -> PathBuf {
    let base = PathBuf::from("/tmp/rtk-dev-parity/smoke").join(case.name);
    let _ = fs::remove_dir_all(&base);
    let root = base.join("root");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("Cargo.toml"), "[package]\nname = \"rtk\"\n").unwrap();
    fs::write(root.join("LICENSE"), "Apache-2.0\n").unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();

    let bin = base.join("bin");
    fs::create_dir_all(&bin).unwrap();
    for tool in REAL_TOOLS {
        symlink(real_tool(tool), bin.join(tool)).unwrap();
    }
    write_exe(&bin.join("git"), GIT_STUB);
    write_exe(&bin.join("date"), DATE_STUB);
    for (tool, body) in case.tools {
        write_exe(&bin.join(tool), body);
    }
    // Numbered files under `tmp`, so the script's temporary paths are fixed;
    // `tmp` is in the snapshot, so a file left behind shows.
    fs::create_dir_all(base.join("tmp")).unwrap();
    write_exe(
        &bin.join("mktemp"),
        &format!(
            "#!/bin/sh
state={b}/mktemp.n
n=$(cat \"$state\" 2>/dev/null || echo 0)
n=$((n + 1))
echo $n > \"$state\"
p={b}/tmp/f$n; : > \"$p\"
echo \"$p\"
",
            b = base.display()
        ),
    );
    write_exe(&base.join("rtkbin/rtk"), RTK_STUB);
    base
}

/// Every file under `dir`, sorted, as `== path` followed by its bytes.
fn snapshot(dir: &Path) -> Vec<u8> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    walk(dir, &mut files);
    files.sort();
    let mut snap = Vec::new();
    for file in files {
        snap.extend_from_slice(
            format!("== {}\n", file.strip_prefix(dir).unwrap().display()).as_bytes(),
        );
        snap.extend_from_slice(&fs::read(&file).unwrap());
        snap.push(b'\n');
    }
    snap
}

/// stdout with the `Date:` line's value masked.
fn mask_date(stdout: &[u8]) -> Vec<u8> {
    let mut masked = Vec::new();
    for line in stdout.split_inclusive(|&b| b == b'\n') {
        if line.starts_with(b"Date: ") {
            masked.extend_from_slice(b"Date: -\n");
        } else {
            masked.extend_from_slice(line);
        }
    }
    masked
}

/// Runs the case with `program` (the script through bash, or the port) and
/// returns stdout, stderr, the exit code and the folder snapshot.
fn run(case: &Case, program: &[&str]) -> [Vec<u8>; 4] {
    let base = fixture(case);
    let root = base.join("root");
    let mut path = base.join("bin").display().to_string();
    if case.rtk_on_path {
        path = format!("{path}:{}", base.join("rtkbin").display());
    }
    let mut cmd = Command::new(program[0]);
    cmd.args(&program[1..])
        .current_dir(&root)
        .env_clear()
        .env("PATH", path)
        .env("PWD", &root)
        .env("LANG", "C.UTF-8")
        .env("TMPDIR", base.join("tmp"))
        .env("RTK_STUB_MODE", case.mode);
    if !case.in_repo {
        cmd.env("GIT_STUB_FAIL", "1");
    }
    let out = cmd.output().expect("run the smoke suite");
    let code = format!("{}\n", out.status.code().unwrap_or(-1)).into_bytes();
    let mut files = snapshot(&root);
    files.extend_from_slice(b"== [tmp]\n");
    files.extend(snapshot(&base.join("tmp")));
    [mask_date(&out.stdout), out.stderr, code, files]
}

const PARTS: [&str; 4] = ["stdout", "stderr", "code", "files"];

#[test]
fn port_matches_the_recorded_script() {
    let cases_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cases/smoke");
    if let Ok(script) = std::env::var("RTK_DEV_RECORD_SMOKE") {
        let bash = real_tool("bash").display().to_string();
        for case in CASES {
            let parts = run(case, &[&bash, &script]);
            let dir = cases_dir.join(case.name);
            fs::create_dir_all(&dir).unwrap();
            for (part, bytes) in PARTS.iter().zip(parts) {
                fs::write(dir.join(part), bytes).unwrap();
            }
        }
    }
    let mut failures = Vec::new();
    for case in CASES {
        let parts = run(case, &[env!("CARGO_BIN_EXE_rtk-dev"), "smoke"]);
        for (part, got) in PARTS.iter().zip(parts) {
            let want = fs::read(cases_dir.join(case.name).join(part)).unwrap();
            if got != want {
                failures.push(format!(
                    "{} {part}:\n--- recorded\n{}\n--- port\n{}",
                    case.name,
                    String::from_utf8_lossy(&want),
                    String::from_utf8_lossy(&got)
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
