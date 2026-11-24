//! PostToolUse output filtering for Claude Code — the `hook.after_tool`
//! adapter.
//!
//! The tool runs raw; Claude Code sends the event JSON on stdin; this module
//! maps the tool name to its agnostic subject, evaluates the resolved
//! pipeline's `hook.after_tool.<subject>` rules, runs the winning chain
//! through `rtk-engine`, and returns `hookSpecificOutput.updatedToolOutput`.
//! Everything that knows the words "claude", "Read", "tool_response" lives
//! here — engine methods stay agent-agnostic.
//!
//! Protocol invariants:
//! - **Fail-open everywhere**: any error emits nothing and exits 0; Claude
//!   Code keeps the original output. A broken pipeline file falls back to
//!   `rtk:default` (no after_tool rules → passthrough) and surfaces via
//!   pipeline-status.json, never in the session.
//! - The only stdout write is the final serialized `hookSpecificOutput`.
//! - Never emit an unchanged or larger output — emit nothing instead.
//! - One `Config::load()` + one pipeline load per invocation.

use super::constants::POST_TOOL_USE_KEY;
use crate::core::config::Config;
use crate::core::pipeline_config::{self, LoadedPipeline};
use crate::core::pipeline_log;
use crate::core::tee;
use crate::core::tracking::{Tracker, estimate_tokens};
use regex::Regex;
use rtk_engine::block::{Block, ContentFormat, Event, Origin, Provenance, Subject};
use rtk_engine::method::{Method, MethodCtx};
use rtk_engine::pipeline::{Action, DecisionKind, Fact, Facts, Position, Resolved, evaluate};
use serde_json::{Value, json};
use std::io::{self, Write};
use std::sync::LazyLock;

/// Read→Edit fidelity guard, emitted as `additionalContext` whenever a Read
/// output was format-converted (content no longer textually identical to the
/// on-disk file).
const READ_CONVERSION_NOTE: &str = "RTK compressed this Read output (format conversion, \
     e.g. json->toon, lockfile summary). The on-disk file is unchanged; re-read with rtk \
     disabled or check the recall file before constructing Edit old_string values.";

fn apply_terminal(
    text: String,
    ctx: &MethodCtx,
    subject: Subject,
    format: ContentFormat,
) -> String {
    let Some(redact) = rtk_engine::registry::redact() else {
        return text;
    };
    let origin = Origin::default();
    let provenance = Provenance::default();
    let block = Block {
        text: &text,
        subject,
        format,
        origin: &origin,
        provenance: &provenance,
    };
    rtk_engine::chain::run_chain(&[redact], &block, ctx)
}

/// Entry point: never panics, never errors, emits at most one stdout line.
/// The decision log flushes only after that line — it never sits in front
/// of the response.
pub fn run(event: &Value) {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| process(event)));
    if let Ok(Some((output, loaded))) = outcome {
        let _ = writeln!(io::stdout(), "{output}");
        pipeline_config::write_status(&loaded);
        pipeline_log::flush();
    } else if let Ok(None) = outcome {
        pipeline_log::flush();
    }
}

/// Full pipeline behind the fail-open envelope. `None` = emit nothing.
fn process(event: &Value) -> Option<(String, LoadedPipeline)> {
    if crate::core::user_env::var("RTK_DISABLED").as_deref() == Some("1") {
        return None;
    }
    if crate::core::user_env::var("RTK_POSTHOOK").as_deref() == Some("0") {
        return None;
    }

    let config = Config::load().unwrap_or_default();
    let loaded = pipeline_config::load();
    let output = process_with_config(event, &config, &loaded.resolved)?;
    Some((output, loaded))
}

/// The agent whose PostToolUse payload this process is handling.
///
/// Only Claude Code dispatches a posthook today, but the tool-name vocabulary
/// is per-agent — `Read` on Claude Code, `view` on Copilot, `view_file` on
/// Antigravity all mean the same subject — so the lookup takes the agent rather
/// than assuming one. `rtk hook <agent>` sets it; the default keeps the
/// existing behaviour for any caller that does not.
fn subject_of(tool_name: &str) -> Subject {
    subject_of_for(
        super::hook_agent().as_deref().unwrap_or("claude-code"),
        tool_name,
    )
}

fn subject_of_for(agent: &str, tool_name: &str) -> Subject {
    rtk_engine::harness::profile(agent)
        .map(|p| p.map_tool(tool_name))
        .unwrap_or(Subject::Any)
}

/// Testable core: the filtered output, or a repeat pointer for a Read, Grep
/// or Glob result.
fn process_with_config(event: &Value, config: &Config, pipeline: &Resolved) -> Option<String> {
    let emitted = filter_tool_output(event, config, pipeline);
    match event.get("tool_name").and_then(Value::as_str) {
        Some("Read" | "Grep" | "Glob") => deliver_native(config, event, emitted),
        _ => emitted,
    }
}

/// Gates → extract → classify → evaluate rules → guard → re-wrap.
fn filter_tool_output(event: &Value, config: &Config, pipeline: &Resolved) -> Option<String> {
    let tool_name = event.get("tool_name")?.as_str()?;
    let subject = subject_of(tool_name);

    let source = source_of(event, tool_name);
    let duration_ms = event
        .get("duration_ms")
        .and_then(|d| d.as_u64())
        .unwrap_or(0);

    if tool_name == "WebSearch" {
        return process_websearch(event, config, pipeline, &source, duration_ms);
    }
    if subject == Subject::Shell {
        return process_shell(event, config, pipeline, duration_ms);
    }

    let (raw, format) = extract(event, tool_name, &source)?;
    if raw.len() < pipeline.guards.min_size {
        return None;
    }

    let Some(format) = format else {
        // Unsupported shape (e.g. Grep count mode): passthrough, but tracked
        // so coverage stays visible in `rtk gain`.
        track(tool_name, "raw", &source, &raw, &raw, duration_ms);
        return None;
    };

    let composed = filter_and_compose(
        config,
        pipeline,
        subject,
        tool_name,
        &source,
        &raw,
        format,
        duration_ms,
    )?;

    let (updated, converted) = rebuild(event, tool_name, &composed)?;
    let mut hook_output = json!({
        "hookEventName": POST_TOOL_USE_KEY,
        "updatedToolOutput": updated,
    });
    if converted {
        hook_output["additionalContext"] = json!(READ_CONVERSION_NOTE);
    }

    track(
        tool_name,
        format.as_str(),
        &source,
        &raw,
        &composed,
        duration_ms,
    );
    Some(json!({ "hookSpecificOutput": hook_output }).to_string())
}

fn facts_for(
    subject: Subject,
    format: ContentFormat,
    source: &str,
    raw: &str,
    duration_ms: u64,
    is_rtk_command: Option<bool>,
) -> Facts {
    let mut facts = Facts::new();
    facts.insert("tool".into(), Fact::Str(subject.as_str().to_string()));
    facts.insert("format".into(), Fact::Str(format.as_str().to_string()));
    facts.insert("bytes".into(), Fact::Num(raw.len() as f64));
    facts.insert("lines".into(), Fact::Num(raw.lines().count() as f64));
    facts.insert("tokens".into(), Fact::Num(estimate_tokens(raw) as f64));
    facts.insert("duration_ms".into(), Fact::Num(duration_ms as f64));
    if let Some(value) = is_rtk_command {
        facts.insert("is_rtk_command".into(), Fact::Bool(value));
    }
    if !source.is_empty() {
        if source.starts_with("http://") || source.starts_with("https://") {
            facts.insert("url".into(), Fact::Str(source.to_string()));
        } else if subject == Subject::Shell {
            facts.insert("command".into(), Fact::Str(source.to_string()));
        } else {
            facts.insert("file_path".into(), Fact::Str(source.to_string()));
            facts.insert(
                "extension".into(),
                Fact::Str(extension_of(source).to_string()),
            );
            facts.insert("basename".into(), Fact::Str(basename_of(source)));
        }
    }
    facts
}

struct Applied {
    text: String,
    methods: Vec<&'static str>,
    rules: Vec<String>,
    pipeline: String,
    subject: Subject,
    shadow: bool,
}

fn log_entry_for(
    subject: Subject,
    command: Option<&str>,
    source: &str,
    applied: &Applied,
    before: usize,
    after: usize,
) -> pipeline_log::LogEntry {
    pipeline_log::LogEntry {
        ts: pipeline_log::now_ts(),
        session: None,
        site: "hook".to_string(),
        event: "after_tool".to_string(),
        subject: subject.as_str().to_string(),
        tool: Some(subject.as_str().to_string()),
        command: command
            .map(str::to_string)
            .or_else(|| (!source.is_empty()).then(|| source.to_string())),
        rule: applied.rules.join("+"),
        action: None,
        methods: applied.methods.iter().map(|m| m.to_string()).collect(),
        tokens_before: Some(before),
        tokens_after: Some(after),
        shadow: applied.shadow,
        recovery: None,
    }
}

fn queue_log(
    subject: Subject,
    command: Option<&str>,
    source: &str,
    applied: &Applied,
    before: usize,
    after: usize,
) {
    if !pipeline_log::enabled() {
        return;
    }
    pipeline_log::queue(log_entry_for(
        subject, command, source, applied, before, after,
    ));
}

#[allow(clippy::too_many_arguments)]
fn run_selected(
    pipeline: &Resolved,
    subject: Subject,
    tool_name: &str,
    format: ContentFormat,
    source: &str,
    raw: &str,
    duration_ms: u64,
    is_rtk_command: Option<bool>,
) -> Option<Applied> {
    let mut facts = facts_for(subject, format, source, raw, duration_ms, is_rtk_command);
    if subject == Subject::Mcp
        && let Some(rest) = tool_name.strip_prefix("mcp__")
    {
        let server = rest.split("__").next().unwrap_or(rest);
        if !server.is_empty() {
            facts.insert("server".into(), Fact::Str(server.to_string()));
        }
    }
    let position = Position::hook(Event::AfterTool, subject);
    crate::core::pipeline_config::insert_call_context(pipeline, position, &mut facts);
    let call = crate::core::pipeline_config::call_state(&facts, None);
    crate::core::classify::insert_classifier_facts(
        pipeline,
        position,
        raw,
        Some(&call),
        &mut facts,
    );
    shadow_eval_candidate(subject, format, source, raw, &facts);
    let decisions = evaluate(pipeline, position, &facts);
    if decisions.is_empty() {
        return None;
    }
    let mut methods: Vec<&'static dyn Method> = Vec::new();
    let mut method_names: Vec<&'static str> = Vec::new();
    let mut rules: Vec<String> = Vec::new();
    let mut shadow = false;
    for decision in &decisions {
        match &decision.kind {
            DecisionKind::Transform {
                methods: selected, ..
            } => {
                methods.extend_from_slice(selected);
                method_names.extend(selected.iter().map(|m| m.name()));
                rules.push(decision.rule.name.clone());
                if decision.shadow() {
                    shadow = true;
                }
            }
            DecisionKind::Act(Action::Route) => {
                let task = decision.rule.def.task.as_deref()?;
                let label = if source.is_empty() { tool_name } else { source };
                let text = crate::core::task_cmd::digest_output(task, raw, label)?;
                return Some(Applied {
                    text,
                    methods: vec!["digest"],
                    rules: vec![decision.rule.name.clone()],
                    pipeline: pipeline.name.clone(),
                    subject,
                    shadow: decision.shadow(),
                });
            }
            DecisionKind::Act(_) => return None,
        }
    }
    if methods.is_empty() {
        return None;
    }
    let ctx = MethodCtx::new(source);
    let origin = Origin {
        tool: Some(tool_name.to_string()),
        path: (!source.is_empty()).then(|| source.to_string()),
        command: None,
        block_index: None,
    };
    let provenance = rtk_engine::marker::provenance_of(raw);
    let block = Block {
        text: raw,
        subject,
        format,
        origin: &origin,
        provenance: &provenance,
    };
    // Panic containment per chain rather than per invocation. `run` catches
    // panics too, but there it costs the whole response: this returns None,
    // the caller passes the raw output through, and tracking and the decision
    // log still run. The size guard stays with the caller, which needs the
    // unguarded text to record what the chain actually did.
    let text = rtk_engine::chain::run_chain_caught(&methods, &block, &ctx)?;
    Some(Applied {
        text,
        methods: method_names,
        rules,
        pipeline: pipeline.name.clone(),
        subject,
        shadow,
    })
}

fn shadow_eval_candidate(
    subject: Subject,
    format: ContentFormat,
    source: &str,
    raw: &str,
    facts: &Facts,
) {
    let Some((name, loaded)) = crate::core::pipeline_config::load_eval_candidate() else {
        return;
    };
    let mut facts = facts.clone();
    facts.retain(|k, _| {
        !k.starts_with("class.") && !k.starts_with("score.") && !k.starts_with("noul.")
    });
    let position = Position::hook(Event::AfterTool, subject);
    let decisions = evaluate(&loaded.resolved, position, &facts);
    let mut methods: Vec<&'static dyn Method> = Vec::new();
    let mut method_names: Vec<&'static str> = Vec::new();
    let mut rules: Vec<String> = Vec::new();
    for decision in &decisions {
        if let DecisionKind::Transform {
            methods: selected, ..
        } = &decision.kind
        {
            methods.extend_from_slice(selected);
            method_names.extend(selected.iter().map(|m| m.name()));
            rules.push(decision.rule.name.clone());
        }
    }
    if methods.is_empty() {
        return;
    }
    let ctx = MethodCtx::new(source);
    let origin = Origin {
        tool: None,
        path: (!source.is_empty()).then(|| source.to_string()),
        command: None,
        block_index: None,
    };
    let provenance = rtk_engine::marker::provenance_of(raw);
    let block = Block {
        text: raw,
        subject,
        format,
        origin: &origin,
        provenance: &provenance,
    };
    let Some(text) = rtk_engine::chain::run_chain_caught(&methods, &block, &ctx) else {
        return;
    };
    let applied = Applied {
        text,
        methods: method_names,
        rules,
        pipeline: name,
        subject,
        shadow: true,
    };
    attribute(&applied, raw, &applied.text);
}

#[cfg(not(test))]
fn tracker() -> anyhow::Result<Tracker> {
    Tracker::new()
}

#[cfg(test)]
fn tracker() -> anyhow::Result<Tracker> {
    Tracker::new_in_memory()
}

fn attribute(applied: &Applied, raw: &str, emitted: &str) {
    let Ok(tracker) = tracker() else {
        return;
    };
    let tokens_in = estimate_tokens(raw);
    let tokens_out = estimate_tokens(emitted);
    let kind = if applied.shadow { "shadow" } else { "hook" };
    for method in &applied.methods {
        let _ = tracker.record_attribution(kind, "method", method, tokens_in, tokens_out);
    }
    for rule in &applied.rules {
        let _ = tracker.record_attribution(kind, "rule", rule, tokens_in, tokens_out);
    }
    if !applied.pipeline.is_empty() {
        let _ =
            tracker.record_attribution(kind, "pipeline", &applied.pipeline, tokens_in, tokens_out);
        let slice =
            crate::core::tracking::pipeline_slice_key(&applied.pipeline, applied.subject.as_str());
        let _ = tracker.record_attribution(kind, "pipeline-slice", &slice, tokens_in, tokens_out);
    }
}

static RECALL_SLUG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"posthook/\d+_\d+-\d+_([a-z]+)").expect("valid regex"));

static RTK_RECALL_COMMAND: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|[|&;(\s])rtk\s+recall\b").expect("valid regex"));

fn record_recovery(command: &str) {
    let tool = RECALL_SLUG
        .captures(command)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| {
            if RTK_RECALL_COMMAND.is_match(command) {
                "recall".to_string()
            } else {
                "unknown".to_string()
            }
        });
    if let Ok(tracker) = tracker() {
        let _ = tracker.record_attribution("hook", "recovery", &tool, 0, 0);
    }
}

/// Chain + recall store + guard for a single content string.
/// `None` = no matching rule, chain was a no-op, not smaller, or shadow.
#[allow(clippy::too_many_arguments)]
fn filter_and_compose(
    config: &Config,
    pipeline: &Resolved,
    subject: Subject,
    tool_name: &str,
    source: &str,
    raw: &str,
    format: ContentFormat,
    duration_ms: u64,
) -> Option<String> {
    let applied = run_selected(
        pipeline,
        subject,
        tool_name,
        format,
        source,
        raw,
        duration_ms,
        None,
    )?;
    let filtered = applied.text.clone();

    if filtered == raw {
        // No-op chain: skip the recall store entirely.
        track(tool_name, format.as_str(), source, raw, raw, duration_ms);
        return None;
    }

    // Ordering invariant (emit_guarded pattern): compose filtered + hint
    // FIRST, then guard — the hint can never push output above raw.
    let slug = format!("{}_{}", tool_name.to_lowercase(), basename_of(source));
    let hint = tee::posthook_tee_hint(config, raw, &slug);
    let composed = match hint {
        Some(h) => format!("{filtered}\n{h}"),
        None => filtered,
    };

    if estimate_tokens(&composed) > estimate_tokens(raw) {
        track(tool_name, format.as_str(), source, raw, raw, duration_ms);
        return None;
    }
    attribute(&applied, raw, &composed);
    queue_log(
        subject,
        None,
        source,
        &applied,
        estimate_tokens(raw),
        estimate_tokens(&composed),
    );
    if applied.shadow {
        track(tool_name, format.as_str(), source, raw, raw, duration_ms);
        return None;
    }
    let ctx = MethodCtx::new(source);
    Some(apply_terminal(composed, &ctx, subject, format))
}

/// WebSearch responses carry text as string elements inside a mixed
/// `results` array (strings + link objects). Each string element runs the
/// web rules; the array is rebuilt in place with link objects untouched.
fn process_websearch(
    event: &Value,
    config: &Config,
    pipeline: &Resolved,
    source: &str,
    duration_ms: u64,
) -> Option<String> {
    let results = event.pointer("/tool_response/results")?.as_array()?;

    let raw_concat: String = results
        .iter()
        .filter_map(|r| r.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    if raw_concat.len() < pipeline.guards.min_size {
        return None;
    }

    let mut changed = false;
    let mut shadow = false;
    let mut last_applied: Option<Applied> = None;
    let mut new_results: Vec<Value> = Vec::with_capacity(results.len());
    let mut filtered_concat = String::new();
    for r in results {
        match r.as_str() {
            Some(text) => {
                let filtered = match run_selected(
                    pipeline,
                    Subject::Web,
                    "WebSearch",
                    ContentFormat::Web,
                    source,
                    text,
                    duration_ms,
                    None,
                ) {
                    Some(applied) => {
                        let text_out = applied.text.clone();
                        shadow |= applied.shadow;
                        last_applied = Some(applied);
                        text_out
                    }
                    None => text.to_string(),
                };
                if filtered != text {
                    changed = true;
                }
                if !filtered_concat.is_empty() {
                    filtered_concat.push_str("\n\n");
                }
                filtered_concat.push_str(&filtered);
                new_results.push(Value::String(filtered));
            }
            None => new_results.push(r.clone()),
        }
    }

    if !changed || shadow {
        if let Some(applied) = &last_applied
            && shadow
        {
            attribute(applied, &raw_concat, &filtered_concat);
        }
        track(
            "WebSearch",
            "web",
            source,
            &raw_concat,
            &raw_concat,
            duration_ms,
        );
        return None;
    }

    // Compose-then-guard: hint rides in the last string element.
    let hint = tee::posthook_tee_hint(config, &raw_concat, "websearch_results");
    if let (Some(h), Some(last)) = (
        hint,
        new_results.iter_mut().rev().find_map(|v| match v {
            Value::String(s) => Some(s),
            _ => None,
        }),
    ) {
        last.push('\n');
        last.push_str(&h);
        filtered_concat.push('\n');
        filtered_concat.push_str(last);
    }

    if estimate_tokens(&filtered_concat) > estimate_tokens(&raw_concat) {
        track(
            "WebSearch",
            "web",
            source,
            &raw_concat,
            &raw_concat,
            duration_ms,
        );
        return None;
    }

    let mut updated = event.get("tool_response")?.clone();
    updated["results"] = Value::Array(new_results);

    if let Some(applied) = &last_applied {
        attribute(applied, &raw_concat, &filtered_concat);
        queue_log(
            Subject::Web,
            None,
            source,
            applied,
            estimate_tokens(&raw_concat),
            estimate_tokens(&filtered_concat),
        );
    }
    track(
        "WebSearch",
        "web",
        source,
        &raw_concat,
        &filtered_concat,
        duration_ms,
    );
    Some(
        json!({
            "hookSpecificOutput": {
                "hookEventName": POST_TOOL_USE_KEY,
                "updatedToolOutput": updated,
            }
        })
        .to_string(),
    )
}

/// `rtk` invoked anywhere in the command (start or after a shell operator):
/// feeds the `is_rtk_command` condition — the shipped `term-default` rule
/// skips such output because it already went through RTK's own filters.
static RTK_IN_COMMAND: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|[|&;(\s])rtk\s").expect("valid regex"));

fn combine_streams(stdout: &str, stderr: &str) -> String {
    if stderr.is_empty() {
        stdout.to_string()
    } else {
        format!("{stdout}\n--- stderr ---\n{stderr}")
    }
}

fn process_shell(
    event: &Value,
    config: &Config,
    pipeline: &Resolved,
    duration_ms: u64,
) -> Option<String> {
    let response = event.get("tool_response")?;
    if response
        .get("interrupted")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || response
            .get("isImage")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        return None;
    }

    let command = event
        .pointer("/tool_input/command")
        .and_then(|c| c.as_str())
        .unwrap_or("");
    let is_rtk_command = RTK_IN_COMMAND.is_match(command);
    // Recall-recovery gate: a command reading the recall store must see the
    // raw bytes (else e.g. an elided base64 blob could never be recovered).
    if command.contains("tee/posthook")
        || RTK_RECALL_COMMAND.is_match(command)
        || tee::posthook_recall_dir(config)
            .is_some_and(|d| command.contains(&d.display().to_string()))
    {
        record_recovery(command);
        return None;
    }

    let stdout = response
        .get("stdout")
        .and_then(|s| s.as_str())
        .unwrap_or("");
    let stderr = response
        .get("stderr")
        .and_then(|s| s.as_str())
        .unwrap_or("");
    if stdout.len() + stderr.len() < pipeline.guards.min_size {
        return None;
    }

    let first_word = command.split_whitespace().next().unwrap_or("");
    let applied_stdout = run_selected(
        pipeline,
        Subject::Shell,
        "Bash",
        ContentFormat::Term,
        command,
        stdout,
        duration_ms,
        Some(is_rtk_command),
    );
    let shadow = applied_stdout.as_ref().map(|a| a.shadow).unwrap_or(false);
    let filtered_stdout = applied_stdout
        .as_ref()
        .map(|a| a.text.clone())
        .unwrap_or_else(|| stdout.to_string());
    let filtered_stderr = if stderr.is_empty() {
        String::new()
    } else {
        run_selected(
            pipeline,
            Subject::Shell,
            "Bash",
            ContentFormat::Term,
            command,
            stderr,
            duration_ms,
            Some(is_rtk_command),
        )
        .map(|a| a.text)
        .unwrap_or_else(|| stderr.to_string())
    };

    let raw_combined = combine_streams(stdout, stderr);

    if (filtered_stdout == stdout && filtered_stderr == stderr) || shadow {
        if shadow && let Some(applied) = &applied_stdout {
            attribute(applied, &raw_combined, &filtered_stdout);
            queue_log(
                Subject::Shell,
                Some(command),
                "",
                applied,
                estimate_tokens(&raw_combined),
                estimate_tokens(&filtered_stdout),
            );
        }
        track(
            "Bash",
            "term",
            first_word,
            &raw_combined,
            &raw_combined,
            duration_ms,
        );
        return deliver(config, event, response, command, stdout, stderr, None);
    }

    let slug = format!("bash_{first_word}");
    let hint = tee::posthook_tee_hint(config, &raw_combined, &slug);
    let composed_stdout = match hint {
        Some(h) => format!("{filtered_stdout}\n{h}"),
        None => filtered_stdout,
    };

    let emitted_combined = combine_streams(&composed_stdout, &filtered_stderr);
    if estimate_tokens(&emitted_combined) > estimate_tokens(&raw_combined) {
        track(
            "Bash",
            "term",
            first_word,
            &raw_combined,
            &raw_combined,
            duration_ms,
        );
        return deliver(config, event, response, command, stdout, stderr, None);
    }

    let mut updated = response.clone();
    let obj = updated.as_object_mut()?;
    obj.insert("stdout".into(), json!(composed_stdout));
    obj.insert("stderr".into(), json!(filtered_stderr));

    if let Some(applied) = &applied_stdout {
        attribute(applied, &raw_combined, &emitted_combined);
        queue_log(
            Subject::Shell,
            Some(command),
            "",
            applied,
            estimate_tokens(&raw_combined),
            estimate_tokens(&emitted_combined),
        );
    }
    track(
        "Bash",
        "term",
        first_word,
        &raw_combined,
        &emitted_combined,
        duration_ms,
    );
    let emitted = json!({
        "hookSpecificOutput": {
            "hookEventName": POST_TOOL_USE_KEY,
            "updatedToolOutput": updated,
        }
    })
    .to_string();
    deliver(
        config,
        event,
        response,
        command,
        &composed_stdout,
        &filtered_stderr,
        Some(emitted),
    )
}

/// Tools whose repeated results can become pointers.
const REPEAT_TOOLS: &[&str] = &["Bash", "Read", "Grep", "Glob"];

/// Where each agent's last deliveries in a session are remembered.
fn repeat_dir(config: &Config, event: &Value) -> Option<std::path::PathBuf> {
    let session = event
        .get("session_id")?
        .as_str()
        .filter(|s| !s.is_empty())?;
    Some(
        tee::posthook_recall_dir(config)?
            .join("repeat")
            .join(short_hash(session)),
    )
}

/// An agent's repeat state in a session's folder.
struct AgentFiles {
    dir: std::path::PathBuf,
    /// The agent's name in the folder; its mark is `<agent>.run`.
    agent: String,
    /// The record this call compares against: the agent's last completed
    /// Bash call (`<agent>`), or its last completed Read, Grep or Glob call
    /// with this same input (`<agent>.<input digest>`).
    record: String,
}

impl AgentFiles {
    fn record(&self) -> std::path::PathBuf {
        self.dir.join(&self.record)
    }

    fn mark(&self) -> std::path::PathBuf {
        self.dir.join(format!("{}.run", self.agent))
    }

    /// A mark holds the id of the call in flight (empty: none matches),
    /// then the records to clear if it is found stale. A mark written before
    /// Read, Grep and Glob were covered holds the id alone and stands for the
    /// Bash record.
    fn records_of(&self, mark: &str) -> Vec<String> {
        let mut lines = mark.split('\n');
        lines.next();
        let names: Vec<String> = lines
            .filter(|name| self.is_record_name(name))
            .map(str::to_string)
            .collect();
        if names.is_empty() {
            vec![self.agent.clone()]
        } else {
            names
        }
    }

    fn is_record_name(&self, name: &str) -> bool {
        name.strip_prefix(&self.agent).is_some_and(|rest| {
            rest.is_empty()
                || rest
                    .strip_prefix('.')
                    .is_some_and(|d| d.len() == 16 && d.bytes().all(|b| b.is_ascii_hexdigit()))
        })
    }

    /// Clears `names` and writes the mark: this call's, when every one is
    /// gone, else an empty id naming the records still to clear.
    fn settle(&self, id: &str, names: Vec<String>) {
        let mut left: Vec<String> = Vec::new();
        for name in names {
            if !left.contains(&name) && !clear(&self.dir.join(&name)) {
                left.push(name);
            }
        }
        let body = if left.is_empty() {
            mark_body(id, std::slice::from_ref(&self.record))
        } else {
            mark_body("", &left)
        };
        self.write_mark(&body);
    }

    /// The record may still be there: no call matches the mark until a
    /// start has cleared it.
    fn poison(&self) {
        let mut names = std::fs::read_to_string(self.mark())
            .map(|mark| self.records_of(&mark))
            .unwrap_or_default();
        if !names.contains(&self.record) {
            names.push(self.record.clone());
        }
        self.write_mark(&mark_body("", &names));
    }

    /// A mark that cannot be rewritten is removed, so the call it named
    /// matches nothing.
    fn write_mark(&self, body: &str) {
        if std::fs::write(self.mark(), body).is_err() {
            let _ = std::fs::remove_file(self.mark());
        }
    }
}

fn mark_body(id: &str, records: &[String]) -> String {
    let mut body = id.to_string();
    for name in records {
        body.push('\n');
        body.push_str(name);
    }
    body
}

fn mark_id(mark: &str) -> &str {
    mark.split('\n').next().unwrap_or("")
}

/// `agent_id` is sent only from a subagent.
fn agent_files(config: &Config, event: &Value) -> Option<AgentFiles> {
    let agent = short_hash(
        event
            .get("agent_id")
            .and_then(Value::as_str)
            .unwrap_or("main"),
    );
    let record = match event.get("tool_name").and_then(Value::as_str) {
        Some("Bash") => agent.clone(),
        _ => format!("{agent}.{}", short_hash(&input_key(event))),
    };
    Some(AgentFiles {
        dir: repeat_dir(config, event)?,
        agent,
        record,
    })
}

/// A Read, Grep or Glob call's tool and arguments.
fn input_key(event: &Value) -> String {
    format!(
        "{}\0{}",
        event
            .get("tool_name")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        event.get("tool_input").unwrap_or(&Value::Null)
    )
}

/// Pointers need the option, recall storage that accepts writes, the Claude
/// Code hook and a Bash, Read, Grep or Glob call.
fn dedup_active(config: &Config, event: &Value) -> bool {
    config.hooks.dedup_repeats
        && matches!(
            config.retriever.mode,
            crate::core::retriever::RecoveryMode::Sqlite
        )
        && !crate::core::retriever::recovery_disabled_by_env()
        && super::hook_agent().as_deref().unwrap_or("claude-code") == "claude-code"
        && event
            .get("tool_name")
            .and_then(Value::as_str)
            .is_some_and(|tool| REPEAT_TOOLS.contains(&tool))
}

fn tool_use_id(event: &Value) -> &str {
    event
        .get("tool_use_id")
        .and_then(Value::as_str)
        .unwrap_or("")
}

fn short_hash(text: &str) -> String {
    full_hash(text)[..16].to_string()
}

/// The whole SHA-256 digest: equal digests stand for equal bytes.
fn full_hash(text: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A session that starts, resumes, clears or compacts has lost the outputs a
/// pointer would refer to.
pub fn forget_repeats(event: &Value) {
    if let Ok(config) = Config::load() {
        forget_repeats_with(&config, event);
    }
}

/// PreToolUse of a Bash, Read, Grep or Glob call. A call still marked never
/// reached PostToolUse (it failed, was denied or interrupted, or its hook
/// died), or is still running beside this one, so the records it and this
/// call compare against no longer hold what the agent last got. Then this
/// call is marked.
pub fn note_tool_start(event: &Value) {
    if event
        .get("tool_name")
        .and_then(Value::as_str)
        .is_some_and(|tool| REPEAT_TOOLS.contains(&tool))
    {
        note_tool_start_with(&crate::core::config::cached_config(), event);
    }
}

fn note_tool_start_with(config: &Config, event: &Value) {
    if !dedup_active(config, event) {
        return;
    }
    let Some(files) = agent_files(config, event) else {
        return;
    };
    let id = tool_use_id(event);
    let mark = files.mark();
    // A session's first call prunes, for installs without SessionStart.
    if !files.dir.exists() && crate::core::utils::create_private_dir(&files.dir).is_ok() {
        prune_repeats(&files.dir, std::time::SystemTime::now());
    }
    // Creating the mark is the check: of 2 parallel starts, the second finds
    // it and clears both calls' records.
    let created = crate::core::utils::create_private_dir(&files.dir).is_ok()
        && std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&mark)
            .and_then(|mut file| {
                std::io::Write::write_all(
                    &mut file,
                    mark_body(id, std::slice::from_ref(&files.record)).as_bytes(),
                )
            })
            .is_ok();
    if !created {
        let mut names = files.records_of(&std::fs::read_to_string(&mark).unwrap_or_default());
        names.push(files.record.clone());
        files.settle(id, names);
    }
}

/// Deletes a record; false when one may still be there.
fn clear(record: &std::path::Path) -> bool {
    match std::fs::remove_file(record) {
        Ok(()) => true,
        Err(e) => e.kind() == std::io::ErrorKind::NotFound,
    }
}

fn forget_repeats_with(config: &Config, event: &Value) {
    if let Some(dir) = repeat_dir(config, event) {
        let _ = std::fs::remove_dir_all(&dir);
        prune_repeats(&dir, std::time::SystemTime::now());
    }
}

/// A session folder untouched this long belongs to a finished session.
const REPEAT_SESSION_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 3600);
/// A taken mark (`<agent>.run.<pid>`) lives for one PostToolUse; one this old
/// was left by a hook that died.
const TAKEN_MARK_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(3600);

/// Removes other sessions' folders untouched for `REPEAT_SESSION_MAX_AGE`
/// and taken marks older than `TAKEN_MARK_MAX_AGE`. `live` (the session
/// being started or used) is never removed. Removing state only ever makes a
/// result delivered whole.
fn prune_repeats(live: &std::path::Path, now: std::time::SystemTime) {
    let Some(entries) = live.parent().and_then(|root| std::fs::read_dir(root).ok()) else {
        return;
    };
    for entry in entries.flatten() {
        // Not followed: a link is not a session's folder.
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let path = entry.path();
        if path != live && older_than(&path, now, REPEAT_SESSION_MAX_AGE) {
            let _ = std::fs::remove_dir_all(&path);
        } else {
            prune_taken_marks(&path, now);
        }
    }
}

fn prune_taken_marks(dir: &std::path::Path, now: std::time::SystemTime) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let taken = entry.file_name().to_str().is_some_and(|name| {
            name.split_once(".run.").is_some_and(|(agent, pid)| {
                agent.len() == 16
                    && agent.bytes().all(|b| b.is_ascii_hexdigit())
                    && !pid.is_empty()
                    && pid.bytes().all(|b| b.is_ascii_digit())
            })
        });
        if taken && older_than(&entry.path(), now, TAKEN_MARK_MAX_AGE) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn older_than(
    path: &std::path::Path,
    now: std::time::SystemTime,
    age: std::time::Duration,
) -> bool {
    std::fs::symlink_metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|modified| now.duration_since(modified).ok())
        .is_some_and(|elapsed| elapsed >= age)
}

/// Hands back what the agent gets for this Bash call (`emitted`, or `None`
/// for the command's own output), or a pointer when `[hooks] dedup_repeats`
/// is on and the agent's last completed Bash call ran the same command and
/// got these same bytes.
fn deliver(
    config: &Config,
    event: &Value,
    response: &Value,
    command: &str,
    stdout: &str,
    stderr: &str,
    emitted: Option<String>,
) -> Option<String> {
    if !dedup_active(config, event) {
        return emitted;
    }
    let delivered = combine_streams(stdout, stderr);
    deliver_seen(
        config,
        event,
        Some((command, delivered.as_str())),
        emitted,
        || pointer(config, response, &delivered),
    )
}

/// Read, Grep and Glob: hands back what the agent gets (`emitted`, or `None`
/// for the tool's own result), or a pointer when the agent's last completed
/// call of the same tool with the same arguments got this same result. The
/// result is compared whole, so a file that changed is delivered whole.
fn deliver_native(config: &Config, event: &Value, emitted: Option<String>) -> Option<String> {
    if !dedup_active(config, event) {
        return emitted;
    }
    let envelope = emitted
        .as_deref()
        .and_then(|out| serde_json::from_str::<Value>(out).ok());
    let response = match &emitted {
        Some(_) => envelope
            .as_ref()
            .and_then(|v| v.pointer("/hookSpecificOutput/updatedToolOutput").cloned()),
        None => event.get("tool_response").cloned(),
    };
    let text = response.as_ref().and_then(native_text);
    // Everything the agent is shown but the call's duration.
    let shown = response.as_ref().filter(|_| text.is_some()).map(|r| {
        let mut r = r.clone();
        if let Some(obj) = r.as_object_mut() {
            obj.remove("durationMs");
        }
        r.to_string()
    });
    let key = input_key(event);
    let seen = shown.as_deref().map(|shown| (key.as_str(), shown));
    deliver_seen(config, event, seen, emitted, || {
        native_pointer(config, event, envelope, response?, &text?)
    })
}

/// A call counts as completed only when its PreToolUse marked it and nothing
/// else started since; any doubt delivers the output. `seen` is the call's
/// input and what the agent was shown (`None`: nothing to compare). Claude
/// Code sends `agent_id` only from a subagent, so its absence is the main
/// thread; other hosts are left alone.
fn deliver_seen(
    config: &Config,
    event: &Value,
    seen: Option<(&str, &str)>,
    emitted: Option<String>,
    pointer: impl FnOnce() -> Option<String>,
) -> Option<String> {
    if !dedup_active(config, event) {
        return emitted;
    }
    let Some(files) = agent_files(config, event) else {
        return emitted;
    };
    let (record, mark) = (files.record(), files.mark());
    let id = tool_use_id(event);
    if id.is_empty()
        || std::fs::read_to_string(&mark)
            .ok()
            .is_none_or(|body| mark_id(&body) != id)
    {
        if !clear(&record) {
            files.poison();
        }
        return emitted;
    }
    let mut sound = true;
    let repeat = match seen.map(|(key, shown)| format!("{} {}", full_hash(key), full_hash(shown))) {
        Some(seen) if std::fs::read_to_string(&record).ok().as_deref() == Some(seen.as_str()) => {
            pointer()
        }
        Some(seen) => {
            if std::fs::write(&record, seen).is_err() {
                sound = clear(&record);
            }
            None
        }
        None => {
            sound = clear(&record);
            None
        }
    };
    // Take the mark atomically: a call that started meanwhile overwrote it,
    // and its record must not survive this call.
    let mut taken = mark.clone().into_os_string();
    taken.push(format!(".{}", std::process::id()));
    let taken = std::path::PathBuf::from(taken);
    let ours = std::fs::rename(&mark, &taken).is_ok()
        && std::fs::read_to_string(&taken)
            .ok()
            .is_some_and(|body| mark_id(&body) == id);
    if ours && sound {
        let _ = std::fs::remove_file(&taken);
        return repeat.or(emitted);
    }
    if !clear(&record) {
        files.poison();
    } else if !ours && let Ok(other) = std::fs::read(&taken) {
        let _ = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&mark)
            .and_then(|mut file| std::io::Write::write_all(&mut file, &other));
    }
    let _ = std::fs::remove_file(&taken);
    emitted
}

/// Saves `delivered` for `rtk recall`; its handle.
fn store_repeat(config: &Config, delivered: &str) -> Option<String> {
    match crate::core::retriever::store(
        &config.retriever,
        delivered.as_bytes(),
        "posthook:repeat",
        None,
        1,
    ) {
        crate::core::retriever::Stored::Saved(stored) => Some(stored.hash),
        _ => None,
    }
}

fn hook_output(updated: Value) -> String {
    json!({
        "hookSpecificOutput": {
            "hookEventName": POST_TOOL_USE_KEY,
            "updatedToolOutput": updated,
        }
    })
    .to_string()
}

fn pointer(config: &Config, response: &Value, delivered: &str) -> Option<String> {
    let hash = store_repeat(config, delivered)?;
    let text = format!(
        "[rtk: same output as the previous run of this command ({} lines): rtk recall {hash}]",
        delivered.lines().count(),
    );
    if text.len() >= delivered.len() {
        return None;
    }
    let mut updated = response.clone();
    let obj = updated.as_object_mut()?;
    obj.insert("stdout".into(), json!(text));
    obj.insert("stderr".into(), json!(""));
    Some(hook_output(updated))
}

/// The text a Read, Grep or Glob result shows: a file's content, matching
/// lines or counts, or the file names found.
fn native_text(response: &Value) -> Option<String> {
    if let Some(text) = response
        .pointer("/file/content")
        .or_else(|| response.get("content"))
    {
        return text.as_str().map(str::to_string);
    }
    let names = response.get("filenames")?.as_array()?;
    names
        .iter()
        .map(|name| name.as_str())
        .collect::<Option<Vec<_>>>()
        .map(|names| names.join("\n"))
}

/// `response` with its text replaced by a pointer to `text`, inside the
/// hook's own output (`envelope`) when it emitted one.
fn native_pointer(
    config: &Config,
    event: &Value,
    envelope: Option<Value>,
    mut response: Value,
    text: &str,
) -> Option<String> {
    let tool = event.get("tool_name").and_then(Value::as_str)?;
    let hash = store_repeat(config, text)?;
    let line = format!(
        "[rtk: same result as an earlier identical {tool} in this session ({} lines): rtk recall {hash}]",
        text.lines().count(),
    );
    if line.len() >= text.len() {
        return None;
    }
    if let Some(content) = response.pointer_mut("/file/content") {
        *content = json!(line);
    } else if response.get("content").is_some() {
        response["content"] = json!(line);
    } else {
        response["filenames"] = json!([line]);
    }
    // The rest of the hook's own output (a format conversion note) stays.
    match envelope {
        Some(mut envelope) => {
            *envelope.pointer_mut("/hookSpecificOutput/updatedToolOutput")? = response;
            Some(envelope.to_string())
        }
        None => Some(hook_output(response)),
    }
}

/// Path or URL the tracking rows and `when` facts key on.
fn source_of(event: &Value, tool_name: &str) -> String {
    let pointer = match tool_name {
        "Read" | "Grep" | "Glob" => "/tool_input/file_path",
        "WebFetch" => "/tool_input/url",
        _ => return String::new(),
    };
    event
        .pointer(pointer)
        .or_else(|| event.pointer("/tool_input/path"))
        .and_then(|p| p.as_str())
        .unwrap_or_default()
        .to_string()
}

/// Content string the model would read + its format
/// (`None` format = unsupported → tracked passthrough).
fn extract(
    event: &Value,
    tool_name: &str,
    source: &str,
) -> Option<(String, Option<ContentFormat>)> {
    match tool_name {
        "Read" => {
            let content = event.pointer("/tool_response/file/content")?.as_str()?;
            // Lockfile check first: package-lock.json would otherwise
            // classify as plain json by extension.
            let format = if is_lockfile_name(&basename_of(source)) {
                ContentFormat::Lockfile
            } else if extension_of(source) == "json" {
                ContentFormat::Json
            } else {
                // Plain text/code read: reaches the rules as `term` so
                // conditions like `strip-line-numbers` can fire; no shipped
                // rule matches it, so the default behaviour is passthrough.
                ContentFormat::Term
            };
            Some((content.to_string(), Some(format)))
        }
        "Grep" => {
            let response = event.get("tool_response")?;
            let mode = response.get("mode").and_then(|m| m.as_str())?;
            let content = response.get("content").and_then(|c| c.as_str());
            match (mode, content) {
                ("content", Some(text)) => Some((text.to_string(), Some(ContentFormat::Matches))),
                // count / files_with_matches are already compact.
                (_, Some(text)) => Some((text.to_string(), None)),
                _ => None,
            }
        }
        "WebFetch" => {
            let result = event.pointer("/tool_response/result")?.as_str()?;
            Some((result.to_string(), Some(ContentFormat::Web)))
        }
        name if name.starts_with("mcp__") => {
            let text = mcp_text(event.get("tool_response")?)?;
            let format = if serde_json::from_str::<Value>(text.trim()).is_ok() {
                ContentFormat::Json
            } else {
                ContentFormat::Term
            };
            Some((text, Some(format)))
        }
        // Glob output (filenames array) is not `matches` format.
        "Glob" => {
            let filenames = event.pointer("/tool_response/filenames")?.as_array()?;
            let joined = filenames
                .iter()
                .filter_map(|f| f.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            Some((joined, None))
        }
        "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => None,
        _ => {
            let text = mcp_text(event.get("tool_response")?)?;
            let format = if serde_json::from_str::<Value>(text.trim()).is_ok() {
                ContentFormat::Json
            } else {
                ContentFormat::Term
            };
            Some((text, Some(format)))
        }
    }
}

/// Lockfile basenames served by the `lockfile` format (type is then
/// content-sniffed inside the method).
fn is_lockfile_name(basename: &str) -> bool {
    matches!(
        basename,
        "package-lock.json" | "Cargo.lock" | "yarn.lock" | "pnpm-lock.yaml"
    )
}

/// Re-wrap the filtered text in the same output shape the tool produced.
/// Returns `(updatedToolOutput, format_converted)`; `None` when the shape
/// can't be reproduced confidently → emit nothing.
fn rebuild(event: &Value, tool_name: &str, composed: &str) -> Option<(Value, bool)> {
    let mut response = event.get("tool_response")?.clone();
    match tool_name {
        "Read" => {
            let file = response.get_mut("file")?.as_object_mut()?;
            file.insert("content".into(), json!(composed));
            // Host renders line numbers from raw content: keep metadata
            // coherent with the replacement.
            let lines = composed.lines().count();
            file.insert("numLines".into(), json!(lines));
            file.insert("totalLines".into(), json!(lines));
            // Converted content can no longer anchor Edit.old_string.
            Some((response, true))
        }
        "Grep" => {
            let obj = response.as_object_mut()?;
            obj.insert("content".into(), json!(composed));
            obj.insert("numLines".into(), json!(composed.lines().count()));
            Some((response, false))
        }
        "WebFetch" => {
            let obj = response.as_object_mut()?;
            obj.insert("result".into(), json!(composed));
            Some((response, false))
        }
        "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => None,
        _ => {
            let obj = response.as_object_mut()?;
            if obj.get("result").and_then(Value::as_str).is_some() {
                obj.insert("result".into(), json!(composed));
            } else {
                obj.get("content")?.as_array()?;
                obj.insert(
                    "content".into(),
                    json!([{"type": "text", "text": composed}]),
                );
            }
            Some((response, false))
        }
    }
}

/// The text an MCP tool returned, for the two shapes RTK can put back
/// unambiguously: a `result` string, or a `content` array whose every block is
/// text. Anything else — an image, an embedded resource, a mixed array —
/// returns `None`, and the output passes through untouched.
fn mcp_text(response: &Value) -> Option<String> {
    if let Some(text) = response.get("result").and_then(Value::as_str) {
        return Some(text.to_string());
    }
    let blocks = response.get("content")?.as_array()?;
    let texts: Vec<&str> = blocks
        .iter()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|b| b.get("text").and_then(Value::as_str))
        .collect();
    if texts.is_empty() || texts.len() != blocks.len() {
        return None;
    }
    Some(texts.join("\n"))
}

fn extension_of(source: &str) -> &str {
    std::path::Path::new(source)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
}

fn basename_of(source: &str) -> String {
    if let Some(host) = source
        .strip_prefix("https://")
        .or_else(|| source.strip_prefix("http://"))
    {
        return host.split('/').next().unwrap_or(host).to_string();
    }
    std::path::Path::new(source)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("output")
        .to_string()
}

/// Record a tracking row without ever crashing (mirrors
/// `record_parse_failure_silent`). Every posthook code path tracks, including
/// passthroughs — otherwise gains silently fail (pipe_cmd precedent).
fn track(
    tool_name: &str,
    format_token: &str,
    source: &str,
    raw: &str,
    emitted: &str,
    duration_ms: u64,
) {
    let rtk_cmd = format!("rtk posthook {} {}", tool_name.to_lowercase(), format_token);
    let original_cmd = if source.is_empty() {
        tool_name.to_string()
    } else {
        format!("{tool_name} {source}")
    };
    if let Ok(tracker) = tracker() {
        let _ = tracker.record(
            &original_cmd,
            &rtk_cmd,
            estimate_tokens(raw),
            estimate_tokens(emitted),
            duration_ms,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recall_commands_pass_the_recovery_gate() {
        for cmd in [
            "rtk recall 3f9a1c",
            "cd x && rtk recall 3f9a1c --tail 50",
            "(rtk recall abc)",
        ] {
            assert!(RTK_RECALL_COMMAND.is_match(cmd), "{cmd}");
        }
        for cmd in ["rtk gain", "echo recall", "myrtk recall x", "rtk recalled"] {
            assert!(!RTK_RECALL_COMMAND.is_match(cmd), "{cmd}");
        }
    }
    use rtk_engine::pipeline::{Scope, SourceFile, parse, resolve};

    fn test_config() -> Config {
        let mut config = Config::default();
        config.retriever.mode = crate::core::retriever::RecoveryMode::Disabled;
        config
    }

    fn pipeline_from(text: &str) -> Resolved {
        let parsed = parse(text, "test.toml");
        assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
        let (resolved, diagnostics) = resolve(&[SourceFile {
            scope: Scope::User,
            label: "test.toml".to_string(),
            file: parsed.file.expect("parses"),
        }]);
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
        resolved
    }

    fn posthook_pipeline() -> Resolved {
        pipeline_from("[pipeline]\nextends = \"rtk:posthook\"\n")
    }

    fn json_reads_pipeline() -> Resolved {
        pipeline_from(
            "[pipeline]\nextends = \"rtk:posthook\"\n[override.json-reads]\nenabled = true\n",
        )
    }

    fn default_pipeline() -> Resolved {
        let (resolved, _) = resolve(&[]);
        resolved
    }

    fn run_with(event: &Value, config: &Config, pipeline: &Resolved) -> Option<String> {
        process_with_config(event, config, pipeline)
    }

    fn repeat_config(dir: &std::path::Path, on: bool) -> Config {
        let mut config = test_config();
        config.retriever.mode = crate::core::retriever::RecoveryMode::Sqlite;
        config.retriever.database_path = Some(dir.join("recall.db"));
        config.retriever.tee_directory = Some(dir.join("tee"));
        config.hooks.dedup_repeats = on;
        config
    }

    fn bash_event(command: &str, stdout: &str, agent: Option<&str>) -> Value {
        let mut event = serde_json::json!({
            "session_id": "s1",
            "tool_name": "Bash",
            "tool_input": {"command": command},
            "tool_response": {"stdout": stdout, "stderr": ""},
            "duration_ms": 10
        });
        if let Some(agent) = agent {
            event["agent_id"] = serde_json::json!(agent);
        }
        event
    }

    /// One Bash call's events: `pre` is its PreToolUse, `post` its PostToolUse.
    struct Call {
        event: Value,
    }

    impl Call {
        fn new(id: &str, command: &str, stdout: &str, agent: Option<&str>) -> Self {
            let mut event = bash_event(command, stdout, agent);
            event["tool_use_id"] = serde_json::json!(id);
            Self { event }
        }

        fn in_session(mut self, session: &str) -> Self {
            self.event["session_id"] = serde_json::json!(session);
            self
        }

        fn pre(&self, config: &Config) {
            note_tool_start_with(config, &self.event);
        }

        fn post(&self, config: &Config) -> Option<String> {
            run_with(&self.event, config, &default_pipeline())
        }

        /// PreToolUse then PostToolUse: a call that completed.
        fn run(&self, config: &Config) -> Option<String> {
            self.pre(config);
            self.post(config)
        }
    }

    fn lines(n: usize) -> String {
        (0..n).map(|i| format!("line {i}\n")).collect()
    }

    fn pointer_of(out: &Option<String>) -> Option<String> {
        let parsed: Value = serde_json::from_str(out.as_deref()?).ok()?;
        let stdout = parsed
            .pointer("/hookSpecificOutput/updatedToolOutput/stdout")?
            .as_str()?;
        stdout
            .starts_with("[rtk: same output as the previous run")
            .then(|| stdout.to_string())
    }

    /// Off by default: a repeated output reaches the agent exactly as before.
    #[test]
    fn test_repeat_pointer_is_off_by_default() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = repeat_config(dir.path(), false);
        let output: String = (0..400).map(|i| format!("line {i}\n")).collect();
        let event = bash_event("make test", &output, None);
        let first = run_with(&event, &config, &default_pipeline());
        let second = run_with(&event, &config, &default_pipeline());
        assert_eq!(first, second);
        assert!(pointer_of(&second).is_none());
    }

    #[test]
    fn test_repeat_pointer_replaces_only_a_consecutive_identical_output() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = repeat_config(dir.path(), true);
        let output = lines(400);
        let mut n = 0;
        let mut run = |command: &str, stdout: &str, agent: Option<&str>| {
            n += 1;
            Call::new(&format!("t{n}"), command, stdout, agent).run(&config)
        };

        let first = run("make test", &output, None);
        assert!(
            pointer_of(&first).is_none(),
            "a first run is delivered whole"
        );
        let pointer = pointer_of(&run("make test", &output, None)).expect("a pointer");
        assert_eq!(
            pointer
                .split_once(" lines): rtk recall ")
                .map(|(head, _)| head),
            Some("[rtk: same output as the previous run of this command (400"),
        );
        let hash = pointer
            .rsplit_once("rtk recall ")
            .and_then(|(_, rest)| rest.strip_suffix(']'))
            .expect("a recall handle");
        let conn = crate::core::retriever::open_existing(&config.retriever)
            .unwrap()
            .unwrap();
        let row = crate::core::retriever::load_by_hash(&conn, hash)
            .unwrap()
            .unwrap();
        assert_eq!(
            crate::core::retriever::decode(&row).unwrap(),
            output.as_bytes(),
            "the handle reads back the output the pointer stands for"
        );

        // A call the hook passes over (below its size floor) in between ends the run.
        assert!(pointer_of(&run("true", "", None)).is_none());
        assert!(pointer_of(&run("make test", &output, None)).is_none());
        // Another command in between, other bytes, or another agent: delivered whole.
        assert!(pointer_of(&run("git status", &output, None)).is_none());
        assert!(pointer_of(&run("make test", &output, None)).is_none());
        let changed = format!("{output}one more\n");
        assert!(pointer_of(&run("make test", &changed, None)).is_none());
        assert!(pointer_of(&run("make test", &changed, Some("sub-1"))).is_none());
        assert!(pointer_of(&run("make test", &changed, None)).is_some());

        // A SessionStart, where it is registered, forgets.
        forget_repeats_with(&config, &bash_event("make test", "", None));
        assert!(pointer_of(&run("make test", &changed, None)).is_none());
    }

    /// Pass, fail, pass: a failed call fires PostToolUseFailure, which this
    /// hook never sees, so only its PreToolUse arrives.
    #[test]
    fn test_repeat_pointer_not_given_after_a_failed_call() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = repeat_config(dir.path(), true);
        let pass = lines(400);
        assert!(pointer_of(&Call::new("a", "cargo test", &pass, None).run(&config)).is_none());
        Call::new("b", "cargo test", &lines(300), None).pre(&config);
        assert!(
            pointer_of(&Call::new("c", "cargo test", &pass, None).run(&config)).is_none(),
            "the previous run the agent saw is the failed one"
        );
        assert!(pointer_of(&Call::new("d", "cargo test", &pass, None).run(&config)).is_some());
    }

    /// Without PreToolUse (not registered, or an older host without
    /// `tool_use_id`), nothing shows a call completed: always whole.
    #[test]
    fn test_repeat_pointer_needs_the_pre_tool_use_registration() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = repeat_config(dir.path(), true);
        let output = lines(400);
        for id in ["a", "b", "c"] {
            let call = Call::new(id, "make test", &output, None);
            assert!(pointer_of(&call.post(&config)).is_none());
        }
        let mut no_id = bash_event("make test", &output, None);
        for _ in 0..3 {
            note_tool_start_with(&config, &no_id);
            assert!(pointer_of(&run_with(&no_id, &config, &default_pipeline())).is_none());
        }
        no_id["tool_use_id"] = serde_json::json!("");
        note_tool_start_with(&config, &no_id);
        assert!(pointer_of(&run_with(&no_id, &config, &default_pipeline())).is_none());
    }

    /// A new session has no previous call.
    #[test]
    fn test_repeat_pointer_not_given_across_sessions() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = repeat_config(dir.path(), true);
        let output = lines(400);
        let call = |id: &str, session: &str| {
            Call::new(id, "make test", &output, None)
                .in_session(session)
                .run(&config)
        };
        assert!(pointer_of(&call("a", "s1")).is_none());
        assert!(pointer_of(&call("b", "s2")).is_none());
        assert!(pointer_of(&call("c", "s2")).is_some());
    }

    /// A hook process that died between a call's events leaves the state
    /// doubtful, so the next identical output is delivered whole.
    #[test]
    fn test_repeat_pointer_not_given_after_a_crashed_hook() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = repeat_config(dir.path(), true);
        let output = lines(400);
        Call::new("a", "make test", &output, None).run(&config);
        // PreToolUse died before marking its call.
        let b = Call::new("b", "make test", &output, None);
        assert!(pointer_of(&b.post(&config)).is_none());
        // PostToolUse died after the agent got the output and before it
        // finished: its call is still marked.
        Call::new("c", "make test", &output, None).run(&config);
        let d = Call::new("d", "make test", &lines(300), None);
        d.pre(&config);
        assert!(pointer_of(&Call::new("e", "make test", &lines(300), None).run(&config)).is_none());
    }

    /// Parallel calls: A and B start together, B finishes, C starts, then A
    /// finishes. C matching B must not point at B: A came back last.
    #[test]
    fn test_repeat_pointer_not_given_after_parallel_calls() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = repeat_config(dir.path(), true);
        let a = Call::new("a", "make test", &lines(300), None);
        let b = Call::new("b", "make test", &lines(400), None);
        let c = Call::new("c", "make test", &lines(400), None);
        a.pre(&config);
        b.pre(&config);
        assert!(pointer_of(&b.post(&config)).is_none());
        c.pre(&config);
        assert!(pointer_of(&a.post(&config)).is_none());
        assert!(pointer_of(&c.post(&config)).is_none());
    }

    /// Recall storage switched off: nothing is stored and no pointer given.
    #[test]
    fn test_repeat_pointer_respects_disabled_recall_storage() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut config = repeat_config(dir.path(), true);
        config.retriever.mode = crate::core::retriever::RecoveryMode::Disabled;
        let output = lines(400);
        for id in ["a", "b"] {
            let out = Call::new(id, "make test", &output, None).run(&config);
            assert!(pointer_of(&out).is_none());
        }
        assert!(!dir.path().join("recall.db").exists());
    }

    /// A record that cannot be deleted (another process holds it open without
    /// delete sharing) must not let a later call point at it.
    #[cfg(windows)]
    #[test]
    fn test_repeat_pointer_not_given_while_the_record_cannot_be_deleted() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let config = repeat_config(dir.path(), true);
        let output = lines(400);
        let a = Call::new("a", "make test", &output, None);
        assert!(pointer_of(&a.run(&config)).is_none());
        let record = agent_files(&config, &a.event)
            .expect("state files")
            .record();
        const FILE_SHARE_READ: u32 = 1;
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&record)
            .expect("hold the record");
        Call::new("b", "make test", &lines(300), None).pre(&config);
        for id in ["c", "d"] {
            let call = Call::new(id, "make test", &output, None);
            assert!(pointer_of(&call.run(&config)).is_none(), "call {id}");
        }
        drop(held);
        assert!(pointer_of(&Call::new("e", "make test", &output, None).run(&config)).is_none());
        assert!(pointer_of(&Call::new("f", "make test", &output, None).run(&config)).is_some());
    }

    /// An overlapping start that can neither delete the record nor rewrite
    /// the mark (another process holds both, the mark still renamable) must
    /// not leave the first call's mark valid.
    #[cfg(windows)]
    #[test]
    fn test_repeat_pointer_not_given_when_the_mark_cannot_be_rewritten() {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 1;
        const FILE_SHARE_DELETE: u32 = 4;
        let dir = tempfile::tempdir().expect("tempdir");
        let config = repeat_config(dir.path(), true);
        let output = lines(400);
        assert!(pointer_of(&Call::new("a", "make test", &output, None).run(&config)).is_none());
        let b = Call::new("b", "make test", &output, None);
        b.pre(&config);
        let files = agent_files(&config, &b.event).expect("state files");
        let hold = |path: std::path::PathBuf, share: u32| {
            std::fs::OpenOptions::new()
                .read(true)
                .share_mode(share)
                .open(path)
                .expect("hold the file")
        };
        let record = hold(files.record(), FILE_SHARE_READ);
        let mark = hold(files.mark(), FILE_SHARE_READ | FILE_SHARE_DELETE);
        // c starts whilst b is in flight.
        Call::new("c", "make test", &output, None).pre(&config);
        drop(record);
        drop(mark);
        assert!(
            pointer_of(&b.post(&config)).is_none(),
            "b overlapped c: delivered whole"
        );
    }

    /// No other shell tool, and never a pointer longer than the output it
    /// replaces.
    #[test]
    fn test_repeat_pointer_not_for_other_shells_and_only_when_shorter() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = repeat_config(dir.path(), true);
        let output = lines(400);
        for id in ["a", "b"] {
            let mut call = Call::new(id, "Get-ChildItem", &output, None);
            call.event["tool_name"] = serde_json::json!("PowerShell");
            assert!(pointer_of(&call.run(&config)).is_none());
        }
        let short = "ok\n".repeat(20);
        let pipeline =
            pipeline_from("[pipeline]\nextends = \"rtk:default\"\n[guards]\nmin_size = 1\n");
        for id in ["c", "d"] {
            let call = Call::new(id, "make check", &short, None);
            call.pre(&config);
            assert!(pointer_of(&run_with(&call.event, &config, &pipeline)).is_none());
        }
    }

    impl Call {
        /// A Read, Grep or Glob call.
        fn tool(id: &str, tool: &str, input: Value, response: Value) -> Self {
            Self {
                event: serde_json::json!({
                    "session_id": "s1",
                    "tool_use_id": id,
                    "tool_name": tool,
                    "tool_input": input,
                    "tool_response": response,
                    "duration_ms": 10
                }),
            }
        }

        fn read(id: &str, path: &str, content: &str) -> Self {
            let n = content.lines().count();
            Self::tool(
                id,
                "Read",
                serde_json::json!({"file_path": path}),
                serde_json::json!({"type": "text", "file": {
                    "filePath": path, "content": content,
                    "numLines": n, "startLine": 1, "totalLines": n
                }}),
            )
        }

        fn glob(id: &str, pattern: &str, files: &[String], duration: u64) -> Self {
            Self::tool(
                id,
                "Glob",
                serde_json::json!({"pattern": pattern}),
                serde_json::json!({
                    "filenames": files, "durationMs": duration,
                    "numFiles": files.len(), "truncated": false
                }),
            )
        }

        fn by(mut self, agent: &str) -> Self {
            self.event["agent_id"] = serde_json::json!(agent);
            self
        }
    }

    /// The pointer a native tool's result was replaced with, wherever its
    /// text lives in that tool's output.
    fn native_pointer_of(out: &Option<String>) -> Option<String> {
        let parsed: Value = serde_json::from_str(out.as_deref()?).ok()?;
        let updated = parsed.pointer("/hookSpecificOutput/updatedToolOutput")?;
        let text = updated
            .pointer("/file/content")
            .or_else(|| updated.get("content"))
            .or_else(|| updated.pointer("/filenames/0"))?
            .as_str()?;
        text.starts_with("[rtk: same result as an earlier identical ")
            .then(|| text.to_string())
    }

    fn recalled(config: &Config, pointer: &str) -> Vec<u8> {
        let hash = pointer
            .rsplit_once("rtk recall ")
            .and_then(|(_, rest)| rest.strip_suffix(']'))
            .expect("a recall handle");
        let conn = crate::core::retriever::open_existing(&config.retriever)
            .unwrap()
            .unwrap();
        let row = crate::core::retriever::load_by_hash(&conn, hash)
            .unwrap()
            .unwrap();
        crate::core::retriever::decode(&row).unwrap()
    }

    fn files(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("/repo/src/file_{i}.rs")).collect()
    }

    #[test]
    fn test_repeat_pointer_for_a_read_is_off_by_default() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = repeat_config(dir.path(), false);
        for id in ["a", "b"] {
            let out = Call::read(id, "/repo/src/lib.rs", &lines(400)).run(&config);
            assert!(native_pointer_of(&out).is_none());
        }
    }

    /// A re-read of the same bytes points at them; the handle reads them back.
    /// Other reads in between do not matter: each input has its own record.
    #[test]
    fn test_repeat_pointer_for_an_unchanged_read() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = repeat_config(dir.path(), true);
        let content = lines(400);
        let first = Call::read("a", "/repo/src/lib.rs", &content).run(&config);
        assert!(native_pointer_of(&first).is_none(), "a first read is whole");
        Call::read("b", "/repo/src/main.rs", &lines(300)).run(&config);
        let pointer =
            native_pointer_of(&Call::read("c", "/repo/src/lib.rs", &content).run(&config))
                .expect("a pointer");
        assert!(
            pointer.starts_with("[rtk: same result as an earlier identical Read in this session (400 lines): rtk recall "),
            "{pointer}"
        );
        assert_eq!(recalled(&config, &pointer), content.as_bytes());
    }

    /// A file that changed between reads is compared by content, not path.
    #[test]
    fn test_repeat_pointer_not_given_for_a_changed_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = repeat_config(dir.path(), true);
        let path = "/repo/src/lib.rs";
        Call::read("a", path, &lines(400)).run(&config);
        let changed = format!("{}changed\n", lines(400));
        let out = Call::read("b", path, &changed).run(&config);
        assert!(
            native_pointer_of(&out).is_none(),
            "changed content is whole"
        );
        assert!(native_pointer_of(&Call::read("c", path, &changed).run(&config)).is_some());
        // Back to the first content: that is no longer the last read.
        assert!(native_pointer_of(&Call::read("d", path, &lines(400)).run(&config)).is_none());
    }

    /// Another range of the same file is another input.
    #[test]
    fn test_repeat_pointer_keys_on_the_whole_input() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = repeat_config(dir.path(), true);
        let content = lines(400);
        Call::read("a", "/repo/src/lib.rs", &content).run(&config);
        let mut ranged = Call::read("b", "/repo/src/lib.rs", &content);
        ranged.event["tool_input"]["offset"] = serde_json::json!(1);
        assert!(native_pointer_of(&ranged.run(&config)).is_none());
    }

    /// Failed, unmarked and overlapping calls are delivered whole, and the
    /// call after a failed one too.
    #[test]
    fn test_repeat_pointer_for_a_read_not_given_when_completion_is_in_doubt() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = repeat_config(dir.path(), true);
        let path = "/repo/src/lib.rs";
        let content = lines(400);
        Call::read("a", path, &content).run(&config);
        // b failed: only its PreToolUse arrived.
        Call::read("b", path, &content).pre(&config);
        assert!(native_pointer_of(&Call::read("c", path, &content).run(&config)).is_none());
        // d has no PreToolUse.
        assert!(native_pointer_of(&Call::read("d", path, &content).post(&config)).is_none());
        // No tool_use_id.
        let mut no_id = Call::read("", path, &content);
        no_id.event.as_object_mut().unwrap().remove("tool_use_id");
        assert!(native_pointer_of(&no_id.run(&config)).is_none());
        assert!(native_pointer_of(&Call::read("e", path, &content).run(&config)).is_none());
        assert!(native_pointer_of(&Call::read("e2", path, &content).run(&config)).is_some());
        // f and g overlap, across tools: both whole.
        let f = Call::read("f", path, &content);
        let g = Call::glob("g", "**/*.rs", &files(200), 3);
        f.pre(&config);
        g.pre(&config);
        assert!(native_pointer_of(&g.post(&config)).is_none());
        assert!(native_pointer_of(&f.post(&config)).is_none());
        assert!(native_pointer_of(&Call::read("h", path, &content).run(&config)).is_none());
        assert!(native_pointer_of(&Call::read("i", path, &content).run(&config)).is_some());
    }

    /// A subagent never gets a pointer to what only another agent read.
    #[test]
    fn test_repeat_pointer_for_a_read_is_per_agent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = repeat_config(dir.path(), true);
        let path = "/repo/src/lib.rs";
        let content = lines(400);
        Call::read("a", path, &content).run(&config);
        let sub = Call::read("b", path, &content).by("sub-1").run(&config);
        assert!(native_pointer_of(&sub).is_none());
        assert!(
            native_pointer_of(&Call::read("c", path, &content).by("sub-1").run(&config)).is_some()
        );
    }

    /// Glob: the volatile duration is not part of the result compared.
    #[test]
    fn test_repeat_pointer_for_glob_and_grep() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = repeat_config(dir.path(), true);
        let found = files(200);
        Call::glob("a", "**/*.rs", &found, 3).run(&config);
        let pointer = native_pointer_of(&Call::glob("b", "**/*.rs", &found, 9).run(&config))
            .expect("a Glob pointer");
        assert_eq!(recalled(&config, &pointer), found.join("\n").as_bytes());
        let more = files(201);
        assert!(native_pointer_of(&Call::glob("c", "**/*.rs", &more, 3).run(&config)).is_none());

        let grep = |id: &str| {
            Call::tool(
                id,
                "Grep",
                serde_json::json!({"pattern": "fn ", "output_mode": "files_with_matches"}),
                serde_json::json!({
                    "mode": "files_with_matches", "filenames": found,
                    "numFiles": found.len()
                }),
            )
            .run(&config)
        };
        assert!(native_pointer_of(&grep("d")).is_none());
        assert!(native_pointer_of(&grep("e")).is_some());
    }

    /// A Read between 2 identical Bash runs leaves the Bash pointer alone.
    #[test]
    fn test_repeat_pointer_for_bash_unaffected_by_a_read_between() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = repeat_config(dir.path(), true);
        let output = lines(400);
        Call::new("a", "make test", &output, None).run(&config);
        Call::read("b", "/repo/src/lib.rs", &lines(300)).run(&config);
        assert!(pointer_of(&Call::new("c", "make test", &output, None).run(&config)).is_some());
    }

    fn session_dir(config: &Config, session: &str) -> std::path::PathBuf {
        repeat_dir(config, &serde_json::json!({"session_id": session})).expect("a folder")
    }

    const DAY: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

    /// Old sessions' folders go; the live session's stays however old, and a
    /// recent session's stays.
    #[test]
    fn test_prune_repeats_keeps_the_live_session() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = repeat_config(dir.path(), true);
        for session in ["live", "old"] {
            Call::new("a", "make test", &lines(400), None)
                .in_session(session)
                .run(&config);
        }
        let (live, old) = (session_dir(&config, "live"), session_dir(&config, "old"));
        let now = std::time::SystemTime::now();

        prune_repeats(&live, now + DAY);
        assert!(live.exists() && old.exists(), "a day idle is kept");

        prune_repeats(&live, now + 8 * DAY);
        assert!(live.exists(), "the live session survives");
        assert!(!old.exists(), "a week idle is pruned");
        assert!(
            pointer_of(
                &Call::new("b", "make test", &lines(400), None)
                    .in_session("live")
                    .run(&config)
            )
            .is_some(),
            "the live session's records are intact"
        );
    }

    /// A taken mark a dead hook left goes after an hour, in every session;
    /// marks and records stay.
    #[test]
    fn test_prune_repeats_removes_stale_taken_marks() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = repeat_config(dir.path(), true);
        let call = Call::new("a", "make test", &lines(400), None);
        call.run(&config);
        Call::new("b", "make test", &lines(400), None).pre(&config);
        let files = agent_files(&config, &call.event).expect("state files");
        let mut taken = files.mark().into_os_string();
        taken.push(".4242");
        let taken = std::path::PathBuf::from(taken);
        std::fs::write(&taken, "a").unwrap();
        let other = files.dir.join("notes.run.txt");
        std::fs::write(&other, "x").unwrap();
        let now = std::time::SystemTime::now();

        prune_repeats(&files.dir, now);
        assert!(taken.exists(), "a recent taken mark may be in use");

        prune_repeats(&files.dir, now + std::time::Duration::from_secs(2 * 3600));
        assert!(!taken.exists());
        assert!(files.mark().exists() && files.record().exists());
        assert!(other.exists(), "only taken marks are removed");
    }

    /// SessionStart and a session's first call both prune.
    #[cfg(unix)]
    #[test]
    fn test_prune_repeats_runs_at_session_start_and_first_call() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = repeat_config(dir.path(), true);
        let age = |session: &str| {
            let path = session_dir(&config, session);
            std::fs::File::open(&path)
                .and_then(|f| f.set_modified(std::time::SystemTime::now() - 8 * DAY))
                .expect("age the folder");
            path
        };
        Call::new("a", "make test", &lines(400), None)
            .in_session("old")
            .run(&config);
        let old = age("old");
        forget_repeats_with(&config, &serde_json::json!({"session_id": "new"}));
        assert!(!old.exists(), "SessionStart prunes");

        Call::new("b", "make test", &lines(400), None)
            .in_session("old2")
            .run(&config);
        let old2 = age("old2");
        Call::new("c", "make test", &lines(400), None)
            .in_session("new2")
            .run(&config);
        assert!(!old2.exists(), "a session's first call prunes");
    }

    /// A pointer to a converted Read keeps the note that the text is not the
    /// file's bytes.
    #[test]
    fn test_repeat_pointer_keeps_the_read_conversion_note() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = repeat_config(dir.path(), true);
        let read = |id: &str| {
            let mut event = fixture("read_flat");
            event["session_id"] = json!("s1");
            event["tool_use_id"] = json!(id);
            note_tool_start_with(&config, &event);
            run_with(&event, &config, &json_reads_pipeline())
        };
        let note = |out: &Option<String>| {
            let parsed: Value = serde_json::from_str(out.as_deref().expect("output")).unwrap();
            parsed["hookSpecificOutput"]["additionalContext"].clone()
        };
        let first = read("a");
        assert_eq!(note(&first), json!(READ_CONVERSION_NOTE));
        let second = read("b");
        assert!(native_pointer_of(&second).is_some(), "{second:?}");
        assert_eq!(note(&second), json!(READ_CONVERSION_NOTE));
    }

    fn run_posthook(event: &Value) -> Option<String> {
        run_with(event, &test_config(), &posthook_pipeline())
    }

    fn fixture(name: &str) -> Value {
        let raw = match name {
            "read_flat" => include_str!("../../tests/fixtures/posthook/read_json_flat.json"),
            "read_nested" => include_str!("../../tests/fixtures/posthook/read_json_nested.json"),
            "grep_small" => include_str!("../../tests/fixtures/posthook/grep_content_small.json"),
            "grep_large" => {
                include_str!("../../tests/fixtures/posthook/grep_content_large_truncated.json")
            }
            "grep_count" => include_str!("../../tests/fixtures/posthook/grep_count.json"),
            "webfetch_md" => {
                include_str!("../../tests/fixtures/posthook/webfetch_markdown_url.json")
            }
            "websearch" => include_str!("../../tests/fixtures/posthook/websearch_results.json"),
            "glob" => include_str!("../../tests/fixtures/posthook/glob_json.json"),
            _ => panic!("unknown fixture {name}"),
        };
        serde_json::from_str(raw).expect("fixture parses")
    }

    #[test]
    fn combine_streams_is_symmetric_so_the_separator_is_not_counted_as_savings() {
        assert_eq!(combine_streams("out", ""), "out");
        assert_eq!(combine_streams("out", "err"), "out\n--- stderr ---\nerr");
        let raw = combine_streams("a\nb", "e1\ne2");
        let emitted = combine_streams("a", "e1");
        let sep = "\n--- stderr ---\n";
        assert!(raw.contains(sep) && emitted.contains(sep));
        assert_eq!(
            estimate_tokens(&raw) - estimate_tokens(&emitted),
            estimate_tokens("a\nb") + estimate_tokens("e1\ne2")
                - estimate_tokens("a")
                - estimate_tokens("e1"),
            "the shared separator must cancel out of the saved-token delta"
        );
    }

    #[test]
    fn test_default_pipeline_has_no_after_tool_rules_and_passes_everything_through() {
        let pipeline = default_pipeline();
        for name in ["read_flat", "grep_large", "webfetch_md", "websearch"] {
            assert!(
                run_with(&fixture(name), &test_config(), &pipeline).is_none(),
                "{name} must pass through on rtk:default: upgrading is a no-op"
            );
        }
    }

    #[test]
    fn test_read_json_flat_rewrites_to_toon_with_note_when_json_reads_enabled() {
        let out = run_with(
            &fixture("read_flat"),
            &test_config(),
            &json_reads_pipeline(),
        )
        .expect("flat json must be rewritten");
        let out: Value = serde_json::from_str(&out).expect("valid JSON");
        let hook = &out["hookSpecificOutput"];
        assert_eq!(hook["hookEventName"], "PostToolUse");

        let file = &hook["updatedToolOutput"]["file"];
        let content = file["content"].as_str().unwrap();
        assert!(
            content.starts_with("[120]{id,name,email,active,score}:"),
            "TOON content"
        );
        assert!(
            file["filePath"]
                .as_str()
                .unwrap()
                .ends_with("data_flat.json")
        );
        assert_eq!(
            file["numLines"].as_u64().unwrap() as usize,
            content.lines().count()
        );
        assert_eq!(hook["updatedToolOutput"]["type"], "text");

        assert!(
            hook["additionalContext"]
                .as_str()
                .unwrap()
                .contains("Edit old_string")
        );
    }

    #[test]
    fn test_read_json_passes_through_on_plain_posthook() {
        assert!(
            run_posthook(&fixture("read_flat")).is_none(),
            "json-reads ships disabled"
        );
    }

    #[test]
    fn test_read_nested_above_min_size_minifies_when_json_reads_enabled() {
        let out = run_with(
            &fixture("read_nested"),
            &test_config(),
            &json_reads_pipeline(),
        );
        if let Some(out) = out {
            let v: Value = serde_json::from_str(&out).expect("valid JSON");
            let content = v["hookSpecificOutput"]["updatedToolOutput"]["file"]["content"]
                .as_str()
                .unwrap();
            assert!(content.starts_with('{'), "minified JSON expected");
        }
    }

    #[test]
    fn test_grep_content_groups_by_file() {
        let out = run_posthook(&fixture("grep_large")).expect("large grep content rewritten");
        let v: Value = serde_json::from_str(&out).expect("valid JSON");
        let hook = &v["hookSpecificOutput"];
        let response = &hook["updatedToolOutput"];
        let content = response["content"].as_str().unwrap();
        assert!(content.contains("matches in"), "grouped header");
        assert_eq!(response["mode"], "content");
        assert_eq!(
            response["numLines"].as_u64().unwrap() as usize,
            content.lines().count()
        );
        assert!(hook.get("additionalContext").is_none());
    }

    #[test]
    fn test_mcp_text_reads_both_shapes_and_refuses_the_rest() {
        let result = serde_json::json!({"result": "hello"});
        assert_eq!(mcp_text(&result).as_deref(), Some("hello"));

        let blocks = serde_json::json!({"content": [
            {"type": "text", "text": "a"},
            {"type": "text", "text": "b"}
        ]});
        assert_eq!(mcp_text(&blocks).as_deref(), Some("a\nb"));

        // A non-text block cannot be put back, so the output is left alone.
        let mixed = serde_json::json!({"content": [
            {"type": "text", "text": "a"},
            {"type": "image", "data": "AAAA"}
        ]});
        assert!(mcp_text(&mixed).is_none());
        assert!(mcp_text(&serde_json::json!({"content": []})).is_none());
        assert!(mcp_text(&serde_json::json!({"other": 1})).is_none());
    }

    fn mcp_event(tool: &str, response: serde_json::Value) -> Value {
        serde_json::json!({
            "tool_name": tool,
            "tool_input": {},
            "tool_response": response,
            "duration_ms": 5
        })
    }

    fn mcp_pipeline() -> Resolved {
        pipeline_from(
            "[pipeline]\nextends = \"rtk:posthook\"\n\
             [[hook.after_tool.mcp]]\nname = \"squeeze\"\nuse = [\"dedup\", \"cap\"]\n",
        )
    }

    #[test]
    fn test_mcp_output_is_filtered_and_rebuilt_in_place() {
        let noisy = std::iter::repeat_n("repeated mcp line", 400)
            .collect::<Vec<_>>()
            .join("\n");
        for (tool, response, pointer) in [
            (
                "mcp__linear__get_issue",
                serde_json::json!({"result": noisy.clone()}),
                "/hookSpecificOutput/updatedToolOutput/result",
            ),
            (
                "mcp__linear__list",
                serde_json::json!({"content": [{"type": "text", "text": noisy.clone()}]}),
                "/hookSpecificOutput/updatedToolOutput/content/0/text",
            ),
        ] {
            let event = mcp_event(tool, response);
            let out = run_with(&event, &test_config(), &mcp_pipeline())
                .unwrap_or_else(|| panic!("{tool}: expected a replacement"));
            let parsed: Value = serde_json::from_str(&out).expect("valid json");
            let text = parsed
                .pointer(pointer)
                .and_then(Value::as_str)
                .expect(pointer);
            assert!(text.contains("[x400]"), "{tool}: {text}");
        }
    }

    #[test]
    fn test_subagent_output_is_filtered_and_rebuilt_in_place() {
        let noisy = std::iter::repeat_n("repeated subagent finding", 400)
            .collect::<Vec<_>>()
            .join("\n");
        let pipeline =
            pipeline_from("[[hook.after_tool.subagent]]\nname = \"squeeze\"\nuse = [\"dedup\"]\n");
        for tool in ["Task", "Agent"] {
            let event = mcp_event(
                tool,
                serde_json::json!({
                    "content": [{"type": "text", "text": noisy.clone()}],
                    "totalTokens": 1234
                }),
            );
            let out = run_with(&event, &test_config(), &pipeline)
                .unwrap_or_else(|| panic!("{tool}: expected a replacement"));
            let parsed: Value = serde_json::from_str(&out).expect("valid json");
            let text = parsed
                .pointer("/hookSpecificOutput/updatedToolOutput/content/0/text")
                .and_then(Value::as_str)
                .expect("text block");
            assert!(text.contains("[x400]"), "{tool}: {text}");
            assert_eq!(
                parsed.pointer("/hookSpecificOutput/updatedToolOutput/totalTokens"),
                Some(&serde_json::json!(1234)),
                "the rest of the response is kept"
            );
        }
    }

    #[test]
    fn test_write_and_edit_output_is_never_rewritten() {
        let pipeline = pipeline_from("[[hook.after_tool.any]]\nname = \"a\"\nuse = [\"dedup\"]\n");
        let body = std::iter::repeat_n("same line", 400)
            .collect::<Vec<_>>()
            .join("\n");
        for tool in ["Write", "Edit"] {
            let event = mcp_event(
                tool,
                serde_json::json!({"content": body.clone(), "filePath": "a.rs"}),
            );
            assert!(
                run_with(&event, &test_config(), &pipeline).is_none(),
                "{tool}"
            );
        }
    }

    #[test]
    fn test_mcp_output_with_an_image_block_passes_through() {
        let event = mcp_event(
            "mcp__x__screenshot",
            serde_json::json!({"content": [
                {"type": "text", "text": "x".repeat(4000)},
                {"type": "image", "data": "AAAA"}
            ]}),
        );
        assert!(run_with(&event, &test_config(), &mcp_pipeline()).is_none());
    }

    /// A route whose worker cannot be reached must leave the agent's output
    /// alone. `digest-none` has no upstream, which is the cheapest reachable
    /// failure: no network call is attempted and nothing is emitted.
    #[test]
    fn test_after_tool_route_without_an_upstream_passes_output_through() {
        let pipeline = pipeline_from(
            "[pipeline]\nname = \"t\"\n\
             [task.digest-none]\nkind = \"digest\"\n\
             [[hook.after_tool.shell]]\nname = \"route-all\"\n\
             action = \"route\"\ntask = \"digest-none\"\n",
        );
        let noisy = (0..400)
            .map(|i| format!("line {i}: verbose build output"))
            .collect::<Vec<_>>()
            .join("\n");
        let event = serde_json::json!({
            "tool_name": "Bash",
            "tool_input": {"command": "make build"},
            "tool_response": {"stdout": noisy, "stderr": ""},
            "duration_ms": 10
        });
        assert!(
            run_with(&event, &test_config(), &pipeline).is_none(),
            "an unreachable worker must not cost the agent its output"
        );
    }

    /// The route arm must not fire for a task that is not a `digest`: a
    /// `summarize` instance reads files, and handing it a tool's stdout would
    /// send the worker a prompt its template never expected.
    #[test]
    fn test_after_tool_route_ignores_a_non_digest_task() {
        let pipeline = pipeline_from(
            "[pipeline]\nname = \"t\"\n\
             [upstream.u]\nprovider = \"anthropic\"\nmodel = \"m\"\n\
             [task.reader]\nkind = \"summarize\"\nupstream = \"u\"\n\
             [[hook.after_tool.shell]]\nname = \"route-all\"\n\
             action = \"route\"\ntask = \"reader\"\n",
        );
        let event = serde_json::json!({
            "tool_name": "Bash",
            "tool_input": {"command": "make build"},
            "tool_response": {"stdout": "x\n".repeat(400), "stderr": ""},
            "duration_ms": 10
        });
        assert!(run_with(&event, &test_config(), &pipeline).is_none());
    }

    #[test]
    fn test_never_worse_guard_hint_overhead_emits_nothing() {
        let tee_dir = tempfile::tempdir().expect("tempdir");
        let mut config = Config::default();
        config.retriever.tee_directory = Some(tee_dir.path().to_path_buf());

        let pairs: Vec<String> = (0..40).map(|i| format!("\"k{i}\":{i}")).collect();
        let almost_min = format!("{{\n{}}}", pairs.join(",\n"));
        let minified_len = serde_json::to_string(
            &serde_json::from_str::<serde_json::Value>(&almost_min).expect("valid"),
        )
        .expect("serialize")
        .len();
        assert!(
            almost_min.len() - minified_len < 60,
            "premise: savings smaller than hint length"
        );

        let event = serde_json::json!({
            "tool_name": "Read",
            "tool_input": {"file_path": "/tmp/almost.json"},
            "tool_response": {"type": "text", "file": {
                "filePath": "/tmp/almost.json",
                "content": almost_min,
                "numLines": 42, "startLine": 1, "totalLines": 42
            }},
            "duration_ms": 1
        });
        assert!(
            run_with(&event, &config, &json_reads_pipeline()).is_none(),
            "composed (filtered+hint) > raw must emit nothing"
        );
    }

    #[test]
    fn test_grep_count_mode_not_rewritten() {
        assert!(
            run_posthook(&fixture("grep_count")).is_none(),
            "count mode is compact already"
        );
    }

    #[test]
    fn test_webfetch_markdown_passes_through() {
        assert!(run_posthook(&fixture("webfetch_md")).is_none());
    }

    #[test]
    fn test_websearch_passes_through_when_chain_noop() {
        assert!(run_posthook(&fixture("websearch")).is_none());
    }

    #[test]
    fn test_glob_unsupported_format_passes_through() {
        assert!(run_posthook(&fixture("glob")).is_none());
    }

    #[test]
    fn test_malformed_event_emits_nothing() {
        assert!(run_posthook(&serde_json::json!({})).is_none());
        assert!(run_posthook(&serde_json::json!({"tool_name": 42})).is_none());
        assert!(
            run_posthook(
                &serde_json::json!({"tool_name": "Read", "tool_response": "not an object"})
            )
            .is_none()
        );
    }

    #[test]
    fn test_unknown_tool_shape_emits_nothing() {
        let event = serde_json::json!({
            "tool_name": "Bash",
            "tool_response": {"output": "x".repeat(2000)}
        });
        assert!(run_posthook(&event).is_none());
    }

    #[test]
    fn test_override_disables_the_term_rule() {
        let pipeline = pipeline_from(
            "[pipeline]\nextends = \"rtk:posthook\"\n[override.term-default]\nenabled = false\n",
        );
        assert!(run_with(&bash_fixture("repetitive"), &test_config(), &pipeline).is_none());
    }

    #[test]
    fn test_shadow_rule_records_but_emits_nothing() {
        let pipeline = pipeline_from(
            "[pipeline]\nextends = \"rtk:posthook\"\n[override.term-default]\nshadow = true\n",
        );
        assert!(run_with(&bash_fixture("repetitive"), &test_config(), &pipeline).is_none());
    }

    #[test]
    fn test_gate_min_size() {
        let event = serde_json::json!({
            "tool_name": "Read",
            "tool_input": {"file_path": "/tmp/small.json"},
            "tool_response": {"type": "text", "file": {
                "filePath": "/tmp/small.json",
                "content": "{\"a\": 1}",
                "numLines": 1, "startLine": 1, "totalLines": 1
            }}
        });
        assert!(run_with(&event, &test_config(), &json_reads_pipeline()).is_none());
    }

    #[test]
    fn test_gate_min_size_from_guards() {
        let pipeline = pipeline_from(
            "[pipeline]\nextends = \"rtk:posthook\"\n[override.json-reads]\nenabled = true\n[guards]\nmin_size = 100000\n",
        );
        assert!(
            run_with(&fixture("read_flat"), &test_config(), &pipeline).is_none(),
            "raised min_size must gate the read"
        );
    }

    #[test]
    fn test_read_non_json_tracked_passthrough_no_output() {
        let event = serde_json::json!({
            "tool_name": "Read",
            "tool_input": {"file_path": "/tmp/code.rs"},
            "tool_response": {"type": "text", "file": {
                "filePath": "/tmp/code.rs",
                "content": "fn main() {}\n".repeat(100),
                "numLines": 100, "startLine": 1, "totalLines": 100
            }}
        });
        assert!(run_posthook(&event).is_none());
    }

    #[test]
    fn test_strip_line_numbers_rule_fires_when_enabled() {
        let numbered: String = (1..=60)
            .map(|i| format!("{i}\u{2192}let value_{i} = compute({i});"))
            .collect::<Vec<_>>()
            .join("\n");
        let event = serde_json::json!({
            "tool_name": "Read",
            "tool_input": {"file_path": "/tmp/code.rs"},
            "tool_response": {"type": "text", "file": {
                "filePath": "/tmp/code.rs",
                "content": numbered,
                "numLines": 60, "startLine": 1, "totalLines": 60
            }}
        });
        assert!(
            run_posthook(&event).is_none(),
            "strip-line-numbers ships off"
        );
        let pipeline = pipeline_from(
            "[pipeline]\nextends = \"rtk:posthook\"\n[override.strip-line-numbers]\nenabled = true\n",
        );
        let out = run_with(&event, &test_config(), &pipeline)
            .expect("enabled strip-line-numbers must fire");
        let v: Value = serde_json::from_str(&out).expect("valid JSON");
        let content = v["hookSpecificOutput"]["updatedToolOutput"]["file"]["content"]
            .as_str()
            .unwrap();
        assert!(content.starts_with("let value_1"));
        assert!(!content.contains('\u{2192}'));
    }

    #[test]
    fn test_read_cargo_lock_summarized_with_note() {
        let content = include_str!("../../Cargo.lock");
        let event = serde_json::json!({
            "tool_name": "Read",
            "tool_input": {"file_path": "/repo/Cargo.lock"},
            "tool_response": {"type": "text", "file": {
                "filePath": "/repo/Cargo.lock",
                "content": content,
                "numLines": content.lines().count(),
                "startLine": 1,
                "totalLines": content.lines().count()
            }},
            "duration_ms": 3
        });
        let out = run_posthook(&event).expect("lockfile must summarize");
        let v: Value = serde_json::from_str(&out).expect("valid JSON");
        let hook = &v["hookSpecificOutput"];
        let new_content = hook["updatedToolOutput"]["file"]["content"]
            .as_str()
            .unwrap();
        assert!(new_content.starts_with("Cargo.lock: "), "summary header");
        assert!(new_content.contains("serde@"));
        assert!(
            hook["additionalContext"].as_str().is_some(),
            "Read conversion note"
        );
    }

    fn bash_fixture(name: &str) -> Value {
        let raw = match name {
            "ansi" => include_str!("../../tests/fixtures/posthook/bash_ansi_output.json"),
            "progress" => include_str!("../../tests/fixtures/posthook/bash_progress_cr.json"),
            "repetitive" => {
                include_str!("../../tests/fixtures/posthook/bash_repetitive_lines.json")
            }
            _ => panic!("unknown fixture {name}"),
        };
        serde_json::from_str(raw).expect("fixture parses")
    }

    #[test]
    fn test_bash_ansi_output_stripped_and_rewrapped() {
        let out = run_posthook(&bash_fixture("ansi")).expect("ansi output must be filtered");
        let v: Value = serde_json::from_str(&out).expect("valid JSON");
        let response = &v["hookSpecificOutput"]["updatedToolOutput"];
        let stdout = response["stdout"].as_str().unwrap();
        assert!(!stdout.contains('\x1b'), "ANSI gone");
        assert!(stdout.contains("PASS test_0"), "content kept");
        assert_eq!(response["interrupted"], false, "metadata preserved");
        assert_eq!(response["isImage"], false);
        assert!(
            v["hookSpecificOutput"].get("additionalContext").is_none(),
            "no conversion note for the generic floor"
        );
    }

    #[test]
    fn test_bash_progress_frames_collapsed() {
        let out = run_posthook(&bash_fixture("progress")).expect("progress output filtered");
        let v: Value = serde_json::from_str(&out).expect("valid JSON");
        let stdout = v["hookSpecificOutput"]["updatedToolOutput"]["stdout"]
            .as_str()
            .unwrap();
        assert!(stdout.contains("Downloading 100% done"));
        assert!(!stdout.contains("Downloading 0%"));
    }

    #[test]
    fn test_bash_repetitive_lines_deduped() {
        let out = run_posthook(&bash_fixture("repetitive")).expect("repetitive output filtered");
        let v: Value = serde_json::from_str(&out).expect("valid JSON");
        let stdout = v["hookSpecificOutput"]["updatedToolOutput"]["stdout"]
            .as_str()
            .unwrap();
        assert!(stdout.contains("[x40]"), "explicit dedup marker: {stdout}");
    }

    #[test]
    fn test_bash_rtk_rewritten_command_skipped() {
        let mut event = bash_fixture("repetitive");
        event["tool_input"]["command"] = json!("rtk cargo test");
        assert!(run_posthook(&event).is_none());

        event["tool_input"]["command"] = json!("git add . && rtk cargo test");
        assert!(run_posthook(&event).is_none());
    }

    #[test]
    fn test_bash_recall_recovery_command_skipped() {
        let mut event = bash_fixture("repetitive");
        event["tool_input"]["command"] =
            json!("cat \"$HOME/.local/share/rtk/tee/posthook/123_1-0_bash_x.log\"");
        assert!(run_posthook(&event).is_none());
    }

    #[test]
    fn test_bash_interrupted_skipped() {
        let mut event = bash_fixture("repetitive");
        event["tool_response"]["interrupted"] = json!(true);
        assert!(run_posthook(&event).is_none());
    }

    #[test]
    fn test_bash_stderr_filtered_independently() {
        let mut event = bash_fixture("ansi");
        event["tool_response"]["stderr"] = json!("\x1b[31merror line\x1b[0m\n".repeat(10));
        let out = run_posthook(&event).expect("stderr must be filtered too");
        let v: Value = serde_json::from_str(&out).expect("valid JSON");
        let stderr = v["hookSpecificOutput"]["updatedToolOutput"]["stderr"]
            .as_str()
            .unwrap();
        assert!(!stderr.contains('\x1b'));
        assert!(stderr.contains("[x10]"));
    }

    #[test]
    fn test_run_never_panics_on_garbage() {
        run(&serde_json::json!(null));
        run(&serde_json::json!("string event"));
        run(&serde_json::json!({"tool_name": "Read"}));
    }

    #[test]
    fn test_recall_hint_ordering_never_pushes_above_raw() {
        let raw = format!(
            "[{}]",
            (0..200)
                .map(|i| format!("{i}"))
                .collect::<Vec<_>>()
                .join(",")
        );
        let event = serde_json::json!({
            "tool_name": "Read",
            "tool_input": {"file_path": "/tmp/nums.json"},
            "tool_response": {"type": "text", "file": {
                "filePath": "/tmp/nums.json",
                "content": raw,
                "numLines": 1, "startLine": 1, "totalLines": 1
            }}
        });
        assert!(run_with(&event, &test_config(), &json_reads_pipeline()).is_none());
    }

    #[test]
    fn applied_carries_the_resolved_pipeline_name_for_attribution() {
        let pipeline =
            pipeline_from("[pipeline]\nname = \"shared-pipe\"\nextends = \"rtk:posthook\"\n");
        let raw = "\x1b[32mcompiling module ok\x1b[0m\n".repeat(200);
        let applied = run_selected(
            &pipeline,
            Subject::Shell,
            "Bash",
            ContentFormat::Term,
            "",
            &raw,
            0,
            Some(false),
        )
        .expect("term chain applies");
        assert_eq!(applied.pipeline, "shared-pipe");
    }

    #[test]
    fn test_log_entry_never_contains_tool_output() {
        let sentinel = "SENTINEL-9f2c-THE-TOOL-OUTPUT";
        let applied = Applied {
            text: format!("filtered {sentinel} filtered"),
            methods: vec!["ansi", "cap"],
            rules: vec!["term-default".to_string()],
            pipeline: "rtk:posthook".to_string(),
            subject: Subject::Shell,
            shadow: false,
        };
        let entry = log_entry_for(
            Subject::Shell,
            Some("cargo test"),
            "",
            &applied,
            estimate_tokens(&applied.text),
            10,
        );
        let line = serde_json::to_string(&entry).expect("serializes");
        assert!(
            !line.contains(sentinel),
            "the decision log records decisions, never content: {line}"
        );
        assert!(line.contains("term-default"));
        assert!(line.contains("cargo test"));
    }

    #[test]
    fn test_source_and_helpers() {
        assert_eq!(extension_of("/a/b/data.json"), "json");
        assert_eq!(extension_of("https://docs.rs/toon"), "");
        assert_eq!(basename_of("/a/b/data.json"), "data.json");
        assert_eq!(basename_of("https://docs.rs/toon-format"), "docs.rs");
    }
}
