use super::looks_like_html;

#[test]
fn html_sniff_handles_a_utf8_character_split_by_the_byte_limit() {
    let tags = "<p>first</p><p>second</p>";

    // The four-byte crab starts at byte 1021 in both inputs, so byte 1024
    // is not a character boundary. Only tags before the cutoff should count.
    let before_cutoff = format!("{}{tags}🦀", "x".repeat(1021 - tags.len()));
    let after_cutoff = format!("{}🦀{tags}", "x".repeat(1021));

    assert!(looks_like_html(&before_cutoff));
    assert!(!looks_like_html(&after_cutoff));
}
