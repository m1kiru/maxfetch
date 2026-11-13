//! Parsing of `yt-dlp` output: playlists, track metadata, best audio format.
//! There are no network calls here: the binary is only ever started by an
//! explicit path handed in by the caller.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context;

use crate::fetch::ProgressSink;
use crate::media::{remux_to_opus, write_sidecar};
use crate::model::{Container, Quality};
use crate::probe::probe;

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

/// What [`finalize`] left on disk and whether it can still be tagged.
///
/// `untagged` means "a sidecar JSON sits next to the media file", not "the
/// media file lacks embedded tags": tagging is out of scope here.
#[derive(Clone, Debug, PartialEq)]
pub struct FinalizedAudio {
    pub path: PathBuf,
    pub container: Container,
    pub untagged: bool,
    pub sidecar: Option<PathBuf>,
}

/// Downloads the best audio stream of `url` into `work_dir`.
///
/// The output template is `<work_dir>/<video_id>.%(ext)s`, so exactly one file
/// with that stem is expected. A `yt-dlp` failure is reported with the tail of
/// its stderr. Progress is not parsed: only [`ProgressSink::on_finished`] runs.
pub fn download_audio(
    yt_dlp_path: &str,
    video_id: &str,
    url: &str,
    work_dir: &Path,
    sink: &dyn ProgressSink,
) -> anyhow::Result<PathBuf> {
    let template = format!("{}/{}.%(ext)s", work_dir.display(), video_id);
    let output = Command::new(yt_dlp_path)
        .args(["-f", "bestaudio", "-o", &template, url])
        .output()
        .with_context(|| format!("не удалось запустить yt-dlp: {yt_dlp_path}"))?;

    if !output.status.success() {
        anyhow::bail!(
            "yt-dlp завершился с кодом {}: {}",
            output.status,
            tail_of(&output.stderr)
        );
    }

    let mut found: Vec<PathBuf> = std::fs::read_dir(work_dir)
        .with_context(|| format!("reading work directory {}", work_dir.display()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.file_stem() == Some(OsStr::new(video_id)))
        .collect();
    found.sort();

    match found.len() {
        1 => {
            let path = found.remove(0);
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            sink.on_finished(size);
            Ok(path)
        }
        0 => anyhow::bail!(
            "yt-dlp не создал файл для {video_id} в {}: {}",
            work_dir.display(),
            list_dir(work_dir)
        ),
        _ => anyhow::bail!(
            "yt-dlp создал несколько файлов для {video_id}: {}",
            found
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Remuxes `downloaded` into `dest`, falling back to a sidecar on failure.
///
/// The remux writes to `<dest>.tmp.opus` first because `ffmpeg` picks the muxer
/// from the output extension and `dest` may be `<uuid>.part`. When the remux
/// fails the downloaded bytes are copied as is, a JSON sidecar is written next
/// to `dest` and `untagged` is set. In both branches `container` comes from
/// [`probe`], never from the file name.
pub fn finalize(
    ffmpeg_path: &str,
    downloaded: &Path,
    dest: &Path,
    video_id: &str,
    meta: &YoutubeTrackMeta,
) -> anyhow::Result<FinalizedAudio> {
    let temp = remux_temp_path(dest);

    let (untagged, sidecar) = match remux_to_opus(ffmpeg_path, downloaded, &temp) {
        Ok(()) => {
            std::fs::rename(&temp, dest)
                .with_context(|| format!("moving {} to {}", temp.display(), dest.display()))?;
            // The source file is no longer needed; a failure to remove it is
            // not worth masking a successful finalize.
            let _ = std::fs::remove_file(downloaded);
            (false, None)
        }
        Err(error) => {
            tracing::warn!(
                video_id,
                error = %error,
                "remux to opus failed; keeping the downloaded bytes and writing a sidecar"
            );
            let _ = std::fs::remove_file(&temp);
            std::fs::copy(downloaded, dest).with_context(|| {
                format!("copying {} to {}", downloaded.display(), dest.display())
            })?;
            write_sidecar(dest, &sidecar_payload(video_id, meta))?;
            (true, Some(sidecar_path(dest)))
        }
    };

    let container = match probe(dest) {
        Ok(result) => result.container,
        Err(error) => {
            tracing::warn!(
                dest = %dest.display(),
                error = %error,
                "probe failed on the finished file"
            );
            // known limitation: unprobeable bytes are reported as Webm
            Container::Webm
        }
    };

    Ok(FinalizedAudio {
        path: dest.to_path_buf(),
        container,
        untagged,
        sidecar,
    })
}

/// JSON written next to an untagged file so the metadata is not lost.
pub fn sidecar_payload(video_id: &str, meta: &YoutubeTrackMeta) -> serde_json::Value {
    serde_json::json!({
        "source": "youtube",
        "video_id": video_id,
        "title": meta.title,
        "artist": meta.artist,
        "album": meta.album,
    })
}

/// True when `path` and its `<path>.json` sidecar both exist.
pub fn untagged(path: &Path) -> bool {
    path.exists() && sidecar_path(path).exists()
}

/// Renames `<temp_media>.json` onto `<final_media>.json`.
///
/// Returns the new sidecar path, or `None` when the media file never had one.
/// The destination directory must already exist.
pub fn move_sidecar(temp_media: &Path, final_media: &Path) -> anyhow::Result<Option<PathBuf>> {
    let from = sidecar_path(temp_media);
    if !from.exists() {
        return Ok(None);
    }
    let to = sidecar_path(final_media);
    std::fs::rename(&from, &to)
        .with_context(|| format!("moving sidecar {} to {}", from.display(), to.display()))?;
    Ok(Some(to))
}

/// Path of the sidecar: `.json` appended to the whole file name.
fn sidecar_path(path: &Path) -> PathBuf {
    let mut raw = path.as_os_str().to_os_string();
    raw.push(".json");
    PathBuf::from(raw)
}

/// Temporary remux target next to `dest` with an `.opus` extension.
///
/// The extension matters: `ffmpeg` chooses the muxer from it, and `dest` itself
/// may carry an unrelated extension such as `.part`.
fn remux_temp_path(dest: &Path) -> PathBuf {
    let name = dest
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    dest.with_file_name(format!("{name}.tmp.opus"))
}

/// Last ~500 characters of `bytes`, for stderr diagnostics.
fn tail_of(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let count = text.chars().count();
    let skip = count.saturating_sub(500);
    text.chars()
        .skip(skip)
        .collect::<String>()
        .trim()
        .to_string()
}

/// Comma-separated listing of `dir`, for "nothing was produced" diagnostics.
fn list_dir(dir: &Path) -> String {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    if names.is_empty() {
        "<empty>".to_string()
    } else {
        names.join(", ")
    }
}
