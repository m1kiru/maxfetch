//! Parsing of `yt-dlp` output: playlists, track metadata, best audio format.
//! There are no network calls here: the binary is only ever started by an
//! explicit path handed in by the caller.

use std::process::Command;

use anyhow::Context;

use crate::model::{Container, Quality};

/// One video from a flat playlist listing.
#[derive(Clone, Debug, PartialEq)]
pub struct YoutubeVideo {
    pub video_id: String,
    pub title: String,
    pub duration_sec: Option<f64>,
    pub url: String,
}

/// Track metadata extracted from `yt-dlp -J` for a single video.
#[derive(Clone, Debug, PartialEq)]
pub struct YoutubeTrackMeta {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
}

/// Anything longer than this is a DJ set or a radio broadcast, not a track.
pub const MAX_TRACK_SECONDS: u64 = 900;

/// Placeholder for a track whose uploader carries no usable artist name.
const UNKNOWN_ARTIST: &str = "Unknown Artist";

/// Reads a numeric JSON value that may be either a float or an integer.
fn number(value: Option<&serde_json::Value>) -> Option<f64> {
    let value = value?;
    value.as_f64().or_else(|| value.as_u64().map(|n| n as f64))
}

/// Reads a string JSON value, treating blank strings as absent.
fn non_empty(value: Option<&serde_json::Value>) -> Option<String> {
    let text = value?.as_str()?;
    if text.trim().is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

/// Strips a trailing ` - Topic` (case-insensitive) from a channel name.
fn strip_topic_suffix(name: &str) -> Option<&str> {
    const SUFFIX: &str = " - topic";
    let head_len = name.len().checked_sub(SUFFIX.len())?;
    if !name.is_char_boundary(head_len) {
        return None;
    }
    let (head, tail) = name.split_at(head_len);
    tail.eq_ignore_ascii_case(SUFFIX).then_some(head)
}

/// Drops the `- Topic` loader suffix and never lets `Topic` become an artist.
fn clean_artist(raw: Option<String>) -> String {
    let Some(raw) = raw else {
        return UNKNOWN_ARTIST.to_string();
    };
    let trimmed = raw.trim();
    let stripped = match strip_topic_suffix(trimmed) {
        Some(head) => head.trim(),
        None => trimmed,
    };
    if stripped.is_empty() || stripped.eq_ignore_ascii_case("Topic") {
        UNKNOWN_ARTIST.to_string()
    } else {
        stripped.to_string()
    }
}

/// Reads `entries[]` of a flat playlist listing, skipping entries without an
/// `id` (they cannot be downloaded, so they are useless downstream).
pub fn parse_flat_playlist(json: &serde_json::Value) -> anyhow::Result<Vec<YoutubeVideo>> {
    let entries = json
        .get("entries")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow::anyhow!("yt-dlp playlist json has no entries array"))?;
    let mut videos = Vec::with_capacity(entries.len());
    for entry in entries {
        let Some(id) = entry
            .get("id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        videos.push(YoutubeVideo {
            video_id: id.to_string(),
            title: entry
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            duration_sec: number(entry.get("duration")),
            url: format!("https://www.youtube.com/watch?v={id}"),
        });
    }
    Ok(videos)
}

/// Maps a single `yt-dlp -J` video object to tag metadata.
///
/// `track` wins over `title`; `artist` falls back to `uploader` then `channel`.
/// A missing `duration` is not a rejection — the length is simply unknown.
pub fn parse_video(json: &serde_json::Value) -> anyhow::Result<YoutubeTrackMeta> {
    let id = json.get("id").and_then(|v| v.as_str());

    if let Some(secs) = number(json.get("duration"))
        && secs > MAX_TRACK_SECONDS as f64
    {
        anyhow::bail!(
            "слишком длинное видео ({} с): DJ-сет или радиопередача",
            secs as u64
        );
    }

    // untested: no plan test covers a missing title
    let title = non_empty(json.get("track"))
        .or_else(|| non_empty(json.get("title")))
        .ok_or_else(|| match id {
            Some(id) => anyhow::anyhow!("video {id} has no title"),
            None => anyhow::anyhow!("video has no title"),
        })?;

    let raw_artist = non_empty(json.get("artist"))
        .or_else(|| non_empty(json.get("uploader")))
        .or_else(|| non_empty(json.get("channel")));

    Ok(YoutubeTrackMeta {
        title,
        artist: clean_artist(raw_artist),
        album: non_empty(json.get("album")),
    })
}

/// Picks the `formats[]` entry with the highest `abr` and maps it to `Quality`.
///
/// YouTube only serves lossy audio, so the result is never lossless. Formats
/// without a numeric `abr` cannot win; a missing or empty list yields the
/// default lossy quality rather than an error (the signature has no `Result`).
pub fn bestaudio_quality(json: &serde_json::Value) -> Quality {
    let mut best: Option<(&serde_json::Value, f64)> = None;
    for format in json
        .get("formats")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
    {
        let Some(abr) = number(format.get("abr")) else {
            continue;
        };
        // Strictly greater keeps the first entry on ties.
        if best.is_none_or(|(_, current)| abr > current) {
            best = Some((format, abr));
        }
    }
    let Some((format, abr)) = best else {
        return Quality {
            lossless: false,
            container: Container::Opus,
            ..Quality::default()
        };
    };
    Quality {
        lossless: false,
        container: if format
            .get("acodec")
            .and_then(|v| v.as_str())
            .is_some_and(|codec| codec.starts_with("opus"))
        {
            Container::Opus
        } else {
            Container::M4aAac
        },
        // YouTube never reports a bit depth for its lossy streams.
        bit_depth: None,
        sample_rate: number(format.get("asr")).map(|value| value as u32),
        bitrate_kbps: Some(abr.round() as u32),
    }
}

/// Runs `yt-dlp` with `args` and parses its stdout as JSON.
pub fn run_json(yt_dlp_path: &str, args: &[&str]) -> anyhow::Result<serde_json::Value> {
    let output = Command::new(yt_dlp_path)
        .args(args)
        .output()
        .with_context(|| format!("не удалось запустить yt-dlp: {yt_dlp_path}"))?;
    // untested: the plan only covers the spawn failure, not a non-zero exit
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: String = stderr
            .chars()
            .rev()
            .take(500)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        anyhow::bail!(
            "yt-dlp завершился с кодом {}: {}",
            output.status,
            tail.trim()
        );
    }
    serde_json::from_slice(&output.stdout).context("yt-dlp вернул некорректный JSON")
}

/// Reports whether `path` is a usable `yt-dlp`: `None` when it answers
/// `--version`, otherwise a Russian message meaning "missing or unusable".
///
/// The message keeps the plan's wording ("не найден") even though a binary may
/// also be present and broken; the caller only uses it as a skip reason.
pub fn binary_status(path: &str) -> Option<String> {
    match Command::new(path).arg("--version").output() {
        Ok(output) if output.status.success() => None,
        _ => Some(format!("yt-dlp не найден: {path}")),
    }
}

/// True for any URL carrying a `list=` query parameter.
pub fn is_playlist_url(url: &str) -> bool {
    url.contains("list=")
}
