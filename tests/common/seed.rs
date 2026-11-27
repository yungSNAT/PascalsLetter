//! Seeded data and stand-in services for the view tests: hook attribution in
//! the tracking database, the proxy's usage database and a running proxy.

use std::io::{Read, Write};
use std::path::Path;

/// Attribution rows for today in the tracking database at `db`:
/// `(site, kind, name, calls, tokens_in, tokens_out)`.
pub fn attribution(db: &Path, rows: &[(&str, &str, &str, i64, i64, i64)]) {
    let conn = rusqlite::Connection::open(db).expect("open tracking db");
    for (site, kind, name, calls, tokens_in, tokens_out) in rows {
        conn.execute(
            "INSERT INTO engine_attribution (day, site, kind, name, calls, tokens_in, tokens_out)
             VALUES (date('now'), ?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![site, kind, name, calls, tokens_in, tokens_out],
        )
        .expect("record attribution");
    }
}

/// The proxy's usage database with the current schema under `config`
/// (`~/.config/rtk`), filled by `rows`.
pub fn proxy_usage(config: &Path, rows: &str) {
    let dir = config.join("llm-proxy");
    std::fs::create_dir_all(&dir).expect("create proxy dir");
    rusqlite::Connection::open(dir.join("rtk-proxy.db"))
        .expect("open proxy db")
        .execute_batch(&format!(
            "CREATE TABLE usage_records (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                ts TEXT NOT NULL,
                tokens_in INTEGER NOT NULL DEFAULT 0,
                tokens_out INTEGER NOT NULL DEFAULT 0,
                tokens_saved INTEGER NOT NULL DEFAULT 0,
                cost_usd REAL NOT NULL DEFAULT 0,
                status INTEGER NOT NULL DEFAULT 200,
                cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
                ccr_retrieve_calls INTEGER NOT NULL DEFAULT 0,
                ctx_total INTEGER NOT NULL DEFAULT 0,
                ctx_tool_result INTEGER NOT NULL DEFAULT 0,
                ctx_tool_use INTEGER NOT NULL DEFAULT 0,
                ctx_duplicate INTEGER NOT NULL DEFAULT 0,
                ctx_image INTEGER NOT NULL DEFAULT 0,
                saved_ingest_filter INTEGER NOT NULL DEFAULT 0,
                saved_pixel_compress INTEGER NOT NULL DEFAULT 0,
                saved_ccr INTEGER NOT NULL DEFAULT 0,
                saved_normalize INTEGER NOT NULL DEFAULT 0,
                session TEXT,
                pipeline TEXT,
                routed_from TEXT,
                cost_without REAL NOT NULL DEFAULT 0,
                saved_engine_methods INTEGER NOT NULL DEFAULT 0,
                saved_rules TEXT
            );
            {rows}"
        ))
        .expect("seed proxy usage");
}

/// A loopback server that answers `/health` as the rtk proxy does, for as long
/// as the test runs; its address, for `RTK_PROXY_BIND`.
pub fn fake_proxy() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let addr = listener.local_addr().expect("address").to_string();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            let body = r#"{"service":"rtk-proxy"}"#;
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    addr
}
