use super::*;

fn strategy_lines(text: &str) -> Vec<SLine> {
    to_slines(&content::normalize(text), text)
}

fn json_preview(text: &str) -> (String, String) {
    JsonSkeleton
        .apply(&strategy_lines(text), &StrategyCtx { file_coords: false })
        .expect("valid JSON should produce a skeleton")
}

#[test]
fn struct_max_lines_validates_overrides() {
    const CHILD_EXPECTED: &str = "RTK_ENGINE_COVERAGE_CHILD_EXPECTED_STRUCT_CAP";

    if let Ok(expected) = std::env::var(CHILD_EXPECTED) {
        assert_eq!(
            struct_max_lines(),
            expected.parse::<usize>().expect("numeric expected cap")
        );
        return;
    }

    // Each case needs a fresh OnceLock without changing the parent environment.
    let qualified_name = concat!(module_path!(), "::struct_max_lines_validates_overrides");
    let (_, test_name) = qualified_name
        .split_once("::")
        .expect("module path starts with the crate name");
    let executable = std::env::current_exe().expect("test executable");

    for (value, expected) in [(" 8 ", "8"), ("7", "48"), ("not a number", "48")] {
        let output = std::process::Command::new(&executable)
            .args(["--exact", test_name, "--format", "pretty"])
            .env("RTK_PROXY_CCR_STRUCT_MAX_LINES", value)
            .env(CHILD_EXPECTED, expected)
            .output()
            .expect("run isolated cap test");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains("1 passed;"),
            "override {value:?} should yield {expected}; child output:\n{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn preview_strategies_decline_input_at_their_size_boundaries() {
    let ctx = StrategyCtx { file_coords: false };

    let short = "header\n    one\n    two\n    three\n    four";
    assert!(Structural.apply(&strategy_lines(short), &ctx).is_none());

    let long = format!("{short}\n    five");
    assert_eq!(
        Structural.apply(&strategy_lines(&long), &ctx),
        Some(("header\n[… 5 lines elided …]".to_owned(), "×5".to_owned()))
    );

    let head = "é".repeat(PREVIEW_HEAD_CHARS);
    let tail = "界".repeat(PREVIEW_TAIL_CHARS);
    let fits = format!("{head}{tail}");
    assert!(CharWindow.apply(&strategy_lines(&fits), &ctx).is_none());

    let over = format!("{head}x{tail}");
    let (body, note) = CharWindow
        .apply(&strategy_lines(&over), &ctx)
        .expect("one extra char is elided");
    // Prefix and suffix checks leave room for a singular `1 char`.
    assert!(body.starts_with(&format!("{head}[… 1 char")), "{body}");
    assert!(body.ends_with(&format!(" elided …]{tail}")), "{body}");
    assert_eq!(note, "chars×1");
}

#[test]
fn json_skeleton_summarizes_values_at_each_depth() {
    let input = serde_json::json!({
        "empty": [],
        "flags": {"missing": null, "off": false, "on": true},
        "items": [1, 2, 3],
        "nested": {"inner": {"hidden": 7, "other": 9}},
        "text": "é".repeat(25),
    });
    let (body, note) = json_preview(&input.to_string());
    assert_eq!(
        body,
        format!(
            "[json object, 5 keys] {{empty: [], flags: {{missing: null, off: false, on: true}}, \
             items: [3 items], nested: {{inner: {{2 keys}}}}, text: \"{}…\"(25 chars)}}",
            "é".repeat(24)
        )
    );
    assert_eq!(note, "json");

    let (body, note) = json_preview(" \n false \n ");
    assert_eq!(body, "[json value] false");
    assert_eq!(note, "json");
}

#[test]
fn json_skeleton_omits_remaining_keys_and_truncates_on_utf8_boundaries() {
    let key = "a".repeat(710);
    let input = format!(r#"{{"{key}":null,"b":true,"c":false}}"#);
    let (body, note) = json_preview(&input);
    assert_eq!(
        body,
        format!("[json object, 3 keys] {{{key}: null, … +2 more keys}}")
    );
    assert_eq!(note, "json");

    let key = format!("a{}", "界".repeat(400));
    let input = format!(r#"{{"{key}":null}}"#);
    let (body, note) = json_preview(&input);
    // The 293rd '界' starts at byte 900 and must be kept whole.
    // Prefix and suffix checks leave room for a singular `1 key`.
    assert!(body.starts_with("[json object, 1 key"), "{body}");
    assert!(
        body.ends_with(&format!("] {{a{}…", "界".repeat(293))),
        "{body}"
    );
    assert_eq!(note, "json");
}

#[test]
fn rle_annotation_reports_the_first_and_last_file_coordinates() {
    let seq = ["before", "same", "same", "same", "after"]
        .into_iter()
        .zip(41_u64..)
        .map(|(key, coord)| SLine {
            raw: format!("{coord:>6}→{key}"),
            key: key.to_owned(),
            indent: 0,
            blank: false,
            coord: Some(coord),
            annotation: false,
        })
        .collect();

    let (lines, collapsed) = rle_collapse(seq, &StrategyCtx { file_coords: true });
    assert_eq!(collapsed, 2);
    assert_eq!(
        lines
            .iter()
            .map(|line| line.raw.as_str())
            .collect::<Vec<_>>(),
        [
            "    41→before",
            "    42→same",
            "[… ×3 identical lines (lines 42–44) …]",
            "    45→after",
        ]
    );
    assert!(lines[2].annotation);
    assert_eq!(lines[2].coord, None);
}
