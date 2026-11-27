use super::super::resolve::{Scope, SourceFile, resolve};
use super::super::schema;
use super::*;
use crate::block::Event;

fn test_pipeline(text: &str) -> Resolved {
    let label = "coverage.toml";
    let parsed = schema::parse(text, label);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let (resolved, _) = resolve(&[SourceFile {
        scope: Scope::User,
        label: label.to_owned(),
        file: parsed.file.expect("valid test pipeline"),
    }]);
    resolved
}

fn string_facts(pairs: &[(&str, &str)]) -> Facts {
    pairs
        .iter()
        .map(|&(key, value)| (key.to_owned(), Fact::Str(value.to_owned())))
        .collect()
}

#[test]
fn action_names_round_trip() {
    for (action, name) in [
        (Action::Rewrite, "rewrite"),
        (Action::Deny, "deny"),
        (Action::Passthrough, "passthrough"),
        (Action::Route, "route"),
    ] {
        assert_eq!(action.as_str(), name);
        assert_eq!(Action::parse(name), Some(action));
    }
}

#[test]
fn any_groups_require_a_match_and_continue_only_when_requested() {
    let resolved = test_pipeline(
        r#"
[[hook.after_tool.read]]
name = "conditional"
when = { format = "term" }
any = [{ basename = "notes.txt" }, { basename = "todo.txt" }]
use = ["ansi"]
continue = true

[[hook.after_tool.read]]
name = "terminal"
use = ["dedup"]

[[hook.after_tool.read]]
name = "after-terminal"
use = ["cap"]
"#,
    );
    let position = Position::hook(Event::AfterTool, Subject::Read);
    let both: &[&str] = &["conditional", "terminal"];
    let terminal_only: &[&str] = &["terminal"];

    for (format, basename, expected) in [
        ("term", "notes.txt", both),
        ("term", "todo.txt", both),
        ("term", "other.txt", terminal_only),
        ("json", "notes.txt", terminal_only),
    ] {
        let facts = string_facts(&[("format", format), ("basename", basename)]);
        let decisions = evaluate(&resolved, position, &facts);
        let names: Vec<_> = decisions.iter().map(|d| d.rule.name.as_str()).collect();
        assert_eq!(names.as_slice(), expected, "{format}/{basename}");
    }
}

#[test]
fn configured_chains_expand_builtins_and_discard_partial_failures() {
    let mut resolved = test_pipeline(
        r#"
[pipeline]
extends = "rtk:posthook"

[method.coverage-valid]
chain = ["dedup"]

[method.coverage-broken]
chain = ["cap"]
"#,
    );
    let mut facts = string_facts(&[("format", "term")]);
    facts.insert("is_rtk_command".into(), Fact::Bool(false));
    let decisions = evaluate(
        &resolved,
        Position::hook(Event::AfterTool, Subject::Shell),
        &facts,
    );
    let (builtin_name, mut expected) = decisions
        .iter()
        .flat_map(|decision| &decision.rule.def.use_)
        .find_map(|name| {
            registry::chain(name).map(|chain| {
                let names: Vec<_> = chain
                    .iter()
                    .map(|method| method.name().to_owned())
                    .collect();
                (name.clone(), names)
            })
        })
        .expect("the terminal posthook uses a registered chain");

    resolved
        .methods
        .get_mut("coverage-valid")
        .expect("configured valid chain")
        .0
        .chain = Some(vec![builtin_name, "dedup".to_owned()]);
    // Inject a bad entry after resolution to exercise the evaluator's rollback.
    resolved
        .methods
        .get_mut("coverage-broken")
        .expect("configured broken chain")
        .0
        .chain = Some(vec!["cap".to_owned(), "coverage-missing-entry".to_owned()]);

    let requested = [
        "coverage-valid".to_owned(),
        "coverage-broken".to_owned(),
        "coverage-missing-method".to_owned(),
        "ansi".to_owned(),
    ];
    let (methods, unresolved) = resolve_chain(&resolved, &requested);
    let names: Vec<_> = methods
        .iter()
        .map(|method| method.name().to_owned())
        .collect();

    expected.extend(["dedup".to_owned(), "ansi".to_owned()]);
    assert_eq!(names, expected);
    assert_eq!(unresolved, ["coverage-broken", "coverage-missing-method"]);
}

#[test]
fn custom_route_message_overrides_command_hint_and_formats_typed_facts() {
    let mut facts = string_facts(&[("command", "cargo test")]);
    facts.insert("queued".into(), Fact::Bool(true));
    facts.insert("cached".into(), Fact::Bool(false));
    facts.insert("delta".into(), Fact::Num(-12_345.0));
    facts.insert("ratio".into(), Fact::Num(1.25));

    assert_eq!(
        route_message(
            "inspect",
            &facts,
            Some("queued=${queued}, cached=${cached}, delta=${delta}, ratio=${ratio}"),
        ),
        "queued=true, cached=false, delta=-12,345, ratio=1.25"
    );
}

#[test]
fn route_messages_use_placeholders_when_tool_inputs_are_missing() {
    assert_eq!(
        route_message("inspect", &string_facts(&[("tool", "read")]), None),
        "Read of <file> is blocked. Run: `rtk inspect --question \"<what you need to know>\" <file>`"
    );
    // Without a URL or pattern, web and search still name the routed task;
    // their exact wording is not pinned.
    for tool in ["web", "search"] {
        let message = route_message("inspect", &string_facts(&[("tool", tool)]), None);
        assert!(
            message.contains("Run: `rtk inspect --question \""),
            "{tool}: {message}"
        );
    }

    assert_eq!(
        route_message(
            "inspect",
            &string_facts(&[("tool", "subagent")]),
            Some(" \t"),
        ),
        "This subagent task is handled by a worker model. Run: `rtk inspect --question \"<the subagent's question>\" <the files to look at>`"
    );
}
