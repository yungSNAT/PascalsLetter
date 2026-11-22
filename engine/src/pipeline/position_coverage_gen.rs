use super::*;

#[test]
fn parse_rejects_unknown_sites_even_with_valid_event_and_subject() {
    assert!(Position::parse("hooks.before_tool.shell").is_none());
    assert!(Position::parse("proxies.request.cache").is_none());
}
