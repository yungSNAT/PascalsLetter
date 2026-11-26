use super::{CAP_LIST, CAP_WARNINGS, filter_cargo_clippy};
use crate::core::{test_isolation, user_env};
use std::fmt::Write as _;
use std::path::Path;

fn filter_with_tee(input: &str, root: &Path) -> String {
    let _cwd = test_isolation::enter(root);
    test_isolation::with_root(root, || {
        user_env::with_path("RTK_TEE_DIR", Some(root), || {
            user_env::with_vars(&[("RTK_TEE", Some("1"))], || filter_cargo_clippy(input))
        })
    })
}

fn assert_recall_hint(summary: &str, prefix: &str) {
    let id = summary
        .lines()
        .find_map(|line| match prefix.strip_prefix("  [+") {
            Some(hidden) => hidden_hint_id(line.strip_prefix("  ")?, hidden.parse().ok()?),
            None => line.strip_prefix(prefix)?.strip_suffix(']'),
        })
        .unwrap_or_else(|| panic!("missing recall hint starting with {prefix:?}: {summary}"));
    assert_eq!(id.len(), 16, "invalid recall ID: {id:?}");
    assert!(
        id.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "invalid recall ID: {id:?}"
    );
}

#[test]
fn clippy_caps_error_blocks_and_offers_full_output_recall() {
    let root = test_isolation::tempdir();
    let total = CAP_WARNINGS + 2;
    let blocks: Vec<_> = (1..=total)
        .map(|i| {
            format!("error[E0308]: mismatched types in binding {i}\n   = note: expected an integer")
        })
        .collect();
    let input = blocks.join("\n\n");

    let result = filter_with_tee(&input, root.path());

    assert!(result.starts_with(&format!("cargo clippy: {total} errors, 0 warnings\n")));
    let shown: Vec<_> = result
        .lines()
        .filter_map(|line| line.strip_prefix("  error[E0308]: mismatched types in binding "))
        .collect();
    let expected: Vec<_> = (1..=CAP_WARNINGS).map(|i| i.to_string()).collect();
    assert_eq!(shown, expected, "{result}");
    assert!(result.lines().any(|line| line == "  … +2 more errors"));
    assert_recall_hint(&result, "  [full output: rtk recall ");
}

#[test]
fn clippy_caps_locations_and_rules_and_offers_hidden_rule_recall() {
    let root = test_isolation::tempdir();
    let other_rules = CAP_LIST + 1;
    let mut input = String::new();
    for i in 1..=5 {
        writeln!(
            input,
            "warning: repeated issue [clippy::repeat_lint]\n --> repeated.rs:{i}:9"
        )
        .unwrap();
    }
    for i in 1..=other_rules {
        writeln!(
            input,
            "warning: another issue [clippy::sample_lint_{i}]\n --> other.rs:{i}:9"
        )
        .unwrap();
    }

    let result = filter_with_tee(&input, root.path());

    let warnings = 5 + other_rules;
    assert!(result.starts_with(&format!("cargo clippy: 0 errors, {warnings} warnings\n")));
    assert!(
        result.contains(concat!(
            "  clippy::repeat_lint (5x)\n",
            "    repeated.rs:1:9\n",
            "    repeated.rs:2:9\n",
            "    repeated.rs:3:9\n",
            "    … +2 more\n",
        )),
        "{result}"
    );
    assert!(!result.contains("repeated.rs:4:9"), "{result}");
    assert!(!result.contains("repeated.rs:5:9"), "{result}");
    assert_eq!(
        result
            .lines()
            .filter(|line| line.starts_with("  clippy::sample_lint_"))
            .count(),
        CAP_LIST - 1,
        "{result}"
    );
    assert!(result.lines().any(|line| line == "… +2 more rules"));
    assert_recall_hint(&result, "  [+2");
}

#[cfg(unix)]
#[test]
fn cargo_passthrough_preserves_arguments_and_child_output() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Stdio;

    if !test_isolation::rtk_binary_ready() {
        return;
    }

    let tools = test_isolation::tempdir();
    let fake_cargo = tools.path().join("cargo");
    fs::write(
        &fake_cargo,
        "#!/bin/sh\nprintf '<%s>\\n' \"$@\"\nprintf '%s\\n' passthrough-stderr >&2\n",
    )
    .unwrap();
    fs::set_permissions(&fake_cargo, fs::Permissions::from_mode(0o700)).unwrap();

    let mut cmd = test_isolation::rtk_command();
    let inherited_path = cmd
        .get_envs()
        .find(|(key, _)| *key == "PATH")
        .and_then(|(_, value)| value);
    let path = std::env::join_paths(
        std::iter::once(tools.path().to_path_buf())
            .chain(inherited_path.into_iter().flat_map(std::env::split_paths)),
    )
    .unwrap();
    let output = cmd
        .env("PATH", path)
        .current_dir(tools.path())
        .stdin(Stdio::null())
        .args([
            "cargo",
            "metadata",
            "--offline",
            "--no-deps",
            "--manifest-path",
            "example workspace.toml",
        ])
        .output()
        .expect("run isolated rtk cargo passthrough");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "<metadata>\n<--offline>\n<--no-deps>\n<--manifest-path>\n<example workspace.toml>\n"
    );
    assert!(
        stderr.lines().any(|line| line == "passthrough-stderr"),
        "{stderr}"
    );
}

/// The recall ID in a `[+N hidden (lines A-B): rtk recall <id>]` tail hint,
/// provided the range covers exactly `hidden` lines.
fn hidden_hint_id(line: &str, hidden: usize) -> Option<&str> {
    let rest = line.strip_prefix(&format!("[+{hidden} hidden (lines "))?;
    let (range, id) = rest.split_once("): rtk recall ")?;
    let (first, last) = range.split_once('-')?;
    let (first, last): (usize, usize) = (first.parse().ok()?, last.parse().ok()?);
    (first >= 1 && last + 1 == first + hidden).then_some(id.strip_suffix(']')?)
}
