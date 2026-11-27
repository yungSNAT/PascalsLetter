use super::{CAP_WARNINGS, filter_next_build};

#[test]
fn build_summary_counts_diagnostics_and_marks_only_large_bundle_increases() {
    let result = filter_next_build(concat!(
        "Cache restored\n",
        "\x1b[32m○ /home 100 kB 110 kB\x1b[0m\n",
        "● /dashboard 100 kB 150 kB\n",
        "λ /empty 1 kB 0 kB\n",
        "WARNING: deprecated configuration\n",
        "Warning: missing metadata\n",
        "ERROR: failed to prerender an optional page\n",
        "Type check: 0 errors\n",
        "Compiled in 1250ms\n",
    ));

    let lines: Vec<_> = result
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect();

    assert_eq!(
        lines,
        [
            "Next.js Build",
            "3 routes (1 static, 1 dynamic)",
            "",
            "Bundles:",
            "/dashboard 150 kB [warn] (+50%)",
            "/home 110 kB",
            "/empty 0 kB",
            "",
            "Time: 1250ms | Errors: 1 | Warnings: 2",
        ]
    );
}

#[test]
fn cached_build_without_routes_omits_bundle_table_and_timing() {
    assert_eq!(
        filter_next_build("Build already optimized\n"),
        "Next.js Build\nAlready built (using cache)\n\nErrors: 0 | Warnings: 0"
    );
}

#[test]
fn bundle_limit_keeps_the_largest_routes_and_reports_the_omitted_count() {
    use std::fmt::Write as _;

    let mut input = String::new();
    let route_count = CAP_WARNINGS + 2;
    for size in 1..=route_count {
        writeln!(input, "○ /route-{size} {size} kB {size} kB").unwrap();
    }

    let result = filter_next_build(&input);
    let shown: Vec<usize> = result
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix("/route-"))
        .map(|row| row.split_whitespace().next().unwrap().parse().unwrap())
        .collect();

    assert_eq!(shown, (3..=route_count).rev().collect::<Vec<_>>());
    assert!(result.ends_with("\n\n  ... +2 more routes\n\nErrors: 0 | Warnings: 0"));
}
