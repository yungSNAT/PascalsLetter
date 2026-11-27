use super::*;

#[test]
fn explain_with_an_unknown_agent_lists_the_known_harnesses_and_fails() {
    let agent = "definitely-not-a-real-harness-zz";
    assert!(rtk_engine::harness::profile(agent).is_none());
    assert!(
        !rtk_engine::harness::HARNESSES.iter().any(|h| h.id == agent),
        "test agent name must not be a real harness"
    );
    let code = run_explain(agent, "Bash", None, None, None, false).unwrap();
    assert_eq!(code, 1);
}

#[test]
fn log_toggle_show_and_explain_last_without_entries() {
    let root = crate::core::test_isolation::tempdir();
    crate::core::test_isolation::with_root(root.path(), || {
        assert_eq!(run_log_toggle(true).unwrap(), 0);
        assert!(crate::core::pipeline_log::enabled());

        // A freshly enabled log has nothing recorded yet.
        assert!(crate::core::pipeline_log::read_last(10, None).is_empty());
        assert_eq!(run_log_show(10, None).unwrap(), 0);
        assert_eq!(run_log_show(10, Some("no-such-rule")).unwrap(), 0);

        // Nothing logged: explain-last reports failure.
        assert_eq!(run_explain_last().unwrap(), 1);

        assert_eq!(run_log_toggle(false).unwrap(), 0);
        assert!(!crate::core::pipeline_log::enabled());
        assert_eq!(run_log_show(3, None).unwrap(), 0);
        assert_eq!(run_explain_last().unwrap(), 1);
    });
}

#[test]
fn bench_reports_an_unreadable_input() {
    let root = crate::core::test_isolation::tempdir();
    crate::core::test_isolation::with_root(root.path(), || {
        let missing = root.path().join("missing-input.txt");
        let err = run_bench(&missing).unwrap_err().to_string();
        assert!(err.contains("Failed to read"), "{err}");
    });
}
