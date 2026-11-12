//! Search screen: the hint about accepted inputs and the enabled sources.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph};

use crate::tui::App;
use crate::tui::views::hint_style;

/// Draws the input hints and the sources the search will run against.
pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let [hints_area, sources_area] =
        Layout::vertical([Constraint::Length(5), Constraint::Min(0)]).areas(area);

    let hints = vec![
        Line::from("Название трека, исполнитель, ссылка на трек или альбом, ISRC."),
        Line::from("Enter — начать поиск · / — правка запроса · ? — справка по клавишам."),
    ];
    frame.render_widget(
        Paragraph::new(hints).block(Block::bordered().title("Что можно искать")),
        hints_area,
    );

    let enabled = if app.config.sources.enabled.is_empty() {
        Line::styled("источников не включено", hint_style())
    } else {
        Line::from(app.config.sources.enabled.join(", "))
    };
    let block = Block::bordered().title(format!(
        "Включённые источники ({})",
        app.config.sources.enabled.len()
    ));
    frame.render_widget(Paragraph::new(enabled).block(block), sources_area);
}
