use super::filter_sqlfluff_lint_json;

#[test]
fn summaries_keep_the_first_reported_line_after_unlocated_violations() {
    let input = r#"[
        {"filepath":"unlocated.sql","violations":[
            {"code":"LT01","description":"Unexpected spacing","fixes":[]}
        ]},
        {"filepath":"located.sql","violations":[
            {"code":"LT01","description":"Unexpected spacing","fixes":[]},
            {"code":"LT01","description":"Unexpected spacing","line_no":7,"fixes":[]},
            {"code":"LT01","description":"Unexpected spacing","start_line_no":12,"fixes":[]}
        ]}
    ]"#;
    let output = filter_sqlfluff_lint_json(input);
    let (summary, detail) = output
        .split_once("\nViolations:\n")
        .expect("report should include violation details");

    assert!(
        summary.contains("  LT01 (4x, first at located.sql:7): Unexpected spacing\n"),
        "{output}"
    );
    assert!(
        summary.contains("  located.sql (3 violations)\n    LT01 (3, first at line 7)\n"),
        "{output}"
    );
    assert_eq!(
        detail,
        "  located.sql LT01\n  located.sql:7 LT01\n  located.sql:12 LT01\n  unlocated.sql LT01"
    );
}
