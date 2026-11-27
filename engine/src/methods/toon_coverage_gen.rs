use super::*;
use crate::block::ContentFormat;

#[test]
fn shape_gate_rejects_short_arrays_and_non_object_rows() {
    for value in [
        serde_json::json!([]),
        serde_json::json!([{"a": 1}]),
        serde_json::json!([{"a": 1}, null]),
    ] {
        assert!(
            !is_uniform_flat_array(&value),
            "unexpectedly eligible for TOON: {value}"
        );
    }
}

#[test]
fn uniform_array_keeps_minified_json_when_toon_is_not_smaller() {
    let input = "[\n  {},\n  {}\n]";
    let minified = "[{},{}]";
    let value: Value = serde_json::from_str(input).expect("valid JSON");
    assert!(is_uniform_flat_array(&value));

    let encoded = toon_format::encode_default(&value).expect("TOON can encode empty objects");
    assert!(
        encoded.len() >= minified.len(),
        "fixture must not save bytes with TOON: {encoded:?}"
    );

    let block = crate::testutil::block(input, ContentFormat::Json);
    let ctx = MethodCtx {
        source: "test.json",
    };
    let Outcome::ShortCircuit(output) = Toon.apply(&block, &ctx) else {
        panic!("expected minified JSON as a terminal result");
    };
    assert_eq!(output, minified);
}
