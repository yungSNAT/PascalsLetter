//! A billed A/B over corpus commands in the style of dtk's `scripts/ab-test.sh`: for each
//! case in `ab-cases.toml`, a headless Claude Code session is asked to run the command and
//! answer a question in one line, with no hook (raw) and with each rtk build's PreToolUse
//! hook. An answer is right when it holds every word of the case's answer whole, case
//! aside. Usage and cost come from each session's `result` event. Needs the corpus clones
//! an output-tokens run set up (`published` leaves them in `<work>/repos`) and
//! `ANTHROPIC_API_KEY` in the environment.

#[path = "../fact.rs"]
#[allow(dead_code)] // `copies`, `Keep` and some corpus fields serve the output-tokens run only.
mod fact;

use fact::{Corpus, CorpusCommand, count_tokens, label_value, slug};

use anyhow::{Context, Result, ensure};
use clap::Parser;
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const SESSION_TIMEOUT: Duration = Duration::from_secs(900);
const RTK_HOOK: &str = r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"rtk hook claude"}]}]}}"#;

#[derive(Parser)]
#[command(about = "Billed A/B over corpus commands, in dtk's ab-test.sh style")]
struct Args {
    #[arg(long, default_value = "corpus.toml")]
    corpus: PathBuf,
    #[arg(long, default_value = "ab-cases.toml")]
    cases: PathBuf,
    /// The corpus clones, already set up by an output-tokens run.
    #[arg(long)]
    repos: PathBuf,
    /// A new folder for results.csv, summary.md, transcripts and session homes.
    #[arg(long)]
    out: PathBuf,
    /// The Claude Code binary.
    #[arg(long)]
    claude: PathBuf,
    /// The model every session uses.
    #[arg(long)]
    model: String,
    /// Sessions per side per case.
    #[arg(long, default_value_t = 5)]
    runs: usize,
    /// An rtk build to hook in, as LABEL=PATH (repeatable). Raw always runs.
    #[arg(long = "rtk", value_name = "LABEL=PATH", value_parser = label_value)]
    rtk: Vec<(String, String)>,
    /// No session starts once the sessions so far cost this much (their `total_cost_usd`).
    #[arg(long)]
    budget_usd: f64,
    /// Only the cases whose command matches this pattern.
    #[arg(long)]
    only: Option<Regex>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Cases {
    case: Vec<Case>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    /// A corpus command's `run`.
    run: String,
    question: String,
    /// What a right answer holds; every word of each must stand whole in it.
    answer: Vec<String>,
}

/// Each case's corpus command, failing on a case the corpus does not have.
fn commands<'a>(corpus: &'a Corpus, cases: &[Case]) -> Result<Vec<&'a CorpusCommand>> {
    cases
        .iter()
        .map(|case| {
            ensure!(
                !case.answer.is_empty(),
                "case '{}' lists no answer",
                case.run
            );
            corpus
                .command
                .iter()
                .find(|c| c.run == case.run)
                .with_context(|| format!("case '{}' is not a corpus command", case.run))
        })
        .collect()
}

/// dtk's `correct()`, every word of the answer in the reply, case aside, with each word
/// held whole by the facts check's rule, so `37` is not in `137`. The rule lets a `.`
/// follow a fact, as a full stop; a word must not run on past one into more of a name or
/// number, so neither `Cargo.lock.bak` nor `112.5` holds `Cargo.lock` or `112`.
fn correct(reply: &str, answer: &[String]) -> bool {
    let reply = reply.to_lowercase();
    let runs_on = |after: &str| {
        after
            .strip_prefix('.')
            .is_some_and(|rest| rest.starts_with(char::is_alphanumeric))
    };
    answer
        .iter()
        .flat_map(|a| a.split_whitespace())
        .all(|word| fact::matches(&reply, &word.to_lowercase()).any(|after| !runs_on(after)))
}

/// What one session's stream-json transcript reports.
#[derive(Default, Debug, PartialEq)]
struct Session {
    answer: String,
    is_error: bool,
    turns: u64,
    cost_usd: f64,
    // None when the result reports no usage: unknown, not zero.
    input: Option<u64>,
    output: Option<u64>,
    cache_write: Option<u64>,
    cache_read: Option<u64>,
    model_usage: String,
    tool_calls: usize,
    commands: Vec<String>,
    tool_result_chars: usize,
    tool_result_tokens: usize,
}

fn tool_result_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect(),
        _ => String::new(),
    }
}

/// Reads a transcript; `None` when it has no `result` event or that event gives no cost.
fn parse(transcript: &str) -> Option<Session> {
    let mut s = Session::default();
    let mut result = None;
    for event in transcript
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
    {
        let empty = Vec::new();
        let blocks = event
            .pointer("/message/content")
            .and_then(Value::as_array)
            .unwrap_or(&empty);
        match event["type"].as_str() {
            Some("assistant") => {
                for b in blocks.iter().filter(|b| b["type"] == "tool_use") {
                    s.tool_calls += 1;
                    if let Some(cmd) = b.pointer("/input/command").and_then(Value::as_str) {
                        s.commands.push(cmd.to_string());
                    }
                }
            }
            Some("user") => {
                for b in blocks.iter().filter(|b| b["type"] == "tool_result") {
                    let text = tool_result_text(&b["content"]);
                    s.tool_result_chars += text.chars().count();
                    s.tool_result_tokens += count_tokens(&text);
                }
            }
            Some("result") => result = Some(event),
            _ => {}
        }
    }
    let r = result?;
    let n = |p: &str| r.pointer(p).and_then(Value::as_u64).unwrap_or(0);
    s.answer = r["result"].as_str().unwrap_or_default().to_string();
    s.is_error = r["is_error"].as_bool().unwrap_or(false);
    s.turns = n("/num_turns");
    s.cost_usd = r["total_cost_usd"].as_f64()?;
    let usage = |p: &str| r.pointer(p).and_then(Value::as_u64);
    s.input = usage("/usage/input_tokens");
    s.output = usage("/usage/output_tokens");
    s.cache_write = usage("/usage/cache_creation_input_tokens");
    s.cache_read = usage("/usage/cache_read_input_tokens");
    s.model_usage = r
        .get("modelUsage")
        .map(Value::to_string)
        .unwrap_or_default();
    Some(s)
}

struct Side {
    label: String,
    rtk: Option<PathBuf>,
}

/// A session's own home and Claude Code configuration, as THOL isolates a run, so no
/// user settings, hooks or memory reach it. An rtk side's settings hold its hook only,
/// and PATH starts with a directory holding only that rtk, then the repository's
/// `path` entries. Returns that PATH.
fn make_home(home: &Path, side: &Side, first: &[PathBuf]) -> Result<OsString> {
    let config = home.join(".claude");
    fs::create_dir_all(&config)?;
    fs::create_dir_all(home.join("tmp"))?;
    fs::write(
        config.join(".claude.json"),
        r#"{"hasCompletedOnboarding":true}"#,
    )?;
    let mut path = Vec::new();
    let settings = match &side.rtk {
        Some(bin) => {
            let shim = home.join("rtk-bin");
            fs::create_dir_all(&shim)?;
            link_rtk(bin, &shim)?;
            path.push(shim);
            RTK_HOOK
        }
        None => "{}",
    };
    fs::write(config.join("settings.json"), settings)?;
    path.extend(first.iter().cloned());
    if let Some(user_path) = std::env::var_os("PATH") {
        path.extend(std::env::split_paths(&user_path));
    }
    Ok(std::env::join_paths(path)?)
}

/// The session environment: the parent's, less any `CLAUDE*` or `XDG_*` variable, with
/// the session's home, configuration, temporary directory and PATH. Cargo and rustup
/// keep the user's directories.
fn session_env(cmd: &mut Command, home: &Path, path: &OsString) -> Result<()> {
    let user_home = std::env::var_os("HOME").context("HOME is not set")?;
    for (var, dir) in [("CARGO_HOME", ".cargo"), ("RUSTUP_HOME", ".rustup")] {
        if std::env::var_os(var).is_none() {
            cmd.env(var, Path::new(&user_home).join(dir));
        }
    }
    for (var, _) in std::env::vars_os() {
        let name = var.to_string_lossy();
        if name.starts_with("CLAUDE") || name.starts_with("XDG_") {
            cmd.env_remove(&var);
        }
    }
    cmd.env("HOME", home)
        .env("TMPDIR", home.join("tmp"))
        .env("PATH", path)
        .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
        .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
        .env("DISABLE_AUTOUPDATER", "1")
        .env("RTK_TELEMETRY_DISABLED", "1");
    Ok(())
}

#[cfg(unix)]
fn link_rtk(bin: &Path, dir: &Path) -> Result<()> {
    std::os::unix::fs::symlink(fs::canonicalize(bin)?, dir.join("rtk")).context("linking rtk")
}

#[cfg(not(unix))]
fn link_rtk(_bin: &Path, _dir: &Path) -> Result<()> {
    anyhow::bail!("ab needs a Unix shell")
}

/// Runs a corpus `before` command in the session's environment; it must succeed.
fn before(script: &str, dir: &Path, home: &Path, path: &OsString) -> Result<()> {
    // nosemgrep: interpreter-execution -- a benchmark, not rtk: runs the corpus's `before` command through sh
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(script)
        .current_dir(dir)
        .stdin(Stdio::null());
    session_env(&mut cmd, home, path)?;
    let out = cmd.output().with_context(|| format!("running {script}"))?;
    ensure!(
        out.status.success(),
        "'{script}' failed in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(())
}

/// Runs one session in `dir`, writing its transcript; `false` when it timed out.
fn claude(
    args: &Args,
    prompt: &str,
    dir: &Path,
    home: &Path,
    path: &OsString,
    transcript: &Path,
) -> Result<bool> {
    // nosemgrep: dynamic-command-execution -- a benchmark, not rtk: runs the Claude Code binary under test (--claude)
    let mut cmd = Command::new(&args.claude);
    cmd.args(["-p", prompt, "--model", &args.model])
        .args(["--allowedTools", "Bash", "--setting-sources", "user"])
        .args(["--output-format", "stream-json", "--verbose"])
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(fs::File::create(transcript)?)
        .stderr(fs::File::create(transcript.with_extension("stderr"))?);
    session_env(&mut cmd, home, path)?;
    let mut child = cmd.spawn().context("starting claude")?;
    let deadline = Instant::now() + SESSION_TIMEOUT;
    loop {
        if child.try_wait()?.is_some() {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            child.kill()?;
            child.wait()?;
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// A count as a CSV field, or an empty one when it is unknown.
fn blank_if_none(n: Option<u64>) -> String {
    n.map(|n| n.to_string()).unwrap_or_default()
}

fn csv(fields: &[String]) -> String {
    let quoted: Vec<String> = fields
        .iter()
        .map(|f| format!("\"{}\"", f.replace('"', "\"\"")))
        .collect();
    quoted.join(",") + "\n"
}

struct Row {
    case: usize,
    side: usize,
    finished: bool,
    session: Session,
    right: bool,
}

fn mean(values: impl Iterator<Item = f64>) -> f64 {
    let (sum, n) = values.fold((0.0, 0usize), |(s, n), v| (s + v, n + 1));
    if n == 0 { 0.0 } else { sum / n as f64 }
}

fn summary(
    args: &Args,
    cases: &[Case],
    commands: &[&CorpusCommand],
    sides: &[Side],
    rows: &[Row],
    stopped: Option<&str>,
) -> String {
    let mut md = format!(
        "Model `{}`, {} session(s) planned per side per case; {} ran.{}\n\n",
        args.model,
        args.runs,
        rows.len(),
        stopped.map(|why| format!(" {why}")).unwrap_or_default()
    );
    md.push_str("| Side | Right | Not finished | Tool result tokens (mean) | Turns (mean) | Cost (US$, total_cost_usd) |\n| --- | ---: | ---: | ---: | ---: | ---: |\n");
    for (i, side) in sides.iter().enumerate() {
        let mine: Vec<&Row> = rows.iter().filter(|r| r.side == i).collect();
        let done = || mine.iter().filter(|r| r.finished);
        let _ = writeln!(
            md,
            "| {} | {} of {} | {} | {:.0} | {:.1} | {:.4} |",
            side.label,
            mine.iter().filter(|r| r.right).count(),
            mine.len(),
            mine.len() - done().count(),
            mean(done().map(|r| r.session.tool_result_tokens as f64)),
            mean(done().map(|r| r.session.turns as f64)),
            mine.iter().map(|r| r.session.cost_usd).sum::<f64>(),
        );
    }
    md.push_str("\nPer case: sessions right, and mean tool result tokens.\n\n| # | Repository | Command | Answer |");
    for side in sides {
        let _ = write!(md, " {} |", side.label);
    }
    md.push_str("\n| ---: | --- | --- | --- |");
    md.push_str(&" --- |".repeat(sides.len()));
    md.push('\n');
    for (c, case) in cases.iter().enumerate() {
        let _ = write!(
            md,
            "| {} | {} | `{}` | {} |",
            c + 1,
            commands[c].repo,
            case.run,
            case.answer.join("; ")
        );
        for s in 0..sides.len() {
            let mine: Vec<&Row> = rows.iter().filter(|r| r.case == c && r.side == s).collect();
            let tokens = mean(
                mine.iter()
                    .filter(|r| r.finished)
                    .map(|r| r.session.tool_result_tokens as f64),
            );
            let right = mine.iter().filter(|r| r.right).count();
            let _ = write!(md, " {right} of {}, {tokens:.0} |", mine.len());
        }
        md.push('\n');
    }
    md
}

fn main() -> Result<()> {
    let args = Args::parse();
    let corpus: Corpus = toml::from_str(&fs::read_to_string(&args.corpus)?)
        .with_context(|| format!("reading {}", args.corpus.display()))?;
    let cases: Vec<Case> = toml::from_str::<Cases>(&fs::read_to_string(&args.cases)?)
        .with_context(|| format!("reading {}", args.cases.display()))?
        .case
        .into_iter()
        .filter(|c| args.only.as_ref().is_none_or(|re| re.is_match(&c.run)))
        .collect();
    ensure!(!cases.is_empty(), "no case to run");
    let commands = commands(&corpus, &cases)?;
    let mut sides = vec![Side {
        label: "raw".into(),
        rtk: None,
    }];
    for (label, path) in &args.rtk {
        sides.push(Side {
            label: label.clone(),
            rtk: Some(PathBuf::from(path)),
        });
    }
    let mut slugs: Vec<String> = sides.iter().map(|s| slug(&s.label)).collect();
    slugs.sort();
    slugs.dedup();
    ensure!(
        slugs.len() == sides.len(),
        "2 sides would share a session folder; give each --rtk a label distinct from the others and from raw"
    );
    ensure!(
        args.budget_usd.is_finite() && args.budget_usd > 0.0,
        "--budget-usd must be a positive number"
    );
    ensure!(
        !args.out.exists(),
        "{} exists; give a new folder",
        args.out.display()
    );
    fs::create_dir_all(args.out.join("transcripts"))?;
    let homes = fs::canonicalize(&args.out)?.join("homes");
    let header = [
        "run",
        "case",
        "side",
        "status",
        "right",
        "answer",
        "turns",
        "tool_calls",
        "commands",
        "tool_result_chars",
        "tool_result_tokens",
        "input_tokens",
        "output_tokens",
        "cache_creation_tokens",
        "cache_read_tokens",
        "total_cost_usd",
        "model_usage",
    ];
    let mut table = csv(&header.map(String::from));
    let mut rows = Vec::new();
    let mut spent = 0.0;
    let mut stopped = None;
    'plan: for run in 0..args.runs {
        for (c, case) in cases.iter().enumerate() {
            let command = commands[c];
            let repo = corpus
                .repo
                .iter()
                .find(|r| r.name == command.repo)
                .with_context(|| format!("no [[repo]] named {}", command.repo))?;
            let dir = fs::canonicalize(args.repos.join(&repo.name)).with_context(|| {
                format!("no clone of {} in {}", repo.name, args.repos.display())
            })?;
            let first: Vec<PathBuf> = repo.path.iter().map(|p| dir.join(p)).collect();
            let prompt = format!(
                "Run this bash command: {}   Then answer in one short line: {}",
                case.run, case.question
            );
            // Each run starts with a different side, so drift spreads over the sides.
            for k in 0..sides.len() {
                if spent >= args.budget_usd {
                    stopped = Some("The budget stopped the plan early.");
                    break 'plan;
                }
                let s = (k + run) % sides.len();
                let name = format!("{:02}-{:03}-{}", run + 1, c + 1, slug(&sides[s].label));
                let home = homes.join(&name);
                let path = make_home(&home, &sides[s], &first)?;
                if let Some(script) = &command.before {
                    before(script, &dir, &home, &path)?;
                }
                let transcript = args.out.join("transcripts").join(format!("{name}.jsonl"));
                let finished = claude(&args, &prompt, &dir, &home, &path, &transcript)?;
                let parsed = parse(&fs::read_to_string(&transcript)?);
                let (status, session) = match (finished, parsed) {
                    (false, _) => ("timeout", Session::default()),
                    (true, None) => ("no result", Session::default()),
                    (true, Some(s)) if s.is_error => ("error", s),
                    (true, Some(s)) => ("ok", s),
                };
                let finished = status == "ok";
                // Only a `result` event carries the session's cost.
                let priced = matches!(status, "ok" | "error");
                let right = finished && correct(&session.answer, &case.answer);
                spent += session.cost_usd;
                eprintln!(
                    "{name}: {status} right={right} ${:.4} (total ${spent:.4})",
                    session.cost_usd
                );
                table.push_str(&csv(&[
                    (run + 1).to_string(),
                    (c + 1).to_string(),
                    sides[s].label.clone(),
                    status.to_string(),
                    right.to_string(),
                    session.answer.clone(),
                    session.turns.to_string(),
                    session.tool_calls.to_string(),
                    session.commands.join(" ; "),
                    session.tool_result_chars.to_string(),
                    session.tool_result_tokens.to_string(),
                    blank_if_none(session.input),
                    blank_if_none(session.output),
                    blank_if_none(session.cache_write),
                    blank_if_none(session.cache_read),
                    if priced {
                        format!("{:.6}", session.cost_usd)
                    } else {
                        String::new()
                    },
                    session.model_usage.clone(),
                ]));
                fs::write(args.out.join("results.csv"), &table)?;
                rows.push(Row {
                    case: c,
                    side: s,
                    finished,
                    session,
                    right,
                });
                if !priced {
                    stopped = Some(
                        "A session ended without its cost, which the budget cannot count, so the plan stopped there.",
                    );
                    break 'plan;
                }
            }
        }
    }
    fs::write(
        args.out.join("summary.md"),
        summary(&args, &cases, &commands, &sides, &rows, stopped),
    )?;
    println!("{spent:.6}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(list: &[&str]) -> Vec<String> {
        list.iter().map(|w| w.to_string()).collect()
    }

    #[test]
    fn a_reply_is_right_only_with_every_word_whole() {
        let answer = words(&["deliberate failure", "Cargo.lock"]);
        assert!(correct("A DELIBERATE failure; cargo.lock changed", &answer));
        assert!(!correct("deliberate failure", &answer));
        assert!(!correct("deliberate failures in Cargo.lock", &answer));
        assert!(correct("250 lines", &words(&["250"])));
        assert!(!correct("1250 lines", &words(&["250"])));
        assert!(!correct("2500 lines", &words(&["250"])));
        // A short hash still matches the full hash it starts.
        assert!(correct(
            "newest 4649aa9700619f94, oldest f7b677d",
            &words(&["4649aa9", "f7b677d"])
        ));
        assert!(!correct("main.rsx", &words(&["main.rs"])));
        assert!(correct("It is main.rs.", &words(&["main.rs"])));
        // A full stop may follow a word; more of a name or number may not.
        assert!(!correct("Cargo.lock.bak changed", &words(&["Cargo.lock"])));
        assert!(!correct("112.5 options", &words(&["112"])));
        assert!(correct("Oldest: f7b677d9.", &words(&["f7b677d"])));
        assert!(correct(
            "Cargo.lock.bak and Cargo.lock",
            &words(&["Cargo.lock"])
        ));
    }

    #[test]
    fn every_case_must_name_a_corpus_command_and_an_answer() {
        let corpus: Corpus = toml::from_str(
            r#"
            [[repo]]
            name = "r"
            url = "u"
            rev = "x"
            [[command]]
            repo = "r"
            run = "git status"
            question = "Which files?"
            keep = ["a.rs"]
            "#,
        )
        .unwrap();
        let case = |run: &str, answer: &[&str]| Case {
            run: run.into(),
            question: "q".into(),
            answer: words(answer),
        };
        let found = commands(&corpus, &[case("git status", &["a.rs"])]).unwrap();
        assert_eq!(found[0].repo, "r");
        let err = commands(&corpus, &[case("git diff", &["a.rs"])]).unwrap_err();
        assert!(err.to_string().contains("not a corpus command"));
        let err = commands(&corpus, &[case("git status", &[])]).unwrap_err();
        assert!(err.to_string().contains("lists no answer"));
    }

    #[test]
    fn the_shipped_cases_name_corpus_commands() {
        let here = Path::new(env!("CARGO_MANIFEST_DIR"));
        let corpus: Corpus =
            toml::from_str(&fs::read_to_string(here.join("corpus.toml")).unwrap()).unwrap();
        let cases: Cases =
            toml::from_str(&fs::read_to_string(here.join("ab-cases.toml")).unwrap()).unwrap();
        assert_eq!(commands(&corpus, &cases.case).unwrap().len(), 11);
    }

    #[test]
    fn a_transcript_gives_the_answer_usage_and_every_tool_result() {
        let transcript = [
            r#"{"type":"system","subtype":"init"}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"git status"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"hello world"}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","content":[{"type":"text","text":"hello"}]}]}}"#,
            "not json",
            r#"{"type":"result","result":"a.rs","is_error":false,"num_turns":2,"total_cost_usd":0.01,"usage":{"input_tokens":3,"output_tokens":4,"cache_creation_input_tokens":5,"cache_read_input_tokens":6},"modelUsage":{"m":{}}}"#,
        ]
        .join("\n");
        let s = parse(&transcript).unwrap();
        assert_eq!(s.answer, "a.rs");
        assert_eq!(
            (s.turns, s.input, s.output, s.cache_write, s.cache_read),
            (2, Some(3), Some(4), Some(5), Some(6))
        );
        assert_eq!(s.tool_calls, 1);
        assert_eq!(s.commands, ["git status"]);
        assert_eq!(s.tool_result_chars, 16);
        assert_eq!(
            s.tool_result_tokens,
            count_tokens("hello world") + count_tokens("hello")
        );
        assert_eq!(s.model_usage, r#"{"m":{}}"#);
        assert!((s.cost_usd - 0.01).abs() < 1e-12);
        assert_eq!(parse(r#"{"type":"assistant"}"#), None);
        // A result with no cost cannot be charged, so it counts as no result.
        assert_eq!(parse(r#"{"type":"result","result":"a.rs"}"#), None);
    }
}
