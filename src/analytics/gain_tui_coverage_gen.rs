use super::*;
use crate::core::{test_isolation, user_env};
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};

fn empty_state(last_refresh: Instant) -> State {
    State {
        tracker_summary: None,
        proxy_summary: None,
        last_refresh,
        project_scope: None,
    }
}

fn row_text(buffer: &Buffer, y: u16) -> String {
    (0..buffer.area.width)
        .map(|x| buffer[(x, y)].symbol())
        .collect()
}

#[test]
fn refresh_replaces_a_stale_tracker_snapshot() {
    let scratch = test_isolation::tempdir();
    let db = scratch.path().join("history.db");
    test_isolation::with_root(scratch.path(), || {
        let _working_dir = test_isolation::enter(scratch.path());
        user_env::with_path("RTK_DB_PATH", Some(&db), || {
            let mut state = empty_state(Instant::now() - REFRESH_INTERVAL);
            state.project_scope = Some("isolated-project".to_owned());
            state.refresh();

            let summary = state
                .tracker_summary
                .as_mut()
                .expect("a fresh tracking database should yield an empty summary");
            assert_eq!(summary.total_commands, 0);
            assert!(state.proxy_summary.is_none());

            // A refresh must replace the cached summary, not keep displaying it.
            summary.total_commands = 37;
            let before = Instant::now();
            state.last_refresh = before - REFRESH_INTERVAL;
            state.refresh();

            let summary = state.tracker_summary.as_ref().expect("refreshed summary");
            assert_eq!(summary.total_commands, 0);
            assert!(state.proxy_summary.is_none());
            assert!(state.last_refresh >= before);
            assert!(state.last_refresh <= Instant::now());
        });
    });
}

#[test]
fn dashboard_shows_empty_sections_and_updates_refresh_age() {
    let scratch = test_isolation::tempdir();
    test_isolation::with_root(scratch.path(), || {
        let _working_dir = test_isolation::enter(scratch.path());
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();

        // Instant::elapsed saturates at zero for a future instant, so the
        // fresh-header assertion doesn't depend on how quickly CI draws.
        let mut state = empty_state(Instant::now() + Duration::from_secs(60));
        terminal.draw(|frame| draw(frame, &state)).unwrap();

        let buffer = terminal.backend().buffer();
        let rows: Vec<_> = (0..buffer.area.height)
            .map(|y| row_text(buffer, y))
            .collect();
        assert!(rows[0].contains("rtk gain"));
        assert!(rows[0].contains(" live "));
        assert_eq!(buffer[(2, 0)].fg, Color::Black);
        assert_eq!(buffer[(2, 0)].bg, Color::Green);
        assert!(buffer[(2, 0)].modifier.contains(Modifier::BOLD));
        assert!(rows[1].contains("refreshed 0s ago"));
        assert!(rows[1].contains("r refresh now  ·  q/Esc quit"));
        assert_eq!(buffer[(1, 1)].fg, Color::Green);

        assert!(rows[3].contains("Command filtering"));
        assert!(rows[3].contains("LLM Proxy ("));
        assert!(rows[4].contains("no data yet"));
        assert_eq!(
            rows.iter().any(|row| row.contains("Share it")),
            share_cta_available()
        );

        let context = rows
            .iter()
            .position(|row| row.contains("Context composition"))
            .expect("context panel");
        let table = rows
            .iter()
            .position(|row| row.contains("By command"))
            .expect("command table");
        assert!(context > 3 && table > context);
        assert!(rows[context + 1].contains("no data yet"));
        assert!(rows[table + 1].contains("no data yet"));
        assert!(rows[rows.len() - 1].contains("data unaffected by watching it"));

        state.last_refresh = Instant::now() - REFRESH_INTERVAL;
        terminal.draw(|frame| draw(frame, &state)).unwrap();
        let buffer = terminal.backend().buffer();
        let header = row_text(buffer, 1);
        assert!(header.contains("refreshed ") && header.contains("s ago"));
        assert!(!header.contains("refreshed 0s ago"));
        assert_eq!(buffer[(1, 1)].fg, Color::Gray);
    });
}

#[test]
fn share_panel_displays_both_commands() {
    let mut terminal = Terminal::new(TestBackend::new(120, 3)).unwrap();
    terminal
        .draw(|frame| draw_share_cta(frame, frame.area()))
        .unwrap();

    let buffer = terminal.backend().buffer();
    assert!(row_text(buffer, 0).contains("Share it"));
    assert_eq!(buffer[(0, 0)].fg, Color::Green);
    assert!(row_text(buffer, 1).contains(
        "share these numbers: rtk pipeline share   ·   publish them: rtk pipeline share publish"
    ));
}

#[test]
fn kpi_rows_align_labels_and_emphasize_values_at_tier_boundaries() {
    let label = Style::default().fg(Color::Gray);
    let bold_value = Style::default().add_modifier(Modifier::BOLD);
    assert_eq!(
        kpi_line("Commands", "7".to_owned()),
        Line::from(vec![
            Span::styled("Commands ", label),
            Span::styled("7", bold_value.fg(Color::White)),
        ])
    );

    for (pct, color) in [
        (39.9, Color::Red),
        (40.0, Color::Yellow),
        (69.9, Color::Yellow),
        (70.0, Color::Green),
    ] {
        let value = format!("{pct:.1}%");
        assert_eq!(
            kpi_line_tiered("Saved", value.clone(), pct),
            Line::from(vec![
                Span::styled("Saved    ", label),
                Span::styled(value, bold_value.fg(color)),
            ]),
            "savings tier at {pct:.1}%"
        );
    }
}
