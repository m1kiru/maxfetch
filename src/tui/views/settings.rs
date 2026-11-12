//! Settings screen: the library path, the source toggles and the limits.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph};

use crate::tui::views::hint_style;
use crate::tui::{App, source_names};

/// Draws the editable configuration. Nothing is written until Settings is left.
pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let title = if app.dirty {
        "Настройки · есть несохранённые изменения · e — путь · Space — источник"
    } else {
        "Настройки · e — путь · Space — источник"
    };
    let block = Block::bordered().title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [library_area, toggles_area, limits_area] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(0),
        Constraint::Length(4),
    ])
    .areas(inner);

    let library = format!("Библиотека: {}", app.config.library.path.display());
    let library = if app.editing {
        Line::styled(library, Style::new().add_modifier(Modifier::REVERSED))
    } else {
        Line::from(library)
    };
    frame.render_widget(
        Paragraph::new(vec![
            library,
            Line::styled(
                "    e — изменить, Enter — принять, Esc — отменить",
                hint_style(),
            ),
        ]),
        library_area,
    );

    let names = source_names(&app.config);
    let items: Vec<ListItem> = names
        .iter()
        .map(|name| {
            let mark = if app.config.sources.enabled.iter().any(|e| e == name) {
                "[x]"
            } else {
                "[ ]"
            };
            ListItem::new(Line::from(format!("{mark} {name}")))
        })
        .collect();
    let toggles = List::new(items)
        .block(Block::bordered().title("Источники"))
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    let state_selected = app.selected.min(names.len().saturating_sub(1));
    let mut state = ListState::default().with_selected(Some(state_selected));
    frame.render_stateful_widget(toggles, toggles_area, &mut state);

    let limits = vec![
        Line::from(format!(
            "загрузок одновременно: {} · попыток: {}",
            app.config.download.max_concurrency, app.config.download.retry_attempts
        )),
        Line::from(format!(
            "таймаут: {} с · простой: {} с",
            app.config.download.timeout_sec, app.config.download.stall_timeout_sec
        )),
        Line::from(format!("yt-dlp: {}", app.config.youtube.yt_dlp_path)),
        Line::from(format!("ffmpeg: {}", app.config.media.ffmpeg_path)),
    ];
    frame.render_widget(
        Paragraph::new(limits).block(Block::bordered().title("Лимиты")),
        limits_area,
    );
}
