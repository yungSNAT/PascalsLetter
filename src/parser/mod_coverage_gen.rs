use super::*;

#[test]
fn passthrough_map_keeps_raw_output_without_calling_the_mapper() {
    let mut mapper_called = false;
    let mapped = ParseResult::Passthrough("original tool output".to_owned()).map(|value: u8| {
        mapper_called = true;
        value.to_string()
    });

    assert!(!mapper_called);
    assert!(mapped.warnings().is_empty());
    assert!(matches!(
        mapped,
        ParseResult::Passthrough(raw) if raw == "original tool output"
    ));
}

#[test]
#[should_panic(expected = "Called unwrap on Passthrough result")]
fn passthrough_cannot_be_unwrapped_as_parsed_data() {
    let result = ParseResult::<u8>::Passthrough("unparsed tool output".to_owned());
    let _ = result.unwrap();
}

#[test]
fn tier_cap_preserves_allowed_data_and_truncates_rejected_input() {
    struct PartialParser;

    impl OutputParser for PartialParser {
        type Output = usize;

        fn parse(input: &str) -> ParseResult<Self::Output> {
            ParseResult::Degraded(input.chars().count(), vec!["partial data".to_owned()])
        }
    }

    let root = crate::core::test_isolation::tempdir();
    let _working_dir = crate::core::test_isolation::enter(root.path());
    crate::core::test_isolation::with_root(root.path(), || {
        let limit = crate::core::config::limits().passthrough_max_chars;
        let prefix = "é".repeat(limit);
        let input = format!("{prefix}!");
        let original_len = limit + 1;

        let accepted = PartialParser::parse_with_tier(&input, 2);
        assert_eq!(accepted.tier(), 2);
        assert_eq!(accepted.warnings(), ["partial data"]);
        assert_eq!(accepted.unwrap(), original_len);

        let rejected = PartialParser::parse_with_tier(&input, 1);
        let ParseResult::Passthrough(raw) = rejected else {
            panic!("expected passthrough when tier 2 is disallowed, got {rejected:?}");
        };
        assert_eq!(
            raw,
            format!(
                "{prefix}\n\n[RTK:PASSTHROUGH] Output truncated ({original_len} chars → {limit} chars)"
            )
        );
    });
}

#[test]
fn json_extraction_handles_escapes_after_a_banner_and_rejects_unclosed_objects() {
    let json = r#"{"message":"café \"} still inside","nested":{"path":"a\\"}}"#;
    let input = format!("Préparation\nrunning tests\n{json}\nfinished");
    assert_eq!(extract_json_object(&input), Some(json));

    let incomplete = json.strip_suffix('}').expect("fixture has a closing brace");
    assert_eq!(extract_json_object(incomplete), None);
}
