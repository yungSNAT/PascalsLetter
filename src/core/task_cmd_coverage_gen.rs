use super::*;
use std::path::Path;

/// Serves one canned JSON reply on an ephemeral loopback port and returns the
/// (lower-cased) request it received.
fn capture(reply: &'static str) -> (String, std::thread::JoinHandle<String>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let base = format!("http://{}", listener.local_addr().expect("local addr"));
    let handle = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().expect("accept");
        let mut request = Vec::new();
        let mut buffer = [0u8; 4096];
        loop {
            let count = socket.read(&mut buffer).expect("read request");
            request.extend_from_slice(&buffer[..count]);
            if count == 0 {
                break;
            }
            let text = String::from_utf8_lossy(&request).to_ascii_lowercase();
            let Some(end) = text.find("\r\n\r\n") else {
                continue;
            };
            let length = text[..end]
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .map_or(0, |value| value.trim().parse::<usize>().unwrap_or(0));
            if request.len() >= end + 4 + length {
                break;
            }
        }
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
            reply.len()
        );
        socket
            .write_all(response.as_bytes())
            .expect("write response");
        String::from_utf8_lossy(&request).to_ascii_lowercase()
    });
    (base, handle)
}

/// The JSON body of a captured request.
fn body_of(seen: &str) -> serde_json::Value {
    let start = seen.find("\r\n\r\n").expect("headers end") + 4;
    serde_json::from_str(&seen[start..]).expect("json body")
}

fn slug(path: &Path) -> String {
    path.to_str()
        .expect("utf-8 path")
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect()
}

fn keyed_upstream(provider: &str, base: &str) -> UpstreamDef {
    UpstreamDef {
        provider: provider.to_string(),
        model: "worker-model".to_string(),
        base_url: Some(base.to_string()),
        api_key_command: Some("echo k".to_string()),
        timeout_secs: Some(5),
        ..UpstreamDef::default()
    }
}

fn request() -> WorkerRequest {
    WorkerRequest {
        system: "sys".into(),
        user: "usr".into(),
        max_tokens: 16,
    }
}

#[test]
fn default_key_env_names_each_providers_variable() {
    assert_eq!(default_key_env("anthropic"), Some("ANTHROPIC_API_KEY"));
    assert_eq!(default_key_env("openai"), Some("OPENAI_API_KEY"));
    assert_eq!(default_key_env("google"), Some("GEMINI_API_KEY"));
    assert_eq!(default_key_env("nonesuch"), None);
}

#[test]
fn an_upstream_without_any_credential_source_is_refused() {
    let upstream = UpstreamDef {
        provider: "nonesuch".to_string(),
        ..UpstreamDef::default()
    };
    let err = api_key(&upstream).expect_err("no credential source");
    assert!(
        err.to_string().contains("upstream has no credential"),
        "{err}"
    );
    let keyed = UpstreamDef {
        api_key_command: Some("echo k".to_string()),
        ..UpstreamDef::default()
    };
    assert_eq!(
        borrowable_api_key(&keyed, false).expect("key resolves"),
        Some("k".to_string())
    );
}

#[test]
fn an_unknown_provider_is_an_error() {
    let upstream = UpstreamDef {
        provider: "bogus".to_string(),
        ..UpstreamDef::default()
    };
    let err = call_worker(&upstream, &request(), None).expect_err("unknown provider");
    assert!(
        err.to_string().contains("unknown provider `bogus`"),
        "{err}"
    );
}

#[test]
fn read_corpus_reads_regular_files() {
    let dir = crate::core::test_isolation::tempdir();
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "alpha\nbeta\n").expect("write");
    let path = file.display().to_string();
    let corpus = read_corpus(std::slice::from_ref(&path)).expect("reads");
    assert_eq!(corpus.len(), 1);
    assert_eq!(corpus[0].path, path);
    assert_eq!(corpus[0].content, "alpha\nbeta\n");
}

#[test]
fn a_missing_program_fails_to_run() {
    let err = run_for_output(&["rtk-coverage-missing-program-0x1".to_string()])
        .expect_err("no such program");
    assert!(format!("{err:#}").contains("Failed to run"), "{err:#}");
}

#[cfg(unix)]
#[test]
fn output_without_trailing_newlines_is_separated() {
    let (captured, code) = run_for_output(&[
        "sh".to_string(),
        "-c".to_string(),
        "printf out; printf err 1>&2".to_string(),
    ])
    .expect("runs");
    assert_eq!(code, 0);
    assert_eq!(captured.content, "out\nerr\n[exit status 0]");
}

#[test]
fn last_assistant_model_takes_the_newest_real_model() {
    let dir = crate::core::test_isolation::tempdir();
    let path = dir.path().join("session.jsonl");
    std::fs::write(
        &path,
        concat!(
            "not json\n",
            "{\"message\":{\"model\":\"first-model\"}}\n",
            "{\"message\":{\"model\":\"<synthetic>\"}}\n",
            "{\"message\":{\"model\":\"\"}}\n",
            "{\"message\":{\"model\":\"second-model\"}}\n",
            "{\"message\":{\"role\":\"user\",\"content\":\"hi\"}}\n",
        ),
    )
    .expect("write transcript");
    assert_eq!(last_assistant_model(&path).as_deref(), Some("second-model"));
    let empty = dir.path().join("empty.jsonl");
    std::fs::write(&empty, "{\"type\":\"user\"}\n").expect("write");
    assert_eq!(last_assistant_model(&empty), None);
    assert_eq!(
        last_assistant_model(&dir.path().join("missing.jsonl")),
        None
    );
}

#[test]
fn an_unconfigured_task_is_refused_and_listing_succeeds() {
    let root = crate::core::test_isolation::tempdir();
    let work = root.path().join("work");
    std::fs::create_dir_all(&work).expect("work dir");
    crate::core::test_isolation::with_root(root.path(), || {
        let _guard = crate::core::test_isolation::enter(&work);
        let err = run_task("rtk-coverage-no-such-task", &[]).expect_err("not configured");
        assert_eq!(
            err.to_string(),
            "`rtk-coverage-no-such-task` is not a configured task"
        );
        assert_eq!(run_tasks_list().expect("lists"), 0);
    });
}

#[test]
fn an_anthropic_worker_via_the_daemon_sends_key_temperature_and_displaced_model() {
    let root = crate::core::test_isolation::tempdir();
    let work = root.path().join("work");
    std::fs::create_dir_all(&work).expect("work dir");
    let custom_claude = root.path().join("custom_claude");
    let mut slugs = vec![slug(&work)];
    if let Ok(canonical) = std::fs::canonicalize(&work)
        && let Some(s) = canonical.to_str().map(|_| slug(&canonical))
    {
        slugs.push(s);
    }
    for s in &slugs {
        let project = custom_claude.join("projects").join(s);
        std::fs::create_dir_all(&project).expect("project dir");
        std::fs::write(
            project.join("notes.txt"),
            "{\"message\":{\"model\":\"nope\"}}\n",
        )
        .expect("write notes");
        std::fs::write(
            project.join("session.jsonl"),
            "{\"message\":{\"role\":\"user\"}}\n{\"message\":{\"model\":\"claude-opus-cov\"}}\n",
        )
        .expect("write transcript");
    }

    crate::core::test_isolation::with_agent_dir(
        root.path(),
        "CLAUDE_CONFIG_DIR",
        &custom_claude,
        || {
            let _guard = crate::core::test_isolation::enter(&work);
            assert_eq!(displaced_model().as_deref(), Some("claude-opus-cov"));

            let (base, seen) = capture(
                r#"{"content":[{"text":"hel"},{"text":"lo"}],"usage":{"input_tokens":7,"output_tokens":3}}"#,
            );
            let answer = call_worker(&keyed_upstream("anthropic", &base), &request(), Some(0.5))
                .expect("anthropic worker");
            assert_eq!(answer, ("hello".to_string(), 7, 3));
            let seen = seen.join().expect("server");
            assert!(seen.contains("\r\nx-api-key: k\r\n"), "{seen}");
            assert!(seen.contains("\r\nx-rtk-proxy: 1\r\n"), "{seen}");
            assert!(
                seen.contains("\r\nx-rtk-routed-from: claude-opus-cov\r\n"),
                "{seen}"
            );
            let body = body_of(&seen);
            assert_eq!(body["temperature"], serde_json::json!(0.5), "{seen}");
            assert_eq!(body["max_tokens"], serde_json::json!(16), "{seen}");
        },
    );
}

#[test]
fn an_openai_worker_sends_bearer_key_and_temperature() {
    let root = crate::core::test_isolation::tempdir();
    let work = root.path().join("work");
    std::fs::create_dir_all(&work).expect("work dir");
    let custom_claude = root.path().join("custom_claude");
    std::fs::create_dir_all(&custom_claude).expect("claude dir");

    crate::core::test_isolation::with_agent_dir(
        root.path(),
        "CLAUDE_CONFIG_DIR",
        &custom_claude,
        || {
            let _guard = crate::core::test_isolation::enter(&work);
            let (base, seen) = capture(
                r#"{"choices":[{"message":{"content":"answer"}}],"usage":{"prompt_tokens":4,"completion_tokens":2}}"#,
            );
            let answer = call_worker(&keyed_upstream("openai", &base), &request(), Some(0.25))
                .expect("openai worker");
            assert_eq!(answer, ("answer".to_string(), 4, 2));
            let seen = seen.join().expect("server");
            assert!(seen.contains("\r\nauthorization: bearer k\r\n"), "{seen}");
            assert!(seen.contains("\r\nx-rtk-proxy: 1\r\n"), "{seen}");
            assert!(!seen.contains("x-rtk-routed-from"), "{seen}");
            let body = body_of(&seen);
            assert_eq!(body["temperature"], serde_json::json!(0.25), "{seen}");
            assert_eq!(
                body["max_completion_tokens"],
                serde_json::json!(16),
                "{seen}"
            );

            // A worker that cannot be reached is a transport error.
            let port = {
                let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
                listener.local_addr().expect("addr").port()
            };
            let closed = format!("http://127.0.0.1:{port}");
            let err = call_worker(&keyed_upstream("openai", &closed), &request(), None)
                .expect_err("nothing listens there");
            assert!(err.to_string().starts_with("worker call failed:"), "{err}");
        },
    );
}
