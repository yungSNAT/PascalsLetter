use super::*;
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};

fn render(width: u16, height: u16, widget: impl FnOnce(&mut Frame<'_>, Rect)) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| {
            let area = frame.area();
            widget(frame, area);
        })
        .unwrap()
        .buffer
        .clone()
}

fn row(buffer: &Buffer, y: u16) -> String {
    (0..buffer.area.width)
        .map(|x| buffer[(x, y)].symbol())
        .collect()
}

fn empty_tracker_summary() -> TrackerSummary {
    let root = crate::core::test_isolation::tempdir();
    let db_path = root.path().join("gain-tui.sqlite3");
    crate::core::test_isolation::with_root(root.path(), || {
        crate::core::user_env::with_path("RTK_DB_PATH", Some(&db_path), || {
            Tracker::new()
                .expect("open isolated tracker")
                .get_summary_filtered(None)
                .expect("read isolated tracker summary")
        })
    })
}

fn render_context(tracker: Option<&TrackerSummary>, proxy: Option<&ProxyGainSummary>) -> Buffer {
    render(100, 11, |frame, area| {
        draw_context_panel(frame, area, tracker, proxy);
    })
}

#[test]
fn empty_panels_show_a_placeholder_instead_of_rows() {
    let tracker = empty_tracker_summary();
    let proxy = ProxyGainSummary::default();

    for (tracker, proxy) in [
        (None, Some(&proxy)),
        (Some(&tracker), None),
        (Some(&tracker), Some(&proxy)),
    ] {
        let buffer = render_context(tracker, proxy);
        assert!(row(&buffer, 0).contains("Context composition (RTK + LLM Proxy)"));
        assert_eq!(row(&buffer, 1).trim_matches('│').trim(), "no data yet");
        assert!(row(&buffer, 2).trim_matches('│').trim().is_empty());
    }

    for tracker_summary in [None, Some(tracker)] {
        let state = State {
            tracker_summary,
            proxy_summary: None,
            last_refresh: Instant::now(),
            project_scope: None,
        };
        let buffer = render(100, 5, |frame, area| draw_table(frame, area, &state));
        assert!(row(&buffer, 0).contains("By command"));
        assert_eq!(row(&buffer, 1).trim_matches('│').trim(), "no data yet");
        assert!(row(&buffer, 2).trim_matches('│').trim().is_empty());
    }
}

#[test]
fn context_composition_shares_one_total_but_measures_duplicates_against_sent_tokens() {
    let mut tracker = empty_tracker_summary();
    tracker.total_saved = 20;
    let mut proxy = ProxyGainSummary::default();

    let sparse = render_context(Some(&tracker), Some(&proxy));
    assert!(row(&sparse, 1).contains("██████████████████ 100.0%  20"));
    assert!(row(&sparse, 2).contains("Tool output"));
    assert!(row(&sparse, 6).contains("0.0% of what's sent is duplicate content"));

    proxy.saved_ccr = 30;
    proxy.saved_ingest_filter = 10;
    proxy.saved_pixel_compress = 40;
    proxy.ctx_total = 100;
    proxy.ctx_tool_result = 50;
    proxy.ctx_tool_use = 20;
    proxy.ctx_image = 10;
    proxy.ctx_duplicate = 25;
    let buffer = render_context(Some(&tracker), Some(&proxy));

    let expected_rows = [
        ("RTK (bash/tool)", 20, "10.0", 2, Color::Green),
        ("proxy: ccr", 30, "15.0", 3, Color::LightBlue),
        ("proxy: ingest", 10, "5.0", 1, Color::LightGreen),
        ("proxy: pixel", 40, "20.0", 4, Color::LightCyan),
        ("Tool output", 50, "25.0", 5, Color::Yellow),
        ("Tool calls", 20, "10.0", 2, Color::Blue),
        ("Images", 10, "5.0", 1, Color::Magenta),
        ("Other/text", 20, "10.0", 2, Color::Gray),
    ];
    for (y, (label, tokens, pct, filled, color)) in (1..).zip(expected_rows) {
        let expected = format!(
            "{label:<16}{}{} {pct:>4}%  {tokens}",
            "█".repeat(filled),
            "░".repeat(18 - filled)
        );
        assert_eq!(row(&buffer, y).trim_matches('│').trim_end(), expected);
        assert_eq!(buffer[(17, y)].fg, color, "{label}");
    }

    assert!(
        row(&buffer, 9)
            .contains("25.0% of what's sent is duplicate content repeated across blocks")
    );
    let duplicate_note = &buffer[(1, 9)];
    assert_eq!(duplicate_note.fg, Color::DarkGray);
    assert!(duplicate_note.modifier.contains(Modifier::ITALIC));
}

#[test]
fn command_table_caps_rows_and_preserves_values_and_styles() {
    let mut tracker = empty_tracker_summary();
    tracker.by_command = (0..11)
        .map(|idx| {
            let (count, saved, pct) = match idx {
                0 => (7, 321, 75.6),
                1 => (4, 123, 54.2),
                _ => (1, 12, 19.3),
            };
            (
                format!("cmd-{idx:02}"),
                count,
                saved,
                pct,
                Default::default(),
            )
        })
        .collect();
    let state = State {
        tracker_summary: Some(tracker),
        proxy_summary: None,
        last_refresh: Instant::now(),
        project_scope: None,
    };

    let buffer = render(100, 16, |frame, area| {
        draw_table(frame, Rect::new(0, 0, area.width, 15), &state);
        draw_footer(frame, Rect::new(0, 15, area.width, 1));
    });
    let header = row(&buffer, 1);
    assert_eq!(
        header
            .trim_matches('│')
            .split_whitespace()
            .collect::<Vec<_>>(),
        ["Command", "Count", "Saved", "Avg%"]
    );
    let header_cell = &buffer[(1, 1)];
    assert_eq!(header_cell.fg, Color::Cyan);
    assert!(
        header_cell
            .modifier
            .contains(Modifier::BOLD | Modifier::UNDERLINED)
    );

    for (y, fields, color) in [
        (2, ["cmd-00", "7", "321", "75.6%"], Color::Green),
        (3, ["cmd-01", "4", "123", "54.2%"], Color::Yellow),
        (4, ["cmd-02", "1", "12", "19.3%"], Color::Red),
    ] {
        let text = row(&buffer, y);
        assert_eq!(
            text.trim_matches('│')
                .split_whitespace()
                .collect::<Vec<_>>(),
            fields
        );
        let percent_cell = (1..buffer.area.width - 1)
            .map(|x| &buffer[(x, y)])
            .find(|cell| cell.symbol() == "%")
            .unwrap();
        assert_eq!(percent_cell.fg, color);
    }

    for (idx, y) in (2..12).enumerate() {
        assert!(row(&buffer, y).contains(&format!("cmd-{idx:02}")));
        let command_cell = &buffer[(1, y)];
        assert_eq!(command_cell.fg, Color::White);
        assert!(command_cell.modifier.contains(Modifier::BOLD));
        assert_eq!(command_cell.modifier.contains(Modifier::DIM), idx % 2 == 1);
    }
    assert!((2..15).all(|y| !row(&buffer, y).contains("cmd-10")));

    assert_eq!(
        row(&buffer, 15).trim(),
        "live view — data unaffected by watching it"
    );
    let footer_cell = &buffer[(1, 15)];
    assert_eq!(footer_cell.fg, Color::DarkGray);
    assert!(footer_cell.modifier.contains(Modifier::ITALIC));
}

#[test]
fn savings_gauge_clamps_extremes_and_rounds_fill_without_rounding_away_label_precision() {
    for (pct, label, color, fill_percent) in [
        (-10.0, "0.0%", Color::Red, 0),
        (63.6, "63.6%", Color::Yellow, 64),
        (120.0, "100.0%", Color::Green, 100),
    ] {
        let actual = render(100, 1, |frame, area| {
            frame.render_widget(savings_gauge(pct), area);
        });
        // Let Ratatui render the expected fill and label colors; filled bar
        // cells need not use the same foreground/background as label cells.
        let expected = render(100, 1, |frame, area| {
            frame.render_widget(
                Gauge::default()
                    .percent(fill_percent)
                    .label(label)
                    .gauge_style(Style::default().fg(color).add_modifier(Modifier::BOLD)),
                area,
            );
        });
        assert!(row(&actual, 0).contains(label));
        assert_eq!(actual, expected, "gauge at {pct}%");
    }
}
