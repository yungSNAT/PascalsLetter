use super::*;

#[test]
fn list_json_skips_versionless_roots_and_keeps_transitive_dev_dependencies() {
    let input = r#"[
        {
            "name": "workspace",
            "dependencies": {"runtime": {"version": "1.2.3"}},
            "devDependencies": {
                "compiler": {
                    "version": "4.0.0",
                    "dependencies": {"helper": {"version": "2.0.0"}}
                }
            }
        }
    ]"#;

    let ParseResult::Full(state) = PnpmListParser::parse(input) else {
        panic!("expected a full parse of the workspace tree");
    };
    assert_eq!(state.total_packages, 3);
    assert_eq!(state.outdated_count, 0);
    assert_eq!(
        format_dependency_listing(&state, false),
        "3 packages (1 prod / 2 dev)\n[prod]\n  runtime 1.2.3\n[dev]\n  compiler 4.0.0\n  helper 2.0.0"
    );
}

#[test]
fn list_text_recovers_scoped_packages_but_passes_through_unrecognized_output() {
    let input = "\
Legend: production and development dependencies
hidden@9.9.9 │ nested row
bare-name
@missing-name
missing-version@

devDependencies:
@tools/check@2.1.0 optional-column
dependencies:
runtime@1.4.0
";
    let ParseResult::Degraded(state, warnings) = PnpmListParser::parse(input) else {
        panic!("expected text extraction after the JSON parse failed");
    };
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].starts_with("JSON parse failed:"));
    assert_eq!(state.total_packages, 2);
    assert_eq!(
        format_dependency_listing(&state, false),
        "2 packages (1 prod / 1 dev)\n[prod]\n  runtime 1.4.0\n[dev]\n  @tools/check 2.1.0"
    );

    let raw = "Legend: no dependency rows\nbare-name\nmissing-version@";
    let ParseResult::Passthrough(output) = PnpmListParser::parse(raw) else {
        panic!("metadata and incomplete package names must not produce a listing");
    };
    assert_eq!(output, raw);
}

#[test]
fn outdated_parsers_count_only_packages_whose_latest_version_differs() {
    let json = r#"{
        "stable": {"current": "3.0.0", "latest": "3.0.0"},
        "behind": {
            "current": "1.0.0",
            "wanted": "1.1.0",
            "latest": "2.0.0",
            "dependencyType": "devDependencies"
        }
    }"#;
    let text = "\
Legend: version columns
Package Current Wanted Latest
stable 3.0.0 3.0.0 3.0.0
ignored 1.0.0 1.0.0 9.0.0 │
incomplete 1.0.0 1.0.0

behind 1.0.0 1.1.0 2.0.0
";
    let ParseResult::Full(json_state) = PnpmOutdatedParser::parse(json) else {
        panic!("expected a full parse of outdated JSON");
    };
    let ParseResult::Degraded(text_state, warnings) = PnpmOutdatedParser::parse(text) else {
        panic!("expected a degraded parse of the text table");
    };
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].starts_with("JSON parse failed:"));

    for (state, expect_dev) in [(json_state, true), (text_state, false)] {
        assert_eq!(state.total_packages, 2);
        assert_eq!(state.outdated_count, 1);
        assert_eq!(state.dependencies.len(), 2);
        let stable = state
            .dependencies
            .iter()
            .find(|d| d.name == "stable")
            .unwrap();
        assert_eq!(stable.current_version, "3.0.0");
        assert_eq!(stable.latest_version.as_deref(), Some("3.0.0"));
        let behind = state
            .dependencies
            .iter()
            .find(|d| d.name == "behind")
            .unwrap();
        assert_eq!(behind.current_version, "1.0.0");
        assert_eq!(behind.wanted_version.as_deref(), Some("1.1.0"));
        assert_eq!(behind.latest_version.as_deref(), Some("2.0.0"));
        assert_eq!(behind.dev_dependency, expect_dev);
    }
}

#[test]
fn listing_caps_each_group_independently_and_adds_a_hint_for_each() {
    let count = MAX_LISTING + 2;
    let dependencies = [("runtime", false), ("tool", true)]
        .into_iter()
        .flat_map(|(prefix, dev_dependency)| {
            (0..count).map(move |index| Dependency {
                name: format!("{prefix}-{index}"),
                current_version: "1.0.0".to_string(),
                latest_version: None,
                wanted_version: None,
                dev_dependency,
            })
        })
        .collect();
    let state = DependencyState {
        total_packages: count * 2,
        outdated_count: 0,
        dependencies,
    };

    let root = crate::core::test_isolation::tempdir();
    let tee_dir = root.path().join("tee");
    std::fs::create_dir(&tee_dir).unwrap();
    let capped = crate::core::test_isolation::with_root(root.path(), || {
        crate::core::user_env::with_vars(&[("RTK_TEE", Some("1"))], || {
            crate::core::user_env::with_path("RTK_TEE_DIR", Some(&tee_dir), || {
                format_dependency_listing(&state, true)
            })
        })
    });

    let header = format!("{} packages ({count} prod / {count} dev)", count * 2);
    let rows = |prefix: &str, n| {
        (0..n)
            .map(|i| format!("  {prefix}-{i} 1.0.0"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let prod_prefix = format!(
        "{header}\n[prod]\n{}\n  … +2 more\n",
        rows("runtime", MAX_LISTING)
    );
    let (prod_hint, dev_section) = capped
        .strip_prefix(&prod_prefix)
        .expect("the production section should be capped")
        .split_once("\n[dev]\n")
        .expect("both sections should be present");
    let dev_prefix = format!("{}\n  … +2 more\n", rows("tool", MAX_LISTING));
    let dev_hint = dev_section
        .strip_prefix(&dev_prefix)
        .expect("the development section should be capped");
    for hint in [prod_hint, dev_hint] {
        assert!(
            hint.starts_with("  ") && !hint.trim().is_empty(),
            "{capped}"
        );
        assert_eq!(hint.lines().count(), 1, "{capped}");
    }
    assert_ne!(prod_hint, dev_hint);

    assert_eq!(
        format_dependency_listing(&state, false),
        format!(
            "{header}\n[prod]\n{}\n[dev]\n{}",
            rows("runtime", count),
            rows("tool", count)
        )
    );
}

#[test]
fn install_filter_keeps_changes_summaries_and_errors_but_discards_progress() {
    let log = "\
Lockfile is up to date
Progress: resolved 12, reused 10
+ downloading archive 40%
- unpacking │ working

  dependencies:
+ runtime 1.2.3
- old-runtime 1.0.0
2 packages in 120ms
  ERR_PNPM_FETCH: fixture failure
  error: retry unavailable
ERROR: lockfile conflict
Done
";
    assert_eq!(
        filter_pnpm_install(log),
        "dependencies:\n+ runtime 1.2.3\n- old-runtime 1.0.0\n2 packages in 120ms\n  ERR_PNPM_FETCH: fixture failure\n  error: retry unavailable\nERROR: lockfile conflict"
    );
    assert_eq!(filter_pnpm_install("Progress: resolved 1\n\nDone\n"), "ok");
}

#[cfg(all(unix, feature = "engine"))]
mod cli {
    use crate::core::test_isolation::{rtk_binary_is_built, rtk_command, tempdir};
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::process::Stdio;

    fn fake_pnpm(body: &str) -> tempfile::TempDir {
        let dir = tempdir();
        let tool = dir.path().join("pnpm");
        std::fs::write(&tool, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(tool, std::fs::Permissions::from_mode(0o700)).unwrap();
        dir
    }

    fn invoke(dir: &Path, args: &[&str], fixture: &str) -> (i32, String, String) {
        let mut cmd = rtk_command();
        let inherited_path = cmd
            .get_envs()
            .find(|(key, _)| *key == "PATH")
            .and_then(|(_, value)| value)
            .unwrap_or_default();
        let path = std::env::join_paths(
            std::iter::once(dir.to_path_buf()).chain(std::env::split_paths(inherited_path)),
        )
        .unwrap();
        let output = cmd
            .env("PATH", path)
            .env("PNPM_FIXTURE", fixture)
            .current_dir(dir)
            .stdin(Stdio::null())
            .args(["-v", "pnpm"])
            .args(args)
            .output()
            .unwrap();
        (
            output.status.code().expect("rtk should exit normally"),
            String::from_utf8(output.stdout).unwrap(),
            String::from_utf8(output.stderr).unwrap(),
        )
    }

    #[test]
    fn list_dispatches_all_parse_tiers_and_preserves_tool_failure_status() {
        if !rtk_binary_is_built() {
            return;
        }
        let dir = fake_pnpm(
            r#"
[ "$#" -eq 4 ] && [ "$1" = list ] && [ "$3" = --json ] && [ "$4" = --prod ] || exit 91
case "$2" in --depth=[0-9]*) ;; *) exit 92 ;; esac
case "$PNPM_FIXTURE" in
    json)
        printf '[{"name":"library","version":"1.2.3","description":"%256s"}]\n' 'fixture metadata'
        ;;
    text)
        printf 'Legend: %256s\ndependencies:\nlibrary@1.2.3\n' 'fixture metadata'
        ;;
    raw)
        printf '%s\n' 'unrecognized-list-output'
        ;;
    failure)
        printf '%s\n' 'library@1.2.3'
        printf '%s\n' 'fixture-list-failure' >&2
        exit 17
        ;;
    *) exit 93 ;;
esac
"#,
        );
        let args = ["list", "--prod"];
        let expected = "1 packages (1 prod / 0 dev)\n[prod]\n  library 1.2.3";
        for (fixture, diagnostic) in [
            ("json", "pnpm list (Tier 1: Full JSON parse)"),
            ("text", "JSON parse failed:"),
        ] {
            let (code, stdout, stderr) = invoke(dir.path(), &args, fixture);
            assert_eq!(code, 0, "{fixture}: {stdout}\n{stderr}");
            assert_eq!(stdout.trim_end(), expected);
            assert!(stderr.contains(diagnostic), "{stderr}");
        }

        let (code, stdout, stderr) = invoke(dir.path(), &args, "raw");
        assert_eq!(code, 0, "{stderr}");
        assert_eq!(stdout.trim(), "unrecognized-list-output");

        let (code, stdout, stderr) = invoke(dir.path(), &args, "failure");
        assert_eq!(code, 17, "{stderr}");
        assert!(stdout.is_empty(), "{stdout}");
        assert!(stderr.contains("fixture-list-failure"), "{stderr}");
    }

    #[test]
    fn outdated_compacts_recognized_output_and_preserves_unrecognized_output() {
        if !rtk_binary_is_built() {
            return;
        }
        let dir = fake_pnpm(
            r#"
[ "$#" -eq 3 ] && [ "$1" = outdated ] && [ "$2" = --format ] && [ "$3" = json ] || exit 91
case "$PNPM_FIXTURE" in
    json)
        printf '{"library":{"current":"1.0.0","wanted":"1.1.0","latest":"2.0.0","description":"%256s"}}\n' 'fixture metadata'
        exit 1
        ;;
    text)
        printf 'Legend: %256s\nPackage Current Wanted Latest\nlibrary 1.0.0 1.1.0 2.0.0\n' 'fixture metadata'
        ;;
    raw) printf '%s\n' 'unrecognized-outdated-output' ;;
    empty) : ;;
    *) exit 92 ;;
esac
"#,
        );
        for (fixture, expected_code, diagnostic) in [
            ("json", 1, "pnpm outdated (Tier 1: Full JSON parse)"),
            ("text", 0, "JSON parse failed:"),
        ] {
            let (code, stdout, stderr) = invoke(dir.path(), &["outdated"], fixture);
            assert_eq!(code, expected_code, "{fixture}: {stdout}\n{stderr}");
            assert!(stdout.contains("library"), "{stdout}");
            assert!(stdout.contains("2.0.0"), "{stdout}");
            assert!(!stdout.contains("fixture metadata"), "{stdout}");
            assert!(stderr.contains(diagnostic), "{stderr}");
        }

        let (code, stdout, stderr) = invoke(dir.path(), &["outdated"], "raw");
        assert_eq!(code, 0, "{stderr}");
        assert_eq!(stdout.trim(), "unrecognized-outdated-output");

        let (code, stdout, stderr) = invoke(dir.path(), &["outdated"], "empty");
        assert_eq!(code, 0, "{stderr}");
        assert_eq!(
            stdout.trim_end(),
            super::never_worse("", "All packages up-to-date")
        );
    }

    #[test]
    fn install_filters_success_but_forwards_failure_and_leaves_other_commands_raw() {
        if !rtk_binary_is_built() {
            return;
        }
        let dir = fake_pnpm(
            r#"
if [ "$1" = store ]; then
    [ "$#" -eq 2 ] && [ "$2" = path ] || exit 91
    printf '%s\n' 'fixture-store-path'
    exit 0
fi
[ "$#" -eq 2 ] && [ "$1" = install ] && [ "$2" = --frozen-lockfile ] || exit 92
case "$PNPM_FIXTURE" in
    success)
        printf '%s\n' 'Progress: resolved 80, reused 79, downloaded 1, added 1' '1 packages in 120ms'
        printf '%s\n' 'error: fixture diagnostic from stderr' >&2
        ;;
    failure)
        printf '%s\n' '1 packages in 120ms'
        printf '%s\n' 'fixture-install-failure' >&2
        exit 19
        ;;
    *) exit 93 ;;
esac
"#,
        );
        let args = ["install", "--frozen-lockfile"];
        let (code, stdout, stderr) = invoke(dir.path(), &args, "success");
        assert_eq!(code, 0, "{stderr}");
        assert!(
            stdout.lines().any(|line| line == "1 packages in 120ms"),
            "{stdout}"
        );
        assert!(
            stdout.contains("fixture diagnostic from stderr"),
            "{stdout}"
        );
        assert!(!stdout.contains("Progress:"), "{stdout}");
        assert!(stderr.contains("pnpm install running..."), "{stderr}");

        let (code, stdout, stderr) = invoke(dir.path(), &args, "failure");
        assert_eq!(code, 19, "{stderr}");
        assert!(stdout.is_empty(), "{stdout}");
        assert!(stderr.contains("fixture-install-failure"), "{stderr}");

        let (code, stdout, stderr) = invoke(dir.path(), &["store", "path"], "");
        assert_eq!(code, 0, "{stderr}");
        assert_eq!(stdout.trim(), "fixture-store-path");
    }
}
