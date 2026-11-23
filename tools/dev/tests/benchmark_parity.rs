//! `rtk-dev benchmark` against cases recorded from the shell script it replaced
//! (scripts/benchmark.sh at 4a0da8b7). Each case runs in a fixed folder under
//! /tmp with stub tools, so the output is the same on every Linux machine; it
//! compares stdout, stderr, the exit code and every file left in the folder.
//!
//! To record a case again from a shell script:
//! `RTK_DEV_PARITY_RECORD=/abs/path/benchmark.sh cargo test --test benchmark_parity`.
#![cfg(target_os = "linux")]

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;

const REAL_TOOLS: &[&str] = &[
    "bash", "cat", "env", "wc", "tr", "rm", "mkdir", "dirname", "basename", "awk", "sed", "seq",
    "head", "sleep",
];

const RTK_STUB: &str = r#"#!/bin/sh
sub=$1; shift
case "$sub" in
  rewrite)
    if [ "$RTK_STUB_MODE" = good ]; then
      printf 'rtk %s\n' "$*" | sed 's/&& /\&\& rtk /'
    else
      printf 'rtk %s\n' "$*"
    fi ;;
  find) printf '4F 2D:\n./ Cargo.toml\nsrc/ lib.rs main.rs\nscripts/ keep\next: .rs 2\n[+3 hidden: rtk recall ab12]\n' ;;
  *)
    if [ "$RTK_STUB_MODE" = good ]; then echo ok; exit 0; fi
    case "$sub" in
      read)
        case "$*" in
          *minimal*) cat src/lib.rs ;;
          *aggressive*) head -c 60 src/lib.rs ;;
          *-n*) cat src/lib.rs src/lib.rs src/lib.rs ;;
          *) echo short ;;
        esac ;;
      json) case "$*" in *-d*) ;; *) echo '{"name":"rtk"}' ;; esac ;;
      deps) cat Cargo.toml Cargo.toml ;;
      git) echo "git: é ok" >&2 ;;
      *) echo ok ;;
    esac ;;
esac
"#;

const LS_STUB: &str = r#"#!/bin/sh
echo "total 3"
echo "drwxr-xr-x 2 user group 4096 Jan  1 00:00 src"
echo "-rw-r--r-- 1 user group  120 Jan  1 00:00 Cargo.toml"
echo "args: $*"
"#;

const FIND_STUB: &str = r#"#!/bin/sh
printf './Cargo.toml\n./src/lib.rs\n./src/main.rs\n./scripts/keep\n'
printf './docs/guide/getting-started/installation.md\n./docs/guide/resources/troubleshooting.md\n'
"#;

const GIT_STUB: &str = r#"#!/bin/sh
case "$1" in
  status) printf 'On branch develop\nnothing to commit, working tree clean\n' ;;
  log) printf 'commit 0123456789abcdef\nAuthor: A <a@example.com>\n\n    First commit\n' ;;
  show) printf 'commit 0123456789abcdef\n src/lib.rs | 2 +-\n 1 file changed\n' ;;
  *) exit 1 ;;
esac
"#;

const DATE_STUB: &str = "#!/bin/sh\necho '24/11/2026 10:00:00'\n";

const CARGO_STUB: &str = r#"#!/bin/sh
case "$1" in
  --help) printf 'Rust package manager\n\nUsage: cargo [OPTIONS] [COMMAND]\n\nCommands:\n    build    Compile\n' ;;
  *) printf '   Compiling rtk v1.2.0 (/src)\n    Finished `dev` profile [unoptimized] target(s) in 0.10s\n' ;;
esac
"#;

// Stands in for `python3 -u -m http.server`: prints the ready line, then waits.
const PYTHON_STUB: &str = "#!/bin/sh\necho 'Serving HTTP on 127.0.0.1 port 40404 (http://127.0.0.1:40404/) ...'\nexec sleep 30\n";

const CURL_STUB: &str = r#"#!/bin/sh
case "$*" in
  *robots*) printf 'User-agent: *\nDisallow: /private/\n' ;;
  *) printf '{\n    "message": "Hello",\n    "status": "success"\n}\n' ;;
esac
"#;

const TREE_STUB: &str = "#!/bin/sh\nprintf '.\\n├── Cargo.toml\\n└── src\\n    └── lib.rs\\n'\n";

const RG_STUB: &str = "#!/bin/sh\nprintf 'src/lib.rs:1:pub fn run() {}\\n'\n";

// Stands in for the Python and Go tools: shows where it ran and what it saw.
const WHERE_STUB: &str = "#!/bin/sh
echo \"$(basename \"$0\") $*\"
pwd
echo *
";

/// Optional tools a case puts on PATH, by name.
const STUBS: &[(&str, &str)] = &[
    ("cargo", CARGO_STUB),
    ("python3", PYTHON_STUB),
    ("curl", CURL_STUB),
    ("wget", CURL_STUB),
    ("tree", TREE_STUB),
    ("rg", RG_STUB),
    ("ruff", WHERE_STUB),
    ("pytest", WHERE_STUB),
    ("pip", WHERE_STUB),
    ("mypy", WHERE_STUB),
    ("go", WHERE_STUB),
    ("golangci-lint", WHERE_STUB),
];

struct Case {
    name: &'static str,
    ci: bool,
    rtk_bin: Option<&'static str>,
    stub_mode: &'static str,
    tools: &'static [&'static str],
    rtk_in_target: bool,
    rtk_on_path: bool,
    /// `scripts/benchmark` is a file, so the output folders cannot be made.
    block_output: bool,
}

const CASES: &[Case] = &[
    Case {
        name: "mixed-ci",
        ci: true,
        rtk_bin: Some("rtkbin-é/rtk"),
        stub_mode: "mixed",
        tools: &[],
        rtk_in_target: false,
        rtk_on_path: false,
        block_output: false,
    },
    Case {
        name: "full-local",
        ci: false,
        rtk_bin: Some("rtkbin/rtk"),
        stub_mode: "mixed",
        tools: &["cargo", "python3", "curl", "wget", "tree", "rg"],
        rtk_in_target: false,
        rtk_on_path: false,
        block_output: false,
    },
    Case {
        name: "good-target-release",
        ci: true,
        rtk_bin: None,
        stub_mode: "good",
        tools: &[],
        rtk_in_target: true,
        rtk_on_path: false,
        block_output: false,
    },
    Case {
        name: "good-on-path",
        ci: true,
        rtk_bin: None,
        stub_mode: "good",
        tools: &[],
        rtk_in_target: false,
        rtk_on_path: true,
        block_output: false,
    },
    Case {
        name: "bad-rtk-bin",
        ci: true,
        rtk_bin: Some("rtkbin/missing"),
        stub_mode: "mixed",
        tools: &[],
        rtk_in_target: false,
        rtk_on_path: false,
        block_output: false,
    },
    Case {
        name: "no-rtk",
        ci: true,
        rtk_bin: None,
        stub_mode: "mixed",
        tools: &[],
        rtk_in_target: false,
        rtk_on_path: false,
        block_output: false,
    },
    Case {
        name: "python-go",
        ci: true,
        rtk_bin: Some("rtkbin/rtk"),
        stub_mode: "mixed",
        tools: &[
            "python3",
            "ruff",
            "pytest",
            "pip",
            "mypy",
            "go",
            "golangci-lint",
        ],
        rtk_in_target: false,
        rtk_on_path: false,
        block_output: false,
    },
    Case {
        name: "curl-file-fallback",
        ci: true,
        rtk_bin: Some("rtkbin/rtk"),
        stub_mode: "mixed",
        tools: &["curl"],
        rtk_in_target: false,
        rtk_on_path: false,
        block_output: false,
    },
    Case {
        name: "output-blocked",
        ci: false,
        rtk_bin: Some("rtkbin/rtk"),
        stub_mode: "mixed",
        tools: &[],
        rtk_in_target: false,
        rtk_on_path: false,
        block_output: true,
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

/// Builds the case folder: `root` (the repository stand-in), `bin` (stubs and
/// the real tools the script needs), `rtkbin` (the rtk stub).
fn fixture(case: &Case) -> PathBuf {
    let base = PathBuf::from("/tmp/rtk-dev-parity").join(case.name);
    let _ = fs::remove_dir_all(&base);
    let root = base.join("root");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("scripts")).unwrap();
    fs::write(root.join("scripts/keep"), "").unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"rtk\"\nversion = \"1.2.0\"\n\n[dependencies]\nanyhow = \"1\"\n",
    )
    .unwrap();
    fs::write(
        root.join("src/lib.rs"),
        "//! Fixture.\n\npub struct Config {\n    pub depth: usize,\n}\n\npub fn run() -> usize {\n    Config { depth: 2 }.depth\n}\n",
    )
    .unwrap();
    fs::write(
        root.join("src/main.rs"),
        "fn main() {\n    rtk::run();\n}\n",
    )
    .unwrap();

    let bin = base.join("bin");
    fs::create_dir_all(&bin).unwrap();
    for tool in REAL_TOOLS {
        symlink(real_tool(tool), bin.join(tool)).unwrap();
    }
    let grep = real_tool("grep");
    write_exe(
        &bin.join("grep"),
        &format!(
            "#!/bin/sh\ncase \"$*\" in\n  -r*) printf 'src/lib.rs:7:pub fn run() -> usize {{\\nsrc/main.rs:1:fn main() {{\\n' ;;\n  *) exec {} \"$@\" ;;\nesac\n",
            grep.display()
        ),
    );
    for (name, body) in [
        ("ls", LS_STUB),
        ("find", FIND_STUB),
        ("git", GIT_STUB),
        ("date", DATE_STUB),
    ] {
        write_exe(&bin.join(name), body);
    }
    for tool in case.tools {
        let (_, body) = STUBS.iter().find(|(name, _)| name == tool).unwrap();
        write_exe(&bin.join(tool), body);
    }
    // Numbered folders and files under `tmp`, so temporary paths are the same
    // on every run; `tmp` is in the snapshot, so a leaked one shows.
    fs::create_dir_all(base.join("tmp")).unwrap();
    write_exe(
        &bin.join("mktemp"),
        &format!(
            "#!/bin/sh
state={b}/mktemp.n
n=$(cat \"$state\" 2>/dev/null || echo 0)
n=$((n + 1))
echo $n > \"$state\"
if [ \"$1\" = -d ]; then p={b}/tmp/d$n; mkdir \"$p\"; else p={b}/tmp/f$n; : > \"$p\"; fi
echo \"$p\"
",
            b = base.display()
        ),
    );
    if case.block_output {
        fs::write(root.join("scripts/benchmark"), "").unwrap();
    }
    write_exe(&base.join("rtkbin/rtk"), RTK_STUB);
    // A non-ASCII path: bash's printf pads by bytes, not characters.
    write_exe(&base.join("rtkbin-é/rtk"), RTK_STUB);
    if case.rtk_in_target {
        write_exe(&root.join("target/release/rtk"), RTK_STUB);
    }
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
        .env("RTK_STUB_MODE", case.stub_mode);
    if case.ci {
        cmd.env("CI", "true");
    }
    if let Some(bin) = case.rtk_bin {
        cmd.env("RTK_BIN", base.join(bin));
    }
    let out = cmd.output().expect("run the benchmark");
    let code = format!("{}\n", out.status.code().unwrap_or(-1)).into_bytes();
    let mut files = snapshot(&root);
    files.extend_from_slice(
        b"== [tmp]
",
    );
    files.extend(snapshot(&base.join("tmp")));
    [out.stdout, out.stderr, code, files]
}

const PARTS: [&str; 4] = ["stdout", "stderr", "code", "files"];

#[test]
fn port_matches_the_recorded_script() {
    let cases_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cases/benchmark");
    if let Ok(script) = std::env::var("RTK_DEV_PARITY_RECORD") {
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
        let parts = run(case, &[env!("CARGO_BIN_EXE_rtk-dev"), "benchmark"]);
        for (part, got) in PARTS.iter().zip(parts) {
            let want = fs::read(cases_dir.join(case.name).join(part)).unwrap();
            // The error text differs by design (bash names the script and line).
            if case.block_output && *part == "stderr" && !got.is_empty() && !want.is_empty() {
                continue;
            }
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
