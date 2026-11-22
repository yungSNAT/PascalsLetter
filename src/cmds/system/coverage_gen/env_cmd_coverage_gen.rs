use super::{CAP_LIST, CAP_WARNINGS, render};
use std::fmt::Write as _;

#[test]
fn filtered_render_groups_cloud_and_tool_vars_and_caps_other_vars() {
    let preview = "é".repeat(50);
    let mut vars = vec![
        ("FIXTURE_AWS_REGION".to_owned(), format!("{preview}tail")),
        ("FIXTURE_EDITOR".to_owned(), "fixture-editor".to_owned()),
    ];
    vars.extend(
        (0..(CAP_LIST + 2)).map(|i| (format!("FIXTURE_VALUE_{i:04}"), format!("value-{i}"))),
    );
    vars.push(("UNRELATED".to_owned(), "not listed".to_owned()));

    let mut expected = format!(
        "\nCloud/Services:\n  FIXTURE_AWS_REGION={preview}... (54 chars)\n\
         \nTools:\n  FIXTURE_EDITOR=fixture-editor\n\
         \nOther:\n"
    );
    for i in 0..CAP_LIST {
        writeln!(expected, "  FIXTURE_VALUE_{i:04}=value-{i}").unwrap();
    }
    expected.push_str("  ... +2 more\n");

    assert_eq!(render(&vars, Some("fixture_")), expected);
}

#[test]
fn render_caps_search_path_entries_but_prints_other_path_vars_as_assignments() {
    let (path_key, other_key) = if cfg!(windows) {
        ("pAtH", "pythonPath")
    } else {
        ("PATH", "PYTHONPATH")
    };
    let entries: Vec<_> = (0..(CAP_WARNINGS + 2))
        .map(|i| format!("fixture-tool-{i}"))
        .collect();
    let path = std::env::join_paths(&entries)
        .unwrap()
        .into_string()
        .unwrap();
    let vars = [
        (path_key.to_owned(), path),
        (other_key.to_owned(), "fixture-modules".to_owned()),
    ];

    let mut expected = format!("PATH Variables:\n  PATH ({} entries):\n", entries.len());
    for entry in entries.iter().take(CAP_WARNINGS) {
        writeln!(expected, "    {entry}").unwrap();
    }
    expected.push_str("    ... +2 more\n");
    writeln!(expected, "  {other_key}=fixture-modules").unwrap();

    assert_eq!(render(&vars, Some("path")), expected);
}

#[cfg(all(unix, feature = "engine"))]
#[test]
fn verbose_env_cli_prints_sorted_filtered_assignments_when_raw_is_shorter() {
    use crate::core::test_isolation::{rtk_binary_is_built, rtk_command};

    if !rtk_binary_is_built() {
        return;
    }

    let output = rtk_command()
        .args(["-v", "env", "-f", "rtk_env_coverage_"])
        .env("RTK_ENV_COVERAGE_Z", "last")
        .env("RTK_ENV_COVERAGE_A", "first")
        .env("RTK_ENV_UNMATCHED", "not listed")
        .output()
        .expect("run the isolated rtk env command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{}: {stderr}", output.status);
    assert!(
        stderr.lines().any(|line| line == "Environment variables:"),
        "{stderr}"
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "RTK_ENV_COVERAGE_A=first\nRTK_ENV_COVERAGE_Z=last\n"
    );
}
