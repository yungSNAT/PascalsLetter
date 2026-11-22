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

#[cfg(unix)]
#[test]
fn next_build_prefers_next_and_falls_back_to_npx_with_the_same_build_arguments() {
    use crate::core::test_isolation::{rtk_binary_is_built, rtk_command, tempdir};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Stdio;

    if !rtk_binary_is_built() {
        return;
    }

    for (executable, invocation) in [("next", "next"), ("npx", "npx next")] {
        let dir = tempdir();
        let install_tool = |name: &str| {
            let path = dir.path().join(name);
            fs::write(
                &path,
                r#"#!/bin/sh
tool=${0##*/}
case "$tool:$1" in
    next:build) ;;
    npx:next) shift ;;
    *) set -- ;;
esac
if [ "$#" -eq 2 ] && [ "$1" = build ] && [ "$2" = 'fixture project' ]; then
    printf '○ /%s 10 kB 11 kB\n' "$tool"
    printf '%s\n' 'Compiled in 125ms'
fi
"#,
            )
            .unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        };
        install_tool("npx");
        if executable == "next" {
            install_tool("next");
        }

        let mut command = rtk_command();
        let inherited_path = command
            .get_envs()
            .find(|(key, _)| *key == "PATH")
            .and_then(|(_, value)| value)
            .unwrap_or_default();
        // Keep system entries only, and don't let a host Next.js install mask the fallback.
        let system_paths = std::env::split_paths(inherited_path).filter(|path| {
            matches!(path.to_str(), Some("/bin" | "/usr/bin")) && !path.join("next").exists()
        });
        let path =
            std::env::join_paths(std::iter::once(dir.path().to_path_buf()).chain(system_paths))
                .unwrap();

        let output = command
            .env("PATH", path)
            .current_dir(dir.path())
            .stdin(Stdio::null())
            .args(["-v", "next", "build", "fixture project"])
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(output.status.success(), "{invocation}: {stderr}");
        assert!(
            stderr.contains(&format!("Running: {invocation} build")),
            "{stderr}"
        );
        assert!(
            stdout.contains("Next.js Build\n1 routes (1 static, 0 dynamic)"),
            "{stdout}"
        );
        assert!(stdout.contains(&format!("  /{executable} ")), "{stdout}");
        assert!(
            stdout.contains("Time: 125ms | Errors: 0 | Warnings: 0"),
            "{stdout}"
        );
    }
}
