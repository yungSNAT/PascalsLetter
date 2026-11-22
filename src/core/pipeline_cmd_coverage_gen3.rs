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
fn bench_classifies_inputs_by_name_and_reports_success() {
    let root = crate::core::test_isolation::tempdir();
    crate::core::test_isolation::with_root(root.path(), || {
        let json = "{\"items\": [1, 2, 3, 4, 5], \"name\": \"demo\", \"nested\": {\"a\": 1}}\n";
        let html = "<html><head><title>t</title></head><body><p>hello world</p></body></html>\n";
        let text = "line one\nline two\nline two\nline three\n";
        let cases: [(&str, &str); 7] = [
            ("package-lock.json", json),
            (
                "Cargo.lock",
                "[[package]]\nname = \"demo\"\nversion = \"0.1.0\"\n",
            ),
            ("yarn.lock", "demo@1:\n  version \"1\"\n"),
            ("data.json", json),
            ("page.html", html),
            ("page.htm", html),
            ("output.log", text),
        ];
        for (name, content) in cases {
            let path = root.path().join(name);
            std::fs::write(&path, content).unwrap();
            assert_eq!(run_bench(&path).unwrap(), 0, "{name}");
        }

        let missing = root.path().join("missing-input.txt");
        let err = run_bench(&missing).unwrap_err().to_string();
        assert!(err.contains("Failed to read"), "{err}");
    });
}
