use super::*;
use std::fmt::Write as _;
use std::fs;

#[test]
fn formatter_detection_prefers_explicit_choice_then_project_config() {
    let project = crate::core::test_isolation::tempdir();
    let dir = project.path();
    let pyproject = dir.join("pyproject.toml");
    let args = ["main.py".to_string()];

    fs::write(&pyproject, "[project]\nname = 'example'\n").unwrap();
    assert_eq!(detect_formatter_in_dir(&args, dir), "ruff");

    for name in [".prettierrc", ".prettierrc.json", ".prettierrc.js"] {
        let config = dir.join(name);
        fs::write(&config, "").unwrap();
        assert_eq!(detect_formatter_in_dir(&args, dir), "prettier", "{name}");
        fs::remove_file(config).unwrap();
    }

    fs::write(dir.join(".prettierrc.js"), "module.exports = {};").unwrap();
    fs::write(&pyproject, "[tool.ruff]\nline-length = 88\n").unwrap();
    assert_eq!(detect_formatter_in_dir(&args, dir), "ruff");

    fs::write(&pyproject, "[tool.ruff.format]\n[tool.black]\n").unwrap();
    assert_eq!(detect_formatter_in_dir(&args, dir), "black");
    assert_eq!(
        detect_formatter_in_dir(&["biome".to_string(), "check".to_string()], dir),
        "biome"
    );
}

#[test]
fn black_filter_caps_the_file_list_without_losing_totals() {
    let total = CAP_WARNINGS + 2;
    let mut raw = String::new();
    for index in 1..=total {
        writeln!(raw, "Would reformat: file-{index}.py").unwrap();
    }
    raw.push_str("3 files would be left unchanged.\n");

    let filtered = filter_black_output(&raw);
    let mut lines = filtered.lines();
    assert_eq!(
        lines.next().unwrap(),
        format!("Format (black): {total} files need formatting")
    );
    for index in 1..=CAP_WARNINGS {
        assert_eq!(lines.next().unwrap(), format!("{index}. file-{index}.py"));
    }
    assert_eq!(
        lines.collect::<Vec<_>>(),
        [
            "",
            "... +2 more files",
            "",
            "3 files already formatted",
            "",
            "[hint] Run `black .` to format these files",
        ]
    );
}

#[test]
fn black_filter_handles_summaries_and_preserves_unknown_diagnostics() {
    for (raw, expected) in [
        (
            "All done!\n4 files left unchanged.\n",
            "Format (black): All files formatted (4 files checked)",
        ),
        (
            "1 file would be reformatted, 2 files would be left unchanged.\n",
            "Format (black): 1 files need formatting\n\n2 files already formatted\n\n[hint] Run `black .` to format these files",
        ),
        (
            "\n  error: cannot parse formatter configuration\n",
            "error: cannot parse formatter configuration",
        ),
    ] {
        assert_eq!(filter_black_output(raw), expected);
    }
}

#[cfg(all(unix, feature = "engine"))]
mod cli {
    use super::{fs, never_worse, prettier_cmd, ruff_cmd};
    use crate::core::test_isolation;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    fn install_fake_tool(path: &Path) {
        fs::write(
            path,
            r#"#!/bin/sh
tool=${0##*/}
case "$tool" in
    black|ruff|prettier|biome) ;;
    *)
        # Recognize either JS formatter without invoking a real package manager.
        while [ "$#" -gt 0 ]; do
            case "$1" in
                prettier|biome) tool=$1; shift; break ;;
                @biomejs/biome) tool=biome; shift; break ;;
            esac
            shift
        done
        ;;
esac
invocation=$tool
for arg; do
    invocation="$invocation|$arg"
done
if [ "$invocation" != "$FORMAT_TEST_INVOCATION" ]; then
    printf 'unexpected formatter invocation: %s\n' "$invocation" >&2
    exit 97
fi
printf '%s' "$FORMAT_TEST_STDOUT"
printf '%s' "$FORMAT_TEST_STDERR" >&2
exit "$FORMAT_TEST_STATUS"
"#,
        )
        .unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[test]
    fn format_forwards_arguments_and_reports_formatter_output_and_status() {
        if !test_isolation::rtk_binary_is_built() {
            return;
        }

        let project = test_isolation::tempdir();
        let dir = project.path();
        fs::write(dir.join("pyproject.toml"), "[tool.ruff]\n").unwrap();
        fs::write(dir.join("package.json"), "{}").unwrap();

        let bin = dir.join("bin");
        fs::create_dir(&bin).unwrap();
        for name in [
            "black", "ruff", "prettier", "biome", "npx", "npm", "pnpm", "pnpx", "yarn", "bun",
            "bunx", "corepack",
        ] {
            install_fake_tool(&bin.join(name));
        }
        let local_bin = dir.join("node_modules").join(".bin");
        fs::create_dir_all(&local_bin).unwrap();
        for name in ["prettier", "biome"] {
            install_fake_tool(&local_bin.join(name));
        }

        for (args, invocation, status) in [
            (&["black", "--quiet"][..], "black|--check|--quiet|.", 1),
            (&["black", "--diff", "main.py"], "black|--diff|main.py", 0),
            (&["main.py"], "ruff|format|main.py", 0),
            (
                &["ruff", "format", "--check", "main.py"],
                "ruff|format|--check|main.py",
                1,
            ),
            (
                &["prettier", "--check", "main.js"],
                "prettier|--check|main.js",
                1,
            ),
            (&["biome", "check", "main.js"], "biome|check|main.js", 0),
        ] {
            let (tool, _) = invocation.split_once('|').unwrap();
            let (stdout, stderr) = match tool {
                "black" => (
                    "",
                    concat!(
                        "Checking Python sources in the formatting fixture project.\n",
                        "would reformat: main.py\n",
                        "Oh no! Formatting changes are required before checking again.\n",
                        "1 file would be reformatted, 3 files would be left unchanged.",
                    ),
                ),
                "ruff" => (
                    "Would reformat: main.py\n1 file would be reformatted, 2 files already formatted",
                    "",
                ),
                "prettier" => (
                    "Checking formatting...",
                    "[warn] main.js\n[warn] Code style issues found in the above file. Run Prettier with --write to fix.",
                ),
                "biome" => ("Checked 1 file. No fixes applied.", "Configuration note"),
                _ => unreachable!(),
            };

            let mut cmd = test_isolation::rtk_command();
            let inherited_path = cmd
                .get_envs()
                .find_map(|(key, value)| if key == "PATH" { value } else { None })
                .expect("isolated rtk command supplies PATH");
            let path = std::env::join_paths(
                std::iter::once(bin.clone()).chain(std::env::split_paths(inherited_path)),
            )
            .unwrap();
            let output = cmd
                .current_dir(dir)
                .env("PATH", path)
                .env("FORMAT_TEST_INVOCATION", invocation)
                .env("FORMAT_TEST_STDOUT", stdout)
                .env("FORMAT_TEST_STDERR", stderr)
                .env("FORMAT_TEST_STATUS", status.to_string())
                .args(["-v", "format"])
                .args(args)
                .output()
                .unwrap();

            let actual_stdout = String::from_utf8(output.stdout).unwrap();
            let actual_stderr = String::from_utf8(output.stderr).unwrap();
            assert_eq!(
                output.status.code(),
                Some(status),
                "format {args:?}: {actual_stdout}\n{actual_stderr}"
            );

            let raw = format!("{stdout}\n{stderr}");
            let expected = match tool {
                "black" => concat!(
                    "Format (black): 1 files need formatting\n",
                    "1. main.py\n\n",
                    "3 files already formatted\n\n",
                    "[hint] Run `black .` to format these files",
                )
                .to_string(),
                "ruff" => {
                    let filtered = ruff_cmd::filter_ruff_format(&raw);
                    never_worse(&raw, &filtered).trim().to_string()
                }
                "prettier" => {
                    let filtered = prettier_cmd::filter_prettier_output(&raw);
                    never_worse(&raw, &filtered).trim().to_string()
                }
                "biome" => raw.trim().to_string(),
                _ => unreachable!(),
            };
            assert_eq!(actual_stdout.trim(), expected, "format {args:?}");

            let user_args = if args[0] == tool { &args[1..] } else { args };
            assert!(
                actual_stderr.contains(&format!(
                    "Detected formatter: {tool}\nArguments: {}\nRunning: {tool} {}\n",
                    user_args.join(" "),
                    user_args.join(" "),
                )),
                "format {args:?}: {actual_stderr}"
            );
        }
    }
}
