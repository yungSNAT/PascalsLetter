//! Runs the `ab` binary with a stand-in Claude Code, to check what each session is
//! given and what lands in results.csv and summary.md, and that the budget stops it.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A fresh folder for one test; the name keeps parallel tests apart.
fn scratch(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ab-{}-{test}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn executable(path: &Path, text: &str) {
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

// Answers right only when its settings hold rtk's hook; logs its arguments, working
// directory, home and the rtk first on PATH; costs US$0.50 a session.
const FAKE_CLAUDE: &str = r#"#!/bin/sh
{ echo "args: $*"; echo "pwd: $(pwd)"; echo "home: $HOME"; echo "config: $CLAUDE_CONFIG_DIR";
  echo "rtk: $(command -v rtk)"; echo "leak: ${CLAUDE_LEAK:-none}"; } >> "$FAKE_LOG"
if grep -q 'rtk hook claude' "$CLAUDE_CONFIG_DIR/settings.json"; then answer='It is A.rs'; else answer='none'; fi
echo '{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"git status"}}]}}'
echo '{"type":"user","message":{"content":[{"type":"tool_result","content":"M a.rs"}]}}'
echo "{\"type\":\"result\",\"result\":\"$answer\",\"is_error\":false,\"num_turns\":2,\"total_cost_usd\":0.5,\"usage\":{\"input_tokens\":1,\"output_tokens\":2,\"cache_creation_input_tokens\":3,\"cache_read_input_tokens\":4}}"
"#;

struct Setup {
    dir: PathBuf,
    corpus: PathBuf,
    cases: PathBuf,
    claude: PathBuf,
    rtk: PathBuf,
}

fn setup(test: &str) -> Setup {
    let dir = scratch(test);
    fs::create_dir_all(dir.join("repos/r")).unwrap();
    let corpus = dir.join("corpus.toml");
    fs::write(
        &corpus,
        r#"
[[repo]]
name = "r"
url = "u"
rev = "x"

[[command]]
repo = "r"
run = "git status"
before = "echo ran >> \"$MARK\""

[[command]]
repo = "r"
run = "git diff"
"#,
    )
    .unwrap();
    let cases = dir.join("ab-cases.toml");
    fs::write(
        &cases,
        "[[case]]
run = \"git status\"
question = \"Which files changed?\"
answer = [\"a.rs\"]
",
    )
    .unwrap();
    let claude = dir.join("claude");
    executable(&claude, FAKE_CLAUDE);
    let rtk = dir.join("fake-rtk");
    executable(&rtk, "#!/bin/sh\n");
    Setup {
        dir,
        corpus,
        cases,
        claude,
        rtk,
    }
}

fn ab(s: &Setup, out: &str, runs: &str, budget: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_ab"))
        .args(["--corpus".as_ref(), s.corpus.as_os_str()])
        .args(["--cases".as_ref(), s.cases.as_os_str()])
        .args(["--repos".as_ref(), s.dir.join("repos").as_os_str()])
        .args(["--out".as_ref(), s.dir.join(out).as_os_str()])
        .args(["--claude".as_ref(), s.claude.as_os_str()])
        .args(["--model", "m", "--runs", runs, "--budget-usd", budget])
        .arg(format!("--rtk=fake rtk={}", s.rtk.display()))
        .env("FAKE_LOG", s.dir.join("claude.log"))
        .env("MARK", s.dir.join("mark"))
        .env("CLAUDE_LEAK", "leaked")
        .output()
        .unwrap()
}

#[test]
fn each_side_gets_its_own_hook_home_and_rtk_and_is_graded() {
    let s = setup("each_side_gets_its_own_hook_home_and_rtk_and_is_graded");
    let out = ab(&s, "out", "2", "100");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "2.000000");

    // Only the listed case runs: 2 runs of 2 sides, `before` each time.
    assert_eq!(
        fs::read_to_string(s.dir.join("mark"))
            .unwrap()
            .lines()
            .count(),
        4
    );
    let csv = fs::read_to_string(s.dir.join("out/results.csv")).unwrap();
    let rows: Vec<&str> = csv.lines().skip(1).collect();
    assert_eq!(rows.len(), 4);
    // Run 1 starts with raw, run 2 with the rtk side.
    assert!(rows[0].starts_with(r#""1","1","raw","ok","false","none""#));
    assert!(rows[1].starts_with(r#""1","1","fake rtk","ok","true","It is A.rs""#));
    assert!(rows[2].starts_with(r#""2","1","fake rtk""#));
    assert!(rows[3].starts_with(r#""2","1","raw""#));
    assert!(rows[0].contains(r#","2","1","git status","6","#));
    assert!(rows[0].contains(r#","1","2","3","4","0.500000""#));

    let log = fs::read_to_string(s.dir.join("claude.log")).unwrap();
    assert!(log.contains("args: -p Run this bash command: git status   Then answer in one short line: Which files changed? --model m --allowedTools Bash --setting-sources user --output-format stream-json --verbose"));
    let repo = fs::canonicalize(s.dir.join("repos/r")).unwrap();
    assert!(log.contains(&format!("pwd: {}", repo.display())));
    assert!(
        !log.contains("leaked"),
        "a CLAUDE* variable reached the session"
    );
    let homes = fs::canonicalize(s.dir.join("out/homes")).unwrap();
    assert!(log.contains(&format!("home: {}", homes.join("01-001-raw").display())));
    assert!(log.contains(&format!(
        "rtk: {}",
        homes.join("01-001-fake-rtk/rtk-bin/rtk").display()
    )));
    assert_eq!(
        fs::read_to_string(homes.join("01-001-raw/.claude/settings.json")).unwrap(),
        "{}"
    );
    assert_eq!(
        fs::read_link(homes.join("01-001-fake-rtk/rtk-bin/rtk")).unwrap(),
        fs::canonicalize(&s.rtk).unwrap()
    );

    let summary = fs::read_to_string(s.dir.join("out/summary.md")).unwrap();
    assert!(summary.contains("| raw | 0 of 2 | 0 |"));
    assert!(summary.contains("| fake rtk | 2 of 2 | 0 |"));
    assert!(!summary.contains("budget"));
}

#[test]
fn the_budget_stops_the_plan_before_the_next_session() {
    let s = setup("the_budget_stops_the_plan_before_the_next_session");
    let out = ab(&s, "out", "5", "0.7");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // 0.50 is under 0.70, so a second session runs; then 1.00 stops the plan.
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "1.000000");
    let csv = fs::read_to_string(s.dir.join("out/results.csv")).unwrap();
    assert_eq!(csv.lines().count(), 3);
    let summary = fs::read_to_string(s.dir.join("out/summary.md")).unwrap();
    assert!(summary.contains("2 ran. The budget stopped the plan early."));
}
