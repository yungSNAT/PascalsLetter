use super::*;
use crate::block::{Event, Subject};

fn parse_fixture(scope: Scope, label: &str, text: &str) -> SourceFile {
    let parsed = crate::pipeline::parse(text, label);
    assert!(
        parsed.diagnostics.is_empty(),
        "{label}: {:#?}",
        parsed.diagnostics
    );
    SourceFile {
        scope,
        label: label.to_owned(),
        file: parsed.file.expect("fixture parsed"),
    }
}

#[test]
fn resolving_local_entries_preserves_scope_order_and_handles_missing_names() {
    let mut user = parse_fixture(
        Scope::User,
        "user.toml",
        r#"
[pipeline]
extends = ["rtk:posthook", "rtk:posthook"]

[[hook.after_tool.shell]]
use = ["ansi"]
"#,
    );
    user.file.extends.insert(1, "not-a-builtin".to_owned());

    let position = Position::hook(Event::AfterTool, Subject::Shell);
    let rules = user.file.rules.get_mut(&position).expect("fixture rules");
    rules[0].use_.clear();
    rules[0].action = Some("route".to_owned());
    assert_eq!(
        effective_rule_name(&rules[0]),
        Some(("route".to_owned(), false))
    );

    let mut nameless = rules[0].clone();
    nameless.action = None;
    assert_eq!(effective_rule_name(&nameless), None);
    rules.push(nameless);

    let mut directive = builtins::builtin(builtins::RTK_DEFAULT)
        .expect("default builtin")
        .directives
        .first()
        .expect("default directive")
        .clone();
    directive.name = None;
    directive.use_ = None;
    directive.text = Some("Anonymous fixture text".to_owned());
    user.file.directives.push(directive);

    let project = parse_fixture(
        Scope::Project,
        "project.toml",
        r#"
[[hook.after_tool.shell]]
name = "coverage-project"
use = ["dedup"]
"#,
    );

    let (resolved, diagnostics) = resolve(&[user, project]);
    assert_eq!(resolved.extends, ["rtk:posthook", "rtk:default"]);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    let diagnostic = &diagnostics[0];
    assert_eq!(diagnostic.code, Code::E002);
    assert!(diagnostic.message.contains("`not-a-builtin`"));
    assert!(
        diagnostic.hint.as_deref().is_some_and(|hint| {
            hint.starts_with("built-ins: ") && hint.contains("rtk:posthook")
        })
    );

    let rules = resolved.rules_at(position);
    let names_and_scopes: Vec<_> = rules
        .iter()
        .map(|rule| (rule.name.as_str(), rule.scope.as_str()))
        .collect();
    assert_eq!(
        names_and_scopes,
        [
            ("coverage-project", "project"),
            ("route", "user"),
            ("term-default", "builtin"),
        ]
    );
    assert_eq!(insertion_point(rules, Scope::Builtin), 3);

    let directive = resolved
        .directives
        .iter()
        .find(|directive| directive.name == "directive")
        .expect("the unnamed local directive is retained");
    assert_eq!(directive.origin, "user.toml");
    assert_eq!(directive.scope, Scope::User);
    assert_eq!(
        directive.def.text.as_deref(),
        Some("Anonymous fixture text")
    );
    assert!(directive.overridden_by.is_empty());
}

fn assert_inherited_origins<'a, T, I>(
    actual: &BTreeMap<String, (T, String)>,
    sources: &[(&'a str, &'a PipelineFile)],
    keys: impl Fn(&'a PipelineFile) -> I,
) where
    I: IntoIterator<Item = &'a String>,
{
    let expected: BTreeMap<_, _> = sources
        .iter()
        .rev()
        .flat_map(|&(origin, file)| {
            keys(file)
                .into_iter()
                .map(move |name| (name.as_str(), origin))
        })
        .collect();
    let actual: BTreeMap<_, _> = actual
        .iter()
        .map(|(name, (_, origin))| (name.as_str(), origin.as_str()))
        .collect();
    assert_eq!(actual, expected);
}

#[test]
fn builtin_stack_keeps_first_definitions_and_reports_conflicting_rules() {
    let mut user = parse_fixture(
        Scope::User,
        "stack.toml",
        "[pipeline]\nextends = \"rtk:posthook\"\n",
    );
    let requested: Vec<_> = builtins::BUILTIN_NAMES
        .iter()
        .map(|&name| name.to_owned())
        .collect();
    user.file.extends.clone_from(&requested);
    let (resolved, diagnostics) = resolve(&[user]);
    assert_eq!(resolved.extends, requested);

    let sources: Vec<_> = builtins::BUILTIN_NAMES
        .iter()
        .filter_map(|&name| builtins::builtin(name).map(|file| (name, file)))
        .collect();
    let mut rule_definitions = BTreeMap::<_, Vec<_>>::new();
    let mut directive_origins = BTreeMap::new();

    for &(origin, file) in &sources {
        for def in file.rules.values().flatten() {
            if let Some((name, _)) = effective_rule_name(def) {
                rule_definitions
                    .entry(name)
                    .or_default()
                    .push((origin, def));
            }
        }
        for def in &file.directives {
            if let Some((name, _)) = effective_directive_name(def) {
                directive_origins.entry(name).or_insert(origin);
            }
        }
    }

    assert_eq!(
        resolved.rules.values().flatten().count(),
        rule_definitions.len()
    );
    let mut expected_conflicts = Vec::new();
    for (name, definitions) in rule_definitions {
        let (first_origin, first_def) = definitions[0];
        let rule = resolved.find_rule(&name).expect("inherited rule");
        assert_eq!(rule.origin, first_origin);
        assert_eq!(rule.scope, Scope::Builtin);
        for &(origin, def) in &definitions[1..] {
            if def != first_def {
                expected_conflicts.push(format!(
                    "`extends`: `{origin}` and `{first_origin}` both define rule `{name}` differently"
                ));
            }
        }
    }
    let mut conflicts: Vec<_> = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == Code::E017)
        .map(|diagnostic| diagnostic.message.clone())
        .collect();
    conflicts.sort_unstable();
    expected_conflicts.sort_unstable();
    assert_eq!(conflicts, expected_conflicts);

    assert_eq!(resolved.directives.len(), directive_origins.len());
    for directive in &resolved.directives {
        assert_eq!(directive.origin, directive_origins[&directive.name]);
        assert_eq!(directive.scope, Scope::Builtin);
    }

    assert_inherited_origins(&resolved.methods, &sources, |file| file.methods.keys());
    assert_inherited_origins(&resolved.upstreams, &sources, |file| file.upstreams.keys());
    assert_inherited_origins(&resolved.classifiers, &sources, |file| {
        file.classifiers.keys()
    });
    assert_eq!(
        resolved.guards,
        Guards {
            never_worse: sources
                .iter()
                .find_map(|(_, file)| file.guards.never_worse)
                .unwrap_or(Guards::default().never_worse),
            min_size: sources
                .iter()
                .find_map(|(_, file)| file.guards.min_size)
                .unwrap_or(Guards::default().min_size),
        }
    );
}

#[test]
fn project_definitions_merge_fields_into_user_definitions() {
    let mut user = parse_fixture(
        Scope::User,
        "user.toml",
        r#"
[method.coverage-chain]
chain = ["ansi"]

[task.coverage-summary]
kind = "summarize"
upstream = "coverage-original"

[upstream.coverage-api]
provider = "openai"
model = "user-model"
"#,
    );
    let mut project = parse_fixture(
        Scope::Project,
        "project.toml",
        r#"
[method.coverage-chain]
chain = ["dedup"]

[task.coverage-summary]
upstream = "coverage-api"

[upstream.coverage-api]
provider = "google"
model = "project-model"
"#,
    );

    let base_method = user
        .file
        .methods
        .get_mut("coverage-chain")
        .expect("user method");
    base_method.options.extend([
        ("retained".to_owned(), toml::Value::Integer(17)),
        ("replaced".to_owned(), toml::Value::Integer(20)),
    ]);
    let method = project
        .file
        .methods
        .get_mut("coverage-chain")
        .expect("project method");
    method.options.extend([
        ("replaced".to_owned(), toml::Value::Integer(30)),
        ("added".to_owned(), toml::Value::Boolean(true)),
    ]);
    let expected_chain = method.chain.clone().expect("project chain");
    assert_ne!(base_method.chain.as_ref(), Some(&expected_chain));

    let task = project
        .file
        .tasks
        .get_mut("coverage-summary")
        .expect("project task");
    task.temperature = Some(0.25);
    task.instructions = Some("Keep the headings.".to_owned());
    task.prompt = Some("Summarize the fixture.".to_owned());

    let upstream = project
        .file
        .upstreams
        .get_mut("coverage-api")
        .expect("project upstream");
    upstream.base_url = Some("https://resolver.invalid/v1".to_owned());
    upstream.api_key_env = Some("RTK_COVERAGE_KEY".to_owned());
    upstream.api_key_literal = true;
    upstream.timeout_secs = Some(37);

    let (resolved, diagnostics) = resolve(&[user, project]);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");

    let (method, origin) = &resolved.methods["coverage-chain"];
    assert_eq!(origin, "project.toml");
    assert_eq!(method.chain.as_ref(), Some(&expected_chain));
    assert_eq!(
        method.options.get("retained"),
        Some(&toml::Value::Integer(17))
    );
    assert_eq!(
        method.options.get("replaced"),
        Some(&toml::Value::Integer(30))
    );
    assert_eq!(
        method.options.get("added"),
        Some(&toml::Value::Boolean(true))
    );

    let (task, origin) = &resolved.tasks["coverage-summary"];
    assert_eq!(origin, "project.toml");
    assert_eq!(task.kind, "summarize");
    assert_eq!(task.upstream.as_deref(), Some("coverage-api"));
    assert_eq!(task.temperature, Some(0.25));
    assert_eq!(task.instructions.as_deref(), Some("Keep the headings."));
    assert_eq!(task.prompt.as_deref(), Some("Summarize the fixture."));

    let (upstream, origin) = &resolved.upstreams["coverage-api"];
    assert_eq!(origin, "project.toml");
    assert_eq!(upstream.provider, "google");
    assert_eq!(upstream.model, "project-model");
    assert_eq!(
        upstream.base_url.as_deref(),
        Some("https://resolver.invalid/v1")
    );
    assert_eq!(upstream.api_key_env.as_deref(), Some("RTK_COVERAGE_KEY"));
    assert!(upstream.api_key_literal);
    assert_eq!(upstream.timeout_secs, Some(37));
}

fn set_patch(source: &mut SourceFile, name: &str, text: &str) {
    source
        .file
        .overrides
        .get_mut(name)
        .expect("override target")
        .patch = toml::from_str(text).expect("patch is syntactically valid TOML");
}

#[test]
fn overrides_apply_valid_fields_and_leave_previous_values_on_rejected_edits() {
    let mut user = parse_fixture(
        Scope::User,
        "user.toml",
        r#"
[[hook.after_tool.shell]]
name = "coverage-patch"
use = ["ansi"]
when = { tokens_gt = 100 }

[override.coverage-patch]
enabled = true

[override.rtk-awareness]
enabled = false
"#,
    );
    set_patch(
        &mut user,
        "coverage-patch",
        r#"
action = "route"
task = "coverage-task"
upstream = "coverage-api"
message = "Patched message"
mode = "replace"
continue = true
use = ["dedup", false, "cap"]
"#,
    );
    set_patch(
        &mut user,
        "rtk-awareness",
        r#"
enabled = false
channel = "system"
text = "Patched directive text"
"#,
    );

    let mut project = parse_fixture(
        Scope::Project,
        "project.toml",
        "[override.coverage-patch]\nenabled = true\n",
    );
    // Inject these after schema parsing to exercise the resolver's own checks.
    set_patch(
        &mut project,
        "coverage-patch",
        r#"
action = false
task = 42
upstream = []
message = {}
mode = true
use = "not-an-array"
when = false
name = "attempted-rename"
enabld = false
"#,
    );

    let (resolved, diagnostics) = resolve(&[user, project]);
    let rule = resolved
        .find_rule("coverage-patch")
        .expect("original name retained");
    assert!(resolved.find_rule("attempted-rename").is_none());
    assert_eq!(rule.def.action.as_deref(), Some("route"));
    assert_eq!(rule.def.task.as_deref(), Some("coverage-task"));
    assert_eq!(rule.def.upstream.as_deref(), Some("coverage-api"));
    assert_eq!(rule.def.message.as_deref(), Some("Patched message"));
    assert_eq!(rule.def.mode.as_deref(), Some("replace"));
    assert!(rule.def.enabled);
    assert!(rule.def.continue_);
    assert_eq!(rule.def.use_, ["dedup", "cap"]);
    assert_eq!(
        rule.when_value("tokens_gt"),
        Some(&toml::Value::Integer(100))
    );
    let conditions = rule
        .when
        .as_ref()
        .expect("original condition still compiles");
    assert_eq!(conditions.len(), 1);
    assert_eq!(conditions[0].key, "tokens");
    assert_eq!(rule.overridden_by, ["user.toml", "project.toml"]);

    assert_eq!(diagnostics.len(), 7, "{diagnostics:#?}");
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code == Code::E002)
    );
    for key in ["action", "task", "upstream", "message", "mode"] {
        let expected = format!("`{key}` expects a string");
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains(&expected))
        );
    }
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("cannot change `name`"))
    );
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.message.contains("unknown key `enabld`")
            && diagnostic.message.contains("did you mean `enabled`?")
    }));

    let directive = resolved
        .directives
        .iter()
        .find(|directive| directive.name == "rtk-awareness")
        .expect("inherited directive");
    assert_eq!(directive.scope, Scope::Builtin);
    assert!(!directive.def.enabled);
    assert_eq!(directive.def.channel.as_deref(), Some("system"));
    assert_eq!(
        directive.def.text.as_deref(),
        Some("Patched directive text")
    );
    assert_eq!(directive.overridden_by, ["user.toml"]);
}

#[test]
fn required_env_lists_only_used_upstreams_that_need_environment_credentials() {
    let mut user = parse_fixture(
        Scope::User,
        "credentials.toml",
        r#"
[upstream.coverage-anthropic]
provider = "anthropic"
model = "fixture"

[upstream.coverage-command]
provider = "openai"
model = "fixture"
api_key_env = "RTK_IGNORED_COMMAND_KEY"

[upstream.coverage-custom]
provider = "openai"
model = "fixture"
api_key_env = "RTK_COVERAGE_CUSTOM_KEY"

[upstream.coverage-google]
provider = "google"
model = "fixture"

[upstream.coverage-openai]
provider = "openai"
model = "fixture"

[upstream.coverage-unknown]
provider = "openai"
model = "fixture"

[upstream.coverage-unused]
provider = "anthropic"
model = "fixture"

[task.coverage-summary]
kind = "summarize"
upstream = "coverage-anthropic"

[task.coverage-command]
kind = "summarize"
upstream = "coverage-command"

[task.coverage-custom]
kind = "summarize"
upstream = "coverage-custom"

[task.coverage-google]
kind = "summarize"
upstream = "coverage-google"

[task.coverage-openai]
kind = "summarize"
upstream = "coverage-openai"

[task.coverage-unknown]
kind = "summarize"
upstream = "coverage-unknown"

[classifier.coverage-topic]
upstream = "coverage-openai"
"#,
    );
    // Only the presence of the command matters; required_env never runs it.
    user.file
        .upstreams
        .get_mut("coverage-command")
        .expect("command upstream")
        .api_key_command = Some(Default::default());
    user.file
        .upstreams
        .get_mut("coverage-unknown")
        .expect("unknown-provider upstream")
        .provider = "fixture-provider".to_owned();

    let (resolved, diagnostics) = resolve(&[user]);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
    let expected = [
        (
            "ANTHROPIC_API_KEY",
            "coverage-anthropic",
            &["task `coverage-summary`"][..],
        ),
        (
            "RTK_COVERAGE_CUSTOM_KEY",
            "coverage-custom",
            &["task `coverage-custom`"][..],
        ),
        (
            "GEMINI_API_KEY",
            "coverage-google",
            &["task `coverage-google`"][..],
        ),
        (
            "OPENAI_API_KEY",
            "coverage-openai",
            &["task `coverage-openai`", "classifier `coverage-topic`"][..],
        ),
    ]
    .map(|(env, upstream, users)| {
        (
            env.to_owned(),
            upstream.to_owned(),
            users
                .iter()
                .map(|&name| name.to_owned())
                .collect::<Vec<_>>(),
        )
    });
    assert_eq!(resolved.required_env(), expected);
}
