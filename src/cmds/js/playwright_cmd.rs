//! Filters Playwright E2E test output to show only failures.

use crate::core::shell::{display_args, with_args};
use crate::core::stream::exec_capture;
use crate::core::tracking;
use crate::core::utils::{detect_package_manager, resolved_command, strip_ansi};
use anyhow::{Context, Result};
use regex::Regex;
use serde::Deserialize;
use std::sync::LazyLock;

use crate::parser::{
    FormatMode, OutputParser, ParseResult, TestFailure, TestResult, TokenFormatter,
    emit_degradation_warning, emit_passthrough_warning, truncate_passthrough,
};

/// Matches real Playwright JSON reporter output (suites → specs → tests → results)
#[derive(Debug, Deserialize)]
struct PlaywrightJsonOutput {
    stats: PlaywrightStats,
    #[serde(default)]
    suites: Vec<PlaywrightSuite>,
    /// Errors raised outside any test: global setup or teardown, a web
    /// server that would not start, no tests found.
    #[serde(default)]
    errors: Vec<PlaywrightError>,
}

/// Counts of test executions: one per test per project (browser).
#[derive(Debug, Deserialize)]
struct PlaywrightStats {
    expected: usize,
    unexpected: usize,
    skipped: usize,
    /// Failed at first, then passed on a retry.
    #[serde(default)]
    flaky: usize,
    /// Duration in milliseconds (float in real Playwright output)
    #[serde(default)]
    duration: f64,
}

/// File-level or describe-level suite
#[derive(Debug, Deserialize)]
struct PlaywrightSuite {
    title: String,
    #[serde(default)]
    file: Option<String>,
    /// Individual test specs (test functions)
    #[serde(default)]
    specs: Vec<PlaywrightSpec>,
    /// Nested describe blocks
    #[serde(default)]
    suites: Vec<PlaywrightSuite>,
}

/// A single test function (may run in multiple browsers/projects)
#[derive(Debug, Deserialize)]
struct PlaywrightSpec {
    title: String,
    /// Per-project/browser executions
    #[serde(default)]
    tests: Vec<PlaywrightExecution>,
}

/// A test execution in a specific browser/project
#[derive(Debug, Deserialize)]
struct PlaywrightExecution {
    /// "expected", "unexpected", "skipped", "flaky"
    status: String,
    #[serde(default, rename = "projectName")]
    project_name: String,
    #[serde(default)]
    results: Vec<PlaywrightAttempt>,
}

/// A single attempt/result for a test execution
#[derive(Debug, Deserialize)]
struct PlaywrightAttempt {
    /// "passed", "failed", "timedOut", "interrupted"
    status: String,
    /// Error details (array in Playwright >= v1.30)
    #[serde(default)]
    errors: Vec<PlaywrightError>,
}

#[derive(Debug, Deserialize)]
struct PlaywrightError {
    #[serde(default)]
    message: String,
    /// Set instead of `message` when something other than an Error was thrown.
    #[serde(default)]
    value: String,
}

impl PlaywrightError {
    fn text(&self) -> String {
        if self.message.is_empty() {
            self.value.clone()
        } else {
            self.message.clone()
        }
    }
}

/// Parser for Playwright JSON output
pub struct PlaywrightParser;

impl OutputParser for PlaywrightParser {
    type Output = TestResult;

    fn parse(input: &str) -> ParseResult<TestResult> {
        // Tier 1: Try JSON parsing
        match serde_json::from_str::<PlaywrightJsonOutput>(input) {
            Ok(json) => {
                // Errors outside tests first: the compact view lists only the
                // first few failures, and these explain a whole failed run.
                let mut failures: Vec<TestFailure> = json
                    .errors
                    .iter()
                    .map(|e| TestFailure {
                        test_name: "error outside any test".to_string(),
                        file_path: String::new(),
                        error_message: e.text(),
                        stack_trace: None,
                    })
                    .collect();
                collect_test_results(&json.suites, &mut failures);
                let stats = &json.stats;
                // The stats count executions (a test in each project), so
                // the total does too; a flaky test passed on its retry.
                let result = TestResult {
                    total: stats.expected + stats.unexpected + stats.flaky + stats.skipped,
                    passed: stats.expected + stats.flaky,
                    failed: stats.unexpected,
                    skipped: stats.skipped,
                    duration_ms: Some(json.stats.duration as u64),
                    failures,
                };

                ParseResult::Full(result)
            }
            Err(e) => {
                // Tier 2: Try regex extraction
                match extract_playwright_regex(input) {
                    Some(result) => {
                        ParseResult::Degraded(result, vec![format!("JSON parse failed: {}", e)])
                    }
                    None => {
                        // Tier 3: Passthrough
                        ParseResult::Passthrough(truncate_passthrough(input))
                    }
                }
            }
        }
    }
}

/// One failure per failed execution, so a test that fails in two projects
/// is listed twice, matching the failed count; the project is named when
/// the test ran in more than one.
fn collect_test_results(suites: &[PlaywrightSuite], failures: &mut Vec<TestFailure>) {
    for suite in suites {
        let file_path = suite.file.as_deref().unwrap_or(&suite.title);

        // Every spec is searched: the reporter can keep `ok` from the first
        // project when a later one failed.
        for spec in &suite.specs {
            for test in spec.tests.iter().filter(|t| t.status == "unexpected") {
                let error_msg = test
                    .results
                    .iter()
                    .find(|r| r.status == "failed" || r.status == "timedOut")
                    .and_then(|r| r.errors.first())
                    .map(PlaywrightError::text)
                    .unwrap_or_else(|| "Test failed".to_string());
                let test_name = if spec.tests.len() > 1 && !test.project_name.is_empty() {
                    format!("{} [{}]", spec.title, test.project_name)
                } else {
                    spec.title.clone()
                };
                failures.push(TestFailure {
                    test_name,
                    file_path: file_path.to_string(),
                    error_message: error_msg,
                    stack_trace: None,
                });
            }
        }

        // Recurse into nested suites (describe blocks)
        collect_test_results(&suite.suites, failures);
    }
}

/// Tier 2: Extract test statistics using regex (degraded mode)
fn extract_playwright_regex(output: &str) -> Option<TestResult> {
    static SUMMARY_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(\d+)\s+(passed|failed|flaky|skipped)").unwrap());
    static DURATION_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\((\d+(?:\.\d+)?)(ms|s|m)\)").unwrap());

    let clean_output = strip_ansi(output);

    let mut passed = 0;
    let mut failed = 0;
    let mut skipped = 0;

    // Parse summary counts
    for caps in SUMMARY_RE.captures_iter(&clean_output) {
        let count: usize = caps[1].parse().unwrap_or(0);
        match &caps[2] {
            "passed" => passed = count,
            "failed" => failed = count,
            "skipped" => skipped = count,
            _ => {}
        }
    }

    // Parse duration
    let duration_ms = DURATION_RE.captures(&clean_output).and_then(|caps| {
        let value: f64 = caps[1].parse().ok()?;
        let unit = &caps[2];
        Some(match unit {
            "ms" => value as u64,
            "s" => (value * 1000.0) as u64,
            "m" => (value * 60000.0) as u64,
            _ => value as u64,
        })
    });

    // Only return if we found valid data
    let total = passed + failed + skipped;
    if total > 0 {
        Some(TestResult {
            total,
            passed,
            failed,
            skipped,
            duration_ms,
            failures: extract_failures_regex(&clean_output),
        })
    } else {
        None
    }
}

/// Extract failures using regex
fn extract_failures_regex(output: &str) -> Vec<TestFailure> {
    static TEST_PATTERN: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"[×✗]\s+.*?›\s+([^›]+\.spec\.[tj]sx?)").unwrap());

    let mut failures = Vec::new();

    for caps in TEST_PATTERN.captures_iter(output) {
        if let Some(spec) = caps.get(1) {
            failures.push(TestFailure {
                test_name: caps[0].to_string(),
                file_path: spec.as_str().to_string(),
                error_message: String::new(),
                stack_trace: None,
            });
        }
    }

    failures
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    // Skip `which playwright` — it can find pyenv shims or other non-Node
    // binaries. Always resolve through the package manager.
    let pm = detect_package_manager();
    let mut cmd = match pm {
        "pnpm" => {
            let mut c = resolved_command("pnpm");
            c.arg("exec").arg("--").arg("playwright");
            c
        }
        "yarn" => {
            let mut c = resolved_command("yarn");
            c.arg("exec").arg("--").arg("playwright");
            c
        }
        _ => {
            let mut c = resolved_command("npx");
            c.arg("--no-install").arg("--").arg("playwright");
            c
        }
    };

    // Only inject --reporter=json for `playwright test` runs
    let is_test = args.first().map(|a| a == "test").unwrap_or(false);
    if is_test {
        cmd.arg("test");
        cmd.arg("--reporter=json");
        // Strip user's --reporter to avoid conflicts with our forced JSON
        for arg in &args[1..] {
            if !arg.starts_with("--reporter") {
                cmd.arg(arg);
            }
        }
    } else {
        for arg in args {
            cmd.arg(arg);
        }
    }

    if verbose > 0 {
        eprintln!("Running: playwright {}", args.join(" "));
    }

    let result = exec_capture(&mut cmd)
        .context("Failed to run playwright (try: npm install -g playwright)")?;

    let raw = format!("{}\n{}", result.stdout, result.stderr);

    // Parse output using PlaywrightParser
    let parse_result = PlaywrightParser::parse(&result.stdout);
    let mode = FormatMode::from_verbosity(verbose);

    let filtered = match parse_result {
        ParseResult::Full(data) => {
            if verbose > 0 {
                eprintln!("playwright test (Tier 1: Full JSON parse)");
            }
            data.format(mode)
        }
        ParseResult::Degraded(data, warnings) => {
            if verbose > 0 {
                emit_degradation_warning("playwright", &warnings.join(", "));
            }
            data.format(mode)
        }
        ParseResult::Passthrough(raw) => {
            emit_passthrough_warning("playwright", "All parsing tiers failed");
            raw
        }
    };

    let hint = crate::core::tee::tee_and_hint(&raw, "playwright", result.exit_code);
    let shown = crate::core::runner::emit_guarded(&filtered, hint.as_deref(), &raw);

    let tracked = with_args("playwright", &display_args(args));
    timer.track(&tracked, &format!("rtk {tracked}"), &raw, &shown);

    // Preserve exit code for CI/CD
    if !result.success() {
        return Ok(result.exit_code);
    }

    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_playwright_parser_json() {
        // Real Playwright JSON structure: suites → specs, with float duration
        let json = r#"{
            "config": {},
            "stats": {
                "startTime": "2026-01-01T00:00:00.000Z",
                "expected": 1,
                "unexpected": 0,
                "skipped": 0,
                "flaky": 0,
                "duration": 7300.5
            },
            "suites": [
                {
                    "title": "auth",
                    "specs": [],
                    "suites": [
                        {
                            "title": "login.spec.ts",
                            "specs": [
                                {
                                    "title": "should login",
                                    "ok": true,
                                    "tests": [
                                        {
                                            "status": "expected",
                                            "results": [{"status": "passed", "errors": [], "duration": 2300}]
                                        }
                                    ]
                                }
                            ],
                            "suites": []
                        }
                    ]
                }
            ],
            "errors": []
        }"#;

        let result = PlaywrightParser::parse(json);
        assert_eq!(result.tier(), 1);
        assert!(result.is_ok());

        let data = result.unwrap();
        assert_eq!(data.passed, 1);
        assert_eq!(data.failed, 0);
        assert_eq!(data.duration_ms, Some(7300));
    }

    #[test]
    fn test_playwright_parser_json_float_duration() {
        // Real Playwright output uses float duration (e.g. 3519.7039999999997)
        let json = r#"{
            "stats": {
                "startTime": "2026-02-18T10:17:53.187Z",
                "expected": 4,
                "unexpected": 0,
                "skipped": 0,
                "flaky": 0,
                "duration": 3519.7039999999997
            },
            "suites": [],
            "errors": []
        }"#;

        let result = PlaywrightParser::parse(json);
        assert_eq!(result.tier(), 1);
        assert!(result.is_ok());

        let data = result.unwrap();
        assert_eq!(data.passed, 4);
        assert_eq!(data.duration_ms, Some(3519));
    }

    #[test]
    fn test_playwright_parser_json_with_failure() {
        let json = r#"{
            "stats": {
                "expected": 0,
                "unexpected": 1,
                "skipped": 0,
                "duration": 1500.0
            },
            "suites": [
                {
                    "title": "my.spec.ts",
                    "specs": [
                        {
                            "title": "should work",
                            "ok": false,
                            "tests": [
                                {
                                    "status": "unexpected",
                                    "results": [
                                        {
                                            "status": "failed",
                                            "errors": [{"message": "Expected true to be false"}],
                                            "duration": 500
                                        }
                                    ]
                                }
                            ]
                        }
                    ],
                    "suites": []
                }
            ],
            "errors": []
        }"#;

        let result = PlaywrightParser::parse(json);
        assert_eq!(result.tier(), 1);
        assert!(result.is_ok());

        let data = result.unwrap();
        assert_eq!(data.failed, 1);
        assert_eq!(data.failures.len(), 1);
        assert_eq!(data.failures[0].test_name, "should work");
        assert_eq!(data.failures[0].error_message, "Expected true to be false");
    }

    #[test]
    fn totals_count_every_project_and_flaky_tests() {
        let json = r#"{
            "stats": {"expected": 3, "unexpected": 2, "skipped": 1, "flaky": 1, "duration": 10.0},
            "suites": [{
                "title": "login.spec.ts",
                "file": "login.spec.ts",
                "specs": [
                    {"title": "logs in", "ok": false, "tests": [
                        {"projectName": "chromium", "status": "unexpected",
                         "results": [{"status": "failed", "errors": [{"message": "no button"}]}]},
                        {"projectName": "firefox", "status": "unexpected",
                         "results": [{"status": "timedOut", "errors": [{"message": "timed out"}]}]},
                        {"projectName": "webkit", "status": "expected", "results": [{"status": "passed"}]}
                    ]},
                    {"title": "logs out", "ok": true, "tests": [
                        {"projectName": "chromium", "status": "flaky",
                         "results": [{"status": "failed", "errors": [{"message": "race"}]}, {"status": "passed"}]},
                        {"projectName": "firefox", "status": "expected", "results": [{"status": "passed"}]},
                        {"projectName": "webkit", "status": "skipped", "results": []}
                    ]},
                    {"title": "remembers me", "ok": true, "tests": [
                        {"projectName": "chromium", "status": "expected", "results": [{"status": "passed"}]}
                    ]}
                ]
            }],
            "errors": []
        }"#;
        let data = PlaywrightParser::parse(json).unwrap();
        assert_eq!(
            (data.total, data.passed, data.failed, data.skipped),
            (7, 4, 2, 1)
        );
        let failures: Vec<_> = data
            .failures
            .iter()
            .map(|f| (f.test_name.as_str(), f.error_message.as_str()))
            .collect();
        assert_eq!(
            failures,
            [
                ("logs in [chromium]", "no button"),
                ("logs in [firefox]", "timed out")
            ]
        );
    }

    #[test]
    fn errors_outside_tests_are_reported() {
        let json = r#"{
            "stats": {"expected": 0, "unexpected": 0, "skipped": 0, "flaky": 0, "duration": 5.0},
            "suites": [],
            "errors": [{"message": "Error: globalTeardown failed: database still locked"}]
        }"#;
        let data = PlaywrightParser::parse(json).unwrap();
        assert_eq!(data.failures.len(), 1);
        assert_eq!(data.failures[0].test_name, "error outside any test");
        let shown = data.format(FormatMode::Compact);
        assert!(
            shown.contains("globalTeardown failed: database still locked"),
            "{shown}"
        );
    }

    #[test]
    fn a_failure_in_a_later_project_is_listed_when_the_spec_reads_ok() {
        let json = r#"{
            "stats": {"expected": 1, "unexpected": 1, "skipped": 0, "flaky": 0, "duration": 1.0},
            "suites": [{"title": "cart.spec.ts", "specs": [
                {"title": "adds an item", "ok": true, "tests": [
                    {"projectName": "chromium", "status": "expected", "results": [{"status": "passed"}]},
                    {"projectName": "firefox", "status": "unexpected",
                     "results": [{"status": "failed", "errors": [{"message": "count was 0"}]}]}
                ]}
            ]}]
        }"#;
        let data = PlaywrightParser::parse(json).unwrap();
        assert_eq!(data.failed, 1);
        assert_eq!(data.failures.len(), 1);
        assert_eq!(data.failures[0].test_name, "adds an item [firefox]");
        assert_eq!(data.failures[0].error_message, "count was 0");
    }

    #[test]
    fn errors_outside_tests_lead_the_failures_and_keep_thrown_values() {
        let failing: Vec<String> = (1..=5)
            .map(|i| {
                format!(
                    r#"{{"title": "case {i}", "ok": false, "tests": [{{"status": "unexpected",
                        "results": [{{"status": "failed", "errors": [{{"message": "boom {i}"}}]}}]}}]}}"#
                )
            })
            .collect();
        let json = format!(
            r#"{{"stats": {{"expected": 0, "unexpected": 5, "skipped": 0, "duration": 1.0}},
                "suites": [{{"title": "many.spec.ts", "specs": [{}]}}],
                "errors": [{{"message": "Error: globalTeardown failed"}}, {{"value": "'stopped'"}}]}}"#,
            failing.join(",")
        );
        let data = PlaywrightParser::parse(&json).unwrap();
        assert_eq!(data.failures.len(), 7);
        assert_eq!(data.failures[1].error_message, "'stopped'");
        let shown = data.format(FormatMode::Compact);
        assert!(shown.contains("globalTeardown failed"), "{shown}");
        assert!(shown.contains("'stopped'"), "{shown}");
    }

    #[test]
    fn test_playwright_parser_regex_fallback() {
        let text = "3 passed (7.3s)";
        let result = PlaywrightParser::parse(text);
        assert_eq!(result.tier(), 2); // Degraded
        assert!(result.is_ok());

        let data = result.unwrap();
        assert_eq!(data.passed, 3);
        assert_eq!(data.failed, 0);
    }

    #[test]
    fn test_playwright_parser_passthrough() {
        let invalid = "random output";
        let result = PlaywrightParser::parse(invalid);
        assert_eq!(result.tier(), 3); // Passthrough
        assert!(!result.is_ok());
    }
}

#[cfg(test)]
#[path = "coverage_gen/playwright_cmd_coverage_gen.rs"]
mod coverage_gen_src_cmds_js_playwright_cmd;
