mod support;

use maxfetch::sources::bandcamp::{
    is_zip, match_members_to_tracks, track_number_from_stem, unpack_archive, unpack_cached,
};
use support::{fixture, write_zip};

fn track(id: u64, num: u32, title: &str) -> maxfetch::sources::bandcamp::BandcampTrack {
    maxfetch::sources::bandcamp::BandcampTrack {
        track_id: id,
        title: title.into(),
        track_num: num,
        duration_sec: 200.0,
        band_name: "Nightwave".into(),
        album_title: "Night Drive".into(),
        is_streamable: true,
        track_url: format!(
            "https://nightwave.bandcamp.com/track/{}",
            title.to_lowercase()
        ),
    }
}

fn flac() -> Vec<u8> {
    std::fs::read(fixture("probe/sine_44100_s16.flac")).unwrap()
}

fn build_zip(dir: &std::path::Path, members: &[&str]) -> std::path::PathBuf {
    let path = dir.join("promo.zip");
    let audio = flac();
    let payload: Vec<(&str, &[u8])> = members.iter().map(|m| (*m, audio.as_slice())).collect();
    write_zip(&path, &payload).unwrap();
    path
}

#[test]
fn every_file_in_the_zip_becomes_an_entry() {
    let dir = tempfile::tempdir().unwrap();
    let zip = build_zip(dir.path(), &["01 Neon.flac", "02 Midnight.flac"]);
    assert!(is_zip(&zip).unwrap());
    let entries = unpack_archive(&zip, &dir.path().join("out")).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].member, "01 Neon.flac");
    assert!(entries[0].path.exists());
}

#[test]
fn macosx_sidecars_are_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let zip = build_zip(
        dir.path(),
        &["01 Neon.flac", "__MACOSX/01 Neon.flac", ".DS_Store"],
    );
    let entries = unpack_archive(&zip, &dir.path().join("out")).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].member, "01 Neon.flac");
}

#[test]
fn a_non_zip_file_is_not_an_archive() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("single.flac");
    std::fs::copy(fixture("probe/sine_44100_s16.flac"), &path).unwrap();
    assert!(!is_zip(&path).unwrap());
}

#[test]
fn members_are_matched_to_tracks_by_position_when_stems_differ() {
    let dir = tempfile::tempdir().unwrap();
    let zip = build_zip(dir.path(), &["a.flac", "b.flac"]);
    let entries = unpack_archive(&zip, &dir.path().join("out")).unwrap();
    let tracks = vec![track(9001, 1, "Neon"), track(9002, 2, "Midnight")];
    let pairs = match_members_to_tracks(&entries, &tracks);
    assert_eq!(pairs.len(), 2);
    assert_eq!(pairs[0].1.track_id, 9001);
    assert_eq!(pairs[1].1.track_id, 9002);
}

#[test]
fn a_title_stem_match_wins_over_position() {
    let dir = tempfile::tempdir().unwrap();
    let zip = build_zip(dir.path(), &["dawn.flac", "neon.flac"]);
    let entries = unpack_archive(&zip, &dir.path().join("out")).unwrap();
    let tracks = vec![track(9001, 1, "Neon"), track(9003, 3, "Dawn")];
    let pairs = match_members_to_tracks(&entries, &tracks);
    assert_eq!(pairs[0].0.member, "dawn.flac");
    assert_eq!(pairs[0].1.track_id, 9003);
    assert_eq!(pairs[1].0.member, "neon.flac");
    assert_eq!(pairs[1].1.track_id, 9001);
}

#[test]
fn unmatched_members_and_tracks_are_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let zip = build_zip(dir.path(), &["neon.flac"]);
    let entries = unpack_archive(&zip, &dir.path().join("out")).unwrap();
    let tracks = vec![track(9001, 1, "Neon"), track(9002, 2, "Midnight")];
    let pairs = match_members_to_tracks(&entries, &tracks);
    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0].1.track_id, 9001);
}

#[test]
fn unpacking_is_cached_and_reuses_the_members_directory() {
    let dir = tempfile::tempdir().unwrap();
    let zip = build_zip(dir.path(), &["01 Neon.flac", "02 Midnight.flac"]);
    let first = unpack_cached(&zip, dir.path(), 2814).unwrap();
    std::fs::remove_file(&zip).unwrap();
    let second = unpack_cached(&zip, dir.path(), 2814).unwrap();
    assert_eq!(first.len(), second.len());
    assert_eq!(second[0].member, "01 Neon.flac");
    assert!(second[0].path.exists());
}

#[test]
fn the_track_number_is_read_from_the_member_stem() {
    assert_eq!(track_number_from_stem("01 Neon"), Some(1));
    assert_eq!(track_number_from_stem("12 Dawn"), Some(12));
    assert_eq!(track_number_from_stem("Neon"), None);
}
