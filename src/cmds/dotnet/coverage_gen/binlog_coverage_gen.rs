use super::*;
use flate2::{Compression, write::GzEncoder};
use std::fmt::Write as _;
use std::io::Write as _;

fn write_varints(bytes: &mut Vec<u8>, values: &[i32]) {
    for &value in values {
        let mut remaining = value as u32;
        while remaining >= 0x80 {
            bytes.push(((remaining as u8) & 0x7f) | 0x80);
            remaining >>= 7;
        }
        bytes.push(remaining as u8);
    }
}

fn append_string(records: &mut Vec<u8>, text: &str) {
    write_varints(records, &[RECORD_STRING, text.len() as i32]);
    records.extend_from_slice(text.as_bytes());
}

fn append_record(records: &mut Vec<u8>, kind: i32, payload: &[u8]) {
    write_varints(records, &[kind, payload.len() as i32]);
    records.extend_from_slice(payload);
}

fn write_binlog(path: &Path, version: i32, records: &[u8]) {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(&version.to_le_bytes())
        .expect("write version");
    encoder
        .write_all(&version.min(18).to_le_bytes())
        .expect("write minimum reader version");
    encoder.write_all(records).expect("write records");
    std::fs::write(path, encoder.finish().expect("finish gzip")).expect("write binlog");
}

#[test]
fn structured_binlog_preserves_project_and_diagnostic_events() {
    let dir = tempfile::tempdir().expect("create binlog directory");
    let path = dir.path().join("events.binlog");
    let mut records = Vec::new();

    append_record(&mut records, RECORD_NAME_VALUE_LIST, &[0, 255]);
    append_record(&mut records, RECORD_PROJECT_IMPORT_ARCHIVE, &[0, 128, 128]);
    append_record(&mut records, 99, &[255]);

    for text in [
        "App.csproj",                    // 10
        "Library.csproj",                // 11
        "Nullable reference assignment", // 12
        "CS8601",                        // 13
        "Program.cs",                    // 14
        "Compiler requested attention",  // 15
        concat!(
            "Restored Console.csproj (in 1 ms).\n",
            "Console.csproj : warning NU1900: Package audit unavailable\n",
            "error NU1101: Dependency not found\n",
            "Test run for App.Tests.dll\n",
            "Passed! - Failed: 0, Passed: 2, Skipped: 1, Total: 3, Duration: 7 ms"
        ),
    ] {
        append_string(&mut records, text);
    }

    let context = [135, 22, 33, 44, 55, 66, 77];
    let mut project_started = Vec::new();
    write_varints(
        &mut project_started,
        &[FLAG_BUILD_EVENT_CONTEXT | FLAG_EXTENDED | FLAG_ARGUMENTS],
    );
    write_varints(&mut project_started, &context);
    write_varints(&mut project_started, &[1, 130, 2]); // Extended type, dictionary, data.
    write_varints(&mut project_started, &[2, 10, 11]); // Two string arguments.
    write_varints(&mut project_started, &[1]); // Has a parent context.
    write_varints(&mut project_started, &context);
    write_varints(&mut project_started, &[10]); // Project file.
    append_record(&mut records, RECORD_PROJECT_STARTED, &project_started);
    append_record(&mut records, RECORD_PROJECT_STARTED, &[0, 0, 1]); // Empty filename.
    append_record(&mut records, RECORD_PROJECT_FINISHED, &[0, 11, 1]);

    append_record(
        &mut records,
        RECORD_WARNING,
        &[FLAG_MESSAGE as u8, 12, 0, 13, 14, 10, 42, 3, 42, 4],
    );
    append_record(&mut records, RECORD_MESSAGE, &[FLAG_MESSAGE as u8, 12]);
    // An empty message is preserved; a reserved string index produces no message.
    for index in [15, 1, 2] {
        append_record(
            &mut records,
            RECORD_CRITICAL_BUILD_MESSAGE,
            &[FLAG_MESSAGE as u8, index],
        );
    }
    write_varints(&mut records, &[RECORD_END_OF_FILE]);
    write_binlog(&path, 25, &records);

    let parsed = parse_events_from_binlog(&path).expect("read structured events");
    assert_eq!(
        parsed.project_files,
        HashSet::from(["App.csproj".to_owned(), "Library.csproj".to_owned()])
    );
    assert_eq!(
        parsed.messages,
        [
            "Nullable reference assignment",
            "Compiler requested attention",
            ""
        ]
    );
    assert!(parsed.errors.is_empty());
    assert_eq!(
        parsed.warnings,
        [BinlogIssue {
            code: "CS8601".to_owned(),
            file: "Program.cs".to_owned(),
            line: 42,
            column: 3,
            message: "Nullable reference assignment".to_owned(),
        }]
    );

    let tests = parse_test(&path).expect("summarize tests");
    assert_eq!(tests.project_count, 2);
    assert_eq!(
        (tests.passed, tests.failed, tests.skipped, tests.total),
        (2, 0, 1, 3)
    );
    assert_eq!(tests.duration_text.as_deref(), Some("7 ms"));

    let restore = parse_restore(&path).expect("summarize restore");
    assert_eq!(restore.restored_projects, 2);
    assert_eq!((restore.errors, restore.warnings), (1, 1));
}

#[test]
fn build_without_project_events_counts_distinct_projects_from_strings() {
    let dir = tempfile::tempdir().expect("create binlog directory");
    let path = dir.path().join("build.binlog");
    let mut records = Vec::new();
    append_string(
        &mut records,
        concat!(
            "Restored App.csproj (in 1 ms).\n",
            "Restored Library.csproj (in 2 ms).\n",
            "App.csproj : warning NU1900: Package audit unavailable\n",
            "Time Elapsed 00:00:03.00"
        ),
    );
    append_record(&mut records, RECORD_BUILD_FINISHED, &[0, 1]);
    write_varints(&mut records, &[RECORD_END_OF_FILE]);
    write_binlog(&path, 25, &records);

    let build = parse_build(&path).expect("summarize build");
    assert!(build.succeeded);
    assert_eq!(build.project_count, 2);
    assert_eq!(build.duration_text, None);
    assert!(build.errors.is_empty());
    assert_eq!(
        build.warnings,
        [BinlogIssue {
            code: "NU1900".to_owned(),
            file: "App.csproj".to_owned(),
            line: 0,
            column: 0,
            message: "Package audit unavailable".to_owned(),
        }]
    );
}

#[test]
fn malformed_binlogs_report_which_record_could_not_be_read() {
    let dir = tempfile::tempdir().expect("create binlog directory");
    let path = dir.path().join("malformed.binlog");
    std::fs::write(&path, b"").expect("write empty file");
    let error = parse_build(&path).expect_err("reject empty file");
    assert!(format!("{error:#}").contains("empty file"));

    let cases: &[(i32, &[u8], &str)] = &[
        (17, &[], "unsupported binlog format 17"),
        (
            25,
            &[0x80; 5],
            "failed to read record kind: invalid 7-bit encoded integer",
        ),
        (
            25,
            &[RECORD_NAME_VALUE_LIST as u8],
            "failed to read record length: unexpected end of stream",
        ),
        (
            25,
            &[RECORD_PROJECT_IMPORT_ARCHIVE as u8, 2, 0],
            "failed to skip auxiliary record payload: unexpected end of stream",
        ),
    ];
    for &(version, records, expected) in cases {
        write_binlog(&path, version, records);
        let error = parse_build(&path).expect_err("reject malformed binlog");
        assert!(format!("{error:#}").contains(expected), "{error:#}");
    }

    for (kind, expected) in [
        (RECORD_NAME_VALUE_LIST, "negative record length: -1"),
        (RECORD_MESSAGE, "negative event length: -1"),
        (RECORD_STRING, "negative string length: -1"),
    ] {
        let mut records = Vec::new();
        write_varints(&mut records, &[kind, -1]);
        write_binlog(&path, 25, &records);
        let error = parse_build(&path).expect_err("reject negative length");
        assert!(format!("{error:#}").contains(expected), "{error:#}");
    }
}

#[test]
fn event_fields_reject_missing_string_records_and_legacy_dictionaries() {
    let mut parsed = ParsedBinlog {
        string_records: vec!["First string".to_owned()],
        ..ParsedBinlog::default()
    };
    let error = parse_event_record(
        RECORD_MESSAGE,
        &mut BinReader::new(&[FLAG_MESSAGE as u8, 11]),
        25,
        &mut parsed,
    )
    .expect_err("reject an index past the string table");
    assert_eq!(error.to_string(), "invalid string record index 11");
    assert!(parsed.messages.is_empty());

    let mut payload = Vec::new();
    write_varints(&mut payload, &[FLAG_EXTENDED, 0, 0, 0]);
    let error = parse_event_record(
        RECORD_CRITICAL_BUILD_MESSAGE,
        &mut BinReader::new(&payload),
        9,
        &mut parsed,
    )
    .expect_err("reject legacy dictionary encoding");
    assert_eq!(error.to_string(), "legacy dictionary format is unsupported");
    assert!(parsed.messages.is_empty());
}

#[test]
fn issue_selection_rewards_diagnostic_codes_and_specific_messages() {
    let placeholder = BinlogIssue {
        code: String::new(),
        file: "Program.cs".to_owned(),
        line: 42,
        column: 3,
        message: "Build issue".to_owned(),
    };
    let with_message = BinlogIssue {
        message: "Nullable reference assignment".to_owned(),
        ..placeholder.clone()
    };
    let with_code = BinlogIssue {
        code: "CS8601".to_owned(),
        ..placeholder.clone()
    };

    assert_eq!(
        select_best_issues(vec![placeholder], vec![with_message.clone()]).as_slice(),
        std::slice::from_ref(&with_message)
    );
    assert_eq!(
        select_best_issues(vec![with_message], vec![with_code.clone()]).as_slice(),
        std::slice::from_ref(&with_code)
    );
}

#[test]
fn build_text_supplies_placeholders_for_incomplete_diagnostics() {
    let build = parse_build_from_text(concat!(
        "Build FAILED.\n",
        "First.cs(12,3): ERROR CS0131: Invalid assignment\n",
        "Second.cs(4,5): WARNING CS0219: Unused variable\n"
    ));
    assert!(!build.succeeded);
    for (issues, message) in [
        (build.errors, "Build error #1 (details omitted)"),
        (build.warnings, "Build warning #1 (details omitted)"),
    ] {
        assert_eq!(
            issues,
            [BinlogIssue {
                code: String::new(),
                file: String::new(),
                line: 0,
                column: 0,
                message: message.to_owned(),
            }]
        );
    }

    let build = parse_build_from_text("Second.cs(4,5): warning CS0219:   ");
    assert_eq!(build.warnings.len(), 1);
    assert_eq!(build.warnings[0].code, "CS0219");
    assert_eq!(build.warnings[0].message, "diagnostic without message");
}

#[test]
fn failed_build_recovers_only_distinct_binary_like_diagnostics_with_context() {
    let padding = "\n     \n  x  \n";
    assert!(extract_printable_runs(padding).is_empty());
    assert!(extract_binary_like_issues(padding).is_empty());

    let source = "/src/Broken.cs";
    // Keep the first code outside the four-run window of any source or message.
    let input = format!(
        "CS9999\nnoise\nnoise\nnoise\nnoise\n\
         Invalid assignment target\nCS0131\n{source}\n\
         Invalid assignment target\nCS0131\n{source}\n\
         Build FAILED."
    );
    let build = parse_build_from_text(&input);
    assert!(!build.succeeded);
    assert_eq!(
        build.errors,
        [BinlogIssue {
            code: "CS0131".to_owned(),
            file: source.to_owned(),
            line: 0,
            column: 0,
            message: "Invalid assignment target".to_owned(),
        }]
    );
}

#[test]
fn test_text_caps_failure_details_and_prefers_the_final_summary() {
    let mut text = String::from(concat!(
        "Test run for App.Tests.dll (net8.0)\n",
        "Test run for App.Tests.dll (net8.0)\n",
        "Test run for Library.Tests.dll (net8.0)\n",
        "Failed!  - Failed: 1, Passed: 3, Skipped: 0, Total: 4, Duration: 7 ms\n",
        "  Failed AppTests.InvalidInput [2 ms]\n",
        "\n",
        "    Error Message:   \n",
        "\n"
    ));
    for index in 1..=22 {
        writeln!(text, "    Frame {index}").expect("append stack frame");
    }

    let preliminary = parse_test_from_text(&text);
    assert_eq!(preliminary.project_count, 2);
    assert_eq!(preliminary.duration_text.as_deref(), Some("7 ms"));
    let details = ["Error Message:".to_owned(), String::new()]
        .into_iter()
        .chain((1..=18).map(|index| format!("Frame {index}")))
        .collect();
    assert_eq!(
        preliminary.failed_tests,
        [FailedTest {
            name: "AppTests.InvalidInput".to_owned(),
            details,
        }]
    );

    text.push_str("Test summary: total: 9, failed: 2, succeeded: 6, skipped: 1, duration: 42 ms\n");
    let summary = parse_test_from_text(&text);
    assert_eq!(
        (
            summary.passed,
            summary.failed,
            summary.skipped,
            summary.total
        ),
        (6, 2, 1, 9)
    );
    assert_eq!(summary.duration_text.as_deref(), Some("42 ms"));
    assert_eq!(summary.failed_tests, preliminary.failed_tests);
}
