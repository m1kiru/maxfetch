mod support;

use maxfetch::sources::bandcamp::{TralbumKind, classify_url, parse_bandcamp_ids};
use support::{MockServer, Reply, Route, fixture, write_zip};

#[test]
fn album_url_is_classified_as_album() {
    assert_eq!(
        classify_url("https://nightwave.bandcamp.com/album/night-drive"),
        Some(TralbumKind::Album)
    );
    assert_eq!(
        classify_url("https://nightwave.bandcamp.com/album/night-drive?from=embed"),
        Some(TralbumKind::Album)
    );
}

#[test]
fn track_url_is_classified_as_track() {
    assert_eq!(
        classify_url("https://nightwave.bandcamp.com/track/neon"),
        Some(TralbumKind::Track)
    );
}

#[test]
fn artist_root_is_classified_as_artist() {
    assert_eq!(
        classify_url("https://nightwave.bandcamp.com"),
        Some(TralbumKind::Artist)
    );
    assert_eq!(
        classify_url("https://nightwave.bandcamp.com/"),
        Some(TralbumKind::Artist)
    );
}

#[test]
fn foreign_hosts_are_not_bandcamp() {
    assert_eq!(classify_url("https://example.com/album/x"), None);
    assert_eq!(classify_url("night drive"), None);
}

#[test]
fn ids_are_read_from_the_recorded_album_page() {
    let html = std::fs::read_to_string(fixture("bandcamp/album_page.html")).unwrap();
    let ids = parse_bandcamp_ids(&html).unwrap();
    assert_eq!(ids.band_id, 4171);
    assert_eq!(ids.tralbum_id, 2814);
}

#[test]
fn a_page_without_a_band_id_yields_none() {
    assert!(parse_bandcamp_ids("<html><body>nope</body></html>").is_none());
}

#[tokio::test]
async fn the_mock_server_serves_a_recorded_body_and_counts_requests() {
    let server = MockServer::start(vec![Route::new(
        "/album/night-drive",
        vec![Reply::ok("text/html", "recorded")],
    )])
    .await;
    let body = reqwest::get(format!("{}/album/night-drive?x=1", server.base_url()))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(body, "recorded");
    assert_eq!(server.request_count("/album/night-drive"), 1);
    assert_eq!(server.request_count("/album/other"), 0);
}

// extra: a bare bandcamp.com host is a foreign host, never an artist page.
#[test]
fn a_bare_bandcamp_host_is_never_an_artist() {
    assert_eq!(classify_url("https://bandcamp.com"), None);
    assert_eq!(
        classify_url("https://bandcamp.com/album/x"),
        Some(TralbumKind::Album)
    );
}

// extra: the host match is case-insensitive.
#[test]
fn the_host_match_is_case_insensitive() {
    assert_eq!(
        classify_url("https://NightWave.BandCamp.COM/album/night-drive"),
        Some(TralbumKind::Album)
    );
}

// extra: a band_id without the item_id yields None.
#[test]
fn a_band_id_without_an_item_id_yields_none() {
    assert!(
        parse_bandcamp_ids("<a href=\"https://bandcamp.com/album/x?from=embed&amp;band_id=4171\">")
            .is_none()
    );
}

// extra: the mock server answers an unknown path with a bare 404.
#[tokio::test]
async fn the_mock_server_answers_an_unknown_path_with_404() {
    let server = MockServer::start(vec![Route::new(
        "/album/night-drive",
        vec![Reply::ok("text/html", "recorded")],
    )])
    .await;
    let resp = reqwest::get(format!("{}/album/other", server.base_url()))
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    assert_eq!(resp.text().await.unwrap(), "");
    assert_eq!(server.request_count("/album/other"), 1);
}

// extra: write_zip round-trips its members.
#[test]
fn write_zip_round_trips_its_members() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("album.zip");
    let members: [(&str, &[u8]); 2] = [("01 Neon.flac", b"flac"), ("art.jpg", b"jpg")];
    write_zip(&path, &members).unwrap();

    let mut archive = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
    assert_eq!(archive.len(), 2);

    {
        let mut entry = archive.by_index(0).unwrap();
        assert_eq!(entry.name().unwrap(), "01 Neon.flac");
        assert_eq!(entry.size(), 4);
        assert!(!entry.is_dir());
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut bytes).unwrap();
        assert_eq!(bytes, b"flac");
    }

    {
        let mut entry = archive.by_index(1).unwrap();
        assert_eq!(entry.name().unwrap(), "art.jpg");
        assert_eq!(entry.size(), 3);
        assert!(!entry.is_dir());
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut bytes).unwrap();
        assert_eq!(bytes, b"jpg");
    }
}

// extra: the mock server replays replies in order, then holds the last one.
#[tokio::test]
async fn the_mock_server_replays_replies_in_order_then_holds_the_last() {
    let server = MockServer::start(vec![Route::new(
        "/album/night-drive",
        vec![
            Reply::ok("text/plain", "first"),
            Reply::ok("text/plain", "second"),
        ],
    )])
    .await;
    for expected in ["first", "second", "second"] {
        let body = reqwest::get(format!("{}/album/night-drive", server.base_url()))
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert_eq!(body, expected);
    }
    assert_eq!(server.request_count("/album/night-drive"), 3);
}
