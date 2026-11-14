//! Qobuz offline helpers: app credential scraping and the format ladder.
//!
//! This module is network-free. It maps Qobuz `format_id` values onto the
//! crate [`Quality`] model (spec §8.5: 5 = MP3 320, 6 = FLAC-CD,
//! 7 = FLAC 24/96, 27 = FLAC 24/192) and extracts the `app_id`/`app_secret`
//! pair embedded in the minified Qobuz web bundle. No `qobuz-api`
//! dependency in this task.

use std::cmp::Ordering;
use std::sync::LazyLock;

use regex::Regex;

use crate::model::{Container, Quality};

/// The rungs of the Qobuz ladder.
///
/// Rung order by [`QobuzFormat::format_id`] matches quality order:
/// 5 < 6 < 7 < 27.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QobuzFormat {
    /// MP3 320 kbps (`format_id` 5).
    Mp3_320,
    /// FLAC 16-bit / 44.1 kHz (`format_id` 6).
    FlacCd,
    /// FLAC 24-bit / 96 kHz (`format_id` 7).
    FlacHiRes96,
    /// FLAC 24-bit / 192 kHz (`format_id` 27).
    FlacHiRes192,
}

impl QobuzFormat {
    /// Maps a Qobuz `format_id` onto a ladder rung.
    pub fn from_format_id(id: u32) -> Option<QobuzFormat> {
        match id {
            5 => Some(QobuzFormat::Mp3_320),
            6 => Some(QobuzFormat::FlacCd),
            7 => Some(QobuzFormat::FlacHiRes96),
            27 => Some(QobuzFormat::FlacHiRes192),
            _ => None,
        }
    }

    /// The Qobuz `format_id` of this rung.
    pub fn format_id(&self) -> u32 {
        match self {
            QobuzFormat::Mp3_320 => 5,
            QobuzFormat::FlacCd => 6,
            QobuzFormat::FlacHiRes96 => 7,
            QobuzFormat::FlacHiRes192 => 27,
        }
    }
}

/// The [`Quality`] a track gets on this ladder rung.
///
/// The spec pins no sample rate for MP3-320 and no bitrate for the FLAC
/// rungs.
pub fn quality_of(format: QobuzFormat) -> Quality {
    match format {
        QobuzFormat::Mp3_320 => Quality {
            lossless: false,
            container: Container::Mp3,
            bit_depth: None,
            sample_rate: None,
            bitrate_kbps: Some(320),
        },
        QobuzFormat::FlacCd => lossless_flac(16, 44_100),
        QobuzFormat::FlacHiRes96 => lossless_flac(24, 96_000),
        QobuzFormat::FlacHiRes192 => lossless_flac(24, 192_000),
    }
}

/// Scrapes the embedded `appId`/`appSecret` pair out of the minified Qobuz
/// web bundle. Returns the first `(app_id, app_secret)` match, or `None`.
pub fn scrape_app_credentials(bundle_js: &str) -> Option<(String, String)> {
    static APP_CREDENTIALS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"production:\{api:\{appId:"(\d{9})",appSecret:"(\w{32})""#)
            .expect("valid Qobuz app credential pattern")
    });
    let caps = APP_CREDENTIALS.captures(bundle_js)?;
    Some((caps[1].to_string(), caps[2].to_string()))
}

/// The best [`Quality`] a rung can deliver at most.
///
/// Numerically the same as [`quality_of`]; the semantics differ: what the
/// rung can output at its best, not what a track on it promises.
pub fn format_ceiling(format: QobuzFormat) -> Quality {
    quality_of(format)
}

/// The best [`Quality`] the account tier permits: hi-res with Studio,
/// FLAC-CD without it.
pub fn account_ceiling(studio: bool) -> Quality {
    if studio {
        lossless_flac(24, 192_000)
    } else {
        lossless_flac(16, 44_100)
    }
}

/// The best quality a setup can deliver, with an optional honest reason.
#[derive(Clone, Debug, PartialEq)]
pub struct QualityCeiling {
    /// The best [`Quality`] available after combining the limits.
    pub max_lossless: Quality,
    /// Set exactly when something was capped, explaining what was lost.
    pub reason: Option<String>,
}

/// Combines the track's best rung with the account tier into an honest
/// ceiling.
///
/// `reason` is set exactly when the account ceiling ranks strictly below the
/// track ceiling, i.e. when something actually got capped.
pub fn ceiling_from_limits(track_max: QobuzFormat, studio: bool) -> QualityCeiling {
    let track_ceiling = format_ceiling(track_max);
    let account = account_ceiling(studio);
    if compare_quality(&account, &track_ceiling) == Ordering::Less {
        QualityCeiling {
            max_lossless: account,
            reason: Some("нет подписки Studio, максимум FLAC-CD 16/44.1".to_string()),
        }
    } else {
        QualityCeiling {
            max_lossless: track_ceiling,
            reason: None,
        }
    }
}

/// Picks the best rung the ceiling allows: `wanted` when its ceiling fits,
/// otherwise the greatest rung at or below the ceiling.
///
/// `Mp3_320` always fits: a lossy rung never exceeds a lossless ceiling.
pub fn best_format_within(ceiling: &QualityCeiling, wanted: QobuzFormat) -> QobuzFormat {
    let limit = &ceiling.max_lossless;
    let fits =
        |rung: QobuzFormat| compare_quality(&format_ceiling(rung), limit) != Ordering::Greater;
    if fits(wanted) {
        return wanted;
    }
    [
        QobuzFormat::FlacHiRes192,
        QobuzFormat::FlacHiRes96,
        QobuzFormat::FlacCd,
        QobuzFormat::Mp3_320,
    ]
    .into_iter()
    .find(|rung| fits(*rung))
    .unwrap_or(QobuzFormat::Mp3_320)
}

/// §7.4 ordering over [`Quality`]: lossless above lossy; within lossless,
/// bit depth descending then sample rate descending; within lossy, bitrate
/// descending then sample rate. Missing values rank below known ones.
///
/// Private because [`Quality`] carries no `Ord` (the model must stay
/// serde-facing), unlike [`crate::quality::compare_quality`], which orders
/// whole candidates.
fn compare_quality(a: &Quality, b: &Quality) -> Ordering {
    match (a.lossless, b.lossless) {
        (true, false) => return Ordering::Greater,
        (false, true) => return Ordering::Less,
        _ => {}
    }
    if a.lossless {
        cmp_opt_desc(a.bit_depth, b.bit_depth)
            .then_with(|| cmp_opt_desc(a.sample_rate, b.sample_rate))
    } else {
        cmp_opt_desc(a.bitrate_kbps, b.bitrate_kbps)
            .then_with(|| cmp_opt_desc(a.sample_rate, b.sample_rate))
    }
}

fn cmp_opt_desc<T: Ord>(a: Option<T>, b: Option<T>) -> Ordering {
    match (a, b) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => Ordering::Greater,
        (None, Some(_)) => Ordering::Less,
        (None, None) => Ordering::Equal,
    }
}

fn lossless_flac(bit_depth: u8, sample_rate: u32) -> Quality {
    Quality {
        lossless: true,
        container: Container::Flac,
        bit_depth: Some(bit_depth),
        sample_rate: Some(sample_rate),
        bitrate_kbps: None,
    }
}
