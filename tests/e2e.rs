#![cfg(feature = "e2e")]

//! One end-to-end pass over the real wiring: search, candidates, download,
//! then `library scan` and `library dedupe` through the headless CLI.
//!
//! The test is behind the `e2e` feature so the ordinary `cargo test` run stays
//! hermetic and fast; without the feature this file compiles to an empty test
//! binary.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use clap::Parser;

use maxfetch::cli::{Cli, run_with};
use maxfetch::config::{Config, Credentials, state_path};
use maxfetch::db::Db;
use maxfetch::fetch::{NoopSink, ProgressSink};
use maxfetch::model::{Candidate, Container, Locator, Quality, Release, SourceId};
use maxfetch::pipeline::Pipeline;
use maxfetch::sources::{AuthState, Capabilities, Query, Source};

struct FakeSource {
    bytes: Vec<u8>,
}

impl FakeSource {
    fn new() -> Self {
        let bytes = std::fs::read(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/probe/sine_44100_s16.flac"),
        )
        .unwrap();
        Self { bytes }
    }
}

fn rel() -> Release {
    Release {
        title: "Get Lucky".into(),
        artist: "Daft Punk".into(),
        album: Some("RAM".into()),
        track_no: Some(8),
        duration_sec: Some(369.0),
        isrc: Some("FR123400001".into()),
        year: Some(2013),
        source: SourceId::Soulseek,
        source_item_id: "r1".into(),
    }
}

#[async_trait]
impl Source for FakeSource {
    fn id(&self) -> SourceId {
        SourceId::Soulseek
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            audio: true,
            metadata_only: false,
            lossless: true,
            needs_auth: false,
        }
    }

    fn auth_state(&self) -> AuthState {
        AuthState::NotNeeded
    }

    async fn search(&self, _q: &Query) -> anyhow::Result<Vec<Release>> {
        Ok(vec![rel()])
    }

    async fn resolve(&self, _q: &Query) -> anyhow::Result<Vec<Release>> {
        Ok(vec![rel()])
    }

    async fn candidates(&self, release: &Release) -> anyhow::Result<Vec<Candidate>> {
        Ok(vec![Candidate {
            source: SourceId::Soulseek,
            release: release.clone(),
            quality: Quality {
                lossless: true,
                container: Container::Flac,
                bit_depth: Some(16),
                sample_rate: Some(44100),
                bitrate_kbps: None,
            },
            size_bytes: Some(self.bytes.len() as u64),
            locator: Locator::Soulseek {
                username: "peer".into(),
                filename: "f".into(),
                size: self.bytes.len() as u64,
            },
        }])
    }

    async fn download(
        &self,
        _candidate: &Candidate,
        dest: &Path,
        _sink: &dyn ProgressSink,
    ) -> anyhow::Result<()> {
        std::fs::write(dest, &self.bytes)?;
        Ok(())
    }
}

#[test]
fn search_fetch_scan_and_dedupe_dry_run() {
    // One test in its own binary and no tokio runtime yet, so mutating the
    // environment here is safe: state.db and the config both land under the
    // temporary directory.
    let dir = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("XDG_DATA_HOME", dir.path().join("data"));
        std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
        std::env::set_var("XDG_STATE_HOME", dir.path().join("state"));
    }

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let mut cfg = Config::default();
        cfg.library.path = dir.path().join("library");

        let db = Db::open(&state_path()).unwrap();
        db.migrate().unwrap();
        let pipeline = Pipeline::new(cfg.clone(), db, vec![Box::new(FakeSource::new())]);
        let q = Query {
            text: Some("daft punk get lucky".into()),
            url: None,
            items: vec![],
        };
        let groups = pipeline.search(&q).await.unwrap();
        assert_eq!(groups.len(), 1, "the fake source is enabled by default");
        let cands = pipeline.candidates(&groups[0]).await.unwrap();
        assert_eq!(cands.len(), 1);
        let res = pipeline.download(&cands, &NoopSink).await.unwrap();
        assert!(!res.rejected);
        assert!(res.path.starts_with(&cfg.library.path));
        assert_eq!(pipeline.db().all_tracks().unwrap().len(), 1);
        drop(pipeline);

        // A file that arrived outside maxfetch (a manual rip): the scan has to
        // index exactly this one file, because the download above is already
        // recorded in state.db.
        let manual = cfg
            .library
            .path
            .join("Manual")
            .join("Album")
            .join("01 Extra.flac");
        std::fs::create_dir_all(manual.parent().unwrap()).unwrap();
        std::fs::copy(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/probe/sine_44100_s16.flac"),
            &manual,
        )
        .unwrap();

        let mut out = Vec::new();
        let cli = Cli::parse_from(["maxfetch", "library", "scan"]);
        run_with(cli, cfg.clone(), Credentials::default(), &mut out)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&out);
        assert!(
            text.contains("проиндексировано файлов: 1"),
            "the scanned file is reported: {text}"
        );

        let mut out = Vec::new();
        let cli = Cli::parse_from(["maxfetch", "library", "dedupe"]);
        run_with(cli, cfg.clone(), Credentials::default(), &mut out)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&out);
        assert!(
            text.contains("нечего удалять") || text.is_empty(),
            "the dry run deletes nothing"
        );

        let mut out = Vec::new();
        let cli = Cli::parse_from(["maxfetch", "library", "dedupe"]);
        run_with(cli, cfg, Credentials::default(), &mut out)
            .await
            .unwrap();
        assert!(res.path.exists(), "the dry run deletes nothing");
    });
}
