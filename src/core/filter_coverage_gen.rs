use super::*;

#[test]
fn filter_levels_round_trip_and_reject_unknown_names() {
    for (level, name) in [
        (FilterLevel::None, "none"),
        (FilterLevel::Minimal, "minimal"),
        (FilterLevel::Aggressive, "aggressive"),
    ] {
        let rendered = level.to_string();
        assert_eq!(rendered, name);
        assert_eq!(
            rendered.to_ascii_uppercase().parse::<FilterLevel>(),
            Ok(level)
        );
    }

    assert_eq!(
        "Verbose".parse::<FilterLevel>().unwrap_err(),
        "Unknown filter level: Verbose"
    );
}

#[test]
fn file_extensions_select_language_specific_comment_markers() {
    let c_style = (Some("//"), Some("/*"), Some("*/"), None, Some("/**"));
    let cases = [
        (
            ["PY", "pyw"].as_slice(),
            Language::Python,
            (
                Some("#"),
                Some("\"\"\""),
                Some("\"\"\""),
                None,
                Some("\"\"\""),
            ),
        ),
        (["ts", "TSX"].as_slice(), Language::TypeScript, c_style),
        (["GO"].as_slice(), Language::Go, c_style),
        (["c", "H"].as_slice(), Language::C, c_style),
        (
            ["cpp", "cc", "cxx", "hpp", "HH"].as_slice(),
            Language::Cpp,
            c_style,
        ),
        (["JAVA"].as_slice(), Language::Java, c_style),
        (
            ["RB"].as_slice(),
            Language::Ruby,
            (Some("#"), Some("=begin"), Some("=end"), None, None),
        ),
        (
            ["sh", "bash", "ZSH"].as_slice(),
            Language::Shell,
            (Some("#"), None, None, None, None),
        ),
    ];

    for (extensions, expected_language, expected_markers) in cases {
        for &extension in extensions {
            let language = Language::from_extension(extension);
            assert_eq!(language, expected_language, "extension {extension}");

            let CommentPatterns {
                line,
                block_start,
                block_end,
                doc_line,
                doc_block_start,
            } = language.comment_patterns();
            assert_eq!(
                (line, block_start, block_end, doc_line, doc_block_start),
                expected_markers,
                "comment markers for {extension}"
            );
        }
    }
}

#[test]
fn minimal_python_preserves_triple_quoted_text_after_escaped_delimiters() {
    let input = concat!(
        r#"escaped = "quote: \" # still inside the string" + '''
# literal after the escaped quote
'''
# remove this comment
"#,
        " \t\n\n",
        r#"formatted = f"{{literal" + '''
# literal after the doubled brace
'''
# remove this comment too
print(escaped, formatted)
"#,
    );
    let expected = r#"escaped = "quote: \" # still inside the string" + '''
# literal after the escaped quote
'''

formatted = f"{{literal" + '''
# literal after the doubled brace
'''
print(escaped, formatted)"#;

    assert_eq!(MinimalFilter.filter(input, &Language::Python), expected);
}

#[test]
fn minimal_rust_keeps_indented_doc_comments_but_drops_plain_comments() {
    let input = concat!(
        "mod example {\n",
        "    // Internal note.\n",
        "    /// Returns the limit.\n",
        "    pub fn limit() -> usize { 3 }\n",
        "}",
    );

    assert_eq!(
        MinimalFilter.filter(input, &Language::Rust),
        "mod example {\n    /// Returns the limit.\n    pub fn limit() -> usize { 3 }\n}"
    );
}

#[test]
fn aggressive_rust_keeps_declarations_and_replaces_implementation_lines() {
    let input = concat!(
        "use std::fmt;\n",
        "fn empty() {\n",
        "}\n",
        "const LIMIT: usize = 3;\n",
        "fn next_limit() -> usize {\n",
        "    let next = LIMIT + 1;\n",
        "    next\n",
        "}\n",
        "static READY: bool = true;\n",
    );

    let output = AggressiveFilter.filter(input, &Language::Rust);
    assert!(output.starts_with("use std::fmt;\nfn empty() {\n}\nconst LIMIT: usize = 3;\n"));
    assert!(output.contains("fn next_limit() -> usize {\n    // ... implementation\n"));
    assert!(!output.contains("let next ="));
    assert!(!output.lines().any(|line| line.trim() == "next"));
    assert!(output.ends_with("static READY: bool = true;"));
}
