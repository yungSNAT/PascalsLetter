//! Runs the binary over a one-repository corpus with a stand-in rtk, to check
//! what each side is given and what lands in results.csv and summary.md.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("output-tokens-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

// Rewrites only `cat notes.txt`, prints a shorter form for it, and records the
// home it ran in.
const FAKE_RTK: &str = r#"#!/bin/sh
case "$1" in
  --version) echo "rtk 0.0.0-test" ;;
  rewrite)
    [ "$2" = "cat notes.txt" ] || exit 1
    printf 'rtk read notes.txt'
    exit 3 ;;
  read) echo "$HOME" > "$HOME/ran"; echo "short"; exit 4 ;;
esac
"#;

#[test]
fn every_side_runs_from_the_pinned_commit_in_its_own_home() {
    let root = scratch();
    let upstream = root.join("upstream");
    fs::create_dir_all(&upstream).unwrap();
    git(&upstream, &["init", "--quiet"]);
    fs::write(upstream.join("notes.txt"), "one two three four five six\n".repeat(20)).unwrap();
    git(&upstream, &["add", "notes.txt"]);
    git(&upstream, &["commit", "--quiet", "-m", "pinned"]);
    let rev = git(&upstream, &["rev-parse", "HEAD"]);
    // A later commit the corpus must not see.
    fs::write(upstream.join("notes.txt"), "moved on\n").unwrap();
    git(&upstream, &["commit", "--quiet", "-am", "later"]);

    let rtk = root.join("fake-rtk");
    fs::write(&rtk, FAKE_RTK).unwrap();
    fs::set_permissions(&rtk, fs::Permissions::from_mode(0o755)).unwrap();
    let corpus = root.join("corpus.toml");
    fs::write(
        &corpus,
        format!(
            "[[repo]]\nname = \"demo\"\nurl = \"{}\"\nrev = \"{rev}\"\nsetup = [\"echo set > setup.txt\"]\n\n\
             [[command]]\nrepo = \"demo\"\nrun = \"cat notes.txt\"\n\
             before = \"cat setup.txt >> before.log\"\n\n\
             [[command]]\nrepo = \"demo\"\nrun = \"printf 'x%.0s' 1 2 3; echo err >&2; exit 2\"\n",
            upstream.display()
        ),
    )
    .unwrap();

    let out = root.join("out");
    let run = Command::new(env!("CARGO_BIN_EXE_output-tokens"))
        .arg("--corpus")
        .arg(&corpus)
        .arg("--work")
        .arg(root.join("work"))
        .arg("--out")
        .arg(&out)
        .arg("--rtk")
        .arg(format!("fake={}", rtk.display()))
        .args(["--wrap", "wrapped=sh -c {qcmd}"])
        .output()
        .unwrap();
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));

    let csv = fs::read_to_string(out.join("results.csv")).unwrap();
    let lines: Vec<&str> = csv.lines().collect();
    assert_eq!(lines[0], "n,repo,command,side,rewritten_to,exit,bytes,tokens");
    assert_eq!(lines.len(), 1 + 2 * 3);
    // The raw side reads the pinned file, 20 lines of 28 bytes.
    assert!(lines[1].starts_with("1,demo,cat notes.txt,raw,,0,560,"), "{}", lines[1]);
    assert_eq!(lines[2], "1,demo,cat notes.txt,fake,rtk read notes.txt,4,6,2");
    assert!(lines[3].starts_with("1,demo,cat notes.txt,wrapped,,0,560,"), "{}", lines[3]);
    // stderr is joined to stdout, and a command the hook leaves alone keeps the raw output.
    let second = "2,demo,printf 'x%.0s' 1 2 3; echo err >&2; exit 2";
    let raw_second = lines[4].strip_prefix(&format!("{second},raw,,2,7,")).unwrap();
    assert_eq!(lines[5], format!("{second},fake,,2,7,{raw_second}"));
    assert_eq!(lines[6], format!("{second},wrapped,,2,7,{raw_second}"));
    assert_eq!(
        fs::read_to_string(out.join("outputs/002-raw.txt")).unwrap(),
        "xxxerr\n"
    );

    // `before` ran once for each side that ran the command: raw, fake and wrapped.
    let before = fs::read_to_string(root.join("work/demo/before.log")).unwrap();
    assert_eq!(before, "set\n".repeat(3));
    let home = fs::read_to_string(out.join("homes/fake/ran")).unwrap();
    assert_eq!(Path::new(home.trim()), out.join("homes/fake").canonicalize().unwrap());

    let summary = fs::read_to_string(out.join("summary.md")).unwrap();
    assert!(summary.contains("- fake: `rtk 0.0.0-test`"), "{summary}");
    let fake_total = format!("| fake | {} |", 2 + raw_second.parse::<usize>().unwrap());
    let fake_row = summary.lines().find(|l| l.starts_with(&fake_total)).expect(&summary);
    // Not larger than raw, one exit code differs (4 against 0), one command rewritten.
    assert!(fake_row.ends_with("| 0 | 1 | 1 |"), "{fake_row}");

    // A rerun reuses the clone and starts again from the pinned commit, so
    // `git clean` has removed the first run's log before this one writes it.
    let rerun = Command::new(env!("CARGO_BIN_EXE_output-tokens"))
        .arg("--corpus")
        .arg(&corpus)
        .arg("--work")
        .arg(root.join("work"))
        .arg("--out")
        .arg(root.join("out2"))
        .output()
        .unwrap();
    assert!(rerun.status.success(), "{}", String::from_utf8_lossy(&rerun.stderr));
    assert_eq!(
        fs::read_to_string(root.join("work/demo/before.log")).unwrap(),
        "set\n"
    );
    let refused = Command::new(env!("CARGO_BIN_EXE_output-tokens"))
        .arg("--corpus")
        .arg(&corpus)
        .arg("--work")
        .arg(root.join("work"))
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();
    assert!(!refused.status.success(), "an existing out directory is refused");

    fs::remove_dir_all(&root).unwrap();
}
