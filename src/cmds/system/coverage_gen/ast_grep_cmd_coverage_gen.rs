use super::*;

#[test]
fn bare_words_do_not_take_option_values() {
    type ArityLookup = fn(TokenKind, &str) -> Option<ValueSpec>;
    let cases: [(ArityLookup, &str); 2] =
        [(global_takes_value, "config"), (run_takes_value, "pattern")];

    for (lookup, name) in cases {
        let args = [name.to_owned()];
        let tokens = arg_tokenizer::tokenize_grammar(&args, &lookup, arg_tokenizer::Dialect::Posix);
        let positional = tokens
            .into_iter()
            .find(|token| token.is_free_positional())
            .unwrap();
        assert!(lookup(positional.kind, name).is_none(), "{name}");
    }
}

#[test]
fn blank_separators_do_not_count_toward_match_caps() {
    let raw = "\n a.rs:1:first\n \t\n a.rs:2:second\n\nb.rs:3:third\n";
    let result = filter_ast_grep(raw, 1, 2);

    assert_eq!(
        result.text,
        " a.rs:1:first\n  … 1 more match line(s) in  a.rs\nb.rs:3:third\n"
    );
    assert_eq!(result.hidden_lines, 1);
}

#[cfg(unix)]
mod cli {
    use crate::core::test_isolation;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::process::Stdio;

    const RUN: &[&str] = &["run", "-p", "fixture pattern"];
    const KEPT: &str = "a.rs:1:one\na.rs:2:two\na.rs:3:three\na.rs:4:four\na.rs:5:five\n";
    const COUNT: &str = "  … 1 more match line(s) in a.rs\n";
    const FALLBACK: &str = "  (use --json, narrow the pattern, or rtk proxy ast-grep)\n";

    fn fixture() -> tempfile::TempDir {
        let root = test_isolation::tempdir();
        let bin = root.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let tool = bin.join("ast-grep");
        fs::write(
            &tool,
            r#"#!/bin/sh
case "$#:$1:$2:$3:$4" in
    '3:run:-p:fixture pattern:'|'4:run:-p:fixture pattern:--json=stream'|'3:scan:--report-style:short:') ;;
    *) printf '%s\n' 'unexpected fixture arguments' >&2; exit 90 ;;
esac
printf '%s' "$AST_GREP_TEST_STDOUT"
printf '%s\n' 'fixture-stderr' >&2
exit "$AST_GREP_TEST_STATUS"
"#,
        )
        .unwrap();
        fs::set_permissions(tool, fs::Permissions::from_mode(0o755)).unwrap();

        let tee = root.path().join("tee");
        fs::create_dir(&tee).unwrap();
        let config = format!(
            "[tee]\nenabled = true\ntee_directory = {}\n",
            toml::Value::String(tee.to_str().unwrap().to_owned())
        );
        for base in [
            root.path().join(".config"),
            root.path().join("Library").join("Application Support"),
        ] {
            let config_dir = base.join("rtk");
            fs::create_dir_all(&config_dir).unwrap();
            fs::write(config_dir.join("config.toml"), &config).unwrap();
        }
        root
    }

    fn invoke(root: &Path, args: &[&str], raw: &str, code: i32) -> String {
        let mut cmd = test_isolation::rtk_command();
        let inherited_path = cmd
            .get_envs()
            .find(|(key, _)| *key == "PATH")
            .and_then(|(_, value)| value)
            .unwrap_or_default();
        let path = std::env::join_paths(
            std::iter::once(root.join("bin")).chain(std::env::split_paths(inherited_path)),
        )
        .unwrap();

        let output = cmd
            .current_dir(root)
            .env("PATH", path)
            .env("HOME", root)
            .env("XDG_CONFIG_HOME", root.join(".config"))
            .env("XDG_DATA_HOME", root.join("data"))
            .env("TMPDIR", root)
            .env("AST_GREP_TEST_STDOUT", raw)
            .env("AST_GREP_TEST_STATUS", code.to_string())
            .stdin(Stdio::null())
            .arg("ast-grep")
            .args(args)
            .output()
            .unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(output.status.code(), Some(code), "{args:?}: {stderr}");
        assert!(stderr.contains("fixture-stderr\n"), "{args:?}: {stderr}");
        String::from_utf8(output.stdout).unwrap()
    }

    #[test]
    fn json_and_non_run_invocations_preserve_stdout() {
        if !test_isolation::rtk_binary_is_built() {
            return;
        }

        let root = fixture();
        // Use match-shaped data for both requests so accidental filtering is observable.
        let raw = format!("{KEPT}a.rs:6:{}\n", "omitted detail ".repeat(120));
        let mut json_args = RUN.to_vec();
        json_args.push("--json=stream");
        assert_eq!(invoke(root.path(), &json_args, &raw, 7), raw);
        assert_eq!(
            invoke(root.path(), &["scan", "--report-style", "short"], &raw, 0),
            raw
        );
    }

    #[test]
    fn capped_output_uses_a_fallback_or_links_the_complete_archive() {
        if !test_isolation::rtk_binary_is_built() {
            return;
        }

        let root = fixture();
        let short = format!("{KEPT}a.rs:6:{}\n", "omitted detail ".repeat(14));
        assert_eq!(
            invoke(root.path(), RUN, &short, 0),
            format!("{KEPT}{COUNT}{FALLBACK}")
        );

        let long = format!("{KEPT}a.rs:6:{}\n", "omitted detail ".repeat(120));
        let stdout = invoke(root.path(), RUN, &long, 0);
        let hint = stdout
            .strip_prefix(&format!("{KEPT}{COUNT}"))
            .and_then(|tail| tail.strip_suffix('\n'))
            .unwrap_or_else(|| panic!("expected capped matches followed by a hint: {stdout:?}"));
        assert!(!hint.contains('\n'), "{stdout:?}");
        assert_ne!(hint, FALLBACK.trim_end());

        let archive = walkdir::WalkDir::new(root.path())
            .into_iter()
            .map(|entry| entry.unwrap())
            .filter(|entry| entry.file_type().is_file())
            .find(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| hint.contains(name))
                    && fs::read(entry.path()).is_ok_and(|bytes| bytes == long.as_bytes())
            });
        assert!(
            archive.is_some(),
            "recovery hint did not name an archive containing the original stdout: {hint:?}"
        );
    }
}
