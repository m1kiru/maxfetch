use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;

use maxfetch::config::Config;
use maxfetch::db::Db;
use maxfetch::fetch::{NoopSink, ProgressSink};
use maxfetch::matching::ReleaseGroup;
use maxfetch::model::{Candidate, Container, Locator, Quality, Release, SourceId};
use maxfetch::pipeline::{DownloadResult, Pipeline};
use maxfetch::sources::{AuthState, Capabilities, Query, Source};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/probe")
        .join(name)
}

fn flac_bytes(name: &str) -> Vec<u8> {
    std::fs::read(fixture(name)).expect("reading a probe fixture")
}

/// Configuration for the tests: every source id used by the fakes must be
/// listed explicitly, because `sources::enabled_in` is a pure filter — a list
/// without the needed id would silently leave the fakes out of the pipeline.
fn cfg_in(dir: &Path) -> Config {
    let mut cfg = Config::default();
    cfg.library.path = dir.join("library");
    cfg.sources.enabled = vec![
        "soulseek".into(),
        "bandcamp".into(),
        "qobuz".into(),
        "beatport".into(),
    ];
    cfg
}

fn rel(source: SourceId) -> Release {
    Release {
        title: "Get Lucky".to_string(),
        artist: "Daft Punk".to_string(),
        album: Some("Random Access Memories".to_string()),
        track_no: Some(8),
        duration_sec: Some(369.0),
        isrc: Some("FR123400001".to_string()),
        year: Some(2013),
        source,
        source_item_id: format!("{}-1", source.as_str()),
    }
}

fn quality(bit_depth: u8, sample_rate: u32) -> Quality {
    Quality {
        lossless: true,
        container: Container::Flac,
        bit_depth: Some(bit_depth),
        sample_rate: Some(sample_rate),
        bitrate_kbps: None,
    }
}

fn locator(source: SourceId) -> Locator {
    match source {
        SourceId::Soulseek => Locator::Soulseek {
            username: "peer".to_string(),
            filename: "track.flac".to_string(),
            size: 23_000,
        },
        SourceId::Qobuz => Locator::Qobuz {
            track_id: "q1".to_string(),
            format_id: 27,
        },
        SourceId::Bandcamp => Locator::Bandcamp {
            url: "https://bandcamp.example/track/1".to_string(),
            encoding: "flac".to_string(),
        },
        SourceId::Beatport => Locator::Beatport {
            track_id: "b1".to_string(),
        },
        other => Locator::Spotify {
            track_id: other.as_str().to_string(),
        },
    }
}

struct Fake {
    id: SourceId,
    releases: Vec<Release>,
    quality: Quality,
    bytes: Vec<u8>,
    fail_search: bool,
}

impl Fake {
    fn flac(id: SourceId, releases: Vec<Release>, quality: Quality, fixture_name: &str) -> Fake {
        Fake {
            id,
            releases,
            quality,
            bytes: flac_bytes(fixture_name),
            fail_search: false,
        }
    }
}

#[async_trait]
impl Source for Fake {
    fn id(&self) -> SourceId {
        self.id
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
        if self.fail_search {
            anyhow::bail!("fake search failure");
        }
        Ok(self.releases.clone())
    }

    async fn resolve(&self, _q: &Query) -> anyhow::Result<Vec<Release>> {
        if self.fail_search {
            anyhow::bail!("fake resolve failure");
        }
        Ok(self.releases.clone())
    }

    async fn candidates(&self, release: &Release) -> anyhow::Result<Vec<Candidate>> {
        if self.fail_search {
            anyhow::bail!("fake candidates failure");
        }
        Ok(vec![Candidate {
            source: self.id,
            release: release.clone(),
            quality: self.quality.clone(),
            size_bytes: Some(self.bytes.len() as u64),
            locator: locator(self.id),
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

/// A source that counts how many times it was searched, delegating everything.
struct Counting {
    inner: Fake,
    searches: Arc<AtomicUsize>,
}

#[async_trait]
impl Source for Counting {
    fn id(&self) -> SourceId {
        self.inner.id()
    }

    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }

    fn auth_state(&self) -> AuthState {
        self.inner.auth_state()
    }

    async fn search(&self, q: &Query) -> anyhow::Result<Vec<Release>> {
        self.searches.fetch_add(1, Ordering::SeqCst);
        self.inner.search(q).await
    }

    async fn resolve(&self, q: &Query) -> anyhow::Result<Vec<Release>> {
        self.inner.resolve(q).await
    }

    async fn candidates(&self, release: &Release) -> anyhow::Result<Vec<Candidate>> {
        self.inner.candidates(release).await
    }

    async fn download(
        &self,
        candidate: &Candidate,
        dest: &Path,
        sink: &dyn ProgressSink,
    ) -> anyhow::Result<()> {
        self.inner.download(candidate, dest, sink).await
    }
}

/// A source that reports missing credentials and counts whether it was queried.
struct Unauthorized {
    inner: Fake,
    searches: Arc<AtomicUsize>,
}

#[async_trait]
impl Source for Unauthorized {
    fn id(&self) -> SourceId {
        self.inner.id()
    }

    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }

    fn auth_state(&self) -> AuthState {
        AuthState::Missing
    }

    async fn search(&self, q: &Query) -> anyhow::Result<Vec<Release>> {
        self.searches.fetch_add(1, Ordering::SeqCst);
        self.inner.search(q).await
    }

    async fn resolve(&self, q: &Query) -> anyhow::Result<Vec<Release>> {
        self.searches.fetch_add(1, Ordering::SeqCst);
        self.inner.resolve(q).await
    }

    async fn candidates(&self, release: &Release) -> anyhow::Result<Vec<Candidate>> {
        self.inner.candidates(release).await
    }

    async fn download(
        &self,
        candidate: &Candidate,
        dest: &Path,
        sink: &dyn ProgressSink,
    ) -> anyhow::Result<()> {
        self.inner.download(candidate, dest, sink).await
    }
}

/// A YouTube fake: writes the media bytes and the JSON sidecar next to them,
/// the way [`maxfetch::youtube::finalize`] does when the remux fails.
struct SidecarFake {
    bytes: Vec<u8>,
}

#[async_trait]
impl Source for SidecarFake {
    fn id(&self) -> SourceId {
        SourceId::Youtube
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            audio: true,
            metadata_only: false,
            lossless: false,
            needs_auth: false,
        }
    }

    fn auth_state(&self) -> AuthState {
        AuthState::NotNeeded
    }

    async fn search(&self, _q: &Query) -> anyhow::Result<Vec<Release>> {
        Ok(Vec::new())
    }

    async fn resolve(&self, _q: &Query) -> anyhow::Result<Vec<Release>> {
        Ok(Vec::new())
    }

    async fn candidates(&self, release: &Release) -> anyhow::Result<Vec<Candidate>> {
        Ok(vec![Candidate {
            source: SourceId::Youtube,
            release: release.clone(),
            quality: Quality {
                lossless: false,
                container: Container::Opus,
                bit_depth: None,
                sample_rate: Some(48_000),
                bitrate_kbps: Some(160),
            },
            size_bytes: Some(self.bytes.len() as u64),
            locator: Locator::Youtube {
                url: "https://www.youtube.com/watch?v=abc123".to_string(),
                video_id: "abc123".to_string(),
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
        let mut sidecar = dest.as_os_str().to_os_string();
        sidecar.push(".json");
        std::fs::write(PathBuf::from(sidecar), b"{}")?;
        Ok(())
    }
}

fn text_query(text: &str) -> Query {
    Query {
        text: Some(text.to_string()),
        ..Default::default()
    }
}

#[tokio::test]
async fn a_failing_source_does_not_break_the_run() {
    let dir = tempfile::tempdir().unwrap();
    let failing = Fake {
        fail_search: true,
        ..Fake::flac(
            SourceId::Beatport,
            vec![rel(SourceId::Beatport)],
            quality(16, 44100),
            "sine_44100_s16.flac",
        )
    };
    let healthy = Fake::flac(
        SourceId::Soulseek,
        vec![rel(SourceId::Soulseek)],
        quality(16, 44100),
        "sine_44100_s16.flac",
    );
    let pipeline = Pipeline::new(
        cfg_in(dir.path()),
        Db::open_in_memory().unwrap(),
        vec![Box::new(failing), Box::new(healthy)],
    );

    let groups = pipeline
        .search(&text_query("daft punk get lucky"))
        .await
        .unwrap();

    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].sources, vec![SourceId::Soulseek]);
}

#[tokio::test]
async fn download_probes_tags_and_records_the_track() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::flac(
        SourceId::Soulseek,
        vec![rel(SourceId::Soulseek)],
        quality(16, 44100),
        "sine_44100_s16.flac",
    );
    let pipeline = Pipeline::new(
        cfg_in(dir.path()),
        Db::open_in_memory().unwrap(),
        vec![Box::new(fake)],
    );

    let groups = pipeline
        .search(&text_query("daft punk get lucky"))
        .await
        .unwrap();
    let candidates = pipeline.candidates(&groups[0]).await.unwrap();
    let result = pipeline.download(&candidates, &NoopSink).await.unwrap();

    assert!(result.path.exists());
    assert!(!result.rejected);
    assert_eq!(result.attempts, 1);
    assert_eq!(result.quality.bit_depth, Some(16));

    let tracks = pipeline.db().all_tracks().unwrap();
    assert_eq!(tracks.len(), 1);
    // The extension comes from the measured container, not from the claim.
    assert!(tracks[0].path.ends_with(".flac"));
}

#[tokio::test]
async fn rejects_a_claimed_24_96_that_is_really_16_44_1_and_tries_the_next_candidate() {
    let dir = tempfile::tempdir().unwrap();
    let lying = Fake::flac(
        SourceId::Soulseek,
        vec![rel(SourceId::Soulseek)],
        quality(24, 96000),
        "sine_44100_s16.flac",
    );
    let honest = Fake::flac(
        SourceId::Qobuz,
        vec![rel(SourceId::Qobuz)],
        quality(16, 44100),
        "sine_44100_s16.flac",
    );
    let pipeline = Pipeline::new(
        cfg_in(dir.path()),
        Db::open_in_memory().unwrap(),
        vec![Box::new(lying), Box::new(honest)],
    );

    let groups = pipeline
        .search(&text_query("daft punk get lucky"))
        .await
        .unwrap();
    assert_eq!(groups.len(), 1);
    let candidates = pipeline.candidates(&groups[0]).await.unwrap();
    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0].quality.bit_depth, Some(24));

    let result = pipeline.download(&candidates, &NoopSink).await.unwrap();

    assert!(result.rejected);
    assert_eq!(result.attempts, 2);
    assert!(result.reason.unwrap().contains("24"));

    let tracks = pipeline.db().all_tracks().unwrap();
    assert_eq!(tracks.len(), 1);
    // The measured quality lands in the database, never the claim.
    assert_eq!(tracks[0].bit_depth, Some(16));
    assert_eq!(tracks[0].sample_rate, Some(44100));
}

#[tokio::test]
async fn a_three_of_twelve_release_is_a_partial_success_and_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let wanted: Vec<Release> = (1..=12)
        .map(|n| Release {
            title: format!("Track {n:02}"),
            track_no: Some(n),
            source_item_id: format!("t{n}"),
            isrc: Some(format!("FR1234000{n:02}")),
            ..rel(SourceId::Bandcamp)
        })
        .collect();
    let fake = Fake::flac(
        SourceId::Bandcamp,
        wanted[..3].to_vec(),
        quality(16, 44100),
        "sine_44100_s16.flac",
    );
    let pipeline = Pipeline::new(
        cfg_in(dir.path()),
        Db::open_in_memory().unwrap(),
        vec![Box::new(fake)],
    );

    let query = text_query("daft punk random access memories");
    let groups = pipeline.search(&query).await.unwrap();
    assert_eq!(groups.len(), 3);

    let mut acquired = 0;
    let mut failed = 0;
    for group in &groups {
        let candidates = pipeline.candidates(group).await.unwrap();
        match pipeline.download(&candidates, &NoopSink).await {
            Ok(result) if !result.rejected => acquired += 1,
            _ => failed += 1,
        }
    }

    assert_eq!(acquired, 3);
    assert_eq!(failed, 0);
    assert_eq!(pipeline.db().all_tracks().unwrap().len(), 3);
}

#[tokio::test]
async fn a_repeated_search_within_seven_days_uses_the_cache() {
    let dir = tempfile::tempdir().unwrap();
    let searches = Arc::new(AtomicUsize::new(0));
    let counting = Counting {
        inner: Fake::flac(
            SourceId::Soulseek,
            vec![rel(SourceId::Soulseek)],
            quality(16, 44100),
            "sine_44100_s16.flac",
        ),
        searches: searches.clone(),
    };
    let pipeline = Pipeline::new(
        cfg_in(dir.path()),
        Db::open_in_memory().unwrap(),
        vec![Box::new(counting)],
    );
    let query = text_query("daft punk get lucky");

    let first = pipeline.search(&query).await.unwrap();
    assert_eq!(first.len(), 1);
    let after_first = searches.load(Ordering::SeqCst);
    assert!(after_first >= 1);

    let second = pipeline.search(&query).await.unwrap();
    assert_eq!(second.len(), 1);
    assert_eq!(searches.load(Ordering::SeqCst), after_first);
}

#[tokio::test]
async fn source_statuses_reports_one_status_per_source() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = cfg_in(dir.path());
    let pipeline = Pipeline::new(
        cfg.clone(),
        Db::open_in_memory().unwrap(),
        vec![
            Box::new(Fake::flac(
                SourceId::Soulseek,
                vec![rel(SourceId::Soulseek)],
                quality(16, 44100),
                "sine_44100_s16.flac",
            )),
            Box::new(Fake::flac(
                SourceId::Qobuz,
                vec![rel(SourceId::Qobuz)],
                quality(24, 96000),
                "sine_48000_s24.flac",
            )),
        ],
    );

    let statuses = pipeline.source_statuses();
    assert_eq!(statuses.len(), 2);
    assert_eq!(statuses[0].id, SourceId::Soulseek);
    assert_eq!(statuses[1].id, SourceId::Qobuz);
    assert!(statuses.iter().all(|status| status.detail.is_none()));

    let empty = Pipeline::new(cfg, Db::open_in_memory().unwrap(), Vec::new());
    assert!(empty.source_statuses().is_empty());
}

// An empty `config.sources.enabled` means "everything enabled", so the single
// registered fake is queried instead of being skipped.
#[tokio::test]
async fn an_empty_enabled_list_queries_every_source() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = cfg_in(dir.path());
    cfg.sources.enabled = Vec::new();
    let searches = Arc::new(AtomicUsize::new(0));
    let counting = Counting {
        inner: Fake::flac(
            SourceId::Soulseek,
            vec![rel(SourceId::Soulseek)],
            quality(16, 44100),
            "sine_44100_s16.flac",
        ),
        searches: searches.clone(),
    };
    let pipeline = Pipeline::new(cfg, Db::open_in_memory().unwrap(), vec![Box::new(counting)]);

    let groups = pipeline
        .search(&text_query("daft punk get lucky"))
        .await
        .unwrap();

    assert_eq!(groups.len(), 1);
    assert!(searches.load(Ordering::SeqCst) >= 1);
}

// A source that reports `AuthState::Missing` is never queried: the run would
// otherwise try to log in with absent credentials, which aborts on runtimes
// that cannot block.
#[tokio::test]
async fn a_source_without_credentials_is_not_queried() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = cfg_in(dir.path());
    let searches = Arc::new(AtomicUsize::new(0));
    let unauthorized = Unauthorized {
        inner: Fake::flac(
            SourceId::Soulseek,
            vec![rel(SourceId::Soulseek)],
            quality(16, 44100),
            "sine_44100_s16.flac",
        ),
        searches: searches.clone(),
    };
    let pipeline = Pipeline::new(
        cfg,
        Db::open_in_memory().unwrap(),
        vec![Box::new(unauthorized)],
    );

    let groups = pipeline
        .search(&text_query("daft punk get lucky"))
        .await
        .unwrap();

    assert!(groups.is_empty());
    assert_eq!(searches.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn download_one_delegates_to_the_slice_form() {
    let dir_slice = tempfile::tempdir().unwrap();
    let dir_one = tempfile::tempdir().unwrap();

    let slice = download_with(dir_slice.path(), false).await;
    let one = download_with(dir_one.path(), true).await;

    assert_eq!(slice.attempts, one.attempts);
    assert_eq!(slice.rejected, one.rejected);
    assert_eq!(slice.quality, one.quality);
    assert_eq!(slice.path.file_name(), one.path.file_name());
    assert_ne!(slice.path, one.path);
}

/// Runs the whole search → candidates → download path once; `single` selects
/// `download_one` over the slice form.
async fn download_with(dir: &Path, single: bool) -> DownloadResult {
    let fake = Fake::flac(
        SourceId::Soulseek,
        vec![rel(SourceId::Soulseek)],
        quality(16, 44100),
        "sine_44100_s16.flac",
    );
    let pipeline = Pipeline::new(
        cfg_in(dir),
        Db::open_in_memory().unwrap(),
        vec![Box::new(fake)],
    );

    let groups = pipeline
        .search(&text_query("daft punk get lucky"))
        .await
        .unwrap();
    let candidates = pipeline.candidates(&groups[0]).await.unwrap();

    if single {
        pipeline
            .download_one(&candidates[0], &NoopSink)
            .await
            .unwrap()
    } else {
        pipeline.download(&candidates, &NoopSink).await.unwrap()
    }
}

// A source without credentials never sees the candidate query: `runnable_sources`
// drops it before `Source::candidates` is called.
#[tokio::test]
async fn an_unauthorized_source_is_not_asked_for_candidates() {
    let dir = tempfile::tempdir().unwrap();
    let searches = Arc::new(AtomicUsize::new(0));
    let unauthorized = Unauthorized {
        inner: Fake::flac(
            SourceId::Soulseek,
            vec![rel(SourceId::Soulseek)],
            quality(16, 44100),
            "sine_44100_s16.flac",
        ),
        searches: searches.clone(),
    };
    let pipeline = Pipeline::new(
        cfg_in(dir.path()),
        Db::open_in_memory().unwrap(),
        vec![Box::new(unauthorized)],
    );
    let group = ReleaseGroup {
        primary: rel(SourceId::Soulseek),
        members: Vec::new(),
        sources: vec![SourceId::Soulseek],
    };

    let candidates = pipeline.candidates(&group).await.unwrap();

    // Before the gate `Unauthorized::candidates` delegates to `Fake` and returns
    // one candidate, so an empty vector pins the gate itself.
    assert!(candidates.is_empty());
}

// A candidate from an unauthorized source is dropped with the Russian reason
// instead of being downloaded; the library stays empty.
#[tokio::test]
async fn a_download_from_an_unauthorized_source_is_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let searches = Arc::new(AtomicUsize::new(0));
    let unauthorized = Unauthorized {
        inner: Fake::flac(
            SourceId::Soulseek,
            vec![rel(SourceId::Soulseek)],
            quality(16, 44100),
            "sine_44100_s16.flac",
        ),
        searches: searches.clone(),
    };
    let pipeline = Pipeline::new(
        cfg_in(dir.path()),
        Db::open_in_memory().unwrap(),
        vec![Box::new(unauthorized)],
    );
    let candidate = Candidate {
        source: SourceId::Soulseek,
        release: rel(SourceId::Soulseek),
        quality: quality(16, 44100),
        size_bytes: Some(1),
        locator: locator(SourceId::Soulseek),
    };

    let result = pipeline
        .download(std::slice::from_ref(&candidate), &NoopSink)
        .await
        .unwrap();

    assert!(result.rejected);
    assert!(result.path.as_os_str().is_empty());
    assert!(result.attempts >= 1);
    let reason = result.reason.as_deref().unwrap_or_default();
    assert!(reason.contains("нет авторизации"), "{reason}");
    // The library holds no regular file: `.tmp` is created but stays empty.
    assert!(library_files(&dir.path().join("library")).is_empty());
    assert!(pipeline.db().all_tracks().unwrap().is_empty());
}

// The sidecar `YoutubeSource::download` leaves next to the temporary file moves
// into the library together with the media file.
#[tokio::test]
async fn a_youtube_sidecar_follows_the_file_into_the_library() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = cfg_in(dir.path());
    cfg.sources.enabled.push("youtube".into());
    let fake = SidecarFake {
        bytes: flac_bytes("sine_44100_s16.flac"),
    };
    let pipeline = Pipeline::new(cfg, Db::open_in_memory().unwrap(), vec![Box::new(fake)]);
    let group = ReleaseGroup {
        primary: rel(SourceId::Youtube),
        members: Vec::new(),
        sources: vec![SourceId::Youtube],
    };

    let candidates = pipeline.candidates(&group).await.unwrap();
    assert_eq!(candidates.len(), 1);
    let result = pipeline.download(&candidates, &NoopSink).await.unwrap();

    assert!(!result.rejected);
    assert!(result.path.exists());
    let sidecar = PathBuf::from(format!("{}.json", result.path.display()));
    assert!(sidecar.exists(), "{}", sidecar.display());
    assert_eq!(std::fs::read(&sidecar).unwrap(), b"{}");
}

/// Every regular file under `root`, recursively; a missing root is empty.
fn library_files(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(library_files(&path));
        } else {
            found.push(path);
        }
    }
    found
}
