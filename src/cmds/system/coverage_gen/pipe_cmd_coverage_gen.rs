use super::*;

#[test]
fn named_tool_filters_route_captured_output_to_their_formatters() {
    let pest = concat!(
        "   PASS  Tests\\Unit\\ExampleTest\n",
        "  ✓ it adds two numbers\n\n",
        "  Tests:    1 passed (1 assertion)\n",
        "  Duration: 0.02s\n",
    );
    let expected = crate::cmds::php::test_output::filter_test_runner_output(pest);
    for name in ["pest", "paratest", "php-test"] {
        let filter = resolve_filter(name).expect("PHP test runner filter");
        assert_eq!(filter(pest), expected, "filter: {name}");
    }

    let ecs = " [OK] No errors found.\n";
    let filter = resolve_filter("ecs").expect("ECS filter");
    assert_eq!(
        filter(ecs),
        crate::cmds::php::ecs_cmd::filter_ecs_output(ecs)
    );

    let status = "## main...origin/main [ahead 2]\n M widget.rs\n?? notes.txt\n";
    let filter = resolve_filter("git-status").expect("Git status filter");
    assert_eq!(
        filter(status),
        crate::cmds::git::git_cmd::format_status_output(status)
    );
}

#[test]
fn phpstan_and_pint_choose_a_formatter_based_on_the_input() {
    let phpstan = resolve_filter("phpstan").expect("PHPStan filter");
    let phpstan_json = concat!(
        " \n\t",
        r#"{"totals":{"errors":0,"file_errors":1},"files":{"Example.php":{"errors":1,"messages":[{"message":"Undefined variable: $total","line":7,"ignorable":true,"identifier":"variable.undefined"}]}},"errors":[]}"#,
    );
    let phpstan_text = concat!(
        " ------ ---------------------------------\n",
        "  Line   Example.php\n",
        " ------ ---------------------------------\n",
        "  7      Undefined variable: $total\n",
        " ------ ---------------------------------\n\n",
        " [ERROR] Found 1 error\n",
    );
    let shown = phpstan(phpstan_json);
    assert_eq!(
        shown,
        crate::cmds::php::phpstan_cmd::filter_phpstan_json(phpstan_json)
    );
    assert_ne!(shown, phpstan_json);
    assert_eq!(
        phpstan(phpstan_text),
        crate::cmds::php::phpstan_cmd::filter_phpstan_text(phpstan_text)
    );

    let pint = resolve_filter("pint").expect("Pint filter");
    let pint_json = concat!(
        "\n  ",
        r#"{"result":"fixed","files":[{"path":"Example.php","fixers":["single_line_empty_body"]}]}"#,
    );
    let shown = pint(pint_json);
    assert_eq!(
        shown,
        crate::cmds::php::pint_cmd::filter_pint_json(pint_json)
    );
    assert_ne!(shown, pint_json);

    let mut pint_text = String::from("Starting formatter pass\n");
    pint_text.push_str(&"Checking file\n".repeat(70));
    pint_text.push_str("Finished formatting Example.php\n");
    let shown = pint(&pint_text);
    assert_eq!(
        shown,
        crate::core::utils::fallback_tail(&pint_text, "pint", 60)
    );
    assert!(shown.contains("Finished formatting Example.php"), "{shown}");
    assert!(!shown.contains("Starting formatter pass"), "{shown}");
}

#[test]
fn vitest_compacts_reports_and_preserves_unrecognized_output() {
    use crate::cmds::js::vitest_cmd::VitestParser;
    use crate::parser::{FormatMode, OutputParser, ParseResult, TokenFormatter};

    let json = concat!(
        r#"{"numTotalTestSuites":1,"numPassedTestSuites":1,"numFailedTestSuites":0,"numPendingTestSuites":0,"#,
        r#""numTotalTests":1,"numPassedTests":1,"numFailedTests":0,"numPendingTests":0,"numTodoTests":0,"#,
        r#""startTime":1710000000000,"success":true,"testResults":[{"name":"math.test.ts","status":"passed","#,
        r#""startTime":1710000000000,"endTime":1710000000001,"message":"","assertionResults":[{"ancestorTitles":["math"],"#,
        r#""fullName":"math adds numbers","title":"adds numbers","status":"passed","duration":1,"failureMessages":[],"meta":{}}]}]}"#,
    );
    let text = concat!(
        " RUN  v3.2.0\n\n",
        " ❯ math.test.ts (2 tests | 1 failed) 8ms\n",
        "   ✓ math > adds numbers 1ms\n",
        "   × math > subtracts numbers 7ms\n",
        "     → expected 3 to be 2\n\n",
        " FAIL  math.test.ts > math > subtracts numbers\n",
        "AssertionError: expected 3 to be 2 // Object.is equality\n\n",
        " Test Files  1 failed (1)\n",
        "      Tests  1 failed | 1 passed (2)\n",
        "   Duration  312ms\n",
    );
    let mixed = format!(" RUN  v3.2.0\n\n{json}\n");
    let vitest = resolve_filter("vitest").expect("Vitest filter");

    for input in [json, text, mixed.as_str()] {
        let expected = match VitestParser::parse(input) {
            ParseResult::Full(data) | ParseResult::Degraded(data, _) => {
                data.format(FormatMode::Compact)
            }
            ParseResult::Passthrough(_) => input.to_owned(),
        };
        assert_eq!(vitest(input), expected, "report: {input}");
    }

    let shown = vitest(json);
    assert_ne!(shown, json, "the JSON report should be compacted");
    assert_eq!(auto_detect_filter(json)(json), shown);

    let raw = "ordinary build chatter\n\n";
    assert_eq!(vitest(raw), raw);
}

#[test]
fn grep_and_find_handle_empty_results_and_unqualified_file_names() {
    let grep = resolve_filter("rg").expect("rg filter");
    let malformed = "widget.rs:unknown:matching text\nplain diagnostic\n";
    assert_eq!(grep(malformed), malformed);
    assert_eq!(
        grep("widget.rs:7:  label: value \nplain diagnostic\n"),
        "1 matches in 1F:\n\n[file] widget.rs (1):\n     7: label: value\n\n"
    );

    let find = resolve_filter("fd").expect("fd filter");
    let blank = " \n\t\n";
    assert_eq!(find(blank), blank);

    let shown = find("README.md\n\nCargo.toml\n");
    let mut lines = shown.lines();
    assert_eq!(lines.next(), Some("2 files in 1 dirs:"));
    assert_eq!(lines.next(), Some(""));
    let (dir, count) = lines
        .next()
        .expect("directory heading")
        .split_once("  ")
        .expect("directory and file count");
    assert_eq!(std::path::Path::new(dir), std::path::Path::new("."));
    assert_eq!(count, "(2)");
    assert!(lines.eq(["  README.md", "  Cargo.toml"]));
}

#[cfg(all(unix, feature = "engine"))]
#[test]
fn pipe_passthrough_relays_non_utf8_stdin_verbatim() {
    use crate::core::test_isolation::{rtk_binary_ready, rtk_command};
    use std::io::Write;
    use std::process::Stdio;

    if !rtk_binary_ready() {
        return;
    }

    let input = b"widget.rs:7: matching text\nraw bytes: \xff\0\n";
    let mut child = rtk_command()
        .args(["pipe", "--passthrough"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start isolated rtk pipe");
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(input)
        .expect("send input and close stdin");

    let output = child.wait_with_output().expect("wait for rtk pipe");
    assert!(
        output.status.success(),
        "rtk pipe failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout.as_slice(), input);
}
