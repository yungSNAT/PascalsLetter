use super::*;
use crate::core::test_isolation;
use crate::core::tracking::AttributionRow;
use rtk_engine::pipeline::{Scope, SourceFile, parse, resolve};

fn row(name: &str, calls: i64, tokens_in: i64, tokens_out: i64) -> AttributionRow {
    AttributionRow {
        name: name.to_string(),
        calls,
        tokens_in,
        tokens_out,
    }
}

fn card(
    what: Vec<(String, String, String)>,
    hook: Option<AttributionRow>,
    slices: Vec<(String, i64, i64, i64)>,
    tasks: Vec<AttributionRow>,
    proxy: Option<crate::llm_proxy::ProxyPipelineRow>,
    published: Option<&str>,
) -> CardData {
    CardData {
        display: "cov-card".to_string(),
        is_active: true,
        sources: vec![(
            "user cov-card.toml".to_string(),
            "[pipeline]\nname = \"cov-card\"\n".to_string(),
        )],
        what,
        filtering: None,
        hook,
        slices,
        tasks,
        proxy,
        published: published.map(str::to_string),
        date: "2026-01-02".to_string(),
    }
}

#[test]
fn headline_prefers_hooks_then_proxy_cache_then_nothing() {
    let proxy = || crate::llm_proxy::ProxyPipelineRow {
        name: "cov-card".into(),
        requests: 7,
        tokens_in: 450,
        tokens_saved: 1050,
        saved_ccr: 700,
        saved_normalize: 350,
        saved_ingest_filter: 0,
        saved_pixel_compress: 0,
        saved_rules: 0,
        cache_read: 4000,
        cache_created: 1000,
    };

    let with_hook = card(
        vec![],
        Some(row("cov-card", 42, 1000, 300)),
        vec![],
        vec![],
        Some(proxy()),
        None,
    );
    assert_eq!(
        with_hook.headline().as_deref(),
        Some("70% of tool output removed")
    );

    // A hook that barely moved falls through to the proxy's cache figure.
    let weak_hook = card(
        vec![],
        Some(row("cov-card", 1, 1000, 996)),
        vec![],
        vec![],
        Some(proxy()),
        None,
    );
    assert_eq!(
        weak_hook.headline().as_deref(),
        Some("80% prompt-cache hit")
    );

    let mut no_cache = proxy();
    no_cache.cache_read = 0;
    no_cache.cache_created = 0;
    let empty_cache = card(vec![], None, vec![], vec![], Some(no_cache), None);
    assert_eq!(empty_cache.headline(), None);

    let nothing = card(vec![], None, vec![], vec![], None, None);
    assert_eq!(nothing.headline(), None);
}

#[test]
fn terminal_and_markdown_cards_render_every_lever() {
    let long_when = "file_lines_gt=1200, ".repeat(10);
    let full = card(
        vec![
            (
                "before read".to_string(),
                "delegate -> bulk-read".to_string(),
                long_when.clone(),
            ),
            (
                "after shell".to_string(),
                "compress [dedup]".to_string(),
                String::new(),
            ),
        ],
        Some(row("cov-card", 42, 1000, 300)),
        vec![
            ("shell".to_string(), 3, 1000, 700),
            ("reads".to_string(), 1, 400, 160),
            ("web".to_string(), 2, 1000, 100),
            ("idle".to_string(), 0, 0, 0),
        ],
        vec![row("bulk-read", 5, 20000, 500)],
        Some(crate::llm_proxy::ProxyPipelineRow {
            name: "cov-card".into(),
            requests: 7,
            tokens_in: 450,
            tokens_saved: 1050,
            saved_ccr: 700,
            saved_normalize: 350,
            saved_ingest_filter: 0,
            saved_pixel_compress: 0,
            saved_rules: 0,
            cache_read: 4000,
            cache_created: 1000,
        }),
        Some("github:alice/pipes"),
    );
    let md = full.card_md();
    assert!(md.starts_with("# rtk pipeline · cov-card\n"), "{md}");
    assert!(
        md.contains("| · shell | 3 outputs | `███████░░░` **70%** removed |"),
        "{md}"
    );
    assert!(
        md.contains("| · reads | 1 outputs | `████░░░░░░` **40%** removed |"),
        "{md}"
    );
    assert!(!md.contains("· idle"), "{md}");
    assert!(
        md.contains("| delegated reads (`bulk-read`) | 5 reads |"),
        "{md}"
    );
    assert!(md.contains(&format!("when {long_when}")), "{md}");
    assert_eq!(
        full.headline().as_deref(),
        Some("70% of tool output removed")
    );

    let hook_only = card(
        vec![],
        Some(row("cov-card", 2, 100, 10)),
        vec![],
        vec![],
        None,
        None,
    );
    assert!(
        hook_only
            .card_md()
            .contains("| agent hooks (last 30d) | 2 outputs |")
    );

    let empty = card(vec![], None, vec![], vec![], None, None);
    let md = empty.card_md();
    assert!(md.contains("No recorded activity yet"), "{md}");
    assert!(!md.contains("## What it does"), "{md}");
}

#[test]
fn describe_rules_names_deny_and_unknown_actions() {
    let text = "[pipeline]\nname = \"t\"\n\n[[hook.before_tool.shell]]\naction = \"deny\"\n\n[[hook.before_tool.read]]\naction = \"custom-action\"\n";
    let parsed = parse(text, "t.toml");
    let (resolved, _) = resolve(&[SourceFile {
        scope: Scope::User,
        label: "t.toml".to_string(),
        file: parsed.file.expect("parses"),
    }]);
    let what = describe_rules(&resolved);
    assert!(
        what.iter()
            .any(|(p, d, w)| p.contains("shell") && d == "block" && w.is_empty()),
        "{what:?}"
    );
    assert!(
        what.iter()
            .any(|(p, d, _)| p.contains("read") && d == "action custom-action"),
        "{what:?}"
    );
}

#[test]
fn run_test_and_share_follow_the_installed_user_pipeline() {
    let root = test_isolation::tempdir();
    let work = test_isolation::tempdir();
    let _cwd = test_isolation::enter(work.path());
    test_isolation::with_root(root.path(), || {
        assert!(pipeline_config::load_paths().is_empty());
        assert_eq!(run_test().unwrap(), 0);

        let dir = pipeline_config::user_pipelines_dir().unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        let text =
            "schema_version = 1\n\n[pipeline]\nname = \"cov-mine\"\nextends = \"rtk:default\"\n";
        std::fs::write(dir.join("cov-mine.toml"), text).unwrap();
        assert_eq!(pipeline_config::load_paths().len(), 1);
        assert_eq!(run_test().unwrap(), 0);

        let card = collect_card(Some("cov-mine")).unwrap().unwrap();
        assert!(card.is_active);
        assert_eq!(card.display, "cov-mine");
        assert_eq!(
            card.sources,
            vec![("user cov-mine.toml".to_string(), text.to_string())]
        );
        assert!(card.card_md().contains("## user cov-mine.toml"));

        assert!(collect_card(Some("cov-not-a-pipeline")).unwrap().is_none());
        assert_eq!(
            run_share(Some("cov-not-a-pipeline"), true, false, false).unwrap(),
            1
        );
        assert_eq!(run_share(Some("cov-mine"), true, false, false).unwrap(), 0);

        std::fs::write(dir.join("broken.toml"), "[pipeline\nname = ").unwrap();
        assert!(has_errors(&load_for_cli().diagnostics));
        assert_eq!(run_test().unwrap(), 1);
    });
}
