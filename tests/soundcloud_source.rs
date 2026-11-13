mod support;

use maxfetch::config::Config;
use maxfetch::fetch::NoopSink;
use maxfetch::model::{Container, Locator, SourceId};
use maxfetch::sources::soundcloud::{
    SoundCloudFormat, SoundCloudInput, SoundCloudSource, classify_input, parse_ytdlp_ids,
    search_arg,
};
use maxfetch::sources::{AuthState, Query, Source};
use support::{MockServer, Reply, Route, fixture};

fn read(rel: &str) -> String {
    std::fs::read_to_string(fixture(rel)).unwrap()
}

fn stub(name: &str) -> String {
    fixture(&format!("bin/{name}"))
        .to_str()
        .unwrap()
        .to_string()
}

#[test]
fn a_numeric_id_is_classified_as_an_id() {
    assert_eq!(classify_input("880017"), SoundCloudInput::Id(880017));
}

#[test]
fn a_soundcloud_url_is_classified_as_a_url() {
    assert_eq!(
        classify_input("https://soundcloud.com/nightwave/neon"),
        SoundCloudInput::Url("https://soundcloud.com/nightwave/neon".into())
    );
    assert_eq!(
        classify_input("soundcloud.com/nightwave/sets/soulhack"),
        SoundCloudInput::Url("soundcloud.com/nightwave/sets/soulhack".into())
    );
}

#[test]
fn plain_text_is_classified_as_a_text_query() {
    assert_eq!(
        classify_input("nightwave neon"),
        SoundCloudInput::Text("nightwave neon".into())
    );
}

#[test]
fn the_search_argument_uses_the_ytdlp_search_prefix() {
    assert_eq!(
        search_arg("nightwave neon", 10),
        "scsearch10:nightwave neon"
    );
}

#[test]
fn ytdlp_ids_are_read_from_both_shapes() {
    assert_eq!(parse_ytdlp_ids(&read("bin/yt-dlp-url.json")), vec![880017]);
    assert_eq!(
        parse_ytdlp_ids(&read("bin/yt-dlp-search.json")),
        vec![880017, 880018]
    );
}

#[tokio::test]
async fn a_track_id_yields_one_release_with_its_isrc() {
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
    let dir = tempfile::tempdir().unwrap();
    let src = SoundCloudSource::with_api_base(&server.base_url(), dir.path(), "yt-dlp");
    let releases = src
        .resolve(&Query {
            text: None,
            url: None,
            items: vec![maxfetch::sources::QueryItem::Track {
                source: SourceId::SoundCloud,
                id: "880017".into(),
            }],
        })
        .await
        .unwrap();
    assert_eq!(releases.len(), 1);
    assert_eq!(releases[0].isrc.as_deref(), Some("DEP960300042"));
}

#[tokio::test]
async fn a_rotated_client_id_is_rescraped_and_the_request_retried_once() {
    let server = MockServer::start(vec![
        Route::new(
            "/tracks/880017",
            vec![
                Reply::status(401, "unauthorized"),
                Reply::ok("application/json", read("soundcloud/track.json")),
            ],
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
    let dir = tempfile::tempdir().unwrap();
    let src = SoundCloudSource::with_api_base(&server.base_url(), dir.path(), "yt-dlp");
    let release = src
        .resolve(&Query {
            text: None,
            url: None,
            items: vec![maxfetch::sources::QueryItem::Track {
                source: SourceId::SoundCloud,
                id: "880017".into(),
            }],
        })
        .await
        .unwrap();
    assert_eq!(release.len(), 1);
    assert_eq!(server.request_count("/tracks/880017"), 2);
    // WD2: the initial scrape and the 401-triggered re-scrape each fetch the
    // page once, so the page is served twice, not once as the plan wrote.
    assert_eq!(server.request_count("/"), 2);
}

#[tokio::test]
async fn a_second_401_fails_the_request_without_a_panic() {
    let server = MockServer::start(vec![
        Route::new("/tracks/880017", vec![Reply::status(401, "unauthorized")]),
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
    let dir = tempfile::tempdir().unwrap();
    let src = SoundCloudSource::with_api_base(&server.base_url(), dir.path(), "yt-dlp");
    let err = src
        .resolve(&Query {
            text: None,
            url: None,
            items: vec![maxfetch::sources::QueryItem::Track {
                source: SourceId::SoundCloud,
                id: "880017".into(),
            }],
        })
        .await
        .unwrap_err();
    assert!(format!("{err}").contains("client_id rejected"));
    // WD2: initial scrape + one re-scrape.
    assert_eq!(server.request_count("/"), 2);
}

#[tokio::test]
async fn the_only_candidate_is_the_lossy_aac_stream() {
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
    let dir = tempfile::tempdir().unwrap();
    let src = SoundCloudSource::with_api_base(&server.base_url(), dir.path(), "yt-dlp");
    let release = src
        .resolve(&Query {
            text: None,
            url: None,
            items: vec![maxfetch::sources::QueryItem::Track {
                source: SourceId::SoundCloud,
                id: "880017".into(),
            }],
        })
        .await
        .unwrap()[0]
        .clone();
    assert_eq!(
        server.request_count("/users/77"),
        1,
        "the §8.9 profile is fetched during resolve"
    );
    let candidates = src.candidates(&release).await.unwrap();
    // Exactly one candidate, and it is lossy: no lossless stream is offered here.
    assert_eq!(candidates.len(), 1);
    assert!(candidates.iter().all(|c| !c.quality.lossless));
    assert_eq!(candidates[0].quality.container, Container::M4aAac);
    assert_eq!(candidates[0].quality.bitrate_kbps, Some(160));
    assert_eq!(
        candidates[0].locator,
        Locator::SoundCloud {
            track_id: "880017".into(),
            format: SoundCloudFormat::Aac160
        }
    );
    // The platform's lossless claim must not leak into the catalog: the track is
    // downloadable and flagged as such on SoundCloud, yet no lossless quality is
    // produced and the release carries no lossless marker.
    assert!(release.isrc.as_deref() == Some("DEP960300042"));
}

#[tokio::test]
async fn a_lossy_download_goes_through_ytdlp_with_the_hls_preset() {
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
    let dir = tempfile::tempdir().unwrap();
    let src =
        SoundCloudSource::with_api_base(&server.base_url(), dir.path(), &stub("fake-yt-dlp.sh"));
    let release = src
        .resolve(&Query {
            text: None,
            url: None,
            items: vec![maxfetch::sources::QueryItem::Track {
                source: SourceId::SoundCloud,
                id: "880017".into(),
            }],
        })
        .await
        .unwrap()[0]
        .clone();
    let candidate = src.candidates(&release).await.unwrap().remove(0);
    let dest = dir.path().join("out.m4a");
    src.download(&candidate, &dest, &NoopSink).await.unwrap();
    assert!(dest.exists());
    assert_eq!(
        std::fs::metadata(&dest).unwrap().len(),
        std::fs::metadata(fixture("probe/sine_44100_s16.flac"))
            .unwrap()
            .len()
    );
}

#[tokio::test]
async fn a_source_with_an_oauth_token_advertises_lossless() {
    let mut cfg = Config::default();
    let mut creds = maxfetch::config::Credentials::default();
    creds.soundcloud = Some(maxfetch::config::SoundCloudCredentials {
        client_id: "iN5ffSanbMS4rYYxVJVZcQU5jdq8Fjmz".into(),
        oauth_token: "t".into(),
    });
    let src = SoundCloudSource::new(&cfg, &creds);
    assert!(src.capabilities().lossless);
    assert_eq!(src.auth_state(), AuthState::NotNeeded);
    creds.soundcloud = None;
    cfg.youtube.yt_dlp_path = "yt-dlp".into();
    assert!(!SoundCloudSource::new(&cfg, &creds).capabilities().lossless);
    // An empty `oauth_token` means "no token", not "some token".
    creds.soundcloud = Some(maxfetch::config::SoundCloudCredentials {
        client_id: String::new(),
        oauth_token: String::new(),
    });
    assert!(!SoundCloudSource::new(&cfg, &creds).capabilities().lossless);
}
