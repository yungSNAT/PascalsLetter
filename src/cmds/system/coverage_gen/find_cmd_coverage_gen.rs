use super::*;
use crate::core::{test_isolation, user_env};
use std::fs;

#[test]
fn dispatch_preserves_native_flags_and_does_not_swallow_an_invalid_max() {
    for (type_flag, max_flag) in [("-type", "-m"), ("-t", "--max"), ("--file-type", "-m")] {
        let args = [
            "-iname",
            "*.LOG",
            type_flag,
            "d",
            max_flag,
            "4",
            "-maxdepth",
            "2",
        ]
        .map(str::to_owned);
        let Dispatch::Native { parsed, .. } = dispatch(&args).unwrap() else {
            panic!("expected native dispatch for {args:?}");
        };
        assert_eq!(parsed.pattern, "*.LOG");
        assert!(parsed.case_insensitive);
        assert_eq!(parsed.file_type, "d");
        assert_eq!(parsed.max_results, 4);
        assert!(parsed.max_explicit);
        assert_eq!(parsed.max_depth, Some(2));
    }

    let args = ["-name", "*.log", "--max", "invalid", "--file-type", "d"].map(str::to_owned);
    let Dispatch::Compress {
        expr,
        max,
        file_type,
        ..
    } = dispatch(&args).unwrap()
    else {
        panic!("expected compress dispatch to preserve the invalid argument");
    };
    assert_eq!(expr, ["-name", "*.log", "--max", "invalid"]);
    assert_eq!(max, None);
    assert_eq!(file_type.as_deref(), Some("d"));
}

#[test]
fn grouping_keeps_a_path_with_neither_parent_nor_basename() {
    let root = std::path::MAIN_SEPARATOR_STR;
    let grouped = group_by_dir(&[root.to_owned(), "loose.txt".to_owned()]);
    assert_eq!(grouped.len(), 1);
    assert_eq!(grouped["."], [root, "loose.txt"]);
}

#[test]
fn native_find_handles_explicit_file_directory_and_missing_roots() {
    let tmp = test_isolation::tempdir();
    let file = tmp.path().join("only.txt");
    fs::write(&file, "sample").unwrap();
    fs::create_dir(tmp.path().join("nested")).unwrap();

    let file = file.to_str().unwrap();
    let (files, filtered) = native_walk(file, "*.txt", Some(0), false, false, false);
    assert_eq!(files, [file]);
    assert!(filtered.is_empty(), "{filtered:?}");

    let root = tmp.path().to_str().unwrap();
    let (dirs, filtered) = native_walk(root, "*", Some(1), true, false, false);
    assert_eq!(dirs, ["nested"]);
    assert!(filtered.is_empty(), "{filtered:?}");

    let missing = tmp.path().join("absent");
    test_isolation::with_root(tmp.path(), || {
        assert_eq!(
            run(
                "find",
                "*.txt",
                missing.to_str().unwrap(),
                10,
                true,
                None,
                "f",
                false,
                1,
            )
            .unwrap(),
            1
        );
    });
}

#[test]
fn crossing_the_walk_budget_suppresses_disclosure_but_not_matches() {
    let tmp = test_isolation::tempdir();
    let root = tmp.path().to_str().unwrap();
    fs::write(tmp.path().join(".ignore"), ".ignore\nomitted.txt\n").unwrap();
    fs::write(tmp.path().join("omitted.txt"), "").unwrap();

    // The starting directory also counts toward the disclosure budget.
    for i in 0..DISCLOSURE_ENTRY_CAP - 1 {
        fs::write(tmp.path().join(format!("entry-{i:05}.txt")), "").unwrap();
    }
    let (files, filtered) = native_walk(root, "*.txt", None, false, false, false);
    assert_eq!(files.len(), DISCLOSURE_ENTRY_CAP - 1);
    assert_eq!(filtered, ["omitted.txt"]);

    fs::write(tmp.path().join("extra.txt"), "").unwrap();
    let (files, filtered) = native_walk(root, "*.txt", None, false, false, false);
    assert_eq!(files.len(), DISCLOSURE_ENTRY_CAP);
    assert!(files.iter().any(|file| file == "extra.txt"));
    assert!(!files.iter().any(|file| file == "omitted.txt"));
    assert!(filtered.is_empty(), "{filtered:?}");
}

#[test]
fn disclosure_skips_missing_directories_and_reports_its_cap() {
    let tmp = test_isolation::tempdir();
    let late = tmp.path().join("z-first");
    let early = tmp.path().join("a-second");
    fs::create_dir(&late).unwrap();
    fs::create_dir(&early).unwrap();
    fs::write(late.join("marker.txt"), "").unwrap();
    for i in 0..FILTERED_CAP {
        fs::write(early.join(format!("hidden-{i:04}.txt")), "").unwrap();
    }

    let filtered = disclose_filtered(
        tmp.path().to_str().unwrap(),
        "*.txt",
        None,
        false,
        false,
        &[(tmp.path().join("vanished"), 1), (late, 1), (early, 1)],
        &HashSet::new(),
    );
    assert_eq!(filtered.len(), FILTERED_CAP);
    assert!(filtered.windows(2).all(|pair| pair[0] < pair[1]));
    let marker = Path::new("z-first").join("marker.txt");
    assert_eq!(
        Path::new(filtered.last().unwrap()),
        marker.as_path(),
        "the first collected entry should sort after the rest"
    );

    let home = test_isolation::tempdir();
    let tee = test_isolation::tempdir();
    test_isolation::with_root(home.path(), || {
        user_env::with_path("RTK_TEE_DIR", Some(tee.path()), || {
            let hint = filtered_hint(&filtered).expect("a capped disclosure hint");
            let expected = format!("... ({FILTERED_CAP}+ filtered)");
            // The count is stable whether or not a recall hint is appended.
            assert_eq!(hint.lines().next(), Some(expected.as_str()));
        });
    });
}

#[cfg(unix)]
#[test]
fn compressed_find_caps_text_but_preserves_non_utf8_results_and_tracks_passthrough() {
    use std::os::unix::fs::PermissionsExt;

    if !test_isolation::rtk_binary_is_built() {
        return;
    }

    let tools = test_isolation::tempdir();
    let home = test_isolation::tempdir();
    let fake_find = tools.path().join("find");
    fs::write(
        &fake_find,
        r#"#!/bin/sh
if [ "$#" -ne 9 ] || [ "$1" != '-L' ] || [ "$2" != '.' ] ||
   [ "$3" != '(' ] || [ "$4" != '-name' ] || [ "$6" != ')' ] ||
   [ "$7" != '-type' ] || [ "$8" != 'f' ] || [ "$9" != '-print0' ]; then
    printf 'unexpected fixture arguments: %s\n' "$*" >&2
    exit 97
fi
case "$5" in
    '*.txt') printf './listed/z.txt\000./listed/a.txt\000' ;;
    '*.rs') printf 'z.rs\000bad\377.rs\000' ;;
    *) exit 98 ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_find, fs::Permissions::from_mode(0o755)).unwrap();

    test_isolation::with_root(home.path(), || {
        let run_find = |pattern: &str| {
            let mut cmd = test_isolation::rtk_command();
            let inherited = cmd
                .get_envs()
                .find(|(key, _)| *key == "PATH")
                .and_then(|(_, value)| value)
                .into_iter()
                .flat_map(std::env::split_paths);
            let path =
                std::env::join_paths(std::iter::once(tools.path().to_path_buf()).chain(inherited))
                    .unwrap();
            let output = cmd
                .env("PATH", path)
                .current_dir(tools.path())
                .stdin(std::process::Stdio::null())
                .args([
                    "-v",
                    "find",
                    "-L",
                    ".",
                    "-name",
                    pattern,
                    "--file-type",
                    "f",
                    "--max",
                    "1",
                ])
                .output()
                .expect("run rtk with the fake find");
            assert!(output.status.success(), "{output:?}");
            output
        };

        let text = run_find("*.txt");
        let stderr = String::from_utf8_lossy(&text.stderr);
        assert!(
            stderr.contains("find: results from find, compressed by rtk"),
            "{stderr}"
        );
        let shown = std::str::from_utf8(&text.stdout).unwrap();
        let unstripped_dir = Path::new(".").join("listed");
        assert!(
            shown.contains("listed") && shown.contains("a.txt"),
            "{shown}"
        );
        assert!(
            shown.contains("+1 more") && !shown.contains("z.txt"),
            "{shown}"
        );
        assert!(!shown.contains(unstripped_dir.to_str().unwrap()), "{shown}");

        let raw = run_find("*.rs");
        assert_eq!(raw.stdout.as_slice(), b"z.rs\nbad\xff.rs\n");

        let tracker = tracking::Tracker::new().expect("open fixture tracking database");
        let recent = tracker.get_recent(5).expect("read fixture tracking rows");
        let label = "rtk:passthrough find -L . -name '*.rs' --file-type f --max 1";
        assert!(recent.iter().any(|row| row.rtk_cmd == label), "{recent:?}");
    });
}
