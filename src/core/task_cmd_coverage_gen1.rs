use super::*;

/// Serves one canned JSON reply on loopback and returns the request it received,
/// lower-cased, once the reply has been written.
fn capture(reply: &'static str) -> (String, std::thread::JoinHandle<String>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let base = format!("http://{}", listener.local_addr().expect("local addr"));
    let handle = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().expect("accept");
        let mut request = Vec::new();
        let mut buffer = [0; 4096];
        loop {
            let count = socket.read(&mut buffer).expect("read request");
            request.extend_from_slice(&buffer[..count]);
            let text = String::from_utf8_lossy(&request).to_ascii_lowercase();
            if let Some(end) = text.find("\r\n\r\n") {
                let length = text[..end]
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .map_or(0, |value| value.trim().parse::<usize>().unwrap_or(0));
                if request.len() >= end + 4 + length {
                    break;
                }
            }
            if count == 0 {
                break;
            }
        }
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
            reply.len()
        );
        socket.write_all(response.as_bytes()).expect("write reply");
        String::from_utf8_lossy(&request).to_ascii_lowercase()
    });
    (base, handle)
}

/// The request with all whitespace removed, so body checks do not depend on
/// how the JSON was formatted.
fn compact(seen: &str) -> String {
    seen.chars().filter(|c| !c.is_whitespace()).collect()
}

fn request() -> WorkerRequest {
    WorkerRequest {
        system: "sys".into(),
        user: "usr".into(),
        max_tokens: 9,
    }
}

fn google_upstream(base: &str) -> UpstreamDef {
    UpstreamDef {
        provider: "google".to_string(),
        model: "gem-model".to_string(),
        base_url: Some(base.to_string()),
        api_key_command: Some("echo google-key".to_string()),
        ..UpstreamDef::default()
    }
}

#[test]
fn google_worker_sends_key_and_config_and_joins_parts() {
    let (base, seen) = capture(
        r#"{"candidates":[{"content":{"parts":[{"text":"hel"},{"text":"lo"}]}}],"usageMetadata":{"promptTokenCount":7,"candidatesTokenCount":3}}"#,
    );
    let (text, tokens_in, tokens_out) =
        call_google(&google_upstream(&base), &request(), Some(0.5)).expect("google worker");
    let seen = seen.join().expect("server");
    assert_eq!(text, "hello");
    assert_eq!(tokens_in, 7);
    assert_eq!(tokens_out, 3);
    assert!(
        seen.contains("/v1beta/models/gem-model:generatecontent"),
        "{seen}"
    );
    assert!(
        seen.contains("\r\nx-goog-api-key: google-key\r\n"),
        "{seen}"
    );
    let body = compact(&seen);
    assert!(body.contains("\"temperature\":0.5"), "{seen}");
    assert!(body.contains("\"maxoutputtokens\":9"), "{seen}");
}

#[test]
fn google_worker_with_unexpected_reply_yields_empty_answer() {
    let (base, seen) = capture(r#"{"candidates":[]}"#);
    let (text, tokens_in, tokens_out) =
        call_google(&google_upstream(&base), &request(), None).expect("google worker");
    let seen = seen.join().expect("server");
    assert_eq!(text, "");
    assert_eq!((tokens_in, tokens_out), (0, 0));
    assert!(!seen.contains("\"temperature\""), "{seen}");
}

#[test]
fn openai_through_the_daemon_names_the_displaced_model() {
    let root = crate::core::test_isolation::tempdir();
    let work = root.path().join("work.dir");
    std::fs::create_dir_all(&work).expect("work dir");
    let claude = root.path().join("claude");

    let mut cwds = vec![work.clone()];
    if let Ok(canonical) = std::fs::canonicalize(&work) {
        cwds.push(canonical);
    }
    for cwd in &cwds {
        let slug: String = cwd
            .to_str()
            .expect("utf-8 path")
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect();
        let project = claude.join("projects").join(slug);
        std::fs::create_dir_all(&project).expect("project dir");
        std::fs::write(
            project.join("session.jsonl"),
            "{\"message\":{\"model\":\"claude-opus-test\"}}\n{\"message\":{\"model\":\"<synthetic>\"}}\nnot json\n",
        )
        .expect("transcript");
    }

    crate::core::test_isolation::with_agent_dir(root.path(), "CLAUDE_CONFIG_DIR", &claude, || {
        let _guard = crate::core::test_isolation::enter(&work);
        let (base, seen) = capture(
            r#"{"choices":[{"message":{"content":"ok"}}],"usage":{"prompt_tokens":4,"completion_tokens":2}}"#,
        );
        let upstream = UpstreamDef {
            provider: "openai".to_string(),
            model: "m".to_string(),
            base_url: Some(base),
            api_key_env: Some("RTK_TEST_KEY_THAT_IS_NOT_SET".to_string()),
            ..UpstreamDef::default()
        };
        let (text, tokens_in, tokens_out) =
            call_openai(&upstream, &request(), Some(0.25)).expect("openai worker");
        let seen = seen.join().expect("server");
        assert_eq!(text, "ok");
        assert_eq!((tokens_in, tokens_out), (4, 2));
        assert!(seen.contains("\r\nx-rtk-proxy: 1\r\n"), "{seen}");
        assert!(
            seen.contains("\r\nx-rtk-routed-from: claude-opus-test\r\n"),
            "{seen}"
        );
    });
}

#[test]
fn answer_with_returns_the_trimmed_worker_answer_or_nothing() {
    let root = crate::core::test_isolation::tempdir();
    crate::core::test_isolation::with_root(root.path(), || {
        let corpus = vec![CorpusFile {
            path: "build.log".to_string(),
            content: "corpus-marker-text\nerror: something broke\n".to_string(),
        }];
        let task = TaskDef {
            kind: "digest".to_string(),
            context: true,
            ..TaskDef::default()
        };

        // No upstream: nothing to ask.
        let unbound = ResolvedTask {
            task: task.clone(),
            upstream: None,
        };
        assert_eq!(
            answer_with("t", &unbound, &corpus, Some(IN_HOOK_QUESTION)),
            None
        );

        // An unknown kind: nothing to assemble.
        let unknown = ResolvedTask {
            task: TaskDef {
                kind: "no-such-kind".to_string(),
                ..TaskDef::default()
            },
            upstream: Some(google_upstream("http://127.0.0.1:9")),
        };
        assert_eq!(
            answer_with("t", &unknown, &corpus, Some(IN_HOOK_QUESTION)),
            None
        );

        // A real answer, trimmed.
        let (base, seen) = capture(
            r#"{"candidates":[{"content":{"parts":[{"text":"  the answer \n"}]}}],"usageMetadata":{"promptTokenCount":5,"candidatesTokenCount":2}}"#,
        );
        let resolved = ResolvedTask {
            task: task.clone(),
            upstream: Some(google_upstream(&base)),
        };
        let answer = answer_with("t", &resolved, &corpus, Some(IN_HOOK_QUESTION));
        let seen = seen.join().expect("server");
        assert_eq!(answer.as_deref(), Some("the answer"));
        assert!(seen.contains("corpus-marker-text"), "{seen}");

        // A blank answer is no answer.
        let (base, seen) = capture(r#"{"candidates":[{"content":{"parts":[{"text":"   "}]}}]}"#);
        let blank = ResolvedTask {
            task,
            upstream: Some(google_upstream(&base)),
        };
        assert_eq!(
            answer_with("t", &blank, &corpus, Some(IN_HOOK_QUESTION)),
            None
        );
        seen.join().expect("server");
    });
}
