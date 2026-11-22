#![cfg(unix)]

use super::npx_package;
use crate::core::test_isolation::{rtk_binary_is_built, rtk_command, tempdir};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Output;

fn write_tool(dir: &Path, name: &str, contents: &str) {
    let path = dir.join(name);
    fs::write(&path, contents).expect("write fake tool");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .expect("make fake tool executable");
}

fn gain_with_tools(dir: &Path) -> Output {
    // Use only the fixture directory so a host ccusage or npx cannot run.
    let output = rtk_command()
        .env("PATH", dir)
        .args(["gain", "--all"])
        .output()
        .expect("run isolated rtk gain");
    assert!(
        output.status.success(),
        "rtk gain failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[test]
fn successful_npx_probe_is_followed_by_pinned_usage_request() {
    if !rtk_binary_is_built() {
        return;
    }

    let tools = tempdir();
    write_tool(
        tools.path(),
        "npx",
        r#"#!/bin/sh
printf '%s\n' "$*" >> "${0%/*}/npx-calls"
case "$3" in
    --help) exit 0 ;;
    daily|weekly|monthly) printf '{"%s":[]}\n' "$3" ;;
    *) exit 2 ;;
esac
"#,
    );

    let output = gain_with_tools(tools.path());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("[warn] ccusage"), "{stderr}");

    let package = npx_package().expect("pinned ccusage package");
    let probe = format!("--yes {package} --help");
    let daily = format!("--yes {package} daily --json --since 20250101");
    let trace = fs::read_to_string(tools.path().join("npx-calls")).expect("npx invocation trace");
    let calls: Vec<_> = trace.lines().collect();
    assert!(
        calls
            .windows(2)
            .any(|pair| pair == [probe.as_str(), daily.as_str()]),
        "expected a successful probe followed by the pinned daily request; got:\n{trace}"
    );
}

#[test]
fn unlaunchable_ccusage_warns_without_failing_gain() {
    if !rtk_binary_is_built() {
        return;
    }

    let tools = tempdir();
    // The executable is discoverable, but the interpreter cannot exist under /dev/null.
    write_tool(
        tools.path(),
        "ccusage",
        "#!/dev/null/rtk-missing-interpreter\n",
    );

    let output = gain_with_tools(tools.path());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[warn] ccusage execution failed:"),
        "{stderr}"
    );
    assert!(!stderr.contains("[warn] ccusage not found"), "{stderr}");
    assert!(
        !output.stdout.is_empty(),
        "expected an RTK-only gain report"
    );
}
