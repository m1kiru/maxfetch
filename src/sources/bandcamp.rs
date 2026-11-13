//! Bandcamp URL classification and tralbum id extraction.
//!
//! Everything here is pure computation: no HTTP, no I/O. Recorded pages are
//! replayed by the test harness instead of being fetched.

use std::sync::OnceLock;

use regex::Regex;

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
