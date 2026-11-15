//! Counts the tokens a coding agent would read from each tool's version of a
//! command's output, over a fixed corpus of commands on pinned public
//! repositories. It measures command output only, not billed agent sessions:
//! see README.md for the method and its limits.

use anyhow::{Context, Result, bail, ensure};
use clap::Parser;
use serde::Deserialize;
use std::collections::HashSet;
use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Parser)]
#[command(about = "Count the tokens in each tool's output for a fixed command corpus")]
struct Args {
    /// The corpus: pinned repositories and the commands run in them.
    #[arg(long, default_value = "corpus.toml")]
    corpus: PathBuf,
    /// Where the repositories are cloned; reused between runs.
    #[arg(long)]
    work: PathBuf,
    /// A new directory for the outputs, results.csv and summary.md.
    #[arg(long)]
    out: PathBuf,
    /// An rtk binary, run as its hook rewrites each command: LABEL=PATH.
    #[arg(long = "rtk", value_name = "LABEL=PATH", value_parser = label_value)]
    rtk: Vec<(String, String)>,
    /// A tool that runs each command itself: LABEL=TEMPLATE, where {cmd} is
    /// the command as written and {qcmd} the command as one quoted shell word.
    #[arg(long = "wrap", value_name = "LABEL=TEMPLATE", value_parser = label_value)]
    wrap: Vec<(String, String)>,
    /// Chisle's hooks/chisle-compress-output.js, given each raw output:
    /// LABEL=PATH.
    #[arg(long, value_name = "LABEL=PATH", value_parser = label_value)]
    chisle: Option<(String, String)>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Corpus {
    repo: Vec<Repo>,
    command: Vec<CorpusCommand>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Repo {
    name: String,
    url: String,
    /// A full commit hash, so the corpus cannot drift with a moved tag.
    rev: String,
    /// Directories, relative to the checkout, put first on PATH.
    #[serde(default)]
    path: Vec<String>,
    /// Shell commands run once after checkout.
    #[serde(default)]
    setup: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CorpusCommand {
    repo: String,
    run: String,
    /// Run before each side's run, so every side starts from the files and
    /// build artefacts it resets.
    before: Option<String>,
    /// What an agent asks of this output; `keep` names the facts its answer needs.
    question: Option<String>,
    keep: Option<Keep>,
}

/// The facts an answer needs, all taken from the raw output: listed, or by a rule.
#[derive(Deserialize)]
#[serde(untagged)]
enum Keep {
    Rule(Rule),
    Facts(Vec<String>),
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum Rule {
    /// Every line.
    Lines,
    /// Every grep or rg match line's text, after its `path:N:` or `N:`.
    Matches,
    /// Every added or removed line of a patch, with its sign.
    Diff,
    /// The last path component of every line.
    Basenames,
}

enum Kind {
    Raw,
    Rtk(PathBuf),
    Wrap(String),
    Chisle(PathBuf),
}

struct Side {
    label: String,
    kind: Kind,
}

struct Outcome {
    text: String,
    exit: Option<i32>,
    /// The command an rtk hook ran instead, when it rewrote this one.
    rewritten: Option<String>,
}

struct Row {
    command: usize,
    side: usize,
    exit: Option<i32>,
    rewritten: Option<String>,
    bytes: usize,
    tokens: usize,
    /// The facts of the command's `keep` this output lacks, when it has one.
    lost: Option<Vec<String>>,
    /// Whether the output names a way to recover what it left out.
    recovery: bool,
}

fn label_value(arg: &str) -> Result<(String, String), String> {
    match arg.split_once('=') {
        Some((label, value)) if !label.trim().is_empty() && !value.is_empty() => {
            Ok((label.trim().to_string(), value.to_string()))
        }
        _ => Err(format!("expected LABEL=VALUE, got '{arg}'")),
    }
}

fn validate(corpus: &Corpus) -> Result<()> {
    let mut names = HashSet::new();
    for repo in &corpus.repo {
        ensure!(
            !repo.name.is_empty()
                && repo
                    .name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
                && !repo.name.starts_with('.'),
            "repository name '{}' must be letters, digits, '.', '_' or '-'",
            repo.name
        );
        ensure!(
            names.insert(repo.name.as_str()),
            "repository '{}' is listed twice",
            repo.name
        );
        ensure!(
            repo.rev.len() == 40 && repo.rev.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
            "repository '{}': rev must be a full lowercase commit hash, got '{}'",
            repo.name,
            repo.rev
        );
    }
    for command in &corpus.command {
        ensure!(
            names.contains(command.repo.as_str()),
            "command '{}' names unknown repository '{}'",
            command.run,
            command.repo
        );
        ensure!(
            command.question.is_some() == command.keep.is_some(),
            "command '{}': give question and keep together",
            command.run
        );
    }
    Ok(())
}

/// Whitespace runs as one space, so a fact survives re-indentation.
fn squeeze(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A grep or rg `-n` line's text: what follows `path:N:` or `N:`.
fn match_text(line: &str) -> &str {
    let mut rest = line;
    loop {
        let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        if digits > 0 && rest[digits..].starts_with(':') {
            return &rest[digits + 1..];
        }
        match rest.find(':') {
            Some(at) => rest = &rest[at + 1..],
            None => return line,
        }
    }
}

/// The distinct facts `keep` takes from the raw output, squeezed; each must be in it.
fn facts(keep: &Keep, raw: &str) -> Result<Vec<String>> {
    let lines = raw.lines().filter(|l| !l.trim().is_empty());
    let found: Vec<String> = match keep {
        Keep::Facts(list) => list.iter().map(|f| squeeze(f)).collect(),
        Keep::Rule(Rule::Lines) => lines.map(squeeze).collect(),
        Keep::Rule(Rule::Matches) => lines.map(|l| squeeze(match_text(l))).collect(),
        Keep::Rule(Rule::Diff) => lines
            .filter(|l| {
                (l.starts_with('+') || l.starts_with('-'))
                    && !l.starts_with("+++ ")
                    && !l.starts_with("--- ")
                    && !l[1..].trim().is_empty()
            })
            .map(squeeze)
            .collect(),
        Keep::Rule(Rule::Basenames) => lines
            .map(|l| squeeze(l.trim_end_matches('/').rsplit('/').next().unwrap_or(l)))
            .collect(),
    };
    let mut seen = HashSet::new();
    let found: Vec<String> = found
        .into_iter()
        .filter(|f| !f.is_empty() && seen.insert(f.clone()))
        .collect();
    let raw_lines: Vec<String> = raw.lines().map(squeeze).collect();
    for fact in &found {
        ensure!(
            raw_lines.iter().any(|l| l.contains(fact.as_str())),
            "fact '{fact}' is not in the raw output"
        );
    }
    Ok(found)
}

/// The `facts` no line of `text` holds, whitespace aside.
fn lost(facts: &[String], text: &str) -> Vec<String> {
    let lines: Vec<String> = text.lines().map(squeeze).collect();
    facts
        .iter()
        .filter(|f| !lines.iter().any(|l| l.contains(f.as_str())))
        .cloned()
        .collect()
}

/// Whether `text` tells the agent how to get back what it left out, in any of the measured
/// tools' words.
fn names_a_recovery(text: &str) -> bool {
    [
        "rtk recall ",
        "[full output: ",
        "[full diff: ",
        "tokenade expand-ref ",
        "Full patch: ",
    ]
    .iter()
    .any(|hint| text.contains(hint))
}

fn sides(args: &Args) -> Result<Vec<Side>> {
    let mut sides = vec![Side {
        label: "raw".into(),
        kind: Kind::Raw,
    }];
    for (label, path) in &args.rtk {
        let path = fs::canonicalize(path).with_context(|| format!("rtk binary {path}"))?;
        sides.push(Side {
            label: label.clone(),
            kind: Kind::Rtk(path),
        });
    }
    for (label, template) in &args.wrap {
        ensure!(
            template.contains("{cmd}") || template.contains("{qcmd}"),
            "--wrap {label}: the template needs {{cmd}} or {{qcmd}}"
        );
        sides.push(Side {
            label: label.clone(),
            kind: Kind::Wrap(template.clone()),
        });
    }
    if let Some((label, hook)) = &args.chisle {
        let hook = fs::canonicalize(hook).with_context(|| format!("Chisle hook {hook}"))?;
        sides.push(Side {
            label: label.clone(),
            kind: Kind::Chisle(hook),
        });
    }
    let mut slugs = HashSet::new();
    for side in &sides {
        ensure!(
            slugs.insert(slug(&side.label)),
            "two sides share the label '{}'",
            side.label
        );
    }
    Ok(sides)
}

/// A label as a file name.
fn slug(label: &str) -> String {
    label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

/// `text` as one single-quoted POSIX shell word.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

fn fill_template(template: &str, cmd: &str) -> String {
    template
        .replace("{qcmd}", &shell_quote(cmd))
        .replace("{cmd}", cmd)
}

/// What a hook does with `rtk rewrite`'s reply: exit 0 (allow) and exit 3
/// (ask) print the command to run instead; every other reply leaves it alone.
fn rewrite_reply(stdout: &str, exit: Option<i32>) -> Option<String> {
    let rewritten = stdout.trim();
    (matches!(exit, Some(0 | 3)) && !rewritten.is_empty()).then(|| rewritten.to_string())
}

/// Chisle's hook input for a Copilot CLI bash call. Its Copilot path follows
/// the default mode, so it compresses without the flag file Claude Code's
/// path reads; both paths share one compressor.
fn chisle_payload(raw: &str) -> String {
    serde_json::json!({
        "sessionId": "output-tokens",
        "toolName": "bash",
        "toolArgs": {},
        "toolResult": { "resultType": "success", "textResultForLlm": raw },
    })
    .to_string()
}

/// The text Chisle hands the agent: its replacement, or the raw output when it
/// replies with nothing.
fn chisle_reply(stdout: &str, raw: &str) -> Result<String> {
    if stdout.trim().is_empty() {
        return Ok(raw.to_string());
    }
    let reply: serde_json::Value = serde_json::from_str(stdout).context("Chisle's reply")?;
    match reply["modifiedResult"]["textResultForLlm"].as_str() {
        Some(text) => Ok(text.to_string()),
        None => bail!("Chisle replied without modifiedResult.textResultForLlm: {stdout}"),
    }
}

fn count_tokens(text: &str) -> usize {
    tiktoken_rs::o200k_base_singleton()
        .encode_ordinary(text)
        .len()
}

/// The environment every side's command runs in: a home of its own, so no
/// tool reads the user's configuration or another side's home, and PATH
/// with `first` ahead of the user's. The repositories and Cargo's build
/// artefacts are shared.
fn side_env(cmd: &mut Command, home: &Path, first: &[PathBuf]) -> Result<()> {
    let user_home = std::env::var_os("HOME").context("HOME is not set")?;
    for (var, dir) in [("CARGO_HOME", ".cargo"), ("RUSTUP_HOME", ".rustup")] {
        if std::env::var_os(var).is_none() {
            cmd.env(var, Path::new(&user_home).join(dir));
        }
    }
    let mut path: Vec<PathBuf> = first.to_vec();
    if let Some(user_path) = std::env::var_os("PATH") {
        path.extend(std::env::split_paths(&user_path));
    }
    cmd.env("HOME", home)
        .env("PATH", std::env::join_paths(path)?)
        .env("RTK_TELEMETRY_DISABLED", "1");
    for var in [
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
        "XDG_STATE_HOME",
    ] {
        cmd.env_remove(var);
    }
    Ok(())
}

/// Runs `script` with `sh`, stderr joined to stdout as an agent's shell tool
/// returns them, and no stdin.
fn run_shell(script: &str, dir: &Path, home: &Path, first: &[PathBuf]) -> Result<Outcome> {
    // nosemgrep: interpreter-execution -- a benchmark, not rtk: it runs each corpus command through sh, as an agent's shell tool does
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(format!("exec 2>&1\n{script}"))
        .current_dir(dir)
        .stdin(Stdio::null());
    side_env(&mut cmd, home, first)?;
    let output = cmd.output().with_context(|| format!("running {script}"))?;
    Ok(Outcome {
        text: String::from_utf8_lossy(&output.stdout).into_owned(),
        exit: output.status.code(),
        rewritten: None,
    })
}

/// Runs a setup or `before` command, which must succeed.
fn prepare(script: &str, dir: &Path, home: &Path, first: &[PathBuf]) -> Result<()> {
    let outcome = run_shell(script, dir, home, first)?;
    ensure!(
        outcome.exit == Some(0),
        "'{script}' failed in {}:\n{}",
        dir.display(),
        outcome.text
    );
    Ok(())
}

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .output()
        .context("running git")?;
    ensure!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Clones once, then puts the checkout back at `rev`, keeps ignored build
/// output so reruns stay fast, and runs the setup commands.
fn checkout(repo: &Repo, work: &Path, home: &Path) -> Result<PathBuf> {
    let dir = work.join(&repo.name);
    if !dir.exists() {
        git(work, &["clone", "--quiet", &repo.url, &repo.name])?;
    }
    git(
        &dir,
        &["checkout", "--quiet", "--force", "--detach", &repo.rev],
    )?;
    git(&dir, &["clean", "--quiet", "-fd"])?;
    let head = git(&dir, &["rev-parse", "HEAD"])?;
    ensure!(
        head == repo.rev,
        "{} is at {head}, not {}",
        repo.name,
        repo.rev
    );
    let first = repo_path(&dir, repo);
    for script in &repo.setup {
        prepare(script, &dir, home, &first)?;
    }
    Ok(dir)
}

fn repo_path(dir: &Path, repo: &Repo) -> Vec<PathBuf> {
    repo.path.iter().map(|p| dir.join(p)).collect()
}

#[cfg(unix)]
fn link_shim(bin: &Path, shim_dir: &Path) -> Result<()> {
    fs::create_dir_all(shim_dir)?;
    std::os::unix::fs::symlink(bin, shim_dir.join("rtk")).context("linking the rtk shim")
}

#[cfg(not(unix))]
fn link_shim(_bin: &Path, _shim_dir: &Path) -> Result<()> {
    bail!("output-tokens needs a Unix shell")
}

struct Layout {
    out: PathBuf,
}

impl Layout {
    fn home(&self, side: &Side) -> PathBuf {
        self.out.join("homes").join(slug(&side.label))
    }
    fn shim(&self, side: &Side) -> PathBuf {
        self.out.join("shims").join(slug(&side.label))
    }
}

fn run_side(
    side: &Side,
    layout: &Layout,
    command: &CorpusCommand,
    dir: &Path,
    repo_first: &[PathBuf],
    raw: &Outcome,
) -> Result<Outcome> {
    let home = layout.home(side);
    let before = |first: &[PathBuf]| match &command.before {
        Some(script) => prepare(script, dir, &home, first),
        None => Ok(()),
    };
    match &side.kind {
        Kind::Raw => unreachable!("the raw side runs first"),
        Kind::Rtk(bin) => {
            let mut first = vec![layout.shim(side)];
            first.extend_from_slice(repo_first);
            // nosemgrep: dynamic-command-execution -- a benchmark, not rtk: asks the rtk build under test (--rtk) for its rewrite
            let mut ask = Command::new(bin);
            ask.args(["rewrite", &command.run])
                .current_dir(dir)
                .stdin(Stdio::null());
            side_env(&mut ask, &home, &first)?;
            let reply = ask.output().context("running rtk rewrite")?;
            let stdout = String::from_utf8_lossy(&reply.stdout);
            let Some(rewritten) = rewrite_reply(&stdout, reply.status.code()) else {
                return Ok(Outcome {
                    text: raw.text.clone(),
                    exit: raw.exit,
                    rewritten: None,
                });
            };
            before(&first)?;
            let mut outcome = run_shell(&rewritten, dir, &home, &first)?;
            outcome.rewritten = Some(rewritten);
            Ok(outcome)
        }
        Kind::Wrap(template) => {
            before(repo_first)?;
            run_shell(
                &fill_template(template, &command.run),
                dir,
                &home,
                repo_first,
            )
        }
        Kind::Chisle(hook) => {
            // nosemgrep: interpreter-execution -- a benchmark, not rtk: runs Chisle's hook (--chisle), which is JavaScript
            let mut node = Command::new("node");
            node.arg(hook)
                .current_dir(dir)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped());
            side_env(&mut node, &home, repo_first)?;
            let mut child = node.spawn().context("starting node for Chisle")?;
            child
                .stdin
                .take()
                .context("Chisle's stdin")?
                .write_all(chisle_payload(&raw.text).as_bytes())?;
            let reply = child.wait_with_output()?;
            ensure!(reply.status.success(), "Chisle's hook failed");
            Ok(Outcome {
                text: chisle_reply(&String::from_utf8_lossy(&reply.stdout), &raw.text)?,
                exit: raw.exit,
                rewritten: None,
            })
        }
    }
}

fn csv_field(field: &str) -> String {
    if field.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

fn exit_text(exit: Option<i32>) -> String {
    exit.map_or_else(|| "signal".to_string(), |code| code.to_string())
}

fn results_csv(rows: &[Row], corpus: &Corpus, sides: &[Side]) -> String {
    let mut csv = String::from("n,repo,command,side,rewritten_to,exit,bytes,tokens\n");
    for row in rows {
        let command = &corpus.command[row.command];
        let fields = [
            (row.command + 1).to_string(),
            command.repo.clone(),
            command.run.clone(),
            sides[row.side].label.clone(),
            row.rewritten.clone().unwrap_or_default(),
            exit_text(row.exit),
            row.bytes.to_string(),
            row.tokens.to_string(),
        ];
        let line: Vec<String> = fields.iter().map(|f| csv_field(f)).collect();
        csv.push_str(&line.join(","));
        csv.push('\n');
    }
    csv
}

fn percent_saved(raw: usize, side: usize) -> f64 {
    if raw == 0 {
        0.0
    } else {
        (raw as f64 - side as f64) * 100.0 / raw as f64
    }
}

fn markdown_cell(text: &str) -> String {
    text.replace('|', "\\|").replace('`', "'")
}

fn summary_md(rows: &[Row], corpus: &Corpus, sides: &[Side], versions: &[String]) -> String {
    let cell = |command: usize, side: usize| {
        rows.iter()
            .find(|r| r.command == command && r.side == side)
            .expect("every command has a row per side")
    };
    let mut md = String::from("Tokens are o200k_base counts of stdout and stderr together.\n\n");
    for (side, version) in sides.iter().zip(versions) {
        if !version.is_empty() {
            let _ = writeln!(md, "- {}: `{}`", side.label, markdown_cell(version));
        }
    }
    md.push_str("\n| # | Repository | Command |");
    for side in sides {
        let _ = write!(md, " {} |", markdown_cell(&side.label));
    }
    md.push_str("\n| ---: | --- | --- |");
    md.push_str(&" ---: |".repeat(sides.len()));
    md.push('\n');
    for (n, command) in corpus.command.iter().enumerate() {
        let _ = write!(
            md,
            "| {} | {} | `{}` |",
            n + 1,
            command.repo,
            markdown_cell(&command.run)
        );
        for side in 0..sides.len() {
            let _ = write!(md, " {} |", cell(n, side).tokens);
        }
        md.push('\n');
    }
    md.push_str(
        "\n| Side | Tokens | Saved against raw | Larger than raw | Exit code differs from raw | Rewritten by its hook |\n",
    );
    md.push_str("| --- | ---: | ---: | ---: | ---: | ---: |\n");
    let total = |side: usize| {
        rows.iter()
            .filter(|r| r.side == side)
            .map(|r| r.tokens)
            .sum()
    };
    let raw_total: usize = total(0);
    for (index, side) in sides.iter().enumerate() {
        let side_rows: Vec<&Row> = rows.iter().filter(|r| r.side == index).collect();
        let larger = side_rows
            .iter()
            .filter(|r| r.tokens > cell(r.command, 0).tokens)
            .count();
        let exit_differs = side_rows
            .iter()
            .filter(|r| r.exit != cell(r.command, 0).exit)
            .count();
        let rewritten = match side.kind {
            Kind::Rtk(_) => side_rows
                .iter()
                .filter(|r| r.rewritten.is_some())
                .count()
                .to_string(),
            _ => "n/a".into(),
        };
        let _ = writeln!(
            md,
            "| {} | {} | {:.1}% | {} | {} | {} |",
            markdown_cell(&side.label),
            total(index),
            percent_saved(raw_total, total(index)),
            larger,
            exit_differs,
            rewritten
        );
    }
    md
}

/// What each side kept of the facts each command's answer needs: a table per side, then per
/// command with the first facts each side lost.
fn facts_md(rows: &[Row], corpus: &Corpus, sides: &[Side], facts_of: &[usize]) -> String {
    let mut md = String::from(
        "No model is asked. Each command's answer needs facts taken from its raw output \
         (`keep` in corpus.toml), and an output keeps a fact when one of its lines holds it, \
         whitespace aside. A fact an output reworded counts as lost, so read the lost facts \
         before reading a loss as a defect.\n\n\
         | Side | Facts kept | Commands that lost a fact | Of those, naming a way to recover |\n\
         | --- | ---: | ---: | ---: |\n",
    );
    let total: usize = corpus
        .command
        .iter()
        .zip(facts_of)
        .filter(|(c, _)| c.keep.is_some())
        .map(|(_, n)| n)
        .sum();
    for (index, side) in sides.iter().enumerate() {
        let losing: Vec<&Row> = rows
            .iter()
            .filter(|r| r.side == index && r.lost.as_ref().is_some_and(|l| !l.is_empty()))
            .collect();
        let lost: usize = losing
            .iter()
            .filter_map(|r| r.lost.as_ref())
            .map(Vec::len)
            .sum();
        let _ = writeln!(
            md,
            "| {} | {} of {total} | {} | {} |",
            markdown_cell(&side.label),
            total - lost,
            losing.len(),
            losing.iter().filter(|r| r.recovery).count()
        );
    }
    for (n, command) in corpus.command.iter().enumerate() {
        let Some(question) = &command.question else {
            continue;
        };
        let _ = write!(
            md,
            "\n## {}. `{}` ({})\n\n{} {} facts.\n\n\
             | Side | Kept | Names a way to recover | First facts lost |\n| --- | ---: | --- | --- |\n",
            n + 1,
            markdown_cell(&command.run),
            command.repo,
            question,
            facts_of[n]
        );
        for row in rows.iter().filter(|r| r.command == n) {
            let lost = row.lost.as_deref().unwrap_or_default();
            let first: Vec<String> = lost
                .iter()
                .take(3)
                .map(|f| {
                    format!(
                        "`{}`",
                        markdown_cell(&f.chars().take(80).collect::<String>())
                    )
                })
                .collect();
            let _ = writeln!(
                md,
                "| {} | {} | {} | {} |",
                markdown_cell(&sides[row.side].label),
                facts_of[n] - lost.len(),
                if row.recovery { "yes" } else { "no" },
                first.join(", ")
            );
        }
    }
    md
}

fn version_of(side: &Side) -> String {
    let Kind::Rtk(bin) = &side.kind else {
        return String::new();
    };
    // nosemgrep: dynamic-command-execution -- a benchmark, not rtk: reads the --version of the rtk build under test
    Command::new(bin)
        .arg("--version")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

fn main() -> Result<()> {
    let args = Args::parse();
    let text = fs::read_to_string(&args.corpus)
        .with_context(|| format!("reading {}", args.corpus.display()))?;
    let corpus: Corpus = toml::from_str(&text).context("parsing the corpus")?;
    validate(&corpus)?;
    let sides = sides(&args)?;
    ensure!(
        !args.out.exists(),
        "{} already exists; give a new directory",
        args.out.display()
    );
    fs::create_dir_all(args.out.join("outputs"))?;
    fs::create_dir_all(&args.work)?;
    let work = fs::canonicalize(&args.work)?;
    let layout = Layout {
        out: fs::canonicalize(&args.out)?,
    };
    for side in &sides {
        fs::create_dir_all(layout.home(side))?;
        if let Kind::Rtk(bin) = &side.kind {
            link_shim(bin, &layout.shim(side))?;
        }
    }
    let versions: Vec<String> = sides.iter().map(version_of).collect();
    let setup_home = layout.out.join("homes").join("setup");
    fs::create_dir_all(&setup_home)?;
    let mut dirs = Vec::new();
    for repo in &corpus.repo {
        eprintln!("checking out {} at {}", repo.name, repo.rev);
        dirs.push(checkout(repo, &work, &setup_home)?);
    }

    let mut rows = Vec::new();
    // Per command, how many facts its `keep` names.
    let mut facts_of = Vec::new();
    for (n, command) in corpus.command.iter().enumerate() {
        let index = corpus
            .repo
            .iter()
            .position(|r| r.name == command.repo)
            .expect("validated");
        let (repo, dir) = (&corpus.repo[index], &dirs[index]);
        let first = repo_path(dir, repo);
        eprintln!(
            "[{}/{}] {}: {}",
            n + 1,
            corpus.command.len(),
            repo.name,
            command.run
        );
        let raw_home = layout.home(&sides[0]);
        if let Some(script) = &command.before {
            prepare(script, dir, &raw_home, &first)?;
        }
        let raw = run_shell(&command.run, dir, &raw_home, &first)?;
        let mut outcomes = Vec::with_capacity(sides.len());
        for side in &sides[1..] {
            outcomes.push(run_side(side, &layout, command, dir, &first, &raw)?);
        }
        let wanted = match &command.keep {
            Some(keep) => facts(keep, &raw.text).with_context(|| command.run.clone())?,
            None => Vec::new(),
        };
        outcomes.insert(0, raw);
        for (index, outcome) in outcomes.into_iter().enumerate() {
            let file = format!("{:03}-{}.txt", n + 1, slug(&sides[index].label));
            fs::write(layout.out.join("outputs").join(file), &outcome.text)?;
            rows.push(Row {
                command: n,
                side: index,
                exit: outcome.exit,
                rewritten: outcome.rewritten,
                bytes: outcome.text.len(),
                tokens: count_tokens(&outcome.text),
                lost: command.keep.as_ref().map(|_| lost(&wanted, &outcome.text)),
                recovery: names_a_recovery(&outcome.text),
            });
        }
        facts_of.push(wanted.len());
    }
    fs::write(
        layout.out.join("facts.md"),
        facts_md(&rows, &corpus, &sides, &facts_of),
    )?;
    fs::write(
        layout.out.join("results.csv"),
        results_csv(&rows, &corpus, &sides),
    )?;
    let summary = summary_md(&rows, &corpus, &sides, &versions);
    fs::write(layout.out.join("summary.md"), &summary)?;
    print!("{summary}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corpus(rev: &str, command_repo: &str) -> Corpus {
        toml::from_str(&format!(
            "[[repo]]\nname = \"r\"\nurl = \"u\"\nrev = \"{rev}\"\n\
             [[command]]\nrepo = \"{command_repo}\"\nrun = \"git log\"\n"
        ))
        .unwrap()
    }

    const REV: &str = "4649aa9700619f94cf9c66876e9549d83420e16c";

    #[test]
    fn a_corpus_pins_full_commits_and_known_repositories() {
        assert!(validate(&corpus(REV, "r")).is_ok());
        assert!(validate(&corpus("14.1.1", "r")).is_err());
        assert!(validate(&corpus(&REV.to_uppercase(), "r")).is_err());
        assert!(validate(&corpus(REV, "other")).is_err());
    }

    #[test]
    fn a_question_comes_with_its_facts() {
        let with = |extra: &str| -> Corpus {
            toml::from_str(&format!(
                "[[repo]]\nname = \"r\"\nurl = \"u\"\nrev = \"{REV}\"\n\
                 [[command]]\nrepo = \"r\"\nrun = \"git log\"\n{extra}"
            ))
            .unwrap()
        };
        assert!(validate(&with("question = \"q\"\nkeep = \"lines\"\n")).is_ok());
        assert!(validate(&with("question = \"q\"\nkeep = [\"a\"]\n")).is_ok());
        assert!(validate(&with("question = \"q\"\n")).is_err());
        assert!(validate(&with("keep = \"diff\"\n")).is_err());
        assert!(
            toml::from_str::<Corpus>(&format!(
                "[[repo]]\nname = \"r\"\nurl = \"u\"\nrev = \"{REV}\"\n\
             [[command]]\nrepo = \"r\"\nrun = \"x\"\nquestion = \"q\"\nkeep = \"words\"\n"
            ))
            .is_err()
        );
    }

    #[test]
    fn facts_come_from_the_raw_output_by_rule_or_list() {
        let grep = "src/a.rs:12:    let x = 1;\nsrc/b.rs:3:fn f() {}\n4:solo: line\n";
        assert_eq!(
            facts(&Keep::Rule(Rule::Matches), grep).unwrap(),
            ["let x = 1;", "fn f() {}", "solo: line"]
        );
        let diff = "--- a/f\n+++ b/f\n@@ -1 +1 @@\n-old  line\n+new line\n+\n+new line\n";
        assert_eq!(
            facts(&Keep::Rule(Rule::Diff), diff).unwrap(),
            ["-old line", "+new line"]
        );
        assert_eq!(
            facts(&Keep::Rule(Rule::Basenames), "a/b/c.rs\nd/c.rs\ne/\n").unwrap(),
            ["c.rs", "e"]
        );
        assert_eq!(
            facts(&Keep::Rule(Rule::Lines), "x  y\n\nz\n").unwrap(),
            ["x y", "z"]
        );
        assert_eq!(
            facts(&Keep::Facts(vec!["b  c".into()]), "a b c d\n").unwrap(),
            ["b c"]
        );
        assert!(facts(&Keep::Facts(vec!["absent".into()]), "a b c\n").is_err());
    }

    #[test]
    fn a_fact_is_kept_on_one_line_whatever_its_spacing() {
        let facts = ["let x = 1;".to_string(), "fn f() {}".to_string()];
        assert!(lost(&facts, "12:let   x = 1;\n  fn f() {}\n").is_empty());
        assert_eq!(lost(&facts, "let x =\n1;\nfn f() {}\n"), ["let x = 1;"]);
    }

    #[test]
    fn each_tools_recovery_hint_is_recognised() {
        assert!(names_a_recovery(
            "[full output: rtk recall 4ac7b03c95a09e7f]"
        ));
        assert!(names_a_recovery("+11 hidden: rtk recall 995969b07316e2e1]"));
        assert!(names_a_recovery(
            "[full output: /home/u/.local/share/snip/tee/1-cargo.log]"
        ));
        assert!(names_a_recovery(
            "recover with `tokenade expand-ref 704eb3a4aef9`"
        ));
        assert!(names_a_recovery(
            "diffstat only. Full patch: git show -p <rev>"
        ));
        assert!(!names_a_recovery("... more matches"));
    }

    #[test]
    fn a_label_value_needs_both_halves() {
        assert_eq!(
            label_value("rtk 1.2.0=/bin/rtk=x"),
            Ok(("rtk 1.2.0".into(), "/bin/rtk=x".into()))
        );
        assert!(label_value("/bin/rtk").is_err());
        assert!(label_value("=/bin/rtk").is_err());
        assert!(label_value("rtk=").is_err());
    }

    #[test]
    fn templates_take_the_command_raw_or_quoted() {
        let cmd = "grep -n 'a b' f";
        assert_eq!(fill_template("snip {cmd}", cmd), "snip grep -n 'a b' f");
        let quoted = fill_template("tool wrap {qcmd}", cmd);
        assert_eq!(quoted, r"tool wrap 'grep -n '\''a b'\'' f'");
        // nosemgrep: interpreter-execution -- test: checks that sh reads a quoted template back as one word
        let echoed = Command::new("sh")
            .arg("-c")
            .arg(fill_template("printf %s {qcmd}", cmd))
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&echoed.stdout), cmd);
    }

    #[test]
    fn only_allow_and_ask_replies_rewrite() {
        assert_eq!(
            rewrite_reply("rtk git status", Some(0)),
            Some("rtk git status".into())
        );
        assert_eq!(
            rewrite_reply("rtk git status\n", Some(3)),
            Some("rtk git status".into())
        );
        assert_eq!(rewrite_reply("", Some(1)), None);
        assert_eq!(rewrite_reply("", Some(3)), None);
        assert_eq!(rewrite_reply("rtk git push", Some(2)), None);
        assert_eq!(rewrite_reply("rtk git status", None), None);
    }

    #[test]
    fn chisle_gets_the_raw_text_and_its_silence_keeps_it() {
        let payload: serde_json::Value = serde_json::from_str(&chisle_payload("a\"b\n")).unwrap();
        assert_eq!(payload["toolName"], "bash");
        assert_eq!(payload["toolResult"]["textResultForLlm"], "a\"b\n");
        assert_eq!(chisle_reply("", "raw").unwrap(), "raw");
        let reply = r#"{"modifiedResult":{"resultType":"success","textResultForLlm":"cut"}}"#;
        assert_eq!(chisle_reply(reply, "raw").unwrap(), "cut");
        assert!(chisle_reply(r#"{"other":1}"#, "raw").is_err());
        assert!(chisle_reply("not json", "raw").is_err());
    }

    #[test]
    fn tokens_are_counted_offline_with_o200k_base() {
        assert_eq!(count_tokens(""), 0);
        assert_eq!(count_tokens("hello world"), 2);
        // Special-token text in command output is counted as ordinary text.
        assert!(count_tokens("<|endoftext|>") > 1);
    }

    #[test]
    fn savings_are_against_the_raw_total() {
        assert_eq!(percent_saved(200, 50), 75.0);
        assert_eq!(percent_saved(100, 150), -50.0);
        assert_eq!(percent_saved(0, 0), 0.0);
    }

    #[test]
    fn csv_fields_with_separators_are_quoted() {
        assert_eq!(csv_field("git log"), "git log");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
    }

    #[test]
    fn labels_become_file_names() {
        assert_eq!(slug("rtk 1.2.0 (oss)"), "rtk-1-2-0--oss-");
    }
}
