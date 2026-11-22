use super::filter_mypy_output;

#[test]
fn groups_delayed_notes_and_formats_errors_without_codes() {
    let input = concat!(
        "setup.py:2: note: Configuration context\n",
        "module.py:12:5: error: Types do not match\n",
        "Checking annotations\n",
        "module.py:12: note: Expected an integer\n",
        "other.py:6: note: Unrelated context\n",
        "module.py:18: error: Missing annotation  [no-untyped-def]\n",
        "Found 2 errors in 1 file\n",
    );

    assert_eq!(
        filter_mypy_output(input),
        concat!(
            "setup.py:2: note: Configuration context\n",
            "other.py:6: note: Unrelated context\n\n",
            "mypy: 2 errors in 1 files\n",
            "module.py (2 errors)\n",
            "  L12: Types do not match\n",
            "    Expected an integer\n",
            "  L18: [no-untyped-def] Missing annotation",
        ),
    );
}

#[test]
fn ignores_progress_but_retains_standalone_notes_without_errors() {
    assert_eq!(
        filter_mypy_output("Checking annotations\n\n"),
        "mypy: No issues found",
    );
    assert_eq!(
        filter_mypy_output("Checking annotations\nmodule.py:3: note: Inferred type is int\n"),
        "module.py:3: note: Inferred type is int",
    );
}
