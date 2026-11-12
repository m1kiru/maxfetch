//! Library layout and filesystem-safe filename sanitization.
//!
//! This module only builds paths: it never touches the filesystem and never
//! transcodes audio.

use std::path::{Path, PathBuf};

use crate::model::Release;

/// Maximum length, in bytes, of a single path component.
const MAX_COMPONENT_BYTES: usize = 240;

/// Characters that are unsafe in a file name on at least one supported platform.
const FORBIDDEN_CHARS: [char; 9] = ['/', '\\', ':', '*', '?', '"', '<', '>', '|'];

/// Device names reserved by Windows (compared case-insensitively, without extension).
const WINDOWS_RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Builds a single path component that is safe on Linux, macOS and Windows.
///
/// Forbidden characters become `_`, whitespace runs collapse into a single
/// space, leading and trailing spaces and dots are removed, Windows device
/// names get a `_` suffix and the result is truncated to 240 bytes on a
/// character boundary (preferably at the last space).
pub fn sanitize_component(input: &str) -> String {
    let replaced: String = input
        .chars()
        .map(|c| if FORBIDDEN_CHARS.contains(&c) { '_' } else { c })
        .collect();
    let collapsed = collapse_whitespace(&replaced);
    let trimmed = trim_edges(&collapsed).to_string();
    let escaped = escape_windows_reserved(&trimmed);
    let truncated = truncate_component(&escaped);
    trim_edges(truncated).to_string()
}

/// Builds the library-relative path `<Artist>/<Album>/<NN> <Title>.<ext>`.
///
/// A release without an album is filed under `Singles`; a release without a
/// track number is numbered `01`.
pub fn relative_path(release: &Release, ext: &str) -> String {
    let artist = sanitize_component(&release.artist);
    let album = match release.album.as_deref() {
        Some(album) => sanitize_component(album),
        None => "Singles".to_string(),
    };
    let track_no = release.track_no.unwrap_or(1);
    let title = sanitize_component(&release.title);
    format!("{artist}/{album}/{track_no:02} {title}.{ext}")
}

/// Joins the library root with the path produced by [`relative_path`].
pub fn absolute_path(root: &Path, release: &Release, ext: &str) -> PathBuf {
    root.join(relative_path(release, ext))
}

/// Replaces every run of whitespace with a single space.
fn collapse_whitespace(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_whitespace = false;
    for c in input.chars() {
        if c.is_whitespace() {
            if !in_whitespace {
                out.push(' ');
                in_whitespace = true;
            }
        } else {
            out.push(c);
            in_whitespace = false;
        }
    }
    out
}

/// Strips leading and trailing spaces and dots.
fn trim_edges(input: &str) -> &str {
    input.trim_matches(|c: char| c.is_whitespace() || c == '.')
}

/// Appends `_` to the stem of a Windows device name, keeping any extension.
fn escape_windows_reserved(input: &str) -> String {
    if input.is_empty() {
        return String::new();
    }
    let stem_end = input.find('.').unwrap_or(input.len());
    let stem = &input[..stem_end];
    let is_reserved = WINDOWS_RESERVED
        .iter()
        .any(|reserved| stem.eq_ignore_ascii_case(reserved));
    if is_reserved {
        format!("{}_", &input[..stem_end]) + &input[stem_end..]
    } else {
        input.to_string()
    }
}

/// Cuts the component down to [`MAX_COMPONENT_BYTES`], never splitting a
/// character and preferring the last space before the limit.
fn truncate_component(input: &str) -> &str {
    if input.len() <= MAX_COMPONENT_BYTES {
        return input;
    }
    let mut end = MAX_COMPONENT_BYTES;
    while end > 0 && !input.is_char_boundary(end) {
        end -= 1;
    }
    let head = &input[..end];
    match head.rfind(' ') {
        Some(space) if space > 0 => &head[..space],
        _ => head,
    }
}