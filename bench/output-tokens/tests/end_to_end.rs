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
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
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

// Stands in for Chisle's hook: replaces the notes with "cut", and replies
// with nothing for any other output, which leaves it as it was.
const FAKE_CHISLE: &str = r#"let input = '';
process.stdin.on('data', (chunk) => { input += chunk; });
process.stdin.on('end', () => {
  const result = JSON.parse(input).toolResult;
  if (result.textResultForLlm.startsWith('one two')) {
    process.stdout.write(JSON.stringify({ modifiedResult: { resultType: result.resultType, textResultForLlm: 'cut' } }));
  }
});
"#;

// Stands in for a Claude Code plugin: logs each event it is given, starts the
// session with plain text, rewrites `echo long`, refuses `rm -rf x` by exit 2,
// replaces the notes and adds context to a failed call.
const FAKE_PLUGIN_HOOK: &str = r#"let input = '';
process.stdin.on('data', (chunk) => { input += chunk; });
process.stdin.on('end', () => {
  const e = JSON.parse(input);
  require('fs').appendFileSync(process.env.CLAUDE_PLUGIN_DATA + '/events.log',
    `${e.hook_event_name} ${e.tool_input ? e.tool_input.command : e.source}\n`);
  const reply = (o) => process.stdout.write(JSON.stringify({ hookSpecificOutput: { hookEventName: e.hook_event_name, ...o } }));
  switch (e.hook_event_name) {
    case 'SessionStart': process.stdout.write('session hello'); break;
    case 'PreToolUse':
      if (e.tool_input.command === 'echo long') reply({ permissionDecision: 'allow', updatedInput: { command: 'echo short' } });
      if (e.tool_input.command === 'rm -rf x') { process.stderr.write('refused'); process.exit(2); }
      break;
    case 'PostToolUse':
      if (e.tool_response.stdout.startsWith('one two')) reply({ updatedToolOutput: { stdout: 'cut', stderr: '', interrupted: false }, additionalContext: 'see notes' });
      break;
    case 'PostToolUseFailure': reply({ additionalContext: 'failed: ' + e.error.split('\n')[0] }); break;
  }
});
"#;

// Exec and shell forms, matchers that select Bash and one that does not (its
// prompt hook would be refused if it were picked), and a hook that outlives
// its timeout, whose reply is dropped.
const FAKE_PLUGIN_HOOKS: &str = r#"{"hooks": {
  "SessionStart": [{"hooks": [{"type": "command", "command": "node", "args": ["${CLAUDE_PLUGIN_ROOT}/hook.js"]}]}],
  "PreToolUse": [
    {"matcher": "^(Bash|Read)$", "hooks": [{"type": "command", "command": "node \"${CLAUDE_PLUGIN_ROOT}/hook.js\""}]},
    {"matcher": "Read", "hooks": [{"type": "prompt", "prompt": "x"}]}
  ],
  "PostToolUse": [{"matcher": "Edit|Bash", "hooks": [
    {"type": "command", "command": "node", "args": ["${CLAUDE_PLUGIN_ROOT}/hook.js"]},
    {"type": "command", "command": "sleep 3; echo late", "timeout": 1}
  ]}],
  "PostToolUseFailure": [{"hooks": [{"type": "command", "command": "node", "args": ["${CLAUDE_PLUGIN_ROOT}/hook.js"]}]}],
  "Stop": [{"hooks": [{"type": "prompt", "prompt": "x"}]}]
}}"#;

#[test]
fn a_plugin_side_gets_its_hooks_as_claude_code_runs_them() {
    let root = scratch();
    let upstream = root.join("upstream");
    fs::create_dir_all(&upstream).unwrap();
    git(&upstream, &["init", "--quiet"]);
    fs::write(upstream.join("notes.txt"), "one two three four\n").unwrap();
    git(&upstream, &["add", "notes.txt"]);
    git(&upstream, &["commit", "--quiet", "-m", "pinned"]);
    let rev = git(&upstream, &["rev-parse", "HEAD"]);

    let plugin = root.join("plugin");
    fs::create_dir_all(plugin.join(".claude-plugin")).unwrap();
    fs::create_dir_all(plugin.join("hooks")).unwrap();
    fs::write(
        plugin.join(".claude-plugin/plugin.json"),
        r#"{"name": "fake", "hooks": "./hooks/claude.json"}"#,
    )
    .unwrap();
    fs::write(plugin.join("hooks/claude.json"), FAKE_PLUGIN_HOOKS).unwrap();
    fs::write(plugin.join("hook.js"), FAKE_PLUGIN_HOOK).unwrap();
    let corpus = root.join("corpus.toml");
    fs::write(
        &corpus,
        format!(
            "[[repo]]\nname = \"demo\"\nurl = \"{}\"\nrev = \"{rev}\"\n\n\
             [[command]]\nrepo = \"demo\"\nrun = \"cat notes.txt\"\n\
             question = \"What do the notes say?\"\nkeep = [\"three four\"]\n\n\
             [[command]]\nrepo = \"demo\"\nrun = \"echo long\"\nbefore = \"echo x >> before.log\"\n\n\
             [[command]]\nrepo = \"demo\"\nrun = \"printf oops; exit 2\"\n\n\
             [[command]]\nrepo = \"demo\"\nrun = \"rm -rf x\"\n",
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
        .arg("--claude-plugin")
        .arg(format!("fake plugin={}", plugin.display()))
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );

    let output =
        |n: u32| fs::read_to_string(out.join(format!("outputs/{n:03}-fake-plugin.txt"))).unwrap();
    // A replaced output, with the context its hook added after it.
    assert_eq!(output(1), "cut\nsee notes");
    // A rewritten command runs after `before`, in place of the raw one.
    assert_eq!(output(2), "short\n");
    assert_eq!(
        fs::read_to_string(root.join("work/demo/before.log")).unwrap(),
        "x\nx\n"
    );
    // A failed call keeps its output; its failure hook can only add to it.
    assert_eq!(output(3), "oops\nfailed: Exit code 2");
    // A refused call never runs: the agent reads the refusal.
    assert_eq!(output(4), "refused");
    assert_eq!(
        fs::read_to_string(out.join("outputs/000-fake-plugin-session-start.txt")).unwrap(),
        "session hello"
    );

    let csv = fs::read_to_string(out.join("results.csv")).unwrap();
    let rows: Vec<&str> = csv
        .lines()
        .filter(|l| l.contains(",fake plugin,"))
        .collect();
    assert_eq!(rows.len(), 4, "{csv}");
    assert!(
        rows[1].starts_with("2,demo,echo long,fake plugin,echo short,0,"),
        "{}",
        rows[1]
    );
    assert!(rows[2].contains(",fake plugin,,2,"), "{}", rows[2]);
    assert!(
        rows[3].starts_with("4,demo,rm -rf x,fake plugin,,none,"),
        "{}",
        rows[3]
    );

    // PostToolUse gets the command that ran; a failed call gets PostToolUseFailure only.
    let data = out.join("homes/fake-plugin/.claude/plugins/data/output-tokens-fake-plugin");
    assert_eq!(
        fs::read_to_string(data.join("events.log")).unwrap(),
        "SessionStart startup\n\
         PreToolUse cat notes.txt\nPostToolUse cat notes.txt\n\
         PreToolUse echo long\nPostToolUse echo short\n\
         PreToolUse printf oops; exit 2\nPostToolUseFailure printf oops; exit 2\n\
         PreToolUse rm -rf x\n"
    );
    let log = fs::read_to_string(out.join("hooks/fake-plugin.jsonl")).unwrap();
    assert_eq!(
        log.lines().filter(|l| l.contains("timed out")).count(),
        2,
        "{log}"
    );

    let summary = fs::read_to_string(out.join("summary.md")).unwrap();
    assert!(
        summary.contains("- fake plugin: its SessionStart hooks give the agent "),
        "{summary}"
    );
    let facts = fs::read_to_string(out.join("facts.md")).unwrap();
    assert!(
        facts
            .lines()
            .any(|l| l == "| fake plugin | 0 of 1 | 1 | 0 |"),
        "{facts}"
    );

    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn every_side_runs_from_the_pinned_commit_in_its_own_home() {
    let root = scratch();
    let upstream = root.join("upstream");
    fs::create_dir_all(&upstream).unwrap();
    git(&upstream, &["init", "--quiet"]);
    fs::write(
        upstream.join("notes.txt"),
        "one two three four five six\n".repeat(20),
    )
    .unwrap();
    git(&upstream, &["add", "notes.txt"]);
    git(&upstream, &["commit", "--quiet", "-m", "pinned"]);
    let rev = git(&upstream, &["rev-parse", "HEAD"]);
    // A later commit the corpus must not see.
    fs::write(upstream.join("notes.txt"), "moved on\n").unwrap();
    git(&upstream, &["commit", "--quiet", "-am", "later"]);

    let rtk = root.join("fake-rtk");
    fs::write(&rtk, FAKE_RTK).unwrap();
    fs::set_permissions(&rtk, fs::Permissions::from_mode(0o755)).unwrap();
    let chisle = root.join("fake-chisle.js");
    fs::write(&chisle, FAKE_CHISLE).unwrap();
    let corpus = root.join("corpus.toml");
    fs::write(
        &corpus,
        format!(
            "[[repo]]\nname = \"demo\"\nurl = \"{}\"\nrev = \"{rev}\"\nsetup = [\"echo set > setup.txt\"]\n\n\
             [[command]]\nrepo = \"demo\"\nrun = \"cat notes.txt\"\n\
             before = \"cat setup.txt >> before.log\"\n\
             question = \"What do the notes say?\"\nkeep = [\"three four\"]\n\n\
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
        .arg("--chisle")
        .arg(format!("trimmed={}", chisle.display()))
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );

    let csv = fs::read_to_string(out.join("results.csv")).unwrap();
    let lines: Vec<&str> = csv.lines().collect();
    assert_eq!(
        lines[0],
        "n,repo,command,side,rewritten_to,exit,bytes,tokens"
    );
    assert_eq!(lines.len(), 1 + 2 * 4);
    // The raw side reads the pinned file, 20 lines of 28 bytes.
    assert!(
        lines[1].starts_with("1,demo,cat notes.txt,raw,,0,560,"),
        "{}",
        lines[1]
    );
    assert_eq!(
        lines[2],
        "1,demo,cat notes.txt,fake,rtk read notes.txt,4,6,2"
    );
    assert!(
        lines[3].starts_with("1,demo,cat notes.txt,wrapped,,0,560,"),
        "{}",
        lines[3]
    );
    // The Chisle side takes the raw output and its exit code.
    assert!(
        lines[4].starts_with("1,demo,cat notes.txt,trimmed,,0,3,"),
        "{}",
        lines[4]
    );
    // stderr is joined to stdout, and a command the hook leaves alone keeps the raw output.
    let second = "2,demo,printf 'x%.0s' 1 2 3; echo err >&2; exit 2";
    let raw_second = lines[5]
        .strip_prefix(&format!("{second},raw,,2,7,"))
        .unwrap();
    assert_eq!(lines[6], format!("{second},fake,,2,7,{raw_second}"));
    assert_eq!(lines[7], format!("{second},wrapped,,2,7,{raw_second}"));
    assert_eq!(lines[8], format!("{second},trimmed,,2,7,{raw_second}"));
    assert_eq!(
        fs::read_to_string(out.join("outputs/002-raw.txt")).unwrap(),
        "xxxerr\n"
    );

    // `before` ran once for each side that ran the command: raw, fake and wrapped.
    let before = fs::read_to_string(root.join("work/demo/before.log")).unwrap();
    assert_eq!(before, "set\n".repeat(3));
    let home = fs::read_to_string(out.join("homes/fake/ran")).unwrap();
    assert_eq!(
        Path::new(home.trim()),
        out.join("homes/fake").canonicalize().unwrap()
    );

    let summary = fs::read_to_string(out.join("summary.md")).unwrap();
    assert!(summary.contains("- fake: `rtk 0.0.0-test`"), "{summary}");
    let fake_total = format!("| fake | {} |", 2 + raw_second.parse::<usize>().unwrap());
    let fake_row = summary
        .lines()
        .find(|l| l.starts_with(&fake_total))
        .expect(&summary);
    // Not larger than raw, one exit code differs (4 against 0), one command rewritten.
    assert!(fake_row.ends_with("| 0 | 1 | 1 |"), "{fake_row}");

    // Only the first command asks a question; the stand-in rtk and Chisle cut its one fact.
    let facts = fs::read_to_string(out.join("facts.md")).unwrap();
    for row in [
        "| raw | 1 of 1 | 0 | 0 |",
        "| fake | 0 of 1 | 1 | 0 |",
        "| wrapped | 1 of 1 | 0 | 0 |",
        "| trimmed | 0 of 1 | 1 | 0 |",
        "| fake | 0 | no | `three four` |",
    ] {
        assert!(facts.lines().any(|l| l == row), "{row} missing:\n{facts}");
    }
    assert!(!facts.contains("printf"), "{facts}");

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
    assert!(
        rerun.status.success(),
        "{}",
        String::from_utf8_lossy(&rerun.stderr)
    );
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
    assert!(
        !refused.status.success(),
        "an existing out directory is refused"
    );

    fs::remove_dir_all(&root).unwrap();
}
