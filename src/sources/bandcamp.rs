//! Bandcamp URL classification, tralbum metadata and promo archives.
//!
//! There is no HTTP here: recorded pages are replayed by the test harness
//! instead of being fetched, and the download chain is a pure URL/metadata
//! computation. The filesystem is touched only by the archive layer
//! ([`is_zip`], [`unpack_archive`], [`unpack_cached`]) and its private listing.

use std::fs::File;
use std::io::{Read, copy};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::anyhow;
use chrono::Datelike;
use regex::Regex;
use serde_json::{Map, Value};

use crate::model::{Container, Quality, Release, SourceId};

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
