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

    // The failure list is pinned only at its ends: today it lists test_total
    // twice, once from the traceback and once from the short summary.
    let output = filter_pytest_output(raw);
    assert!(
        output.starts_with(
            r#"Pytest: 0 passed, 1 failed, 1 skipped, 1 xpassed, 1 error in 0.04s

Expected-failure outcomes:
  XPASS test_invoice.py::test_rounding - fixed upstream

Failures:
1. [FAIL] test_total
     E   AssertionError: totals differ
"#
        ),
        "{output}"
    );
    assert!(
        output.ends_with(
            "[ERROR] test_invoice.py::test_fixture\n     RuntimeError: fixture unavailable"
        ),
        "{output}"
    );
    assert!(!output.contains("RuntimeWarning"), "{output}");
    assert!(!output.contains("SKIPPED"), "{output}");
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
