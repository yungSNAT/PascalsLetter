use super::*;

#[test]
fn newline_modes_distinguish_lone_cr_from_crlf() {
    let input = "é\rg\r\nz";
    let tokens = tokenize_with_newlines(input);
    let spans: Vec<_> = tokens
        .iter()
        .map(|token| (token.kind, token.value.as_str(), token.offset))
        .collect();

    assert_eq!(
        spans,
        [
            (TokenKind::Arg, "é\rg", 0),
            (TokenKind::Operator, "\n", 4),
            (TokenKind::Operator, "\n", 5),
            (TokenKind::Arg, "z", 6),
        ]
    );
    assert_eq!(split_for_permissions(input), ["é", "g", "z"]);
}

#[test]
fn comment_scan_tracks_backticks_inside_double_quotes() {
    let scan = blank_comments(r#"echo "`printf "%s # marker"`" # note"#);

    // The inner hash is ambiguous to the scanner; the trailing comment is not.
    assert!(scan.ambiguous);
    assert_eq!(scan.code, r#"echo "`printf "%s # marker"`"       "#);
}

#[test]
fn fish_token_check_distinguishes_combined_pipe_from_lookalikes() {
    // The dialect-level check catches the combined pipe before this helper runs.
    for (script, expected) in [
        ("echo ok &| cat", true),
        ("echo ok & | cat", false),
        ("echo ok |& cat", false),
        (r#"echo "ok &| cat""#, false),
    ] {
        assert_eq!(
            contains_fish_only_construct(&tokenize(script)),
            expected,
            "{script:?}"
        );
    }
}

#[test]
fn trailing_redirects_preserve_the_first_cut_and_require_a_target() {
    let expected = ["echo ok", "echo done"];
    for (cmd, unattestable) in [
        ("echo ok 2>&1 >/dev/null; echo done", false),
        ("echo ok 2>&1 >; echo done", true),
    ] {
        assert_eq!(
            contains_unattestable_construct(cmd),
            unattestable,
            "{cmd:?}"
        );
        assert_eq!(split_for_permissions(cmd), expected, "{cmd:?}");
        assert_eq!(
            legacy_segmenters::split_for_permissions_legacy(cmd),
            expected,
            "{cmd:?}"
        );
    }
}

#[test]
fn shell_words_resolve_double_quote_escapes_and_preserve_non_ascii_control_escape() {
    assert_eq!(
        shell_split_as_passed(r#"echo "a\"b\\c\q" $'left\cé\u0021'"#),
        ["echo", r#"a"b\c\q"#, r"left\cé!"]
    );
}
