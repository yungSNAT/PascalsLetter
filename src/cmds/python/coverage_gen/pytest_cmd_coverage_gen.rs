use super::*;
use crate::core::{test_isolation, user_env};

#[test]
fn warnings_end_tracebacks_and_short_summary_keeps_only_failure_outcomes() {
    let raw = r#"=== ERRORS ===
=== warnings summary ===
test_invoice.py:3: RuntimeWarning: before the failure
=== FAILURES ===
___ test_total ___
E   AssertionError: totals differ
=== warnings summary ===
test_invoice.py:99: RuntimeWarning: after the failure
=== short test summary info ===
SKIPPED [1] test_invoice.py:4: optional dependency
FAILED test_invoice.py::test_total
ERROR test_invoice.py::test_fixture - RuntimeError: fixture unavailable
XPASS test_invoice.py::test_rounding - fixed upstream
=== 1 failed, 1 skipped, 1 xpassed, 1 error in 0.04s ==="#;

    assert_eq!(
        filter_pytest_output(raw),
        r#"Pytest: 0 passed, 1 failed, 1 skipped, 1 xpassed, 1 error in 0.04s

Expected-failure outcomes:
  XPASS test_invoice.py::test_rounding - fixed upstream

Failures:
1. [FAIL] test_total
     E   AssertionError: totals differ

2. [FAIL] test_invoice.py::test_total
3. [ERROR] test_invoice.py::test_fixture
     RuntimeError: fixture unavailable"#
    );
}

#[test]
fn overflow_caps_both_sections_and_emits_distinct_recall_hints() {
    let root = test_isolation::tempdir();
    let failure_count = MAX_PYTEST_FAILURES + 2;
    let xfail_count = MAX_XFAIL + 1;
    let failures: Vec<_> = (0..failure_count)
        .map(|i| format!("FAILED test_many.py::test_failure_{i:03} - mismatch {i:03}"))
        .collect();
    let xfails: Vec<_> = (0..xfail_count)
        .map(|i| format!("XFAIL test_many.py::test_xfail_{i:03} - known issue {i:03}"))
        .collect();
    let raw = format!(
        "=== short test summary info ===\n{}\n{}\n\
         === {failure_count} failed, {xfail_count} xfailed in 0.03s ===",
        failures.join("\n"),
        xfails.join("\n")
    );

    let result = test_isolation::with_root(root.path(), || {
        user_env::with_path("RTK_TEE_DIR", Some(root.path()), || {
            user_env::with_vars(&[("RTK_TEE", Some("1"))], || filter_pytest_output(&raw))
        })
    });

    for (entries, cap) in [(&failures, MAX_PYTEST_FAILURES), (&xfails, MAX_XFAIL)] {
        for (i, entry) in entries.iter().enumerate() {
            let (_, reason) = entry.split_once(" - ").unwrap();
            assert_eq!(result.contains(reason), i < cap, "{entry}: {result}");
        }
    }

    let [xfail_id, failure_id] = [
        ("  … +1 more", "  [+1 hidden: rtk recall "),
        ("… +2 more failures", "  [full output: rtk recall "),
    ]
    .map(|(overflow, prefix)| {
        let id = result
            .lines()
            .skip_while(|line| *line != overflow)
            .nth(1)
            .and_then(|line| line.strip_prefix(prefix))
            .and_then(|line| line.strip_suffix(']'))
            .unwrap_or_else(|| panic!("missing recall hint after {overflow:?}: {result}"));
        assert!(
            !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "invalid recall ID: {id:?}"
        );
        id
    });
    assert_ne!(xfail_id, failure_id, "{result}");
}

#[cfg(unix)]
#[test]
fn wrapper_forwards_arguments_and_distinguishes_startup_failure_from_no_tests() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Stdio;

    if !test_isolation::rtk_binary_is_built() {
        return;
    }

    let root = test_isolation::tempdir();
    let pytest = root.path().join("pytest");
    let python = root.path().join("python");
    fs::write(
        &pytest,
        r#"#!/bin/sh
[ "$#" -eq 4 ] && [ "$1" = --tb=short ] && [ "$2" = -q ] && [ "$3" = -rxX ] || exit 97
case "$4" in
    'test passing.py') printf '%s\n' '2 passed in 0.04s' ;;
    'test broken.py') printf '  \033[31msynthetic startup failure\033[0m  \n'; exit 4 ;;
    'test empty.py') printf '%s\n' '=== no tests ran in 0.00s ==='; exit 5 ;;
    *) exit 98 ;;
esac
"#,
    )
    .unwrap();
    fs::write(
        &python,
        r#"#!/bin/sh
[ "$#" -eq 6 ] && [ "$1" = -m ] && [ "$2" = pytest ] &&
    [ "$3" = --tb=short ] && [ "$4" = -q ] && [ "$5" = -rxX ] &&
    [ "$6" = 'test fallback.py' ] || exit 97
printf '%s\n' '3 passed in 0.05s'
"#,
    )
    .unwrap();
    for tool in [&pytest, &python] {
        fs::set_permissions(tool, fs::Permissions::from_mode(0o755)).unwrap();
    }

    for (arg, code, expected) in [
        ("test passing.py", 0, "Pytest: 2 passed in 0.04s"),
        ("test broken.py", 4, "synthetic startup failure"),
        ("test empty.py", 5, PYTEST_NO_TESTS),
        ("test fallback.py", 0, "Pytest: 3 passed in 0.05s"),
    ] {
        let fallback = arg == "test fallback.py";
        if fallback {
            fs::remove_file(&pytest).unwrap();
        }

        let mut cmd = test_isolation::rtk_command();
        let inherited_path = cmd
            .get_envs()
            .find(|(key, _)| *key == "PATH")
            .and_then(|(_, value)| value)
            .expect("isolated command supplies PATH");
        // For the fallback case, don't let an installed pytest mask the fake Python.
        let inherited_dirs = std::env::split_paths(inherited_path)
            .filter(|dir| !fallback || !root.path().join(dir).join("pytest").exists());
        let path =
            std::env::join_paths(std::iter::once(root.path().to_path_buf()).chain(inherited_dirs))
                .unwrap();

        let output = cmd
            .current_dir(root.path())
            .env("PATH", path)
            .env("RTK_TEE", "0")
            .stdin(Stdio::null())
            .args(["-v", "pytest", arg])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(code), "{arg}: {output:?}");

        let stdout = String::from_utf8(output.stdout).unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(
            stdout.lines().any(|line| line == expected),
            "{arg}: {stdout:?}"
        );
        assert_eq!(
            stdout.contains(PYTEST_NO_TESTS),
            code == 5,
            "{arg}: {stdout:?}"
        );
        assert!(
            stderr.contains(&format!("Running: pytest --tb=short -q -rxX {arg}")),
            "{arg}: {stderr:?}"
        );
    }
}
