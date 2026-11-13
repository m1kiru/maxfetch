use maxfetch::model::{Container, Quality, Release, SourceId};
use maxfetch::sources::soulseek::{
    ATTR_BIT_DEPTH, ATTR_BITRATE, ATTR_DURATION, ATTR_SAMPLE_RATE, PeerCandidate, attrib,
    describe_peer, into_candidate, lossless_extension, peer_from_file, quality_from_attributes,
    rank_peers,
};
use std::collections::HashMap;

fn peer(user: &str, slots: u8, speed: u32, quality: Quality, size: u64) -> PeerCandidate {
    PeerCandidate {
        username: user.into(),
        filename: "f.flac".into(),
        size,
        slots,
        speed,
        quality,
    }
}

fn flac(depth: u8, rate: u32) -> Quality {
    Quality {
        lossless: true,
        container: Container::Flac,
        bit_depth: Some(depth),
        sample_rate: Some(rate),
        bitrate_kbps: None,
    }
}

#[test]
fn zero_slot_peer_is_dropped() {
    let busy = peer("busy", 0, 4, flac(24, 96000), 900);
    let free = peer("free", 1, 1, flac(16, 44100), 100);
    let ranked = rank_peers(&[busy, free]);
    assert_eq!(ranked.len(), 1);
    assert_eq!(ranked[0].username, "free");
}

#[test]
fn flac_with_zero_bitrate_ranks_by_depth_then_rate() {
    let deep = quality_from_attributes(
        "a.flac",
        &HashMap::from([
            (ATTR_BITRATE, 0),
            (ATTR_SAMPLE_RATE, 96000),
            (ATTR_BIT_DEPTH, 24),
        ]),
    )
    .unwrap();
    let shallow = quality_from_attributes(
        "b.flac",
        &HashMap::from([
            (ATTR_BITRATE, 0),
            (ATTR_SAMPLE_RATE, 192000),
            (ATTR_BIT_DEPTH, 16),
        ]),
    )
    .unwrap();
    assert_eq!(deep.bit_depth, Some(24));
    assert_eq!(deep.sample_rate, Some(96000));
    assert_eq!(deep.bitrate_kbps, None);
    let ranked = rank_peers(&[
        peer("shallow", 2, 4, shallow, 200),
        peer("deep", 1, 1, deep, 100),
    ]);
    assert_eq!(ranked[0].username, "deep");
}

#[test]
fn flac_attributes_of_a_twenty_four_forty_eight_come_out_intact() {
    let q = quality_from_attributes(
        "song.FLAC",
        &HashMap::from([(4u32, 48000u32), (5u32, 24u32)]),
    )
    .unwrap();
    assert!(q.lossless);
    assert_eq!(q.bit_depth, Some(24));
    assert_eq!(q.sample_rate, Some(48000));
    assert_eq!(q.bitrate_kbps, None);
    assert_eq!(
        attrib(&HashMap::from([(5u32, 24u32)]), ATTR_BIT_DEPTH),
        Some(24)
    );
    assert_eq!(
        attrib(&HashMap::from([(5u32, 24u32)]), ATTR_SAMPLE_RATE),
        None
    );
}

#[test]
fn lossless_container_never_reports_a_bitrate() {
    let q = quality_from_attributes(
        "a.flac",
        &HashMap::from([
            (ATTR_BITRATE, 1411),
            (ATTR_SAMPLE_RATE, 44100),
            (ATTR_BIT_DEPTH, 16),
        ]),
    )
    .unwrap();
    assert_eq!(q.bitrate_kbps, None);
}

#[test]
fn lossy_attributes_yield_an_mp3_with_bitrate() {
    let q = quality_from_attributes(
        "a.MP3",
        &HashMap::from([(ATTR_BITRATE, 320), (ATTR_SAMPLE_RATE, 44100)]),
    )
    .unwrap();
    assert!(!q.lossless);
    assert_eq!(q.container, Container::Mp3);
    assert_eq!(q.bitrate_kbps, Some(320));
}

#[test]
fn quality_outranks_free_slots() {
    let ranked = rank_peers(&[
        peer("many-slots", 9, 4, flac(16, 44100), 300),
        peer("one-slot", 1, 1, flac(24, 96000), 200),
    ]);
    assert_eq!(ranked[0].username, "one-slot");
}

#[test]
fn free_slots_outrank_peer_speed() {
    let ranked = rank_peers(&[
        peer("slow", 2, 1, flac(16, 44100), 100),
        peer("fast", 1, 4, flac(16, 44100), 100),
    ]);
    assert_eq!(ranked[0].username, "slow");
}

#[test]
fn peer_speed_breaks_ties() {
    let ranked = rank_peers(&[
        peer("slow", 1, 1, flac(16, 44100), 100),
        peer("fast", 1, 4, flac(16, 44100), 100),
    ]);
    assert_eq!(ranked[0].username, "fast");
}

#[test]
fn file_size_breaks_remaining_ties() {
    let ranked = rank_peers(&[
        peer("small", 1, 4, flac(16, 44100), 100),
        peer("large", 1, 4, flac(16, 44100), 900),
    ]);
    assert_eq!(ranked[0].username, "large");
}

#[test]
fn lossy_peer_never_beats_lossless() {
    let mp3 = Quality {
        lossless: false,
        container: Container::Mp3,
        bit_depth: None,
        sample_rate: Some(44100),
        bitrate_kbps: Some(320),
    };
    let ranked = rank_peers(&[
        peer("mp3", 9, 4, mp3, 300),
        peer("flac", 1, 1, flac(16, 44100), 100),
    ]);
    assert_eq!(ranked[0].username, "flac");
}

#[test]
fn ape_and_wv_are_lossless_but_have_no_container() {
    assert!(lossless_extension("ape"));
    assert!(lossless_extension("wv"));
    assert!(lossless_extension("FLAC"));
    assert!(!lossless_extension("opus"));
    assert!(
        quality_from_attributes("x.ape", &HashMap::from([(ATTR_SAMPLE_RATE, 44100)])).is_none()
    );
}

#[test]
fn a_file_the_library_cannot_lay_out_becomes_no_peer() {
    let attribs = HashMap::from([(ATTR_SAMPLE_RATE, 44100)]);
    assert!(peer_from_file("alice", 1, 4, "x.ape", 10, &attribs).is_none());
    let flac_attribs = HashMap::from([(ATTR_SAMPLE_RATE, 96000), (ATTR_BIT_DEPTH, 24)]);
    let p = peer_from_file("alice", 0, 4, "a.flac", 10, &flac_attribs).unwrap();
    assert_eq!(p.size, 10);
    assert_eq!(p.slots, 0);
    assert_eq!(p.quality.bit_depth, Some(24));
}

#[test]
fn a_flac_without_bitrate_rate_and_depth_ranks_below_a_known_flac() {
    let unknown =
        quality_from_attributes("unknown.flac", &HashMap::from([(ATTR_DURATION, 240)])).unwrap();
    assert_eq!(unknown.bit_depth, None);
    assert_eq!(unknown.sample_rate, None);
    assert_eq!(unknown.bitrate_kbps, None);
    let ranked = rank_peers(&[
        peer("unknown", 9, 4, unknown, 900),
        peer("known", 1, 1, flac(16, 44100), 100),
    ]);
    assert_eq!(ranked[0].username, "known");
}

#[test]
fn describe_peer_names_the_slots_and_the_speed() {
    assert_eq!(
        describe_peer(&peer("alice", 2, 4, flac(16, 44100), 1)),
        "пир alice: скорость 4, слотов 2"
    );
}

#[test]
fn into_candidate_carries_the_soulseek_locator() {
    let p = peer("alice", 2, 4, flac(16, 44100), 1234);
    let release = Release {
        title: "Get Lucky".into(),
        artist: "Daft Punk".into(),
        album: Some("RAM".into()),
        track_no: Some(8),
        duration_sec: Some(369.0),
        isrc: None,
        year: None,
        source: SourceId::Soulseek,
        source_item_id: "peer:alice:f.flac".into(),
    };
    let c = into_candidate(&p, &release);
    assert_eq!(c.size_bytes, Some(1234));
    assert!(matches!(
        c.locator,
        maxfetch::model::Locator::Soulseek { .. }
    ));
}
