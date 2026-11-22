use super::*;

#[test]
fn malformed_package_json_identifies_the_failed_query() {
    let input = r#"[{"name":"demo"}]"#;

    let list = filter_pip_list(input);
    assert!(list.starts_with("pip list (JSON parse failed: "), "{list}");

    let outdated = filter_pip_outdated(input);
    assert!(
        outdated.starts_with("pip outdated (JSON parse failed: "),
        "{outdated}"
    );
}

#[test]
fn inventory_reports_hidden_rows_without_hiding_the_next_letter_group() {
    let mut packages = vec![serde_json::json!({"name": "Beta", "version": "2.0"})];
    packages.extend(
        (1..=CAP_INVENTORY + 2)
            .map(|index| serde_json::json!({"name": format!("Alpha-{index}"), "version": "1.0"})),
    );

    let summary = filter_pip_list(&serde_json::to_string(&packages).unwrap());

    assert!(summary.starts_with(&format!("pip list: {} packages\n", packages.len())));
    assert_eq!(
        summary
            .lines()
            .filter(|line| line.starts_with("  Alpha-"))
            .count(),
        CAP_INVENTORY
    );
    assert!(
        summary.ends_with(&format!(
            "  Alpha-{CAP_INVENTORY} (1.0)\n  ... +2 more\n\n[B]\n  Beta (2.0)"
        )),
        "{summary}"
    );
    assert!(!summary.contains(&format!("Alpha-{} (", CAP_INVENTORY + 1)));
}

#[test]
fn outdated_reports_the_omitted_count_before_the_upgrade_hint() {
    let packages: Vec<_> = (1..=CAP_LIST + 3)
        .map(|index| {
            serde_json::json!({
                "name": format!("demo-{index}"),
                "version": "1.0",
                "latest_version": "2.0"
            })
        })
        .collect();

    let summary = filter_pip_outdated(&serde_json::to_string(&packages).unwrap());

    assert!(summary.starts_with(&format!(
        "pip outdated: {} packages\n1. demo-1 (1.0 → 2.0)\n",
        packages.len()
    )));
    assert_eq!(
        summary
            .lines()
            .filter(|line| line.contains(". demo-"))
            .count(),
        CAP_LIST
    );
    assert!(
        summary.ends_with(&format!(
            "{CAP_LIST}. demo-{CAP_LIST} (1.0 → 2.0)\n\n\
             ... +3 more packages\n\n\
             [hint] Run `pip install --upgrade <package>` to update"
        )),
        "{summary}"
    );
    assert!(!summary.contains(&format!("demo-{} (", CAP_LIST + 1)));
}

#[cfg(unix)]
#[test]
fn uv_fallback_filters_queries_and_preserves_passthrough_streams() {
    use crate::core::test_isolation::{rtk_binary_is_built, rtk_command, tempdir};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    if !rtk_binary_is_built() {
        return;
    }

    let tools = tempdir();
    let uv = tools.path().join("uv");
    fs::write(
        &uv,
        r#"#!/bin/sh
case "$#:$*" in
    '4:pip list --format=json --local'|'5:pip list --outdated --format=json --user')
        printf '%s\n' '[
  {"name": "Demo", "version": "1.0", "latest_version": "2.0", "latest_filetype": "wheel"},
  {"name": "demo-extra", "version": "1.1", "latest_version": "2.1", "latest_filetype": "wheel"}
]'
        ;;
    '3:pip show demo package')
        printf 'fixture name: %s\n' "$3"
        printf '%s\n' 'fixture show diagnostic' >&2
        ;;
    '3:pip freeze --all')
        printf '%s\n' 'demo==1.0'
        printf '%s\n' 'fixture freeze diagnostic' >&2
        ;;
    *)
        printf 'unexpected fake uv arguments: %s\n' "$*" >&2
        false
        ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&uv, fs::Permissions::from_mode(0o700)).unwrap();

    let cases = [
        (
            ["list", "--local"],
            "pip list: 2 packages\n\n[D]\n  Demo (1.0)\n  demo-extra (1.1)",
            "Running: uv pip list --format=json",
            None,
        ),
        (
            ["outdated", "--user"],
            "pip outdated: 2 packages\n1. Demo (1.0 → 2.0)\n2. demo-extra (1.1 → 2.1)\n\n\
             [hint] Run `pip install --upgrade <package>` to update",
            "Running: uv pip list --outdated --format=json",
            None,
        ),
        (
            ["show", "demo package"],
            "fixture name: demo package",
            "Running: uv pip show demo package",
            Some("fixture show diagnostic"),
        ),
        (
            ["freeze", "--all"],
            "demo==1.0",
            "Running: uv pip freeze --all",
            Some("fixture freeze diagnostic"),
        ),
    ];

    for (args, expected_stdout, trace, diagnostic) in cases {
        let mut cmd = rtk_command();
        let inherited_path = cmd
            .get_envs()
            .find(|(key, _)| *key == "PATH")
            .and_then(|(_, value)| value)
            .expect("isolated rtk command should provide PATH");
        // Keep the inherited absolute tool directories that cannot supply pip.
        let path = std::env::join_paths(
            std::iter::once(tools.path().to_path_buf()).chain(
                std::env::split_paths(inherited_path)
                    .filter(|entry| entry.is_absolute() && !entry.join("pip").exists()),
            ),
        )
        .unwrap();

        let output = cmd
            .env("PATH", path)
            .args(["-v", "pip"])
            .args(args)
            .output()
            .unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(output.status.code(), Some(0), "{args:?}: {stderr}");
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert_eq!(stdout.trim_end(), expected_stdout, "{args:?}");
        assert!(
            stderr
                .lines()
                .any(|line| line == "pip not found — falling back to `uv pip`"),
            "{args:?}: {stderr}"
        );
        assert!(
            stderr.lines().any(|line| line == trace),
            "{args:?}: {stderr}"
        );
        if let Some(diagnostic) = diagnostic {
            assert!(
                stderr.lines().any(|line| line == diagnostic),
                "{args:?}: {stderr}"
            );
        }
    }
}
