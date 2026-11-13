mod support;

use maxfetch::model::SourceId;
use maxfetch::sources::bandcamp::{
    TralbumKind, TralbumType, parse_band_details, parse_fuzzy_search, parse_tralbum_details,
    split_source_item_id,
};
use support::fixture;

fn read(rel: &str) -> String {
    std::fs::read_to_string(fixture(rel)).unwrap()
}

#[test]
fn search_hits_map_the_three_result_types() {
    let hits = parse_fuzzy_search(&read("bandcamp/fuzzysearch.json")).unwrap();
    assert_eq!(hits.len(), 3);
    assert_eq!(hits[0].kind, TralbumKind::Artist);
    assert_eq!(hits[0].id, 4171);
    assert_eq!(hits[1].kind, TralbumKind::Album);
    assert_eq!(hits[1].id, 2814);
    assert_eq!(hits[2].kind, TralbumKind::Track);
    assert_eq!(hits[2].id, 909);
}

#[test]
fn an_album_hit_carries_the_band_id_tralbum_details_needs() {
    let hits = parse_fuzzy_search(&read("bandcamp/fuzzysearch.json")).unwrap();
    assert_eq!(hits[1].band_id, Some(4171));
    assert_eq!(
        hits[1].location.as_deref(),
        Some("https://nightwave.bandcamp.com/album/night-drive?band_id=4171")
    );
    assert_eq!(hits[2].band_id, Some(4171));
    assert_eq!(hits[0].band_id, Some(4171));
}

#[test]
fn an_album_hit_without_a_direct_band_id_reads_it_from_location() {
    let hits = parse_fuzzy_search(
        r#"{"results":[{"type":"a","item_id":2814,"item_type":"a","location":"https://nightwave.bandcamp.com/album/night-drive?band_id=4171","name":"Night Drive"}]}"#,
    )
    .unwrap();
    assert_eq!(hits[0].band_id, Some(4171));
    assert_eq!(hits[0].id, 2814);
}

#[test]
fn unknown_result_types_are_skipped() {
    let hits = parse_fuzzy_search(&read("bandcamp/fuzzysearch.json")).unwrap();
    assert!(hits.iter().all(|h| h.id != 5));
}

#[test]
fn album_details_map_to_one_release_per_track() {
    let tralbum = parse_tralbum_details(&read("bandcamp/tralbum_details_album.json")).unwrap();
    assert_eq!(tralbum.title, "Night Drive");
    assert_eq!(tralbum.artist, "Nightwave");
    let releases = tralbum.to_releases("https://nightwave.bandcamp.com/album/night-drive");
    assert_eq!(releases.len(), 3);
    assert_eq!(releases[0].track_no, Some(1));
    assert_eq!(releases[0].title, "Neon");
    assert_eq!(releases[0].album.as_deref(), Some("Night Drive"));
    assert_eq!(releases[0].duration_sec, Some(214.5));
    assert_eq!(releases[0].year, Some(2021));
    assert_eq!(releases[0].source, SourceId::Bandcamp);
}

#[test]
fn album_details_carry_the_free_download_flag() {
    let tralbum = parse_tralbum_details(&read("bandcamp/tralbum_details_album.json")).unwrap();
    assert!(tralbum.free_download);
    assert!(tralbum.has_digital_download);
    assert_eq!(tralbum.currency.as_deref(), Some("USD"));
}

#[test]
fn bandcamp_releases_never_carry_an_isrc() {
    let tralbum = parse_tralbum_details(&support::tralbum_json(12)).unwrap();
    assert_eq!(tralbum.tracks.len(), 12);
    let releases = tralbum.to_releases("https://nightwave.bandcamp.com/album/night-drive");
    assert_eq!(releases.len(), 12);
    assert!(releases.iter().all(|r| r.isrc.is_none()));
}

#[test]
fn the_source_item_id_round_trips_to_a_page_url_and_track_id() {
    let tralbum = parse_tralbum_details(&read("bandcamp/tralbum_details_album.json")).unwrap();
    let releases = tralbum.to_releases("https://nightwave.bandcamp.com/album/night-drive");
    let (url, track_id) = split_source_item_id(&releases[0].source_item_id).unwrap();
    assert_eq!(url, "https://nightwave.bandcamp.com/album/night-drive");
    assert_eq!(track_id, 9001);
}

#[test]
fn discography_items_keep_their_release_date() {
    let items = parse_band_details(&read("bandcamp/band_details.json")).unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].item_type, TralbumType::Album);
    assert_eq!(items[0].title, "Night Drive");
    assert_eq!(items[0].release_date_unix, 1609459200);
}

#[test]
fn unstreamable_tracks_are_not_offered() {
    let mut tralbum = parse_tralbum_details(&read("bandcamp/tralbum_details_album.json")).unwrap();
    tralbum.tracks[1].is_streamable = false;
    let releases = tralbum.to_releases("https://nightwave.bandcamp.com/album/night-drive");
    assert_eq!(releases.len(), 2);
    assert_eq!(releases[1].track_no, Some(3));
}
