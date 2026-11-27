// Coverage tests for src/cmds/dotnet/dotnet_cmd.rs

use super::*;
use crate::core::test_isolation;

fn owned(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

fn issue(code: &str, file: &str, message: &str) -> binlog::BinlogIssue {
    binlog::BinlogIssue {
        code: code.to_string(),
        file: file.to_string(),
        line: 3,
        column: 7,
        message: message.to_string(),
    }
}

#[test]
fn passthrough_without_subcommand_is_an_error() {
    let err = run_passthrough(&[], 0).expect_err("empty args must bail");
    assert!(err.to_string().contains("no subcommand"), "{err}");
}

#[test]
fn format_report_fallbacks_return_raw_output() {
    let dir = test_isolation::tempdir();
    let raw = "RAW FORMAT OUTPUT";

    // No report path at all.
    assert_eq!(format_report_summary_or_raw(None, true, raw, None), raw);

    // Report path that does not exist: metadata fails.
    let missing = dir.path().join("missing.json");
    assert!(!is_fresh_report(&missing, None));
    assert_eq!(
        format_report_summary_or_raw(Some(&missing), true, raw, None),
        raw
    );

    // Fresh but unparsable report.
    let broken = dir.path().join("broken.json");
    std::fs::write(&broken, "this is { not json").expect("write broken report");
    assert_eq!(
        format_report_summary_or_raw(Some(&broken), true, raw, None),
        raw
    );
}

#[test]
fn format_report_summary_lists_capped_files_and_write_mode() {
    let dir = test_isolation::tempdir();
    let changed = CAP_LIST + 2;
    let mut entries: Vec<Value> = (0..changed)
        .map(|i| {
            serde_json::json!({
                "DocumentId": {
                    "ProjectId": { "Id": "00000000-0000-0000-0000-000000000001" },
                    "Id": format!("00000000-0000-0000-0000-{:012}", i + 10)
                },
                "FileName": format!("File{i}.cs"),
                "FilePath": format!("src/File{i}.cs"),
                "FileChanges": [{
                    "LineNumber": 1,
                    "CharNumber": 2,
                    "DiagnosticId": "",
                    "FormatDescription": "Fix whitespace formatting."
                }]
            })
        })
        .collect();
    entries.push(serde_json::json!({
        "DocumentId": {
            "ProjectId": { "Id": "00000000-0000-0000-0000-000000000001" },
            "Id": "00000000-0000-0000-0000-000000000002"
        },
        "FileName": "Clean.cs",
        "FilePath": "src/Clean.cs",
        "FileChanges": []
    }));
    let report = dir.path().join("report.json");
    std::fs::write(&report, Value::Array(entries).to_string()).expect("write report");

    let root = test_isolation::tempdir();
    test_isolation::with_root(root.path(), || {
        let check = format_report_summary_or_raw(Some(&report), true, "RAW", None);
        assert!(
            check.contains(&format!("Format: {changed} files need formatting")),
            "{check}"
        );
        assert!(
            check.contains("line 1, col 2, Fix whitespace formatting.)"),
            "{check}"
        );
        assert!(check.contains("… +2 more files"), "{check}");
        assert!(
            check.contains("Run `dotnet format` to apply fixes"),
            "{check}"
        );

        let write = format_report_summary_or_raw(Some(&report), false, "RAW", None);
        assert!(
            write.starts_with(&format!("ok dotnet format: formatted {changed} files (")),
            "{write}"
        );
        assert!(write.contains("already formatted"), "{write}");
    });
}

#[test]
fn build_output_caps_errors_and_warnings() {
    let mut errors = vec![issue("", "", "bare message without file")];
    errors.extend((0..CAP_ERRORS + 2).map(|i| issue("CS0001", "src/A.cs", &format!("err {i}"))));
    let warnings: Vec<binlog::BinlogIssue> = (0..CAP_WARNINGS + 2)
        .map(|i| issue("CS0219", "src/B.cs", &format!("warn {i}")))
        .collect();
    let error_count = errors.len();
    let warning_count = warnings.len();
    let summary = binlog::BuildSummary {
        succeeded: false,
        project_count: 4,
        errors,
        warnings,
        duration_text: None,
    };

    let root = test_isolation::tempdir();
    test_isolation::with_root(root.path(), || {
        let output = format_build_output(&summary, Path::new("build.binlog"));
        assert!(
            output.contains("  error bare message without file"),
            "{output}"
        );
        assert!(output.contains("… +3 more errors"), "{output}");
        assert!(output.contains("… +2 more warnings"), "{output}");
        let last = output.lines().last().expect("non-empty output");
        assert_eq!(
            last,
            format!(
                "fail dotnet build: 4 projects, {error_count} errors, {warning_count} warnings (unknown)"
            )
        );
    });
}

#[test]
fn test_output_caps_failed_tests_and_errors() {
    let failed_tests: Vec<binlog::FailedTest> = (0..CAP_WARNINGS + 2)
        .map(|i| binlog::FailedTest {
            name: format!("Suite.Case{i}"),
            details: vec![format!("Assert failure {i}")],
        })
        .collect();
    let failed = failed_tests.len();
    let summary = binlog::TestSummary {
        passed: 1,
        failed,
        skipped: 0,
        total: failed + 1,
        project_count: 1,
        failed_tests,
        duration_text: Some("2 s".to_string()),
    };
    let errors: Vec<binlog::BinlogIssue> = (0..CAP_WARNINGS + 1)
        .map(|i| issue("TESTERROR", "tests/T.cs", &format!("boom {i}")))
        .collect();

    let root = test_isolation::tempdir();
    test_isolation::with_root(root.path(), || {
        let output = format_test_output(&summary, &errors, &[], Path::new("test.binlog"));
        assert!(output.contains("Failed Tests:"), "{output}");
        assert!(output.contains("… +2 more failed tests"), "{output}");
        assert!(output.contains("… +1 more errors"), "{output}");
        let last = output.lines().last().expect("non-empty output");
        assert!(
            last.starts_with(&format!("fail dotnet test: 1 passed, {failed} failed")),
            "{last}"
        );
    });
}

#[test]
fn summary_normalization_and_merging() {
    let built = normalize_build_summary(binlog::BuildSummary::default(), true);
    assert!(built.succeeded);
    assert_eq!(built.project_count, 1);
    let failed_build = normalize_build_summary(binlog::BuildSummary::default(), false);
    assert!(!failed_build.succeeded);
    assert_eq!(failed_build.project_count, 0);

    let raw_build = binlog::BuildSummary {
        warnings: vec![issue("CS1", "a.cs", "w")],
        project_count: 3,
        duration_text: Some("1s".to_string()),
        ..Default::default()
    };
    let merged = merge_build_summaries(binlog::BuildSummary::default(), raw_build);
    assert_eq!(merged.warnings.len(), 1);
    assert_eq!(merged.project_count, 3);
    assert_eq!(merged.duration_text.as_deref(), Some("1s"));

    let failed_tests = normalize_test_summary(
        binlog::TestSummary {
            total: 5,
            passed: 5,
            ..Default::default()
        },
        false,
    );
    assert_eq!((failed_tests.failed, failed_tests.total), (1, 5));
    let ok_tests = normalize_test_summary(binlog::TestSummary::default(), true);
    assert_eq!(ok_tests.project_count, 1);
    assert_eq!(ok_tests.failed, 0);

    let ok_restore = normalize_restore_summary(binlog::RestoreSummary::default(), true);
    assert_eq!(ok_restore.errors, 0);
    let bad_restore = normalize_restore_summary(binlog::RestoreSummary::default(), false);
    assert_eq!(bad_restore.errors, 1);

    let raw_restore = binlog::RestoreSummary {
        restored_projects: 4,
        warnings: 2,
        errors: 1,
        duration_text: Some("3s".to_string()),
    };
    let merged = merge_restore_summaries(binlog::RestoreSummary::default(), raw_restore);
    assert_eq!(merged.restored_projects, 4);
    assert_eq!(merged.errors, 1);
    assert_eq!(merged.warnings, 2);
    assert_eq!(merged.duration_text.as_deref(), Some("3s"));

    assert_eq!(
        compose_failure_output(false, true, "  ", "\n", "filtered only"),
        "filtered only"
    );
}

#[test]
fn runner_mode_detection_edge_cases() {
    let dir = test_isolation::tempdir();

    let missing_json = dir.path().join("global.json");
    assert!(!parse_global_json_mtp_mode(&missing_json));
    std::fs::write(&missing_json, "{ not valid json").expect("write global.json");
    assert!(!parse_global_json_mtp_mode(&missing_json));

    let nowhere = dir.path().join("does-not-exist");
    assert!(!is_global_json_mtp_mode(&nowhere));
    let no_args: Vec<String> = Vec::new();
    let tokens = tokenize_dotnet_args(&no_args);
    assert_eq!(
        detect_test_runner_mode_in_dir(&tokens, &nowhere),
        TestRunnerMode::Classic
    );

    let props_dir = test_isolation::tempdir();
    std::fs::write(
        props_dir.path().join("Plain.Tests.csproj"),
        r#"<Project Sdk="Microsoft.NET.Sdk"></Project>"#,
    )
    .expect("write csproj");
    std::fs::write(
        props_dir.path().join("Directory.Build.props"),
        "<Project><PropertyGroup><TestingPlatformDotnetTestSupport>true</TestingPlatformDotnetTestSupport></PropertyGroup></Project>",
    )
    .expect("write props");
    assert_eq!(
        detect_test_runner_mode_in_dir(&tokens, props_dir.path()),
        TestRunnerMode::MtpVsTestBridge
    );
}

#[test]
fn effective_args_for_build_and_mtp_native() {
    let args = owned(&["-c", "Release"]);
    let tokens = tokenize_dotnet_args(&args);
    let effective = build_effective_dotnet_args(
        "build",
        &args,
        &tokens,
        Path::new("rtk.binlog"),
        None,
        TestRunnerMode::Classic,
    );
    assert_eq!(
        effective,
        ["-bl:rtk.binlog", "-v:minimal", "-nologo", "-c", "Release"]
    );

    let args = owned(&["--no-build"]);
    let tokens = tokenize_dotnet_args(&args);
    let effective = build_effective_dotnet_args(
        "test",
        &args,
        &tokens,
        Path::new("rtk.binlog"),
        Some(Path::new("results")),
        TestRunnerMode::MtpNative,
    );
    assert_eq!(effective, ["--report-trx", "--no-build"]);

    let args = owned(&["--report-trx"]);
    let tokens = tokenize_dotnet_args(&args);
    let effective = build_effective_dotnet_args(
        "test",
        &args,
        &tokens,
        Path::new("rtk.binlog"),
        None,
        TestRunnerMode::MtpNative,
    );
    assert_eq!(effective, ["--report-trx"]);
}

#[test]
fn stale_trx_in_results_dir_is_still_used_as_last_resort() {
    let dir = test_isolation::tempdir();
    let trx = dir.path().join("old.trx");
    std::fs::write(
        &trx,
        r#"<?xml version="1.0" encoding="utf-8"?>
<TestRun xmlns="http://microsoft.com/schemas/VisualStudio/TeamTest/2010">
  <ResultSummary outcome="Completed">
    <Counters total="4" executed="4" passed="3" failed="1" error="0" />
  </ResultSummary>
</TestRun>"#,
    )
    .expect("write trx");
    std::fs::File::options()
        .write(true)
        .open(&trx)
        .expect("open trx")
        .set_modified(SystemTime::now() - std::time::Duration::from_secs(10))
        .expect("set mtime");
    let started = SystemTime::now();

    let summary = merge_test_summary_from_trx(
        binlog::TestSummary::default(),
        Some(dir.path()),
        None,
        &[],
        started,
    );
    assert_eq!(summary.total, 4);
    assert_eq!(summary.passed, 3);
    assert_eq!(summary.failed, 1);
}

#[test]
fn reported_trx_paths_skip_empty_trailing_marker() {
    assert!(find_reported_trx_paths("Results File:").is_empty());
    let paths = find_reported_trx_paths("noise Results File: one.trxResults File:");
    assert_eq!(paths, vec![PathBuf::from("one.trx")]);
}

#[cfg(unix)]
const FAKE_DOTNET: &str = r#"#!/bin/sh
sub="$1"
touch_binlog=""
keep_report=""
for arg in "$@"; do
  if [ "$arg" = "touch-binlog" ]; then touch_binlog=1; fi
  if [ "$arg" = "keep-report" ]; then keep_report=1; fi
done
prev=""
for arg in "$@"; do
  case "$arg" in
    -bl:*) if [ -n "$touch_binlog" ]; then : > "${arg#-bl:}"; fi ;;
  esac
  if [ "$prev" = "--report" ] && [ -z "$keep_report" ]; then printf '[]' > "$arg"; fi
  prev="$arg"
done
i=0
while [ "$i" -lt 40 ]; do
  echo "fake dotnet $sub output line $i with some padding text to keep it long"
  i=$((i+1))
done
exit 0
"#;

#[cfg(unix)]
fn run_rtk_with_fake_dotnet(args: &[&str]) -> std::process::Output {
    use std::os::unix::fs::PermissionsExt;
    let dir = test_isolation::tempdir();
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).expect("create bin dir");
    let script = bin.join("dotnet");
    std::fs::write(&script, FAKE_DOTNET).expect("write fake dotnet");
    let mut perms = std::fs::metadata(&script).expect("metadata").permissions();
    perms.set_mode(0o700);
    std::fs::set_permissions(&script, perms).expect("chmod fake dotnet");

    let mut cmd = test_isolation::rtk_command();
    let inherited = cmd
        .get_envs()
        .find(|(k, _)| k.to_str() == Some("PATH"))
        .and_then(|(_, v)| v.map(|v| v.to_os_string()));
    let mut path = OsString::from(bin.as_os_str());
    if let Some(inherited) = inherited {
        path.push(":");
        path.push(inherited);
    }
    cmd.env("PATH", path);
    cmd.current_dir(dir.path());
    cmd.args(args);
    cmd.output().expect("run rtk")
}

#[cfg(unix)]
#[test]
fn rtk_dotnet_restore_and_build_with_fake_tool() {
    if !test_isolation::rtk_binary_ready() {
        return;
    }

    let out = run_rtk_with_fake_dotnet(&["-v", "dotnet", "restore", "touch-binlog"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stdout}\n{stderr}");
    assert!(stdout.contains("ok dotnet restore:"), "{stdout}");
    assert!(stderr.contains("Running: dotnet restore"), "{stderr}");
    assert!(stderr.contains("Binlog cleaned up"), "{stderr}");

    let out = run_rtk_with_fake_dotnet(&["dotnet", "restore"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains("ok dotnet restore:"), "{stdout}");

    let out = run_rtk_with_fake_dotnet(&["dotnet", "build", "touch-binlog"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains("ok dotnet build:"), "{stdout}");
}

#[cfg(unix)]
#[test]
fn rtk_dotnet_help_passthrough_with_fake_tool() {
    if !test_isolation::rtk_binary_ready() {
        return;
    }

    let out = run_rtk_with_fake_dotnet(&["-v", "dotnet", "build", "-c", "Release", "--help"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stdout}\n{stderr}");
    assert!(stderr.contains("Running: dotnet build ..."), "{stderr}");
    assert!(stdout.contains("fake dotnet build output line"), "{stdout}");

    let out = run_rtk_with_fake_dotnet(&["dotnet", "format", "-v", "q", "--help"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    assert!(
        stdout.contains("fake dotnet format output line"),
        "{stdout}"
    );
}

#[cfg(unix)]
#[test]
fn rtk_dotnet_format_summarises_only_a_report_the_run_wrote() {
    if !test_isolation::rtk_binary_ready() {
        return;
    }

    // The run writes RTK's own report: its summary replaces the raw output.
    let out = run_rtk_with_fake_dotnet(&["dotnet", "format"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    assert!(
        stdout.contains("ok dotnet format: 0 files formatted correctly"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("fake dotnet format output line"),
        "{stdout}"
    );

    // A report left by an earlier run and untouched by this one is stale: the
    // raw output stands, not the old report's summary.
    let dir = test_isolation::tempdir();
    let stale = dir.path().join("stale.json");
    std::fs::copy(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dotnet/format_success.json"),
        &stale,
    )
    .expect("write stale report");
    let stale_arg = stale.to_str().expect("utf-8 temp path");
    let out = run_rtk_with_fake_dotnet(&["dotnet", "format", "--report", stale_arg, "keep-report"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    assert!(
        stdout.contains("fake dotnet format output line"),
        "{stdout}"
    );
    assert!(!stdout.contains("2 files formatted correctly"), "{stdout}");
}
