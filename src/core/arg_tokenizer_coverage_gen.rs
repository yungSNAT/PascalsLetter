use super::*;

#[test]
fn has_dashdash_distinguishes_separator_from_flag_value() {
    let args = ["-e", "--", "-n"];
    let takes = |kind: TokenKind, name: &str| {
        (kind == TokenKind::Short && name == "e").then(ValueSpec::value)
    };

    let tokens = tokenize_grammar(&args, &takes, Dialect::Posix);
    assert!(has_dashdash(&tokens));
    assert_eq!(tokens[0].value(&tokens), None);
    assert!(tokens[2].is_free_positional());

    let tokens = tokenize_grammar(
        &args,
        &|kind, name| takes(kind, name).map(ValueSpec::claiming_dash_dash),
        Dialect::Posix,
    );
    assert!(!has_dashdash(&tokens));
    assert_eq!(tokens[0].value(&tokens), Some("--"));
    assert_eq!(tokens[2].kind, TokenKind::Short);
    assert_eq!(tokens[2].text, "n");
}
