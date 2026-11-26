use super::*;

fn seed_usage_db(path: &Path) {
    let conn = rusqlite::Connection::open(path).expect("open db");
    conn.execute_batch(
        "CREATE TABLE usage_records (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            ts TEXT NOT NULL,
            tokens_in INTEGER NOT NULL DEFAULT 0,
            tokens_out INTEGER NOT NULL DEFAULT 0,
            tokens_saved INTEGER NOT NULL DEFAULT 0,
            cost_usd REAL NOT NULL DEFAULT 0,
            status INTEGER NOT NULL DEFAULT 0,
            session TEXT,
            ctx_total INTEGER NOT NULL DEFAULT 0,
            ctx_tool_result INTEGER NOT NULL DEFAULT 0,
            ctx_tool_use INTEGER NOT NULL DEFAULT 0,
            ctx_duplicate INTEGER NOT NULL DEFAULT 0,
            ctx_image INTEGER NOT NULL DEFAULT 0,
            saved_ingest_filter INTEGER NOT NULL DEFAULT 0,
            saved_pixel_compress INTEGER NOT NULL DEFAULT 0,
            saved_ccr INTEGER NOT NULL DEFAULT 0,
            saved_normalize INTEGER NOT NULL DEFAULT 0,
            cache_read_tokens INTEGER NOT NULL DEFAULT 0,
            cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
            ccr_retrieve_calls INTEGER NOT NULL DEFAULT 0,
            pipeline TEXT
        );
        INSERT INTO usage_records
            (ts, tokens_in, tokens_out, tokens_saved, status, saved_ccr, saved_normalize,
             cache_read_tokens, cache_creation_tokens, pipeline)
        VALUES ('2026-01-01', 100, 10, 30, 200, 20, 10, 500, 100, 'p1'),
               ('2026-01-02', 200, 20, 0, 200, 0, 0, 0, 0, NULL);",
    )
    .expect("seed db");
}

fn with_seeded_config<F: FnOnce()>(f: F) {
    let dir = crate::core::test_isolation::tempdir();
    crate::core::test_isolation::with_root(dir.path(), || {
        let cfg = crate::core::user_dirs::config().expect("config dir");
        let state = cfg.join("llm-proxy");
        std::fs::create_dir_all(&state).expect("mkdir");
        seed_usage_db(&state.join("rtk-proxy.db"));
        f();
    });
}

#[test]
fn cache_effect_pct_weighs_reads_and_writes() {
    let none = ProxyGainSummary {
        tokens_in: 100,
        ..Default::default()
    };
    assert!(none.cache_effect_pct().is_none());
    assert!(ProxyGainSummary::default().cache_effect_pct().is_none());

    let s = ProxyGainSummary {
        tokens_in: 100,
        cache_read: 100,
        cache_created: 0,
        ..Default::default()
    };
    let pct = s.cache_effect_pct().expect("some");
    assert!((pct - 45.0).abs() < 1e-9, "{pct}");

    let w = ProxyGainSummary {
        tokens_in: 0,
        cache_read: 0,
        cache_created: 100,
        ..Default::default()
    };
    let pct = w.cache_effect_pct().expect("some");
    assert!((pct + 25.0).abs() < 1e-9, "{pct}");
}

#[test]
fn approx_usd_rounds_by_magnitude() {
    assert_eq!(approx_usd(-1.0), "<$0.001");
    assert_eq!(approx_usd(0.0001), "<$0.001");
    assert_eq!(approx_usd(0.05), "~$0.050");
    assert_eq!(approx_usd(1.234), "~$1.23");
}

#[test]
fn pipeline_row_savings_by_method_drops_zeroes() {
    let row = ProxyPipelineRow {
        name: "x".to_string(),
        requests: 1,
        tokens_in: 0,
        tokens_saved: 0,
        saved_ccr: 0,
        saved_normalize: 4,
        saved_ingest_filter: 0,
        saved_pixel_compress: 9,
        cache_read: 0,
        cache_created: 0,
    };
    assert_eq!(
        row.savings_by_method(),
        vec![("normalize", 4), ("pixel-compress", 9)]
    );
}

#[test]
fn savings_by_pipeline_reads_the_config_db() {
    with_seeded_config(|| {
        let rows = savings_by_pipeline().expect("rows");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "p1");
        assert_eq!(rows[0].tokens_saved, 30);
        assert_eq!(
            rows[0].savings_by_method(),
            vec![("ccr", 20), ("normalize", 10)]
        );
        assert_eq!(rows[1].name, "");

        let s = read_gain_summary().expect("summary");
        assert_eq!(
            (s.total_requests, s.tokens_in, s.tokens_saved),
            (2, 300, 30)
        );
        assert_eq!(s.trimmed_requests, 1);
        assert_eq!((s.cache_read, s.cache_created), (500, 100));
    });
}

#[cfg(feature = "engine")]
#[test]
fn daemon_summary_as_pipeline_row() {
    with_seeded_config(|| {
        let row = read_gain_summary_as_pipeline().expect("row");
        assert_eq!(row.name, "llm proxy (daemon)");
        assert_eq!(
            (row.requests, row.tokens_in, row.tokens_saved),
            (2, 300, 30)
        );
        assert_eq!((row.saved_ccr, row.saved_normalize), (20, 10));
        assert_eq!((row.cache_read, row.cache_created), (500, 100));
        assert_eq!((row.saved_ingest_filter, row.saved_pixel_compress), (0, 0));
    });
}

#[test]
fn setup_reports_a_missing_binary() {
    let located = locate_binary();
    let result = run_setup();
    match located {
        Ok(_) => assert!(result.is_ok()),
        Err(_) => {
            let msg = format!("{:#}", result.expect_err("missing binary must fail"));
            assert!(msg.contains(PROXY_BIN), "{msg}");
            assert!(msg.contains("not found"), "{msg}");
        }
    }
}

#[test]
fn run_agent_without_the_tool_returns_127() {
    let agent = ProxyAgent {
        name: "rtk-coverage-no-such-agent-binary",
        base_url_var: "RTK_COVERAGE_TEST_BASE_URL",
    };
    let code = run_agent(&agent, &["--version".to_string()]).expect("no error");
    assert_eq!(code, 127);
}

#[test]
fn start_is_a_noop_when_the_daemon_already_answers() {
    use crate::core::http::test_server::serve_once;

    let (port, server) = serve_once(r#"{"status":"ok","service":"rtk-proxy"}"#);
    let addr = format!("127.0.0.1:{port}");
    let started = ensure_running(&addr).expect("already running");
    assert!(started.is_none());
    server.join().expect("server");

    let (port, server) = serve_once(r#"{"status":"ok","service":"rtk-proxy"}"#);
    let addr = format!("127.0.0.1:{port}");
    run(&LlmProxySubcommand::Start {
        bind: Some(addr),
        env: true,
    })
    .expect("env-only start succeeds");
    server.join().expect("server");

    let (port, server) = serve_once(r#"{"status":"ok","service":"rtk-proxy"}"#);
    let addr = format!("127.0.0.1:{port}");
    run_start(Some(&addr), false).expect("already running start succeeds");
    server.join().expect("server");
}

#[test]
fn no_data_hint_when_the_daemon_is_reachable() {
    use crate::core::http::test_server::serve_once;

    let (port, server) = serve_once(r#"{"status":"ok","service":"rtk-proxy"}"#);
    let addr = format!("127.0.0.1:{port}");
    crate::core::user_env::with_vars(&[("RTK_PROXY_BIND", Some(addr.as_str()))], || {
        let hint = no_data_hint();
        assert!(hint.contains(&format!("running on {addr}")), "{hint}");
        assert!(!hint.contains("llm-proxy start"), "{hint}");
        for agent in PROXY_AGENTS {
            assert!(hint.contains(agent.name), "{hint}");
        }
    });
    server.join().expect("server");
}
