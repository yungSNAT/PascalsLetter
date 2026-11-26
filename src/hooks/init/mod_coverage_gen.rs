use super::*;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn verbose_ctx(dry_run: bool) -> InitContext {
    InitContext {
        verbose: 1,
        dry_run,
        ..Default::default()
    }
}

#[test]
fn run_rejects_invalid_flag_combinations_before_touching_anything() {
    let ctx = InitContext {
        dry_run: true,
        ..Default::default()
    };
    let msg = |r: anyhow::Result<()>| format!("{:#}", r.unwrap_err());

    // --codex conflicts
    let e = msg(run(
        false,
        true,
        true,
        false,
        false,
        false,
        false,
        false,
        true,
        PatchMode::Ask,
        ctx,
    ));
    assert!(
        e.contains("--codex cannot be combined with --opencode"),
        "{e}"
    );
    let e = msg(run(
        false,
        true,
        false,
        false,
        false,
        false,
        true,
        false,
        true,
        PatchMode::Ask,
        ctx,
    ));
    assert!(
        e.contains("--codex cannot be combined with --claude-md"),
        "{e}"
    );
    let e = msg(run(
        false,
        true,
        false,
        false,
        false,
        false,
        false,
        true,
        true,
        PatchMode::Ask,
        ctx,
    ));
    assert!(
        e.contains("--codex cannot be combined with --hook-only"),
        "{e}"
    );

    // Global-only targets
    let e = msg(run(
        false,
        false,
        true,
        false,
        false,
        false,
        false,
        false,
        false,
        PatchMode::Ask,
        ctx,
    ));
    assert!(e.contains("OpenCode plugin is global-only"), "{e}");
    let e = msg(run(
        false,
        false,
        false,
        true,
        false,
        false,
        false,
        false,
        false,
        PatchMode::Ask,
        ctx,
    ));
    assert!(e.contains("Cursor hooks are global-only"), "{e}");
    let e = msg(run(
        false,
        false,
        false,
        false,
        true,
        false,
        false,
        false,
        false,
        PatchMode::Ask,
        ctx,
    ));
    assert!(e.contains("Windsurf support is global-only"), "{e}");

    // No target at all
    let e = msg(run(
        true,
        false,
        false,
        false,
        false,
        false,
        false,
        false,
        false,
        PatchMode::Ask,
        ctx,
    ));
    assert!(
        e.contains("at least one of install_claude or install_opencode must be true"),
        "{e}"
    );
}

#[test]
fn write_if_changed_verbose_paths_create_update_and_skip() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("nested").join("file.md");

    assert!(write_if_changed(&path, "one", "file", verbose_ctx(true)).unwrap());
    assert!(!path.exists(), "dry-run must not create");

    assert!(write_if_changed(&path, "one", "file", verbose_ctx(false)).unwrap());
    assert_eq!(fs::read_to_string(&path).unwrap(), "one");

    assert!(!write_if_changed(&path, "one", "file", verbose_ctx(false)).unwrap());
    assert_eq!(fs::read_to_string(&path).unwrap(), "one");

    assert!(write_if_changed(&path, "two", "file", verbose_ctx(true)).unwrap());
    assert_eq!(fs::read_to_string(&path).unwrap(), "one");

    assert!(write_if_changed(&path, "two", "file", verbose_ctx(false)).unwrap());
    assert_eq!(fs::read_to_string(&path).unwrap(), "two");
}

#[test]
fn write_if_changed_handles_undecodable_existing_file() {
    let dir = TempDir::new().unwrap();
    let bad = dir.path().join("bad.md");
    let bytes = [0xffu8, 0xfe, 0x80, 0x00];
    fs::write(&bad, bytes).unwrap();

    let err = write_if_changed(&bad, "x", "bad file", InitContext::default()).unwrap_err();
    assert!(
        format!("{err:#}").contains("Failed to read bad file"),
        "{err:#}"
    );
    assert_eq!(fs::read(&bad).unwrap(), bytes);

    assert!(
        write_if_changed_allow_read_error(&bad, "fixed", "bad file", verbose_ctx(true)).unwrap()
    );
    assert_eq!(fs::read(&bad).unwrap(), bytes, "dry-run must not rewrite");

    assert!(
        write_if_changed_allow_read_error(&bad, "fixed", "bad file", verbose_ctx(false)).unwrap()
    );
    assert_eq!(fs::read_to_string(&bad).unwrap(), "fixed");
}

#[test]
fn parent_dir_and_atomic_write_error_paths() {
    let dir = TempDir::new().unwrap();

    ensure_parent_dir(Path::new("")).unwrap();

    let blocker = dir.path().join("blocker");
    fs::write(&blocker, "x").unwrap();
    let err = ensure_parent_dir(&blocker.join("sub").join("f.txt")).unwrap_err();
    assert!(
        format!("{err:#}").contains("Failed to create directory"),
        "{err:#}"
    );

    let err = atomic_write(Path::new(""), "x").unwrap_err();
    assert!(
        format!("{err:#}").contains("no parent directory"),
        "{err:#}"
    );

    let occupied = dir.path().join("occupied");
    fs::create_dir(&occupied).unwrap();
    let err = atomic_write(&occupied, "x").unwrap_err();
    assert!(
        format!("{err:#}").contains("Failed to atomically replace"),
        "{err:#}"
    );
    assert!(occupied.is_dir());
}

#[test]
fn update_json_file_dry_run_backup_line_and_failure() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("settings.json");
    fs::write(&path, "{\"old\":true}").unwrap();
    let backup = backup_path_for(&path);
    let root = serde_json::json!({"new": true});

    update_json_file(
        &path,
        &root,
        verbose_ctx(true),
        "settings",
        "[dry-run] would patch settings",
        true,
        Written::Backup,
    )
    .unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "{\"old\":true}");
    assert!(!backup.exists());

    update_json_file(
        &path,
        &root,
        verbose_ctx(false),
        "settings",
        "unused",
        false,
        Written::Backup,
    )
    .unwrap();
    let written: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(written, root);
    assert_eq!(fs::read_to_string(&backup).unwrap(), "{\"old\":true}");

    let third = serde_json::json!({"third": 1});
    update_json_file(
        &path,
        &third,
        verbose_ctx(false),
        "settings",
        "unused",
        false,
        Written::Line("Updated settings".to_string()),
    )
    .unwrap();
    let written: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(written, third);
    let backed_up: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&backup).unwrap()).unwrap();
    assert_eq!(backed_up, root);

    let orphan = dir.path().join("missing-dir").join("x.json");
    let err = update_json_file(
        &orphan,
        &root,
        InitContext::default(),
        "settings",
        "unused",
        false,
        Written::Backup,
    )
    .unwrap_err();
    assert!(
        format!("{err:#}").contains("Failed to update JSON file"),
        "{err:#}"
    );
    assert!(!orphan.exists());
}

#[test]
fn project_scope_footer_reports_each_state() {
    use crate::hooks::hook_check::HookStatus;
    assert_eq!(
        project_scope_footer_lines(HookStatus::Outdated, true, false),
        vec![
            "  hooks:     registered globally (outdated — run `rtk init -g`)".to_string(),
            "  proxy:     configured, daemon not running — `rtk llm-proxy start`".to_string(),
        ]
    );
    assert_eq!(
        project_scope_footer_lines(HookStatus::Ok, true, true),
        vec![
            "  hooks:     registered globally".to_string(),
            "  proxy:     configured and running".to_string(),
        ]
    );
    assert_eq!(
        project_scope_footer_lines(HookStatus::Missing, false, true),
        vec![
            "  hooks:     NOT registered — run `rtk init -g`".to_string(),
            "  proxy:     daemon running but nothing routed to it — `rtk init -g`".to_string(),
        ]
    );
}

#[cfg(feature = "engine")]
#[test]
fn offer_proxy_onramp_never_touches_settings_in_dry_run() {
    let dir = TempDir::new().unwrap();
    let settings = dir.path().join("settings.json");
    for mode in [PatchMode::Ask, PatchMode::Auto, PatchMode::Skip] {
        offer_proxy_onramp(
            &settings,
            mode,
            InitContext {
                dry_run: true,
                ..Default::default()
            },
        )
        .unwrap();
    }
    assert!(!settings.exists());
}

#[cfg(feature = "engine")]
fn resolve_user(toml: &str) -> rtk_engine::pipeline::Resolved {
    let parsed = rtk_engine::pipeline::parse(toml, "u.toml");
    let (pipeline, _diagnostics) =
        rtk_engine::pipeline::resolve(&[rtk_engine::pipeline::SourceFile {
            scope: rtk_engine::pipeline::Scope::User,
            label: "u.toml".to_string(),
            file: parsed.file.expect("pipeline parses"),
        }]);
    pipeline
}

#[cfg(feature = "engine")]
#[test]
fn instructions_body_skips_disabled_and_foreign_channel_directives() {
    let mut pipeline = resolve_user(
        r#"
schema_version = 1
[[directive]]
name = "cov-off"
text = "COVERAGE-DISABLED-TEXT"

[[directive]]
name = "cov-elsewhere"
text = "COVERAGE-ELSEWHERE-TEXT"

[override.rtk-awareness]
level = "full"
"#,
    );
    let mut seen = 0;
    for d in pipeline.directives.iter_mut() {
        if d.name == "cov-off" {
            d.def.enabled = false;
            seen += 1;
        } else if d.name == "cov-elsewhere" {
            d.def.channel = Some("not-awareness".into());
            seen += 1;
        }
    }
    assert_eq!(seen, 2, "both custom directives resolved");
    let body = instructions_body_for(AwarenessLevel::Default, &pipeline);
    assert!(!body.contains("COVERAGE-DISABLED-TEXT"));
    assert!(!body.contains("COVERAGE-ELSEWHERE-TEXT"));
    assert_eq!(body, awareness_content(AwarenessLevel::Full));
}

#[cfg(feature = "engine")]
#[test]
fn instructions_body_explicit_default_level_overrides_config() {
    let pipeline = resolve_user(
        r#"
schema_version = 1
[override.rtk-awareness]
level = "default"
"#,
    );
    let body = instructions_body_for(AwarenessLevel::Full, &pipeline);
    assert_eq!(body, awareness_content(AwarenessLevel::Default));
}

#[cfg(feature = "engine")]
#[test]
fn instructions_body_skips_directive_with_unreadable_file() {
    let dir = TempDir::new().unwrap();
    let missing = dir.path().join("absent-directive.md");
    let toml = format!(
        "schema_version = 1\n[[directive]]\nname = \"cov-missing\"\nfile = '{}'\n",
        missing.display()
    );
    let pipeline = resolve_user(&toml);
    let body = instructions_body_for(AwarenessLevel::High, &pipeline);
    assert_eq!(body, awareness_content(AwarenessLevel::High));
}
