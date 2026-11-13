//! Soulseek peer model, attribute parsing and quality ranking.
//!
//! The functions here are pure: no network, no `soulseek_rs::Client`. The
//! blocking client lives in the next task; this module only decides which
//! peer to try and what the file claims to be.

use std::cmp::Ordering;
use std::collections::HashMap;

use crate::model::{Candidate, Container, Locator, Quality, Release, SourceId};

/// Attribute code: bitrate in kbit/s, `0` for FLAC.
pub const ATTR_BITRATE: u32 = 0;
/// Attribute code: track length in seconds. Declared for completeness, unused.
pub const ATTR_DURATION: u32 = 1;
/// Attribute code: sample rate in Hz.
pub const ATTR_SAMPLE_RATE: u32 = 4;
/// Attribute code: bit depth.
pub const ATTR_BIT_DEPTH: u32 = 5;

/// One peer/file pair as returned by a search.
#[derive(Clone, Debug, PartialEq)]
pub struct PeerCandidate {
    pub username: String,
    pub filename: String,
    pub size: u64,
    pub slots: u8,
    pub speed: u32,
    pub quality: Quality,
}

/// Raw attribute lookup; `None` when the code is absent from the map.
pub fn attrib(attribs: &HashMap<u32, u32>, code: u32) -> Option<u32> {
    attribs.get(&code).copied()
}

/// `ext` is a lowercase extension without the dot.
pub fn lossless_extension(ext: &str) -> bool {
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "flac" | "wav" | "alac" | "aiff" | "ape" | "wv"
    )
}

/// Quality claimed by `name` plus `attribs`; `None` when the extension has no
/// `Container` mapping, i.e. the file cannot be laid out in the library.
pub fn quality_from_attributes(name: &str, attribs: &HashMap<u32, u32>) -> Option<Quality> {
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    let container = Container::from_ext(&ext)?;
    let lossless = container.is_lossless();
    let bitrate_kbps = if lossless {
        None
    } else {
        attrib(attribs, ATTR_BITRATE).filter(|kbps| *kbps > 0)
    };
    Some(Quality {
        lossless,
        container,
        bit_depth: attrib(attribs, ATTR_BIT_DEPTH).map(|depth| depth as u8),
        sample_rate: attrib(attribs, ATTR_SAMPLE_RATE),
        bitrate_kbps,
    })
}

/// One peer/file pair; `None` when [`quality_from_attributes`] yields `None`.
pub fn peer_from_file(
    username: &str,
    slots: u8,
    speed: u32,
    filename: &str,
    size: u64,
    attribs: &HashMap<u32, u32>,
) -> Option<PeerCandidate> {
    let quality = quality_from_attributes(filename, attribs)?;
    Some(PeerCandidate {
        username: username.into(),
        filename: filename.into(),
        size,
        slots,
        speed,
        quality,
    })
}

/// Drops `slots == 0`, then sorts best first: quality, slots, speed, size.
pub fn rank_peers(candidates: &[PeerCandidate]) -> Vec<PeerCandidate> {
    let mut kept: Vec<PeerCandidate> = candidates
        .iter()
        .filter(|candidate| candidate.slots > 0)
        .cloned()
        .collect();
    kept.sort_by(|a, b| {
        cmp_quality(&b.quality, &a.quality)
            .then_with(|| b.slots.cmp(&a.slots))
            .then_with(|| b.speed.cmp(&a.speed))
            .then_with(|| b.size.cmp(&a.size))
    });
    kept
}

/// Builds the pipeline candidate with [`Locator::Soulseek`].
pub fn into_candidate(peer: &PeerCandidate, release: &Release) -> Candidate {
    Candidate {
        source: SourceId::Soulseek,
        release: release.clone(),
        quality: peer.quality.clone(),
        size_bytes: Some(peer.size),
        locator: Locator::Soulseek {
            username: peer.username.clone(),
            filename: peer.filename.clone(),
            size: peer.size,
        },
    }
}

/// `"пир <username>: скорость <speed>, слотов <slots>"` (user-facing, RU).
pub fn describe_peer(peer: &PeerCandidate) -> String {
    format!(
        "пир {}: скорость {}, слотов {}",
        peer.username, peer.speed, peer.slots
    )
}

/// Mirrors `quality::compare_quality` for a bare [`Quality`]; `Greater` = better.
fn cmp_quality(a: &Quality, b: &Quality) -> Ordering {
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
