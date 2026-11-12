//! Downloads screen: one row per download plus a progress gauge for the first
//! visible one.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Gauge, List, ListItem, ListState, Paragraph};

use crate::tui::views::{hint_style, human_size, human_speed, render_scrollbar};
use crate::tui::{App, DownloadItem};

/// Draws the download queue.
pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::bordered().title(format!("Загрузки ({})", app.downloads.len()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.downloads.is_empty() {
        let empty = Paragraph::new(Line::styled("загрузок нет", hint_style()));
        frame.render_widget(empty, inner);
        return;
    }

    let [gauge_area, list_area] =
        Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).areas(inner);

    let current = app
        .downloads
        .get(app.downloads_scroll.offset)
        .unwrap_or(&app.downloads[0]);
    frame.render_widget(
        Gauge::default()
            .ratio(progress_ratio(current))
            .label(progress_label(current)),
        gauge_area,
    );

    let items: Vec<ListItem> = app
        .downloads
        .iter()
        .map(|item| ListItem::new(vec![headline(item), details(item)]))
        .collect();
    let list = List::new(items).highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    let mut state = ListState::default()
        .with_offset(app.downloads_scroll.offset)
        .with_selected(Some(
            app.selected.min(app.downloads.len().saturating_sub(1)),
        ));
    frame.render_stateful_widget(list, list_area, &mut state);

    render_scrollbar(
        frame,
        list_area,
        app.downloads.len(),
        app.downloads_scroll.position(),
    );
}

fn headline(item: &DownloadItem) -> Line<'static> {
    Line::from(format!(
        "{} · {} · {}",
        item.title,
        item.source.as_str(),
        item.state
    ))
}

fn details(item: &DownloadItem) -> Line<'static> {
    let mut parts: Vec<String> = Vec::new();
    parts.push(match item.total {
        Some(total) => format!("{} из {}", human_size(item.transferred), human_size(total)),
        None => human_size(item.transferred),
    });
    if let Some(speed) = item.speed {
        parts.push(human_speed(speed));
    }
    if let Some(eta) = eta_secs(item) {
        parts.push(format!("~{eta} с"));
    }
    if let Some(detail) = &item.detail {
        parts.push(detail.clone());
    }
    Line::styled(format!("    {}", parts.join(" · ")), hint_style())
}

/// Seconds left at the current speed, when both size and speed are known.
fn eta_secs(item: &DownloadItem) -> Option<u64> {
    let total = item.total?;
    let speed = item.speed.filter(|speed| *speed > 0)?;
    let left = total.saturating_sub(item.transferred);
    Some(left / speed)
}

/// Filled fraction of the gauge, `0.0` when the total is unknown.
fn progress_ratio(item: &DownloadItem) -> f64 {
    match item.total {
        Some(total) if total > 0 => item.transferred as f64 / total as f64,
        _ => 0.0,
    }
}

fn progress_label(item: &DownloadItem) -> String {
    match item.total {
        Some(total) if total > 0 => format!(
            "{}% · {} из {}",
            item.transferred.saturating_mul(100) / total,
            human_size(item.transferred),
            human_size(total)
        ),
        _ => human_size(item.transferred),
    }
}
