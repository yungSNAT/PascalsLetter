use super::filter_phpt_output;

#[test]
fn scanned_warnings_and_leaks_keep_only_unexpected_leak_details() {
    let input = concat!(
        "PHP_VERSION : 8.4.20\n",
        "WARN Deprecated setting [warning.phpt]\n",
        "XLEAK Known allocation leak [known.phpt]\n",
        "========DIFF========\n",
        "001- no outstanding allocations\n",
        "001+ one outstanding allocation\n",
        "========DONE========\n",
        "LEAK Allocation survives cleanup [leak.phpt]\n",
    );

    assert_eq!(
        filter_phpt_output(input),
        concat!(
            "phpt: 0 passed, 1 warned, 1 leaked, 1 xleaked  (3 total)\n",
            "PHP 8.4.20\n",
            "\n",
            "FAILURES (1):\n",
            "  leak.phpt -- Allocation survives cleanup\n",
            "    001- no outstanding allocations\n",
            "    001+ one outstanding allocation",
        )
    );
}
