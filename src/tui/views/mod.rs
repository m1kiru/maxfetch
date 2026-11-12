//! One module per screen; each exposes `render(frame, area, app)`.

pub mod candidates;
pub mod downloads;
pub mod log;
pub mod results;
pub mod search;
pub mod settings;
pub mod sources;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget};

use crate::model::{Candidate, Quality};

/// Renders `scrollbar` over the right edge of `area` when the content is taller
/// than the panel, and does nothing otherwise so short lists keep a clean edge.
pub(crate) fn render_scrollbar(frame: &mut Frame, area: Rect, len: usize, position: u16) {
    if len <= area.height as usize || area.height == 0 {
        return;
    }
    let column = Rect {
        x: area.right().saturating_sub(1),
        y: area.y,
        width: 1,
        height: area.height,
    };
    let mut state = ScrollbarState::new(len).position(position as usize);
    Scrollbar::new(ScrollbarOrientation::VerticalRight).render(
        column,
        frame.buffer_mut(),
        &mut state,
    );
}

/// Formats a byte count as `б`, `КиБ`, `МиБ` or `ГиБ`.
pub(crate) fn human_size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    let value = bytes as f64;
    if value < KIB {
        return format!("{bytes} б");
    }
    if value < KIB * KIB {
        return format!("{:.1} КиБ", value / KIB);
    }
    if value < KIB * KIB * KIB {
        return format!("{:.1} МиБ", value / (KIB * KIB));
    }
    format!("{:.2} ГиБ", value / (KIB * KIB * KIB))
}

/// Formats a transition speed as `КиБ/с` or `МиБ/с`.
pub(crate) fn human_speed(bytes_per_sec: u64) -> String {
    const KIB: f64 = 1024.0;
    let value = bytes_per_sec as f64;
    if value < KIB {
        return format!("{bytes_per_sec} б/с");
    }
    if value < KIB * KIB {
        return format!("{:.0} КиБ/с", value / KIB);
    }
    format!("{:.1} МиБ/с", value / (KIB * KIB))
}

/// The quality badge of `quality`: `24/48 FLAC` when lossless, `320 kbps Opus`
/// otherwise, with `?` wherever the source did not say (never invented).
pub(crate) fn quality_label(quality: &Quality) -> String {
    let container = quality.container.as_str().to_uppercase();
    if quality.lossless {
        let depth = quality
            .bit_depth
            .map_or_else(|| "?".to_string(), |bits| bits.to_string());
        let rate = quality
            .sample_rate
            .map_or_else(|| "?".to_string(), |hz| (hz / 1000).to_string());
        format!("{depth}/{rate} {container}")
    } else {
        let bitrate = quality
            .bitrate_kbps
            .map_or_else(|| "?".to_string(), |kbps| kbps.to_string());
        format!("{bitrate} kbps {container}")
    }
}

/// Tells whether `candidate` shares its quality with another candidate in `all`.
///
/// `Candidate` carries no `conflict` flag of its own; the badge is a property of
/// the set currently on screen, so it is computed here.
pub(crate) fn has_conflict(candidate: &Candidate, all: &[Candidate]) -> bool {
    all.iter().any(|other| {
        other.source != candidate.source
            && other.release.source_item_id != candidate.release.source_item_id
            && same_quality(&other.quality, &candidate.quality)
    })
}

fn same_quality(left: &Quality, right: &Quality) -> bool {
    left.lossless == right.lossless
        && left.bit_depth == right.bit_depth
        && left.sample_rate == right.sample_rate
        && left.bitrate_kbps == right.bitrate_kbps
}

/// A dim style for hints and empty-list placeholders.
pub(crate) fn hint_style() -> Style {
    Style::new().add_modifier(ratatui::style::Modifier::DIM)
}
