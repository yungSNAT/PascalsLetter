use super::*;
use toml::Value;

#[test]
fn classifier_confidence_is_a_tool_hook_condition() {
    let key = "class.review.confidence";
    assert_eq!(classifier_condition(key), Some(("choice", "review", true)));
    for malformed in ["class.", "class..confidence", "class.review.extra"] {
        assert_eq!(classifier_condition(malformed), None, "{malformed}");
    }

    for event in [Event::BeforeTool, Event::AfterTool] {
        assert_eq!(
            check_key(Position::hook(event, Subject::Read), key),
            KeyCheck::Ok
        );
    }

    let mut non_tool_hook = Position::hook(Event::AfterTool, Subject::Read);
    non_tool_hook.event = Position::proxy(Subject::User).event;
    assert_eq!(
        check_key(non_tool_hook, key),
        KeyCheck::WrongEvent { hint: None }
    );
}

#[test]
fn hook_keys_respect_subject_and_event() {
    for (subject, key, expected) in [
        (Subject::Write, "file_path", KeyCheck::Ok),
        (Subject::Edit, "extension", KeyCheck::Ok),
        (Subject::Edit, "whole_file", KeyCheck::Unknown),
        (Subject::Web, "url", KeyCheck::Ok),
        (Subject::Web, "basename", KeyCheck::Unknown),
        (Subject::Search, "file_path", KeyCheck::Unknown),
        (Subject::Subagent, "command", KeyCheck::Unknown),
        (Subject::Any, "whole_file", KeyCheck::Ok),
        (Subject::Any, "file_exists", KeyCheck::Ok),
        (Subject::Any, "file_path", KeyCheck::Ok),
        (Subject::Any, "command", KeyCheck::Ok),
        (Subject::Any, "exit_code", KeyCheck::Ok),
        (Subject::Any, "url", KeyCheck::Ok),
        (Subject::Any, "server", KeyCheck::Ok),
        (Subject::User, "file_path", KeyCheck::Unknown),
    ] {
        assert_eq!(
            check_hook_key(Event::AfterTool, subject, key),
            expected,
            "{key}"
        );
    }

    assert_eq!(
        check_hook_key(Event::BeforeTool, Subject::Read, "file_bytes"),
        KeyCheck::Ok
    );
    assert_eq!(
        check_hook_key(Event::AfterTool, Subject::Read, "file_bytes"),
        KeyCheck::WrongEvent {
            hint: Some("`file_bytes` reads the disk before the tool runs".into()),
        }
    );
}

#[test]
fn malformed_operands_report_the_key_and_expected_type() {
    for (key, value, expected) in [
        (
            "tool_not_in",
            Value::Array(vec!["read".into(), Value::Boolean(true)]),
            "`tool_not_in`: list items must be strings",
        ),
        ("tool_in", "read".into(), "`tool_in` expects an array"),
        (
            "command_matches",
            Value::Integer(1),
            "`command_matches` expects a regex string",
        ),
        ("tokens_gt", "100".into(), "`tokens_gt` expects a number"),
        (
            "command_starts_with",
            Value::Boolean(true),
            "`command_starts_with` expects a string",
        ),
        (
            "input_missing",
            Value::Array(vec!["limit".into(), Value::Integer(1)]),
            "`input_missing`: list items must be strings",
        ),
        (
            "tool",
            Value::Array(vec!["read".into()]),
            "`tool`: bare key with an array (use `tool_in`)",
        ),
        (
            "tool",
            Value::Table(toml::map::Map::new()),
            "`tool`: unsupported value type",
        ),
    ] {
        assert_eq!(
            parse_condition(key, &value).expect_err("operand should be rejected"),
            expected
        );
    }
}

#[test]
fn input_missing_checks_field_presence_instead_of_scalar_equality() {
    let missing = parse_condition(
        "input_missing",
        &Value::Array(vec!["limit".into(), "offset".into()]),
    )
    .expect("valid field list");
    let scalar =
        parse_condition("input_missing", &Value::Boolean(true)).expect("valid scalar syntax");

    let unrelated = Facts::from([("input_missing".into(), Fact::Bool(true))]);
    assert!(missing.matches(&unrelated));
    assert!(!scalar.matches(&unrelated));

    for key in ["input.limit", "input.offset"] {
        let present = Facts::from([(key.into(), Fact::Bool(false))]);
        assert!(!missing.matches(&present), "{key} is present");
    }
}

#[test]
fn suffix_only_key_supports_numeric_equality_and_an_inclusive_bound() {
    let threshold = Value::Float(1.5);
    let equal = parse_condition("_lte", &threshold).expect("literal key with an empty suffix base");
    let at_most = parse_condition("_lte_lte", &threshold).expect("inclusive upper bound");
    assert_eq!(equal.key, "_lte");
    assert_eq!(equal.op, Op::Eq);
    assert_eq!(at_most.key, "_lte");
    assert_eq!(at_most.op, Op::Lte);

    for (fact, expected_equal, expected_at_most) in [
        (Fact::Num(1.0), false, true),
        (Fact::Num(1.5), true, true),
        (Fact::Num(2.0), false, false),
        (Fact::Str("1.5".into()), false, false),
    ] {
        let facts = Facts::from([("_lte".into(), fact)]);
        assert_eq!(equal.matches(&facts), expected_equal, "{facts:?}");
        assert_eq!(at_most.matches(&facts), expected_at_most, "{facts:?}");
    }
}
