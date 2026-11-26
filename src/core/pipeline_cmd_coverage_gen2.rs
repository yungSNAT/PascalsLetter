use super::*;

fn empty_stats() -> CardStats<'static> {
    CardStats {
        what: &[],
        filtering: None,
        hook: None,
        hook_slices: &[],
        tasks: &[],
        proxy: None,
    }
}

fn proxy_row(
    tokens_in: i64,
    tokens_saved: i64,
    cache_read: i64,
    cache_created: i64,
) -> crate::llm_proxy::ProxyPipelineRow {
    crate::llm_proxy::ProxyPipelineRow {
        name: "cov-pipe".into(),
        requests: 9,
        tokens_in,
        tokens_saved,
        saved_ccr: 0,
        saved_normalize: 0,
        saved_ingest_filter: 0,
        saved_pixel_compress: 0,
        cache_read,
        cache_created,
    }
}

#[test]
fn share_card_renders_zero_hooks_tasks_small_proxy_and_unterminated_source() {
    let hook = crate::core::tracking::AttributionRow {
        name: "cov-pipe".into(),
        calls: 5,
        tokens_in: 0,
        tokens_out: 0,
    };
    let task = crate::core::tracking::AttributionRow {
        name: "bulk-read".into(),
        calls: 3,
        tokens_in: 5000,
        tokens_out: 200,
    };
    let tasks = vec![task];
    let slices = vec![
        ("empty slice".to_string(), 2, 0, 0),
        ("file reads".to_string(), 1, 400, 200),
    ];
    // 10 / 10010 is well under 1%: no proxy input row; no cache prefix either.
    let proxy = proxy_row(10_000, 10, 0, 0);
    let sources = vec![("user cov.toml".to_string(), "x = 1".to_string())];
    let card = share_card(
        "cov-pipe",
        &sources,
        &CardStats {
            what: &[],
            filtering: None,
            hook: Some(&hook),
            hook_slices: &slices,
            tasks: &tasks,
            proxy: Some(&proxy),
        },
        "9.9.9",
        "2026-01-01",
    );
    assert!(card.contains("## Measured impact"), "{card}");
    assert!(
        card.contains("| agent hooks (last 30d) | 5 outputs | `░░░░░░░░░░` **0%** removed |"),
        "{card}"
    );
    assert!(!card.contains("empty slice"), "{card}");
    assert!(
        card.contains("| · file reads | 1 outputs | `█████░░░░░` **50%** removed |"),
        "{card}"
    );
    let kept = crate::core::utils::format_tokens(4800);
    assert!(
        card.contains(&format!(
            "| delegated reads (`bulk-read`) | 3 reads | **{kept}** kept out of context |"
        )),
        "{card}"
    );
    assert!(!card.contains("llm proxy input"), "{card}");
    assert!(!card.contains("prompt-cache hit rate"), "{card}");
    // A source without a trailing newline still gets its fence on its own line.
    assert!(card.contains("```toml\nx = 1\n```\n"), "{card}");
    assert!(
        card.contains("rtk pipeline add <url-of-this-card>"),
        "{card}"
    );

    // No sources: no install hint of either kind.
    let bare = share_card("bare", &[], &empty_stats(), "9.9.9", "2026-01-01");
    assert!(bare.contains("No recorded activity yet"), "{bare}");
    assert!(bare.ends_with("v9.9.9 on 2026-01-01.\n"), "{bare}");
    assert!(!bare.contains("Install this pipeline"), "{bare}");
    assert!(!bare.contains("multiple configuration sources"), "{bare}");
}

#[test]
fn terminal_card_renders_every_panel_variant_without_panicking() {
    let hook = crate::core::tracking::AttributionRow {
        name: "cov-pipe".into(),
        calls: 4,
        tokens_in: 1000,
        tokens_out: 300,
    };
    let tasks = vec![crate::core::tracking::AttributionRow {
        name: "a-very-long-task-name-that-needs-truncating-in-the-panel".into(),
        calls: 2,
        tokens_in: 900,
        tokens_out: 100,
    }];
    let slices = vec![
        ("skipped".to_string(), 1, 0, 0),
        ("shell output".to_string(), 3, 1000, 400),
        ("file reads".to_string(), 1, 1000, 100),
    ];
    let what = vec![(
        "before read".to_string(),
        "delegate -> bulk-read".to_string(),
        "file_lines_gt=1200, and a long tail of conditions that must be truncated".to_string(),
    )];
    let proxy = proxy_row(450, 1050, 4000, 1000);
    let stats = CardStats {
        what: &what,
        filtering: None,
        hook: Some(&hook),
        hook_slices: &slices,
        tasks: &tasks,
        proxy: Some(&proxy),
    };
    let card = terminal_card("cov-pipe", &stats, None, "9.9.9", "2026-01-01").join("\n");
    assert!(card.contains("rtk pipeline · cov-pipe"), "{card}");
    assert!(card.contains("rtk pipeline share publish"), "{card}");
    let card = terminal_card(
        "cov-pipe",
        &stats,
        Some("github:alice/cov-pipe"),
        "9.9.9",
        "2026-01-01",
    )
    .join("\n");
    assert!(card.contains("https://github.com/alice/cov-pipe"), "{card}");
    assert!(
        card.contains("rtk pipeline add github:alice/cov-pipe"),
        "{card}"
    );
    let card = terminal_card(
        "cov-pipe",
        &empty_stats(),
        Some("https://example.invalid/card"),
        "9.9.9",
        "2026-01-01",
    )
    .join("\n");
    assert!(!card.contains("live at:"), "{card}");
    assert!(
        card.contains("rtk pipeline add https://example.invalid/card"),
        "{card}"
    );
    let card = terminal_card("cov-pipe", &empty_stats(), None, "9.9.9", "2026-01-01").join("\n");
    assert!(card.contains("one command:"), "{card}");

    // The terminal card and the markdown card draw from the same stats.
    let md = share_card("cov-pipe", &[], &stats, "9.9.9", "2026-01-01");
    assert!(
        md.contains("| llm proxy input (all time) | 9 requests | `███████░░░` **70%** removed |"),
        "{md}"
    );
    assert!(
        md.contains("| prompt-cache hit rate | 9 requests | `████████░░` **80%** cached |"),
        "{md}"
    );
    assert!(!md.contains("skipped"), "{md}");
}
