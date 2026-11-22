use super::*;
use crate::pipeline::resolve::{SourceFile, resolve};
use crate::pipeline::schema::parse;

const FIXTURE: &str = "validate-coverage.toml";

fn resolve_fixture(text: &str) -> Resolved {
    let parsed = parse(text, FIXTURE);
    // Keep parser and resolver diagnostics separate from the validator's output.
    let (resolved, _) = resolve(&[SourceFile {
        scope: Scope::User,
        label: FIXTURE.to_string(),
        file: parsed.file.expect("fixture parses"),
    }]);
    resolved
}

fn rule_diagnostics(resolved: &Resolved, name: &str) -> Vec<Diagnostic> {
    let rule = resolved.find_rule(name).expect("fixture rule exists");
    let mut diagnostics = Vec::new();
    validate_rule(resolved, rule, &ValidateCtx::default(), &mut diagnostics);
    diagnostics
}

fn assert_diagnostic(diagnostics: &[Diagnostic], code: Code, message: &str) {
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code == code && d.message == message),
        "expected {code:?}: {message}\nactual diagnostics: {diagnostics:#?}"
    );
}

#[test]
fn invalid_directive_and_task_are_reported_without_warning_about_enabled_rewrite() {
    let mut resolved = resolve_fixture(
        r#"
[pipeline]
extends = "rtk:default"

[[directive]]
name = "coverage_directive"
use = "terse-output"
channel = "awareness_file"

[task.coverage_task]
kind = "digest"
"#,
    );
    let directive = resolved
        .directives
        .iter_mut()
        .find(|directive| directive.origin == FIXTURE)
        .expect("user directive");
    directive.def.use_ = Some("coverage_unknown_directive".into());
    directive.def.channel = Some("coverage_unknown_channel".into());
    resolved
        .tasks
        .get_mut("coverage_task")
        .expect("user task")
        .0
        .kind
        .clear();

    assert!(
        resolved
            .find_rule("rtk-rewrite")
            .expect("default rewrite rule")
            .def
            .enabled
    );
    let diagnostics = validate(&resolved, &ValidateCtx::default());
    assert_diagnostic(
        &diagnostics,
        Code::E007,
        "unknown directive `coverage_unknown_directive`",
    );
    assert_diagnostic(
        &diagnostics,
        Code::E002,
        "unknown directive channel `coverage_unknown_channel` (one of: awareness_file, session_start, per_turn)",
    );
    assert_diagnostic(
        &diagnostics,
        Code::E014,
        "[task.coverage_task] has no `kind` and no inherited task defines one",
    );
    assert!(!diagnostics.iter().any(|d| d.code == Code::W006));
}

#[test]
fn classifier_validation_checks_scalar_and_list_labels_and_required_settings() {
    let mut resolved = resolve_fixture(
        r#"
[upstream.coverage_judge]
provider = "anthropic"
model = "fixture"

[classifier.coverage_intent]
upstream = "coverage_judge"
question = "choice"
instructions = "Choose a label."

[classifier.coverage_intent.labels]
known = "Known output"

[classifier.coverage_score]
upstream = "coverage_judge"
question = "score"
instructions = "Rate the output."
rubric = ["Low", "High"]

[[hook.after_tool.shell]]
name = "coverage_scalar"
when = { "class.coverage_intent" = "bad_scalar" }
use = ["ansi"]

[[hook.after_tool.shell]]
name = "coverage_list"
when = { "class.coverage_intent_in" = ["known", "bad_list"] }
use = ["ansi"]
"#,
    );

    let diagnostics = validate(&resolved, &ValidateCtx::default());
    for label in ["bad_scalar", "bad_list"] {
        assert_diagnostic(
            &diagnostics,
            Code::E019,
            &format!(
                "`class.coverage_intent` tests label `{label}`, which [classifier.coverage_intent] does not define (labels: known)"
            ),
        );
    }
    assert_eq!(
        diagnostics.iter().filter(|d| d.code == Code::E019).count(),
        2,
        "the declared label should not be rejected: {diagnostics:#?}"
    );

    let (choice, _) = resolved
        .classifiers
        .get_mut("coverage_intent")
        .expect("choice classifier");
    choice.labels.clear();
    choice.min_confidence = Some(1.25);
    let (score, _) = resolved
        .classifiers
        .get_mut("coverage_score")
        .expect("score classifier");
    score.rubric.clear();
    score.upstream = None;

    let diagnostics = validate(&resolved, &ValidateCtx::default());
    for message in [
        "[classifier.coverage_intent] question `choice` needs a [classifier.coverage_intent.labels] table",
        "[classifier.coverage_intent] min_confidence must be between 0 and 1",
        "[classifier.coverage_score] question `score` needs `rubric = [..]` — ordered level descriptions, worst to best",
        "[classifier.coverage_score] has no upstream — nothing can answer it",
    ] {
        assert_diagnostic(&diagnostics, Code::E019, message);
    }
}

#[test]
fn routes_require_the_destination_for_their_site() {
    let mut resolved = resolve_fixture(
        r#"
[upstream.coverage_worker]
provider = "anthropic"
model = "fixture"

[[proxy.request.model]]
name = "coverage_model"
action = "route"
upstream = "coverage_worker"

[[hook.after_tool.shell]]
name = "coverage_hook"
action = "route"
when = { tokens_gt = "many" }
"#,
    );

    let diagnostics = rule_diagnostics(&resolved, "coverage_hook");
    assert_diagnostic(
        &diagnostics,
        Code::E014,
        "`route` needs `task = \"<name>\"`",
    );
    assert!(
        diagnostics.iter().any(|d| d.code == Code::E002),
        "a nonnumeric token threshold must be rejected: {diagnostics:#?}"
    );
    let shell = Position::hook(Event::BeforeTool, Subject::Shell);
    assert_eq!(route_kind_mismatch(shell, "coverage_plugin"), None);
    assert!(route_kind_mismatch(shell, "generate").is_some());

    for (action, upstream, code, message) in [
        (
            "route",
            None,
            Code::E014,
            "`proxy.request.model` routing needs `upstream = \"<name>\"`",
        ),
        (
            "route",
            Some("coverage_missing"),
            Code::E014,
            "`route` names unknown upstream `coverage_missing`",
        ),
        (
            "route",
            Some("coverage_worker"),
            Code::W008,
            "`proxy.request.model` routing is not active in this release — the rule is parsed and inert",
        ),
        (
            "passthrough",
            Some("coverage_worker"),
            Code::E006,
            "`proxy.request.model` takes `action = \"route\"`",
        ),
    ] {
        let rule = resolved
            .rules
            .get_mut(&Position::proxy(Subject::Model))
            .expect("model rules")
            .iter_mut()
            .find(|rule| rule.name == "coverage_model")
            .expect("user model rule");
        rule.def.action = Some(action.into());
        rule.def.upstream = upstream.map(str::to_owned);

        let diagnostics = rule_diagnostics(&resolved, "coverage_model");
        assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
        assert_diagnostic(&diagnostics, code, message);
    }
}

#[test]
fn named_chains_expand_builtins_and_preserve_terminal_constraints() {
    let mut resolved = resolve_fixture(
        r#"
[method.coverage_alias]
chain = ["ansi"]

[method.coverage_toon]
chain = ["toon"]

[[hook.after_tool.read]]
name = "coverage_terminal"
use = ["coverage_toon", "cap"]
"#,
    );
    resolved
        .methods
        .get_mut("coverage_alias")
        .expect("user chain")
        .0
        .chain = Some(vec![
        "term".into(),
        "coverage_toon".into(),
        "coverage_missing_method".into(),
    ]);

    let mut expanded: Vec<_> = registry::chain("term")
        .expect("built-in term chain")
        .iter()
        .map(|method| method.name().to_string())
        .collect();
    expanded.extend([
        "coverage_toon".to_string(),
        "coverage_missing_method".to_string(),
    ]);
    assert_eq!(chain_members(&resolved, "coverage_alias"), expanded);

    let diagnostics = validate(&resolved, &ValidateCtx::default());
    assert_diagnostic(
        &diagnostics,
        Code::E007,
        "[method.coverage_alias] chain names unknown method `coverage_missing_method`",
    );
    assert_eq!(
        diagnostics.iter().filter(|d| d.code == Code::E007).count(),
        1,
        "both the built-in and the declared chain should exist: {diagnostics:#?}"
    );
    assert_diagnostic(
        &rule_diagnostics(&resolved, "coverage_terminal"),
        Code::E008,
        "method `coverage_toon` is terminal and must be last in the chain",
    );
}

#[test]
fn named_chains_preserve_declared_formats() {
    const INPUT: &str = "coverage-unsupported-format";

    let mut resolved = resolve_fixture(
        r#"
[method.coverage_format_chain]
chain = ["ansi"]

[[proxy.request.tool_result]]
name = "coverage_format_rule"
use = ["coverage_format_chain", "ansi"]
"#,
    );
    let output_format = |produces| match produces {
        crate::method::Produces::Same => INPUT,
        crate::method::Produces::Format(format) => format.as_str(),
    };

    // Use the registry's declared formats, not assumptions about what a
    // particular method can parse at runtime.
    for name in &registry::known_names() {
        if PRO_METHODS.contains(&name.as_str()) {
            continue;
        }
        let Some(info) =
            registry::info_of(name).or_else(|| registry::chain(name).map(registry::chain_info))
        else {
            continue;
        };
        let entries = std::slice::from_ref(name);
        let wrapped = user_chain_info(&resolved, entries);
        let next_format = output_format(info.produces);
        assert_eq!(output_format(wrapped.produces), next_format, "{name}");

        resolved
            .methods
            .get_mut("coverage_format_chain")
            .expect("user chain")
            .0
            .chain = Some(entries.to_vec());
        let rule = resolved
            .rules
            .get_mut(&Position::proxy(Subject::ToolResult))
            .expect("tool result rules")
            .iter_mut()
            .find(|rule| rule.name == "coverage_format_rule")
            .expect("user format rule");
        rule.def.when = std::collections::BTreeMap::from([(
            "format".to_string(),
            toml::Value::String(INPUT.to_string()),
        )]);
        rule.def.use_ = vec!["coverage_format_chain".to_string(), name.to_string()];

        let diagnostics = rule_diagnostics(&resolved, "coverage_format_rule");
        for (method, format) in [
            ("coverage_format_chain", INPUT),
            (name.as_str(), next_format),
        ] {
            let message = format!("method `{method}` does not consume `{format}`");
            let rejects_format = !info.consumes.is_empty()
                && !info.consumes.iter().any(|input| input.as_str() == format);
            assert_eq!(
                diagnostics
                    .iter()
                    .any(|d| d.code == Code::E008 && d.message == message),
                rejects_format,
                "wrapping `{name}`: {message}\nactual diagnostics: {diagnostics:#?}"
            );
        }
    }
}

#[test]
fn a_user_chain_inherits_its_members_cache_stability_in_the_proxy() {
    let mut resolved = resolve_fixture(
        r#"
[method.coverage_cache_chain]
chain = ["ansi"]

[[proxy.request.tool_result]]
name = "coverage_cache_rule"
use = ["coverage_cache_chain"]
"#,
    );

    for name in &registry::known_names() {
        let Some(info) =
            registry::info_of(name).or_else(|| registry::chain(name).map(registry::chain_info))
        else {
            continue;
        };
        resolved
            .methods
            .get_mut("coverage_cache_chain")
            .expect("user chain")
            .0
            .chain = Some(vec!["ansi".into(), name.to_string()]);

        let diagnostics = rule_diagnostics(&resolved, "coverage_cache_rule");
        assert_eq!(
            diagnostics.iter().any(|d| d.code == Code::E010),
            !info.cache_stable,
            "wrapping `{name}` after `ansi` must preserve its cache stability: {diagnostics:#?}"
        );
        if !info.cache_stable {
            assert_diagnostic(
                &diagnostics,
                Code::E010,
                "method `coverage_cache_chain` is not cache-stable and cannot run under `proxy.request`",
            );
        }
    }
}
