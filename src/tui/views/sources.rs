//! Sources screen: what the registry reports, one row per source.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, List, ListItem, Paragraph};

use crate::sources::{AuthState, SourceStatus};
use crate::tui::App;
use crate::tui::views::hint_style;

/// Draws `app.status`; the screen itself loads nothing.
pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let block =
        Block::bordered().title(format!("Источники ({}) · c — проверить", app.status.len()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.status.is_empty() {
        let empty = Paragraph::new(Line::styled("источников нет", hint_style()));
        frame.render_widget(empty, inner);
        return;
    }

    let rows = app.source_rows();
    let items: Vec<ListItem> = app
        .status
        .iter()
        .zip(rows)
        .map(|(status, row)| item(status, row))
        .collect();
    frame.render_widget(List::new(items), inner);
}

fn item(status: &SourceStatus, headline: String) -> ListItem<'static> {
    let headline = if emphasises_auth(status) {
        Line::styled(headline, Style::new().add_modifier(Modifier::REVERSED))
    } else {
        Line::from(headline)
    };
    let capabilities = capabilities_label(status);
    let detail = match (status.detail.as_deref(), capabilities.is_empty()) {
        (Some(detail), false) => format!("{detail} · {capabilities}"),
        (Some(detail), true) => detail.to_string(),
        (None, false) => capabilities,
        (None, true) => "подробностей нет".to_string(),
    };
    ListItem::new(vec![
        headline,
        Line::styled(format!("    {detail}"), hint_style()),
    ])
}

/// Missing credentials of a source that cannot work without them deserve the
/// loudest row on the screen.
fn emphasises_auth(status: &SourceStatus) -> bool {
    status.auth == AuthState::Missing && status.capabilities.needs_auth
}

fn capabilities_label(status: &SourceStatus) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if status.capabilities.audio {
        parts.push("аудио");
    }
    if status.capabilities.metadata_only {
        parts.push("только метаданные");
    }
    if status.capabilities.lossless {
        parts.push("lossless");
    }
    if status.capabilities.needs_auth {
        parts.push("нужна авторизация");
    }
    parts.join(", ")
}
