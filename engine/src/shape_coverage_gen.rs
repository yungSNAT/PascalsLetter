use super::*;

#[test]
fn normalization_strips_escape_sequences_and_preserves_file_coordinates() {
    let text = concat!(
        "\x1b]0;title\u{7}10|alpha\r\n",
        "11|be\x1b]0;other title\x1b\\ta\r\n",
        "13|gam\x1bMma\r\n",
        "note\r\n",
        "16|delta",
    );

    let normalized = normalize(text);

    assert_eq!(normalized.normaliser_tags(), "linenum|ansi|crlf");
    assert!(normalized.file_coords);
    assert_eq!(
        normalized
            .lines
            .iter()
            .map(|line| (line.content.as_str(), line.number))
            .collect::<Vec<_>>(),
        [
            ("alpha", Some(10)),
            ("beta", Some(11)),
            ("gamma", Some(13)),
            ("note", None),
            ("delta", Some(16)),
        ],
    );
}

#[test]
fn inconsistent_numbering_leaves_all_prefixes_intact() {
    for last in ["13|epsilon", "14:epsilon"] {
        let lines = ["10|alpha", "11|beta", "12|gamma", "13|delta", last];
        let normalized = normalize(&lines.join("\n"));

        assert!(!normalized.file_coords);
        assert_eq!(normalized.normaliser_tags(), "");
        assert_eq!(
            normalized
                .lines
                .iter()
                .map(|line| (line.content.as_str(), line.number))
                .collect::<Vec<_>>(),
            lines.map(|content| (content, None)),
        );
    }
}

#[test]
fn text_that_matches_no_shape_rule_is_labeled_unknown() {
    let text = "alpha\nbeta\nalpha";
    let features = extract(&normalize(text), text.chars().count());
    let (shape, reason) = classify(text, &features);

    assert_eq!((shape, reason), (Shape::Unknown, "no-rule"));
    assert_eq!(shape.as_str(), "unknown");
}
