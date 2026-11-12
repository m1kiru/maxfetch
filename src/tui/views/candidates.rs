//! Candidates screen: every way the pipeline found to deliver one release.

use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Cell, Paragraph, Row, Table, TableState};

use crate::tui::App;
use crate::tui::views::{has_conflict, hint_style, human_size, quality_label, render_scrollbar};

/// Column widths: источник, качество, размер, бейджи.
const WIDTHS: [Constraint; 4] = [
    Constraint::Length(12),
    Constraint::Length(18),
    Constraint::Length(12),
    Constraint::Min(10),
];

/// Draws the candidates of the expanded release as a table.
pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let title = match app.groups.get(app.selected) {
        Some(group) => format!(
            "Кандидаты: {} — {} ({})",
            group.primary.artist,
            group.primary.title,
            app.candidates.len()
        ),
        None => format!("Кандидаты ({})", app.candidates.len()),
    };
    let block = Block::bordered().title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.candidates.is_empty() {
        let empty = Paragraph::new(Line::styled("кандидатов нет", hint_style()));
        frame.render_widget(empty, inner);
        return;
    }

    let header = Row::new(vec!["источник", "качество", "размер", "бейджи"])
        .style(Style::new().add_modifier(Modifier::BOLD));
    let rows: Vec<Row> = app
        .candidates
        .iter()
        .map(|candidate| {
            let mut badges: Vec<&str> = Vec::new();
            if !candidate.quality.lossless {
                badges.push("LOSSY");
            }
            if has_conflict(candidate, &app.candidates) {
                badges.push("конфликт");
            }
            Row::new(vec![
                Cell::from(candidate.source.as_str()),
                Cell::from(quality_label(&candidate.quality)),
                Cell::from(
                    candidate
                        .size_bytes
                        .map_or_else(|| "?".to_string(), human_size),
                ),
                Cell::from(badges.join(" ")),
            ])
        })
        .collect();

    let table = Table::new(rows, WIDTHS)
        .header(header)
        .column_spacing(1)
        .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    let mut state =
        TableState::default().with_selected(Some(app.selected.min(app.candidates.len() - 1)));
    *state.offset_mut() = app.results_scroll.offset;
    frame.render_stateful_widget(table, inner, &mut state);

    render_scrollbar(
        frame,
        inner,
        app.candidates.len(),
        app.results_scroll.position(),
    );
}
