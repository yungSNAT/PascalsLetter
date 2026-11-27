use super::*;
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};
use std::ops::Range;

fn render(width: u16, height: u16, paint: impl FnOnce(&mut Frame<'_>)) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
    terminal.draw(paint).expect("render into test buffer");
    terminal.backend().buffer().clone()
}

fn row_text(buffer: &Buffer, y: u16, columns: Range<u16>) -> String {
    columns.map(|x| buffer[(x, y)].symbol()).collect()
}

fn render_top(state: &State, panel_width: u16) -> Buffer {
    let width = 2 * panel_width;
    render(width, 11, |frame| {
        draw_header(frame, Rect::new(0, 0, width, 3), state);
        draw_kpi_panels(frame, Rect::new(0, 3, width, 8), state, 5);
    })
}

#[test]
fn empty_dashboard_shows_controls_and_distinct_source_hints() {
    let state = State {
        tracker_summary: None,
        proxy_summary: None,
        last_refresh: Instant::now(),
        project_scope: None,
    };
    let hint = llm_proxy::no_data_hint();
    let first_hint_line = hint.lines().next().expect("proxy setup hint");
    let panel_width = u16::try_from(first_hint_line.len().max(100) + 2).unwrap();
    let buffer = render_top(&state, panel_width);
    let left = 1..panel_width - 1;
    let right = panel_width + 1..2 * panel_width - 1;

    assert!(row_text(&buffer, 0, left.clone()).contains("rtk gain  live"));
    assert!(row_text(&buffer, 1, left.clone()).contains("r refresh now  ·  q/Esc quit"));
    assert!(
        row_text(&buffer, 3, left.clone()).contains("Command filtering (rtk git/npm/cargo/...)")
    );
    assert_eq!(row_text(&buffer, 4, left).trim_end(), "no data yet");
    let proxy_title = row_text(&buffer, 3, right.clone());
    assert!(proxy_title.contains("LLM Proxy ("));
    assert!(proxy_title.contains(" llm-proxy)"));
    assert!(row_text(&buffer, 4, right).starts_with(first_hint_line));
    assert_eq!(buffer[(0, 3)].fg, Color::Cyan);
    assert_eq!(buffer[(panel_width, 3)].fg, Color::LightMagenta);
}

#[test]
fn populated_panels_distinguish_filtering_trim_routing_and_recovery() {
    let root = crate::core::test_isolation::tempdir();
    let mut tracker = crate::core::test_isolation::with_root(root.path(), || {
        Tracker::new()
            .expect("isolated tracker")
            .get_summary_filtered(None)
            .expect("empty tracker summary")
    });
    tracker.total_commands = 8;
    tracker.total_input = 800;
    tracker.total_output = 200;
    tracker.total_saved = 600;
    tracker.avg_savings_pct = 75.0;
    tracker.oversized_capped = 2;

    let mut state = State {
        tracker_summary: Some(tracker),
        proxy_summary: Some(ProxyGainSummary::default()),
        last_refresh: Instant::now(),
        project_scope: None,
    };
    let proxy = state.proxy_summary.as_mut().unwrap();
    proxy.total_requests = 7;
    proxy.tokens_in = 800;
    proxy.tokens_out = 100;
    proxy.tokens_saved = 50;
    proxy.trimmed_requests = 3;
    proxy.routed_requests = 2;
    proxy.routed_tokens_in = 250;
    proxy.routed_cost_usd = 0.125;
    proxy.routed_saved_usd = 0.25;
    proxy.retrieve_calls = 4;

    let buffer = render_top(&state, 110);
    let routing = format!(
        "Routed   2 req · 250 in on the cheaper task model · $0.1250 spent · {} saved est.",
        llm_proxy::approx_usd(0.25)
    );
    for (y, (tracker_line, proxy_line)) in (4..).zip([
        ("Commands 8", "Requests 7"),
        ("Input    800", "Tokens   800 in / 100 out"),
        ("Output   200", "Trimmed  50 across 3 req touched"),
        ("Saved    600 (75.0%)", routing.as_str()),
        (
            "Capped   2 outputs counted at the agent cap",
            "Recovery 4 calls",
        ),
    ]) {
        assert_eq!(row_text(&buffer, y, 1..109).trim_end(), tracker_line);
        assert_eq!(row_text(&buffer, y, 111..219).trim_end(), proxy_line);
    }
    assert_eq!(buffer[(10, 7)].fg, Color::Green);
    assert!(row_text(&buffer, 9, 1..109).contains("75.0%"));
    assert_eq!(row_text(&buffer, 9, 111..219).trim(), "0.0%");
}

#[test]
fn proxy_without_prices_clamps_negative_token_counts_and_omits_costs() {
    let proxy = &mut ProxyGainSummary::default();
    proxy.total_requests = 4;
    proxy.tokens_in = -10;
    proxy.tokens_out = -20;
    proxy.tokens_saved = -30;
    proxy.routed_requests = 1;
    proxy.routed_tokens_in = -40;

    let buffer = render(90, 8, |frame| {
        draw_proxy_panel(frame, frame.area(), Some(proxy), 5);
    });
    for (y, expected) in (1..).zip([
        "Requests 4",
        "Tokens   0 in / 0 out",
        "Trimmed  0 (transforms only)",
        "Routed   1 req · 0 in on the cheaper task model",
        "",
    ]) {
        assert_eq!(row_text(&buffer, y, 1..89).trim_end(), expected);
    }
    assert_eq!(row_text(&buffer, 6, 1..89).trim(), "0.0%");
}

#[test]
fn composition_bar_rounds_and_bounds_the_fill_without_hiding_the_share() {
    for (tokens, total, bar, percentage, count) in [
        (3, 8, "██░░", " 37.5%  ", "3"),
        (5, 4, "████", " 125.0%  ", "5"),
        (-7, 0, "░░░░", "  0.0%  ", "0"),
    ] {
        let line = composition_bar("Tool calls", tokens, total, Color::Blue, 4);
        let spans: Vec<_> = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(spans, ["Tool calls      ", bar, percentage, count]);
        assert_eq!(line.spans[1].style.fg, Some(Color::Blue));
        assert_eq!(line.spans[2].style.fg, Some(Color::Blue));
    }
}
