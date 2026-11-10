use unicode_normalization::UnicodeNormalization;

use crate::model::{Release, SourceId};

#[derive(Clone, Debug, PartialEq)]
pub struct ReleaseGroup {
    pub primary: Release,
    pub members: Vec<Release>,
    pub sources: Vec<SourceId>,
}

const KNOWN_MARKERS: &[&str] = &[
    "remaster",
    "live",
    "remix",
    "radio edit",
    "edit",
    "mono",
    "stereo",
    "demo",
    "instrumental",
    "version",
    "bonus",
    "deluxe",
    "anniversary",
    "reissue",
    "official audio",
    "official video",
    "explicit",
];

pub fn is_known_version_marker(text: &str) -> bool {
    let lower = text.to_lowercase();
    KNOWN_MARKERS.iter().any(|m| lower.contains(m))
}

fn strip_bracketed_markers(s: &str) -> String {
    let mut out = s.to_string();
    let mut cursor = 0;
    loop {
        let bytes = out.as_bytes();
        let mut start: Option<usize> = None;
        let mut open = '\0';
        let mut found: Option<(usize, usize)> = None;
        let mut i = cursor;
        while i < bytes.len() {
            let c = bytes[i] as char;
            if c == '(' || c == '[' {
                start = Some(i);
                open = c;
            } else if (c == ')' && open == '(') || (c == ']' && open == '[') {
                if let Some(st) = start {
                    found = Some((st, i));
                    break;
                }
            }
            i += 1;
        }
        let Some((st, en)) = found else { break };
        let inner = out[st + 1..en].to_string();
        if is_known_version_marker(&inner) {
            out.replace_range(st..=en, " ");
            cursor = st;
        } else {
            cursor = en + 1;
        }
    }
    out
}

fn strip_numeric_remaster_suffix(s: &str) -> String {
    // Remove a standalone " - <digits> remaster" suffix (already lowercased).
    if let Some(idx) = s.rfind(" - ") {
        let tail = &s[idx + 3..];
        let mut parts = tail.split_whitespace();
        let digits = parts.next().unwrap_or("");
        let word = parts.next().unwrap_or("");
        if parts.next().is_none()
            && !digits.is_empty()
            && digits.chars().all(|c| c.is_ascii_digit())
            && word == "remaster"
        {
            return s[..idx].to_string();
        }
    }
    s.to_string()
}

pub fn norm(s: &str) -> String {
    let nfc: String = s.nfc().collect();
    let lower = nfc.to_lowercase();
    let no_brackets = strip_bracketed_markers(&lower);
    let no_suffix = strip_numeric_remaster_suffix(&no_brackets);
    let spaced: String = no_suffix
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c.is_whitespace() {
                c
            } else {
                ' '
            }
        })
        .collect();
    spaced.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn artist_title_key(r: &Release) -> String {
    format!("{}|{}", norm(&r.artist), norm(&r.title))
}

pub fn match_key(r: &Release) -> String {
    let album = r.album.as_deref().map(norm).unwrap_or_default();
    format!("{}|{}|{}", norm(&r.artist), norm(&r.title), album)
}

fn same_release(a: &Release, b: &Release, tol: f64) -> bool {
    match (&a.isrc, &b.isrc) {
        (Some(x), Some(y)) => x == y,
        _ => {
            if match_key(a) == match_key(b) {
                return true;
            }
            if artist_title_key(a) != artist_title_key(b) {
                return false;
            }
            match (a.duration_sec, b.duration_sec) {
                (Some(x), Some(y)) => (x - y).abs() <= tol,
                _ => false,
            }
        }
    }
}

fn completeness(r: &Release) -> usize {
    [
        r.album.is_some(),
        r.track_no.is_some(),
        r.duration_sec.is_some(),
        r.isrc.is_some(),
        r.year.is_some(),
    ]
    .iter()
    .filter(|b| **b)
    .count()
}

fn pick_primary(members: &[Release]) -> Release {
    if let Some(s) = members.iter().find(|r| r.source == SourceId::Spotify) {
        return s.clone();
    }
    members
        .iter()
        .max_by(|a, b| {
            completeness(a)
                .cmp(&completeness(b))
                .then_with(|| b.source_item_id.cmp(&a.source_item_id))
        })
        .expect("non-empty group")
        .clone()
}

pub fn group_releases(releases: Vec<Release>, duration_tolerance_sec: f64) -> Vec<ReleaseGroup> {
    let mut groups: Vec<Vec<Release>> = Vec::new();
    for r in releases {
        let mut placed = false;
        for g in groups.iter_mut() {
            if g.iter().any(|m| same_release(m, &r, duration_tolerance_sec)) {
                g.push(r.clone());
                placed = true;
                break;
            }
        }
        if !placed {
            groups.push(vec![r]);
        }
    }
    groups
        .into_iter()
        .map(|members| {
            let primary = pick_primary(&members);
            let mut sources: Vec<SourceId> = members.iter().map(|r| r.source).collect();
            sources.sort_by_key(|s| s.as_str());
            sources.dedup();
            ReleaseGroup {
                primary,
                members,
                sources,
            }
        })
        .collect()
}
