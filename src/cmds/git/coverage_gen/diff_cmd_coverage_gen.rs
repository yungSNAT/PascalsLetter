use super::*;
use crate::core::test_isolation;

#[test]
fn verbose_file_comparison_keeps_diff_exit_codes() {
    let root = test_isolation::tempdir();
    test_isolation::with_root(root.path(), || {
        let _scope = test_isolation::enter(root.path());
        let before = root.path().join("before.txt");
        let after = root.path().join("after.txt");

        fs::write(&before, b"same\n").unwrap();
        fs::write(&after, b"same\n").unwrap();
        assert_eq!(run(&before, &after, 1).unwrap(), 0);

        fs::write(&after, b"same\r\n").unwrap();
        assert_eq!(run(&before, &after, 1).unwrap(), 1);
    });
}

#[test]
fn invisible_difference_diagnostic_does_not_invent_a_cause() {
    assert_eq!(
        describe_invisible_difference("old\n", "new\n"),
        "files differ only in whitespace or line endings (cause outside the line-ending and trailing-newline checks)"
    );
}

#[test]
fn stdin_guard_rejects_a_valid_condensation_that_grows() {
    let input = "Only in d: f\n";
    assert_eq!(
        condense_unified_diff_strict(input).as_deref(),
        Some("[file] d/f (only in one side)")
    );
    assert_eq!(condense_stdin(input.as_bytes()), None);
}

#[test]
fn classic_diff_skips_an_empty_hunk_between_aligned_edits() {
    let before = ["head", "OLD", "gap", "drop", "tail"];
    let after = ["head", "gap", "NEW", "more", "tail"];
    let Ok(Aligned::Script(ops)) = myers_ops(&before, &after) else {
        panic!("the small edit script should align");
    };

    let mut diff = ops_to_hunks(ops, 2);
    assert_eq!(diff.hunks.len(), 2);
    diff.hunks.insert(1, Hunk::at(5, 4));
    assert_eq!(
        format_classic_diff(&diff),
        "4d3\n< OLD\n6c5,6\n< drop\n---\n> NEW\n> more\n"
    );
}

#[test]
fn positional_hunks_close_at_a_match_or_at_eof() {
    let before = ["count = 1", "middle", "old", "tail"];
    let after = ["count = 2", "middle", "new", "tail"];
    for length in [3, 4] {
        let diff = positional_hunks(&before[..length], &after[..length], 7);
        assert!(diff.positional);
        assert_eq!(diff.hunks.len(), 2);
        assert_eq!((diff.added, diff.removed, diff.modified), (1, 1, 1));
        assert_eq!(
            format_diff_changes(&diff),
            "~   8 count = 1 → count = 2\n-  10 old\n+  10 new\n"
        );
    }
}

#[test]
fn backtrack_treats_a_predecessor_below_the_snapshot_as_zero() {
    // A synthetic round-zero snapshot starts one diagonal above the
    // predecessor. Rounding the offset into slot zero would lose a kept line.
    let trace = [2, 1, 1];
    let lines = ["first", "second"];
    let ops = myers_backtrack(&trace, 1, &lines, &lines);
    assert!(matches!(ops.as_slice(), [Op::Keep(2)]));
}

#[test]
fn only_in_delimiter_follows_the_entire_shell_quoted_directory() {
    let input = r#"Only in 'draft: '$'\303\251'\''s': "notes\q.txt""#;
    assert_eq!(
        condense_unified_diff_strict(input).as_deref(),
        Some(r"[file] draft: é's/notes\q.txt (only in one side)")
    );
    assert_eq!(unquote_shell("\"unfinished\\"), "unfinished\\");
}

#[test]
fn hunk_headers_require_a_matching_close_and_at_most_eight_parents() {
    let eight = "@@@@@@@@@ -1 -1 -1 -1 -1 -1 -1 -1 +3,2 @@@@@@@@@";
    let nine = "@@@@@@@@@@ -1 -1 -1 -1 -1 -1 -1 -1 -1 +3,2 @@@@@@@@@@";
    assert_eq!(parse_hunk_header(eight), Some((vec![1; 8], 2)));
    assert_eq!(parse_hunk_header(nine), None);

    let malformed = "--- a/f\n+++ b/f\n@@ -1 +1 @!\n-old\n+new\n";
    assert_eq!(condense_unified_diff_strict(malformed), None);

    let orphan = "--- a/f\n+++ b/f\n@@ -1 +1 @@\n-old\n+new\n--- orphaned removal\n";
    assert_eq!(condense_unified_diff_strict(orphan), None);
}

#[test]
fn incomplete_file_facts_are_prose_only_in_streams_that_allow_prose() {
    let patch = "--- before\n+++ after\n@@ -1 +1 @@\n-old\n+new\n";
    for near_miss in ["Only in this directory", "Submodule update instructions"] {
        let log = format!(
            "{near_miss}\n@@ -1 +1 @!\n---\n 1 file changed, 1 insertion(+), 1 deletion(-)\n{patch}"
        );
        assert_eq!(
            condense_unified_diff_strict(&log).as_deref(),
            Some("[file] after (+1 -1)\n-old\n+new")
        );

        let gnu = format!("diff -ru before after\n{near_miss}\n{patch}");
        assert_eq!(condense_unified_diff_strict(&gnu), None, "{near_miss}");
    }
}

#[test]
fn newline_markers_follow_emitted_changes_not_omitted_context() {
    let input = concat!(
        "--- a/f\n+++ b/f\n@@ -1,2 +1,2 @@\n",
        " context\n\\ marker after omitted context\n",
        "-old\n\\ old side has no newline\n",
        "+new\n\\ new side has no newline\n",
    );
    assert_eq!(
        condense_unified_diff_strict(input).as_deref(),
        Some(
            "[file] f (+1 -1)\n-old\n\\ old side has no newline\n+new\n\\ new side has no newline"
        )
    );
}
