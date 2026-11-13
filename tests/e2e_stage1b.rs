#![cfg(feature = "e2e")]

mod support;

use clap::Parser;

use maxfetch::cli::{Cli, run_with};
use maxfetch::config::{Config, Credentials, state_path};
use maxfetch::db::Db;
use maxfetch::dedup::plan_deletion;
use maxfetch::fetch::NoopSink;
use maxfetch::model::SourceId;
use maxfetch::pipeline::Pipeline;
use maxfetch::sources::Query;
use maxfetch::sources::bandcamp::BandcampSource;

use support::{MockServer, Reply, Route, fixture, write_zip};

fn read(rel: &str) -> String {
    std::fs::read_to_string(fixture(rel)).unwrap()
}

#[tokio::test]
async fn bandcamp_promo_roundtrip_records_a_lossless_track() {
    let dir = tempfile::tempdir().unwrap();
    // The CLI opens the database at `state_path()`, so both the pipeline and the
    // library commands must land in the same temporary directory.
    unsafe {
        std::env::set_var("XDG_DATA_HOME", dir.path().join("data"));
        std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
    }
    let lib = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();

    // The promo archive holds one member whose name matches the single track of
    // the recorded tralbum, so the member is matched to the track by title.
    let audio = std::fs::read(fixture("probe/sine_44100_s16.flac")).unwrap();
    let zip_path = dir.path().join("promo.zip");
    write_zip(&zip_path, &[("01 Track 1.flac", audio.as_slice())]).unwrap();
    let zip_bytes = std::fs::read(&zip_path).unwrap();

    let page = read("bandcamp/album_page.html");
    let retry = read("bandcamp/statdownload_result.txt");
    let fuzzy = read("bandcamp/fuzzysearch_album.json");
    let tralbum = support::tralbum_json(1);

    let server = MockServer::start_with(move |base: &str| {
        vec![
            Route::new(
                "/api/fuzzysearch/2/app_autocomplete",
                vec![Reply::ok(
                    "application/json",
                    fuzzy.replace("https://nightwave.bandcamp.com", base),
                )],
            ),
            Route::new(
                "/album/night-drive",
                vec![Reply::ok(
                    "text/html",
                    page.replace("https://nightwave.bandcamp.com", base),
                )],
            ),
            Route::new(
                "/album/night-drive/download",
                vec![Reply::status(403, "Download Expired")],
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
                vec![Reply::ok("application/json", tralbum.clone())],
            ),
        ]
    })
    .await;

    let mut cfg = Config::default();
    cfg.library.path = lib.path().to_path_buf();

    let db = Db::open(&state_path()).unwrap();
    db.migrate().unwrap();
    let bandcamp = BandcampSource::with_api_base(&server.base_url(), cache.path());
    let p = Pipeline::new(cfg.clone(), db, vec![Box::new(bandcamp)]);

    // 1. Search Bandcamp: the fuzzy search resolves the album into its tracks.
    let groups = p
        .search(&Query {
            text: Some("night drive".into()),
            url: None,
            items: vec![],
        })
        .await
        .unwrap();
    assert!(!groups.is_empty());
    let group = groups
        .iter()
        .find(|g| g.primary.source == SourceId::Bandcamp)
        .unwrap();

    // 2. The free promo yields exactly one lossless candidate.
    let candidates = p.candidates(group).await.unwrap();
    assert_eq!(candidates.len(), 1);
    assert!(candidates[0].quality.lossless);

    // 3. The download writes the member into the library and registers it.
    let result = p.download(&candidates, &NoopSink).await.unwrap();
    assert!(!result.rejected);
    assert!(result.path.exists());
    // `Path::ends_with` compares whole components, so the extension check goes
    // through the string form of the path.
    assert!(result.path.to_string_lossy().ends_with(".flac"));

    // 4. `library scan` indexes the library; the file the pipeline already
    //    registered must not become a second row.
    let mut out = Vec::new();
    let cli = Cli::parse_from(["maxfetch", "library", "scan"]);
    run_with(cli, cfg.clone(), Credentials::default(), &mut out)
        .await
        .unwrap();

    let db = Db::open(&state_path()).unwrap();
    let tracks = db.all_tracks().unwrap();
    assert_eq!(
        tracks.len(),
        1,
        "the scan must not duplicate the recorded track"
    );
    assert!(tracks[0].lossless);
    assert_eq!(tracks[0].source, SourceId::Bandcamp);
    assert!(tracks[0].path.ends_with(".flac"));
    assert!(std::path::Path::new(&tracks[0].path).exists());
    assert!(tracks[0].duplicate_of.is_none());
    assert!(db.duplicate_groups().unwrap().is_empty());

    // 5. `library dedupe --dry-run` finds nothing to delete and deletes nothing.
    //    `--dry-run` is the global flag of the core CLI; `library dedupe` without
    //    `--apply --delete` is already a dry run, and the flag says so explicitly.
    let mut out = Vec::new();
    let cli = Cli::parse_from(["maxfetch", "library", "dedupe", "--dry-run"]);
    run_with(cli, cfg, Credentials::default(), &mut out)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&out);
    assert!(
        text.contains("нечего удалять") || text.is_empty(),
        "the dry run reports no deletion"
    );
    assert!(plan_deletion(&db).unwrap().is_empty());
    assert_eq!(db.all_tracks().unwrap().len(), 1);
}
