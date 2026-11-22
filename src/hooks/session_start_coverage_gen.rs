use super::*;
use crate::core::{test_isolation, user_env};
use crate::hooks::init::with_claude_dir_override;
use std::cell::Cell;
use std::net::{Ipv4Addr, TcpListener};

#[test]
fn unset_and_non_loopback_session_urls_are_silent() {
    for url in [None, Some("https://example.invalid:443/v1")] {
        assert_eq!(
            user_env::with_vars(&[("ANTHROPIC_BASE_URL", url)], ensure_proxy),
            None,
            "unexpected message for {url:?}"
        );
    }
}

fn with_unresponsive_gateway(check: impl FnOnce(&str, &str)) {
    // Keep an ephemeral port occupied without answering the health check.
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind gateway");
    listener
        .set_nonblocking(true)
        .expect("make the connection assertion nonblocking");
    let bind = listener.local_addr().expect("gateway address").to_string();
    let url = format!("http://{bind}/v1");

    user_env::with_vars(
        &[
            ("ANTHROPIC_BASE_URL", Some(url.as_str())),
            ("RTK_PROXY_BIND", Some(bind.as_str())),
        ],
        || check(&url, &bind),
    );

    drop(
        listener
            .accept()
            .expect("the hook must contact the gateway"),
    );
}

#[test]
fn configured_gateway_warns_and_session_start_checks_it() {
    let tmp = test_isolation::tempdir();
    with_claude_dir_override(&tmp, |_| {
        with_unresponsive_gateway(|url, bind| {
            let message = ensure_proxy().expect("an unresponsive configured gateway must warn");
            assert!(message.contains(url), "{message}");
            assert!(
                message.contains("not answering as the RTK proxy"),
                "{message}"
            );
            assert!(
                message.contains(&format!("rtk llm-proxy start --bind {bind}")),
                "{message}"
            );
        });

        // Use a fresh listener so only run() can account for its connection.
        with_unresponsive_gateway(|_, _| run());
    });
}

#[test]
fn startup_retries_after_a_failed_post_spawn_health_check() {
    let bind = "127.0.0.1:18580";
    let url = "http://127.0.0.1:18580/v1";
    let probes = Cell::new(0);
    let first_poll_at = Cell::new(None);
    let spawned = Cell::new(false);

    let message = ensure_proxy_impl(
        url,
        |addr| {
            assert_eq!(addr, bind);
            let attempt = probes.get() + 1;
            probes.set(attempt);
            match attempt {
                1 => {
                    assert!(!spawned.get());
                    false
                }
                2 => {
                    assert!(spawned.get());
                    first_poll_at.set(Some(Instant::now()));
                    false
                }
                3 => {
                    let elapsed = first_poll_at.get().expect("first startup poll").elapsed();
                    assert!(
                        elapsed >= Duration::from_millis(50),
                        "startup polled again without backing off: {elapsed:?}"
                    );
                    true
                }
                _ => panic!("unexpected health check #{attempt}"),
            }
        },
        |addr| {
            assert_eq!(addr, bind);
            false
        },
        |_| panic!("a free port needs no setup check"),
        |addr| {
            assert_eq!(addr, bind);
            assert!(!spawned.replace(true));
            Ok(())
        },
        Duration::from_secs(5),
    )
    .expect("the successful retry must report startup");

    assert_eq!(probes.get(), 3);
    assert_eq!(
        message,
        format!("rtk: started the RTK proxy for this session ({url})")
    );
}
