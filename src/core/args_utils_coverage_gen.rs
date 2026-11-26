use super::restore_double_dash_with_raw;

#[test]
fn keeps_parsed_args_when_restoration_leaves_no_command_prefix() {
    let parsed = vec![String::from("file")];
    let raw = ["--", "file"].map(String::from);

    // The inferred suffix would consume all raw tokens, leaving no command prefix.
    assert_eq!(restore_double_dash_with_raw(&parsed, &raw), parsed);

    // With the raw list truncated, the inferred suffix would not fit at all.
    assert_eq!(restore_double_dash_with_raw(&parsed, &raw[..1]), parsed);
}
