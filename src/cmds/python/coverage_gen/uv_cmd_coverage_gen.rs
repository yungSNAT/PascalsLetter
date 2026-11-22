use super::{
    CAP_INVENTORY, MAX_ERROR_CONTINUATION_LINES, MAX_TRACEBACK_FRAMES, filter_uv_run_output,
};
use crate::core::{test_isolation, user_env};

fn with_tee_root(test: impl FnOnce()) {
    let root = test_isolation::tempdir();
    test_isolation::with_root(root.path(), || {
        user_env::with_vars(
            &[("RTK_TEE", Some("1")), ("RTK_TEE_MODE", Some("always"))],
            || user_env::with_path("RTK_TEE_DIR", Some(root.path()), test),
        );
    });
}

fn recall_id<'a>(hint: &'a str, prefix: &str) -> &'a str {
    let id = hint
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or_else(|| panic!("unexpected recall hint: {hint:?}"));
    assert!(
        id.len() == 16 && id.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "invalid recall ID in {hint:?}"
    );
    id
}

#[test]
fn successful_stderr_only_run_keeps_both_ends_and_offers_recall() {
    with_tee_root(|| {
        let lines: Vec<_> = (0..CAP_INVENTORY + 3)
            .map(|index| format!("diagnostic record {index}"))
            .collect();
        let stderr = lines.join("\n");
        let stdout = " \t\n";
        let shown = filter_uv_run_output(&format!("{stdout}{stderr}"), stdout, &stderr, 0);

        let head = CAP_INVENTORY / 2;
        let preview = format!(
            "{}\n... (3 lines omitted)\n{}\n",
            lines[..head].join("\n"),
            lines[head + 3..].join("\n")
        );
        let hint = shown
            .strip_prefix(preview.as_str())
            .unwrap_or_else(|| panic!("unexpected stderr preview: {shown:?}"));

        // The recall tail starts immediately after the visible head.
        let prefix = format!("[+{} hidden: rtk recall ", lines.len() - head);
        assert!(!recall_id(hint, &prefix).is_empty());
    });
}

#[test]
fn failed_run_separates_diagnostics_and_offers_distinct_recall_ids() {
    with_tee_root(|| {
        let header = "Traceback (most recent call last):";
        let frames: Vec<_> = (0..MAX_TRACEBACK_FRAMES + 2)
            .map(|index| format!("  File \"fixture.py\", line {}, in assemble", index + 1))
            .collect();
        let traceback = format!(
            "{header}\n{}\n    assemble()\nRuntimeError: invalid row",
            frames.join("\n")
        );

        let js_frame = "at load (fixture.js:3:4)";
        let details: Vec<_> = (0..MAX_ERROR_CONTINUATION_LINES + 2)
            .map(|index| match index {
                0 => "  stage: parse".to_string(),
                1 => "\tstage: parse".to_string(),
                _ => format!("  input row {index}"),
            })
            .collect();
        let js_error = format!("{js_frame}\n{}", details.join("\n"));
        let raw = format!(
            "{traceback}\n \t\n{js_error}\nordinary progress\nNo module named coverage_fixture\n"
        );
        let shown = filter_uv_run_output(&raw, "", "", 7);
        let blocks: Vec<_> = shown.split("\n\n").collect();
        let [shown_traceback, shown_js_error, shown_import] = blocks.as_slice() else {
            panic!("expected three separate diagnostic blocks, got {shown:?}");
        };
        assert_eq!(*shown_import, "No module named coverage_fixture");

        let mut traceback_preview = vec![header];
        traceback_preview.extend(
            frames[..MAX_TRACEBACK_FRAMES]
                .iter()
                .map(|frame| frame.trim()),
        );
        traceback_preview.push("... +2 more frames");
        let prefix = format!("{}\n", traceback_preview.join("\n"));
        let (traceback_hint, tail) = shown_traceback
            .strip_prefix(prefix.as_str())
            .and_then(|rest| rest.split_once('\n'))
            .unwrap_or_else(|| panic!("unexpected traceback preview: {shown_traceback:?}"));
        assert_eq!(tail, "assemble()\nRuntimeError: invalid row");

        let mut js_preview = vec![js_frame];
        js_preview.extend(
            details[..MAX_ERROR_CONTINUATION_LINES]
                .iter()
                .enumerate()
                .filter(|(index, _)| *index != 1)
                .map(|(_, detail)| detail.trim()),
        );
        js_preview.push("... +2 more lines");
        let prefix = format!("{}\n", js_preview.join("\n"));
        let js_hint = shown_js_error
            .strip_prefix(prefix.as_str())
            .unwrap_or_else(|| panic!("unexpected continuation preview: {shown_js_error:?}"));

        let hint_prefix = "  [full output: rtk recall ";
        assert_ne!(
            recall_id(traceback_hint, hint_prefix),
            recall_id(js_hint, hint_prefix),
            "different diagnostic blocks must not share a recall ID"
        );
    });
}

#[cfg(all(unix, feature = "engine"))]
#[test]
fn uv_cli_filters_run_but_passes_other_subcommands_through() {
    use std::{fs, os::unix::fs::PermissionsExt, process::Stdio};

    if !test_isolation::rtk_binary_is_built() {
        return;
    }

    let tools = test_isolation::tempdir();
    let fake_uv = tools.path().join("uv");
    fs::write(
        &fake_uv,
        "#!/bin/sh\nprintf 'payload for %s %s\\n\\n' \"$1\" \"$2\"\nprintf 'fixture diagnostic\\n' >&2\n",
    )
    .expect("write fake uv");
    fs::set_permissions(&fake_uv, fs::Permissions::from_mode(0o700))
        .expect("make fake uv executable");

    let invoke = |args: [&str; 2]| {
        let mut cmd = test_isolation::rtk_command();
        let inherited_path = cmd
            .get_envs()
            .find_map(|(key, value)| if key == "PATH" { value } else { None })
            .expect("isolated rtk command has a PATH");
        let path = std::env::join_paths(
            std::iter::once(tools.path().to_path_buf())
                .chain(std::env::split_paths(inherited_path)),
        )
        .expect("prepend fake tool directory to PATH");
        cmd.env("PATH", path)
            .stdin(Stdio::null())
            .args(["-v", "uv"])
            .args(args)
            .output()
            .expect("run isolated rtk with fake uv")
    };

    let filtered = invoke(["run", "coverage-probe"]);
    let stderr = String::from_utf8_lossy(&filtered.stderr);
    assert_eq!(filtered.status.code(), Some(0), "{stderr}");
    assert!(
        stderr.contains("Running: uv run coverage-probe"),
        "{stderr}"
    );
    assert_eq!(
        String::from_utf8_lossy(&filtered.stdout).trim_end(),
        "payload for run coverage-probe\nfixture diagnostic"
    );

    let passthrough = invoke(["pip", "list"]);
    let stderr = String::from_utf8_lossy(&passthrough.stderr);
    assert_eq!(passthrough.status.code(), Some(0), "{stderr}");
    assert_eq!(passthrough.stdout.as_slice(), b"payload for pip list\n\n");
    assert!(stderr.contains("Running: uv pip list"), "{stderr}");
    assert!(
        stderr.lines().any(|line| line == "fixture diagnostic"),
        "{stderr}"
    );
}
