mod support;

use std::path::Path;

use maxfetch::fetch::NoopSink;
use maxfetch::model::{Release, SourceId};
use maxfetch::sources::AuthState;
use maxfetch::sources::Source;
use maxfetch::sources::bandcamp::{
    BandcampSource, PromoAvailability, make_member_locator, parse_locator, parse_tralbum_details,
    promo_availability,
};
use support::{MockServer, Reply, Route, fixture, write_zip};

const PAGE: &str = "/album/night-drive";

fn read(rel: &str) -> String {
    std::fs::read_to_string(fixture(rel)).unwrap()
}

/// The recorded album page, the recorded download page (403 "Download Expired"),
/// the recorded statdownload body and the retry archive, all rewritten onto the
/// mock server's own origin. The 403 route is what makes the test exercise the
/// real §8.2 chain instead of skipping straight to the retry body.
fn album_routes(
    album_page: &str,
    track_count: u32,
    zip: Option<Vec<u8>>,
) -> impl FnOnce(&str) -> Vec<Route> {
    let page = album_page.to_string();
    let tralbum = support::tralbum_json(track_count);
    let retry = read("bandcamp/statdownload_result.txt");
    move |base: &str| {
        vec![
            Route::new(
                PAGE,
                vec![Reply::ok(
                    "text/html",
                    page.replace("https://nightwave.bandcamp.com", base),
                )],
            ),
            Route::new(
                &format!("{PAGE}/download"),
                vec![Reply::status(403, "Download Expired")],
            ),
            Route::new(
                &format!("{PAGE}/statdownload"),
                vec![Reply::ok(
                    "text/plain",
                    retry.replace("https://nightwave.bandcamp.com", base),
                )],
            ),
            Route::new(
                "/download/album",
                vec![Reply::status(200, zip.clone().unwrap_or_default())],
            ),
            Route::new(
                "/api/mobile/25/tralbum_details",
                vec![Reply::ok("application/json", tralbum.clone())],
            ),
        ]
    }
}

fn promo_zip(track_count: u32) -> Vec<u8> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("promo.zip");
    let audio = std::fs::read(fixture("probe/sine_44100_s16.flac")).unwrap();
    let members: Vec<String> = (1..=track_count)
        .map(|i| format!("{i:02} Track {i}.flac"))
        .collect();
    let payload: Vec<(&str, &[u8])> = members
        .iter()
        .map(|m| (m.as_str(), audio.as_slice()))
        .collect();
    write_zip(&path, &payload).unwrap();
    std::fs::read(&path).unwrap()
}

fn album_page() -> String {
    read("bandcamp/album_page.html")
}

/// True when the source unpacked something under the archive cache root.
fn bandcamp_archive_written(cache_root: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(cache_root) else {
        return false;
    };
    for entry in entries.flatten() {
        let members = entry.path().join("members");
        if std::fs::read_dir(&members)
            .map(|d| d.count() > 0)
            .unwrap_or(false)
        {
            return true;
        }
    }
    false
}

#[test]
fn the_source_reports_bandcamp_without_auth() {
    let src = BandcampSource::with_api_base("http://127.0.0.1:1", &std::env::temp_dir());
    assert_eq!(src.id(), SourceId::Bandcamp);
    assert_eq!(src.auth_state(), AuthState::NotNeeded);
    let caps = src.capabilities();
    assert!(caps.audio);
    assert!(caps.lossless);
    assert!(!caps.needs_auth);
}

#[test]
fn a_paid_only_release_reports_no_free_promo() {
    let mut tralbum = parse_tralbum_details(&support::tralbum_json(3)).unwrap();
    tralbum.free_download = false;
    // WD20: `tralbum_json` carries neither `is_purchasable` nor
    // `has_digital_download`, so after `free_download = false` the parsed
    // tralbum is `Unavailable`; `PaidOnly` needs a store flag. Without this the
    // copy below would be identical to `tralbum` and the two asserts could not
    // both hold.
    tralbum.is_purchasable = true;
    assert_eq!(promo_availability(&tralbum), PromoAvailability::PaidOnly);
    assert_eq!(
        promo_availability(&tralbum).message(),
        "нет бесплатного промо"
    );
    assert_eq!(PromoAvailability::Free.message(), "бесплатное промо");
    let mut none = tralbum.clone();
    none.is_purchasable = false;
    none.has_digital_download = false;
    assert_eq!(promo_availability(&none), PromoAvailability::Unavailable);
    assert_eq!(promo_availability(&none).message(), "нет бесплатного промо");
    assert!(promo_availability(&none).is_free() == false);
}

#[test]
fn the_locator_carries_the_archive_member_in_the_fragment() {
    let loc = make_member_locator(
        "https://nightwave.bandcamp.com/download/album?enc=flac&fsig=abc",
        "01 Track 1.flac",
        "flac",
    );
    let target = parse_locator(&loc).unwrap();
    assert_eq!(
        target.request_url,
        "https://nightwave.bandcamp.com/download/album?enc=flac&fsig=abc"
    );
    assert_eq!(target.member.as_deref(), Some("01 Track 1.flac"));
    assert_eq!(target.encoding, "flac");
    assert_eq!(target.tralbum_id, None);
}

#[test]
fn the_tralbum_id_is_recovered_from_the_retry_url_query() {
    // The real `retry_url` shape: `.../download/album?enc=flac&fsig=...&id=2814`.
    let retry = "https://nightwave.bandcamp.com/download/album?enc=flac&fsig=abc123&id=2814";
    let target = parse_locator(&make_member_locator(retry, "01 Track 1.flac", "flac")).unwrap();
    assert_eq!(target.tralbum_id, Some(2814));
    let explicit = "https://nightwave.bandcamp.com/download/album?tralbum_id=2814&enc=flac";
    assert_eq!(
        parse_locator(&make_member_locator(explicit, "01 Track 1.flac", "flac"))
            .unwrap()
            .tralbum_id,
        Some(2814)
    );
    let album = "https://nightwave.bandcamp.com/download/album?album_id=2814&enc=flac";
    assert_eq!(
        parse_locator(&make_member_locator(album, "01 Track 1.flac", "flac"))
            .unwrap()
            .tralbum_id,
        Some(2814)
    );
}

#[test]
fn a_foreign_locator_is_rejected() {
    let loc = maxfetch::model::Locator::SoundCloud {
        track_id: "1".into(),
        format: maxfetch::sources::soundcloud::SoundCloudFormat::Aac160,
    };
    assert!(parse_locator(&loc).is_none());
}

#[tokio::test]
async fn a_twelve_track_free_album_produces_one_candidate_per_track() {
    let zip = promo_zip(12);
    let server = MockServer::start_with(album_routes(&album_page(), 12, Some(zip))).await;
    let cache = tempfile::tempdir().unwrap();
    let src = BandcampSource::with_api_base(&server.base_url(), cache.path());
    let tralbum = parse_tralbum_details(&support::tralbum_json(12)).unwrap();
    let release = tralbum.to_releases(&format!("{}{}", server.base_url(), PAGE))[0].clone();

    let candidates = src.candidates(&release).await.unwrap();
    assert_eq!(candidates.len(), 12);
    assert!(candidates.iter().all(|c| c.quality.lossless));
    assert_eq!(
        parse_locator(&candidates[0].locator)
            .unwrap()
            .member
            .as_deref(),
        Some("01 Track 1.flac")
    );
    assert_eq!(candidates[11].release.track_no, Some(12));
}

#[tokio::test]
async fn downloading_three_of_twelve_writes_exactly_those_three_and_fetches_the_zip_once() {
    const WANTED: [u32; 3] = [1, 5, 12];
    let zip = promo_zip(12);
    let server = MockServer::start_with(album_routes(&album_page(), 12, Some(zip))).await;
    let cache = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let src = BandcampSource::with_api_base(&server.base_url(), cache.path());
    let tralbum = parse_tralbum_details(&support::tralbum_json(12)).unwrap();
    let releases = tralbum.to_releases(&format!("{}{}", server.base_url(), PAGE));
    let candidates = src.candidates(&releases[2]).await.unwrap();
    assert_eq!(candidates.len(), 12);

    // Download tracks 1, 5 and 12 by their track number, not by position, so the
    // test pins which members were written instead of taking whichever come first.
    let mut written: Vec<String> = Vec::new();
    for &n in &WANTED {
        let candidate = candidates
            .iter()
            .find(|c| c.release.track_no == Some(n))
            .unwrap();
        let member = parse_locator(&candidate.locator).unwrap().member.unwrap();
        let dest = out.path().join(&member);
        src.download(candidate, &dest, &NoopSink).await.unwrap();
        assert!(dest.exists());
        written.push(member);
    }
    written.sort();
    assert_eq!(
        written,
        vec!["01 Track 1.flac", "05 Track 5.flac", "12 Track 12.flac"]
    );

    // The other nine members must not have produced a file.
    let files: Vec<String> = std::fs::read_dir(out.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(files.len(), 3);
    for n in (1..=12u32).filter(|n| !WANTED.contains(n)) {
        let prefix = format!("{n:02} ");
        assert!(
            !files.iter().any(|f| f.starts_with(&prefix)),
            "{prefix} was downloaded"
        );
    }

    // One album, one archive: the promo chain and the ZIP are fetched once even
    // though three candidates were written. `request_count` counts requests per
    // path, so three downloads of `/download/album` would read as three.
    assert_eq!(server.request_count(&format!("{PAGE}/download")), 1);
    assert_eq!(server.request_count(&format!("{PAGE}/statdownload")), 1);
    assert_eq!(server.request_count("/download/album"), 1);
}

#[tokio::test]
async fn the_promo_chain_and_the_archive_are_fetched_exactly_once() {
    // The same single-fetch property as the test above, asserted on the source
    // side: one `candidates` call plus three `download` calls of that one album
    // must hit the promo chain once and the archive once — the chain is asked
    // for a fresh `retry_url` only when there is no cached archive yet.
    let zip = promo_zip(12);
    let server = MockServer::start_with(album_routes(&album_page(), 12, Some(zip))).await;
    let cache = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let src = BandcampSource::with_api_base(&server.base_url(), cache.path());
    let tralbum = parse_tralbum_details(&support::tralbum_json(12)).unwrap();
    let releases = tralbum.to_releases(&format!("{}{}", server.base_url(), PAGE));

    let candidates = src.candidates(&releases[0]).await.unwrap();
    assert_eq!(candidates.len(), 12);

    for &n in &[1u32, 5, 12] {
        let candidate = candidates
            .iter()
            .find(|c| c.release.track_no == Some(n))
            .unwrap();
        let member = parse_locator(&candidate.locator).unwrap().member.unwrap();
        let dest = out.path().join(&member);
        src.download(candidate, &dest, &NoopSink).await.unwrap();
        assert!(dest.exists());
    }

    // Each endpoint of the chain was requested exactly once, not once per track.
    assert_eq!(server.request_count(&format!("{PAGE}/download")), 1);
    assert_eq!(server.request_count(&format!("{PAGE}/statdownload")), 1);
    assert_eq!(server.request_count("/download/album"), 1);
    // And the archive was unpacked once, not three times.
    let unpacked: Vec<_> =
        std::fs::read_dir(cache.path().join("bandcamp").join("2814").join("members"))
            .unwrap()
            .collect();
    assert_eq!(unpacked.len(), 12);
}

#[tokio::test]
async fn a_paid_only_release_yields_no_candidates_and_no_error() {
    // WD21: `candidates` re-reads the tralbum through `tralbum_details`, so a
    // flag mutated only on the local copy is invisible to it. The route has to
    // serve the paid-only payload the test means, otherwise the source sees
    // `free_download: true` and runs the chain.
    let page = album_page();
    let paid_only = support::tralbum_json(3).replace(
        "\"free_download\":true",
        "\"free_download\":false,\"is_purchasable\":true",
    );
    let server = MockServer::start_with(move |base: &str| {
        vec![
            Route::new(
                PAGE,
                vec![Reply::ok(
                    "text/html",
                    page.replace("https://nightwave.bandcamp.com", base),
                )],
            ),
            Route::new(
                &format!("{PAGE}/download"),
                vec![Reply::status(403, "Download Expired")],
            ),
            Route::new(
                &format!("{PAGE}/statdownload"),
                vec![Reply::ok(
                    "text/plain",
                    read("bandcamp/statdownload_result.txt"),
                )],
            ),
            Route::new("/download/album", vec![Reply::status(200, Vec::new())]),
            Route::new(
                "/api/mobile/25/tralbum_details",
                vec![Reply::ok("application/json", paid_only.clone())],
            ),
        ]
    })
    .await;
    let cache = tempfile::tempdir().unwrap();
    let src = BandcampSource::with_api_base(&server.base_url(), cache.path());
    let mut tralbum = parse_tralbum_details(&support::tralbum_json(3)).unwrap();
    tralbum.free_download = false;
    tralbum.is_purchasable = true;
    let release = tralbum.to_releases(&format!("{}{}", server.base_url(), PAGE))[0].clone();

    let candidates = src.candidates(&release).await.unwrap();
    assert!(candidates.is_empty());
    assert_eq!(server.request_count(&format!("{PAGE}/statdownload")), 0);
    assert_eq!(server.request_count("/download/album"), 0);
}

#[tokio::test]
async fn a_single_track_promo_downloads_the_file_directly() {
    let audio = std::fs::read(fixture("probe/sine_44100_s16.flac")).unwrap();
    let page = album_page();
    let tralbum = support::tralbum_json(1);
    let retry = read("bandcamp/statdownload_result.txt")
        .replace("/download/album", "/download/track")
        .replace("enc=flac", "enc=flac");
    let server = MockServer::start_with(move |base: &str| {
        vec![
            Route::new(
                PAGE,
                vec![Reply::ok(
                    "text/html",
                    page.replace("https://nightwave.bandcamp.com", base),
                )],
            ),
            Route::new(
                &format!("{PAGE}/statdownload"),
                vec![Reply::ok(
                    "text/plain",
                    retry.replace("https://nightwave.bandcamp.com", base),
                )],
            ),
            Route::new("/download/track", vec![Reply::status(200, audio.clone())]),
            Route::new(
                "/api/mobile/25/tralbum_details",
                vec![Reply::ok("application/json", tralbum.clone())],
            ),
        ]
    })
    .await;
    let cache = tempfile::tempdir().unwrap();
    let src = BandcampSource::with_api_base(&server.base_url(), cache.path());
    let parsed = parse_tralbum_details(&support::tralbum_json(1)).unwrap();
    let release = parsed.to_releases(&format!("{}{}", server.base_url(), PAGE))[0].clone();

    let candidates = src.candidates(&release).await.unwrap();
    assert_eq!(candidates.len(), 1);
    assert!(
        parse_locator(&candidates[0].locator)
            .unwrap()
            .member
            .is_none()
    );

    let dest = cache.path().join("single.flac");
    src.download(&candidates[0], &dest, &NoopSink)
        .await
        .unwrap();
    assert_eq!(
        std::fs::metadata(&dest).unwrap().len(),
        std::fs::metadata(fixture("probe/sine_44100_s16.flac"))
            .unwrap()
            .len()
    );
}

#[tokio::test]
async fn a_release_without_a_page_blob_reports_no_promo() {
    let server = MockServer::start_with(|_base: &str| {
        vec![Route::new(
            PAGE,
            vec![Reply::ok("text/html", "<html></html>")],
        )]
    })
    .await;
    let cache = tempfile::tempdir().unwrap();
    let src = BandcampSource::with_api_base(&server.base_url(), cache.path());
    let release = Release {
        title: "Neon".into(),
        artist: "Nightwave".into(),
        album: Some("Night Drive".into()),
        track_no: Some(1),
        duration_sec: Some(214.5),
        isrc: None,
        year: Some(2021),
        source: SourceId::Bandcamp,
        source_item_id: format!("{}{}|9001", server.base_url(), PAGE),
    };

    // WD13: the plan's test body stops after building `release`; the assertions
    // below are the ones the test name promises.
    let candidates = src.candidates(&release).await.unwrap();
    assert!(candidates.is_empty());
    assert_eq!(server.request_count("/api/mobile/25/tralbum_details"), 0);
}

#[tokio::test]
async fn a_statdl_result_without_a_retry_url_reports_no_promo() {
    let page = album_page();
    let statdl = r#"var _statDL_result = {"result":"err","errortype":"ExpiredFreeDownloadError"};"#;
    let server = MockServer::start_with(move |base: &str| {
        vec![
            Route::new(
                PAGE,
                vec![Reply::ok(
                    "text/html",
                    page.replace("https://nightwave.bandcamp.com", base),
                )],
            ),
            Route::new(
                &format!("{PAGE}/download"),
                vec![Reply::status(403, "Download Expired")],
            ),
            Route::new(
                &format!("{PAGE}/statdownload"),
                vec![Reply::ok("text/plain", statdl)],
            ),
            Route::new(
                "/api/mobile/25/tralbum_details",
                vec![Reply::ok("application/json", support::tralbum_json(3))],
            ),
        ]
    })
    .await;
    let cache = tempfile::tempdir().unwrap();
    let src = BandcampSource::with_api_base(&server.base_url(), cache.path());
    let tralbum = parse_tralbum_details(&support::tralbum_json(3)).unwrap();
    let release = tralbum.to_releases(&format!("{}{}", server.base_url(), PAGE))[0].clone();

    // No retry_url is «нет бесплатного промо», not an error and not a disabled source.
    let candidates = src.candidates(&release).await.unwrap();
    assert!(candidates.is_empty());
    assert_eq!(src.auth_state(), AuthState::NotNeeded);
    assert_eq!(server.request_count("/download/album"), 0);
}

#[tokio::test]
async fn an_html_body_instead_of_a_zip_yields_no_candidates() {
    let page = album_page();
    let retry = read("bandcamp/statdownload_result.txt");
    let server = MockServer::start_with(move |base: &str| {
        vec![
            Route::new(
                PAGE,
                vec![Reply::ok(
                    "text/html",
                    page.replace("https://nightwave.bandcamp.com", base),
                )],
            ),
            Route::new(
                &format!("{PAGE}/download"),
                vec![Reply::status(403, "Download Expired")],
            ),
            Route::new(
                &format!("{PAGE}/statdownload"),
                vec![Reply::ok(
                    "text/plain",
                    retry.replace("https://nightwave.bandcamp.com", base),
                )],
            ),
            // The retry URL answers with the download page instead of the archive.
            Route::new(
                "/download/album",
                vec![Reply::ok("text/html", "<html><body>Download</body></html>")],
            ),
            Route::new(
                "/api/mobile/25/tralbum_details",
                vec![Reply::ok("application/json", support::tralbum_json(3))],
            ),
        ]
    })
    .await;
    let cache = tempfile::tempdir().unwrap();
    let src = BandcampSource::with_api_base(&server.base_url(), cache.path());
    let tralbum = parse_tralbum_details(&support::tralbum_json(3)).unwrap();
    let release = tralbum.to_releases(&format!("{}{}", server.base_url(), PAGE))[0].clone();

    let candidates = src.candidates(&release).await.unwrap();
    assert!(candidates.is_empty());
    // WD13: the plan asserts 0 here, but the retry_url can only come from
    // `/statdownload`, so the chain ran exactly once.
    assert_eq!(server.request_count(&format!("{PAGE}/statdownload")), 1);
    assert!(!bandcamp_archive_written(cache.path()));
}

#[tokio::test]
async fn an_unknown_encoding_skips_only_that_member() {
    // A promo whose archive is an unknown encoding: every member is skipped with a
    // reason, but the source itself still answers Ok with an empty list.
    let page = album_page();
    let retry = read("bandcamp/statdownload_result.txt").replace("enc=flac", "enc=bz2");
    let zip = promo_zip(3);
    let server = MockServer::start_with(move |base: &str| {
        vec![
            Route::new(
                PAGE,
                vec![Reply::ok(
                    "text/html",
                    page.replace("https://nightwave.bandcamp.com", base),
                )],
            ),
            Route::new(
                &format!("{PAGE}/download"),
                vec![Reply::status(403, "Download Expired")],
            ),
            Route::new(
                &format!("{PAGE}/statdownload"),
                vec![Reply::ok(
                    "text/plain",
                    retry.replace("https://nightwave.bandcamp.com", base),
                )],
            ),
            Route::new("/download/album", vec![Reply::status(200, zip.clone())]),
            Route::new(
                "/api/mobile/25/tralbum_details",
                vec![Reply::ok("application/json", support::tralbum_json(3))],
            ),
        ]
    })
    .await;
    let cache = tempfile::tempdir().unwrap();
    let src = BandcampSource::with_api_base(&server.base_url(), cache.path());
    let tralbum = parse_tralbum_details(&support::tralbum_json(3)).unwrap();
    let release = tralbum.to_releases(&format!("{}{}", server.base_url(), PAGE))[0].clone();

    let candidates = src.candidates(&release).await.unwrap();
    assert!(candidates.is_empty());
}

#[tokio::test]
async fn an_artist_url_resolves_to_one_release_per_discography_item() {
    let page = album_page();
    let band_details = read("bandcamp/band_details.json");
    let tralbum = support::tralbum_json(3);
    let server = MockServer::start_with(move |base: &str| {
        vec![
            Route::new(
                "/",
                vec![Reply::ok(
                    "text/html",
                    page.replace("https://nightwave.bandcamp.com", base),
                )],
            ),
            Route::new(
                "/api/mobile/24/band_details",
                vec![Reply::ok("application/json", band_details.clone())],
            ),
            Route::new(
                "/api/mobile/25/tralbum_details",
                vec![Reply::ok("application/json", tralbum.clone())],
            ),
        ]
    })
    .await;
    let cache = tempfile::tempdir().unwrap();
    let src = BandcampSource::with_api_base(&server.base_url(), cache.path());

    let releases = src
        .resolve(&maxfetch::sources::Query {
            text: None,
            url: Some(server.base_url()),
            items: vec![],
        })
        .await
        .unwrap();
    // Two discography items, each resolved through tralbum_details into its tracks.
    assert_eq!(releases.len(), 6);
    assert_eq!(server.request_count("/api/mobile/24/band_details"), 1);
    assert_eq!(server.request_count("/api/mobile/25/tralbum_details"), 2);
}
