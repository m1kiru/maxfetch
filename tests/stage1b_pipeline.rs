mod support;

use maxfetch::config::Config;
use maxfetch::db::Db;
use maxfetch::fetch::NoopSink;
use maxfetch::model::SourceId;
use maxfetch::pipeline::Pipeline;
use maxfetch::sources::bandcamp::BandcampSource;
use maxfetch::sources::soundcloud::SoundCloudSource;
use maxfetch::sources::{Query, QueryItem};

use support::{MockServer, Reply, Route, fixture, write_zip};

fn read(rel: &str) -> String {
    std::fs::read_to_string(fixture(rel)).unwrap()
}

#[tokio::test]
async fn a_bandcamp_flac_wins_over_a_soundcloud_aac_track() {
    let audio = std::fs::read(fixture("probe/sine_44100_s16.flac")).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let zip_path = dir.path().join("promo.zip");
    write_zip(&zip_path, &[("01 Neon.flac", audio.as_slice())]).unwrap();
    let zip_bytes = std::fs::read(&zip_path).unwrap();
    let page = read("bandcamp/album_page.html");
    let retry = read("bandcamp/statdownload_result.txt");

    let server = MockServer::start_with(move |base: &str| {
        vec![
            Route::new(
                "/album/night-drive",
                vec![Reply::ok(
                    "text/html",
                    page.replace("https://nightwave.bandcamp.com", base),
                )],
            ),
            Route::new(
                "/album/night-drive/statdownload",
                vec![Reply::ok(
                    "text/plain",
                    retry.replace("https://nightwave.bandcamp.com", base),
                )],
            ),
            Route::new(
                "/download/album",
                vec![Reply::status(200, zip_bytes.clone())],
            ),
            Route::new(
                "/api/mobile/25/tralbum_details",
                vec![Reply::ok("application/json", support::tralbum_json(1))],
            ),
            Route::new(
                "/tracks/880017",
                vec![Reply::ok("application/json", read("soundcloud/track.json"))],
            ),
            Route::new(
                "/",
                vec![Reply::ok("text/html", read("soundcloud/home.html"))],
            ),
            Route::new(
                "/assets/50-late.js",
                vec![Reply::ok(
                    "application/javascript",
                    read("soundcloud/bundle_late.js"),
                )],
            ),
        ]
    })
    .await;

    let lib = tempfile::tempdir().unwrap();
    let mut cfg = Config::default();
    cfg.library.path = lib.path().to_path_buf();
    let cache = tempfile::tempdir().unwrap();
    let bandcamp = BandcampSource::with_api_base(&server.base_url(), cache.path());
    let soundcloud = SoundCloudSource::with_api_base(&server.base_url(), lib.path(), "yt-dlp");

    let db = Db::open_in_memory().unwrap();
    let p = Pipeline::new(cfg, db, vec![Box::new(bandcamp), Box::new(soundcloud)]);
    let groups = p
        .search(&Query {
            text: None,
            url: Some(format!("{}/album/night-drive", server.base_url())),
            items: vec![QueryItem::Track {
                source: SourceId::SoundCloud,
                id: "880017".into(),
            }],
        })
        .await
        .unwrap();
    assert!(!groups.is_empty());

    let bandcamp_group = groups
        .iter()
        .find(|g| g.primary.source == SourceId::Bandcamp)
        .unwrap();
    let candidates = p.candidates(bandcamp_group).await.unwrap();
    assert!(candidates[0].quality.lossless);
    let result = p.download_one(&candidates[0], &NoopSink).await.unwrap();
    assert!(!result.rejected);
    let tracks = p.db().all_tracks().unwrap();
    assert_eq!(tracks.len(), 1);
    assert!(tracks[0].lossless);
    assert_eq!(tracks[0].source, SourceId::Bandcamp);
    assert!(tracks[0].path.ends_with(".flac"));
}

#[tokio::test]
async fn a_soundcloud_only_run_records_an_honest_lossy_track() {
    let server = MockServer::start(vec![
        Route::new(
            "/tracks/880017",
            vec![Reply::ok("application/json", read("soundcloud/track.json"))],
        ),
        Route::new(
            "/users/77",
            vec![Reply::ok("application/json", read("soundcloud/user.json"))],
        ),
        Route::new(
            "/",
            vec![Reply::ok("text/html", read("soundcloud/home.html"))],
        ),
        Route::new(
            "/assets/50-late.js",
            vec![Reply::ok(
                "application/javascript",
                read("soundcloud/bundle_late.js"),
            )],
        ),
    ])
    .await;
    let lib = tempfile::tempdir().unwrap();
    let mut cfg = Config::default();
    // WD2: the default config does not enable SoundCloud, so the only
    // registered source would be filtered out of the run.
    cfg.sources.enabled.push("soundcloud".into());
    cfg.library.path = lib.path().to_path_buf();
    cfg.youtube.yt_dlp_path = fixture("bin/fake-yt-dlp-lossy.sh")
        .to_str()
        .unwrap()
        .to_string();
    let soundcloud =
        SoundCloudSource::with_api_base(&server.base_url(), lib.path(), &cfg.youtube.yt_dlp_path);
    let db = Db::open_in_memory().unwrap();
    let p = Pipeline::new(cfg, db, vec![Box::new(soundcloud)]);
    let groups = p
        .search(&Query {
            text: None,
            url: None,
            items: vec![QueryItem::Track {
                source: SourceId::SoundCloud,
                id: "880017".into(),
            }],
        })
        .await
        .unwrap();
    let candidates = p.candidates(&groups[0]).await.unwrap();
    assert!(!candidates[0].quality.lossless);
    let result = p.download_one(&candidates[0], &NoopSink).await.unwrap();
    assert!(!result.rejected);
    let tracks = p.db().all_tracks().unwrap();
    assert_eq!(tracks[0].isrc.as_deref(), Some("DEP960300042"));
    assert!(!tracks[0].lossless, "the measured bytes are lossy");
    let comment = maxfetch::tags::read_tags(std::path::Path::new(&tracks[0].path))
        .unwrap()
        .quality_comment;
    assert!(comment.contains("source=lossy"));
    assert!(!comment.contains("source=lossless"));
}
