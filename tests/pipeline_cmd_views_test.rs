//! Integration tests: the `rtk pipeline` views and commands over a seeded home
//! — `show`, `validate`, `test`, `status`, `explain`, the decision log,
//! `bench`, `eval`, `add`, `promote`, `share` and `rtk methods list` — read
//! off a pipe and, where the command asks or draws, off a terminal.
#![cfg(all(feature = "engine", target_os = "linux"))]

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// A user pipeline with one rule of each kind `show` flags, a task bound to an
/// upstream whose credential is unset, an override of a built-in rule and a
/// directive whose file is missing.
const FLAGS: &str = r#"schema_version = 1

[pipeline]
name = "flags"
extends = "rtk:posthook"

[upstream.fx]
provider = "google"
model = "gemini-2.5-flash"
api_key_env = "RTK_TEST_FLAGS_KEY"

[task.digest-it]
kind = "summarize"
upstream = "fx"

[[hook.before_tool.shell]]
name = "blocker"
when = { command_matches = "rm -rf" }
action = "deny"

[[hook.before_tool.read]]
name = "swap-read"
when = { file_lines_gt = 5 }
action = "route"
task = "digest-it"

[[hook.after_tool.shell]]
name = "proxy-only"
when = { command_matches = "proxy-only-probe" }
use = ["ccr"]

[[hook.after_tool.shell]]
name = "watched"
when = { command_matches = "cargo build" }
use = ["dedup"]
shadow = true
continue = true

[[hook.after_tool.shell]]
name = "off"
use = ["dedup"]
enabled = false

[[hook.after_tool.shell]]
name = "pass"
when = { command_matches = "keep-raw" }
action = "passthrough"

[[hook.after_tool.write]]
name = "no-extract"
use = ["dedup"]

[override.term-default]
when.lines_gt = 10

[[directive]]
name = "from-file"
file = "missing-directive.md"
"#;

/// A rule whose action is not legal where it sits, so the file is dropped.
const BROKEN: &str = "[pipeline]\nname = \"broken\"\n\n[[hook.after_tool.shell]]\nname = \"bad\"\naction = \"deny\"\n";

/// A home of the test's own: rtk keeps its data under `rtk/`, its pipelines
/// under `config/` and starts in `project/`.
struct Home {
    root: tempfile::TempDir,
}

/// What a run printed, with its exit code.
struct Out {
    stdout: String,
    stderr: String,
    code: Option<i32>,
}

impl Home {
    fn new() -> Self {
        Self {
            root: common::tempdir(),
        }
    }

    fn with_pipeline(text: &str) -> Self {
        let home = Self::new();
        home.write("config/pipeline.toml", text);
        home
    }

    fn path(&self) -> &Path {
        self.root.path()
    }

    fn write(&self, relative: &str, content: &str) -> PathBuf {
        let path = self.path().join(relative);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("create parent");
        std::fs::write(&path, content).expect("write fixture");
        path
    }

    fn read(&self, relative: &str) -> Option<String> {
        std::fs::read_to_string(self.path().join(relative)).ok()
    }

    fn command(&self, args: &[&str]) -> Command {
        let tmp = self.path().join("tmp");
        std::fs::create_dir_all(&tmp).expect("create tmp");
        let mut cmd = common::rtk_command_in(self.path());
        cmd.env("RTK_CONFIG_DIR", self.path().join("config"))
            .env("TMPDIR", tmp)
            .env("LC_ALL", "C")
            .env("RTK_PROXY_BIND", common::no_proxy_bind())
            .args(args);
        cmd
    }

    fn run(mut cmd: Command, stdin: Stdio) -> Out {
        let output = cmd.stdin(stdin).output().expect("spawn rtk");
        Out {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            code: output.status.code(),
        }
    }

    fn rtk(&self, args: &[&str]) -> Out {
        Self::run(self.command(args), Stdio::null())
    }

    /// `rtk <args>` that must succeed; its stdout.
    fn ok(&self, args: &[&str]) -> String {
        let out = self.rtk(args);
        assert_eq!(
            out.code,
            Some(0),
            "rtk {args:?}: {}{}",
            out.stdout,
            out.stderr
        );
        out.stdout
    }

    /// `rtk <args>` that must fail; its stdout and stderr together.
    fn fails(&self, args: &[&str]) -> String {
        let out = self.rtk(args);
        assert_ne!(out.code, Some(0), "rtk {args:?}: {}", out.stdout);
        format!("{}{}", out.stdout, out.stderr)
    }

    /// Tracking rows: `(site, kind, name, calls, tokens_in, tokens_out)`.
    fn attribution(&self, rows: &[(&str, &str, &str, i64, i64, i64)]) {
        common::seed::attribution(&self.path().join("rtk").join("history.db"), rows);
    }

    /// The proxy's usage database with the current schema, filled by `rows`.
    fn proxy(&self, rows: &str) {
        common::seed::proxy_usage(&self.path().join(".config/rtk"), rows);
    }

    /// A `bin/` holding only a `gh` that answers `--version` and fails
    /// everything else, or nothing at all.
    fn bin(&self, failing_gh: bool) -> String {
        let bin = self.path().join("bin");
        std::fs::create_dir_all(&bin).expect("create bin");
        if failing_gh {
            let stub = bin.join("gh");
            std::fs::write(
                &stub,
                "#!/bin/sh\n[ \"$1\" = --version ] && exit 0\necho 'denied by the stub' >&2\nexit 1\n",
            )
            .expect("write gh stub");
            let mut permissions = std::fs::metadata(&stub).expect("stub").permissions();
            std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
            std::fs::set_permissions(&stub, permissions).expect("make the stub runnable");
        }
        bin.display().to_string()
    }
}

/// A loopback server that serves `text` to every request; its URL.
fn serve(text: &'static str) -> String {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let url = format!(
        "http://127.0.0.1:{}/fetched.toml",
        listener.local_addr().expect("address").port()
    );
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                text.len()
            );
        }
    });
    url
}

#[test]
fn show_names_the_compiled_default_a_fallback_and_a_dropped_file() {
    let home = Home::new();
    let out = home.ok(&["pipeline", "show"]);
    assert!(
        out.contains("(no pipeline file present — this is the compiled rtk:default)"),
        "{out}"
    );
    assert!(!out.contains("Tasks:"), "{out}");
    let out = home.ok(&["pipeline", "test"]);
    assert!(
        out.contains("no pipeline files present — the compiled rtk:default is active"),
        "{out}"
    );

    home.write(
        "config/pipeline.toml",
        "[pipeline]\nname = \"g\"\n\n[guards]\nnever_worse = false\n",
    );
    let out = home.ok(&["pipeline", "show"]);
    assert!(
        out.contains("(fell back to the built-in default — see diagnostics below)"),
        "{out}"
    );
    assert!(
        out.contains("E002  [guards] never_worse cannot be disabled"),
        "{out}"
    );

    let file = home.write("config/pipeline.toml", BROKEN);
    let out = home.ok(&["pipeline", "show"]);
    assert!(
        out.contains(&format!(
            "(not loaded, has errors: {} — the other files are active; see diagnostics below)",
            file.display()
        )),
        "{out}"
    );
    let out = home.rtk(&["pipeline", "test"]);
    assert_eq!(out.code, Some(1), "{}", out.stderr);
    assert!(
        out.stdout
            .contains("pipeline files have errors — hooks fall back to rtk:default"),
        "{}",
        out.stdout
    );
}

#[test]
fn show_flags_each_kind_of_rule_and_validate_names_what_is_missing() {
    let home = Home::with_pipeline(FLAGS);
    home.write(
        ".claude/settings.json",
        r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"rtk hook claude"}]}]}}"#,
    );
    let out = home.ok(&["pipeline", "show"]);
    for expected in [
        "-> use=[ccr] (inert: proxy-only)",
        "-> use=[dedup] (inert: no extraction)",
        "-> use=[dedup] (shadow, continue)",
        "-> use=[dedup] (disabled)",
        "when is_rtk_command=false, lines_gt=10 -> use=[term]  [overridden by ",
        "      [file not readable: missing-directive.md]",
        "Tasks:\n  digest-it              kind=summarize upstream=fx [",
    ] {
        assert!(out.contains(expected), "missing {expected:?} in:\n{out}");
    }

    let out = home.ok(&["pipeline", "validate"]);
    for expected in [
        "pipeline: ok (flags)",
        "auth needed: export RTK_TEST_FLAGS_KEY=...   (upstream `fx` — task `digest-it`)",
        "hook needed: ",
        " — your pipeline has rules there; run `rtk init -g` so Claude Code sends those tool calls to rtk",
    ] {
        assert!(out.contains(expected), "missing {expected:?} in:\n{out}");
    }

    let out = home.ok(&["pipeline", "test"]);
    let file = home.path().join("config/pipeline.toml");
    assert!(out.contains(&format!("{} (User)", file.display())), "{out}");
    assert!(out.contains("resolved: flags ("), "{out}");

    let out = home.ok(&["pipeline", "status"]);
    assert!(out.contains("active: flags"), "{out}");
    assert!(
        out.contains("note: proxy.request rules are present but the RTK proxy is not running — they are inert"),
        "{out}"
    );
}

#[test]
fn methods_list_names_every_method_chain_and_directive() {
    let out = Home::new().ok(&["methods", "list"]);
    for expected in [
        "method               kind      lossy  recovery   cost     site",
        "ansi                 transform false  none       instant  hook/proxy",
        "chains (composites, usable anywhere a method is):",
        "directives:",
        "  rtk-awareness (levels: default | high | full)",
        "reserved names: ",
    ] {
        assert!(out.contains(expected), "missing {expected:?} in:\n{out}");
    }
    assert!(out.contains(" proxy\n"), "a proxy-only method:\n{out}");
}

#[test]
fn status_reports_recorded_problems_and_says_nothing_of_a_running_proxy() {
    let home = Home::new();
    let mut cmd = home.command(&["pipeline", "status"]);
    cmd.env("RTK_PROXY_BIND", common::seed::fake_proxy());
    let out = Home::run(cmd, Stdio::null());
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    assert!(out.stdout.contains("active: rtk:default"), "{}", out.stdout);
    assert!(!out.stdout.contains("note:"), "{}", out.stdout);

    home.write(
        "rtk/pipeline-status.json",
        r#"{"diagnostics":[{"message":"E005 from the last hook"},{"code":"W001"}],"fell_back":true}"#,
    );
    let out = home.rtk(&["pipeline", "status"]);
    assert_eq!(out.code, Some(1), "{}", out.stderr);
    assert_eq!(
        out.stdout,
        "pipeline: problems recorded at the last hook invocation\n\
         E005 from the last hook\n\
         hooks are running the built-in rtk:default until this is fixed\n"
    );

    // A status with neither diagnostics nor a fallback still fails.
    home.write("rtk/pipeline-status.json", "{}");
    let out = home.rtk(&["pipeline", "status"]);
    assert_eq!(out.code, Some(1), "{}", out.stderr);
    assert_eq!(
        out.stdout,
        "pipeline: problems recorded at the last hook invocation\n"
    );
}

#[test]
fn explain_walks_both_positions_for_each_kind_of_agent() {
    let home = Home::with_pipeline(FLAGS);
    let seven = home.write("project/seven.txt", "1\n2\n3\n4\n5\n6\n7\n");
    let cases: [(&[&str], &[&str]); 5] = [
        (
            &["--command", "rm -rf /tmp/x"],
            &["hook.before_tool.shell: rule `blocker` -> action deny"],
        ),
        (
            &["--command", "proxy-only-probe"],
            &[
                "hook.after_tool.shell: rule `proxy-only` -> []  (inert: nothing runs here — see `rtk pipeline validate`)  [warn] unresolved: ccr",
            ],
        ),
        (
            &["--command", "cargo build", "--format", "term"],
            &["hook.after_tool.shell: rule `watched` -> [dedup] (shadow)"],
        ),
        (
            &["--command", "keep-raw"],
            &["hook.after_tool.shell: rule `pass` -> action passthrough"],
        ),
        (
            &["--file", seven.to_str().expect("a UTF-8 path")],
            &[
                "agent: claude-code   tool: Read -> subject: read",
                "hook.before_tool.read: rule `swap-read` -> action route -> task `digest-it`",
                "hook.after_tool.read: no rule matches these facts — output passes through\n",
            ],
        ),
    ];
    for (args, expected) in cases {
        let out = home.ok(&[&["pipeline", "explain"], args].concat());
        for line in expected {
            assert!(out.contains(line), "{args:?}: missing {line:?} in:\n{out}");
        }
        assert!(!out.contains("has no rules at this position"), "{out}");
    }

    // Piped output supplies the size facts the overridden rule needs.
    let big = home.write("big.txt", &"output line\n".repeat(50));
    let piped = Home::run(
        home.command(&["pipeline", "explain", "--command", "ls"]),
        Stdio::from(std::fs::File::open(&big).expect("open big")),
    );
    assert!(
        piped
            .stdout
            .contains("hook.after_tool.shell: rule `term-default` -> [ansi, progress, dedup, base64-elide, cap]"),
        "{}",
        piped.stdout
    );
    let bare = home.ok(&["pipeline", "explain", "--command", "ls"]);
    assert!(
        bare.contains("hook.after_tool.shell: no rule matches these facts"),
        "{bare}"
    );
    // A terminal on stdin is no tool output either.
    let run = common::tty::run(
        home.command(&["pipeline", "explain", "--command", "ls"]),
        b"",
    );
    assert!(run.success, "{}", run.stderr);
    assert!(
        run.screen
            .contains("hook.after_tool.shell: no rule matches these facts"),
        "{}",
        run.screen
    );

    let out = home.rtk(&["pipeline", "explain", "--agent", "nobody"]);
    assert_eq!(out.code, Some(1));
    assert!(
        out.stdout
            .starts_with("unknown agent `nobody` — known: claude-code, "),
        "{}",
        out.stdout
    );
    let roo = home.ok(&["pipeline", "explain", "--agent", "roo"]);
    assert!(
        roo.contains("roo exposes no usable hook site — RTK reaches it through the AwarenessFile directive channel only"),
        "{roo}"
    );
    let gemini = home.ok(&["pipeline", "explain", "--agent", "gemini"]);
    assert!(
        gemini.contains("gemini cannot replace tool output — RTK degrades to deny-with-suggestion before the tool"),
        "{gemini}"
    );
    assert!(
        gemini.contains("start the RTK proxy to enable output transforms: rtk llm-proxy start"),
        "{gemini}"
    );
    let mut cmd = home.command(&["pipeline", "explain", "--agent", "gemini"]);
    cmd.env("RTK_PROXY_BIND", common::seed::fake_proxy());
    let proxied = Home::run(cmd, Stdio::null());
    assert!(
        proxied.stdout.contains(
            "output transforms run at the proxy for gemini (no output replacement in its hooks)"
        ),
        "{}",
        proxied.stdout
    );

    let plain = Home::new().ok(&["pipeline", "explain", "--tool", "Write"]);
    assert!(
        plain.contains("(the active pipeline `rtk:default` has no rules at this position"),
        "{plain}"
    );
}

#[test]
fn the_decision_log_toggles_and_explain_last_reads_its_newest_entry() {
    let home = Home::new();
    let out = home.rtk(&["pipeline", "explain", "--last"]);
    assert_eq!(out.code, Some(1));
    assert!(
        out.stdout.starts_with("no logged decisions"),
        "{}",
        out.stdout
    );
    assert_eq!(
        home.ok(&["pipeline", "log", "tail"]),
        "decision log is off — enable with `rtk pipeline log on`\nno entries\n"
    );

    let out = home.ok(&["pipeline", "log", "on"]);
    let log = home.path().join("rtk").join("pipeline.log");
    assert!(
        out.contains(&format!(
            "decision log enabled — entries at {}",
            log.display()
        )),
        "{out}"
    );
    assert!(home.path().join("rtk/pipeline-log.enabled").exists());

    let deny = r#"{"ts":"2026-11-26T00:00:00Z","site":"hook","event":"before_tool","subject":"shell","rule":"blocker","action":"deny"}"#;
    let shadow = r#"{"ts":"2026-11-26T00:00:01Z","site":"hook","event":"after_tool","subject":"shell","rule":"watched","methods":["dedup"],"tokens_before":200,"tokens_after":50,"shadow":true}"#;
    let empty = r#"{"ts":"2026-11-26T00:00:02Z","site":"hook","event":"after_tool","subject":"read","rule":"quiet","methods":["dedup"],"tokens_before":0,"tokens_after":0}"#;
    let bare = r#"{"ts":"2026-11-26T00:00:03Z","site":"hook","event":"after_tool","subject":"read","rule":"bare"}"#;

    home.write("rtk/pipeline.log", &format!("{deny}\n{shadow}\n"));
    let out = home.ok(&["pipeline", "log", "show", "--rule", "blocker"]);
    assert!(out.contains("blocker") && out.contains("deny"), "{out}");
    assert!(!out.contains("watched"), "{out}");
    let out = home.ok(&["pipeline", "explain", "--last"]);
    for expected in [
        "rule `watched` fired at hook.after_tool.shell; methods applied: [dedup]",
        "tokens 200 -> 50 (75.0% saved)",
        "shadow rule: recorded only, nothing was emitted",
    ] {
        assert!(out.contains(expected), "missing {expected:?} in:\n{out}");
    }

    home.write("rtk/pipeline.log", &format!("{shadow}\n{deny}\n"));
    let out = home.ok(&["pipeline", "explain", "--last"]);
    assert!(
        out.contains("rule `blocker` fired at hook.before_tool.shell with action `deny`"),
        "{out}"
    );

    home.write("rtk/pipeline.log", &format!("{empty}\n"));
    let out = home.ok(&["pipeline", "explain", "--last"]);
    assert!(out.contains("tokens 0 -> 0 (0.0% saved)"), "{out}");

    home.write("rtk/pipeline.log", &format!("{bare}\n"));
    let out = home.ok(&["pipeline", "explain", "--last"]);
    assert_eq!(out.lines().count(), 1, "only the entry itself:\n{out}");
    assert!(out.contains("bare"), "{out}");

    let unmeasured = r#"{"ts":"2026-11-26T00:00:04Z","site":"hook","event":"after_tool","subject":"read","rule":"unmeasured","methods":["dedup"]}"#;
    home.write("rtk/pipeline.log", &format!("{unmeasured}\n"));
    let out = home.ok(&["pipeline", "explain", "--last"]);
    assert!(
        out.contains("rule `unmeasured` fired at hook.after_tool.read; methods applied: [dedup]"),
        "{out}"
    );
    assert!(!out.contains("tokens "), "no sizes were logged: {out}");

    assert_eq!(
        home.ok(&["pipeline", "log", "off"]),
        "decision log disabled\n"
    );
    assert!(!home.path().join("rtk/pipeline-log.enabled").exists());

    // With a file where the data directory belongs, the marker cannot be written.
    let blocked = Home::new();
    blocked.write("rtk", "not a directory");
    let out = blocked.fails(&["pipeline", "log", "on"]);
    assert!(out.contains("failed to toggle the decision log"), "{out}");
}

#[test]
fn bench_runs_the_matching_chain_or_says_why_nothing_matches() {
    let home = Home::with_pipeline(FLAGS);
    let mut term = String::new();
    for n in 0..3000 {
        term.push_str(&format!("line of output number {n}\n"));
    }
    let term = home.write("project/term.log", &term);
    let term = term.to_str().expect("a UTF-8 path");
    let out = home.ok(&["pipeline", "bench", term]);
    assert!(
        out.contains("rule [term-default] chain [ansi, progress, dedup, base64-elide, cap] (hook.after_tool.shell, format=term)"),
        "{out}"
    );
    assert!(out.contains("savings: "), "{out}");

    let lock = home.write("project/Cargo.lock", "");
    let out = home.ok(&["pipeline", "bench", lock.to_str().expect("a UTF-8 path")]);
    assert!(out.contains("rule [lockfiles] chain [lockfile]"), "{out}");
    assert!(
        out.contains("before: 0 tokens   after: 0 tokens   savings: 0.0%"),
        "{out}"
    );

    let json = home.write("project/x.json", "{\"a\":1}\n");
    let out = home.ok(&["pipeline", "bench", json.to_str().expect("a UTF-8 path")]);
    assert_eq!(
        out,
        "no rule matches (hook.after_tool.read, format=json) in `flags` — nothing to bench\n"
    );

    let html = home.write("project/x.html", "<html><body>hi</body></html>\n");
    let out = home.ok(&["pipeline", "bench", html.to_str().expect("a UTF-8 path")]);
    assert!(out.contains("rule [web-default] chain ["), "{out}");

    let plain = Home::new();
    let out = plain.ok(&["pipeline", "bench", term]);
    assert!(out.contains("in `rtk:default` — nothing to bench"), "{out}");
    assert!(
        out.contains("hint: `extends = \"rtk:posthook\"` adds the output-compression rules"),
        "{out}"
    );
    let missing = plain.fails(&["pipeline", "bench", "/nonexistent/input.log"]);
    assert!(
        missing.contains("Failed to read /nonexistent/input.log"),
        "{missing}"
    );

    // A rule that acts instead of transforming leaves nothing to bench.
    let raw = Home::with_pipeline(
        "[pipeline]\nname = \"raw\"\n\n[[hook.after_tool.shell]]\nname = \"keep\"\naction = \"passthrough\"\n",
    );
    let out = raw.ok(&["pipeline", "bench", term]);
    assert_eq!(
        out,
        "no rule matches (hook.after_tool.shell, format=term) in `raw` — nothing to bench\n"
    );
}

#[test]
fn eval_compares_a_measured_candidate_with_the_active_pipeline() {
    let home = Home::new();
    let out = home.ok(&["pipeline", "eval", "cand"]);
    assert!(
        out.contains("  candidate      no measurements yet"),
        "{out}"
    );
    assert!(
        out.contains("  start with: rtk pipeline eval cand --start"),
        "{out}"
    );

    home.attribution(&[
        ("shadow", "pipeline", "idle", 1, 0, 0),
        ("shadow", "pipeline", "cand", 3, 1000, 250),
    ]);
    // Measured, not armed, and nothing active to compare with.
    let out = home.ok(&["pipeline", "eval", "cand"]);
    assert!(out.contains("(75% removed · 3 outputs)   est."), "{out}");
    assert!(!out.contains("active now"), "{out}");
    assert!(!out.contains("still collecting"), "{out}");

    home.attribution(&[("hook", "pipeline", "live", 2, 800, 400)]);
    home.write("rtk/pipeline-eval.json", r#"{"candidate":"idle"}"#);
    let out = home.ok(&["pipeline", "eval", "idle"]);
    assert!(out.contains("no measurements yet"), "{out}");
    assert!(!out.contains("start with"), "an armed candidate: {out}");

    home.write("rtk/pipeline-eval.json", r#"{"candidate":"cand"}"#);
    let out = home.ok(&["pipeline", "eval", "cand"]);
    for expected in [
        "(75% removed · 3 outputs)   est.",
        "  active now     ",
        "(50% removed · 2 outputs)   est.",
        "  ~ classifier/route effects excluded from the candidate",
        "  (still collecting — stop with: rtk pipeline eval cand --stop)",
    ] {
        assert!(out.contains(expected), "missing {expected:?} in:\n{out}");
    }

    home.write(
        "config/pipelines/shadow/fine.toml",
        "[pipeline]\nname = \"fine\"\n",
    );
    let out = home.ok(&["pipeline", "eval", "fine", "--start"]);
    assert!(
        out.contains("shadow-evaluating `fine` on your live sessions"),
        "{out}"
    );
    assert!(
        home.read("rtk/pipeline-eval.json")
            .is_some_and(|state| state.contains("\"fine\"")),
        "the shadow install is the candidate"
    );
    home.write("config/pipelines/shadow/broke.toml", BROKEN);
    let out = home.fails(&["pipeline", "eval", "broke", "--start"]);
    assert!(
        out.contains("`broke` does not validate — fix it before evaluating"),
        "{out}"
    );
}

#[test]
fn add_refuses_what_it_cannot_install_and_reports_what_it_installs() {
    let home = Home::new();
    let empty = home.write("empty.toml", "");
    let out = home.fails(&["pipeline", "add", empty.to_str().expect("path"), "--force"]);
    assert!(
        out.contains("the source contains no pipeline TOML — nothing to install"),
        "{out}"
    );

    let stemmy = home.write(
        "stemmy.toml",
        "[[directive]]\nname = \"x\"\ntext = \"hi\"\n",
    );
    let piped = Home::run(
        home.command(&["pipeline", "add", "-", "--force"]),
        Stdio::from(std::fs::File::open(&stemmy).expect("open stemmy")),
    );
    assert_ne!(piped.code, Some(0));
    assert!(
        piped
            .stderr
            .contains("no `[pipeline] name` — give the pipeline a name with `--name <name>`"),
        "{}",
        piped.stderr
    );
    let stemmy = stemmy.to_str().expect("path");
    let out = home.fails(&["pipeline", "add", stemmy, "--name", "bad name", "--force"]);
    assert!(
        out.contains("pipeline name `bad name` must be alphanumeric with `-` or `_`"),
        "{out}"
    );

    let out = home.ok(&["pipeline", "add", stemmy, "--shadow"]);
    assert!(out.contains("trying `stemmy` silently"), "{out}");
    assert!(!out.contains("stays live"), "nothing was live: {out}");
    let out = home.ok(&["pipeline", "add", stemmy, "--force"]);
    assert!(
        home.read("config/pipelines/stemmy.toml").is_some(),
        "the file stem names it"
    );
    assert!(out.contains("this pipeline uses directives — "), "{out}");

    let plug = home.write(
        "plug.toml",
        "[pipeline]\nname = \"plug\"\n\n[[hook.after_tool.shell]]\nname = \"p\"\nuse = [\"no-such-method\"]\n\n[[hook.after_tool.read]]\nname = \"bad\"\naction = \"deny\"\n",
    );
    let broken = home.write("broken.toml", BROKEN);
    let out = home.rtk(&["pipeline", "add", broken.to_str().expect("path"), "--force"]);
    assert_eq!(out.code, Some(1));
    assert!(
        out.stderr.contains("not installed — fix the errors above"),
        "{}",
        out.stderr
    );
    assert!(
        !out.stderr.contains("needs plugins"),
        "every method is built in: {}",
        out.stderr
    );

    let out = home.rtk(&["pipeline", "add", plug.to_str().expect("path"), "--force"]);
    assert_eq!(out.code, Some(1));
    for expected in [
        "This pipeline needs plugins you do not have",
        "Methods your rtk does not have: no-such-method",
        "not installed — fix the errors above",
    ] {
        assert!(
            out.stderr.contains(expected),
            "missing {expected:?} in:\n{}",
            out.stderr
        );
    }
    assert!(home.read("config/pipelines/plug.toml").is_none());

    let flags = home.write("flags.toml", FLAGS);
    let out = home.ok(&["pipeline", "add", flags.to_str().expect("path"), "--force"]);
    for expected in [
        "  [warn] sends the value of $RTK_TEST_FLAGS_KEY (and routed content) to that URL",
        "auth needed: export RTK_TEST_FLAGS_KEY=...",
        "this pipeline uses output compression and directives — ",
    ] {
        assert!(out.contains(expected), "missing {expected:?} in:\n{out}");
    }

    let quiet = home.write(
        "quiet.toml",
        "[pipeline]\nname = \"quiet\"\nextends = \"rtk:posthook\"\n\n[override.rtk-awareness]\nenabled = false\n",
    );
    let out = home.ok(&["pipeline", "add", quiet.to_str().expect("path"), "--force"]);
    assert!(
        out.contains("this pipeline uses output compression — "),
        "{out}"
    );
    let silent = home.write(
        "silent.toml",
        "[pipeline]\nname = \"silent\"\n\n[override.rtk-awareness]\nenabled = false\n",
    );
    let out = home.ok(&["pipeline", "add", silent.to_str().expect("path"), "--force"]);
    assert!(!out.contains("this pipeline uses"), "{out}");
    assert!(
        out.contains("active now, merged with your other pipelines"),
        "{out}"
    );
}

#[test]
fn add_from_a_url_records_its_origin_for_a_shadow_and_an_identical_reinstall() {
    const FETCHED: &str = "[pipeline]\nname = \"fetched\"\nextends = \"rtk:default\"\n";
    let url = serve(FETCHED);
    let ledger = "rtk/pipeline-installs.toml";

    let shadowed = Home::new();
    let out = shadowed.ok(&["pipeline", "add", &url, "--shadow"]);
    assert!(out.contains("trying `fetched` silently"), "{out}");
    assert!(
        shadowed
            .read(ledger)
            .is_some_and(|text| text.contains(&url)),
        "a shadow install records where it came from"
    );

    let home = Home::new();
    home.ok(&["pipeline", "add", &url, "--force"]);
    std::fs::remove_file(home.path().join(ledger)).expect("drop the ledger");
    let out = home.ok(&["pipeline", "add", &url]);
    assert!(
        out.contains("`fetched` is already installed and identical — nothing to change"),
        "{out}"
    );
    assert!(
        home.read(ledger).is_some_and(|text| text.contains(&url)),
        "an identical reinstall records its origin again"
    );
}

#[test]
fn add_on_a_terminal_offers_install_try_or_quit() {
    const V1: &str = "[pipeline]\nname = \"tidy\"\nextends = \"rtk:default\"\n";
    const V2: &str = "[pipeline]\nname = \"tidy\"\nextends = \"rtk:default\"\n\n[[directive]]\nname = \"tone\"\ntext = \"be brief\"\n";
    let home = Home::new();
    let v1 = home.write("v1.toml", V1);
    let run = common::tty::run_answering(
        home.command(&["pipeline", "add", v1.to_str().expect("path")]),
        b"",
        &[("nothing installed", b"1")],
    );
    assert!(run.success, "{}{}", run.screen, run.stderr);
    assert!(run.screen.contains("installed: "), "{}", run.screen);
    assert_eq!(home.read("config/pipelines/tidy.toml").as_deref(), Some(V1));

    let v2 = home.write("v2.toml", V2);
    let run = common::tty::run_answering(
        home.command(&["pipeline", "add", v2.to_str().expect("path")]),
        b"",
        &[("nothing installed", b"2")],
    );
    assert!(run.success, "{}{}", run.screen, run.stderr);
    assert!(
        run.screen
            .contains("your currently installed `tidy` stays live and untouched."),
        "{}",
        run.screen
    );
    assert_eq!(home.read("config/pipelines/tidy.toml").as_deref(), Some(V1));
    assert_eq!(
        home.read("config/pipelines/shadow/tidy.toml").as_deref(),
        Some(V2)
    );

    let other = Home::new();
    let v1 = other.write("v1.toml", V1);
    let run = common::tty::run_answering(
        other.command(&["pipeline", "add", v1.to_str().expect("path")]),
        b"",
        &[("nothing installed", b"3")],
    );
    assert!(!run.success, "{}", run.screen);
    assert!(
        run.screen.contains("\nnothing installed\n"),
        "{}",
        run.screen
    );
    assert_eq!(other.read("config/pipelines/tidy.toml"), None);
}

#[test]
fn promote_checks_the_shadow_and_asks_on_a_terminal() {
    let home = Home::new();
    home.write("config/pipelines/shadow/broke.toml", BROKEN);
    let out = home.fails(&["pipeline", "promote", "broke", "--force"]);
    assert!(
        out.contains("`broke` does not validate — fix it (or --discard) before promoting"),
        "{out}"
    );

    let fine = "[pipeline]\nname = \"fine\"\n";
    home.write("config/pipelines/shadow/fine.toml", fine);
    let out = home.ok(&["pipeline", "promote", "fine", "--force"]);
    assert!(
        out.contains("`fine` is live — see `rtk pipeline show`"),
        "{out}"
    );
    assert!(
        !out.contains("promoting replaces"),
        "nothing was live: {out}"
    );
    assert_eq!(
        home.read("config/pipelines/fine.toml").as_deref(),
        Some(fine)
    );

    // A live folder that cannot take the file stops the move and keeps the shadow.
    home.write(
        "config/pipelines/shadow/stuck.toml",
        "[pipeline]\nname = \"stuck\"\n",
    );
    let live = home.path().join("config/pipelines");
    let mode = |bits| {
        let mut permissions = std::fs::metadata(&live).expect("live dir").permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, bits);
        std::fs::set_permissions(&live, permissions).expect("set mode");
    };
    mode(0o555);
    // Makes the folder writable again even if an assertion below panics, so the
    // temporary home can be removed.
    struct Writable(std::path::PathBuf);
    impl Drop for Writable {
        fn drop(&mut self) {
            if let Ok(metadata) = std::fs::metadata(&self.0) {
                let mut permissions = metadata.permissions();
                std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
                let _ = std::fs::set_permissions(&self.0, permissions);
            }
        }
    }
    let writable = Writable(live.clone());
    // A superuser writes through the mode, so only an ordinary user sees the refusal.
    // CI runs as an ordinary user, so there the refusal must be checked.
    let refused = std::fs::write(live.join("probe"), "").is_err();
    assert!(
        std::env::var_os("CI").is_none() || refused,
        "the read-only folder took a write under CI"
    );
    if refused {
        let out = home.fails(&["pipeline", "promote", "stuck", "--force"]);
        assert!(
            out.contains(&format!(
                "move {} to {}",
                home.path()
                    .join("config/pipelines/shadow/stuck.toml")
                    .display(),
                live.join("stuck.toml").display()
            )),
            "{out}"
        );
        assert!(
            home.read("config/pipelines/shadow/stuck.toml").is_some(),
            "the shadow install stays"
        );
    }
    drop(writable);

    let kept = "[pipeline]\nname = \"kept\"\n";
    home.write("config/pipelines/shadow/kept.toml", kept);
    let run = common::tty::run_answering(
        home.command(&["pipeline", "promote", "kept"]),
        b"",
        &[("promote? [y/N]", b"n\n")],
    );
    assert!(!run.success, "{}", run.screen);
    assert!(
        run.screen.contains("kept in shadow — nothing changed"),
        "{}",
        run.screen
    );
    assert_eq!(home.read("config/pipelines/kept.toml"), None);

    let run = common::tty::run_answering(
        home.command(&["pipeline", "promote", "kept"]),
        b"",
        &[("promote? [y/N]", b"y\n")],
    );
    assert!(run.success, "{}{}", run.screen, run.stderr);
    assert!(run.screen.contains("`kept` is live"), "{}", run.screen);
    assert_eq!(
        home.read("config/pipelines/kept.toml").as_deref(),
        Some(kept)
    );
    assert_eq!(home.read("config/pipelines/shadow/kept.toml"), None);
}

#[test]
fn share_prints_a_card_for_the_active_pipeline_or_a_built_in() {
    let home = Home::new();
    let out = home.rtk(&["pipeline", "share", "nope"]);
    assert_eq!(out.code, Some(1));
    assert!(
        out.stdout.starts_with(
            "unknown pipeline `nope` — shareable: `rtk:default` (active), built-ins: "
        ),
        "{}",
        out.stdout
    );
    let out = home.ok(&["pipeline", "share", "rtk:terse", "--md"]);
    assert!(
        out.contains("name = \"rtk-terse\"\nextends = \"rtk:terse\"\n"),
        "{out}"
    );

    let flags = Home::with_pipeline(FLAGS);
    let run = common::tty::run(flags.command(&["pipeline", "share"]), b"");
    assert!(run.success, "{}", run.stderr);
    for expected in [
        "rtk pipeline · flags",
        "Measured impact",
        "no recorded activity yet — run sessions with this pipeline active",
        "Put your pipeline in everyone's hands — one command:",
        "markdown: rtk pipeline share --md",
    ] {
        assert!(
            run.screen.contains(expected),
            "missing {expected:?} in:\n{}",
            run.screen
        );
    }
    assert!(!run.screen.contains("```toml"), "{}", run.screen);

    // Named, the active pipeline shares every file on disk, the project's
    // included, and the proxy rows stamped with its name.
    flags.write(
        "project/.rtk/pipeline.toml",
        "[pipeline]\nname = \"local\"\n",
    );
    flags.proxy(
        "INSERT INTO usage_records (ts, pipeline, tokens_in, tokens_saved, cache_read_tokens, cache_creation_tokens)
         VALUES ('2026-11-26T00:00:00Z', 'flags', 300, 700, 800, 200);",
    );
    let out = flags.ok(&["pipeline", "share", "flags", "--md"]);
    for expected in [
        "## user pipeline.toml",
        "## project pipeline.toml",
        "| llm proxy input (all time) | 1 requests | `███████░░░` **70%** removed |",
        "| prompt-cache hit rate | 1 requests | `████████░░` **80%** cached |",
        "This card includes multiple configuration sources.",
    ] {
        assert!(out.contains(expected), "missing {expected:?} in:\n{out}");
    }
}

#[test]
fn publishing_a_gist_warns_scans_and_needs_consent() {
    let email = concat!("fixture", "@example.org");
    let leaky = format!(
        "[pipeline]\nname = \"leaky\"\nextends = \"rtk:default\"\n# kept by {email} in /home/fixture/notes\n"
    );
    let home = Home::with_pipeline(&leaky);

    let out = home.fails(&["pipeline", "share", "--gist"]);
    assert!(
        out.contains(
            "publishing non-interactively needs --force — the warning above is yours to accept"
        ),
        "{out}"
    );

    let no_gh = home.bin(false);
    let mut cmd = home.command(&["pipeline", "share", "--gist", "--force"]);
    cmd.env("PATH", &no_gh);
    let out = Home::run(cmd, Stdio::null());
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    for expected in [
        "Publishing is public, permanent, and yours",
        "Possible secrets or private data",
        &format!("email address: {email}"),
        "absolute home path: /home/fixture",
        "--force given — publishing despite the above.",
    ] {
        assert!(
            out.stderr.contains(expected),
            "missing {expected:?} in:\n{}",
            out.stderr
        );
    }
    let card = home.path().join("tmp/leaky.md");
    assert!(
        out.stdout.contains(&format!(
            "  2. create a PUBLIC gist from: {}",
            card.display()
        )),
        "{}",
        out.stdout
    );
    assert!(
        std::fs::read_to_string(&card).is_ok_and(|text| text.contains("# rtk pipeline · leaky")),
        "the card is written for a manual gist"
    );

    let mut cmd = home.command(&["pipeline", "share", "rtk:terse", "--gist", "--force"]);
    cmd.env("PATH", &no_gh);
    let out = Home::run(cmd, Stdio::null());
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    assert!(
        home.path().join("tmp/rtk-terse.md").is_file(),
        "a built-in's name is made file-safe: {}",
        out.stdout
    );

    let failing = Home::with_pipeline("[pipeline]\nname = \"tidy\"\nextends = \"rtk:default\"\n");
    let mut cmd = failing.command(&["pipeline", "share", "--gist", "--force"]);
    cmd.env("PATH", failing.bin(true));
    let out = Home::run(cmd, Stdio::null());
    assert_eq!(out.code, Some(1), "{}", out.stdout);
    assert!(out.stderr.contains("denied by the stub"), "{}", out.stderr);
    assert!(
        out.stderr
            .contains("gh could not create the gist — is `gh auth login` done?"),
        "{}",
        out.stderr
    );

    let run = common::tty::run_answering(
        home.command(&["pipeline", "share", "--gist"]),
        b"",
        &[("continue? [y/N]", b"n\n")],
    );
    assert!(!run.success, "{}", run.screen);
    assert!(run.screen.contains("not published"), "{}", run.screen);

    let run = common::tty::run_answering(
        home.command(&["pipeline", "share", "--gist"]),
        b"",
        &[("continue? [y/N]", b"y\n")],
    );
    assert!(!run.success, "{}", run.screen);
    assert!(
        run.stderr
            .contains("Not published. Remove the above, or re-run with --force to publish anyway."),
        "{}",
        run.stderr
    );
}

#[test]
fn publishing_to_a_private_repository_still_warns_it_is_published() {
    let home = Home::with_pipeline("[pipeline]\nname = \"tidy\"\nextends = \"rtk:default\"\n");
    let run = common::tty::run_answering(
        home.command(&["pipeline", "share", "publish"]),
        b"",
        &[("no discovery", b"2"), ("continue? [y/N]", b"n\n")],
    );
    assert!(!run.success, "{}", run.screen);
    assert!(
        run.stderr.contains("Private, but still published"),
        "{}",
        run.stderr
    );
    assert!(
        run.stderr
            .contains("This creates a PRIVATE repo: only accounts you grant access can see it."),
        "{}",
        run.stderr
    );
    assert!(run.screen.contains("not published"), "{}", run.screen);
}
