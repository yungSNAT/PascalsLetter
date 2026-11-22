#![cfg(unix)]

use crate::core::test_isolation;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Output, Stdio};

fn install_fake_tool(dir: &Path, name: &str, body: &str) {
    let path = dir.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}")).expect("write fake tool");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
        .expect("make fake tool executable");
}

fn invoke(dir: &Path, args: &[&str], expected_code: i32) -> Output {
    let mut command = test_isolation::rtk_command();
    let inherited_path = command
        .get_envs()
        .find(|(name, _)| *name == "PATH")
        .and_then(|(_, value)| value)
        .expect("isolated command has a PATH");
    let path = std::env::join_paths(
        std::iter::once(dir.to_path_buf()).chain(std::env::split_paths(inherited_path)),
    )
    .expect("prepend fake tool directory to PATH");
    let output = command
        .env("PATH", path)
        .current_dir(dir)
        .stdin(Stdio::null())
        .args(args)
        .output()
        .expect("run isolated rtk");
    assert_eq!(
        output.status.code(),
        Some(expected_code),
        "{args:?}: {output:?}"
    );
    output
}

#[test]
fn package_commands_forward_arguments_and_filter_bun_output() {
    if !test_isolation::rtk_binary_is_built() {
        return;
    }
    let dir = test_isolation::tempdir();
    install_fake_tool(
        dir.path(),
        "bun",
        r#"
case "$1" in
    add)
        [ "$#" -eq 2 ] && [ "$2" = '@scope/pkg@>=1.0.0 <2.0.0' ] || exit 96
        printf '%s\n' 'bun add v1.2.20 (6ad208bc)'
        printf '%s\n' 'Resolving dependencies' 'Resolved, downloaded and extracted [128]' >&2
        printf '%s\n' '+ @scope/pkg@1.4.0' '1 package installed [5.00ms]'
        ;;
    pm)
        [ "$#" -eq 3 ] && [ "$2" = ls ] && [ "$3" = --all ] || exit 96
        printf '%s\n' 'fixture-project node_modules' '├── zeta@2.0.0' \
            '│   └── alpha@1.0.0' '└── alpha@1.0.0'
        ;;
    *) exit 96 ;;
esac
"#,
    );

    let spec = "@scope/pkg@>=1.0.0 <2.0.0";
    let added = invoke(dir.path(), &["-v", "bun", "add", spec], 0);
    let stdout = String::from_utf8_lossy(&added.stdout);
    let stderr = String::from_utf8_lossy(&added.stderr);
    assert!(stdout.contains("+ @scope/pkg@1.4.0"), "{added:?}");
    assert!(stdout.contains("1 package installed"), "{added:?}");
    for noise in [
        "bun add v1.",
        "Resolving dependencies",
        "Resolved, downloaded",
    ] {
        assert!(
            !stdout.contains(noise) && !stderr.contains(noise),
            "{added:?}"
        );
    }
    assert!(
        stderr.contains(&format!("Running: bun add {spec}")),
        "{added:?}"
    );

    let listed = invoke(dir.path(), &["-v", "bun", "pm", "ls", "--all"], 0);
    let stdout = String::from_utf8_lossy(&listed.stdout);
    assert!(
        stdout.contains("2 deps\nalpha@1.0.0\nzeta@2.0.0"),
        "{listed:?}"
    );
    assert!(!stdout.contains("node_modules"), "{listed:?}");
    assert!(
        String::from_utf8_lossy(&listed.stderr).contains("Running: bun pm ls --all"),
        "{listed:?}"
    );
}

#[test]
fn bun_test_summarizes_one_shot_runs_but_keeps_watch_output() {
    if !test_isolation::rtk_binary_is_built() {
        return;
    }
    let dir = test_isolation::tempdir();
    install_fake_tool(
        dir.path(),
        "bun",
        r#"
[ "$1" = test ] || exit 96
shift
if [ "$1" = --watch ]; then
    shift
fi
[ "$#" -eq 1 ] && [ "$1" = 'coverage sample.test.ts' ] || exit 96
printf 'bun test v1.2.20 (6ad208bc)\n\ncoverage sample.test.ts:\n'
for n in 1 2 3 4 5 6 7 8; do
    printf '(pass) coverage passing case %s [0.10ms]\n' "$n"
done
printf '\n 8 pass\n 0 fail\n 8 expect() calls\nRan 8 tests across 1 file. [3.00ms]\n'
"#,
    );

    let file = "coverage sample.test.ts";
    let filtered = invoke(dir.path(), &["bun", "test", file], 0);
    let stdout = String::from_utf8_lossy(&filtered.stdout);
    assert!(!stdout.trim().is_empty(), "{filtered:?}");
    assert!(!stdout.contains("coverage passing case"), "{filtered:?}");
    assert!(
        !String::from_utf8_lossy(&filtered.stderr).contains("coverage passing case"),
        "{filtered:?}"
    );

    let watched = invoke(dir.path(), &["bun", "test", "--watch", file], 0);
    let stdout = String::from_utf8_lossy(&watched.stdout);
    assert!(stdout.starts_with("bun test v1.2.20"), "{watched:?}");
    assert!(
        stdout.contains("(pass) coverage passing case 8 [0.10ms]"),
        "{watched:?}"
    );
}

#[test]
fn build_run_and_bunx_preserve_useful_tool_output() {
    if !test_isolation::rtk_binary_is_built() {
        return;
    }
    let dir = test_isolation::tempdir();
    install_fake_tool(
        dir.path(),
        "bun",
        r#"
[ "$#" -eq 2 ] || exit 96
case "$1" in
    build)
        [ "$2" = 'fixture entry.ts' ] || exit 96
        printf 'export const answer = 42;\n\n'
        ;;
    run)
        [ "$2" = 'fixture task' ] || exit 96
        printf 'fixture script stdout\n\n'
        printf 'fixture script stderr\n' >&2
        exit 23
        ;;
    *) exit 96 ;;
esac
"#,
    );
    install_fake_tool(
        dir.path(),
        "bunx",
        r#"
[ "$1" = fixture-tool ] || exit 96
shift
if [ "$#" -eq 2 ] && [ "$1" = --watch ] && [ "$2" = 'two words' ]; then
    printf '\033[36mfixture watch output\033[0m\n\n'
else
    [ "$#" -eq 1 ] && [ "$1" = 'two words' ] || exit 96
    printf 'fixture tool result: %s\n' "$1"
fi
"#,
    );

    let built = invoke(dir.path(), &["bun", "build", "fixture entry.ts"], 0);
    assert_eq!(built.stdout.as_slice(), b"export const answer = 42;\n\n");

    let ran = invoke(dir.path(), &["bun", "run", "fixture task"], 23);
    assert_eq!(ran.stdout.as_slice(), b"fixture script stdout\n\n");
    assert!(String::from_utf8_lossy(&ran.stderr).contains("fixture script stderr"));

    let exec = invoke(dir.path(), &["bunx", "fixture-tool", "two words"], 0);
    assert!(String::from_utf8_lossy(&exec.stdout).contains("fixture tool result: two words"));

    let watched = invoke(
        dir.path(),
        &["bunx", "fixture-tool", "--watch", "two words"],
        0,
    );
    assert_eq!(
        watched.stdout.as_slice(),
        b"\x1b[36mfixture watch output\x1b[0m\n\n"
    );
}
