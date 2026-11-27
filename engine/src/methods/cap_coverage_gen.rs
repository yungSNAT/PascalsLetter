use super::{Cap, MAX_LINE_CHARS, Method, MethodCtx, Outcome, estimate_tokens};
use crate::block::ContentFormat;

fn apply_cap(input: &str) -> String {
    let block = crate::testutil::block(input, ContentFormat::Term);
    let Outcome::Continue(output) = Cap.apply(&block, &MethodCtx { source: "bash" }) else {
        panic!("cap should return Continue");
    };
    output
}

#[test]
fn preserves_trailing_newline_after_capping_a_line() {
    let kept = "x".repeat(MAX_LINE_CHARS);
    let input = format!("{kept}{}\n", "x".repeat(500));

    assert_eq!(apply_cap(&input), format!("{kept} [+500 chars]\n"));
}

#[test]
fn keeps_original_when_truncation_marker_increases_token_count() {
    let kept = "x".repeat(MAX_LINE_CHARS);
    let input = format!("{kept}x");
    let truncated = format!("{kept} [+1 chars]");

    assert!(estimate_tokens(&truncated) > estimate_tokens(&input));
    assert_eq!(apply_cap(&input), input);
}
