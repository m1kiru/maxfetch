//! Results screen: one row per release group from the last search.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph};

use crate::matching::ReleaseGroup;
use crate::tui::App;
use crate::tui::views::{hint_style, render_scrollbar};

/// Draws the release groups, marking the ones selected with `Space`.
pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::bordered().title(format!("Результаты ({})", app.groups.len()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.groups.is_empty() {
        let empty = Paragraph::new(Line::styled("ничего не найдено", hint_style()));
        frame.render_widget(empty, inner);
        return;
    }

    let items: Vec<ListItem> = app
        .groups
        .iter()
        .enumerate()
        .map(|(index, group)| {
            let mark = if app.marked.contains(&index) {
                "[x]"
            } else {
                "[ ]"
            };
            ListItem::new(vec![
                Line::from(format!("{mark} {}", headline(group))),
                details(group),
            ])
        })
        .collect();

    let list = List::new(items)
        .block(Block::default())
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    let mut state = ListState::default()
        .with_offset(app.results_scroll.offset)
        .with_selected(Some(app.selected.min(app.groups.len() - 1)));
    frame.render_stateful_widget(list, inner, &mut state);

    render_scrollbar(
        frame,
        inner,
        app.groups.len(),
        app.results_scroll.position(),
    );
}

/// `<artist> — <title>`.
fn headline(group: &ReleaseGroup) -> String {
    format!("{} — {}", group.primary.artist, group.primary.title)
}

/// Album, year, source badges and ISRC of one group.
fn details(group: &ReleaseGroup) -> Line<'static> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(album) = group.primary.album.clone() {
        parts.push(album);
    }
    if let Some(year) = group.primary.year {
        parts.push(year.to_string());
    }
    let badges: Vec<&str> = group.sources.iter().map(|source| source.as_str()).collect();
    if !badges.is_empty() {
        parts.push(badges.join(","));
    }
    if let Some(isrc) = group.primary.isrc.clone() {
        parts.push(format!("ISRC {isrc}"));
    }

    let text = if parts.is_empty() {
        "нет подробностей".to_string()
    } else {
        parts.join(" · ")
    };
    Line::styled(format!("    {text}"), hint_style())
}
