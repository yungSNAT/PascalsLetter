use super::*;

#[test]
fn json_uses_reported_correctable_count_and_sorts_unknown_severity_last() {
    let input = r#"{
        "files": [{
            "path": "example.rb",
            "offenses": [
                {"severity": "custom", "message": "Custom finding", "cop_name": "Example/Custom",
                 "correctable": false, "location": {"start_line": 1}},
                {"severity": "convention", "message": "Convention finding", "cop_name": "Example/Convention",
                 "correctable": true, "location": {"start_line": 9}}
            ]
        }],
        "summary": {
            "offense_count": 2,
            "target_file_count": 1,
            "inspected_file_count": 1,
            "correctable_offense_count": 2
        }
    }"#;
    assert_eq!(
        filter_rubocop_json(input),
        concat!(
            "rubocop: 2 offenses (1 files)\n\n",
            "example.rb\n",
            "  :9 Example/Convention — Convention finding\n",
            "  :1 Example/Custom — Custom finding\n\n",
            "(2 correctable, run `rubocop -A`)"
        )
    );
}

#[test]
fn json_file_overflow_keeps_ten_files_inline_and_reports_two_hidden() {
    use crate::core::{test_isolation, user_env};

    let scratch = test_isolation::tempdir();
    let _working_dir = test_isolation::enter(scratch.path());
    let names: Vec<_> = (1..=12).map(|i| format!("model_{i:02}.rb")).collect();
    let files: Vec<_> = names
        .iter()
        .map(|name| {
            serde_json::json!({
                "path": name,
                "offenses": [{
                    "severity": "warning",
                    "message": "Example finding",
                    "cop_name": "Example/Warning",
                    "correctable": false,
                    "location": {"start_line": 1}
                }]
            })
        })
        .collect();
    let input = serde_json::json!({
        "files": files,
        "summary": {"offense_count": 12, "target_file_count": 12, "inspected_file_count": 12}
    });
    let tee_dir = scratch.path().join("tee");

    test_isolation::with_root(scratch.path(), || {
        user_env::with_path("RTK_TEE_DIR", Some(&tee_dir), || {
            user_env::with_vars(&[("RTK_TEE", Some("1"))], || {
                let result = filter_rubocop_json(&input.to_string());
                let mut lines = result.lines();
                assert_eq!(lines.next(), Some("rubocop: 12 offenses (12 files)"));
                for name in &names[..10] {
                    assert_eq!(lines.next(), Some(""));
                    assert_eq!(lines.next(), Some(name.as_str()));
                    assert_eq!(lines.next(), Some("  :1 Example/Warning — Example finding"));
                }
                assert_eq!(lines.next(), Some(""));
                assert_eq!(lines.next(), Some("… +2 more files"));

                let hint = lines
                    .next()
                    .expect("expected a recall hint for the hidden files");
                let recall_id = hint
                    .strip_prefix("  [+2 hidden: rtk recall ")
                    .and_then(|suffix| suffix.strip_suffix(']'))
                    .expect("hint should report two hidden files and give a recall command");
                assert!(
                    !recall_id.is_empty() && recall_id.bytes().all(|byte| byte.is_ascii_hexdigit()),
                    "invalid recall ID in hint: {hint}"
                );
                assert_eq!(lines.next(), None, "{result}");
            });
        });
    });
}

#[test]
fn text_prioritizes_a_load_error_over_a_summary_and_caps_the_diagnostic() {
    let head = (1..=20)
        .map(|i| format!("stack frame {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let input =
        format!("{head}\nBundler::GemNotFound: rubocop\n1 file inspected, no offenses detected");
    assert_eq!(
        filter_rubocop_text(&input),
        format!("RuboCop error:\n{head}\n... (2 more lines)")
    );
}

#[test]
fn text_handles_partial_summaries_and_keeps_only_the_tail_without_one() {
    let zero_corrected = "3 files inspected, 1 offense detected, 0 offenses autocorrected";
    assert_eq!(
        filter_rubocop_text(zero_corrected),
        format!("RuboCop: {zero_corrected}")
    );
    assert_eq!(
        filter_rubocop_text("files inspected, no offenses detected"),
        "ok ✓ rubocop (no offenses)"
    );
    assert_eq!(
        extract_autocorrect_count("3 files inspected, 9 offenses detected"),
        0
    );

    let tail = "third detail\nfourth detail\nfifth detail\nsixth detail\nfinal detail";
    let result = filter_rubocop_text(&format!("startup banner\nprogress banner\n{tail}"));
    assert!(result.contains(tail), "{result}");
    assert!(!result.contains("banner"), "{result}");
}

#[test]
fn compact_paths_keep_the_last_app_directory_or_just_the_basename() {
    use std::path::Path;

    let nested = compact_ruby_path(r"C:\checkout\app\nested\app\extras\widget.rb");
    assert_eq!(
        Path::new(&nested),
        Path::new("app").join("extras").join("widget.rb")
    );
    assert_eq!(compact_ruby_path("/checkout/scripts/tool.rb"), "tool.rb");
    assert_eq!(compact_ruby_path("Rakefile"), "Rakefile");
}
