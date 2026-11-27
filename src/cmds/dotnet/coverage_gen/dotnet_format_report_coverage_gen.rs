use super::parse_format_report;

#[test]
fn distinguishes_missing_report_from_invalid_json() {
    let dir = crate::core::test_isolation::tempdir();
    let path = dir.path().join("format-report.json");

    let missing = parse_format_report(&path).expect_err("report does not exist yet");
    assert_eq!(
        missing.to_string(),
        format!("Failed to read dotnet format report at {}", path.display())
    );
    assert_eq!(
        missing
            .downcast_ref::<std::io::Error>()
            .expect("preserve the file-open error")
            .kind(),
        std::io::ErrorKind::NotFound
    );

    std::fs::write(&path, b"[@]").expect("write malformed report");
    let malformed = parse_format_report(&path).expect_err("report contains invalid JSON");
    assert_eq!(
        malformed.to_string(),
        format!(
            "Failed to parse dotnet format report JSON at {}",
            path.display()
        )
    );
    assert!(
        malformed
            .downcast_ref::<serde_json::Error>()
            .expect("preserve the JSON parser error")
            .is_syntax()
    );
}
