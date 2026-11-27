use super::*;

#[test]
fn dashdash_helpers_distinguish_token_index_from_argument_index() {
    let args = ["-abc", "input", "--", "--", "--output=value"];
    let tokens = tokenize(&args);

    // The short cluster expands into three tokens from one argument.
    assert_eq!(dashdash_index(&tokens), Some(4));
    assert!(has_dashdash(&tokens));
    assert_eq!(injection_point(&tokens, args.len()), 2);

    let prefix = before_dashdash(&tokens);
    assert_eq!(prefix, &tokens[..4]);
    assert_eq!(dashdash_index(prefix), None);
    assert!(!has_dashdash(prefix));
    assert_eq!(before_dashdash(prefix), prefix);
    assert_eq!(injection_point(prefix, 2), 2);
}

#[test]
fn double_dash_flag_values_keep_order_and_skip_nonmatching_or_valueless_flags() {
    let args = [
        "/output:slash",
        "-output:single",
        "--OUTPUT=attached",
        "--other=ignored",
        "--output",
        "--",
        "--output",
        "separate",
        "--output=",
    ];
    let tokens = tokenize_grammar(
        &args,
        &|kind, name| match (kind, name) {
            (TokenKind::Long, "output") => Some(ValueSpec::value()),
            _ => None,
        },
        Dialect::Msbuild,
    );

    // The first bare --output cannot consume the boundary, but MSBuild still
    // classifies the flags that follow it.
    let values: Vec<_> = double_dash_flag_values(&tokens, Dialect::Msbuild, "output").collect();
    assert_eq!(values, ["attached", "separate", ""]);

    let separate = &tokens[7];
    assert_eq!(separate.text, "separate");
    assert_eq!(separate.linked, Some(6));
    assert_eq!(separate.value(&tokens), None);
}

#[test]
fn short_flag_value_specs_control_which_arguments_are_consumed() {
    let args = [
        "-a", "free-a", "-ainline", "-xs", "free-s", "-s", "solo", "-xv", "cluster", "-c", "--",
        "-xc", "free-c", "-s", "--", "tail",
    ];
    let tokens = tokenize_grammar(
        &args,
        &|kind, name| match (kind, name) {
            (TokenKind::Short, "a") => Some(ValueSpec::attached_only()),
            (TokenKind::Short, "s") => Some(ValueSpec::solo_only()),
            (TokenKind::Short, "v") => Some(ValueSpec::value()),
            (TokenKind::Short, "c") => Some(ValueSpec::solo_only().claiming_dash_dash()),
            _ => None,
        },
        Dialect::Posix,
    );

    let short_values: Vec<_> = tokens
        .iter()
        .filter(|token| token.kind == TokenKind::Short)
        .map(|token| (token.text, token.value(&tokens)))
        .collect();
    assert_eq!(
        short_values,
        [
            ("a", None),
            ("a", Some("inline")),
            ("x", None),
            ("s", None),
            ("s", Some("solo")),
            ("x", None),
            ("v", Some("cluster")),
            ("c", Some("--")),
            ("x", None),
            ("c", None),
            ("s", None),
        ]
    );

    let free_positionals: Vec<_> = tokens
        .iter()
        .filter(|token| token.is_free_positional())
        .map(|token| token.text)
        .collect();
    assert_eq!(free_positionals, ["free-a", "free-s", "free-c", "tail"]);
    assert_eq!(injection_point(&tokens, args.len()), 14);
}
