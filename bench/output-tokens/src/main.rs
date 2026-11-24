//! Counts the tokens a coding agent would read from each tool's version of a
//! command's output, over a fixed corpus of commands on pinned public
//! repositories. It measures command output only, not billed agent sessions:
//! see README.md for the method and its limits.

use anyhow::{Context, Result, bail, ensure};
use clap::Parser;
use regex::Regex;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

mod fact;
use fact::{Corpus, CorpusCommand, Keep, Repo, Rule, copies, count_tokens, label_value, slug};

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
    /// A Claude Code plugin whose Bash hooks are driven as Claude Code drives
    /// them: LABEL=DIR, where DIR holds .claude-plugin/plugin.json.
    #[arg(long = "claude-plugin", value_name = "LABEL=DIR", value_parser = label_value)]
    claude_plugin: Vec<(String, String)>,
}

enum Kind {
    Raw,
    Rtk(PathBuf),
    Wrap(String),
    Chisle(PathBuf),
    Plugin(Plugin),
}

/// The hooks a Claude Code plugin runs around a Bash call, from its hooks file.
struct Plugin {
    root: PathBuf,
    session_start: Vec<Hook>,
    pre: Vec<Hook>,
    post: Vec<Hook>,
    failure: Vec<Hook>,
}

/// A command hook: exec form when it has `args`, else a shell command.
struct Hook {
    command: String,
    args: Option<Vec<String>>,
    timeout: Duration,
}

#[derive(Deserialize)]
struct HooksFile {
    hooks: HashMap<String, Vec<HookGroup>>,
}

#[derive(Deserialize)]
struct HookGroup {
    matcher: Option<String>,
    hooks: Vec<HookEntry>,
}

#[derive(Deserialize)]
struct HookEntry {
    #[serde(rename = "type")]
    kind: String,
    command: Option<String>,
    args: Option<Vec<String>>,
    /// Seconds.
    timeout: Option<f64>,
}

/// What one hook run returned; `exit` is None when it timed out or was killed.
struct HookRun {
    exit: Option<i32>,
    stdout: String,
    stderr: String,
}

/// What hook replies give the agent or change, as Claude Code reads them.
#[derive(Default)]
struct Reply {
    /// Text added beside the tool result.
    context: Vec<String>,
    /// Why the command was refused; the agent reads this instead of output.
    deny: Option<String>,
    /// A command to run instead (`updatedInput.command`).
    command: Option<String>,
    /// Output to show instead (`updatedToolOutput`).
    output: Option<String>,
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

/// The facts `keep` takes from the raw output, squeezed, one for each time a rule finds it;
/// each must be in the raw output, and a fact needs a letter or digit.
fn facts(keep: &Keep, raw: &str) -> Result<Vec<String>> {
    let lines = raw.lines().filter(|l| !l.trim().is_empty());
    let found: Vec<String> = match keep {
        Keep::Facts(list) => {
            let mut seen = HashSet::new();
            list.iter()
                .map(|f| squeeze(f))
                .filter(|f| seen.insert(f.clone()))
                .collect()
        }
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
    let found: Vec<String> = found
        .into_iter()
        .filter(|f| f.chars().any(char::is_alphanumeric))
        .collect();
    let missing = lost(&found, raw);
    ensure!(
        missing.is_empty(),
        "fact '{}' is not in the raw output",
        missing[0]
    );
    Ok(found)
}

/// The `facts` that `text` does not hold, whitespace aside: a fact listed n times must stand
/// whole n times across the lines.
fn lost(facts: &[String], text: &str) -> Vec<String> {
    let lines: Vec<String> = text.lines().map(squeeze).collect();
    let mut left: HashMap<&str, usize> = HashMap::new();
    let mut missing = Vec::new();
    for fact in facts {
        let held = left
            .entry(fact)
            .or_insert_with(|| lines.iter().map(|l| copies(l, fact)).sum());
        match held.checked_sub(1) {
            Some(rest) => *held = rest,
            None => missing.push(fact.clone()),
        }
    }
    missing
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
        "ctx_search(",
        "full output: Read ",
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
    for (label, dir) in &args.claude_plugin {
        let root = fs::canonicalize(dir).with_context(|| format!("Claude Code plugin {dir}"))?;
        let plugin = load_plugin(root).with_context(|| format!("--claude-plugin {label}"))?;
        sides.push(Side {
            label: label.clone(),
            kind: Kind::Plugin(plugin),
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

#[derive(Clone, Copy)]
enum Event {
    SessionStart,
    PreToolUse,
    PostToolUse,
    PostToolUseFailure,
}

impl Event {
    fn name(self) -> &'static str {
        match self {
            Event::SessionStart => "SessionStart",
            Event::PreToolUse => "PreToolUse",
            Event::PostToolUse => "PostToolUse",
            Event::PostToolUseFailure => "PostToolUseFailure",
        }
    }
}

/// Whether a hook group's matcher selects `value`, by Claude Code's rules: none, "" or "*"
/// selects everything; only letters, digits, `_` and `|` lists exact values; anything else
/// is a regular expression (Claude Code's are JavaScript's; this reads them with `regex`).
fn matcher_selects(matcher: Option<&str>, value: &str) -> Result<bool> {
    match matcher {
        None | Some("" | "*") => Ok(true),
        Some(m)
            if m.chars()
                .all(|c| c.is_ascii_alphanumeric() || "_|".contains(c)) =>
        {
            Ok(m.split('|').any(|v| v == value))
        }
        Some(m) => Ok(Regex::new(m)
            .with_context(|| format!("matcher '{m}'"))?
            .is_match(value)),
    }
}

/// Reads the hooks file a plugin's manifest names in `hooks`, or else its
/// `hooks/hooks.json`, and keeps the hooks Claude Code runs at session start
/// and around a Bash call.
fn load_plugin(root: PathBuf) -> Result<Plugin> {
    let manifest = root.join(".claude-plugin/plugin.json");
    let manifest: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(&manifest)
            .with_context(|| format!("reading {}", manifest.display()))?,
    )
    .context("parsing .claude-plugin/plugin.json")?;
    let file = match &manifest["hooks"] {
        serde_json::Value::Null => root.join("hooks/hooks.json"),
        serde_json::Value::String(path) => root.join(path),
        other => bail!("plugin.json 'hooks' must be a path in the plugin, got {other}"),
    };
    let text = fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
    let hooks: HooksFile =
        serde_json::from_str(&text).with_context(|| format!("parsing {}", file.display()))?;
    let pick = |event: Event, value: &str| -> Result<Vec<Hook>> {
        let mut picked = Vec::new();
        for group in hooks.hooks.get(event.name()).into_iter().flatten() {
            if !matcher_selects(group.matcher.as_deref(), value)? {
                continue;
            }
            for entry in &group.hooks {
                let event = event.name();
                ensure!(
                    entry.kind == "command",
                    "{event}: only command hooks can run here, not '{}'",
                    entry.kind
                );
                picked.push(Hook {
                    command: entry
                        .command
                        .clone()
                        .with_context(|| format!("{event}: a command hook with no command"))?,
                    args: entry.args.clone(),
                    // Claude Code's default for a command hook.
                    timeout: Duration::try_from_secs_f64(entry.timeout.unwrap_or(600.0))
                        .with_context(|| format!("{event}: timeout {:?}", entry.timeout))?,
                });
            }
        }
        Ok(picked)
    };
    Ok(Plugin {
        session_start: pick(Event::SessionStart, "startup")?,
        pre: pick(Event::PreToolUse, "Bash")?,
        post: pick(Event::PostToolUse, "Bash")?,
        failure: pick(Event::PostToolUseFailure, "Bash")?,
        root,
    })
}

/// Bash's output shape in `updatedToolOutput` (string `stdout` and `stderr`, boolean
/// `interrupted`, `isImage` absent or boolean), as the agent would read it; Claude Code
/// ignores a value of any other shape.
fn bash_output(value: &serde_json::Value) -> Option<String> {
    let stdout = value["stdout"].as_str()?;
    let stderr = value["stderr"].as_str()?;
    value["interrupted"].as_bool()?;
    if !value
        .get("isImage")
        .is_none_or(serde_json::Value::is_boolean)
    {
        return None;
    }
    Some(if stderr.is_empty() {
        stdout.to_string()
    } else {
        format!("{stdout}\n{stderr}")
    })
}

/// What Claude Code takes from one hook run. Exit 0 gives the JSON reply's fields (for
/// SessionStart, plain stdout too); exit 2 gives stderr to the agent, refusing the call when
/// it comes before it (not at session start, where only the user sees it); any other exit,
/// or a timeout, reaches only the user.
fn read_reply(event: Event, run: &HookRun) -> Reply {
    let mut reply = Reply::default();
    match (run.exit, event) {
        (Some(2), Event::PreToolUse) => reply.deny = Some(run.stderr.clone()),
        (Some(2), Event::PostToolUse | Event::PostToolUseFailure) => {
            reply.context.push(run.stderr.clone());
        }
        (Some(0), _) => {
            let text = run.stdout.trim();
            let json = match serde_json::from_str::<serde_json::Value>(text) {
                Ok(json) if json.is_object() => json,
                _ => {
                    if matches!(event, Event::SessionStart) && !text.is_empty() {
                        reply.context.push(text.to_string());
                    }
                    return reply;
                }
            };
            let specific = &json["hookSpecificOutput"];
            if let Some(extra) = specific["additionalContext"].as_str() {
                reply.context.push(extra.to_string());
            }
            match event {
                Event::PreToolUse if specific["permissionDecision"] == "deny" => {
                    let reason = specific["permissionDecisionReason"].as_str();
                    reply.deny = Some(reason.unwrap_or_default().to_string());
                }
                Event::PreToolUse => {
                    reply.command = specific["updatedInput"]["command"]
                        .as_str()
                        .map(str::to_string);
                }
                Event::PostToolUse => {
                    reply.output = bash_output(&specific["updatedToolOutput"]);
                    if json["decision"] == "block" {
                        let reason = json["reason"].as_str();
                        reply.context.push(reason.unwrap_or_default().to_string());
                    }
                }
                _ => {}
            }
        }
        _ => {}
    }
    reply
}

/// Reads a pipe to its end on a thread of its own, so a full pipe cannot stall the child.
fn drain(mut pipe: impl std::io::Read + Send + 'static) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = pipe.read_to_end(&mut bytes);
        String::from_utf8_lossy(&bytes).into_owned()
    })
}

/// A tool result with each piece of hook context after it, on a line of its own.
fn with_context(mut text: String, context: &[String]) -> String {
    for extra in context {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(extra);
    }
    text
}

/// One plugin side's session: where its hooks run, and the log of every run.
struct HookSession<'a> {
    plugin: &'a Plugin,
    id: String,
    home: &'a Path,
    dir: &'a Path,
    first: &'a [PathBuf],
    log: PathBuf,
}

impl HookSession<'_> {
    /// Runs `hooks` for `event` in turn, with `fields` added to the common input, and
    /// merges their replies; a refusal wins, as in Claude Code.
    fn fire(
        &self,
        n: usize,
        event: Event,
        hooks: &[Hook],
        fields: serde_json::Value,
    ) -> Result<Reply> {
        let mut input = serde_json::json!({
            "session_id": self.id,
            "transcript_path": self.home.join("transcript.jsonl"),
            "cwd": self.dir,
            "permission_mode": "default",
            "hook_event_name": event.name(),
        });
        if let (Some(input), serde_json::Value::Object(fields)) = (input.as_object_mut(), fields) {
            input.extend(fields);
        }
        let mut merged = Reply::default();
        for hook in hooks {
            let run = self.run(hook, &input)?;
            let mut log = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.log)?;
            writeln!(
                log,
                "{}",
                serde_json::json!({
                    "command": n, "event": event.name(), "hook": hook.command,
                    "exit": run.exit, "stdout": run.stdout, "stderr": run.stderr,
                })
            )?;
            let reply = read_reply(event, &run);
            merged.context.extend(reply.context);
            merged.deny = merged.deny.or(reply.deny);
            for (kept, new, what) in [
                (&mut merged.command, reply.command, "rewrote the command"),
                (&mut merged.output, reply.output, "replaced the output"),
            ] {
                if new.is_some() {
                    ensure!(kept.is_none(), "{}: 2 hooks {what}", event.name());
                    *kept = new;
                }
            }
        }
        Ok(merged)
    }

    /// Runs a hook as Claude Code does: the input as JSON on stdin, in the session's
    /// directory, with the plugin's placeholders filled in; one still running at its
    /// timeout is killed and its reply dropped.
    fn run(&self, hook: &Hook, input: &serde_json::Value) -> Result<HookRun> {
        let data = self.home.join(".claude/plugins/data").join(&self.id);
        fs::create_dir_all(&data)?;
        let vars = [
            ("CLAUDE_PLUGIN_ROOT", self.plugin.root.as_path()),
            ("CLAUDE_PLUGIN_DATA", data.as_path()),
            ("CLAUDE_PROJECT_DIR", self.dir),
        ];
        let fill = |text: &str| {
            vars.iter().fold(text.to_string(), |text, (var, value)| {
                text.replace(&format!("${{{var}}}"), &value.to_string_lossy())
            })
        };
        let mut cmd = match &hook.args {
            Some(args) => {
                // nosemgrep: dynamic-command-execution -- a benchmark, not rtk: runs a hook of the plugin under test (--claude-plugin)
                let mut cmd = Command::new(fill(&hook.command));
                cmd.args(args.iter().map(|arg| fill(arg)));
                cmd
            }
            None => {
                // nosemgrep: interpreter-execution -- a benchmark, not rtk: runs a shell-form hook of the plugin under test (--claude-plugin)
                let mut cmd = Command::new("sh");
                cmd.arg("-c").arg(fill(&hook.command));
                cmd
            }
        };
        cmd.current_dir(self.dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        side_env(&mut cmd, self.home, self.first)?;
        for (var, value) in vars {
            cmd.env(var, value);
        }
        let mut child = cmd
            .spawn()
            .with_context(|| format!("starting hook {}", hook.command))?;
        let mut stdin = child.stdin.take().context("hook stdin")?;
        let text = input.to_string();
        // A hook may exit without reading its input, so a failed write is no error.
        let writer = std::thread::spawn(move || stdin.write_all(text.as_bytes()));
        let stdout = drain(child.stdout.take().context("hook stdout")?);
        let stderr = drain(child.stderr.take().context("hook stderr")?);
        let deadline = Instant::now() + hook.timeout;
        loop {
            if let Some(status) = child.try_wait()? {
                let _ = writer.join();
                return Ok(HookRun {
                    exit: status.code(),
                    stdout: stdout.join().unwrap_or_default(),
                    stderr: stderr.join().unwrap_or_default(),
                });
            }
            if Instant::now() >= deadline {
                child.kill()?;
                child.wait()?;
                eprintln!("hook {} timed out after {:?}", hook.command, hook.timeout);
                return Ok(HookRun {
                    exit: None,
                    stdout: String::new(),
                    stderr: format!("timed out after {:?}", hook.timeout),
                });
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

/// The environment every side's command runs in: a home and a temporary
/// directory of its own, so no tool reads configuration or a cache from the
/// user's home, another side's or an earlier run's, and PATH with `first` ahead
/// of the user's. The repositories and Cargo's build artefacts are shared.
fn side_env(cmd: &mut Command, home: &Path, first: &[PathBuf]) -> Result<()> {
    let tmp = home.join("tmp");
    fs::create_dir_all(&tmp)?;
    cmd.env("TMPDIR", tmp);
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
    /// A plugin side's session, in `dir` with `first` ahead on PATH.
    fn session<'a>(
        &self,
        side: &Side,
        plugin: &'a Plugin,
        home: &'a Path,
        dir: &'a Path,
        first: &'a [PathBuf],
    ) -> HookSession<'a> {
        HookSession {
            plugin,
            id: format!("output-tokens-{}", slug(&side.label)),
            home,
            dir,
            first,
            log: self
                .out
                .join("hooks")
                .join(format!("{}.jsonl", slug(&side.label))),
        }
    }
}

/// What the agent reads once a plugin side's SessionStart hooks have run.
fn start_session(
    side: &Side,
    plugin: &Plugin,
    layout: &Layout,
    dir: &Path,
    first: &[PathBuf],
) -> Result<String> {
    let home = layout.home(side);
    fs::write(home.join("transcript.jsonl"), "")?;
    let session = layout.session(side, plugin, &home, dir, first);
    let reply = session.fire(
        0,
        Event::SessionStart,
        &plugin.session_start,
        serde_json::json!({ "source": "startup" }),
    )?;
    Ok(with_context(String::new(), &reply.context))
}

fn run_side(
    side: &Side,
    layout: &Layout,
    (n, command): (usize, &CorpusCommand),
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
        Kind::Plugin(plugin) => {
            let session = layout.session(side, plugin, &home, dir, repo_first);
            let mut input = serde_json::json!({ "command": command.run });
            if let Some(question) = &command.question {
                input["description"] = question.as_str().into();
            }
            let tool = |input: &serde_json::Value| {
                serde_json::json!({
                    "tool_name": "Bash",
                    "tool_input": input,
                    "tool_use_id": format!("toolu_{:03}", n + 1),
                })
            };
            let pre = session.fire(n + 1, Event::PreToolUse, &plugin.pre, tool(&input))?;
            let mut context = pre.context;
            if let Some(reason) = pre.deny {
                // The command never runs, so there is no exit code.
                return Ok(Outcome {
                    text: with_context(reason, &context),
                    exit: None,
                    rewritten: None,
                });
            }
            let outcome = match pre.command {
                Some(rewritten) => {
                    before(repo_first)?;
                    input["command"] = rewritten.as_str().into();
                    let mut outcome = run_shell(&rewritten, dir, &home, repo_first)?;
                    outcome.rewritten = Some(rewritten);
                    outcome
                }
                None => Outcome {
                    text: raw.text.clone(),
                    exit: raw.exit,
                    rewritten: None,
                },
            };
            let mut fields = tool(&input);
            // shortcut: every non-zero exit counts as a failed call. Claude Code also lets
            // exit 1 pass as success from grep, rg, find, diff, test and `git diff`; no
            // corpus command exits 1 from those, and a new one would need that rule here.
            let after = if outcome.exit == Some(0) {
                fields["tool_response"] = serde_json::json!({
                    "stdout": outcome.text, "stderr": "", "interrupted": false, "isImage": false,
                });
                session.fire(n + 1, Event::PostToolUse, &plugin.post, fields)?
            } else {
                // A failed call shows the agent its error, and these hooks can only add to it.
                fields["error"] =
                    format!("Exit code {}\n{}", exit_text(outcome.exit), outcome.text).into();
                fields["is_interrupt"] = false.into();
                session.fire(n + 1, Event::PostToolUseFailure, &plugin.failure, fields)?
            };
            context.extend(after.context);
            Ok(Outcome {
                text: with_context(after.output.unwrap_or(outcome.text), &context),
                exit: outcome.exit,
                rewritten: outcome.rewritten,
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

/// An exit code, or 'none' for a command killed by a signal or refused by a hook.
fn exit_text(exit: Option<i32>) -> String {
    exit.map_or_else(|| "none".to_string(), |code| code.to_string())
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

/// `versions` and `session_start` (the tokens a plugin side's SessionStart hooks give the
/// agent once) hold one entry per side.
fn summary_md(
    rows: &[Row],
    corpus: &Corpus,
    sides: &[Side],
    versions: &[String],
    session_start: &[usize],
) -> String {
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
    for (side, tokens) in sides.iter().zip(session_start) {
        if matches!(side.kind, Kind::Plugin(_)) {
            let _ = writeln!(
                md,
                "- {}: its SessionStart hooks give the agent {tokens} tokens once a session, \
                 not counted below",
                markdown_cell(&side.label)
            );
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
            Kind::Rtk(_) | Kind::Plugin(_) => side_rows
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
        "No model is asked. Each command with a `keep` in corpus.toml lists facts its answer \
         needs, taken from its raw output: a chosen set, not all an answer could use. An \
         output keeps a fact when one of its lines holds it whole (no letter, digit, `_`, `.` \
         or `-` just before it, no letter, digit or `_` just after it, though a short hash may \
         run on into more hex digits), whitespace aside, and a fact the raw output has n times \
         must be held \
         n times. A fact an output reworded counts as lost, so read the lost facts before \
         reading a loss as a defect. A way to recover is recognised by each tool's hint text, \
         not tried.\n\n\
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
            let mut shown = HashSet::new();
            let first: Vec<String> = lost
                .iter()
                .filter(|f| shown.insert(f.as_str()))
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
    fs::create_dir_all(layout.out.join("hooks"))?;
    let repo_of = |command: &CorpusCommand| {
        corpus
            .repo
            .iter()
            .position(|r| r.name == command.repo)
            .expect("validated")
    };
    // A plugin side's session starts in the first command's repository.
    let mut session_start = Vec::new();
    for side in &sides {
        let Kind::Plugin(plugin) = &side.kind else {
            session_start.push(0);
            continue;
        };
        let index = corpus.command.first().map_or(0, repo_of);
        let first = repo_path(&dirs[index], &corpus.repo[index]);
        let text = start_session(side, plugin, &layout, &dirs[index], &first)?;
        let file = format!("000-{}-session-start.txt", slug(&side.label));
        fs::write(layout.out.join("outputs").join(file), &text)?;
        session_start.push(count_tokens(&text));
    }

    let mut rows = Vec::new();
    // Per command, how many facts its `keep` names.
    let mut facts_of = Vec::new();
    for (n, command) in corpus.command.iter().enumerate() {
        let index = repo_of(command);
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
        let wanted = match &command.keep {
            Some(keep) => facts(keep, &raw.text).with_context(|| command.run.clone())?,
            None => Vec::new(),
        };
        let mut outcomes = Vec::with_capacity(sides.len());
        for side in &sides[1..] {
            outcomes.push(run_side(side, &layout, (n, command), dir, &first, &raw)?);
        }
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
    let summary = summary_md(&rows, &corpus, &sides, &versions, &session_start);
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
        // A repeated line or name is a fact for each time it appears.
        let diff = "--- a/f\n+++ b/f\n@@ -1 +1 @@\n-old  line\n+new line\n+\n+new line\n";
        assert_eq!(
            facts(&Keep::Rule(Rule::Diff), diff).unwrap(),
            ["-old line", "+new line", "+new line"]
        );
        assert_eq!(
            facts(&Keep::Rule(Rule::Basenames), "a/b/c.rs\nd/c.rs\ne/\n").unwrap(),
            ["c.rs", "c.rs", "e"]
        );
        // A line with no letter or digit, such as a closing brace, tells an agent nothing.
        assert_eq!(
            facts(&Keep::Rule(Rule::Lines), "x  y\n\n}\nz\n").unwrap(),
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
    fn a_fact_is_held_only_as_a_whole_word() {
        let io = ["io.rs".to_string()];
        assert_eq!(lost(&io, "src/radio.rs\n"), ["io.rs"]);
        assert!(lost(&io, "src/io.rs:3: x\n").is_empty());
        assert!(facts(&Keep::Facts(vec!["io.rs".into()]), "src/radio.rs\n").is_err());
        // A short hash names the commit its full hash does.
        assert!(lost(&["4649aa9".to_string()], "commit 4649aa9700619f94\n").is_empty());
        assert_eq!(lost(&["4649aa9".to_string()], "4649aa9z\n"), ["4649aa9"]);
        // A name is a whole name: not the end of another, nor the start of a longer one.
        let names = facts(&Keep::Rule(Rule::Basenames), "a/io.rs\nb/old.io.rs\n").unwrap();
        assert_eq!(lost(&names, "b/old.io.rs\n"), ["io.rs"]);
        assert_eq!(lost(&io, "src/io.rsx\n"), ["io.rs"]);
        // A line that starts with a sign is a whole line too.
        let diff = ["+new line".to_string(), "+not+new line".to_string()];
        assert_eq!(lost(&diff, "+not+new line\n"), ["+new line"]);
    }

    #[test]
    fn each_copy_of_a_fact_must_be_kept() {
        let names = facts(&Keep::Rule(Rule::Basenames), "a/lib.rs\nb/lib.rs\n").unwrap();
        assert_eq!(lost(&names, "a/lib.rs\n"), ["lib.rs"]);
        assert!(lost(&names, "a/ lib.rs\nb/ lib.rs\n").is_empty());
        assert!(lost(&names, "a/ b/ lib.rs lib.rs\n").is_empty());
    }

    #[test]
    fn each_tools_recovery_hint_is_recognised() {
        assert!(names_a_recovery(
            "[full output: rtk recall 4ac7b03c95a09e7f]"
        ));
        assert!(names_a_recovery("+11 hidden: rtk recall 995969b07316e2e1]"));
        assert!(names_a_recovery(
            "[+11 hidden (lines 4-14): rtk recall 995969b07316e2e1]"
        ));
        assert!(names_a_recovery(
            "[full output: /home/u/.local/share/snip/tee/1-cargo.log]"
        ));
        assert!(names_a_recovery(
            "recover with `tokenade expand-ref 704eb3a4aef9`"
        ));
        assert!(names_a_recovery(
            "diffstat only. Full patch: git show -p <rev>"
        ));
        assert!(names_a_recovery(
            "ctx_search({queries:[\"q\"], source:\"s1\"}) searches the whole output"
        ));
        assert!(names_a_recovery(
            "[token-optimizer: 4 KB output compacted; full output: Read /h/.t/o.log]"
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
    fn matchers_follow_claude_codes_rules() {
        for all in [None, Some(""), Some("*")] {
            assert!(matcher_selects(all, "Bash").unwrap());
        }
        assert!(matcher_selects(Some("Bash"), "Bash").unwrap());
        assert!(matcher_selects(Some("Read|Bash"), "Bash").unwrap());
        assert!(!matcher_selects(Some("Bash2"), "Bash").unwrap());
        // Only letters, digits, `_` and `|`: exact names, so no prefix matches.
        assert!(!matcher_selects(Some("mcp__memory"), "mcp__memory__read").unwrap());
        assert!(matcher_selects(Some("^(Bash|Read)$"), "Bash").unwrap());
        assert!(matcher_selects(Some("Edit|Ba.h"), "Bash").unwrap());
        assert!(!matcher_selects(Some("^Notebook"), "Bash").unwrap());
        assert!(matcher_selects(Some("("), "Bash").is_err());
    }

    fn run(exit: Option<i32>, stdout: &str, stderr: &str) -> HookRun {
        HookRun {
            exit,
            stdout: stdout.into(),
            stderr: stderr.into(),
        }
    }

    #[test]
    fn hook_replies_are_read_as_claude_code_reads_them() {
        let pre = |r: &HookRun| read_reply(Event::PreToolUse, r);
        let post = |r: &HookRun| read_reply(Event::PostToolUse, r);
        // Exit 2 refuses before the call and adds stderr after it; at session start
        // only the user sees it. Other exits and timeouts reach only the user.
        assert_eq!(pre(&run(Some(2), "", "no")).deny.as_deref(), Some("no"));
        assert_eq!(post(&run(Some(2), "", "late")).context, ["late"]);
        assert!(
            read_reply(Event::SessionStart, &run(Some(2), "", "x"))
                .context
                .is_empty()
        );
        for quiet in [run(Some(1), "{}", "x"), run(None, "", "")] {
            let reply = pre(&quiet);
            assert!(reply.deny.is_none() && reply.context.is_empty());
        }
        // Plain stdout counts at session start only.
        assert_eq!(
            read_reply(Event::SessionStart, &run(Some(0), "hi\n", "")).context,
            ["hi"]
        );
        assert!(pre(&run(Some(0), "hi", "")).context.is_empty());

        let deny = r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"use rg","updatedInput":{"command":"x"}}}"#;
        let reply = pre(&run(Some(0), deny, ""));
        assert_eq!(
            (reply.deny.as_deref(), reply.command),
            (Some("use rg"), None)
        );
        let rewrite = r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","updatedInput":{"command":"cat o"},"additionalContext":"outline"}}"#;
        let reply = pre(&run(Some(0), rewrite, ""));
        assert_eq!(reply.command.as_deref(), Some("cat o"));
        assert_eq!(reply.context, ["outline"]);

        let replace = r#"{"decision":"block","reason":"why","hookSpecificOutput":{"hookEventName":"PostToolUse","updatedToolOutput":{"stdout":"cut","stderr":"","interrupted":false,"isImage":false}}}"#;
        let reply = post(&run(Some(0), replace, ""));
        assert_eq!(reply.output.as_deref(), Some("cut"));
        assert_eq!(reply.context, ["why"]);
        // A failure hook cannot replace what the agent reads.
        assert!(
            read_reply(Event::PostToolUseFailure, &run(Some(0), replace, ""))
                .output
                .is_none()
        );
    }

    #[test]
    fn a_replacement_needs_bashs_output_shape() {
        let shape = |v: serde_json::Value| bash_output(&v);
        assert_eq!(
            shape(serde_json::json!({"stdout": "a", "stderr": "", "interrupted": false})),
            Some("a".into())
        );
        assert_eq!(
            shape(
                serde_json::json!({"stdout": "a", "stderr": "b", "interrupted": false, "isImage": false})
            ),
            Some("a\nb".into())
        );
        assert_eq!(
            shape(serde_json::json!({"stdout": "a", "interrupted": false})),
            None
        );
        assert_eq!(
            shape(serde_json::json!({"stdout": "a", "stderr": "", "interrupted": "no"})),
            None
        );
        assert_eq!(
            shape(
                serde_json::json!({"stdout": "a", "stderr": "", "interrupted": false, "isImage": null})
            ),
            None
        );
        assert_eq!(shape(serde_json::json!("a")), None);
    }

    #[test]
    fn hook_context_follows_the_result_on_lines_of_its_own() {
        assert_eq!(
            with_context("out".into(), &["a".into(), "b".into()]),
            "out\na\nb"
        );
        assert_eq!(with_context("out\n".into(), &["a".into()]), "out\na");
        assert_eq!(with_context(String::new(), &["a".into()]), "a");
    }

    #[test]
    fn a_plugin_names_its_hooks_file_or_uses_the_default() {
        let root =
            std::env::temp_dir().join(format!("output-tokens-plugin-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join(".claude-plugin")).unwrap();
        fs::create_dir_all(root.join("hooks")).unwrap();
        let hooks = r#"{"hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "a", "timeout": 5}]}],
            "PostToolUse": [{"matcher": "Read", "hooks": [{"type": "command", "command": "b"}]}]}}"#;
        fs::write(root.join("hooks/hooks.json"), hooks).unwrap();
        fs::write(root.join(".claude-plugin/plugin.json"), "{}").unwrap();
        let plugin = load_plugin(root.clone()).unwrap();
        assert_eq!(plugin.pre.len(), 1);
        assert_eq!(plugin.pre[0].timeout, Duration::from_secs(5));
        assert!(plugin.post.is_empty() && plugin.session_start.is_empty());
        fs::write(
            root.join(".claude-plugin/plugin.json"),
            r#"{"hooks": "./hooks/other.json"}"#,
        )
        .unwrap();
        assert!(
            load_plugin(root.clone()).is_err(),
            "the named file is missing"
        );
        fs::write(root.join("hooks/other.json"), r#"{"hooks": {}}"#).unwrap();
        assert!(load_plugin(root.clone()).unwrap().pre.is_empty());
        fs::write(root.join(".claude-plugin/plugin.json"), r#"{"hooks": {}}"#).unwrap();
        assert!(
            load_plugin(root.clone()).is_err(),
            "inline hooks are refused"
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_side_gets_a_temporary_directory_of_its_own() {
        // A plugin keeps state in its temporary directory (token-optimizer-mcp
        // marks each command it has seen there), so a shared one carries it from
        // one run or side to the next.
        let home = std::env::temp_dir().join(format!("output-tokens-home-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        let mut cmd = Command::new("true");
        side_env(&mut cmd, &home, &[]).unwrap();
        let tmp = cmd
            .get_envs()
            .find(|(var, _)| *var == "TMPDIR")
            .and_then(|(_, value)| value);
        assert_eq!(tmp, Some(home.join("tmp").as_os_str()));
        assert!(home.join("tmp").is_dir());
        fs::remove_dir_all(&home).unwrap();
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
