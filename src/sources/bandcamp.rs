//! Bandcamp URL classification and tralbum id extraction.
//!
//! Everything here is pure computation: no HTTP, no I/O. Recorded pages are
//! replayed by the test harness instead of being fetched.

use std::sync::OnceLock;

use anyhow::anyhow;
use chrono::Datelike;
use regex::Regex;
use serde_json::{Map, Value};

use crate::model::{Release, SourceId};

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
