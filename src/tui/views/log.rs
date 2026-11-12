//! Log screen: the in-memory journal, freshest entry first.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph};

use crate::tui::App;
use crate::tui::views::hint_style;

/// Draws `app.log` with the newest line on top.
pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::bordered().title(format!("Журнал ({})", app.log.len()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.log.is_empty() {
        let empty = Paragraph::new(Line::styled("записей нет", hint_style()));
        frame.render_widget(empty, inner);
        return;
    }

    let items: Vec<ListItem> = app
        .log
        .iter()
        .map(|entry| ListItem::new(Line::from(format!("{} {}", entry.level, entry.message))))
        .collect();
    let list = List::new(items).highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    let mut state =
        ListState::default().with_selected(Some(app.selected.min(app.log.len().saturating_sub(1))));
    frame.render_stateful_widget(list, inner, &mut state);
}
