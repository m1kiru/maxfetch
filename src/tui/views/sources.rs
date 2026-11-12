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

    let items: Vec<ListItem> = app.status.iter().map(item).collect();
    frame.render_widget(List::new(items), inner);
}

fn item(status: &SourceStatus) -> ListItem<'static> {
    let headline = format!("{} — {}", status.id.as_str(), auth_label(status.auth));
    let headline = if emphasises_auth(status) {
        Line::styled(headline, Style::new().add_modifier(Modifier::REVERSED))
    } else {
        Line::from(headline)
    };
    let detail = status
        .detail
        .clone()
        .unwrap_or_else(|| "подробностей нет".to_string());
    let capabilities = capabilities_label(status);
    let detail = if capabilities.is_empty() {
        detail
    } else {
        format!("{detail} · {capabilities}")
    };
    ListItem::new(vec![
        headline,
        Line::styled(format!("    {detail}"), hint_style()),
    ])
}

fn auth_label(auth: AuthState) -> &'static str {
    match auth {
        AuthState::NotNeeded | AuthState::Ok => "доступен",
        AuthState::Missing => "нет авторизации",
        AuthState::Expired => "ключ протух",
    }
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
