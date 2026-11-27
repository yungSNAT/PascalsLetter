use super::*;
use crate::core::pipeline_config as cfg;
use crate::core::test_isolation;
use std::fs;
use std::path::{Path, PathBuf};

const LIFECYCLE_V1: &str = "schema_version = 1\n\n[pipeline]\nname = \"lifecycle\"\n\n[[hook.after_tool.shell]]\nuse = [\"dedup\"]\n";

fn write_file(dir: &Path, name: &str, text: &str) -> PathBuf {
    fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    fs::write(&path, text).unwrap();
    path
}

#[test]
fn gist_raw_url_keeps_raw_and_empty_gist_urls() {
    let raw = "https://gist.github.com/alice/abc123/raw/pipe.toml";
    assert_eq!(gist_raw_url(raw), raw);
    let bare = "https://gist.github.com/";
    assert_eq!(gist_raw_url(bare), bare);
}

#[test]
fn scan_flags_email_addresses() {
    let sources = vec![(
        "user pipeline.toml".to_string(),
        "# maintained by alice@example.com\n[pipeline]\nname = \"demo\"\n".to_string(),
    )];
    let hits = scan_for_secrets(&sources);
    // Keep the hits out of the failure message: they are the flagged text.
    assert!(
        hits.iter().any(|h| h == "email address: alice@example.com"),
        "the scan did not flag the email address"
    );
}

#[test]
fn warning_panel_wraps_and_skips_blank_input_lines() {
    let text = warning_panel_text("Heads up", &["   ", "", "short line"]);
    assert!(text.contains("Heads up"), "{text}");
    assert!(text.contains("short line"), "{text}");
    // title + empty row + "short line" row + bottom border; whitespace-only
    // input produces no row.
    assert_eq!(text.lines().count(), 4, "{text}");

    let long = "word ".repeat(40);
    let wrapped = warning_panel_text("T", &[long.trim()]);
    assert!(wrapped.lines().count() > 3, "{wrapped}");
}

#[test]
fn show_and_validate_report_broken_and_flagged_user_pipelines() {
    let root = test_isolation::tempdir();
    test_isolation::with_root(root.path(), || {
        let pristine = load_for_cli();
        assert!(!pristine.has_user_files);
        assert_eq!(run_show(None).unwrap(), 0);

        let dir = cfg::user_pipelines_dir().unwrap();
        write_file(&dir, "broken.toml", "[[[ this is not toml\n");
        let loaded = load_for_cli();
        assert!(loaded.has_user_files);
        assert!(has_errors(&loaded.diagnostics));
        assert_eq!(run_show(None).unwrap(), 0);
        assert_eq!(run_validate().unwrap(), 1);

        write_file(
            &dir,
            "showcase-rules.toml",
            "schema_version = 1\n\n[pipeline]\nname = \"showcase-rules\"\n\n\
             [[hook.after_tool.shell]]\nuse = [\"dedup\"]\nenabled = false\n\n\
             [[hook.after_tool.shell]]\nuse = [\"dedup\"]\ncontinue = true\n",
        );
        write_file(
            &dir,
            "showcase-directive.toml",
            "schema_version = 1\n\n[pipeline]\nname = \"showcase-directive\"\n\n\
             [[directive]]\nname = \"showcase-note\"\ntext = \"Keep answers short.\"\n",
        );
        write_file(
            &dir,
            "showcase-filedir.toml",
            "schema_version = 1\n\n[pipeline]\nname = \"showcase-filedir\"\n\n\
             [[directive]]\nname = \"showcase-file\"\nfile = \"does-not-exist.md\"\n",
        );
        if let Some(method) = rtk_engine::registry::PROXY_METHOD_INFO
            .first()
            .map(|(n, _)| n.to_string())
        {
            write_file(
                &dir,
                "showcase-proxy.toml",
                &format!(
                    "schema_version = 1\n\n[pipeline]\nname = \"showcase-proxy\"\n\n\
                     [[hook.before_tool.shell]]\nuse = [\"{method}\"]\n"
                ),
            );
        }
        let loaded = load_for_cli();
        assert!(loaded.has_user_files);
        assert!(has_errors(&loaded.diagnostics));
        assert_eq!(run_show(None).unwrap(), 0);
        assert_eq!(run_validate().unwrap(), 1);
    });
}

#[test]
fn add_promote_and_eval_reject_unusable_sources() {
    let root = test_isolation::tempdir();
    test_isolation::with_root(root.path(), || {
        let src = root.path().join("sources");

        let empty = write_file(&src, "empty.toml", "\n   \n");
        let err = run_add(empty.to_str().unwrap(), None, true, false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("no pipeline TOML"), "{err}");

        let valid = write_file(&src, "valid.toml", LIFECYCLE_V1);
        let err = run_add(valid.to_str().unwrap(), Some("bad name"), true, false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("must be alphanumeric"), "{err}");

        let unnamed = write_file(&src, "no name.toml", "schema_version = 1\n");
        let err = run_add(unnamed.to_str().unwrap(), None, true, false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("must be alphanumeric"), "{err}");

        let sdir = cfg::shadow_pipelines_dir().unwrap();
        write_file(&sdir, "shadowbroken.toml", "[[[ nope\n");
        let err = run_promote("shadowbroken", false, true)
            .unwrap_err()
            .to_string();
        assert!(err.contains("does not validate"), "{err}");
        assert!(sdir.join("shadowbroken.toml").is_file());

        let udir = cfg::user_pipelines_dir().unwrap();
        write_file(&udir, "evalbroken.toml", "[[[ nope\n");
        let err = run_eval("evalbroken", true, false).unwrap_err().to_string();
        assert!(err.contains("does not validate"), "{err}");
    });
}

#[test]
fn shadow_update_then_promote_replaces_the_live_pipeline() {
    let root = test_isolation::tempdir();
    test_isolation::with_root(root.path(), || {
        let src = root.path().join("sources");
        let v1 = write_file(&src, "lifecycle.toml", LIFECYCLE_V1);
        assert_eq!(run_add(v1.to_str().unwrap(), None, true, false).unwrap(), 0);
        let dest = cfg::user_pipelines_dir().unwrap().join("lifecycle.toml");
        assert_eq!(fs::read_to_string(&dest).unwrap(), LIFECYCLE_V1);

        let v2_text = format!("# revised\n{LIFECYCLE_V1}");
        let v2 = write_file(&src, "lifecycle-v2.toml", &v2_text);
        assert_eq!(run_add(v2.to_str().unwrap(), None, false, true).unwrap(), 0);
        let shadow = cfg::shadow_pipelines_dir().unwrap().join("lifecycle.toml");
        assert_eq!(fs::read_to_string(&shadow).unwrap(), v2_text);
        // The live install stays untouched while the new version is shadowed.
        assert_eq!(fs::read_to_string(&dest).unwrap(), LIFECYCLE_V1);
        assert_eq!(cfg::eval_candidate().as_deref(), Some("lifecycle"));

        assert_eq!(run_promote("lifecycle", false, true).unwrap(), 0);
        assert_eq!(fs::read_to_string(&dest).unwrap(), v2_text);
        assert!(!shadow.exists());
        assert_ne!(cfg::eval_candidate().as_deref(), Some("lifecycle"));
    });
}
