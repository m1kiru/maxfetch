//! Bandcamp URL classification, tralbum metadata and promo archives.
//!
//! The module is split in three layers: the pure parsers of the recorded pages
//! and JSON payloads, the filesystem archive layer ([`is_zip`],
//! [`unpack_archive`], [`unpack_cached`]) and [`BandcampSource`], the single
//! network consumer. The module owns no global client and performs no request
//! at import time; every HTTP call happens inside a `BandcampSource` method.

use std::fs::File;
use std::io::{Read, copy};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, anyhow};
use async_trait::async_trait;
use chrono::Datelike;
use regex::Regex;
use serde_json::{Map, Value};

use crate::config::{Config, Credentials, DownloadConfig};
use crate::fetch::{ProgressSink, http_client};
use crate::model::{Candidate, Container, Locator, Quality, Release, SourceId};
use crate::sources::{AuthState, Capabilities, Query, Source};

/// Which kind of tralbum page a Bandcamp id pair describes.
///
/// [`TralbumType::as_str`] yields the value Bandcamp expects in the
/// `tralbum_type` query parameter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TralbumType {
    Album,
    Track,
}

impl TralbumType {
    /// The `tralbum_type` parameter value: `"a"` for an album, `"t"` for a track.
    pub const fn as_str(&self) -> &'static str {
        match self {
            TralbumType::Album => "a",
            TralbumType::Track => "t",
        }
    }
}

/// What a Bandcamp storefront URL points at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TralbumKind {
    Artist,
    Album,
    Track,
}

/// The identifier pair read from an album or track page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BandcampIds {
    pub band_id: u64,
    pub tralbum_id: u64,
}

/// Classifies a Bandcamp storefront URL.
///
/// The host must be `bandcamp.com` or a subdomain of it (case-insensitive).
/// The path wins in the order track, album, artist; a bare `bandcamp.com`
/// root is not an artist page and yields `None`.
pub fn classify_url(url: &str) -> Option<TralbumKind> {
    let after_scheme = url.split_once("://")?.1;
    let (host, remainder) = match after_scheme.find('/') {
        Some(index) => (&after_scheme[..index], &after_scheme[index..]),
        None => (after_scheme, ""),
    };

    let host = host.to_ascii_lowercase();
    if host != "bandcamp.com" && !host.ends_with(".bandcamp.com") {
        return None;
    }

    if remainder.contains("/track/") {
        Some(TralbumKind::Track)
    } else if remainder.contains("/album/") {
        Some(TralbumKind::Album)
    } else if host == "bandcamp.com" {
        None
    } else {
        Some(TralbumKind::Artist)
    }
}

/// Reads `band_id` and `tralbum_id` from an album or track page.
///
/// Both identifiers are required: a page that carries only one yields `None`.
pub fn parse_bandcamp_ids(html: &str) -> Option<BandcampIds> {
    Some(BandcampIds {
        band_id: first_digit_capture(band_id_regex(), html)?,
        tralbum_id: first_digit_capture(tralbum_id_regex(), html)?,
    })
}

/// Returns the first match of `re`, parsed as a `u64`.
///
/// The patterns only capture ASCII digits, so the parse cannot fail.
fn first_digit_capture(re: &Regex, html: &str) -> Option<u64> {
    re.captures(html)?.get(1)?.as_str().parse().ok()
}

fn band_id_regex() -> &'static Regex {
    static BAND_ID: OnceLock<Regex> = OnceLock::new();
    BAND_ID.get_or_init(|| Regex::new(r"&amp;band_id=([0-9]+)").expect("band_id pattern is valid"))
}

fn tralbum_id_regex() -> &'static Regex {
    static TRALBUM_ID: OnceLock<Regex> = OnceLock::new();
    TRALBUM_ID.get_or_init(|| {
        Regex::new(r#""name":"item_id","value":([0-9]+)"#).expect("item_id pattern is valid")
    })
}

/// One hit from `/api/fuzzysearch/2/app_autocomplete`.
#[derive(Clone, Debug, PartialEq)]
pub struct FuzzyHit {
    pub kind: TralbumKind,
    pub id: u64,
    pub band_id: Option<u64>,
    pub location: Option<String>,
}

/// Parses the autocomplete search response.
///
/// Only the three known result types survive; everything else is skipped. The
/// hit `id` is `band_id` for an artist and `item_id` for an album or track, and
/// the `band_id` that `tralbum_details` needs comes either from the direct
/// field or from the `band_id=` query parameter of `location`.
pub fn parse_fuzzy_search(json: &str) -> anyhow::Result<Vec<FuzzyHit>> {
    let root: Value = serde_json::from_str(json)?;
    let Some(results) = root.get("results").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };

    let mut hits = Vec::with_capacity(results.len());
    for element in results {
        let Some(element) = element.as_object() else {
            continue;
        };
        let Some(kind) = fuzzy_kind(element) else {
            continue;
        };
        let id_source = match kind {
            TralbumKind::Artist => "band_id",
            TralbumKind::Album | TralbumKind::Track => "item_id",
        };
        let Some(id) = numeric_field(element, id_source) else {
            continue;
        };
        let band_id = element.get("band_id").and_then(field_u64).or_else(|| {
            element
                .get("location")
                .and_then(Value::as_str)
                .and_then(band_id_from_location)
        });
        let location = element
            .get("location")
            .and_then(Value::as_str)
            .map(str::to_owned);
        hits.push(FuzzyHit {
            kind,
            id,
            band_id,
            location,
        });
    }
    Ok(hits)
}

/// Maps the fuzzy search `type`/`item_type` pair to a result kind.
///
/// `"b"` is an artist, `"a"`/`"t"` an album/track; when `type` is missing
/// `item_type` is consulted; any other value is not a usable result.
fn fuzzy_kind(element: &Map<String, Value>) -> Option<TralbumKind> {
    match element.get("type").and_then(Value::as_str) {
        Some("b") => Some(TralbumKind::Artist),
        Some("a") => Some(TralbumKind::Album),
        Some("t") => Some(TralbumKind::Track),
        None => match element.get("item_type").and_then(Value::as_str) {
            Some("a") => Some(TralbumKind::Album),
            Some("t") => Some(TralbumKind::Track),
            _ => None,
        },
        _ => None,
    }
}

/// The leading ASCII digits after a `band_id=` parameter, if any.
fn band_id_from_location(location: &str) -> Option<u64> {
    let tail = location.split("band_id=").nth(1)?;
    let digits: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        None
    } else {
        digits.parse().ok()
    }
}

/// Reads a field that is either a JSON number or a numeric string.
fn numeric_field(object: &Map<String, Value>, key: &str) -> Option<u64> {
    match object.get(key)? {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.parse().ok(),
        _ => None,
    }
}

/// A JSON number read as `u64`.
fn field_u64(value: &Value) -> Option<u64> {
    value.as_u64()
}

/// A track inside a tralbum_details response.
#[derive(Clone, Debug, PartialEq)]
pub struct BandcampTrack {
    pub track_id: u64,
    pub title: String,
    pub track_num: u32,
    pub duration_sec: f64,
    pub band_name: String,
    pub album_title: String,
    pub is_streamable: bool,
    pub track_url: String,
}

/// A parsed `/api/tralbum_details` response.
#[derive(Clone, Debug, PartialEq)]
pub struct BandcampRelease {
    pub tralbum_id: u64,
    pub tralbum_type: TralbumType,
    pub title: String,
    pub artist: String,
    pub release_date_unix: u64,
    pub free_download: bool,
    pub is_purchasable: bool,
    pub price: Option<f64>,
    pub currency: Option<String>,
    pub has_digital_download: bool,
    pub tracks: Vec<BandcampTrack>,
}

/// Parses the tralbum_details JSON.
///
/// `id` and `tracks` are required; every other field has a default. The
/// mobile `streaming_url` of each track is deliberately not read: the mp3-128
/// stream is never used as a candidate.
pub fn parse_tralbum_details(json: &str) -> anyhow::Result<BandcampRelease> {
    let root: Value = serde_json::from_str(json)?;
    let object = root
        .as_object()
        .ok_or_else(|| anyhow!("tralbum_details root is not an object"))?;
    let tralbum_id = numeric_field(object, "id")
        .ok_or_else(|| anyhow!("tralbum_details has no numeric `id`"))?;
    let tracks = object
        .get("tracks")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("tralbum_details has no `tracks` array"))?;

    let mut parsed_tracks = Vec::with_capacity(tracks.len());
    for (index, track) in tracks.iter().enumerate() {
        parsed_tracks.push(parse_track(track, index)?);
    }

    Ok(BandcampRelease {
        tralbum_id,
        tralbum_type: tralbum_type(object),
        title: string_field(object, "title"),
        artist: string_field(object, "tralbum_artist"),
        release_date_unix: object
            .get("release_date")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        free_download: object
            .get("free_download")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        is_purchasable: object
            .get("is_purchasable")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        price: object.get("price").and_then(Value::as_f64),
        currency: object
            .get("currency")
            .and_then(Value::as_str)
            .map(str::to_owned),
        has_digital_download: object
            .get("has_digital_download")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        tracks: parsed_tracks,
    })
}

/// The `tralbum_type`/`item_type` pair, defaulting to [`TralbumType::Album`].
fn tralbum_type(object: &Map<String, Value>) -> TralbumType {
    match object
        .get("tralbum_type")
        .and_then(Value::as_str)
        .or_else(|| object.get("item_type").and_then(Value::as_str))
    {
        Some("t") | Some("track") => TralbumType::Track,
        _ => TralbumType::Album,
    }
}

/// A single `tracks[]` element; `track_id` is required.
fn parse_track(value: &Value, index: usize) -> anyhow::Result<BandcampTrack> {
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("track {index} is not an object"))?;
    let track_id = numeric_field(object, "track_id")
        .ok_or_else(|| anyhow!("track {index} has no numeric `track_id`"))?;
    Ok(BandcampTrack {
        track_id,
        title: string_field(object, "title"),
        track_num: object
            .get("track_num")
            .and_then(Value::as_u64)
            .map(|n| n as u32)
            .unwrap_or(index as u32 + 1),
        duration_sec: object
            .get("duration")
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
        band_name: string_field(object, "band_name"),
        album_title: string_field(object, "album_title"),
        is_streamable: object
            .get("is_streamable")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        track_url: string_field(object, "track_url"),
    })
}

impl BandcampRelease {
    /// Converts the parsed release into one catalogue [`Release`] per streamable
    /// track; non-streamable tracks are simply absent.
    pub fn to_releases(&self, page_url: &str) -> Vec<Release> {
        self.tracks
            .iter()
            .filter(|track| track.is_streamable)
            .map(|track| Release {
                title: track.title.clone(),
                artist: self.artist.clone(),
                album: (!track.album_title.is_empty()).then(|| track.album_title.clone()),
                track_no: Some(track.track_num),
                duration_sec: Some(track.duration_sec),
                isrc: None,
                year: chrono::DateTime::from_timestamp(self.release_date_unix as i64, 0)
                    .map(|date| date.year()),
                source: SourceId::Bandcamp,
                source_item_id: format!("{page_url}|{}", track.track_id),
            })
            .collect()
    }
}

/// Splits a `source_item_id` back into the page URL and the track id.
///
/// The split is at the last `|` so a page URL that itself contains one keeps
/// working; the trailing id must be numeric.
pub fn split_source_item_id(id: &str) -> Option<(&str, u64)> {
    let (url, track_id) = id.rsplit_once('|')?;
    Some((url, track_id.parse().ok()?))
}

/// One discography entry from `/api/band/{band_id}/details`.
#[derive(Clone, Debug, PartialEq)]
pub struct DiscographyItem {
    pub item_id: u64,
    pub item_type: TralbumType,
    pub title: String,
    pub release_date_unix: u64,
}

/// Parses the band details JSON into its `discography` array.
///
/// A missing `discography` yields an empty list; elements without a usable
/// `item_id` or `item_type` are skipped.
pub fn parse_band_details(json: &str) -> anyhow::Result<Vec<DiscographyItem>> {
    let root: Value = serde_json::from_str(json)?;
    let Some(discography) = root.get("discography").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };

    let mut items = Vec::with_capacity(discography.len());
    for element in discography {
        let Some(element) = element.as_object() else {
            continue;
        };
        let Some(item_id) = numeric_field(element, "item_id") else {
            continue;
        };
        let item_type = match element.get("item_type").and_then(Value::as_str) {
            Some("a") | Some("album") => TralbumType::Album,
            Some("t") | Some("track") => TralbumType::Track,
            _ => continue,
        };
        items.push(DiscographyItem {
            item_id,
            item_type,
            title: string_field(element, "title"),
            release_date_unix: element
                .get("release_date")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        });
    }
    Ok(items)
}

/// The string value of a field, or `""` when it is missing or not a string.
fn string_field(object: &Map<String, Value>, key: &str) -> String {
    object
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned()
}

/// The promo block carried by a tralbum page's `data-tralbum` attribute.
#[derive(Clone, Debug, PartialEq)]
pub struct DataTralbum {
    pub free_download_page: Option<String>,
}

/// Reads the promo page from the `data-tralbum` attribute of a tralbum page.
///
/// The attribute value is HTML-escaped JSON: `&quot;` and `&#39;` are
/// unescaped first and `&amp;` last, so `&amp;quot;` cannot collapse into a
/// quote. A page without the attribute, a value without a closing quote or
/// invalid JSON all yield `None`; a blob without `freeDownloadPage` yields
/// `Some` with `free_download_page: None`, so callers can tell "no blob" from
/// "blob without a promo".
pub fn parse_data_tralbum(html: &str) -> Option<DataTralbum> {
    const MARKER: &str = "data-tralbum=\"";
    let start = html.find(MARKER)? + MARKER.len();
    let rest = &html[start..];
    let end = rest.find('"')?;
    let raw = &rest[..end];
    let unescaped = raw
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    let value: Value = serde_json::from_str(&unescaped).ok()?;
    Some(DataTralbum {
        free_download_page: value
            .get("freeDownloadPage")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

/// Rewrites a promo `/download` URL into its `/statdownload` form with a nonce.
///
/// The `/download` path segment is replaced by `/statdownload`, the original
/// query string is kept verbatim and `&.rand=<nonce>` is appended. The
/// separator comes from the source URL: `?` when it carries no query and `&`
/// when it does, so the result is always a parseable URL and never
/// `.../statdownload&.rand=N` without a `?`. `None` when the path has no
/// `/download` segment.
pub fn statdownload_url(download_url: &str, nonce: u64) -> Option<String> {
    let (path, query) = match download_url.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (download_url, None),
    };
    let path = if let Some(prefix) = path.strip_suffix("/download") {
        format!("{prefix}/statdownload")
    } else if path.contains("/download/") {
        path.replacen("/download/", "/statdownload/", 1)
    } else {
        return None;
    };
    Some(match query {
        Some(query) => format!("{path}?{query}&.rand={nonce}"),
        None => format!("{path}?&.rand={nonce}"),
    })
}

/// The `var _statDL_result = {...}` assignment answered by `/statdownload`.
#[derive(Clone, Debug, PartialEq)]
pub struct StatDownloadResult {
    pub retry_url: Option<String>,
    pub err_type: Option<String>,
}

/// Parses the JSON object assigned to `_statDL_result`.
///
/// Bandcamp answers the assignment with a trailing `;`, so the object is read
/// with a streaming deserializer that stops after the first value; the tail is
/// ignored. A body without the marker, an invalid object, or a non-object
/// value all yield `None`.
pub fn parse_statdownload_result(body: &str) -> Option<StatDownloadResult> {
    const MARKER: &str = "var _statDL_result = ";
    let start = body.find(MARKER)? + MARKER.len();
    let rest = body[start..].trim_start();
    let value = serde_json::Deserializer::from_str(rest)
        .into_iter::<Value>()
        .next()?
        .ok()?;
    let object = value.as_object()?;
    Some(StatDownloadResult {
        retry_url: object
            .get("retry_url")
            .and_then(Value::as_str)
            .map(str::to_owned),
        err_type: object
            .get("errortype")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

/// Whether a `/statdownload` answer is Bandcamp's expired-download body.
pub fn is_expired_download(status: u16, body: &str) -> bool {
    status == 403 && body.trim() == "Download Expired"
}

/// The `enc` value from a retry URL, `"flac"` when the parameter is absent.
///
/// This is the encoding the promo archive really contains: it is chosen by
/// Bandcamp, not by us. The `enc` token is plain ASCII (`flac`, `mp3-320`,
/// `aac-hi`), so no percent-decoding is applied.
pub fn retry_encoding(retry_url: &str) -> String {
    let Some((_, query)) = retry_url.split_once('?') else {
        return "flac".to_owned();
    };
    for pair in query.split('&') {
        if let Some(value) = pair.strip_prefix("enc=") {
            return value.to_owned();
        }
    }
    "flac".to_owned()
}

/// The eight Bandcamp free-download format keys, lossless first.
pub const FREE_DOWNLOAD_FORMATS: [&str; 8] = [
    "flac", "alac", "wav", "vorbis", "mp3-v0", "mp3-320", "aac-hi", "mp3-128",
];

/// Maps a Bandcamp free-download format key to its fixed [`Quality`].
///
/// This is the single place that maps the keys. `bit_depth` and `sample_rate`
/// stay `None` because only the probe may fill measured values, and `mp3-v0`
/// is VBR and therefore carries no bitrate. `lossless` is derived from the
/// container so the table cannot drift from [`Container::is_lossless`]. Keys
/// outside the table (`zip`, `aiff`) yield `None`.
pub fn format_quality(fmt: &str) -> Option<Quality> {
    let (container, bitrate_kbps) = match fmt {
        "flac" => (Container::Flac, None),
        "alac" => (Container::Alac, None),
        "wav" => (Container::Wav, None),
        "vorbis" => (Container::Opus, None),
        "mp3-v0" => (Container::Mp3, None),
        "mp3-320" => (Container::Mp3, Some(320)),
        "aac-hi" => (Container::M4aAac, None),
        "mp3-128" => (Container::Mp3, Some(128)),
        _ => return None,
    };
    Some(Quality {
        lossless: container.is_lossless(),
        container,
        bit_depth: None,
        sample_rate: None,
        bitrate_kbps,
    })
}

/// One file extracted from a Bandcamp promo archive.
///
/// `member` is the member name inside the ZIP, `path` is where the file was
/// written under the destination directory.
#[derive(Clone, Debug, PartialEq)]
pub struct ArchiveEntry {
    pub member: String,
    pub path: PathBuf,
}

/// Whether the file at `path` starts with the ZIP local-file signature.
///
/// A file shorter than four bytes is not an archive; an unreadable file is an
/// error.
pub fn is_zip(path: &Path) -> anyhow::Result<bool> {
    let mut file = File::open(path)?;
    let mut head = [0u8; 4];
    let n = file.read(&mut head)?;
    Ok(n == 4 && head == *b"PK\x03\x04")
}

/// Extracts every file entry of the ZIP at `zip_path` under `dest`.
///
/// Directory entries, `__MACOSX/` prefixes and `.DS_Store` are skipped. Each
/// member name is joined into `dest` through [`crate::library::sanitize_component`]
/// per path segment, which also drops `.`/`..` segments. The result is sorted by
/// member name.
pub fn unpack_archive(zip_path: &Path, dest: &Path) -> anyhow::Result<Vec<ArchiveEntry>> {
    std::fs::create_dir_all(dest)?;
    let mut archive = zip::ZipArchive::new(File::open(zip_path)?)?;

    let mut entries = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        if entry.is_dir() {
            continue;
        }
        let member = entry.name()?.into_owned();
        if member.starts_with("__MACOSX/") || member == ".DS_Store" {
            continue;
        }
        let relative: PathBuf = member
            .split('/')
            .filter(|segment| !segment.is_empty())
            .map(crate::library::sanitize_component)
            .collect();
        if relative.as_os_str().is_empty() {
            continue;
        }
        let path = dest.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = File::create(&path)?;
        copy(&mut entry, &mut out)?;
        entries.push(ArchiveEntry { member, path });
    }

    entries.sort_by(|a, b| a.member.cmp(&b.member));
    Ok(entries)
}

/// `<cache_root>/bandcamp/<tralbum_id>`, the single key of the unpacked-archive cache.
///
/// Creates nothing; the caller creates the directories it writes to.
pub fn archive_dir(cache_root: &Path, tralbum_id: u64) -> PathBuf {
    cache_root.join("bandcamp").join(tralbum_id.to_string())
}

/// Where the downloaded promo archive is stored for a tralbum.
pub fn archive_zip_path(cache_root: &Path, tralbum_id: u64) -> PathBuf {
    archive_dir(cache_root, tralbum_id).join("promo.zip")
}

/// Where the unpacked members of a tralbum live.
pub fn archive_members_dir(cache_root: &Path, tralbum_id: u64) -> PathBuf {
    archive_dir(cache_root, tralbum_id).join("members")
}

/// Unpacks `zip_path` into the members directory of `tralbum_id`, or lists it.
///
/// When the members directory already holds files the ZIP is not opened again;
/// the archive is read once per tralbum per run.
pub fn unpack_cached(
    zip_path: &Path,
    cache_root: &Path,
    tralbum_id: u64,
) -> anyhow::Result<Vec<ArchiveEntry>> {
    let members_dir = archive_members_dir(cache_root, tralbum_id);
    if members_dir.is_dir() {
        let listing = list_members(&members_dir)?;
        if !listing.is_empty() {
            return Ok(listing);
        }
    }
    unpack_archive(zip_path, &members_dir)
}

/// Lists `dir` one level deep, sorted by member name; a missing dir is empty.
fn list_members(dir: &Path) -> anyhow::Result<Vec<ArchiveEntry>> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            continue;
        }
        entries.push(ArchiveEntry {
            member: entry.file_name().to_string_lossy().into_owned(),
            path: entry.path(),
        });
    }
    entries.sort_by(|a, b| a.member.cmp(&b.member));
    Ok(entries)
}

/// Pairs archive members with tracks, title-stem matches first, then position.
///
/// Pass one pairs a member whose normalized stem contains the normalized track
/// title with a still-unmatched track; pass two pairs the remaining members and
/// tracks in order. The result keeps the order of `members`; unmatched members
/// and unmatched tracks are dropped.
pub fn match_members_to_tracks(
    members: &[ArchiveEntry],
    tracks: &[BandcampTrack],
) -> Vec<(ArchiveEntry, BandcampTrack)> {
    let stem_norms: Vec<String> = members
        .iter()
        .map(|entry| {
            let stem = Path::new(&entry.member)
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default();
            let title = stem
                .trim_start_matches(|c: char| c.is_ascii_digit())
                .trim_start();
            crate::matching::norm(title)
        })
        .collect();
    let title_norms: Vec<String> = tracks
        .iter()
        .map(|track| crate::matching::norm(&track.title))
        .collect();

    let mut matches: Vec<Option<(ArchiveEntry, BandcampTrack)>> =
        members.iter().map(|_| None).collect();
    let mut taken = vec![false; tracks.len()];

    for (member_index, stem_norm) in stem_norms.iter().enumerate() {
        if stem_norm.is_empty() {
            continue;
        }
        let Some(track_index) = title_norms
            .iter()
            .enumerate()
            .find(|(index, title_norm)| {
                !taken[*index] && !title_norm.is_empty() && stem_norm.contains(*title_norm)
            })
            .map(|(index, _)| index)
        else {
            continue;
        };
        taken[track_index] = true;
        matches[member_index] = Some((members[member_index].clone(), tracks[track_index].clone()));
    }

    let leftover_members: Vec<usize> = (0..members.len())
        .filter(|index| matches[*index].is_none())
        .collect();
    let leftover_tracks: Vec<usize> = (0..tracks.len()).filter(|index| !taken[*index]).collect();

    for (member_index, track_index) in leftover_members.iter().zip(leftover_tracks.iter()) {
        matches[*member_index] =
            Some((members[*member_index].clone(), tracks[*track_index].clone()));
    }

    matches.into_iter().flatten().collect()
}

/// The leading track number of a member stem, if any.
///
/// `"01 Neon"` is `Some(1)`, `"12 Dawn"` is `Some(12)`, `"Neon"` is `None`.
pub fn track_number_from_stem(stem: &str) -> Option<u32> {
    let digits: String = stem.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        None
    } else {
        digits.parse().ok()
    }
}

/// Whether a release offers a free promo download.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PromoAvailability {
    Free,
    PaidOnly,
    Unavailable,
}

impl PromoAvailability {
    /// The user-facing line describing the availability.
    pub fn message(&self) -> &'static str {
        match self {
            PromoAvailability::Free => "бесплатное промо",
            PromoAvailability::PaidOnly | PromoAvailability::Unavailable => "нет бесплатного промо",
        }
    }

    /// Whether a promo archive can be fetched at all.
    pub fn is_free(&self) -> bool {
        matches!(self, PromoAvailability::Free)
    }
}

/// Classifies a parsed tralbum by its free-download flag and its store flags.
pub fn promo_availability(release: &BandcampRelease) -> PromoAvailability {
    if release.free_download {
        PromoAvailability::Free
    } else if release.is_purchasable || release.has_digital_download {
        PromoAvailability::PaidOnly
    } else {
        PromoAvailability::Unavailable
    }
}

/// The parts of a Bandcamp locator as understood by the source.
#[derive(Clone, Debug, PartialEq)]
pub struct BandcampTarget {
    /// The download (or retry) URL without the archive-member fragment.
    pub request_url: String,
    /// The archive member name carried percent-decoded in the fragment.
    pub member: Option<String>,
    /// The Bandcamp free-download format key.
    pub encoding: String,
    /// The tralbum id recovered from the `request_url` query, when present.
    pub tralbum_id: Option<u64>,
}

/// Builds a locator that addresses one member of a promo archive.
pub fn make_member_locator(request_url: &str, member: &str, encoding: &str) -> Locator {
    Locator::Bandcamp {
        url: format!("{request_url}#{member}"),
        encoding: encoding.to_owned(),
    }
}

/// Reads a Bandcamp locator back into its parts; a foreign locator is `None`.
pub fn parse_locator(loc: &Locator) -> Option<BandcampTarget> {
    let Locator::Bandcamp { url, encoding } = loc else {
        return None;
    };
    let (request_url, member) = match url.split_once('#') {
        Some((request_url, fragment)) => (request_url, Some(percent_decode(fragment))),
        None => (url.as_str(), None),
    };
    Some(BandcampTarget {
        request_url: request_url.to_owned(),
        member,
        encoding: encoding.clone(),
        tralbum_id: tralbum_id_from_url(request_url),
    })
}

/// Recovers the tralbum id from the retry URL query, most specific name first.
fn tralbum_id_from_url(url: &str) -> Option<u64> {
    ["tralbum_id", "album_id", "id"]
        .into_iter()
        .find_map(|name| query_param(url, name).and_then(|raw| raw.parse().ok()))
}

/// The value of the first query parameter called `name`.
fn query_param<'a>(url: &'a str, name: &str) -> Option<&'a str> {
    let (_, query) = url.split_once('?')?;
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then_some(value)
    })
}

/// Decodes `%XX` escapes; invalid escapes are kept verbatim.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
            if let Some(byte) = hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                decoded.push(byte);
                index += 3;
                continue;
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// Percent-encodes everything except the RFC 3986 unreserved characters.
fn percent_encode(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            b' ' => encoded.push_str("%20"),
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

/// The path part of a URL, with the query string removed.
fn url_path(url: &str) -> &str {
    match url.split_once('?') {
        Some((path, _)) => path,
        None => url,
    }
}

/// Classifies a URL by its path alone; the host is not consulted.
fn page_kind(url: &str) -> TralbumKind {
    let path = url_path(url);
    if path.contains("/track/") {
        TralbumKind::Track
    } else if path.contains("/album/") {
        TralbumKind::Album
    } else {
        TralbumKind::Artist
    }
}

/// Maps a page kind to the `tralbum_type` an API call needs.
fn tralbum_type_of(kind: TralbumKind) -> TralbumType {
    match kind {
        TralbumKind::Track => TralbumType::Track,
        TralbumKind::Album | TralbumKind::Artist => TralbumType::Album,
    }
}

/// Builds the track-level [`Release`] of one matched archive member.
///
/// [`BandcampRelease::to_releases`] drops non-streamable tracks, so a member
/// cannot be indexed by its position there; this repeats the same fields for
/// the track that was actually matched.
fn release_from_track(tralbum: &BandcampRelease, page_url: &str, track: &BandcampTrack) -> Release {
    Release {
        title: track.title.clone(),
        artist: tralbum.artist.clone(),
        album: (!track.album_title.is_empty()).then(|| track.album_title.clone()),
        track_no: Some(track.track_num),
        duration_sec: Some(track.duration_sec),
        isrc: None,
        year: chrono::DateTime::from_timestamp(tralbum.release_date_unix as i64, 0)
            .map(|date| date.year()),
        source: SourceId::Bandcamp,
        source_item_id: format!("{page_url}|{}", track.track_id),
    }
}

/// Finds an already unpacked member under any tralbum of the cache.
fn find_member_in_cache(cache_root: &Path, member: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(cache_root.join("bandcamp")).ok()?;
    for entry in entries.flatten() {
        let mut path = entry.path().join("members");
        for segment in member.split('/').filter(|segment| !segment.is_empty()) {
            path.push(crate::library::sanitize_component(segment));
        }
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

/// The Bandcamp API hosts the source talks to.
#[derive(Clone, Debug)]
pub struct BandcampEndpoints {
    pub api_base: String,
}

impl BandcampEndpoints {
    pub fn production() -> BandcampEndpoints {
        BandcampEndpoints {
            api_base: "https://bandcamp.com".to_owned(),
        }
    }
}

/// A promo archive fetched and unpacked for one tralbum.
struct PromoCached {
    request_url: String,
    encoding: String,
    single_track: bool,
    entries: Vec<ArchiveEntry>,
}

/// Bandcamp as a [`Source`].
#[derive(Clone)]
pub struct BandcampSource {
    endpoints: BandcampEndpoints,
    client: reqwest::Client,
    cache_root: PathBuf,
}

impl BandcampSource {
    /// The production source: the real Bandcamp hosts and the default cache.
    pub fn new(config: &Config, _credentials: &Credentials) -> BandcampSource {
        let client = match http_client(config.download.timeout_sec) {
            Ok(client) => client,
            Err(error) => {
                tracing::warn!("не удалось создать HTTP-клиент Bandcamp: {error}");
                reqwest::Client::new()
            }
        };
        Self::from_parts(
            BandcampEndpoints::production(),
            client,
            cache_root_default(),
        )
    }

    /// A source pointed at `api_base` and at an explicit cache root.
    pub fn with_api_base(api_base: &str, cache_root: &Path) -> BandcampSource {
        let client = http_client(DownloadConfig::default().timeout_sec).unwrap_or_else(|error| {
            tracing::warn!("не удалось создать HTTP-клиент Bandcamp: {error}");
            reqwest::Client::new()
        });
        Self::from_parts(
            BandcampEndpoints {
                api_base: api_base.trim_end_matches('/').to_owned(),
            },
            client,
            cache_root.to_path_buf(),
        )
    }

    /// Assembles the source and prepares the cache root best-effort.
    fn from_parts(
        endpoints: BandcampEndpoints,
        client: reqwest::Client,
        cache_root: PathBuf,
    ) -> BandcampSource {
        if let Err(error) = std::fs::create_dir_all(&cache_root) {
            tracing::warn!(
                "не удалось создать каталог кэша {}: {error}",
                cache_root.display()
            );
        }
        BandcampSource {
            endpoints,
            client,
            cache_root,
        }
    }

    /// Sends a request and returns its status with the body as text.
    async fn fetch_text(&self, url: &str) -> anyhow::Result<(u16, String)> {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .with_context(|| format!("запрос не удался: {url}"))?;
        let status = response.status().as_u16();
        let body = response
            .text()
            .await
            .with_context(|| format!("не удалось прочитать тело ответа: {url}"))?;
        Ok((status, body))
    }

    /// Sends a request and returns its status with the body as bytes.
    async fn fetch_bytes(&self, url: &str) -> anyhow::Result<(u16, Vec<u8>)> {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .with_context(|| format!("запрос не удался: {url}"))?;
        let status = response.status().as_u16();
        let body = response
            .bytes()
            .await
            .with_context(|| format!("не удалось прочитать тело ответа: {url}"))?
            .to_vec();
        Ok((status, body))
    }

    /// Reads one tralbum through `/api/mobile/25/tralbum_details`.
    async fn tralbum(
        &self,
        band_id: u64,
        tralbum_id: u64,
        kind: TralbumType,
    ) -> anyhow::Result<BandcampRelease> {
        let url = format!(
            "{}/api/mobile/25/tralbum_details?band_id={band_id}&tralbum_id={tralbum_id}&tralbum_type={}",
            self.endpoints.api_base,
            kind.as_str()
        );
        let (status, body) = self.fetch_text(&url).await?;
        if !(200..300).contains(&status) {
            anyhow::bail!("tralbum_details ответил статусом {status}: {url}");
        }
        parse_tralbum_details(&body).with_context(|| format!("tralbum_details: {url}"))
    }

    /// Reads a tralbum and maps its streamable tracks onto `page_url`.
    async fn releases_for_tralbum(
        &self,
        band_id: u64,
        tralbum_id: u64,
        kind: TralbumType,
        page_url: &str,
    ) -> anyhow::Result<Vec<Release>> {
        Ok(self
            .tralbum(band_id, tralbum_id, kind)
            .await?
            .to_releases(page_url))
    }

    /// Runs the promo chain, writes the archive to the cache and unpacks it.
    async fn promo_archive(
        &self,
        tralbum: &BandcampRelease,
        free_download_page: &str,
    ) -> anyhow::Result<Option<PromoCached>> {
        let (status, body) = self.fetch_bytes(free_download_page).await?;
        let (request_url, body) = if (200..300).contains(&status) {
            (free_download_page.to_owned(), body)
        } else {
            if is_expired_download(status, &String::from_utf8_lossy(&body)) {
                tracing::debug!("промо-ссылка Bandcamp просрочена: {free_download_page}");
            }
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|elapsed| elapsed.subsec_nanos() as u64)
                .unwrap_or(0);
            let Some(statdownload_url) = statdownload_url(free_download_page, nonce) else {
                return Ok(None);
            };
            let (stat_status, stat_body) = self.fetch_text(&statdownload_url).await?;
            if !(200..300).contains(&stat_status) {
                return Ok(None);
            }
            let Some(retry_url) =
                parse_statdownload_result(&stat_body).and_then(|result| result.retry_url)
            else {
                return Ok(None);
            };
            let (retry_status, retry_body) = self.fetch_bytes(&retry_url).await?;
            if !(200..300).contains(&retry_status) {
                return Ok(None);
            }
            (retry_url, retry_body)
        };

        let zip_path = archive_zip_path(&self.cache_root, tralbum.tralbum_id);
        if let Some(parent) = zip_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("создание каталога кэша {}", parent.display()))?;
        }
        std::fs::write(&zip_path, &body)
            .with_context(|| format!("запись архива {}", zip_path.display()))?;

        let encoding = retry_encoding(&request_url);
        let single_track = url_path(&request_url).contains("/download/track")
            || tralbum.tralbum_type == TralbumType::Track;
        if single_track {
            return Ok(Some(PromoCached {
                request_url,
                encoding,
                single_track,
                entries: Vec::new(),
            }));
        }

        if !is_zip(&zip_path)? {
            tracing::warn!(
                "{}: {} — {}",
                tralbum.artist,
                tralbum.title,
                "страница загрузки вернула не архив"
            );
            return Ok(None);
        }

        let entries = unpack_cached(&zip_path, &self.cache_root, tralbum.tralbum_id)?;
        Ok(Some(PromoCached {
            request_url,
            encoding,
            single_track,
            entries,
        }))
    }

    /// Fetches a promo archive into the cache of `tralbum_id`.
    async fn write_archive(&self, url: &str, tralbum_id: u64) -> anyhow::Result<PathBuf> {
        let (status, body) = self.fetch_bytes(url).await?;
        if !(200..300).contains(&status) {
            anyhow::bail!("архив Bandcamp недоступен: {url} (статус {status})");
        }
        let zip_path = archive_zip_path(&self.cache_root, tralbum_id);
        if let Some(parent) = zip_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("создание каталога кэша {}", parent.display()))?;
        }
        std::fs::write(&zip_path, body)
            .with_context(|| format!("запись архива {}", zip_path.display()))?;
        Ok(zip_path)
    }

    /// Locates one unpacked member, re-fetching the archive when the cache was cleared.
    async fn member_file(
        &self,
        target: &BandcampTarget,
        name: &str,
    ) -> anyhow::Result<Option<PathBuf>> {
        if let Some(id) = target.tralbum_id {
            let zip_path = archive_zip_path(&self.cache_root, id);
            if zip_path.is_file() {
                let entries = unpack_cached(&zip_path, &self.cache_root, id)?;
                return Ok(entries
                    .into_iter()
                    .find(|entry| entry.member == name)
                    .map(|entry| entry.path));
            }
        }
        if let Some(path) = find_member_in_cache(&self.cache_root, name) {
            return Ok(Some(path));
        }
        let Some(id) = target.tralbum_id else {
            return Ok(None);
        };
        self.write_archive(&target.request_url, id).await?;
        let entries = unpack_cached(
            &archive_zip_path(&self.cache_root, id),
            &self.cache_root,
            id,
        )?;
        Ok(entries
            .into_iter()
            .find(|entry| entry.member == name)
            .map(|entry| entry.path))
    }
}

#[async_trait]
impl Source for BandcampSource {
    fn id(&self) -> SourceId {
        SourceId::Bandcamp
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            audio: true,
            metadata_only: false,
            lossless: true,
            needs_auth: false,
        }
    }

    fn auth_state(&self) -> AuthState {
        AuthState::NotNeeded
    }

    async fn search(&self, q: &Query) -> anyhow::Result<Vec<Release>> {
        let text = q
            .text
            .as_deref()
            .or(q.url.as_deref())
            .map(str::trim)
            .unwrap_or("");
        // untested: no plan test covers a text query
        if text.is_empty() {
            anyhow::bail!("пустой поисковый запрос");
        }
        let url = format!(
            "{}/api/fuzzysearch/2/app_autocomplete?q={}&param_with_locations=true",
            self.endpoints.api_base,
            percent_encode(text)
        );
        let (status, body) = self.fetch_text(&url).await?;
        if !(200..300).contains(&status) {
            anyhow::bail!("поиск Bandcamp ответил статусом {status}: {url}");
        }
        let hits = parse_fuzzy_search(&body)?;

        let mut releases: Vec<Release> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for hit in hits
            .into_iter()
            .filter(|hit| matches!(hit.kind, TralbumKind::Album | TralbumKind::Track))
            .take(5)
        {
            let Some(band_id) = hit.band_id else {
                tracing::debug!("поиск Bandcamp: у результата нет band_id, пропуск");
                continue;
            };
            let page_url = hit.location.as_deref().map(url_path).unwrap_or("");
            let kind = tralbum_type_of(hit.kind);
            for release in self
                .releases_for_tralbum(band_id, hit.id, kind, page_url)
                .await?
            {
                if seen.insert(release.source_item_id.clone()) {
                    releases.push(release);
                }
            }
        }
        Ok(releases)
    }

    async fn resolve(&self, q: &Query) -> anyhow::Result<Vec<Release>> {
        let url = q
            .url
            .as_deref()
            .or(q.text.as_deref())
            .map(str::trim)
            .unwrap_or("");
        if url.is_empty() {
            anyhow::bail!("пустая ссылка Bandcamp");
        }

        match page_kind(url) {
            TralbumKind::Track | TralbumKind::Album => {
                let (status, page) = self.fetch_text(url).await?;
                let ids = (200..300)
                    .contains(&status)
                    .then(|| parse_bandcamp_ids(&page))
                    .flatten();
                let Some(ids) = ids else {
                    tracing::info!("нет бесплатного промо: {url}");
                    return Ok(Vec::new());
                };
                let kind = tralbum_type_of(page_kind(url));
                self.releases_for_tralbum(ids.band_id, ids.tralbum_id, kind, url)
                    .await
            }
            TralbumKind::Artist => {
                let (status, page) = self.fetch_text(url).await?;
                let band_id = (200..300)
                    .contains(&status)
                    .then(|| parse_bandcamp_ids(&page).map(|ids| ids.band_id))
                    .flatten()
                    .or_else(|| query_param(url, "band_id").and_then(|raw| raw.parse().ok()));
                let Some(band_id) = band_id else {
                    tracing::info!("дискография пуста: {url}");
                    return Ok(Vec::new());
                };
                let details_url = format!(
                    "{}/api/mobile/24/band_details?band_id={band_id}",
                    self.endpoints.api_base
                );
                let (details_status, details_body) = self.fetch_text(&details_url).await?;
                let items = if (200..300).contains(&details_status) {
                    parse_band_details(&details_body)?
                } else {
                    Vec::new()
                };
                if items.is_empty() {
                    tracing::info!("дискография пуста: {url}");
                    return Ok(Vec::new());
                }
                let mut releases = Vec::new();
                for item in items {
                    releases.extend(
                        self.releases_for_tralbum(band_id, item.item_id, item.item_type, url)
                            .await?,
                    );
                }
                Ok(releases)
            }
        }
    }

    async fn candidates(&self, release: &Release) -> anyhow::Result<Vec<Candidate>> {
        let Some((page_url, _track_id)) = split_source_item_id(&release.source_item_id) else {
            tracing::warn!(
                "{}: {} — некорректный source_item_id: {}",
                release.artist,
                release.title,
                release.source_item_id
            );
            return Ok(Vec::new());
        };
        let (status, page) = self.fetch_text(page_url).await?;
        let page_ids = (200..300)
            .contains(&status)
            .then(|| parse_bandcamp_ids(&page))
            .flatten();
        let free_download_page = (200..300)
            .contains(&status)
            .then(|| parse_data_tralbum(&page).and_then(|blob| blob.free_download_page))
            .flatten();
        let (Some(ids), Some(free_download_page)) = (page_ids, free_download_page) else {
            tracing::info!(
                "{}: {} — {}",
                release.artist,
                release.title,
                "нет бесплатного промо"
            );
            return Ok(Vec::new());
        };

        let tralbum = self
            .tralbum(
                ids.band_id,
                ids.tralbum_id,
                tralbum_type_of(page_kind(page_url)),
            )
            .await?;
        let availability = promo_availability(&tralbum);
        if !availability.is_free() {
            tracing::info!(
                "{}: {} — {}",
                release.artist,
                release.title,
                availability.message()
            );
            return Ok(Vec::new());
        }

        let Some(promo) = self.promo_archive(&tralbum, &free_download_page).await? else {
            tracing::info!(
                "{}: {} — {}",
                release.artist,
                release.title,
                "нет бесплатного промо"
            );
            return Ok(Vec::new());
        };

        if promo.single_track {
            let Some(quality) = format_quality(&promo.encoding) else {
                if let Some(track) = tralbum.tracks.first() {
                    tracing::warn!(
                        "неизвестный формат {} у трека {}",
                        promo.encoding,
                        track.title
                    );
                }
                return Ok(Vec::new());
            };
            let candidate_release = match tralbum.tracks.first() {
                Some(track) => release_from_track(&tralbum, page_url, track),
                None => release.clone(),
            };
            return Ok(vec![Candidate {
                source: SourceId::Bandcamp,
                release: candidate_release,
                quality,
                size_bytes: None,
                locator: Locator::Bandcamp {
                    url: promo.request_url,
                    encoding: promo.encoding,
                },
            }]);
        }

        let Some(quality) = format_quality(&promo.encoding) else {
            for (_, track) in match_members_to_tracks(&promo.entries, &tralbum.tracks) {
                tracing::warn!(
                    "неизвестный формат {} у трека {}",
                    promo.encoding,
                    track.title
                );
            }
            return Ok(Vec::new());
        };

        let mut candidates = Vec::new();
        for (entry, track) in match_members_to_tracks(&promo.entries, &tralbum.tracks) {
            candidates.push(Candidate {
                source: SourceId::Bandcamp,
                release: release_from_track(&tralbum, page_url, &track),
                quality: quality.clone(),
                size_bytes: None,
                locator: make_member_locator(&promo.request_url, &entry.member, &promo.encoding),
            });
        }
        Ok(candidates)
    }

    async fn download(
        &self,
        candidate: &Candidate,
        dest: &Path,
        sink: &dyn ProgressSink,
    ) -> anyhow::Result<()> {
        let Locator::Bandcamp { .. } = &candidate.locator else {
            anyhow::bail!("кандидат не принадлежит Bandcamp");
        };
        let Some(target) = parse_locator(&candidate.locator) else {
            anyhow::bail!("некорректный локатор Bandcamp");
        };

        let source_file = if let Some(name) = target.member.as_deref() {
            match self.member_file(&target, name).await? {
                Some(path) => path,
                None => anyhow::bail!("в архиве нет файла {name}"),
            }
        } else {
            let Some(id) = target.tralbum_id else {
                anyhow::bail!("некорректный локатор Bandcamp");
            };
            let zip_path = archive_zip_path(&self.cache_root, id);
            if zip_path.is_file() {
                zip_path
            } else {
                self.write_archive(&target.request_url, id).await?
            }
        };

        if let Some(parent) = dest.parent()
            && !parent.as_os_str().is_empty()
        {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("создание каталога {}", parent.display()))?;
        }
        tokio::fs::copy(&source_file, dest).await.with_context(|| {
            format!("копирование {} в {}", source_file.display(), dest.display())
        })?;
        let size = std::fs::metadata(dest).map(|meta| meta.len()).unwrap_or(0);
        sink.on_finished(size);
        Ok(())
    }
}

/// The default cache root: `$XDG_CACHE_HOME/maxfetch`, then `~/.cache/maxfetch`.
fn cache_root_default() -> PathBuf {
    if let Some(base) = std::env::var_os("XDG_CACHE_HOME") {
        return PathBuf::from(base).join("maxfetch");
    }
    if let Some(home) = dirs::home_dir() {
        return home.join(".cache").join("maxfetch");
    }
    std::env::temp_dir().join("maxfetch")
}
