use super::*;

#[test]
fn test_result_modes_show_skips_and_control_failure_detail() {
    let result = TestResult {
        total: 12,
        passed: 7,
        failed: 2,
        skipped: 3,
        duration_ms: Some(42),
        failures: vec![
            TestFailure {
                test_name: "rejects invalid input".to_string(),
                file_path: "validation.rs".to_string(),
                error_message: "expected an error".to_string(),
                stack_trace: Some("parse\nvalidate\nrun_case\nrunner".to_string()),
            },
            TestFailure {
                test_name: "keeps defaults".to_string(),
                file_path: "defaults.rs".to_string(),
                error_message: "missing default".to_string(),
                stack_trace: None,
            },
        ],
    };

    assert_eq!(
        result.format(FormatMode::from_verbosity(0)),
        concat!(
            "PASS (7) FAIL (2) skipped (3)\n\n",
            "1. rejects invalid input\n",
            "   expected an error\n",
            "2. keeps defaults\n",
            "   missing default\n\n",
            "Time: 42ms",
        ),
    );
    assert_eq!(
        result.format(FormatMode::from_verbosity(1)),
        concat!(
            "Tests: 7 passed, 2 failed, 3 skipped (total: 12)\n\n",
            "Failures:\n\n",
            "1. rejects invalid input (validation.rs)\n",
            "   expected an error\n",
            "   parse\n",
            "   validate\n",
            "   run_case\n\n",
            "2. keeps defaults (defaults.rs)\n",
            "   missing default\n\n",
            "Duration: 42ms",
        ),
    );
    let ultra = FormatMode::from_verbosity(2);
    assert_eq!(result.format(ultra), "[ok]7 [x]2 [skip]3 (42ms)");

    let untimed = TestResult {
        duration_ms: None,
        ..result
    };
    // Only the counts are pinned: an unknown duration currently prints as 0ms.
    assert!(untimed.format(ultra).starts_with("[ok]7 [x]2 [skip]3"));
}

fn dependency(name: &str, latest: Option<&str>) -> Dependency {
    Dependency {
        name: name.to_string(),
        current_version: "1.0.0".to_string(),
        latest_version: latest.map(str::to_string),
        wanted_version: None,
        dev_dependency: false,
    }
}

#[test]
fn compact_package_inventory_counts_entries_beyond_the_listing_cap() {
    let total = MAX_DEPS_LISTING + 2;
    let state = DependencyState {
        total_packages: total,
        outdated_count: 0,
        dependencies: (0..total)
            .map(|index| Dependency {
                dev_dependency: index == 0,
                ..dependency(&format!("pkg-{index}"), None)
            })
            .collect(),
    };

    let output = state.format(FormatMode::Compact);
    let mut lines = output.lines();
    let header = format!("{total} packages");
    assert_eq!(lines.next(), Some(header.as_str()));
    assert_eq!(lines.next(), Some("  pkg-0 1.0.0 (dev)"));
    for index in 1..MAX_DEPS_LISTING {
        let entry = format!("  pkg-{index} 1.0.0");
        assert_eq!(lines.next(), Some(entry.as_str()));
    }
    assert_eq!(lines.next(), Some("  ... +2 more"));
    assert_eq!(lines.next(), None);
}

#[test]
fn outdated_packages_show_full_upgrade_details_only_in_verbose_mode() {
    let mut dependencies: Vec<_> = (1..=12)
        .map(|index| Dependency {
            dev_dependency: index == 1,
            wanted_version: match index {
                1 => Some("1.5.0".to_string()),
                2 => Some("2.0.0".to_string()),
                _ => None,
            },
            ..dependency(&format!("pkg-{index}"), Some("2.0.0"))
        })
        .collect();
    dependencies.extend([
        dependency("already-current", Some("1.0.0")),
        dependency("unknown-latest", None),
    ]);
    let state = DependencyState {
        total_packages: dependencies.len(),
        outdated_count: 12,
        dependencies,
    };

    let compact = state.format(FormatMode::Compact);
    let mut lines = compact.lines();
    assert_eq!(lines.next(), Some("12 outdated packages (of 14)"));
    for index in 1..=10 {
        let upgrade = format!("pkg-{index}: 1.0.0 → 2.0.0");
        assert_eq!(lines.next(), Some(upgrade.as_str()));
    }
    assert_eq!(lines.next(), Some(""));
    assert_eq!(lines.next(), Some("... +2 more"));
    assert_eq!(lines.next(), None);

    let verbose = state.format(FormatMode::Verbose);
    let mut lines = verbose.lines();
    assert_eq!(lines.next(), Some("Total packages: 14 (12 outdated)"));
    assert_eq!(lines.next(), Some(""));
    assert_eq!(lines.next(), Some("Outdated packages:"));
    assert_eq!(lines.next(), Some("  pkg-1: 1.0.0 → 2.0.0 (dev)"));
    assert_eq!(lines.next(), Some("    (wanted: 1.5.0)"));
    for index in 2..=12 {
        let upgrade = format!("  pkg-{index}: 1.0.0 → 2.0.0");
        assert_eq!(lines.next(), Some(upgrade.as_str()));
    }
    assert_eq!(lines.next(), None);

    assert_eq!(state.format(FormatMode::Ultra), "pkg:14 ^12");
}
