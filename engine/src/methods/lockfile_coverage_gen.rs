use super::*;

#[test]
fn npm_summarizes_only_versioned_entries_from_the_selected_map() {
    let legacy = r#"{
  "lockfileVersion": 1,
  "dependencies": {
    "@scope/legacy": { "version": "1.2.3" },
    "missing-version": {},
    "numeric-version": { "version": 42 }
  }
}"#;
    assert_eq!(
        detect_and_summarize(legacy).as_deref(),
        Some("package-lock.json: 1 packages\n@scope/legacy@1.2.3")
    );

    let modern = r#"{
  "lockfileVersion": 3,
  "packages": {
    "": { "name": "app", "version": "9.0.0" },
    "node_modules/local": { "resolved": "packages/local", "link": true },
    "node_modules/installed": { "version": "2.3.4" }
  },
  "dependencies": {
    "legacy-only": { "version": "5.0.0" }
  }
}"#;
    assert_eq!(
        detect_and_summarize(modern).as_deref(),
        Some("package-lock.json: 1 packages\ninstalled@2.3.4")
    );
}

#[test]
fn cargo_does_not_reuse_names_across_records_or_after_malformed_versions() {
    let input = r#"[[package]]
name = "missing-version"

[[package]]
version = "1.0.0"

[[package]]
name = "malformed"
version = "2.0.0
version = "2.0.1"

[[package]]
name = "complete"
version = "3.0.0"
"#;
    assert_eq!(
        detect_and_summarize(input).as_deref(),
        Some("Cargo.lock: 1 packages\ncomplete@3.0.0")
    );
}

#[test]
fn pnpm_ignores_package_shaped_keys_after_the_packages_section() {
    let input = "\
lockfileVersion: '9.0'
packages:
  first@1.0.0:

  second@2.0.0:

snapshots:
  snapshot-only@3.0.0:
";
    assert_eq!(
        detect_and_summarize(input).as_deref(),
        Some("pnpm-lock.yaml: 2 packages\nfirst@1.0.0\nsecond@2.0.0")
    );
}

#[test]
fn yarn_does_not_reuse_a_selector_after_a_malformed_version() {
    let input = r#"# yarn lockfile v1

broken@^1.0.0:
  version "1.0.0
  version "1.0.1"

"@scope/complete@^2.0.0":
  version "2.3.4"
"#;
    assert_eq!(
        detect_and_summarize(input).as_deref(),
        Some("yarn.lock: 1 packages\n@scope/complete@2.3.4")
    );
}
