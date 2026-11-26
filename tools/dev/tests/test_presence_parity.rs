//! `rtk-dev check-test-presence` against cases recorded from the shell script it
//! replaced (scripts/check-test-presence.sh at fd8262b0). Each case builds a
//! small Git repository in a fixed folder under /tmp and compares stdout, the
//! exit code and the repository's files afterwards.
//!
//! To record the cases again from a shell script:
//! `RTK_DEV_RECORD_TEST_PRESENCE=/abs/path/check-test-presence.sh cargo test --test test_presence_parity`.
#![cfg(target_os = "linux")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const WITH_TESTS: &str = "pub fn run() {}\n\n#[cfg(test)]\nmod tests {}\n";
const WITHOUT_TESTS: &str = "pub fn run() {}\n";

struct Case {
    name: &'static str,
    args: &'static [&'static str],
    /// Files committed on `base`, then the files changed on HEAD (None deletes).
    base: &'static [(&'static str, &'static str)],
    head: &'static [(&'static str, Option<&'static str>)],
    /// Files removed from the work tree after the HEAD commit.
    removed: &'static [&'static str],
    /// The error text differs by design (bash names the script and line).
    stderr_differs: bool,
}

const BASE_FILES: &[(&str, &str)] = &[
    ("src/cmds/system/keep.rs", "\n"),
    ("src/cmds/git/old_cmd.rs", WITHOUT_TESTS),
    ("src/cmds/git/gone_cmd.rs", WITHOUT_TESTS),
    ("src/core/runner.rs", WITHOUT_TESTS),
];

const CASES: &[Case] = &[
    Case {
        name: "mixed",
        args: &["base"],
        base: BASE_FILES,
        head: &[
            ("src/cmds/git/new_cmd.rs", Some(WITH_TESTS)),
            (
                "src/cmds/git/old_cmd.rs",
                Some("pub fn run() { /* changed */ }\n"),
            ),
            ("src/cmds/git/gone_cmd.rs", None),
            ("src/core/runner.rs", Some("// changed\n")),
            ("crates/x/src/cmds/deep_cmd.rs", Some(WITHOUT_TESTS)),
        ],
        removed: &[],
        stderr_differs: false,
    },
    Case {
        name: "all-pass-default-base",
        args: &[],
        base: BASE_FILES,
        head: &[
            ("src/cmds/git/new_cmd.rs", Some(WITH_TESTS)),
            ("src/cmds/git/old_cmd.rs", Some(WITH_TESTS)),
        ],
        removed: &[],
        stderr_differs: false,
    },
    Case {
        name: "no-cmd-changes",
        args: &["base"],
        base: BASE_FILES,
        head: &[("src/core/runner.rs", Some("// changed\n"))],
        removed: &[],
        stderr_differs: false,
    },
    Case {
        // `IFS= read -r` keeps a name's leading blank.
        name: "leading-blank",
        args: &["base"],
        base: BASE_FILES,
        head: &[(" lead/src/cmds/a_cmd.rs", Some(WITHOUT_TESTS))],
        removed: &[],
        stderr_differs: false,
    },
    Case {
        // The script skipped a listed name that is no longer a file.
        name: "removed-after-commit",
        args: &["base"],
        base: BASE_FILES,
        head: &[
            ("src/cmds/git/new_cmd.rs", Some(WITHOUT_TESTS)),
            ("src/cmds/git/old_cmd.rs", Some(WITHOUT_TESTS)),
        ],
        removed: &["src/cmds/git/new_cmd.rs"],
        stderr_differs: false,
    },
    Case {
        name: "unknown-base",
        args: &["no-such-ref"],
        base: BASE_FILES,
        head: &[("src/cmds/git/new_cmd.rs", Some(WITHOUT_TESTS))],
        removed: &[],
        stderr_differs: false,
    },
    Case {
        name: "self-test",
        args: &["--self-test"],
        base: BASE_FILES,
        head: &[],
        removed: &[],
        stderr_differs: false,
    },
    Case {
        name: "self-test-no-folder",
        args: &["--self-test"],
        base: &[("README.md", "x\n")],
        head: &[],
        removed: &[],
        stderr_differs: true,
    },
];

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
        .output()
        .expect("git")
        .status
        .success();
    assert!(ok, "git {args:?}");
}

fn put(root: &Path, path: &str, body: Option<&str>) {
    let file = root.join(path);
    match body {
        Some(body) => {
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, body).unwrap();
        }
        None => fs::remove_file(file).unwrap(),
    }
}

/// A repository whose `base` branch (also `origin/develop`) holds `case.base`
/// and whose HEAD adds `case.head` on top.
fn fixture(case: &Case) -> PathBuf {
    let root = PathBuf::from("/tmp/rtk-dev-parity/test-presence").join(case.name);
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q", "-b", "base"]);
    for (path, body) in case.base {
        put(&root, path, Some(body));
    }
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-q", "-m", "base"]);
    git(
        &root,
        &["update-ref", "refs/remotes/origin/develop", "HEAD"],
    );
    git(&root, &["checkout", "-q", "-b", "topic"]);
    if !case.head.is_empty() {
        for (path, body) in case.head {
            put(&root, path, *body);
        }
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-q", "-m", "topic"]);
        for path in case.removed {
            put(&root, path, None);
        }
    }
    root
}

/// Files in the work tree, outside `.git`, sorted.
fn tree(root: &Path) -> Vec<u8> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<String>) {
        for entry in fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.file_name().is_some_and(|n| n == ".git") {
                continue;
            }
            if path.is_dir() {
                walk(&path, root, out);
            } else {
                out.push(path.strip_prefix(root).unwrap().display().to_string());
            }
        }
    }
    let mut files = Vec::new();
    walk(root, root, &mut files);
    files.sort();
    files
        .iter()
        .map(|f| format!("{f}\n"))
        .collect::<String>()
        .into_bytes()
}

fn run(case: &Case, program: &[&str]) -> [Vec<u8>; 4] {
    let root = fixture(case);
    let out = Command::new(program[0])
        .args(&program[1..])
        .args(case.args)
        .current_dir(&root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", &root)
        .output()
        .expect("run the check");
    let code = format!("{}\n", out.status.code().unwrap_or(-1)).into_bytes();
    [out.stdout, out.stderr, code, tree(&root)]
}

const PARTS: [&str; 4] = ["stdout", "stderr", "code", "files"];

#[test]
fn port_matches_the_recorded_script() {
    let cases_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cases/test-presence");
    if let Ok(script) = std::env::var("RTK_DEV_RECORD_TEST_PRESENCE") {
        for case in CASES {
            let parts = run(case, &["bash", &script]);
            let dir = cases_dir.join(case.name);
            fs::create_dir_all(&dir).unwrap();
            for (part, bytes) in PARTS.iter().zip(parts) {
                fs::write(dir.join(part), bytes).unwrap();
            }
        }
    }
    let mut failures = Vec::new();
    for case in CASES {
        let parts = run(
            case,
            &[env!("CARGO_BIN_EXE_rtk-dev"), "check-test-presence"],
        );
        for (part, got) in PARTS.iter().zip(parts) {
            let want = fs::read(cases_dir.join(case.name).join(part)).unwrap();
            if case.stderr_differs && *part == "stderr" && !got.is_empty() && !want.is_empty() {
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
