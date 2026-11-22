use super::*;
use std::fs;

#[test]
fn content_capture_ignores_markup_and_comments() {
    let mut reader = xml_reader("<Message>before<!-- ignored -->after</Message>");
    let mut text = String::new();

    loop {
        let event = reader.read_event().expect("valid XML");
        if matches!(event, Event::Eof) {
            break;
        }
        push_content(&event, &mut text);
    }

    assert_eq!(text, "beforeafter");
}

#[test]
fn paired_trx_elements_preserve_counts_timing_and_failure_details() {
    let trx = r#"<TestRun xmlns:alt="urn:trx-extra">
  <Times start="2026-01-01T10:00:00Z" finish="2026-01-01T10:00:00.250Z"></Times>
  <Times start="2026-01-01T10:00:00Z"/>
  <ResultSummary>
    <Counters total="8" passed="&#x110000;" alt:passed="5" failed="2"></Counters>
    <Extension/>
  </ResultSummary>
  <Results>
    <UnitTestResult outcome="Passed" testName="passing">
      <Output><ErrorInfo><Message>not a failure</Message></ErrorInfo></Output>
    </UnitTestResult>
    <UnitTestResult outcome="NotExecuted" testName="skipped"/>
    <UnitTestResult outcome="Failed" testName="with details">
      <Output><ErrorInfo>
        <Message>  assertion failed  </Message>
        <StackTrace>
at First()
at Second()
at Third()
at Fourth()
        </StackTrace>
      </ErrorInfo></Output>
    </UnitTestResult>
    <UnitTestResult outcome="Failed">
      <Output><ErrorInfo><Message>   </Message><StackTrace>   </StackTrace></ErrorInfo></Output>
    </UnitTestResult>
  </Results>
</TestRun>"#;

    let summary = parse_trx_content(trx).expect("TRX summary");
    assert_eq!(summary.duration_text.as_deref(), Some("250 ms"));
    assert_eq!(
        (
            summary.total,
            summary.passed,
            summary.failed,
            summary.skipped
        ),
        (8, 5, 2, 1)
    );
    assert_eq!(summary.failed_tests.len(), 2);
    assert_eq!(summary.failed_tests[0].name, "with details");
    assert_eq!(
        summary.failed_tests[0].details,
        ["assertion failed", "at First()\nat Second()\nat Third()"]
    );
    assert_eq!(summary.failed_tests[1].name, "unknown");
    assert!(summary.failed_tests[1].details.is_empty());
}

#[test]
fn mismatched_end_tag_rejects_partial_summary_and_later_times() {
    let trx = r#"<TestRun>
  <Counters total="2" passed="2"/>
  <Results></Wrong>
  <Times start="2026-01-01T10:00:00Z" finish="2026-01-01T10:00:02Z"/>
</TestRun>"#;

    assert!(parse_trx_content(trx).is_none());
    assert!(parse_trx_time_bounds(trx).is_none());
}

#[test]
fn directory_scan_ignores_wrong_extensions_and_invalid_utf8() {
    let scratch = crate::core::test_isolation::tempdir();
    let dir = scratch.path();
    let report = dir.join("report.TRX");
    fs::write(
        &report,
        r#"<TestRun><Counters total="3" passed="2" failed="1"/></TestRun>"#,
    )
    .expect("write valid report");
    fs::write(
        dir.join("report.xml"),
        r#"<TestRun><Counters total="100" passed="100"/></TestRun>"#,
    )
    .expect("write non-TRX file");
    fs::write(
        dir.join("invalid.trx"),
        b"<TestRun><Description>\xff</Description><Counters total=\"50\" passed=\"50\"/></TestRun>",
    )
    .expect("write invalid UTF-8");

    let since = Some(SystemTime::UNIX_EPOCH);
    assert!(parse_trx_files_in_dir_since(&dir.join("missing"), since).is_none());
    assert!(parse_trx_files_in_dir_since(&report, since).is_none());

    let summary = parse_trx_files_in_dir_since(dir, since).expect("valid report is retained");
    assert_eq!((summary.total, summary.passed, summary.failed), (3, 2, 1));
    assert_eq!(summary.project_count, 1);
}

#[cfg(unix)]
#[test]
fn directory_scan_handles_entries_without_search_permission() {
    use std::os::unix::fs::PermissionsExt;

    let scratch = crate::core::test_isolation::tempdir();
    let dir = scratch.path();
    let report = dir.join("report.trx");
    fs::write(
        &report,
        r#"<TestRun><Counters total="1" passed="1"/></TestRun>"#,
    )
    .expect("write report");

    let original = fs::metadata(dir).expect("directory metadata").permissions();
    fs::set_permissions(dir, fs::Permissions::from_mode(0o400)).expect("remove search permission");

    // A privileged runner can still stat and open files in the directory.
    let can_stat_report = fs::metadata(&report).is_ok();
    let since = Some(SystemTime::UNIX_EPOCH);
    let summary = parse_trx_files_in_dir_since(dir, since);

    fs::set_permissions(dir, original).expect("restore directory permissions");
    assert_eq!(
        summary.map(|summary| summary.total),
        can_stat_report.then_some(1)
    );
    assert_eq!(
        parse_trx_files_in_dir_since(dir, since)
            .expect("report is accessible after restoring permissions")
            .total,
        1
    );
}
