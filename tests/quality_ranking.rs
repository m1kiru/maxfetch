use maxfetch::model::{Candidate, Container, Locator, Quality, Release, SourceId};
use maxfetch::quality::{compare_quality, rank_candidates};

fn release() -> Release {
    Release {
        title: "Get Lucky".into(),
        artist: "Daft Punk".into(),
        album: Some("Random Access Memories".into()),
        track_no: Some(8),
        duration_sec: Some(369.0),
        isrc: None,
        year: Some(2013),
        source: SourceId::Soulseek,
        source_item_id: "x".into(),
    }
}

fn cand(container: Container, lossless: bool, depth: Option<u8>, rate: Option<u32>, kbps: Option<u32>) -> Candidate {
    Candidate {
        source: SourceId::Soulseek,
        release: release(),
        quality: Quality { lossless, container, bit_depth: depth, sample_rate: rate, bitrate_kbps: kbps },
        size_bytes: None,
        locator: Locator::Soulseek { username: "peer".into(), filename: "f".into(), size: 0 },
    }
}

#[test]
fn lossless_beats_lossy() {
    let flac = cand(Container::Flac, true, Some(16), Some(44100), None);
    let mp3 = cand(Container::Mp3, false, None, Some(44100), Some(320));
    assert_eq!(compare_quality(&flac, &mp3), std::cmp::Ordering::Greater);
}

#[test]
fn bit_depth_outranks_sample_rate() {
    let deep = cand(Container::Flac, true, Some(24), Some(48000), None);
    let shallow = cand(Container::Flac, true, Some(16), Some(96000), None);
    assert_eq!(compare_quality(&deep, &shallow), std::cmp::Ordering::Greater);
}

#[test]
fn higher_sample_rate_breaks_equal_depth() {
    let a = cand(Container::Flac, true, Some(16), Some(96000), None);
    let b = cand(Container::Flac, true, Some(16), Some(44100), None);
    assert_eq!(compare_quality(&a, &b), std::cmp::Ordering::Greater);
}

#[test]
fn unknown_depth_sorts_below_known() {
    let known = cand(Container::Flac, true, Some(16), Some(44100), None);
    let unknown = cand(Container::Flac, true, None, Some(44100), None);
    assert_eq!(compare_quality(&known, &unknown), std::cmp::Ordering::Greater);
}

#[test]
fn lossy_ranks_by_bitrate_then_rate() {
    let hi = cand(Container::Mp3, false, None, Some(44100), Some(320));
    let lo = cand(Container::Mp3, false, None, Some(44100), Some(128));
    assert_eq!(compare_quality(&hi, &lo), std::cmp::Ordering::Greater);
    let hi48 = cand(Container::Opus, false, None, Some(48000), Some(128));
    let lo44 = cand(Container::Opus, false, None, Some(44100), Some(128));
    assert_eq!(compare_quality(&hi48, &lo44), std::cmp::Ordering::Greater);
}

#[test]
fn rank_drops_lossy_when_lossless_exists() {
    let flac = cand(Container::Flac, true, Some(16), Some(44100), None);
    let mp3 = cand(Container::Mp3, false, None, Some(44100), Some(320));
    let ranked = rank_candidates(&[mp3, flac]);
    assert_eq!(ranked.len(), 1);
    assert!(ranked[0].quality.lossless);
}

#[test]
fn rank_keeps_lossy_when_it_is_all_we_have() {
    let mp3 = cand(Container::Mp3, false, None, Some(44100), Some(320));
    let opus = cand(Container::Opus, false, None, Some(48000), Some(128));
    let ranked = rank_candidates(&[opus, mp3]);
    assert_eq!(ranked.len(), 2);
    assert_eq!(ranked[0].quality.bitrate_kbps, Some(320));
}
