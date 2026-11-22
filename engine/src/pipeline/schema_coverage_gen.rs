use super::*;
use crate::block::{Event, Subject};

fn assert_e002_hints(diagnostics: &[Diagnostic], expected: &[Option<&str>]) {
    let mut actual: Vec<_> = diagnostics
        .iter()
        .map(|diag| {
            assert_eq!(diag.code, Code::E002, "{diag:?}");
            diag.hint.as_deref()
        })
        .collect();
    let mut expected = expected.to_vec();
    actual.sort_unstable();
    expected.sort_unstable();
    assert_eq!(actual, expected, "{diagnostics:#?}");
}

#[test]
fn malformed_sections_report_errors_without_populating_the_file() {
    for (source, expected_code) in [
        ("schema_version = 2", Code::Load),
        ("pipeline = false", Code::E002),
        ("hook = false", Code::E001),
        ("[hook]\nbefore_tool = false", Code::E001),
        (
            "[hook.before_tool.read]\nname = \"single table\"",
            Code::E001,
        ),
        ("directive = false", Code::E001),
        ("task = false", Code::E001),
        ("guards = false", Code::E002),
    ] {
        let result = parse(source, "sections.toml");
        assert_eq!(
            result.diagnostics.len(),
            1,
            "{source}: {:?}",
            result.diagnostics
        );
        assert_eq!(result.diagnostics[0].code, expected_code, "{source}");
        assert_eq!(
            result.file,
            Some(PipelineFile {
                enabled: true,
                ..PipelineFile::default()
            }),
            "{source}"
        );
    }
}

#[test]
fn rule_errors_keep_their_context_and_do_not_discard_valid_rules() {
    let result = parse(
        r#"[[hook.after_tool.shell]]
action = 5
any = false
unexpected = true
use = "term"
when = false

[[hook.after_tool.shell]]
upstream = "local"
any = [
    { input = { limit_gt = 10 } },
    { input = "literal" },
]

[hook.before_tool]
read = [false, { name = "kept", use = ["term"] }]

[hook.after_tool.shelll]
name = "misspelled"
"#,
        "rules.toml",
    );

    let diagnostics = &result.diagnostics;
    assert_eq!(diagnostics.len(), 5, "{diagnostics:#?}");
    let subject = diagnostics
        .iter()
        .find(|diag| diag.code == Code::E018)
        .expect("the misspelled subject is diagnosed");
    assert_eq!(subject.hint.as_deref(), Some("did you mean `shell`?"));
    for code in [Code::E002, Code::W024] {
        let diag = diagnostics
            .iter()
            .find(|diag| diag.code == code && diag.line == Some(1))
            .expect("the diagnostic points to the unnamed rule");
        assert_eq!(diag.context.as_deref(), Some("[[hook.after_tool.shell]]"));
    }
    assert_eq!(
        diagnostics
            .iter()
            .filter(|diag| diag.code == Code::E002 && diag.line.is_none())
            .count(),
        2,
        "invalid `use` and `when` each produce a diagnostic"
    );

    let file = result.file.expect("the file still loads");
    assert_eq!(file.rules.len(), 2);
    let shell = &file.rules[&Position::hook(Event::AfterTool, Subject::Shell)];
    assert_eq!(shell.len(), 2);
    assert!(!shell[0].enabled);
    assert!(shell[0].any.is_empty());
    assert!(shell[0].use_.is_empty());
    assert!(shell[0].when.is_empty());
    assert!(shell[1].enabled);
    assert_eq!(shell[1].upstream.as_deref(), Some("local"));
    assert_eq!(shell[1].any.len(), 2);
    assert_eq!(shell[1].any[0]["input.limit_gt"].as_integer(), Some(10));
    assert!(!shell[1].any[0].contains_key("input"));
    assert_eq!(shell[1].any[1]["input"].as_str(), Some("literal"));

    let read = &file.rules[&Position::hook(Event::BeforeTool, Subject::Read)];
    assert_eq!(read.len(), 1);
    assert_eq!(read[0].name.as_deref(), Some("kept"));
    assert_eq!(read[0].use_, ["term"]);
}

#[test]
fn metadata_errors_preserve_valid_fields_and_only_use_directives_keep_options() {
    let result = parse(
        r#"
directive = [
    false,
    { text = "Keep this", channell = "startup" },
    { use = "custom", level = 2 },
]

[pipeline]
name = "example"
extends = false
enabld = false

[guards]
min_size = 512
never_wors = true
"#,
        "metadata.toml",
    );

    assert_e002_hints(
        &result.diagnostics,
        &[
            None,
            Some("did you mean `channel`?"),
            Some("did you mean `enabled`?"),
            Some("did you mean `never_worse`?"),
        ],
    );
    let file = result.file.expect("the file still loads");
    assert_eq!(file.name.as_deref(), Some("example"));
    assert!(file.enabled);
    assert!(file.extends.is_empty());
    assert_eq!(file.guards.min_size, Some(512));
    assert_eq!(file.guards.never_worse, None);

    assert_eq!(file.directives.len(), 2);
    assert_eq!(file.directives[0].text.as_deref(), Some("Keep this"));
    assert_eq!(file.directives[0].channel, None);
    assert!(file.directives[0].options.is_empty());
    assert_eq!(file.directives[1].use_.as_deref(), Some("custom"));
    assert_eq!(file.directives[1].options["level"].as_integer(), Some(2));
}

#[test]
fn named_definitions_keep_supported_values_despite_malformed_entries() {
    let result = parse(
        r#"
[method]
bad = false

[method.gentle]
chain = "term"
level = 3

[task.review]
kind = "summarize"
upstream = "local"
temperature = 1
instructions = "Use short sentences."
prompt = "Summarize this."

[classifier.review]
question = "score"
min_confidence = 1
context = "false"
rubric = ["low", 17, "high"]
labels = { relevant = "On topic" }
queston = "ignored"

[classifier.partial]
labels = "not a table"
rubric = false

[upstream.local]
provider = "openai"
model = "demo"
modell = "ignored"
"#,
        "definitions.toml",
    );

    assert_e002_hints(
        &result.diagnostics,
        &[
            None,
            None,
            None,
            Some("did you mean `model`?"),
            Some("did you mean `question`?"),
        ],
    );
    let file = result.file.expect("the file still loads");
    assert_eq!(file.methods.len(), 1);
    assert_eq!(file.methods["gentle"].chain, None);
    assert_eq!(
        file.methods["gentle"].options["level"].as_integer(),
        Some(3)
    );

    let task = &file.tasks["review"];
    assert_eq!(task.kind, "summarize");
    assert_eq!(task.upstream.as_deref(), Some("local"));
    assert_eq!(task.temperature, Some(1.0));
    assert_eq!(task.instructions.as_deref(), Some("Use short sentences."));
    assert_eq!(task.prompt.as_deref(), Some("Summarize this."));

    let classifier = &file.classifiers["review"];
    assert_eq!(classifier.question, "score");
    assert_eq!(classifier.min_confidence, Some(1.0));
    assert!(!classifier.context);
    assert_eq!(classifier.labels["relevant"], "On topic");
    assert_eq!(classifier.rubric, ["low", "high"]);
    assert!(file.classifiers["partial"].labels.is_empty());
    assert!(file.classifiers["partial"].rubric.is_empty());

    assert_eq!(file.upstreams["local"].provider, "openai");
    assert_eq!(file.upstreams["local"].model, "demo");
}
