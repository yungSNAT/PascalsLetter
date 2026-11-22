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

#[cfg(unix)]
#[test]
fn verbose_cli_passes_json_format_to_sqlfluff_and_renders_clean_result() {
    use crate::core::test_isolation;
    use std::os::unix::fs::PermissionsExt;

    if !test_isolation::rtk_binary_is_built() {
        return;
    }

    let dir = tempfile::tempdir().expect("create fake tool directory");
    let tool = dir.path().join("sqlfluff");
    std::fs::write(
        &tool,
        r#"#!/bin/sh
if [ "$#" -eq 4 ] && [ "$1" = lint ] && [ "$2" = --format ] &&
    [ "$3" = json ] && [ "$4" = query.sql ]; then
    printf '%s\n' '[]'
else
    printf 'unexpected fake sqlfluff arguments: %s\n' "$*" >&2
fi
"#,
    )
    .expect("write fake sqlfluff");
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755))
        .expect("make fake sqlfluff executable");

    let mut cmd = test_isolation::rtk_command();
    let inherited_path = cmd
        .get_envs()
        .find(|(name, _)| *name == "PATH")
        .and_then(|(_, value)| value);
    let path = std::env::join_paths(
        std::iter::once(dir.path().to_path_buf())
            .chain(inherited_path.into_iter().flat_map(std::env::split_paths)),
    )
    .expect("prepend fake tool directory to child PATH");

    let output = cmd
        .env("PATH", path)
        .current_dir(dir.path())
        .args(["-v", "sqlfluff", "lint", "query.sql"])
        .output()
        .expect("run isolated rtk sqlfluff");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout.trim(), "✓ SQLFluff: No violations found", "{stderr}");
    assert!(
        stderr
            .lines()
            .any(|line| line == "Running: sqlfluff lint --format json query.sql"),
        "{stderr}"
    );
}
