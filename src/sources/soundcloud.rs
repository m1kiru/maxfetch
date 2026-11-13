//! SoundCloud scraping and parsing: the pure seam between Task 6 and the
//! later `SoundCloudSource`.
//!
//! Everything here is stateless: a scraper that extracts the public
//! `client_id` from the web page bundles the way `yt-dlp` does (§8.9), a
//! credentials-file cache around it, and a thin typed layer over the
//! `GET /users/{id}` profile endpoint. No retry logic and no 401/403
//! re-scraping lives here; the `SoundCloudSource` itself arrives in Task 8
//! and wraps these helpers with `http_client` and its retry policy.

use std::path::Path;
use std::sync::OnceLock;

use anyhow::Context;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::{Credentials, SoundCloudCredentials};
use crate::model::{Container, Quality, Release, SourceId};

/// The one `client_id` pattern shared by the scraper and its documentation.
pub const CLIENT_ID_REGEX: &str = r#"client_id\s*:\s*"([0-9a-zA-Z]{32})""#;

fn client_id_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX
        .get_or_init(|| Regex::new(CLIENT_ID_REGEX).expect("CLIENT_ID_REGEX must be a valid regex"))
}

/// The first 32-character `client_id` literal in `js`, if any.
pub fn find_client_id(js: &str) -> Option<String> {
    client_id_regex()
        .captures(js)
        .and_then(|captures| captures.get(1))
        .map(|hit| hit.as_str().to_string())
}

/// Every `<script src=...>` target, in document order.
pub fn script_urls(html: &str) -> Vec<String> {
    let mut urls = Vec::new();
    let mut rest = html;
    while let Some((_, tail)) = rest.split_once("<script") {
        let tag = tag_end(tail);
        if let Some(url) = script_src(tag) {
            urls.push(url);
        }
        rest = tail;
    }
    urls
}

/// The text of the `<script ...>` tag: from the match to its closing `>`.
fn tag_end(tail_after_open: &str) -> &str {
    match tail_after_open.find('>') {
        Some(end) => &tail_after_open[..end],
        None => tail_after_open,
    }
}

/// The value of the first `src="..."` attribute inside one tag, if any.
fn script_src(tag: &str) -> Option<String> {
    let mut rest = tag;
    while let Some((_, tail)) = rest.split_once("src=") {
        if let Some(value) = quoted(tail) {
            return Some(value);
        }
        rest = tail;
    }
    None
}

/// A `"..."` value at the start of `tail`, if the quote is there and closed.
fn quoted(tail: &str) -> Option<String> {
    let tail = tail.strip_prefix('"')?;
    tail.find('"').map(|end| tail[..end].to_string())
}

/// Web, API and script-CDN roots of SoundCloud.
#[derive(Clone, Debug, PartialEq)]
pub struct SoundCloudEndpoints {
    pub web_base: String,
    pub api_base: String,
    pub script_base: String,
}

impl SoundCloudEndpoints {
    pub fn production() -> SoundCloudEndpoints {
        SoundCloudEndpoints {
            web_base: "https://soundcloud.com".to_string(),
            api_base: "https://api-v2.soundcloud.com".to_string(),
            script_base: "https://a-v2.sndcdn.com".to_string(),
        }
    }
}

/// Fetches the web page and pulls the `client_id` out of its bundles.
///
/// Bundles are walked in reverse order of appearance, matching the way
/// `yt-dlp` looks for the id in the newest chunk first. A transport failure on
/// one bundle only logs and the walk continues; only the home page failing is
/// a hard error, because there is no way forward without it.
pub async fn scrape_client_id(
    client: &reqwest::Client,
    endpoints: &SoundCloudEndpoints,
) -> anyhow::Result<String> {
    let home_url = format!("{}/", endpoints.web_base);
    let home = client
        .get(&home_url)
        .send()
        .await
        .with_context(|| format!("не удалось скачать страницу {home_url}"))?
        .error_for_status()
        .with_context(|| format!("страница {home_url} ответила ошибкой"))?
        .text()
        .await
        .with_context(|| format!("не удалось прочитать тело страницы {home_url}"))?;

    for url in script_urls(&home).into_iter().rev() {
        let path = bundle_path(&url);
        let bundle_url = format!("{}{}", endpoints.script_base, path);
        let body = match client.get(&bundle_url).send().await {
            Ok(response) => match response.text().await {
                Ok(text) => text,
                Err(err) => {
                    tracing::debug!("soundcloud: скрипт {bundle_url} недоступен: {err}");
                    continue;
                }
            },
            Err(err) => {
                tracing::debug!("soundcloud: скрипт {bundle_url} недоступен: {err}");
                continue;
            }
        };
        if let Some(client_id) = find_client_id(&body) {
            return Ok(client_id);
        }
    }

    anyhow::bail!("soundcloud: client_id not found in the page bundles")
}

/// Everything from the first `/` after scheme and host: the path of a URL.
fn bundle_path(url: &str) -> &str {
    let after_scheme = url.split("://").nth(1).unwrap_or(url);
    match after_scheme.find('/') {
        Some(start) => &after_scheme[start..],
        None => "/",
    }
}

/// The `client_id` cached in `credentials.toml`, if a non-empty one is there.
///
/// A missing, unreadable or unparsable file simply means "no cache".
pub fn cached_client_id(dir: &Path) -> Option<String> {
    let client_id = Credentials::load_from(dir).ok()?.soundcloud?.client_id;
    if client_id.is_empty() {
        None
    } else {
        Some(client_id)
    }
}

/// Writes `client_id` into the `[soundcloud]` section of `credentials.toml`,
/// leaving every other credential, including `oauth_token`, untouched.
pub fn store_client_id(dir: &Path, client_id: &str) -> anyhow::Result<()> {
    let mut creds = Credentials::load_from(dir)?;
    let section = creds
        .soundcloud
        .get_or_insert_with(SoundCloudCredentials::default);
    section.client_id = client_id.to_string();
    creds.save_to(dir)
}

/// The cached id when there is one, a fresh scrape otherwise (then cached).
pub async fn ensure_client_id(
    dir: &Path,
    client: &reqwest::Client,
    endpoints: &SoundCloudEndpoints,
) -> anyhow::Result<String> {
    if let Some(client_id) = cached_client_id(dir) {
        return Ok(client_id);
    }
    let client_id = scrape_client_id(client, endpoints).await?;
    store_client_id(dir, &client_id)?;
    Ok(client_id)
}

/// The §8.9 `GET /users/{id}` profile that `yt-dlp` cannot give us.
#[derive(Clone, Debug, PartialEq)]
pub struct SoundCloudUser {
    pub id: u64,
    pub username: String,
    pub permalink: String,
    pub followers_count: u64,
    pub city: Option<String>,
    pub country: Option<String>,
}

/// Parses one SoundCloud user profile JSON body.
///
/// Extra fields are ignored on purpose: the real API returns far more than
/// the five fields this build reads.
pub fn parse_user(json: &str) -> anyhow::Result<SoundCloudUser> {
    let value: Value = serde_json::from_str(json)?;

    let id = value
        .get("id")
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow::anyhow!("у профиля SoundCloud нет поля id"))?;
    let username = value
        .get("username")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("у профиля SoundCloud нет поля username"))?
        .to_string();
    let permalink = value
        .get("permalink")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("у профиля SoundCloud нет поля permalink"))?
        .to_string();

    Ok(SoundCloudUser {
        id,
        username,
        permalink,
        followers_count: value
            .get("followers_count")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        city: value
            .get("city")
            .and_then(Value::as_str)
            .map(str::to_string),
        country: value
            .get("country")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

/// One `GET /users/{id}?client_id=...` call, parsed into a [`SoundCloudUser`].
pub async fn get_user(
    client: &reqwest::Client,
    endpoints: &SoundCloudEndpoints,
    client_id: &str,
    user_id: u64,
) -> anyhow::Result<SoundCloudUser> {
    let url = format!(
        "{}/users/{}?client_id={}",
        endpoints.api_base, user_id, client_id
    );
    let body = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("не удалось скачать профиль {url}"))?
        .error_for_status()
        .with_context(|| format!("профиль {url} ответил ошибкой"))?
        .text()
        .await
        .with_context(|| format!("не удалось прочитать тело профиля {url}"))?;
    parse_user(&body)
}

// ─── Track catalog (§8.9, `GET /tracks/{id}`) ────────────────────────────────
//
// Pure parsing and classification only: fetching the body is Task 8. Extra
// body fields are ignored on purpose, mirroring `parse_user`.

/// One `media.transcodings[]` entry: a stream variant the API offers.
#[derive(Clone, Debug, PartialEq)]
pub struct Transcoding {
    pub preset: String,
    pub protocol: String,
    pub mime_type: String,
}

/// The `GET /tracks/{id}` body reduced to the fields this build reads.
#[derive(Clone, Debug, PartialEq)]
pub struct SoundCloudTrack {
    pub id: u64,
    pub title: String,
    pub duration_ms: u64,
    pub created_at: String,
    pub genre: Option<String>,
    pub license: Option<String>,
    pub permalink_url: String,
    pub isrc: Option<String>,
    pub publisher_artist: Option<String>,
    pub label_name: Option<String>,
    pub user_id: u64,
    pub username: String,
    pub user_permalink: String,
    pub downloadable: bool,
    pub has_downloads_left: bool,
    pub transcodings: Vec<Transcoding>,
}

impl SoundCloudTrack {
    /// The canonical `Release` converter for SoundCloud tracks: stage 2
    /// reuses exactly this method and defines no second converter.
    pub fn release_from_track(&self) -> Release {
        let artist = match self.publisher_artist.as_deref() {
            Some(artist) if !artist.is_empty() => artist.to_string(),
            _ => self.username.clone(),
        };
        Release {
            title: self.title.clone(),
            artist,
            album: None,
            track_no: None,
            duration_sec: Some(self.duration_ms as f64 / 1000.0),
            isrc: self.isrc.clone(),
            year: self.created_at.get(..4).and_then(|s| s.parse::<i32>().ok()),
            source: SourceId::SoundCloud,
            source_item_id: self.id.to_string(),
        }
    }
}

/// A required track field by dot path (`user.id`), missing as in `parse_user`.
fn track_field<'a>(value: &'a Value, path: &str) -> anyhow::Result<&'a Value> {
    let mut current = value;
    for segment in path.split('.') {
        current = current
            .get(segment)
            .ok_or_else(|| anyhow::anyhow!("у трека SoundCloud нет поля {path}"))?;
    }
    Ok(current)
}

/// A required `u64` track field by dot path.
fn track_u64(value: &Value, path: &str) -> anyhow::Result<u64> {
    track_field(value, path)?
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("у трека SoundCloud нет поля {path}"))
}

/// A required string track field by dot path.
fn track_str(value: &Value, path: &str) -> anyhow::Result<String> {
    track_field(value, path)?
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("у трека SoundCloud нет поля {path}"))
}

/// `media.transcodings[]` as typed entries: a missing array degrades to an
/// empty `Vec`, a broken entry to an unknown transcoding (never an error).
fn track_transcodings(value: &Value) -> Vec<Transcoding> {
    value
        .get("media")
        .and_then(|media| media.get("transcodings"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| Transcoding {
                    preset: item
                        .get("preset")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    protocol: item
                        .get("protocol")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    mime_type: item
                        .get("mime_type")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parses one SoundCloud track JSON body (`GET /tracks/{id}`).
pub fn parse_track(json: &str) -> anyhow::Result<SoundCloudTrack> {
    let value: Value = serde_json::from_str(json)?;

    let id = track_u64(&value, "id")?;
    let title = track_str(&value, "title")?;
    let duration_ms = track_u64(&value, "duration")?;
    let created_at = track_str(&value, "created_at")?;
    let permalink_url = track_str(&value, "permalink_url")?;

    let publisher_metadata = value.get("publisher_metadata");
    let isrc = publisher_metadata
        .and_then(|metadata| metadata.get("isrc"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let publisher_artist = publisher_metadata
        .and_then(|metadata| metadata.get("artist"))
        .and_then(Value::as_str)
        .map(str::to_string);

    Ok(SoundCloudTrack {
        id,
        title,
        duration_ms,
        created_at,
        genre: value
            .get("genre")
            .and_then(Value::as_str)
            .map(str::to_string),
        license: value
            .get("license")
            .and_then(Value::as_str)
            .map(str::to_string),
        permalink_url,
        isrc,
        publisher_artist,
        label_name: value
            .get("label_name")
            .and_then(Value::as_str)
            .map(str::to_string),
        user_id: track_u64(&value, "user.id")?,
        username: track_str(&value, "user.username")?,
        user_permalink: track_str(&value, "user.permalink")?,
        downloadable: value
            .get("downloadable")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        has_downloads_left: value
            .get("has_downloads_left")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        transcodings: track_transcodings(&value),
    })
}

/// The locator field that distinguishes the lossless original from the
/// lossy AAC transcodes sharing one `track_id`.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SoundCloudFormat {
    Original,
    Aac160,
    Aac96,
    M3u8,
}

/// One downloadable audio stream of a track, classified.
#[derive(Clone, Debug, PartialEq)]
pub enum AudioFormat {
    LossyAac { bitrate_kbps: u32 },
    ProgressiveOriginal { protocol: String },
}

impl AudioFormat {
    /// Classifies one transcoding entry; `None` for unknown presets or
    /// protocols.
    pub fn from_transcoding(t: &Transcoding) -> Option<AudioFormat> {
        match (t.preset.as_str(), t.protocol.as_str()) {
            ("aac_160k", "hls") => Some(AudioFormat::LossyAac { bitrate_kbps: 160 }),
            ("aac_96k", "hls") => Some(AudioFormat::LossyAac { bitrate_kbps: 96 }),
            (_, "http_mp3" | "http_opus" | "http_aac" | "progressive") => {
                Some(AudioFormat::ProgressiveOriginal {
                    protocol: t.protocol.clone(),
                })
            }
            _ => None,
        }
    }

    /// Picks the best stream: the first progressive original, if any,
    /// otherwise the highest-bitrate lossy AAC. Empty input yields `None`.
    pub fn pick(transcodings: &[Transcoding]) -> Option<AudioFormat> {
        let classified: Vec<AudioFormat> = transcodings
            .iter()
            .filter_map(AudioFormat::from_transcoding)
            .collect();
        if let Some(original) = classified
            .iter()
            .find(|format| matches!(format, AudioFormat::ProgressiveOriginal { .. }))
        {
            return Some(original.clone());
        }
        classified
            .into_iter()
            .filter_map(|format| match format {
                AudioFormat::LossyAac { bitrate_kbps } => Some((bitrate_kbps, format)),
                AudioFormat::ProgressiveOriginal { .. } => None,
            })
            .max_by_key(|(bitrate_kbps, _)| *bitrate_kbps)
            .map(|(_, format)| format)
    }

    /// The `Quality` this stream offers.
    ///
    /// For a progressive original the container is preliminary: the pipeline
    /// replaces it with the probed quality before anything is written, and
    /// Task 9 asserts the scanned (not this preliminary) container in the DB.
    pub fn quality(&self) -> Quality {
        match self {
            AudioFormat::LossyAac { bitrate_kbps } => Quality {
                lossless: false,
                container: Container::M4aAac,
                bit_depth: None,
                sample_rate: None,
                bitrate_kbps: Some(*bitrate_kbps),
            },
            AudioFormat::ProgressiveOriginal { protocol } => match protocol.as_str() {
                "http_mp3" => original_quality(Container::Mp3),
                "http_opus" => original_quality(Container::Opus),
                "http_aac" => original_quality(Container::M4aAac),
                "progressive" => original_quality(Container::Flac),
                // Unreachable from `from_transcoding`; kept total because the
                // protocol is an open `String`. A neutral fallback, not a panic.
                _ => Quality::default(),
            },
        }
    }
}

/// A lossless-original `Quality` with everything unprobed left empty.
fn original_quality(container: Container) -> Quality {
    Quality {
        lossless: true,
        container,
        bit_depth: None,
        sample_rate: None,
        bitrate_kbps: None,
    }
}

/// The `yt-dlp -f` format name for a classified stream (consumed by Task 8).
pub fn ytdlp_format(format: &AudioFormat) -> &'static str {
    match format {
        AudioFormat::LossyAac { bitrate_kbps: 160 } => "hls_aac_160k",
        AudioFormat::LossyAac { bitrate_kbps: 96 } => "hls_aac_96k",
        // Only 160/96 arise from `from_transcoding`; kept total because the
        // bitrate is an open `u32`.
        AudioFormat::LossyAac { .. } => "hls_aac_96k",
        AudioFormat::ProgressiveOriginal { .. } => "bestaudio",
    }
}

/// The locator `format` field naming the picked stream. `M3u8` is not
/// produced here: it is the stage 2 slot for an HLS playlist that is neither
/// 160k nor 96k.
pub fn format_of(audio_format: &AudioFormat) -> SoundCloudFormat {
    match audio_format {
        AudioFormat::LossyAac { bitrate_kbps: 160 } => SoundCloudFormat::Aac160,
        AudioFormat::LossyAac { bitrate_kbps: 96 } => SoundCloudFormat::Aac96,
        // Only 160/96 arise from `from_transcoding`; kept total because the
        // bitrate is an open `u32`.
        AudioFormat::LossyAac { .. } => SoundCloudFormat::Aac96,
        AudioFormat::ProgressiveOriginal { .. } => SoundCloudFormat::Original,
    }
}
