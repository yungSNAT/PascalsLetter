use super::*;
use serde_json::json;

fn jsonl(entries: &[Value]) -> String {
    entries
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn assemble_reads_short_transcript_and_trims_context_by_character() {
    let dir = crate::core::test_isolation::tempdir();
    let path = dir.path().join("short.jsonl");
    let goal = "é".repeat(GOAL_CHARS);
    let intent = "界".repeat(INTENT_CHARS);
    let transcript = jsonl(&[
        json!({"role": "user", "content": format!("{goal} trailing instructions")}),
        json!({"role": "assistant", "content": format!("earlier reasoning {intent}")}),
    ]);
    std::fs::write(&path, transcript).expect("write short transcript");

    let context = assemble(&Source {
        transcript: Some(path.to_str().expect("UTF-8 transcript path").to_owned()),
        description: Some("fallback description".into()),
        ..Source::default()
    });

    assert_eq!(
        context,
        CallContext {
            goal: Some(goal),
            intent: Some(intent),
            recent_calls: Vec::new(),
        }
    );
}

#[test]
fn read_tail_drops_the_clipped_line_and_decodes_the_retained_line_lossily() {
    let dir = crate::core::test_isolation::tempdir();
    let path = dir.path().join("large.jsonl");
    let clipped = b"{\"role\":\"user\",\"content\":\"clipped prompt\"}\n";
    let retained = b"{\"role\":\"assistant\",\"content\":\"last response: \xff\"}\n";
    let padding =
        usize::try_from(TAIL_BYTES).expect("tail size fits usize") - clipped.len() - retained.len();

    // The tail starts inside the first line, whose remainder alone looks like valid JSON.
    let mut bytes = b"discarded prefix: ".to_vec();
    bytes.resize(bytes.len() + padding, b' ');
    bytes.extend_from_slice(clipped);
    bytes.extend_from_slice(retained);
    std::fs::write(&path, bytes).expect("write large transcript");

    let tail = read_tail(path.to_str().expect("UTF-8 transcript path")).expect("read tail");
    let expected = "{\"role\":\"assistant\",\"content\":\"last response: \u{fffd}\"}\n";
    assert_eq!(tail.len(), expected.len());
    assert_eq!(tail, expected);
}

#[test]
fn transcript_ignores_noisy_blocks_and_renders_alternate_call_fields() {
    let mut entries = vec![
        json!({"role": "user", "content": "  Inspect the fixture  "}),
        json!({"role": "user", "content": [
            {"type": "input_text", "text": " \t "},
            {"type": "text", "text": " <context>not a goal</context> "}
        ]}),
        json!({"role": "assistant", "content": [
            {"type": "thinking", "thinking": "  Start with the fixture  "}
        ]}),
        json!({"type": "response_item", "payload": {
            "type": "custom_tool_call",
            "name": "inspect",
            "input": {"path": "fixture.json"},
            "action": {"path": "unused.json"}
        }}),
        json!({"type": "response_item", "payload": {
            "type": "local_shell_call",
            "action": {"command": 42}
        }}),
        json!({"type": "response_item", "payload": {
            "type": "reasoning",
            "summary": {"text": "not a summary array"}
        }}),
    ];
    let mut expected = CallContext {
        goal: Some("Inspect the fixture".into()),
        intent: Some("Start with the fixture".into()),
        recent_calls: vec!["local_shell_call 42".into(), "inspect fixture.json".into()],
    };
    assert_eq!(from_transcript(&jsonl(&entries)), expected);

    entries.push(json!({"role": "assistant", "content": [
        {"type": "output_text", "text": "  The fixture is ready  "},
        {"type": "output_text", "text": false},
        {"type": "thinking", "thinking": null},
        {"type": "image", "text": "not assistant text"},
        {"type": "text", "text": " \n\t "}
    ]}));
    expected.intent = Some("The fixture is ready".into());
    assert_eq!(from_transcript(&jsonl(&entries)), expected);
}
