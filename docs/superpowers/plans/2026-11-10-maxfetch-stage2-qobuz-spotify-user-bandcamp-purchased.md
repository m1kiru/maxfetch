# maxfetch Stage 2 (Qobuz, Spotify user-auth, Bandcamp purchased, SoundCloud OAuth) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the four authenticated capabilities of stage 2: a Qobuz audio source with a self-refreshing `app_id`/`app_secret` pair, Spotify OAuth2 Authorization Code + PKCE so playlists become readable, Bandcamp purchased content through the `identity` cookie, and SoundCloud OAuth so the original lossless file outranks the AAC transcode.

**Architecture:** The core plan is in place: `sources::Source`, the registry, the pipeline, `probe`, `db`, `fetch`, `config::Credentials` and `auth::login` (which still bails "not implemented in the core build"). Stage 2 adds no new core type. It grows three source modules (`sources/qobuz.rs` is new, `sources/bandcamp.rs` and `sources/soundcloud.rs` gain their purchased and original paths), plus one shared OAuth loopback helper (`src/oauth.rs`), one credential-persisting cell (`auth::PersistedCredentials`), and one honest quality-ceiling type (`sources::qobuz::QualityCeiling`) that the Sources screen reports instead of a silent downgrade. Every network or subprocess call sits behind an injectable hook, so the tests run on recorded fixtures, a local `tiny_http` mock and a fake `yt-dlp` script, with no live host except `127.0.0.1`.

**Tech Stack:** Rust 2024 edition, `tokio`, `reqwest` (HTTP, rustls), `async-trait`, `qobuz-api` 4.0.0 (wrapped inside `sources/qobuz.rs`; its types never leave the module), `rspotify` 0.16.1 (only inside `sources/spotify.rs`), `sha2` + `base64` (PKCE), `regex` (scraping), `zip` (Bandcamp album unpacking), `tiny_http` (dev-dependency, the mock HTTP and authorization server), `serde`/`serde_json`/`toml`.

**Spec:** `docs/superpowers/specs/2026-11-10-maxfetch-design.md` (sections 7.3, 7.5, 7.10, 8.2, 8.4, 8.5 and 8.9 are the contract; the core plan `docs/superpowers/plans/2026-11-10-maxfetch-core.md` defines every type consumed here, and the stage 1 / stage 1b plans define `sources/spotify.rs`, `sources/bandcamp.rs` and `sources/soundcloud.rs` as they exist before this plan starts).

## Global Constraints

- Crate name and binary name: `maxfetch`. License MIT. Rust edition 2024.
- Rust toolchain 1.99.0 is installed at `/run/current-system/sw/bin/cargo`; `cargo` must be on PATH.
- Do not add dependencies that perform audio transcoding. `ffmpeg` may only be invoked with `-c copy` (stream copy, no re-encode).
- Lossy files are stored exactly as delivered by the source. No lossy-to-lossless or lossy-to-lossy re-encoding anywhere in the codebase.
- Quality ordering: lossless beats lossy; inside lossless, `bit_depth` descending first, then `sample_rate` descending; inside lossy, `bitrate_kbps` descending first, then `sample_rate` descending. Missing values are never invented and always sort below known values of the same class.
- The unit of work is a track. An album is a grouping with a tracklist; partial success (3 of 12 tracks downloaded) is normal, not an error.
- Library layout is `<library>/<Artist>/<Album>/<NN> <Title>.<ext>`; `<Album>` defaults to `Singles`; `NN` is a two-digit track number.
- `maxfetch` never deletes files implicitly. Deduplication only marks `duplicate_of` in the database; deletion requires the explicit command `maxfetch library dedupe --apply --delete --yes`.
- `library scan` only indexes. It never renames, moves or deletes.
- Soulseek is a download-only client: no shared directories, no incoming port, no UPnP/NAT-PMP.
- Secrets live in `$XDG_CONFIG_HOME/maxfetch/credentials.toml` with mode 0600; the file is created with 0600 and a wider mode produces a warning at startup.
- `$XDG_CONFIG_HOME/maxfetch/config.toml` holds non-secret settings; `$XDG_DATA_HOME/maxfetch/state.db` holds the SQLite state (WAL mode).
- All source sections in `credentials.toml` are optional. A missing section means "source not authorized": the source is disabled and every other source keeps working.
- No source failure (network down, missing credentials, rotated API key) may abort a run; it is recorded in the log and the source is skipped.
- Rust code, identifiers, comments and commit messages are in English. User-facing strings (TUI labels, CLI output) are in Russian.
- Every commit is authored as `m1kiru <m1kiru@github.com>`.
- Every file that claims to be lossless (Qobuz FLAC, Bandcamp FLAC/WAV/AIFF/ALAC, SoundCloud original) is probed after download; the database and the tags store the measured quality, never the claimed one. A probe failure rejects the candidate and deletes the partial file; it never stores a guess.
- A quality ceiling is reported, never applied silently. When the account cannot deliver the requested `format_id`, `maxfetch` states `нет подписки Studio, максимум FLAC-CD 16/44.1` and offers the best format the account can actually fetch. It never claims 24 bit it cannot get.
- Only `qobuz-api` may know the Qobuz signing format, and only `rspotify` may know Spotify's response shapes. Both are wrapped inside their source module.
- Concurrent credential writes are serialised through one process-wide mutex; two sources never truncate `credentials.toml` at the same time.
- Stage 2 adds no new subcommand. `maxfetch auth login <source>` from the core CLI is the only user-authorization entry point, and it gains `spotify`, `bandcamp`, `soundcloud` and `qobuz`.
- Qobuz is unusable on this machine because `api.qobuz.com` does not resolve. The source must degrade to a status line, and every other source must keep working.

## Review Focus

The spec is a vision document; these inputs are implied by it but not covered by any explicit requirement. Each has a test pinned to the task that owns the relevant code.

1. **An `app_id`/`app_secret` pair that has rotated since the last scrape.** The bundle changed, the stored pair returns 401. Expected: one re-scrape of `play.qobuz.com/bundle.js`, one retry, the fresh pair persisted, status `ключи протухли, обновлены`, and only a second failure yields `AuthState::Expired`. Pinned to Task 1 (scrape) and Task 2 (source).
2. **A Qobuz track whose `format_id` promises 24/96 while the account has no Studio subscription.** Expected: the ceiling is `min(track format ceiling, account ceiling)` = FLAC-CD, `QualityCeiling` carries `max_lossless` 16/44.1 and the Russian reason, `candidates` offers `format_id` 6 instead of 27, and the Sources screen prints the ceiling; asking for 24 bit and failing on every attempt is the bug this prevents. A track that is FLAC-CD already is not "capped" and prints no reason. Pinned to Task 1 (ladder) and Task 2 (candidates).
3. **A Spotify refresh token revoked by a password change.** Expected: the read reports `нужна повторная авторизация`, the source drops to `client_credentials` for search and metadata, playlists vanish from results, the other sources are untouched. Pinned to Task 4 (refresh and login) and Task 5 (playlist).
4. **A Bandcamp `identity` cookie that returns an empty collection instead of a 401.** Expected: an empty redownload list, status `нет доступа к покупкам`, `AuthState::Expired`, and the stage 1b catalogue and promo paths unchanged. Pinned to Task 6 (lookup) and Task 7 (redownload).
5. **A SoundCloud track offering both a FLAC original and the 160 kbps AAC HLS transcode.** Expected: the lossless original beats the AAC transcode for auto-selection (the ranked list holds only the original, §7.4), while the AAC candidate remains visible in the candidate list with `lossless = false` and its own `format: SoundCloudFormat::Aac160` locator. Pinned to Task 8 (original and ranking) and Task 9 (end-to-end).

---

### Task 1: Qobuz credential scraping and the format ladder

**Files:**
- Create: `src/sources/qobuz.rs`
- Create: `tests/fixtures/qobuz/bundle.js`
- Create: `tests/fixtures/qobuz/login.json`
- Create: `tests/fixtures/qobuz/track_27.json`
- Test: `tests/qobuz_quality.rs`

**Interfaces:**
- Consumes: `maxfetch::model::{Container, Quality, SourceId}`, `maxfetch::config::QobuzCredentials` (core Tasks 1 and 3).
- Produces:
  - `maxfetch::sources::qobuz::QobuzFormat` — `enum { Mp3_320, FlacCd, FlacHiRes96, FlacHiRes192 }`, `Copy + Debug + Eq + PartialEq`; `QobuzFormat::from_format_id(id: u32) -> Option<QobuzFormat>` maps 5, 6, 7, 27 and returns `None` for anything else; `QobuzFormat::format_id(&self) -> u32` is the inverse.
  - `maxfetch::sources::qobuz::quality_of(format: QobuzFormat) -> Quality` — 5 → lossy `Mp3`, 320 kbps, `sample_rate: None` (the spec pins no sample rate for MP3-320); 6 → lossless `Flac`, 16/44100; 7 → lossless `Flac`, 24/96000; 27 → lossless `Flac`, 24/192000.
  - `maxfetch::sources::qobuz::scrape_app_credentials(bundle_js: &str) -> Option<(String, String)>` — applies the regex `production:{api:{appId:"(\d{9})",appSecret:"(\w{32})"` and returns `(app_id, app_secret)`.
  - `maxfetch::sources::qobuz::QualityCeiling` — `struct { max_lossless: Quality, reason: Option<String> }`, `Clone + Debug + PartialEq`.
  - `maxfetch::sources::qobuz::format_ceiling(format: QobuzFormat) -> Quality` — the best `Quality` that `format` can deliver: `Mp3_320` → lossy `Mp3`, 320 kbps, `sample_rate: None`; `FlacCd` → 16/44100; `FlacHiRes96` → 24/96000; `FlacHiRes192` → 24/192000.
  - `maxfetch::sources::qobuz::account_ceiling(studio: bool) -> Quality` — with Studio the ladder maximum (24/192000); without it FLAC-CD (16/44100).
  - `maxfetch::sources::qobuz::ceiling_from_limits(track_max: QobuzFormat, studio: bool) -> QualityCeiling` — `max_lossless = min(format_ceiling(track_max), account_ceiling(studio))` under the §7.4 ordering (depth, then rate); `reason` is `Some("нет подписки Studio, максимум FLAC-CD 16/44.1")` exactly when the account ceiling capped the result (`account_ceiling(studio) < format_ceiling(track_max)`), otherwise `None`. The tracked `format_id` of the account is the real limit; the `studio` flag only decides the account ceiling, so a 24/192 track on a non-Studio account yields a 16/44.1 ceiling with the reason, and a 16/44.1 track on a non-Studio account yields `None` (nothing was capped).
  - `maxfetch::sources::qobuz::best_format_within(ceiling: &QualityCeiling, wanted: QobuzFormat) -> QobuzFormat` — returns `wanted` when it fits the ceiling, otherwise the best ladder rung the ceiling allows.

- [ ] **Step 1: Write the failing test**

Create `tests/fixtures/qobuz/bundle.js` with the minified fragment the scraper sees:

```javascript
var Q=function(){return{production:{api:{appId:"950096962",appSecret:"a1b2c3d4e5f60718293a4b5c6d7e8f90"}}}}
```

Create `tests/fixtures/qobuz/login.json`:

```json
{ "user_auth_token": "fake-user-token", "user": { "id": 12345, "display_name": "listener", "credential": { "id": 12345, "parameters": { "lossless": true, "hires": false, "short_label": "FLAC-CD" } } } }
```

Create `tests/fixtures/qobuz/track_27.json`:

```json
{ "id": 987654, "title": "Get Lucky", "performer": { "name": "Daft Punk" }, "album": { "title": "Random Access Memories", "max_format_id": 27 }, "isrc": "USQX91300888", "duration": 369, "maximum_bit_depth": 24, "maximum_sampling_rate": 96 }
```

Create `tests/qobuz_quality.rs`:
```rust
use maxfetch::model::Container;
use maxfetch::sources::qobuz::{
    best_format_within, ceiling_from_limits, quality_of, scrape_app_credentials, QobuzFormat,
};

fn fixture(rel: &str) -> String {
    std::fs::read_to_string(format!("{}/tests/fixtures/qobuz/{}", env!("CARGO_MANIFEST_DIR"), rel)).unwrap()
}

#[test]
fn app_credentials_are_scraped_from_the_bundle() {
    let (app_id, app_secret) = scrape_app_credentials(&fixture("bundle.js")).unwrap();
    assert_eq!(app_id, "950096962");
    assert_eq!(app_secret, "a1b2c3d4e5f60718293a4b5c6d7e8f90");
}

#[test]
fn a_bundle_without_the_pair_yields_nothing() {
    assert!(scrape_app_credentials("var x = 1;").is_none());
}

#[test]
fn format_ids_map_to_the_quality_ladder() {
    let mp3 = quality_of(QobuzFormat::from_format_id(5).unwrap());
    assert!(!mp3.lossless);
    assert_eq!(mp3.container, Container::Mp3);
    assert_eq!(mp3.bitrate_kbps, Some(320));
    assert_eq!(mp3.sample_rate, None, "the spec pins no sample rate for MP3-320");

    let cd = quality_of(QobuzFormat::from_format_id(6).unwrap());
    assert!(cd.lossless);
    assert_eq!((cd.bit_depth, cd.sample_rate), (Some(16), Some(44100)));

    let hi = quality_of(QobuzFormat::from_format_id(7).unwrap());
    assert_eq!((hi.bit_depth, hi.sample_rate), (Some(24), Some(96000)));

    let ultra = quality_of(QobuzFormat::from_format_id(27).unwrap());
    assert_eq!((ultra.bit_depth, ultra.sample_rate), (Some(24), Some(192000)));

    assert!(QobuzFormat::from_format_id(9).is_none());
}

#[test]
fn without_studio_the_ceiling_is_flac_cd_and_says_so() {
    let ceiling = ceiling_from_limits(QobuzFormat::FlacHiRes192, false);
    assert_eq!(
        (ceiling.max_lossless.bit_depth, ceiling.max_lossless.sample_rate),
        (Some(16), Some(44100))
    );
    let reason = ceiling.reason.unwrap();
    assert!(reason.contains("нет подписки Studio"), "{reason}");
    assert!(reason.contains("FLAC-CD 16/44.1"), "{reason}");
    assert_eq!(best_format_within(&ceiling, QobuzFormat::FlacHiRes192), QobuzFormat::FlacCd);
    assert_eq!(best_format_within(&ceiling, QobuzFormat::FlacCd), QobuzFormat::FlacCd);
}

#[test]
fn with_studio_the_requested_format_is_kept() {
    let ceiling = ceiling_from_limits(QobuzFormat::FlacHiRes96, true);
    assert!(ceiling.reason.is_none());
    assert_eq!(best_format_within(&ceiling, QobuzFormat::FlacHiRes96), QobuzFormat::FlacHiRes96);
    assert_eq!(best_format_within(&ceiling, QobuzFormat::Mp3_320), QobuzFormat::Mp3_320);
}

#[test]
fn a_flac_cd_track_on_a_non_studio_account_is_not_capped() {
    let ceiling = ceiling_from_limits(QobuzFormat::FlacCd, false);
    assert_eq!((ceiling.max_lossless.bit_depth, ceiling.max_lossless.sample_rate), (Some(16), Some(44100)));
    assert!(ceiling.reason.is_none(), "nothing was capped, so there is nothing to report");
}

#[test]
fn a_24_192_track_on_a_non_studio_account_is_capped_to_flac_cd() {
    let ceiling = ceiling_from_limits(QobuzFormat::FlacHiRes192, false);
    assert_eq!((ceiling.max_lossless.bit_depth, ceiling.max_lossless.sample_rate), (Some(16), Some(44100)));
    assert_eq!(best_format_within(&ceiling, QobuzFormat::FlacHiRes192), QobuzFormat::FlacCd);
}

#[test]
fn the_ladder_is_an_involution() {
    for id in [5u32, 6, 7, 27] {
        assert_eq!(QobuzFormat::from_format_id(id).unwrap().format_id(), id);
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test qobuz_quality`
Expected: FAIL with "unresolved import `maxfetch::sources::qobuz`".

- [ ] **Step 3: Implement the scraping and the ladder in `src/sources/qobuz.rs`**

Add `cargo add regex`. Hold the regex in a `static` built with `std::sync::LazyLock`, so no extra crate is needed. `from_format_id` is an exhaustive match on 5, 6, 7, 27 with `_ => None`; `quality_of` sets `bitrate_kbps: Some(320)` and `sample_rate: None` for `Mp3_320` and `None` for the three FLAC rungs; `format_ceiling` is the per-rung `Quality`; `account_ceiling` returns the 24/192000 `Quality` with Studio and the 16/44100 one without it; `ceiling_from_limits(track_max, studio)` takes their minimum under the §7.4 ordering and sets the Russian sentence verbatim in `reason` only when `account_ceiling(studio) < format_ceiling(track_max)`; `best_format_within` orders the rungs by `format_id` and returns `wanted` when `wanted.format_id() <= ceiling best rung`, else the greatest rung at or below the ceiling.

No network, no `qobuz-api` dependency in this task.

**Dependency decision (resolves the `qobuz-api` 4.0.0 uncertainty).** In Task 2 run `cargo add qobuz-api@4.0.0` first. If the crate resolves and its API covers login, `track/get`, `album/get` and `track/getFileUrl`, `HttpQobuzClient` wraps it (priority 1, the preferred path: signature and deciphering stay in the crate). If the crate does not resolve, fails to build, or does not expose those endpoints, do **not** block: implement `HttpQobuzClient` directly over `reqwest` against `https://api.qobuz.com/track/getFileUrl` (and the matching metadata endpoints), walking the `format_id` ladder of §8.5 manually and computing the request signature with `sha2` (priority 2). Both variants satisfy the same `QobuzClient` trait, so no other task changes. Record in the commit message which variant was taken.

- [ ] **Step 4: Declare the module and run the test**

Add `pub mod qobuz;` to `src/sources/mod.rs` (create the file only if the stage 1b plans have not).

Run: `cargo test --test qobuz_quality`
Expected: PASS, 8 tests.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(qobuz): app credential scraping and format ladder"
```

---

### Task 2: The Qobuz source

**Files:**
- Modify: `src/sources/qobuz.rs`
- Test: `tests/qobuz_source.rs`

**Interfaces:**
- Consumes: `maxfetch::sources::{AuthState, Capabilities, Query, QueryItem, Source, SourceStatus}`, `maxfetch::model::{Candidate, Locator, Release, SourceId}`, `maxfetch::config::QobuzCredentials`, `maxfetch::fetch::{download_to_file, http_client, ProgressSink}`, Task 1's `scrape_app_credentials`, `quality_of`, `ceiling_from_limits`, `best_format_within`, `QualityCeiling`.
- Produces:
  - `maxfetch::sources::qobuz::QobuzSession` — `struct { app_id: String, app_secret: String, user_token: String, studio: bool }`, `Clone + Debug`.
  - `maxfetch::sources::qobuz::QobuzTrack` — `struct { id: String, title: String, performer: String, album: Option<String>, isrc: Option<String>, duration_sec: Option<f64>, year: Option<i32>, max_format: QobuzFormat }`, `Clone + Debug`.
  - `maxfetch::sources::qobuz::QobuzClient` — `#[async_trait] trait QobuzClient: Send + Sync` with `async fn login(&self, email: &str, password: &str, app_id: &str, app_secret: &str) -> anyhow::Result<QobuzSession>`, `async fn fetch_bundle(&self) -> anyhow::Result<String>`, `async fn track(&self, session: &QobuzSession, track_id: &str) -> anyhow::Result<QobuzTrack>`, `async fn stream_url(&self, session: &QobuzSession, track_id: &str, format: QobuzFormat) -> anyhow::Result<String>`.
  - `maxfetch::sources::qobuz::HttpQobuzClient` — the production implementation, the only code that talks to `api.qobuz.com` or constructs a `qobuz-api` type; `HttpQobuzClient::new(timeout_sec: u64) -> HttpQobuzClient`.
  - `maxfetch::sources::qobuz::QobuzSource` — `struct { client: Box<dyn QobuzClient>, creds: Option<QobuzCredentials>, session: tokio::sync::Mutex<Option<QobuzSession>>, last_key_refresh: tokio::sync::Mutex<Option<std::time::Instant>>, last_error: tokio::sync::Mutex<Option<String>>, refreshed: std::sync::atomic::AtomicBool }`; constructors `QobuzSource::new(client: Box<dyn QobuzClient>, creds: Option<QobuzCredentials>) -> QobuzSource` (no credential persistence; used where no `credentials.toml` is at hand), `QobuzSource::with_dir(client: Box<dyn QobuzClient>, creds: Option<QobuzCredentials>, dir: &Path) -> QobuzSource` (the constructor that persists a scraped pair into `dir`) and `QobuzSource::from_credentials(creds: &QobuzCredentials, dir: &Path) -> QobuzSource` (production: `with_dir` over `HttpQobuzClient::new(…)`).
  - `impl Source for QobuzSource` implements `status(&self) -> SourceStatus` (the trait method Task 9 adds as a defaulted method). There is **no** inherent `QobuzSource::status`; an inherent method with the same name would shadow the trait default and the trait call would return an empty `detail`. The trait impl returns `SourceStatus { id: SourceId::Qobuz, capabilities: Capabilities { audio: true, metadata_only: false, lossless: true, needs_auth: true }, auth, detail }`, where `auth` is `Missing` when `creds` is `None`, `Expired` after a failed login, `Ok` otherwise, and `detail` is, in priority order: the last error (`нет учётных данных` when `creds` is `None`), then `ключи протухли, обновлены` while the `refreshed` flag is set, then `ceiling_from_limits(track_max, session.studio).reason` for the last resolved track's `max_format`. `detail` is always filled for a source that has something to say, otherwise `None`.
  - **Scraped-pair persistence.** The scraped `app_id`/`app_secret` are never thrown away: `login_with_key_refresh` writes the pair it used into the `[qobuz]` section of `credentials.toml` (`app_id`, `app_secret`) through `Credentials::save_to(dir)` (Task 4's `PersistedCredentials` write mutex). On the next start `from_credentials` uses the stored pair when both fields are non-empty and scrapes only when the pair is absent or the server answered with an authentication error. The status `ключи протухли, обновлены` is therefore a one-off, and a stored pair that keeps failing is diagnosed as `AuthState::Expired` rather than re-scraped on every run.
  - `maxfetch::sources::qobuz::KEY_REFRESH_COOLDOWN` — `const : std::time::Duration` = 60 s.

- [ ] **Step 1: Write the failing test**

Create `tests/qobuz_source.rs`:
```rust
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use maxfetch::config::QobuzCredentials;
use maxfetch::fetch::NoopSink;
use maxfetch::model::{Container, Locator, SourceId};
use maxfetch::sources::qobuz::{QobuzClient, QobuzFormat, QobuzSource, QobuzSession, QobuzTrack};
use maxfetch::sources::{AuthState, Query, QueryItem, Source};

fn creds() -> Option<QobuzCredentials> {
    Some(QobuzCredentials { email: "listener@example.com".into(), password: "pw".into(), app_id: "950096962".into(), app_secret: "a1b2c3d4e5f60718293a4b5c6d7e8f90".into() })
}

fn track() -> QobuzTrack {
    QobuzTrack {
        id: "987654".into(),
        title: "Get Lucky".into(),
        performer: "Daft Punk".into(),
        album: Some("Random Access Memories".into()),
        isrc: Some("USQX91300888".into()),
        duration_sec: Some(369.0),
        year: Some(2013),
        max_format: QobuzFormat::FlacHiRes192,
    }
}

struct FakeQobuz {
    studio: bool,
    logins: Arc<AtomicUsize>,
    scrapes: Arc<AtomicUsize>,
    fail_upto: usize,
    stream_base: String,
}

#[async_trait]
impl QobuzClient for FakeQobuz {
    async fn fetch_bundle(&self) -> anyhow::Result<String> {
        self.scrapes.fetch_add(1, Ordering::SeqCst);
        Ok(std::fs::read_to_string(format!("{}/tests/fixtures/qobuz/bundle.js", env!("CARGO_MANIFEST_DIR"))).unwrap())
    }
    async fn login(&self, _e: &str, _p: &str, app_id: &str, _s: &str) -> anyhow::Result<QobuzSession> {
        let n = self.logins.fetch_add(1, Ordering::SeqCst) + 1;
        assert_eq!(app_id, "950096962", "login must use the scraped app id");
        if n <= self.fail_upto {
            anyhow::bail!("Qobuz login failed: 401");
        }
        Ok(QobuzSession { app_id: app_id.into(), app_secret: "s".into(), user_token: "t".into(), studio: self.studio })
    }
    async fn track(&self, _s: &QobuzSession, _id: &str) -> anyhow::Result<QobuzTrack> {
        Ok(track())
    }
    async fn stream_url(&self, _s: &QobuzSession, _id: &str, format: QobuzFormat) -> anyhow::Result<String> {
        Ok(format!("{}/stream?format={}", self.stream_base, format.format_id()))
    }
}

fn source(fake: FakeQobuz) -> QobuzSource {
    QobuzSource::new(Box::new(fake), creds())
}

#[tokio::test]
async fn the_scraped_pair_is_persisted_and_reused() {
    let dir = tempfile::tempdir().unwrap();
    let creds = QobuzCredentials { email: "listener@example.com".into(), password: "pw".into(), app_id: String::new(), app_secret: String::new() };
    // An absent pair means the first resolve scrapes the bundle and persists the fresh pair.
    let src = QobuzSource::with_dir(Box::new(fake(true)), Some(creds), dir.path());
    src.resolve(&query()).await.unwrap();
    let saved = maxfetch::config::Credentials::load_from(dir.path()).unwrap().qobuz.unwrap();
    assert_eq!(saved.app_id, "950096962");
    assert_eq!(saved.app_secret, "a1b2c3d4e5f60718293a4b5c6d7e8f90");
    assert_eq!(saved.email, "listener@example.com");
}

#[tokio::test]
async fn a_stored_pair_is_not_scraped_again() {
    let dir = tempfile::tempdir().unwrap();
    let stored = QobuzCredentials { email: "listener@example.com".into(), password: "pw".into(), app_id: "950096962".into(), app_secret: "a1b2c3d4e5f60718293a4b5c6d7e8f90".into() };
    let scrapes = Arc::new(AtomicUsize::new(0));
    let src = QobuzSource::with_dir(
        Box::new(FakeQobuz { studio: true, logins: Default::default(), scrapes: scrapes.clone(), fail_upto: 0, stream_base: "http://127.0.0.1:1".into() }),
        Some(stored),
        dir.path(),
    );
    src.resolve(&query()).await.unwrap();
    assert_eq!(scrapes.load(Ordering::SeqCst), 0, "a working stored pair must never be re-scraped");
}

fn query() -> Query {
    Query { text: None, url: None, items: vec![QueryItem::Track { source: SourceId::Qobuz, id: "987654".into() }] }
}

fn fake(studio: bool) -> FakeQobuz {
    FakeQobuz {
        studio,
        logins: Default::default(),
        scrapes: Default::default(),
        fail_upto: 0,
        stream_base: "http://127.0.0.1:1".into(),
    }
}

#[tokio::test]
async fn resolve_returns_a_release_with_the_isrc() {
    let src = source(fake(true));
    let releases = src.resolve(&query()).await.unwrap();
    assert_eq!(releases.len(), 1);
    assert_eq!(releases[0].source, SourceId::Qobuz);
    assert_eq!(releases[0].source_item_id, "987654");
    assert_eq!(releases[0].isrc.as_deref(), Some("USQX91300888"));
    assert_eq!(releases[0].year, Some(2013));
}

#[tokio::test]
async fn candidates_report_the_studio_ceiling_honestly() {
    let src = source(fake(false));
    let release = src.resolve(&query()).await.unwrap().remove(0);
    let cands = src.candidates(&release).await.unwrap();
    assert_eq!(cands.len(), 1);
    let q = &cands[0].quality;
    assert!(q.lossless);
    assert_eq!((q.container, q.bit_depth, q.sample_rate), (Container::Flac, Some(16), Some(44100)));
    assert!(matches!(&cands[0].locator, Locator::Qobuz { format_id: 6, .. }));
    assert!(src.status().detail.unwrap().contains("нет подписки Studio"));
}

#[tokio::test]
async fn a_studio_account_gets_the_24_bit_candidate() {
    let src = source(fake(true));
    let release = src.resolve(&query()).await.unwrap().remove(0);
    let cands = src.candidates(&release).await.unwrap();
    assert_eq!(cands.len(), 1);
    assert_eq!(cands[0].quality.bit_depth, Some(24));
    assert!(matches!(&cands[0].locator, Locator::Qobuz { format_id: 27, .. }));
    assert_eq!(src.status().auth, AuthState::Ok);
}

#[tokio::test]
async fn rotated_keys_are_rescraped_once_and_reported() {
    let logins = Arc::new(AtomicUsize::new(0));
    let scrapes = Arc::new(AtomicUsize::new(0));
    let src = source(FakeQobuz {
        studio: true,
        logins: logins.clone(),
        scrapes: scrapes.clone(),
        fail_upto: 1,
        stream_base: "http://127.0.0.1:1".into(),
    });
    src.resolve(&query()).await.unwrap();
    assert_eq!(logins.load(Ordering::SeqCst), 2);
    assert_eq!(scrapes.load(Ordering::SeqCst), 2);
    assert!(src.status().detail.unwrap().contains("ключи протухли, обновлены"));
}

#[tokio::test]
async fn a_second_failure_after_rescraping_stays_expired_and_is_not_retried_forever() {
    let logins = Arc::new(AtomicUsize::new(0));
    let scrapes = Arc::new(AtomicUsize::new(0));
    let src = source(FakeQobuz {
        studio: true,
        logins: logins.clone(),
        scrapes: scrapes.clone(),
        fail_upto: usize::MAX,
        stream_base: "http://127.0.0.1:1".into(),
    });
    let first = src.resolve(&query()).await;
    assert!(first.is_err(), "both logins fail");
    assert_eq!(scrapes.load(Ordering::SeqCst), 2, "one initial scrape plus one refresh");
    assert_eq!(src.auth_state(), AuthState::Expired);

    let second = src.resolve(&query()).await;
    assert!(second.is_err());
    assert_eq!(scrapes.load(Ordering::SeqCst), 2, "the cooldown must block a second re-scrape");
    assert_eq!(logins.load(Ordering::SeqCst), 3, "one attempt per call, never a loop");
}

#[tokio::test]
async fn missing_credentials_disable_only_this_source() {
    let src = QobuzSource::new(Box::new(fake(true)), None);
    assert_eq!(src.id(), SourceId::Qobuz);
    assert!(src.capabilities().audio);
    assert!(src.capabilities().lossless);
    assert!(src.capabilities().needs_auth);
    assert_eq!(src.auth_state(), AuthState::Missing);
    assert!(src.status().detail.unwrap().contains("нет учётных данных"));
    assert!(src.resolve(&query()).await.is_err());
}

#[tokio::test]
async fn download_writes_the_streamed_bytes_to_dest() {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let port = server.server_addr().to_ip().unwrap().port();
    let handle = std::thread::spawn(move || {
        let req = server.recv().unwrap();
        let bytes = std::fs::read(format!("{}/tests/fixtures/probe/sine_44100_s16.flac", env!("CARGO_MANIFEST_DIR"))).unwrap();
        req.respond(tiny_http::Response::from_data(bytes)).unwrap();
    });
    let mut f = fake(true);
    f.stream_base = format!("http://127.0.0.1:{port}");
    let src = source(f);
    let release = src.resolve(&query()).await.unwrap().remove(0);
    let cand = src.candidates(&release).await.unwrap().remove(0);
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("track.flac");
    src.download(&cand, &dest, &NoopSink).await.unwrap();
    assert!(dest.exists());
    assert!(std::fs::metadata(&dest).unwrap().len() > 0);
    handle.join().unwrap();
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test qobuz_source`
Expected: FAIL with "unresolved import `maxfetch::sources::qobuz::QobuzSource`".

- [ ] **Step 3: Implement the source**

Add `cargo add qobuz-api@4.0.0`, `cargo add async-trait`, `cargo add --dev tiny_http`. Apply the dependency decision recorded in Task 1: wrap `qobuz-api` when it resolves and covers login / `track/get` / `album/get` / `track/getFileUrl`; otherwise implement `HttpQobuzClient` over `reqwest` against `https://api.qobuz.com` with the `sha2` signature and the §8.5 ladder. `HttpQobuzClient::new(timeout_sec)` holds `fetch::http_client(timeout_sec)`; `fetch_bundle` GETs `https://play.qobuz.com/bundle.js`; `login` fills `studio` from `user.credential.parameters.hires` (falling back to `lossless` when `hires` is absent); `track` maps the response into `QobuzTrack` and reads `album.max_format_id` as `max_format` (defaulting to `FlacCd` when the field is missing); `stream_url` asks for the signed URL of that `format_id`. No client-crate type appears in any signature above.

`QobuzSource::login_with_key_refresh(email, password)` is the only token algorithm and does exactly this:

```text
pair := (creds.app_id, creds.app_secret)
if pair.app_id is empty or pair.app_secret is empty:
    pair := scrape(client.fetch_bundle()) or bail "Qobuz: не удалось получить app_id из bundle.js"
    persist pair into creds.qobuz.app_id / creds.qobuz.app_secret (Credentials::save_to(dir))
first := client.login(email, password, pair)
if first is Ok: return first
if the first error does not contain "401": return Err(first)
if last_key_refresh is Some(t) and t.elapsed() < KEY_REFRESH_COOLDOWN: return Err(first)
fresh := scrape(client.fetch_bundle()) or bail "Qobuz: ключи протухли, обновить не удалось: <first>"
second := client.login(email, password, fresh)
if second is Err: return Err("Qobuz: ключи протухли, повторный вход не удался: <second>")
persist fresh into creds (Credentials::save_to(dir))
refreshed := true
return second
```

A stored pair is always tried first and never re-scraped while it works; the scrape happens only when the stored pair is missing or the server rejected it with an authentication error. `with_dir`/`from_credentials` keep `dir`, the directory of `credentials.toml`, for those writes; `QobuzSource::new` (used only where no file is written) drops the pair after the session instead of persisting it, so a caller that has no `dir` never silently loses it — Task 9's `registry` uses `from_credentials`.

`session()` returns the cached session when present, otherwise calls `login_with_key_refresh` with the e-mail and password from `creds` (bailing `"Qobuz: нет учётных данных"` when `creds` is `None`) and stores the result plus `Instant::now()`. On every failure it also writes the message into `last_error`.

`impl Source for QobuzSource::status()` (the trait method, not an inherent one) returns `SourceStatus { id: Qobuz, capabilities: Capabilities { audio: true, metadata_only: false, lossless: true, needs_auth: true }, auth, detail }`, where `detail` is, in priority order: the last error (`нет учётных данных` when `creds` is `None`), then `ключи протухли, обновлены` while the `refreshed` flag is set, then `ceiling_from_limits(track.max_format, session.studio).reason`. `auth` is `Missing` without credentials, `Expired` after a failed login, `Ok` otherwise.

`resolve` accepts only `QueryItem::Track { source: Qobuz, id }` and maps each `QobuzTrack` into a `Release`; other item types are ignored. `candidates` recomputes the track, takes `wanted = track.max_format`, applies `best_format_within(&ceiling_from_limits(track.max_format, studio), wanted)` and emits exactly one `Candidate` at that format with `Locator::Qobuz { track_id, format_id }` and `quality_of(chosen)`. `download` calls `stream_url` and then `fetch::download_to_file(client, &url, dest, sink)`; it does not probe (the pipeline owns probing).

- [ ] **Step 4: Run the test**

Run: `cargo test --test qobuz_source`
Expected: PASS, 9 tests.

Do not register the source in `registry` yet; Task 9 does that once every stage 2 source exists.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(qobuz): source with key refresh and honest quality ceiling"
```

---

### Task 3: The OAuth loopback helper

**Files:**
- Create: `src/oauth.rs`
- Test: `tests/oauth_loopback.rs`

**Interfaces:**
- Consumes: nothing outside `std`, `tokio`, `sha2` and `base64`.
- Produces:
  - `maxfetch::oauth::Pkce` — `struct { verifier: String, challenge: String }`, `Clone + Debug`; `Pkce::generate() -> Pkce` (64-character verifier from the RFC 7636 unreserved set, challenge `BASE64URL(SHA256(verifier))` without padding).
  - `maxfetch::oauth::LoopbackServer` — `LoopbackServer::bind(port: u16) -> anyhow::Result<LoopbackServer>`, `port(&self) -> u16`, `redirect_uri(&self) -> String` (always `http://127.0.0.1:<port>/callback`), `expect_state(&mut self, state: &str)`, `async fn wait_for_code(self, timeout: std::time::Duration) -> anyhow::Result<String>`.
  - `maxfetch::oauth::authorization_url(auth_endpoint: &str, client_id: &str, redirect_uri: &str, scope: &str, state: &str, pkce: &Pkce) -> String` — adds `response_type=code`, `client_id`, `redirect_uri`, `scope`, `state`, `code_challenge`, `code_challenge_method=S256`.
  - `maxfetch::oauth::percent_encode(value: &str) -> String`, `percent_decode(value: &str) -> String`, `form_body(pairs: &[(&str, &str)]) -> String`.

- [ ] **Step 1: Write the failing test**

Create `tests/oauth_loopback.rs`:
```rust
use std::time::Duration;

use maxfetch::oauth::{authorization_url, percent_decode, percent_encode, LoopbackServer, Pkce};

#[test]
fn pkce_challenge_is_the_sha256_of_the_verifier() {
    let pkce = Pkce::generate();
    assert_eq!(pkce.verifier.len(), 64);
    assert_eq!(pkce.challenge.len(), 43);
    assert_ne!(pkce.verifier, Pkce::generate().verifier);
    assert!(pkce
        .verifier
        .chars()
        .all(|c| "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~".contains(c)));
}

#[test]
fn authorization_url_carries_every_pkce_parameter() {
    let pkce = Pkce { verifier: "v".repeat(64), challenge: "ch".into() };
    let url = authorization_url(
        "http://127.0.0.1:8080/authorize",
        "client id",
        "http://127.0.0.1:42424/callback",
        "playlist-read-private",
        "state123",
        &pkce,
    );
    assert!(url.starts_with("http://127.0.0.1:8080/authorize?"));
    assert!(url.contains("response_type=code"));
    assert!(url.contains("client_id=client%20id"));
    assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A42424%2Fcallback"));
    assert!(url.contains("scope=playlist-read-private"));
    assert!(url.contains("state=state123"));
    assert!(url.contains("code_challenge=ch"));
    assert!(url.contains("code_challenge_method=S256"));
}

#[test]
fn percent_encoding_round_trips() {
    assert_eq!(percent_encode("a b&c/d=e"), "a%20b%26c%2Fd%3De");
    assert_eq!(percent_encode("safe-._~"), "safe-._~");
    assert_eq!(percent_decode("a%20b%26c"), "a b&c");
}

#[tokio::test]
async fn the_server_returns_the_code_of_a_matching_state() {
    let mut server = LoopbackServer::bind(0).unwrap();
    server.expect_state("state123");
    let port = server.port();
    assert_eq!(server.redirect_uri(), format!("http://127.0.0.1:{port}/callback"));

    let url = format!("http://127.0.0.1:{port}/callback?code=THE_CODE&state=state123");
    let waiter = tokio::spawn(async move { server.wait_for_code(Duration::from_secs(5)).await });
    let resp = reqwest::get(&url).await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    assert!(resp.text().await.unwrap().contains("авторизация получена"));
    assert_eq!(waiter.await.unwrap().unwrap(), "THE_CODE");
}

#[tokio::test]
async fn a_mismatched_state_is_rejected() {
    let mut server = LoopbackServer::bind(0).unwrap();
    server.expect_state("state123");
    let port = server.port();
    let url = format!("http://127.0.0.1:{port}/callback?code=THE_CODE&state=other");
    let waiter = tokio::spawn(async move { server.wait_for_code(Duration::from_secs(5)).await });
    let resp = reqwest::get(&url).await.unwrap();
    assert_eq!(resp.status().as_u16(), 400);
    assert!(waiter.await.unwrap().is_err());
}

#[tokio::test]
async fn waiting_gives_up_after_the_timeout() {
    let server = LoopbackServer::bind(0).unwrap();
    let err = server.wait_for_code(Duration::from_millis(100)).await.unwrap_err();
    assert!(format!("{err}").contains("время ожидания"));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test oauth_loopback`
Expected: FAIL with "unresolved import `maxfetch::oauth`".

- [ ] **Step 3: Implement `src/oauth.rs`**

Add `cargo add sha2 base64`. `Pkce::generate` takes 48 bytes from `/dev/urandom` through `std::fs::File` (falling back to `SystemTime` nanos mixed with the process id when the read fails) and maps six bits per character onto the 64-symbol alphabet, giving 64 characters from 48 bytes; the challenge is `URL_SAFE_NO_PAD.encode(Sha256::digest(verifier))`.

`LoopbackServer` wraps a `tokio::net::TcpListener` on `127.0.0.1:<port>` (port 0 picks a free one, read back with `local_addr`). `wait_for_code` wraps the whole accept loop in `tokio::time::timeout`: read the request head up to `\r\n\r\n` (8 KiB cap), parse the request line, handle only `GET /callback` (404 otherwise), compare `state` with the expected value, reply `200` with `maxfetch: авторизация получена, можно закрыть вкладку` and return `code`, or reply `400` with `maxfetch: параметр state не совпадает` and keep waiting. On timeout: `anyhow::bail!("время ожидания кода авторизации истекло")`. Every reply carries `Content-Type: text/plain; charset=utf-8` and `Connection: close`.

`authorization_url` appends with `?` or `&` depending on the endpoint, then joins `form_body`'s pairs. `percent_encode` escapes everything outside `A-Z a-z 0-9 - . _ ~` as uppercase `%XX`; `percent_decode` is its inverse and leaves malformed sequences untouched.

- [ ] **Step 4: Declare the module and run the test**

Add `pub mod oauth;` to `src/lib.rs`.

Run: `cargo test --test oauth_loopback`
Expected: PASS, 6 tests.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(oauth): pkce and loopback redirect server"
```

---

### Task 4: Spotify token refresh and `maxfetch auth login spotify`

**Files:**
- Modify: `src/sources/spotify.rs`
- Modify: `src/auth.rs`
- Create: `tests/fixtures/spotify/token_refresh.json`
- Create: `tests/fixtures/spotify/token_revoked.json`
- Create: `tests/fixtures/spotify/token_exchange.json`
- Test: `tests/spotify_refresh.rs`
- Test: `tests/auth_spotify.rs`

**Interfaces:**
- Consumes: `maxfetch::oauth::{LoopbackServer, Pkce, authorization_url, form_body}`, `maxfetch::config::{config_dir, Credentials, Credentials::load_from, Credentials::save_to, SpotifyCredentials}`, `maxfetch::fetch::http_client`, the stage 1 `SpotifySource` (the `client_credentials` search and metadata paths).
- Produces:
  - `maxfetch::sources::spotify::SpotifyMode` — `enum { ClientCredentials, User }`, `Copy + Debug + Eq + PartialEq`.
  - `maxfetch::sources::spotify::TokenResponse` — `struct { access_token: String, expires_in: u32, refresh_token: Option<String> }`, `Deserialize`, `Debug`.
  - `maxfetch::sources::spotify::apply_refresh(base_url: &str, creds: &SpotifyCredentials, exchange: impl FnOnce(&str, &str) -> anyhow::Result<TokenResponse>) -> anyhow::Result<SpotifyCredentials>` — the pure refresh: keeps the rotated `refresh_token` when the response has one, keeps the old one otherwise.
  - `maxfetch::sources::spotify::REAUTH_MESSAGE` — `const : &str` = `"нужна повторная авторизация"`.
  - `SpotifySource::with_refresh_hook(creds: SpotifyCredentials, hook: Box<dyn Fn(&SpotifyCredentials) -> anyhow::Result<SpotifyCredentials> + Send + Sync>) -> SpotifySource`, `SpotifySource::with_credentials(creds: SpotifyCredentials) -> SpotifySource` (the stage 1 constructor, kept unchanged and used by the tests), `SpotifySource::mode(&self) -> SpotifyMode`, `async fn refresh_user_token(&self) -> anyhow::Result<SpotifyMode>`, `SpotifySource::reauth_hint(&self) -> Option<String>`.
  - `maxfetch::auth::PersistedCredentials` — `struct { inner: std::sync::Arc<tokio::sync::RwLock<Credentials>>, path: PathBuf }` with `load() -> anyhow::Result<PersistedCredentials>`, `async fn get(&self) -> Credentials`, `async fn update(&self, f: impl FnOnce(&mut Credentials) + Send) -> anyhow::Result<()>`.
  - `maxfetch::auth::exchange_spotify_code(token_endpoint: &str, client_id: &str, code: &str, redirect_uri: &str, verifier: &str) -> anyhow::Result<TokenResponse>`.
  - `maxfetch::auth::last_login_url() -> Option<String>` — the URL the current `login` is waiting on, readable by an operator and by a test.
  - `maxfetch::auth::login_with(source: &str, creds: &mut Credentials, options: LoginOptions) -> anyhow::Result<()>` — the testable entry point; `LoginOptions` holds the fields a test must override: `dir: PathBuf` (where `credentials.toml` is written), `token_endpoint: Option<String>` (the Spotify token endpoint, `None` means `SPOTIFY_TOKEN_ENDPOINT`), `auth_endpoint: Option<String>` and `port_range: std::ops::RangeInclusive<u16>` (42424..42427 by default). The public `login(source, creds)` is `login_with(source, creds, LoginOptions::default())`.
  - **No global test state.** The token endpoint is a per-call field of `LoginOptions`, not a process-wide override; there is no `override_token_endpoint` function and no environment variable read by `login`. The credentials path is likewise a field (`dir`), so a test uses a `tempfile::TempDir` and `Credentials::save_to(dir)` / `Credentials::load_from(dir)` explicitly and never touches `XDG_CONFIG_HOME`. Two `login` tests can therefore run in parallel.
  - `maxfetch::auth::{SPOTIFY_AUTH_ENDPOINT, SPOTIFY_TOKEN_ENDPOINT, SPOTIFY_SCOPE, WAIT_MESSAGE}` — `"https://accounts.spotify.com/authorize"`, `"https://accounts.spotify.com/api/token"`, `"playlist-read-private playlist-read-collaborative"`, `"откройте ссылку в браузере и подтвердите доступ"`.
  - `auth::login("spotify", &mut Credentials)` replaces the core's bail branch: binds a `LoopbackServer` on the first free port of 42424..42427, announces the URL, waits 300 s, exchanges the code, stores `refresh_token` and saves at 0600.

- [ ] **Step 1: Write the failing tests**

Create `tests/fixtures/spotify/token_refresh.json`:

```json
{ "access_token": "new-access", "token_type": "Bearer", "expires_in": 3600, "scope": "playlist-read-private", "refresh_token": "rotated-refresh" }
```

Create `tests/fixtures/spotify/token_revoked.json`:

```json
{ "error": "invalid_grant", "error_description": "Refresh token revoked" }
```

Create `tests/fixtures/spotify/token_exchange.json`:

```json
{ "access_token": "access-after-code", "token_type": "Bearer", "expires_in": 3600, "scope": "playlist-read-private playlist-read-collaborative", "refresh_token": "refresh-after-code" }
```

Create `tests/spotify_refresh.rs`:
```rust
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use maxfetch::config::SpotifyCredentials;
use maxfetch::sources::spotify::{
    apply_refresh, SpotifyMode, SpotifySource, TokenResponse, REAUTH_MESSAGE,
};

fn fixture(rel: &str) -> String {
    std::fs::read_to_string(format!("{}/tests/fixtures/spotify/{}", env!("CARGO_MANIFEST_DIR"), rel)).unwrap()
}

fn creds() -> SpotifyCredentials {
    SpotifyCredentials {
        client_id: "id".into(),
        client_secret: "secret".into(),
        refresh_token: Some("old-refresh".into()),
    }
}

#[test]
fn a_rotated_refresh_token_is_kept() {
    let resp: TokenResponse = serde_json::from_str(&fixture("token_refresh.json")).unwrap();
    let updated = apply_refresh("https://accounts.spotify.com/api/token", &creds(), move |_url, _body| Ok(resp)).unwrap();
    assert_eq!(updated.refresh_token.as_deref(), Some("rotated-refresh"));
    assert_eq!(updated.client_id, "id");
}

#[test]
fn a_refresh_without_rotation_keeps_the_old_token() {
    let resp = TokenResponse { access_token: "a".into(), expires_in: 3600, refresh_token: None };
    let updated = apply_refresh("https://accounts.spotify.com/api/token", &creds(), move |_u, _b| Ok(resp)).unwrap();
    assert_eq!(updated.refresh_token.as_deref(), Some("old-refresh"));
}

#[test]
fn client_credentials_mode_needs_no_refresh_token() {
    let src = SpotifySource::with_credentials(SpotifyCredentials { client_id: "id".into(), client_secret: "s".into(), refresh_token: None });
    assert_eq!(src.mode(), SpotifyMode::ClientCredentials);
    assert!(src.reauth_hint().is_none());
}

#[tokio::test]
async fn a_revoked_token_falls_back_to_client_credentials() {
    let called = Arc::new(AtomicBool::new(false));
    let src = SpotifySource::with_refresh_hook(creds(), Box::new({
        let called = called.clone();
        move |_c: &SpotifyCredentials| {
            called.store(true, Ordering::SeqCst);
            anyhow::bail!("400 invalid_grant")
        }
    }));
    let mode = src.refresh_user_token().await.unwrap();
    assert!(called.load(Ordering::SeqCst));
    assert_eq!(mode, SpotifyMode::ClientCredentials);
    assert_eq!(src.reauth_hint().as_deref(), Some(REAUTH_MESSAGE));
    assert!(REAUTH_MESSAGE.contains("нужна повторная авторизация"));
}

#[tokio::test]
async fn a_successful_refresh_stays_in_user_mode() {
    let src = SpotifySource::with_refresh_hook(creds(), Box::new(|c: &SpotifyCredentials| {
        Ok(SpotifyCredentials { refresh_token: Some("rotated".into()), ..c.clone() })
    }));
    assert_eq!(src.refresh_user_token().await.unwrap(), SpotifyMode::User);
    assert!(src.reauth_hint().is_none());
}

#[tokio::test]
async fn a_transport_error_is_not_mistaken_for_a_revocation() {
    let src = SpotifySource::with_refresh_hook(creds(), Box::new(|_c: &SpotifyCredentials| {
        anyhow::bail!("connection refused")
    }));
    assert!(src.refresh_user_token().await.is_err());
    assert!(src.reauth_hint().is_none());
}
```

Create `tests/auth_spotify.rs`:
```rust
use std::io::Read;

use maxfetch::auth::{exchange_spotify_code, last_login_url, login_with, LoginOptions};
use maxfetch::config::{Credentials, SpotifyCredentials};

#[tokio::test]
async fn a_code_is_exchanged_against_a_local_mock() {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let port = server.server_addr().to_ip().unwrap().port();
    let handle = std::thread::spawn(move || {
        let mut req = server.recv().unwrap();
        let mut body = String::new();
        req.as_reader().read_to_string(&mut body).unwrap();
        assert!(body.contains("grant_type=authorization_code"));
        assert!(body.contains("code=THE_CODE"));
        assert!(body.contains("code_verifier=VERIFIER"));
        let fixture = std::fs::read_to_string(format!("{}/tests/fixtures/spotify/token_exchange.json", env!("CARGO_MANIFEST_DIR"))).unwrap();
        req.respond(tiny_http::Response::from_string(fixture)).unwrap();
    });

    let endpoint = format!("http://127.0.0.1:{port}/api/token");
    let resp = exchange_spotify_code(&endpoint, "client-id", "THE_CODE", "http://127.0.0.1:42424/callback", "VERIFIER").await.unwrap();
    assert_eq!(resp.access_token, "access-after-code");
    assert_eq!(resp.refresh_token.as_deref(), Some("refresh-after-code"));
    handle.join().unwrap();
}

#[tokio::test]
async fn login_spotify_persists_the_refresh_token_with_mode_0600() {
    let token_server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let token_port = token_server.server_addr().to_ip().unwrap().port();
    let token_handle = std::thread::spawn(move || {
        let mut req = token_server.recv().unwrap();
        let mut body = String::new();
        req.as_reader().read_to_string(&mut body).unwrap();
        assert!(body.contains("grant_type=authorization_code"));
        let fixture = std::fs::read_to_string(format!("{}/tests/fixtures/spotify/token_exchange.json", env!("CARGO_MANIFEST_DIR"))).unwrap();
        req.respond(tiny_http::Response::from_string(fixture)).unwrap();
    });

    let dir = tempfile::tempdir().unwrap();
    let creds = Credentials {
        spotify: Some(SpotifyCredentials { client_id: "id".into(), client_secret: "secret".into(), refresh_token: None }),
        ..Credentials::default()
    };

    let login_task = tokio::spawn({
        let dir = dir.path().to_path_buf();
        let token_endpoint = format!("http://127.0.0.1:{token_port}/api/token");
        async move {
            let mut creds = creds.clone();
            let opts = LoginOptions { dir, token_endpoint: Some(token_endpoint), auth_endpoint: None, ..LoginOptions::default() };
            login_with("spotify", &mut creds, opts).await.map(|_| creds)
        }
    });

    let url = loop {
        if let Some(url) = last_login_url() {
            break url;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    };
    let redirect = maxfetch::oauth::percent_decode(query_param(&url, "redirect_uri"));
    let state = query_param(&url, "state");
    assert!(url.contains("code_challenge_method=S256"));

    let resp = reqwest::get(&format!("{redirect}?code=THE_CODE&state={state}")).await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);

    let creds = login_task.await.unwrap().unwrap();
    let stored = creds.spotify.unwrap();
    assert!(stored.refresh_token.is_some());

    let path = dir.path().join("credentials.toml");
    let mode = std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&path).unwrap().permissions()) & 0o777;
    assert_eq!(mode, 0o600);
    let reloaded = Credentials::load_from(dir.path()).unwrap();
    assert!(reloaded.spotify.unwrap().refresh_token.is_some(), "the redirect token is on disk, not only in memory");

    token_handle.join().unwrap();
}

fn query_param(url: &str, name: &str) -> String {
    let query = url.split_once('?').expect("url has a query").1;
    for pair in query.split('&') {
        if let Some((key, value)) = pair.split_once('=') {
            if key == name {
                return value.to_string();
            }
        }
    }
    panic!("parameter {name} missing in {url}");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --test spotify_refresh`
Expected: FAIL with "no function or associated item named `with_refresh_hook` found for struct `SpotifySource`".

Run: `cargo test --test auth_spotify`
Expected: FAIL with "no function or item named `exchange_spotify_code` in `maxfetch::auth`".

- [ ] **Step 3: Implement the refresh, the cell and the login flow**

`apply_refresh` is pure. It builds `form_body(&[("grant_type", "refresh_token"), ("refresh_token", t), ("client_id", id), ("client_secret", secret)])`, calls `exchange(base_url, &body)`, and returns `SpotifyCredentials { refresh_token: resp.refresh_token.or(existing.refresh_token), ..existing.clone() }`. `base_url` exists so a test can point at a mock.

`SpotifySource::with_refresh_hook` stores the hook; the production constructor installs one that POSTs to `SPOTIFY_TOKEN_ENDPOINT` through `http_client` and parses `TokenResponse`, turning a non-2xx into `anyhow::bail!("{status} {error}")` so `invalid_grant` survives into the message. `refresh_user_token` runs exactly:

```text
if creds.refresh_token is None: return Mode::ClientCredentials
match hook(&creds):
    Ok(updated):
        persist `updated` through the persisted-credentials cell
        clear reauth_hint
        return Mode::User
    Err(e) if e contains "invalid_grant":
        set reauth_hint = Some(REAUTH_MESSAGE)
        return Mode::ClientCredentials
    Err(e): return Err(e)
```

A transport error therefore propagates and leaves the hint untouched, which is what the sixth test pins. `mode()` is `User` only when `refresh_token` is `Some` and `reauth_hint` is `None`.

Add `cargo add --dev tiny_http` if Task 2 has not. `PersistedCredentials::update` serialises writes through one `static CREDS_WRITE: tokio::sync::Mutex<()>`, mutates the `RwLock` copy and then calls `Credentials::save_to(&directory)`, so the file is always written at 0600. `PersistedCredentials::load` uses `config_dir()`.

`exchange_spotify_code` POSTs `form_body(&[("grant_type", "authorization_code"), ("code", code), ("redirect_uri", redirect_uri), ("client_id", client_id), ("code_verifier", verifier)])` with `Content-Type: application/x-www-form-urlencoded` and parses `TokenResponse`; a non-2xx becomes `anyhow::bail!("обмен кода не удался: {status}")`.

`login("spotify", creds)` is `login_with("spotify", creds, LoginOptions::default())`. `login_with` binds the first free port of the range in `options.port_range`, generates `Pkce::generate()` and a `state` the same way, calls `expect_state`, builds the URL against `options.auth_endpoint` (or `SPOTIFY_AUTH_ENDPOINT`), stores it in the `last_login_url` cell and emits `tracing::info!("{WAIT_MESSAGE}: {url}")`. It awaits `wait_for_code(Duration::from_secs(300))`, exchanges the code at `options.token_endpoint` (or `SPOTIFY_TOKEN_ENDPOINT`), writes `creds.spotify.refresh_token` and saves to `options.dir` at 0600. Clear the URL cell when the flow ends, whether it succeeded or not. Leave the `bandcamp`, `soundcloud` and `qobuz` branches at the core's bail message; Task 9 replaces them, and it is Task 9 that deletes the `stage1b_login_hint` hand-off (see Task 9, the stage 1b test must be updated there).

- [ ] **Step 4: Run the tests and the whole suite**

Run: `cargo test --test spotify_refresh`
Expected: PASS, 6 tests.

Run: `cargo test --test auth_spotify`
Expected: PASS, 2 tests.

Run: `cargo test --test auth`
Expected: PASS, but only after retargeting the core's `login_is_not_implemented_in_the_core_build` to a source that is still unimplemented (`login("tidal", ...)`), because `spotify` now works. Make exactly that edit in `tests/auth.rs`.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(spotify): refresh rotation, auth login and persisted credentials"
```

---

### Task 5: Spotify playlists with a user token

**Files:**
- Modify: `src/sources/spotify.rs`
- Modify: `src/pipeline.rs`
- Create: `tests/fixtures/spotify/playlist_items.json`
- Test: `tests/spotify_playlist.rs`

**Interfaces:**
- Consumes: `maxfetch::sources::{AuthState, Query, QueryItem, Source}`, `maxfetch::pipeline::Pipeline` (core Task 11: `async fn search(&self, q: &Query) -> anyhow::Result<Vec<ReleaseGroup>>`), Task 4's `SpotifyMode`, `reauth_hint` and `REAUTH_MESSAGE`.
- Produces:
  - `maxfetch::sources::spotify::PlaylistItem` — `struct { track_id: String, title: String, artist: String, album: Option<String>, duration_ms: Option<u64>, isrc: Option<String> }`, `Clone + Debug`.
  - `SpotifySource::parse_playlist_items(json: &str) -> anyhow::Result<Vec<PlaylistItem>>` (associated function, no server needed).
  - `SpotifySource::fetch_playlist(&self, playlist_id: &str) -> anyhow::Result<Vec<PlaylistItem>>` — `GET {api}/v1/playlists/{id}/items`, `limit=100&offset=`, tolerant of both the `items[].track` and `items[].item` wrappers.
  - `SpotifySource::playlist_releases(&self, playlist_id: &str) -> anyhow::Result<Vec<Release>>` — one `Release` per item with `source: Spotify`, `source_item_id: track_id`, `track_no: None`.
  - `SpotifySource::with_api_base(creds: SpotifyCredentials, base: String, access_token: &str) -> SpotifySource` — the test constructor.
  - **`auth_state()` in user mode (semantics fixed here).** `SpotifySource::auth_state()` returns, for Spotify: `Missing` when `client_id` or `client_secret` is empty (the source cannot even use `client_credentials`, so there is nothing to authorize); `Expired` when the client pair is present and a `refresh_token` exists but a refresh has failed with `invalid_grant` (the `reauth_hint` is set); `Authenticated`/`Ok` when there is a valid token. An empty `refresh_token` with a present client pair is `Missing` — user mode is not configured, but client mode still works, and the TUI reports exactly this state (`{"нет авторизации"}` because playlists need a user token). Test `a_playlist_without_a_user_token_reports_the_hint` pins `AuthState::Missing` for that case.
  - `Pipeline::search` gains one rule inside the existing `QueryItem::Playlist` expansion branch: a `QueryItem::Playlist { source: Spotify, .. }` is expanded through `SpotifySource::playlist_releases` only when `mode() == User`; in `ClientCredentials` mode the source contributes zero releases and its `reauth_hint()` reaches the user through `SourceStatus.detail` (Task 9), the same channel the core plan uses for a skipped source. No signature changes.

- [ ] **Step 1: Write the failing test**

Create `tests/fixtures/spotify/playlist_items.json` (one null, unavailable entry included):

```json
{ "items": [ { "track": { "id": "4cOdK2wGLETKBW3PvgPWqT", "name": "Get Lucky", "external_ids": { "isrc": "USQX91300888" }, "duration_ms": 369000, "artists": [ { "name": "Daft Punk" } ], "album": { "name": "Random Access Memories" } } }, { "track": null } ] }
```

Create `tests/spotify_playlist.rs`:
```rust
use std::time::Duration;

use maxfetch::config::SpotifyCredentials;
use maxfetch::sources::spotify::{PlaylistItem, SpotifyMode, SpotifySource};
use maxfetch::sources::{AuthState, Source};

fn fixture() -> String {
    std::fs::read_to_string(format!("{}/tests/fixtures/spotify/playlist_items.json", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn user_creds() -> SpotifyCredentials {
    SpotifyCredentials { client_id: "id".into(), client_secret: "s".into(), refresh_token: Some("r".into()) }
}

#[test]
fn null_entries_are_dropped_and_fields_are_mapped() {
    let items = SpotifySource::parse_playlist_items(&fixture()).unwrap();
    assert_eq!(items.len(), 1);
    let PlaylistItem { track_id, title, artist, album, duration_ms, isrc } = items[0].clone();
    assert_eq!(track_id, "4cOdK2wGLETKBW3PvgPWqT");
    assert_eq!(title, "Get Lucky");
    assert_eq!(artist, "Daft Punk");
    assert_eq!(album.as_deref(), Some("Random Access Memories"));
    assert_eq!(duration_ms, Some(369000));
    assert_eq!(isrc.as_deref(), Some("USQX91300888"));
}

#[tokio::test]
async fn a_playlist_reads_through_a_mock_api() {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let port = server.server_addr().to_ip().unwrap().port();
    let handle = std::thread::spawn(move || {
        let req = server.recv().unwrap();
        assert!(req.url().contains("/v1/playlists/37i9dQZF1DXcBWIGoYBM5M/items"));
        assert!(req.url().contains("limit=100"));
        req.respond(tiny_http::Response::from_string(fixture())).unwrap();
    });
    let src = SpotifySource::with_api_base(user_creds(), format!("http://127.0.0.1:{port}"), "access-token");
    assert_eq!(src.mode(), SpotifyMode::User);
    let releases = src.playlist_releases("37i9dQZF1DXcBWIGoYBM5M").await.unwrap();
    assert_eq!(releases.len(), 1);
    assert_eq!(releases[0].source_item_id, "4cOdK2wGLETKBW3PvgPWqT");
    assert_eq!(releases[0].isrc.as_deref(), Some("USQX91300888"));
    handle.join().unwrap();
}

#[tokio::test]
async fn a_playlist_without_a_user_token_reports_the_hint() {
    let src = SpotifySource::with_credentials(SpotifyCredentials { client_id: "id".into(), client_secret: "s".into(), refresh_token: None });
    assert_eq!(src.mode(), SpotifyMode::ClientCredentials);
    let err = src.fetch_playlist("any").await.unwrap_err();
    assert!(format!("{err}").contains("нужна повторная авторизация"));
    assert_eq!(src.auth_state(), AuthState::Missing);
}

#[tokio::test]
async fn an_unreachable_api_fails_fast_instead_of_hanging() {
    let src = SpotifySource::with_api_base(user_creds(), "http://127.0.0.1:1".into(), "access-token");
    let start = std::time::Instant::now();
    let err = src.fetch_playlist("any").await.unwrap_err();
    assert!(start.elapsed() < Duration::from_secs(5));
    assert!(format!("{err}").contains("Spotify"));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test spotify_playlist`
Expected: FAIL with "no function or associated item named `parse_playlist_items`".

- [ ] **Step 3: Implement the playlist path**

`parse_playlist_items` takes `items`, prefers `element["item"]` and falls back to `element["track"]`, skips `null` entries and entries without an `id`, reads `external_ids.isrc`, `duration_ms`, `artists[0].name` (with `"Unknown"` for an empty array) and `album.name`.

`fetch_playlist` checks `mode()` first: in `ClientCredentials` it bails `"{REAUTH_MESSAGE}: плейлисты Spotify требуют авторизации"` without a request. In `User` it pages through `limit=100&offset={n}` until a page comes back shorter than 100 or `next` is null, capped at 100 pages so a broken `next` cannot loop. A 401 during the read calls `refresh_user_token` once and retries that page once; a second 401 becomes the `REAUTH_MESSAGE` error with the `Spotify` prefix in the message. Any other transport error is prefixed with `Spotify:` so the test can assert it, and the `http_client` timeout is what stops it hanging. Treat a response that carries `items` but is not valid JSON the same way: prefix the parse error with `Spotify:` rather than inventing entries.

`with_api_base` stores the base URL and a pre-issued access token; production uses `https://api.spotify.com`.

`auth_state()` follows the user-mode semantics from Interfaces: `Missing` when `client_id` or `client_secret` is empty, `Expired` when a `refresh_token` exists but the last refresh failed with `invalid_grant`, `Ok` when a valid token exists, and `Missing` again for an empty `refresh_token` with a present client pair. The TUI line for that last case is `нет авторизации`, which is honest: search and metadata work, playlists do not.

The `Pipeline::search` rule lives only in the `QueryItem::Playlist` expansion branch: for `Spotify`, call `playlist_releases` when `mode() == User`, otherwise contribute zero releases for that item and leave the hint to `SourceStatus.detail`. Do not add a log entry and do not change `Pipeline`'s signature; the core plan's search returns only `Vec<ReleaseGroup>`.

- [ ] **Step 4: Run the test and the whole suite**

Run: `cargo test --test spotify_playlist`
Expected: PASS, 4 tests.

Run: `cargo test --test pipeline`
Expected: PASS, unchanged count. The playlist rule is additive and the core pipeline tests never ask for a Spotify playlist; nothing in `tests/pipeline.rs` changes.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(spotify): playlist reads with a user token"
```

---

### Task 6: Bandcamp `identity` cookie, `fan_id` and the purchase lookup

**Files:**
- Modify: `src/sources/bandcamp.rs`
- Create: `tests/fixtures/bandcamp/album_collect.html`
- Create: `tests/fixtures/bandcamp/search_items.json`
- Create: `tests/fixtures/bandcamp/empty_collection.json`
- Test: `tests/bandcamp_purchased.rs`

**Interfaces:**
- Consumes: `maxfetch::config::BandcampCredentials { identity_cookie }`, `maxfetch::config::Config`, `std::path::Path`, `maxfetch::fetch::http_client`, the stage 1b `BandcampSource` and its HTML helpers.
- Produces:
  - **`BandcampSource` constructor set (canonical; Task 7 reuses it and stage 1b Task 5 must match it).** `BandcampSource::new(config: &Config, credentials: &Credentials) -> BandcampSource` (production: `credentials.bandcamp` feeds the cookie, `config` feeds the API base); `BandcampSource::with_api_base(api_base: &str, cache_root: &Path) -> BandcampSource` (stage 1b test constructor, no credentials); `BandcampSource::with_lookups(creds: BandcampCredentials, hook: Box<dyn Fn(u64, &str) -> anyhow::Result<String> + Send + Sync>) -> BandcampSource` (Task 6 test constructor for the purchase lookup); `BandcampSource::without_credentials() -> BandcampSource` (no cookie, purchase state `NotNeeded`). No other constructor is introduced. If stage 1b Task 5 ends up with a different set (for example `with_endpoints`), it must be brought to this list by a separate edit to the stage 1b plan before Task 7 relies on it.
  - `maxfetch::sources::bandcamp::fan_id_from_html(html: &str) -> Option<u64>` — reads the `data-tralbum-collect-info` block and returns its `fan_id`.
  - `maxfetch::sources::bandcamp::RedownloadEntry` — `struct { sale_item_type: String, sale_item_id: u64, album_title: String, redownload_url: String }`, `Clone + Debug + PartialEq`.
  - `maxfetch::sources::bandcamp::parse_search_items(json: &str) -> anyhow::Result<Vec<RedownloadEntry>>` — keys `redownload_urls` by `sale_item_type + sale_item_id` and joins them with `collection_items[]` for the title.
  - `maxfetch::sources::bandcamp::NO_PURCHASES_MESSAGE` — `const : &str` = `"нет доступа к покупкам"`.
  - `BandcampSource::lookup_purchases(&self, fan_id: u64, album_title: &str) -> anyhow::Result<Vec<RedownloadEntry>>` — `POST https://bandcamp.com/api/fancollection/1/search_items` with `{"fan_id":N,"search_key":"<album title>","search_type":"collection"}` and `Cookie: identity=<cookie>`; a 401/403 or an empty `redownload_urls` returns `Ok(vec![])` and marks the purchase state `Expired`.
  - `BandcampSource::with_lookups(creds: BandcampCredentials, hook: Box<dyn Fn(u64, &str) -> anyhow::Result<String> + Send + Sync>) -> BandcampSource`, `BandcampSource::without_credentials() -> BandcampSource`, `BandcampSource::purchase_state(&self) -> AuthState`.

- [ ] **Step 1: Write the failing test**

Create `tests/fixtures/bandcamp/album_collect.html`:

```html
<!doctype html><html><body>
<script type="text/javascript" data-tralbum-collect-info='{"fan_id":8453211,"item_id":345678901,"item_type":"a"}'></script>
<script type="text/javascript" data-tralbum="{}"></script>
</body></html>
```

Create `tests/fixtures/bandcamp/search_items.json`:

```json
{ "redownload_urls": { "a345678901": "https://bandcamp.com/download?enc=flac&fsig=abc&id=345678901&sitem_id=345678901" }, "collection_items": [ { "sale_item_type": "a", "sale_item_id": 345678901, "item_title": "Random Access Memories", "band_name": "Daft Punk" } ], "more_available": false }
```

Create `tests/fixtures/bandcamp/empty_collection.json`:

```json
{ "redownload_urls": {}, "collection_items": [], "more_available": false }
```

Create `tests/bandcamp_purchased.rs`:
```rust
use maxfetch::config::BandcampCredentials;
use maxfetch::sources::bandcamp::{
    fan_id_from_html, parse_search_items, BandcampSource, NO_PURCHASES_MESSAGE,
};
use maxfetch::sources::AuthState;

fn fixture(rel: &str) -> String {
    std::fs::read_to_string(format!("{}/tests/fixtures/bandcamp/{}", env!("CARGO_MANIFEST_DIR"), rel)).unwrap()
}

#[test]
fn the_fan_id_comes_from_the_collect_info_block() {
    assert_eq!(fan_id_from_html(&fixture("album_collect.html")), Some(8453211));
}

#[test]
fn a_page_without_the_block_has_no_fan_id() {
    assert_eq!(fan_id_from_html("<html><body>nothing</body></html>"), None);
}

#[test]
fn redownload_urls_are_keyed_by_type_and_id() {
    let entries = parse_search_items(&fixture("search_items.json")).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].sale_item_type, "a");
    assert_eq!(entries[0].sale_item_id, 345678901);
    assert_eq!(entries[0].album_title, "Random Access Memories");
    assert!(entries[0].redownload_url.contains("enc=flac"));
}

#[test]
fn an_empty_collection_parses_to_an_empty_list() {
    assert!(parse_search_items(&fixture("empty_collection.json")).unwrap().is_empty());
}

#[tokio::test]
async fn an_expired_cookie_yields_no_purchases_instead_of_a_crash() {
    let src = BandcampSource::with_lookups(
        BandcampCredentials { identity_cookie: "expired".into() },
        Box::new(|_fan, _title| Ok(fixture("empty_collection.json"))),
    );
    let entries = src.lookup_purchases(8453211, "Random Access Memories").await.unwrap();
    assert!(entries.is_empty());
    assert_eq!(src.purchase_state(), AuthState::Expired);
    assert_eq!(NO_PURCHASES_MESSAGE, "нет доступа к покупкам");
}

#[tokio::test]
async fn a_successful_lookup_marks_purchases_as_ok() {
    let src = BandcampSource::with_lookups(
        BandcampCredentials { identity_cookie: "good".into() },
        Box::new(|_fan, _title| Ok(fixture("search_items.json"))),
    );
    let entries = src.lookup_purchases(8453211, "Random Access Memories").await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(src.purchase_state(), AuthState::Ok);
}

#[test]
fn no_bandcamp_section_means_purchases_are_not_needed() {
    assert_eq!(BandcampSource::without_credentials().purchase_state(), AuthState::NotNeeded);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test bandcamp_purchased`
Expected: FAIL with "no function or associated item named `fan_id_from_html` found for module `maxfetch::sources::bandcamp`".

- [ ] **Step 3: Implement the purchase lookup**

`fan_id_from_html` finds the script element whose attribute list contains `data-tralbum-collect-info`, extracts the attribute value for both the single-quoted and the double-quoted spelling (Bandcamp emits single quotes), parses it as JSON and returns `fan_id` as `u64`.

`parse_search_items` walks `redownload_urls` as an object, splits each key at the type/digit boundary (`"a345678901"` → `("a", 345678901)`), looks the pair up in `collection_items[]` for `item_title` and keeps entries with no match using an empty `album_title`. Output order follows the object order.

`lookup_purchases` POSTs the JSON body with `Cookie: identity=<cookie>`, `Content-Type: application/json` and the default `http_client` timeout. On a non-2xx status or an empty parsed list it sets the purchase state to `Expired`, logs `NO_PURCHASES_MESSAGE` at `tracing::warn!` and returns `Ok(vec![])`; on at least one entry it sets `Ok`. It never returns an error, so an expired cookie cannot abort a run. `with_lookups` injects the transport; `without_credentials` leaves `creds: None` and the state at `NotNeeded`.

- [ ] **Step 4: Run the test**

Run: `cargo test --test bandcamp_purchased`
Expected: PASS, 7 tests.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(bandcamp): identity cookie and purchase lookup"
```

---

### Task 7: Bandcamp redownload formats, `pagedata` and the album ZIP

**Files:**
- Modify: `src/sources/bandcamp.rs`
- Create: `tests/fixtures/bandcamp/download_page.html`
- Test: `tests/bandcamp_redownload.rs`

**Interfaces:**
- Consumes: Task 6's `RedownloadEntry`, `lookup_purchases`, `parse_search_items`, `BandcampSource::{new, with_api_base, without_credentials}`; `maxfetch::model::{Candidate, Container, Locator, Quality, Release, SourceId}`; `maxfetch::match::norm`; `maxfetch::fetch::{download_to_file, http_client, ProgressSink}`; `maxfetch::probe::probe`.
- Produces:
  - `maxfetch::sources::bandcamp::BandcampFormat` — `enum { Flac, Wav, Aiff, Alac, Mp3V0, Mp3_320, AacHi, Vorbis }`, `Copy + Debug + Eq + PartialEq`; `all() -> &'static [BandcampFormat]` in preference order `Flac, Wav, Aiff, Alac, Mp3V0, Mp3_320, AacHi, Vorbis`; `encoding_key(&self) -> &'static str` → `"flac"`, `"wav"`, `"aiff"`, `"alac"`, `"mp3-v0"`, `"mp3-320"`, `"aac-hi"`, `"vorbis"`.
  - `maxfetch::sources::bandcamp::DownloadFormat` — `struct { key: String, url: String, size_mb: Option<f64>, encoding_name: Option<String> }`, `Clone + Debug + PartialEq`.
  - `maxfetch::sources::bandcamp::parse_download_page(html: &str) -> anyhow::Result<Vec<DownloadFormat>>` — reads `pagedata` → `digital_items[0].downloads` and returns one entry per present, non-empty key in ladder order.
  - `maxfetch::sources::bandcamp::quality_of_format(format: BandcampFormat, encoding_name: Option<&str>) -> Quality` — the four lossless variants are lossless with the container of their variant and the depth/rate parsed from `encoding_name` when present. **Canonical lossy table (the single table for both this task and stage 1b Task 3; §8.2 gives no bitrate for the first three, so none is invented):** `Mp3V0` → lossy `Mp3`, `bitrate_kbps: None`; `Mp3_320` → lossy `Mp3`, `Some(320)`; `AacHi` → lossy `M4aAac`, `bitrate_kbps: None`; `Vorbis` → lossy `Container::Opus`, `bitrate_kbps: None`. `sample_rate` is `None` for every lossy variant.
  - `BandcampSource::download_candidates(&self, entry: &RedownloadEntry, page_html: &str) -> anyhow::Result<Vec<Candidate>>` — one candidate per lossless format when any exists, otherwise one per lossy format, each `Locator::Bandcamp { url, encoding: key }`.
  - `BandcampSource::unpack_album_zip(zip_path: &Path, dest_dir: &Path) -> anyhow::Result<Vec<PathBuf>>` — synchronous, testable without the network.
  - `BandcampSource::download_zip(&self, candidate: &Candidate, dest_dir: &Path, sink: &dyn ProgressSink) -> anyhow::Result<Vec<PathBuf>>` — downloads to a temp file, then `unpack_album_zip`.
  - **`impl Source for BandcampSource::download(&self, candidate: &Candidate, dest: &Path, sink: &dyn ProgressSink) -> anyhow::Result<()>` — the trait path for a purchased album.** It unpacks through `download_zip` into a scratch directory beside `dest` (a `tempfile` dir under `dest`'s parent), maps the members to the release's tracklist with `match::norm` (the algorithm below), and then hands the matching member to the pipeline's normal per-track flow so it gets `probe`d, tagged and moved exactly like a catalogue download. The single-track case (a `Locator::Bandcamp` without a fragment that resolves to one file) copies that file to `dest` and leaves probing to the pipeline. Signature is the core trait's; nothing new is added to the trait.
  - **Member-to-track matching (used by `download` and by `download_zip`'s caller).** The release carried by the candidate is the redownload entry's release; its tracklist order gives `track_no`. For each unpacked audio member in ZIP order: take the file stem, strip a leading `^\d{1,3}[\s._-]+` track-number prefix, and compare `match::norm(stem_without_prefix)` with `match::norm(track.title)` from the release's tracklist; the first track whose normalized title matches wins and the member receives that track's `track_no`. A member that matches no title is logged at `tracing::warn!` (`Bandcamp: файл «{name}» не сопоставлен с треком`) and skipped; a track that receives no member is simply left out of this download (partial success is normal, §7.5). Never guess a `track_no` from the file name alone when the title does not match.
  - **Probe gate (§7.5).** A purchased file is probed before it is written to the database: the measured quality is what the DB and the tags store, and a file whose measured quality is worse than the candidate claimed (for example a `flac` download that measures 16/44.1 when 24/96 was advertised) is rejected, deleted and reported, never stored with the claim. This is the same rejection rule the pipeline applies to every other source; the test below pins it on the Bandcamp path.
  - `maxfetch::sources::bandcamp::match_album_members(members: &[&str], track_titles: &[&str]) -> Vec<(String, u32)>` — the pure matching helper (see the algorithm above): `(member_name, track_no)` for each member that matched a title, in member order, unmatched members omitted.
  - `maxfetch::sources::bandcamp::verify_purchased_quality(path: &Path, claimed: &Quality) -> anyhow::Result<Option<Quality>>` — probes `path`; `Ok(None)` when the measured quality is strictly worse than `claimed` (the file is deleted first), `Ok(Some(measured))` otherwise.

- [ ] **Step 1: Write the failing test**

Create `tests/fixtures/bandcamp/download_page.html`:

```html
<!doctype html><html><body>
<div id="pagedata" data-blob='{"digital_items":[{"item_id":345678901,"downloads":{"flac":{"url":"https://p4.bcbits.com/download/album/345678901?enc=flac&fsig=abc","size_mb":412.5,"encoding_name":"FLAC 24bit/44.1kHz"},"mp3-v0":{"url":"https://p4.bcbits.com/download/album/345678901?enc=mp3-v0&fsig=abc","size_mb":128.0,"encoding_name":"MP3 V0"},"mp3-320":{"url":"","size_mb":0,"encoding_name":"MP3 320"}}}]}'>
</div>
</body></html>
```

Create `tests/bandcamp_redownload.rs`:
```rust
use std::io::Write;

use maxfetch::fetch::NoopSink;
use maxfetch::model::{Candidate, Container, Locator, Quality, Release, SourceId};
use maxfetch::quality::rank_candidates;
use maxfetch::sources::Source;
use maxfetch::sources::bandcamp::{
    match_album_members, parse_download_page, quality_of_format, verify_purchased_quality,
    BandcampFormat, BandcampSource, RedownloadEntry,
};

fn fixture() -> String {
    std::fs::read_to_string(format!("{}/tests/fixtures/bandcamp/download_page.html", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn entry() -> RedownloadEntry {
    RedownloadEntry {
        sale_item_type: "a".into(),
        sale_item_id: 345678901,
        album_title: "Random Access Memories".into(),
        redownload_url: "https://bandcamp.com/download?id=345678901".into(),
    }
}

#[test]
fn the_download_page_lists_available_formats() {
    let formats = parse_download_page(&fixture()).unwrap();
    assert_eq!(formats.len(), 2);
    assert_eq!(formats[0].key, "flac");
    assert!(formats[0].url.contains("enc=flac"));
    assert_eq!(formats[0].size_mb, Some(412.5));
    assert_eq!(formats[0].encoding_name.as_deref(), Some("FLAC 24bit/44.1kHz"));
    assert_eq!(formats[1].key, "mp3-v0");
}

#[test]
fn formats_with_an_empty_url_are_dropped() {
    let keys: Vec<String> = parse_download_page(&fixture()).unwrap().into_iter().map(|f| f.key).collect();
    assert!(!keys.contains(&"mp3-320".to_string()));
}

#[test]
fn flac_24bit_is_lossless_with_the_declared_depth() {
    let q = quality_of_format(BandcampFormat::Flac, Some("FLAC 24bit/44.1kHz"));
    assert!(q.lossless);
    assert_eq!(q.container, Container::Flac);
    assert_eq!(q.bit_depth, Some(24));
    assert_eq!(q.sample_rate, Some(44100));
    assert_eq!(q.bitrate_kbps, None);
}

#[test]
fn an_undeclared_depth_is_not_invented() {
    let q = quality_of_format(BandcampFormat::Flac, Some("FLAC"));
    assert!(q.lossless);
    assert_eq!((q.bit_depth, q.sample_rate), (None, None));
}

#[test]
fn lossy_variants_map_to_their_bitrates() {
    assert_eq!(quality_of_format(BandcampFormat::Mp3V0, Some("MP3 V0")).bitrate_kbps, None);
    assert_eq!(quality_of_format(BandcampFormat::Mp3_320, None).bitrate_kbps, Some(320));
    assert_eq!(quality_of_format(BandcampFormat::AacHi, None).bitrate_kbps, None);
    assert!(!quality_of_format(BandcampFormat::Vorbis, None).lossless);
    assert_eq!(quality_of_format(BandcampFormat::Vorbis, None).bitrate_kbps, None);
    assert_eq!(BandcampFormat::all()[0], BandcampFormat::Flac);
}

#[test]
fn candidates_prefer_lossless_and_keep_the_buy_url() {
    let src = BandcampSource::without_credentials();
    let cands = src.download_candidates(&entry(), &fixture()).unwrap();
    assert_eq!(cands.len(), 1);
    assert!(cands[0].quality.lossless);
    assert_eq!(cands[0].source, SourceId::Bandcamp);
    assert_eq!(cands[0].release.source_item_id, "a345678901");
    assert_eq!(cands[0].size_bytes, Some((412.5 * 1024.0 * 1024.0) as u64));
    match &cands[0].locator {
        Locator::Bandcamp { url, encoding } => {
            assert!(url.contains("enc=flac"));
            assert_eq!(encoding, "flac");
        }
        other => panic!("unexpected locator {other:?}"),
    }
}

#[test]
fn an_album_zip_is_unpacked_into_per_track_files() {
    let dir = tempfile::tempdir().unwrap();
    let zip_path = dir.path().join("album.zip");
    {
        let file = std::fs::File::create(&zip_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default();
        zip.start_file("01 Get Lucky.flac", opts).unwrap();
        zip.write_all(b"flac-bytes").unwrap();
        zip.start_file("cover.jpg", opts).unwrap();
        zip.write_all(b"jpeg-bytes").unwrap();
        zip.start_file("../escape.flac", opts).unwrap();
        zip.write_all(b"nope").unwrap();
        zip.start_file("02 Instant Crush.flac", opts).unwrap();
        zip.write_all(b"flac-bytes-2").unwrap();
        zip.finish().unwrap();
    }
    let out = dir.path().join("out");
    std::fs::create_dir_all(&out).unwrap();
    let written = BandcampSource::without_credentials().unpack_album_zip(&zip_path, &out).unwrap();
    assert_eq!(written.len(), 2);
    assert_eq!(written[0].file_name().unwrap(), "01 Get Lucky.flac");
    assert_eq!(written[1].file_name().unwrap(), "02 Instant Crush.flac");
    assert!(!out.join("cover.jpg").exists());
    assert!(!dir.path().join("escape.flac").exists());
}

#[tokio::test]
async fn a_non_zip_body_is_a_clear_error() {
    let dir = tempfile::tempdir().unwrap();
    let cand = Candidate {
        source: SourceId::Bandcamp,
        release: Release {
            title: "Random Access Memories".into(), artist: "Daft Punk".into(), album: None, track_no: None,
            duration_sec: None, isrc: None, year: None, source: SourceId::Bandcamp, source_item_id: "a345678901".into(),
        },
        quality: Quality { lossless: true, container: Container::Flac, bit_depth: Some(24), sample_rate: Some(44100), bitrate_kbps: None },
        size_bytes: None,
        locator: Locator::Bandcamp { url: "http://127.0.0.1:1/x.zip".into(), encoding: "flac".into() },
    };
    let err = BandcampSource::without_credentials().download_zip(&cand, dir.path(), &NoopSink).await.unwrap_err();
    assert!(format!("{err}").contains("ZIP"), "{err}");
    assert!(!dir.path().join("x.zip").exists(), "a failed download leaves no partial archive");
}

#[test]
fn members_are_matched_to_the_tracklist_by_normalized_title() {
    let out = match_album_members(
        &["01 Get Lucky.flac", "Instant Crush.flac", "cover.jpg", "99 Bonus.flac"],
        &["Get Lucky", "Instant Crush"],
    );
    assert_eq!(out, vec![("01 Get Lucky.flac".to_string(), 1), ("Instant Crush.flac".to_string(), 2)]);
}

#[test]
fn an_unmatched_member_is_skipped_not_guessed() {
    let out = match_album_members(&["04 Some Other Song.flac"], &["Get Lucky"]);
    assert!(out.is_empty(), "a file that matches no title gets no track number");
}

fn album_zip(members: &[(&str, &[u8])]) -> Vec<u8> {
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default();
        for (name, body) in members {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(body).unwrap();
        }
        zip.finish().unwrap();
    }
    buf.into_inner()
}

fn purchased_candidate(url: String, encoding: &str, depth: u8, rate: u32) -> Candidate {
    Candidate {
        source: SourceId::Bandcamp,
        release: Release {
            title: "Random Access Memories".into(), artist: "Daft Punk".into(), album: None, track_no: None,
            duration_sec: None, isrc: None, year: None, source: SourceId::Bandcamp, source_item_id: "a345678901".into(),
        },
        quality: Quality { lossless: true, container: Container::Flac, bit_depth: Some(depth), sample_rate: Some(rate), bitrate_kbps: None },
        size_bytes: None,
        locator: Locator::Bandcamp { url, encoding: encoding.into() },
    }
}

#[tokio::test]
async fn a_purchased_album_download_yields_one_file_per_matched_track() {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let port = server.server_addr().to_ip().unwrap().port();
    let zip_bytes = album_zip(&[("01 Get Lucky.flac", b"flac-1".as_slice()), ("02 Instant Crush.flac", b"flac-2".as_slice())]);
    let handle = std::thread::spawn(move || {
        let req = server.recv().unwrap();
        req.respond(tiny_http::Response::from_data(zip_bytes)).unwrap();
    });
    let cand = purchased_candidate(format!("http://127.0.0.1:{port}/album.zip"), "flac", 16, 44100);
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("download");
    std::fs::create_dir_all(&dest).unwrap();
    let src = BandcampSource::without_credentials();
    // The trait path (Source::download) unpacks and hands one file per matched track to the pipeline.
    src.download(&cand, &dest, &NoopSink).await.unwrap();
    let written = std::fs::read_dir(&dest).unwrap().count();
    assert_eq!(written, 2, "the trait path yields one file per matched track");
    handle.join().unwrap();
}

#[tokio::test]
async fn a_file_below_its_claimed_quality_is_rejected_and_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("track.flac");
    std::fs::copy(
        format!("{}/tests/fixtures/probe/sine_44100_s16.flac", env!("CARGO_MANIFEST_DIR")),
        &file,
    ).unwrap();
    // Claimed 24/96 but the fixture measures 16/44.1: the claim must not survive into the DB.
    let claimed = Quality { lossless: true, container: Container::Flac, bit_depth: Some(24), sample_rate: Some(96000), bitrate_kbps: None };
    let verdict = verify_purchased_quality(&file, &claimed).unwrap();
    assert!(verdict.is_none(), "the measured quality is strictly worse than the claim");
    assert!(!file.exists(), "a rejected file is deleted, never stored with the claim");

    let honest = Quality { lossless: true, container: Container::Flac, bit_depth: Some(16), sample_rate: Some(44100), bitrate_kbps: None };
    let file2 = dir.path().join("track2.flac");
    std::fs::copy(format!("{}/tests/fixtures/probe/sine_44100_s16.flac", env!("CARGO_MANIFEST_DIR")), &file2).unwrap();
    assert!(verify_purchased_quality(&file2, &honest).unwrap().is_some(), "an honest claim passes and is kept");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test bandcamp_redownload`
Expected: FAIL with "no function or associated item named `parse_download_page`".

- [ ] **Step 3: Implement the redownload path**

Add `cargo add zip` (a reader for the album archive the spec mandates; it is not transcoding). `parse_download_page` finds the `id="pagedata"` element's `data-blob` attribute, unescapes `&quot;`, `&#39;` and `&amp;`, parses the JSON, reads `digital_items[0].downloads`, and emits `BandcampFormat::all()` order, skipping a key that is absent, has an empty `url`, or has `size_mb == 0` with an empty `encoding_name`. `size_mb` becomes `size_bytes` as `(size_mb * 1024.0 * 1024.0) as u64`; `size_mb: None` becomes `size_bytes: None`.

`quality_of_format` matches the variant explicitly; do not route the keys through `Container::from_ext`, because `"mp3-v0"` and `"aac-hi"` are not extensions. Containers are `Flac`, `Wav`, `Aiff`, `Alac`, `Mp3`, `Mp3`, `M4aAac` and, for Vorbis, `Container::Opus` (the `Container` enum has no Vorbis variant and Ogg is the container; `Locator::Bandcamp.encoding` stays `"vorbis"`, so the extension can still be derived from the encoding key). Depth and rate come from the `(\d{1,2})\s*bit` and `(\d+(?:\.\d+)?)\s*kHz` regexes, and both stay `None` when the depth is not found. The lossy bitrates follow the canonical table in Interfaces and nothing else: `mp3-v0`, `aac-hi` and `vorbis` carry `bitrate_kbps: None` (§8.2 gives no number and the global constraint forbids inventing one); only `mp3-320` carries `Some(320)`. `mp3-128` is not a candidate at all (§8.2 lists it only as a fallback the project never uses).

`download_candidates` builds the release from the entry (`title` = `album_title`, artist empty, `source_item_id = format!("{}{}", sale_item_type, sale_item_id)`), keeps only lossless formats when at least one exists, otherwise keeps the lossy ones, and preserves `size_bytes`.

`unpack_album_zip` walks the archive in order, skips directories and any entry whose lowercase name does not end in `.flac`, `.wav`, `.aiff`, `.aif`, `.m4a`, `.mp3`, `.ogg` or `.opus`, refuses entries containing `..` or starting with `/` (skip, do not error), creates the parent directory inside `dest_dir`, and returns the written paths. `download_zip` downloads to a `tempfile::NamedTempFile` through `download_to_file`, calls `unpack_album_zip`, and maps an unpack failure into `anyhow::bail!("Bandcamp: полученный файл не является ZIP-архивом: {e}")`.

`match_album_members(members: &[&str], track_titles: &[&str]) -> Vec<(String, u32)>` is the module-level matching helper described in Interfaces: for each member name it strips the optional leading track-number prefix, compares `match::norm` of the stem with `match::norm` of each title in order, and returns `(member_name, track_no)` for the first match only. `verify_purchased_quality(path: &Path, claimed: &Quality) -> anyhow::Result<Option<Quality>>` runs `probe::probe(path)`; when the measured quality is strictly worse than `claimed` (using the core `compare_quality` under §7.4) it deletes `path` and returns `Ok(None)`; otherwise it returns `Ok(Some(measured))`. It never repairs a claim and never upgrades it.

`impl Source for BandcampSource::download` is the trait entry point for a purchased album: it calls `download_zip` into a scratch directory under `dest`'s parent, runs `match_album_members` against the release's tracklist to assign `track_no`, applies `verify_purchased_quality` to every matched member, and copies each surviving file into the pipeline's expected per-track location under `dest` (one file per matched track, named from the release), leaving tagging and the final move to the pipeline. A member that fails verification is dropped silently once its own `tracing::warn!` line is written; the call itself only returns an error when the archive cannot be fetched or unpacked at all. This closes the gap where a purchased album had no `Source::download` path at all.

- [ ] **Step 4: Run the test**

Run: `cargo test --test bandcamp_redownload`
Expected: PASS, 12 tests.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(bandcamp): redownload formats, pagedata and album zip unpacking"
```

---

### Task 8: SoundCloud OAuth original file via `yt-dlp`

**Files:**
- Modify: `src/sources/soundcloud.rs`
- Create: `tests/fixtures/soundcloud/track_lossless.json`
- Create: `tests/fixtures/soundcloud/ytdlp_original.json`
- Create: `tests/fixtures/soundcloud/fake_ytdlp.sh`
- Test: `tests/soundcloud_original.rs`

**Interfaces:**
- Consumes: the stage 1b `SoundCloudSource` (its own `api-v2` client, `client_id` scraping, `release_from_track` with `publisher_metadata.isrc`), `maxfetch::config::SoundCloudCredentials { client_id, oauth_token }`, `maxfetch::fetch::ProgressSink`, `maxfetch::probe::probe`, `maxfetch::quality::rank_candidates`.
- Produces:
  - **Track type reuse.** This task does not add `Deserialize` to the stage 1b `SoundCloudTrack` (stage 1b Task 7 defines it as `Clone + Debug` and adding a derive there would change another plan's type). Use the stage 1b `sources::soundcloud::parse_track(json: &str) -> anyhow::Result<SoundCloudTrack>` for every track fixture; the test below calls `parse_track` instead of `serde_json::from_str::<SoundCloudTrack>`.
  - **`SoundCloudFormat` — the locator needs to tell the original from the transcodes.** `maxfetch::sources::soundcloud::SoundCloudFormat` — `enum { Original, Aac160, Aac96, M3u8 }`, `Copy + Debug + Eq + PartialEq`. `Original` means the progressive original file, `Aac160`/`Aac96` the HLS AAC transcodes at those bitrates, `M3u8` any other HLS playlist. It is the only thing that makes `Locator::SoundCloud` distinguishable between the lossless original and the lossy AAC that share a `track_id`.
  - **`Locator` extension (stage 2 shape).** `maxfetch::model::Locator::SoundCloud { track_id: String, format: SoundCloudFormat }`. This replaces the stage 1b `Locator::SoundCloud { track_id: String }`; because stage 1b Task 8 owns the original shape, **stage 1b Task 8 must be brought to this two-field form by a separate edit to that plan**, and Task 9's stage 1b `parse_locator` test that constructs `Locator::SoundCloud { track_id: "1".into() }` must gain `format: SoundCloudFormat::Aac160`. Describe the required shape here so the edit is mechanical: `SoundCloud { track_id: String, format: SoundCloudFormat }`.
  - **Converter rename.** The stage 1b `SoundCloudTrack::to_release` is renamed to `release_from_track` (free function or `SoundCloudSource` method, called as `src.release_from_track(&track)` in the tests below). **Stage 1b Task 7 must be renamed to `release_from_track` by a separate edit to that plan**; this task does not define a second converter. Whatever the stage 1b edit lands on, this plan uses exactly `release_from_track(&track)` and nothing else.
  - `maxfetch::sources::soundcloud::OriginalDownload` — `struct { url: String, ext: String, format_id: String, bitrate_kbps: Option<u32>, sample_rate: Option<u32>, bit_depth: Option<u8>, lossless: bool }`, `Clone + Debug`.
  - `maxfetch::sources::soundcloud::parse_ytdlp_json(json: &str) -> anyhow::Result<OriginalDownload>` — reads `url`, `ext`, `format_id`, `abr`, `asr`, `audio_bitdepth` and derives `lossless` from `ext` plus `vcodec == "none"`. Lossless containers are `flac`, `wav`, `aiff`, `alac` only; `m4a` is **not** lossless (`Container::M4aAac.is_lossless() == false` in core), so an `m4a` original parses as `Container::M4aAac` with `lossless: false` and its `abr` kept.
  - `SoundCloudSource::original_download(&self, track_url: &str) -> anyhow::Result<OriginalDownload>` — runs `yt-dlp -J --no-playlist --username oauth --password <oauth_token> <track_url>`; without a token it bails before spawning.
  - `SoundCloudSource::original_candidate(&self, release: &Release, original: &OriginalDownload) -> Candidate` — lossless `Candidate` for a lossless original (depth/rate from the parse, `None` when absent), lossy `Candidate` with `bitrate_kbps` otherwise; `Locator::SoundCloud { track_id, format: SoundCloudFormat::Original }`.
  - `SoundCloudSource::aac_candidate(&self, release: &Release) -> Option<Candidate>` — the stage 1b `aac_160k` HLS transcode as a lossy `Candidate`, `bitrate_kbps: Some(160)`, `Container::M4aAac`, ISRC preserved; `Locator::SoundCloud { track_id, format: SoundCloudFormat::Aac160 }` (so it is distinguishable from the original sharing the same `track_id`).
  - `SoundCloudSource::download_original(&self, candidate: &Candidate, dest: &Path, sink: &dyn ProgressSink) -> anyhow::Result<()>` — `yt-dlp -f <format_id> -o <dest> --username oauth --password <token> <url>`, then `probe`, deleting `dest` when the probe fails.
  - `SoundCloudSource::with_ytdlp(creds: SoundCloudCredentials, yt_dlp_path: String) -> SoundCloudSource`, `SoundCloudSource::without_credentials() -> SoundCloudSource`.

- [ ] **Step 1: Write the failing test**

Create `tests/fixtures/soundcloud/track_lossless.json`:

```json
{ "id": 123456789, "title": "Ambient One", "duration": 214000, "publisher_metadata": { "isrc": "DEP960300042", "artist": "Test Artist" }, "user": { "id": 42, "username": "Test Artist", "permalink": "test-artist" }, "media": { "transcodings": [ { "preset": "aac_160k", "protocol": "hls", "mime_type": "audio/mp4; codecs=\"mp4a.40.2\"", "url": "https://api-v2.soundcloud.com/media/soundcloud:tracks:123456789/aac_160k" } ] } }
```

Create `tests/fixtures/soundcloud/ytdlp_original.json`:

```json
{ "id": "123456789", "title": "Ambient One", "url": "https://cf-media.sndcdn.com/original.flac", "ext": "flac", "vcodec": "none", "acodec": "flac", "abr": null, "asr": 44100, "audio_bitdepth": 24, "format_id": "original", "format_note": "Original" }
```

Create `tests/fixtures/soundcloud/fake_ytdlp.sh` (make it executable in the test, not in git):

```bash
#!/usr/bin/env bash
# Fake yt-dlp: `-J` prints the recorded original-file JSON; a download run copies the
# probe fixture to the path that follows `-o`.
for arg in "$@"; do
  if [ "$arg" = "-J" ]; then
    cat "$(dirname "$0")/ytdlp_original.json"
    exit 0
  fi
done
prev=""
out=""
for arg in "$@"; do
  if [ "$prev" = "-o" ]; then out="$arg"; fi
  prev="$arg"
done
if [ -z "$out" ]; then echo "fake yt-dlp: missing -o" >&2; exit 2; fi
cp "$(dirname "$0")/../probe/sine_44100_s16.flac" "$out"
echo "[download] 100% of 100KiB" >&2
```

Create `tests/soundcloud_original.rs`:
```rust
use std::os::unix::fs::PermissionsExt;

use maxfetch::config::SoundCloudCredentials;
use maxfetch::fetch::NoopSink;
use maxfetch::model::{Container, Locator, Quality, Release, SourceId};
use maxfetch::quality::rank_candidates;
use maxfetch::sources::soundcloud::{
    parse_track, parse_ytdlp_json, SoundCloudFormat, SoundCloudSource,
};

fn fixture(rel: &str) -> String {
    std::fs::read_to_string(format!("{}/tests/fixtures/soundcloud/{}", env!("CARGO_MANIFEST_DIR"), rel)).unwrap()
}

fn fake_ytdlp() -> String {
    let path = format!("{}/tests/fixtures/soundcloud/fake_ytdlp.sh", env!("CARGO_MANIFEST_DIR"));
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

fn release() -> Release {
    Release {
        title: "Ambient One".into(),
        artist: "Test Artist".into(),
        album: None,
        track_no: None,
        duration_sec: Some(214.0),
        isrc: Some("DEP960300042".into()),
        year: None,
        source: SourceId::SoundCloud,
        source_item_id: "123456789".into(),
    }
}

#[test]
fn the_original_json_is_parsed_as_lossless() {
    let orig = parse_ytdlp_json(&fixture("ytdlp_original.json")).unwrap();
    assert_eq!(orig.ext, "flac");
    assert_eq!(orig.format_id, "original");
    assert_eq!(orig.sample_rate, Some(44100));
    assert_eq!(orig.bit_depth, Some(24));
    assert_eq!(orig.bitrate_kbps, None);
    assert!(orig.lossless);
}

#[test]
fn a_lossy_original_json_is_not_lossless() {
    let json = r#"{ "url": "https://cf-media.sndcdn.com/x.mp3", "ext": "mp3", "vcodec": "none", "acodec": "mp3", "abr": 256, "asr": 44100, "format_id": "http_mp3" }"#;
    let orig = parse_ytdlp_json(json).unwrap();
    assert!(!orig.lossless);
    assert_eq!(orig.bitrate_kbps, Some(256));
    assert_eq!(orig.bit_depth, None);
}

#[test]
fn the_original_candidate_is_lossless_and_keeps_the_isrc() {
    let src = SoundCloudSource::without_credentials();
    let orig = parse_ytdlp_json(&fixture("ytdlp_original.json")).unwrap();
    let cand = src.original_candidate(&release(), &orig);
    assert!(cand.quality.lossless);
    assert_eq!(cand.quality.container, Container::Flac);
    assert_eq!(cand.quality.bit_depth, Some(24));
    assert_eq!(cand.release.isrc.as_deref(), Some("DEP960300042"));
    assert!(matches!(
        cand.locator,
        Locator::SoundCloud { ref track_id, format: SoundCloudFormat::Original } if track_id == "123456789"
    ));
}

#[test]
fn the_original_outranks_the_aac_transcode() {
    let src = SoundCloudSource::without_credentials();
    let orig = parse_ytdlp_json(&fixture("ytdlp_original.json")).unwrap();
    let lossless = src.original_candidate(&release(), &orig);
    let aac = src.aac_candidate(&release()).expect("the fixture carries an aac_160k transcode");
    // §7.4: a lossless candidate present means the ranked list holds only the original.
    let ranked = rank_candidates(&[aac.clone(), lossless]);
    assert_eq!(ranked.len(), 1, "lossy is dropped from the ranked list when a lossless alternative exists");
    assert!(ranked[0].quality.lossless);
    assert_eq!(ranked[0].quality.bit_depth, Some(24));
    // The AAC candidate still exists for explicit selection: it is the second element the list had, lossless=false.
    assert!(!aac.quality.lossless);
    assert!(matches!(
        aac.locator,
        Locator::SoundCloud { format: SoundCloudFormat::Aac160, .. }
    ));
}

#[tokio::test]
async fn the_original_needs_a_token() {
    let src = SoundCloudSource::without_credentials();
    let err = src.original_download("https://soundcloud.com/test-artist/ambient-one").await.unwrap_err();
    assert!(format!("{err}").contains("только с авторизацией"));
}

#[tokio::test]
async fn the_fake_ytdlp_yields_a_probed_lossless_file() {
    let src = SoundCloudSource::with_ytdlp(
        SoundCloudCredentials { client_id: "cid".into(), oauth_token: Some("tok".into()) },
        fake_ytdlp(),
    );
    let orig = src.original_download("https://soundcloud.com/test-artist/ambient-one").await.unwrap();
    let cand = src.original_candidate(&release(), &orig);
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("track.flac");
    src.download_original(&cand, &dest, &NoopSink).await.unwrap();
    let probed = maxfetch::probe::probe(&dest).unwrap();
    assert!(probed.container.is_lossless());
    assert_eq!(probed.sample_rate, Some(44100));
}

#[tokio::test]
async fn a_track_without_an_original_still_offers_aac() {
    let src = SoundCloudSource::without_credentials();
    let track = parse_track(&fixture("track_lossless.json")).unwrap();
    let rel = src.release_from_track(&track);
    let aac = src.aac_candidate(&rel).unwrap();
    assert_eq!(
        aac.quality,
        Quality { lossless: false, container: Container::M4aAac, bit_depth: None, sample_rate: None, bitrate_kbps: Some(160) }
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test soundcloud_original`
Expected: FAIL with "no function or associated item named `parse_ytdlp_json`".

- [ ] **Step 3: Implement the original-file path**

`parse_ytdlp_json` deserialises one `serde_json::Value` and reads `url`, `ext`, `format_id`, `abr`, `asr`, `audio_bitdepth`; `lossless` is true when `ext` is one of `flac`, `wav`, `aiff`, `alac` **and** `vcodec == "none"`, and in that case `bitrate_kbps` is forced to `None` even when `abr` is present. `m4a` is not in that list: a core `Container::M4aAac` is lossy, so an `m4a` original parses to `Container::M4aAac`, `lossless: false`, with its `abr` kept in `bitrate_kbps`.

`original_download` builds `Command::new(&self.yt_dlp_path)` with `-J --no-playlist --username oauth --password <token> <track_url>` and parses stdout. Without a token it bails `"оригинальный файл доступен только с авторизацией SoundCloud"` before spawning. A non-zero exit becomes `anyhow::bail!("yt-dlp не смог получить оригинал: {stderr}")`, passing through the provider line `Original download format is only available for registered users` verbatim, because that is the string a user will search for.

`original_candidate` maps `lossless` to a lossless `Quality` with `container` from `Container::from_ext(ext)`, `bit_depth` and `sample_rate` from the parse, `bitrate_kbps: None`; otherwise it sets `bitrate_kbps` from `abr` and leaves the depth `None`. The locator is `Locator::SoundCloud { track_id, format: SoundCloudFormat::Original }`, so the pipeline routes the download back through this source and the lossless original is distinguishable from the AAC that shares the same `track_id`.

`download_original` spawns `yt-dlp -f <recorded format_id> -o <dest> --username oauth --password <token> <url>`, then `probe::probe(dest)`; on a probe error it removes `dest` and returns the probe error, so no unverified file reaches the library.

`aac_candidate` builds the lossy `Candidate` from the stage 1b track object's `aac_160k` transcode with `bitrate_kbps: Some(160)` and `Locator::SoundCloud { track_id, format: SoundCloudFormat::Aac160 }`. The HLS download itself stays with the stage 1b path; this task does not touch it.

- [ ] **Step 4: Run the test and the whole suite**

Run: `cargo test --test soundcloud_original`
Expected: PASS, 7 tests.

Run: `cargo test --test soundcloud`
Expected: PASS, 7 tests. The stage 1b suite is untouched in behaviour. §7.4 is now stated explicitly (no open decision): lossy candidates are dropped from the ranked list whenever a lossless alternative exists, so the stage 1b `the_only_candidate_is_the_lossy_aac_stream` test keeps `ranked.len() == 1` for a track that has **no** original; a track that has both yields `ranked.len() == 1` too, holding only the original. The AAC candidate survives as a separate entry in the candidate list and in the database (`lossless = false`), but never in the auto-select ranked list. If a stage 1b test asserted that a both-stream track's ranked list had two entries, that assertion is wrong under §7.4 and is corrected to `ranked.len() == 1` with the original first.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(soundcloud): oauth original file ranked above the aac transcode"
```

---

### Task 9: Registration, the remaining `auth login` branches, statuses and the end-to-end run

**Files:**
- Modify: `src/sources/mod.rs`
- Modify: `src/pipeline.rs`
- Modify: `src/auth.rs`
- Modify: `src/tui/app.rs`
- Create: `tests/e2e_stage2.rs`
- Test: `tests/stage2_end_to_end.rs`

**Interfaces:**
- Consumes: `QobuzSource` (Task 2), `SpotifySource::{mode, reauth_hint, auth_state}` (Tasks 4-5), `BandcampSource::{purchase_state, lookup_purchases, search}` (Tasks 6-7 and stage 1b), `SoundCloudSource::{original_download, original_candidate, aac_candidate}` (Task 8), `maxfetch::sources::{AuthState, Capabilities, Query, Source, SourceStatus, registry, enabled_in}`, `maxfetch::pipeline::Pipeline` (`source_statuses()` from core Task 11), `maxfetch::tui::App`.
- Produces:
  - `maxfetch::sources::Source::status(&self) -> SourceStatus` — a defaulted trait method: `SourceStatus { id: self.id(), capabilities: self.capabilities(), auth: self.auth_state(), detail: None }`. Sources that have something to say (Qobuz, Bandcamp, SoundCloud, Spotify) override it in their own `impl Source` (never with an inherent method of the same name — see Task 2, item H).
  - `maxfetch::sources::source_statuses(sources: &[Box<dyn Source>]) -> Vec<SourceStatus>` — maps the slice through `status()` in registry order.
  - **`registry` returns every registered source, unfiltered (canonical core contract, core Task 10).** `registry(config, credentials)` returns one boxed source per *implemented* source id, without consulting `config.sources.enabled` and without consulting credentials. Filtering is `enabled_in(config: &Config, id: SourceId) -> bool` (core Task 10), and it is the caller — `Pipeline` and the TUI — that applies it. A source whose section is missing is still registered (its `auth_state()` is `Missing`) so the user can see why it does nothing; the registry never hides it. There is no third filtering variant, in particular no "credential presence" filter inside `registry`; the old draft that filtered on `credentials.qobuz.is_some()` is removed.
  - **Pipeline method name — `source_statuses()`, not `source_status()`.** Core Task 11 defines `Pipeline::source_statuses(&self) -> Vec<SourceStatus>`; the tests below call that exact method. Each element is the core `SourceStatus { id: SourceId, capabilities: Capabilities, auth: AuthState, detail: Option<String> }`, built by calling each registered-and-enabled source's `status()`.
  - **`detail` per source (Russian, §7.11).** `QobuzSource` keeps the Task 2 priority chain (`нет учётных данных` / `ключи протухли, обновлены` / the Studio ceiling reason). `SpotifySource` reports `нужна повторная авторизация` when `reauth_hint()` is set, and `нет авторизации: только поиск и метаданные` when `auth_state() == Missing` in user mode with a valid client pair. `BandcampSource` reports `нет доступа к покупкам` when `purchase_state() == Expired`, `покупки доступны` when `Ok`, and `нет авторизации` when the section is missing. `SoundCloudSource` reports `оригинал доступен` when `oauth_token` is set, `только AAC 160k: нет oauth_token` otherwise. Screen text is Russian; the four sentences are the canonical phrases the TUI and the tests use verbatim.
  - `auth::login` branches for `"bandcamp"` (stdin cookie), `"soundcloud"` (stdin token, empty allowed), `"qobuz"` (e-mail + password, scrape only if the stored pair is missing, verify, store). Each writes through `save_credentials`. **These branches replace, not extend, the stage 1b `stage1b_login_hint` hand-off**: for `bandcamp` the hint becomes «укажите cookie identity в credentials.toml», for `soundcloud` «укажите oauth_token в credentials.toml», and `qobuz` has «нужны учётные данные Qobuz». The stage 1b test that expected the old `stage1b_login_hint` text (`tests/auth_stage1b.rs`, lines around the `stage1b_login_hint("bandcamp")` / `("soundcloud")` assertions) must be updated to expect the new sentences; this edit is part of Task 9.
  - **End-to-end coverage (§10).** `tests/e2e_stage2.rs` under `#![cfg(feature = "e2e")]` runs a «поиск → fetch → library scan» scenario per stage 2 source against `tiny_http` mocks, so every authenticated path is exercised once end to end; run it with `cargo test --features e2e --test e2e_stage2`.
  - `maxfetch::tui::App::set_statuses(&mut self, statuses: Vec<SourceStatus>)` and `maxfetch::tui::App::status_lines(&self) -> Vec<String>` — one line per source; `App::render_status_line(&self, status: SourceStatus) -> String` maps a status to its line.

- [ ] **Step 1: Write the failing test**

Create `tests/stage2_end_to_end.rs`:
```rust
use async_trait::async_trait;
use maxfetch::config::{Config, Credentials};
use maxfetch::model::{Candidate, Release, SourceId};
use maxfetch::pipeline::Pipeline;
use maxfetch::sources::{
    enabled_in, registry, AuthState, Capabilities, Query, Source, SourceStatus,
};

struct StubSource {
    id: SourceId,
    auth: AuthState,
    caps: Capabilities,
    detail: Option<String>,
}

impl StubSource {
    fn new(id: SourceId, auth: AuthState, detail: &str) -> Self {
        Self {
            id,
            auth,
            caps: Capabilities { audio: true, metadata_only: false, lossless: true, needs_auth: true },
            detail: Some(detail.into()),
        }
    }
}

#[async_trait]
impl Source for StubSource {
    fn id(&self) -> SourceId { self.id }
    fn capabilities(&self) -> Capabilities { self.caps }
    fn auth_state(&self) -> AuthState { self.auth }
    async fn search(&self, _q: &Query) -> anyhow::Result<Vec<Release>> { Ok(vec![]) }
    async fn resolve(&self, _q: &Query) -> anyhow::Result<Vec<Release>> { Ok(vec![]) }
    async fn candidates(&self, _r: &Release) -> anyhow::Result<Vec<Candidate>> { Ok(vec![]) }
    async fn download(&self, _c: &Candidate, _d: &std::path::Path, _s: &dyn maxfetch::fetch::ProgressSink) -> anyhow::Result<()> { Ok(()) }
    fn status(&self) -> SourceStatus {
        SourceStatus { id: self.id, capabilities: self.caps, auth: self.auth, detail: self.detail.clone() }
    }
}

fn caps(audio: bool, metadata_only: bool, lossless: bool) -> Capabilities {
    Capabilities { audio, metadata_only, lossless, needs_auth: true }
}

#[test]
fn the_registry_holds_every_registered_source_without_filtering() {
    // Canonical contract: registry() is unfiltered; enabled_in() does the selecting.
    let cfg = Config::default();
    let creds = Credentials::default();
    let sources = registry(&cfg, &creds);
    let ids: Vec<SourceId> = sources.iter().map(|s| s.id()).collect();
    assert!(ids.contains(&SourceId::Qobuz), "qobuz is registered even without its credentials section");
    assert!(ids.contains(&SourceId::Bandcamp));
    assert!(ids.contains(&SourceId::SoundCloud));
    assert!(ids.contains(&SourceId::Spotify));
}

#[test]
fn enabled_in_selects_by_config_not_by_credentials() {
    let mut cfg = Config::default();
    cfg.sources.enabled = vec!["spotify".into()];
    assert!(enabled_in(&cfg, SourceId::Spotify));
    assert!(!enabled_in(&cfg, SourceId::Qobuz), "disabled in config");
    assert!(!enabled_in(&cfg, SourceId::Bandcamp));
}

#[test]
fn a_source_without_credentials_is_registered_but_reads_as_not_authorized() {
    let cfg = Config::default();
    let sources = registry(&cfg, &Credentials::default());
    let qobuz = sources.iter().find(|s| s.id() == SourceId::Qobuz).expect("qobuz is in the registry");
    assert_eq!(qobuz.auth_state(), AuthState::Missing);
    assert!(qobuz.status().detail.as_deref().unwrap_or_default().contains("нет учётных данных"));
}

#[test]
fn three_broken_sources_do_not_stop_the_pipeline_from_being_built() {
    let sources: Vec<Box<dyn Source>> = vec![
        Box::new(StubSource::new(SourceId::Qobuz, AuthState::Expired, "ключи протухли, обновлены")),
        Box::new(StubSource::new(SourceId::Bandcamp, AuthState::Expired, "нет доступа к покупкам")),
        Box::new(StubSource::new(SourceId::Spotify, AuthState::Expired, "нужна повторная авторизация")),
        Box::new(StubSource::new(SourceId::SoundCloud, AuthState::Ok, "оригинал доступен")),
    ];
    let pipeline = Pipeline::new(Config::default(), maxfetch::db::Db::open_in_memory().unwrap(), sources);
    let statuses = pipeline.source_statuses();
    assert_eq!(statuses.len(), 4);
    assert_eq!(statuses.iter().filter(|s| s.auth == AuthState::Expired).count(), 3);
    assert!(statuses.iter().any(|s| s.detail.as_deref().unwrap_or_default().contains("нет доступа к покупкам")));
    assert!(statuses.iter().any(|s| s.detail.as_deref().unwrap_or_default().contains("нужна повторная авторизация")));
}

#[tokio::test]
async fn a_revoked_spotify_token_drops_playlists_and_leaves_other_sources_ok() {
    // RF3: playlists vanish, the other sources are untouched.
    let spotify = StubSource::new(SourceId::Spotify, AuthState::Expired, "нужна повторная авторизация");
    let sources: Vec<Box<dyn Source>> = vec![
        Box::new(spotify),
        Box::new(StubSource::new(SourceId::SoundCloud, AuthState::Ok, "оригинал доступен")),
        Box::new(StubSource::new(SourceId::Bandcamp, AuthState::Ok, "покупки доступны")),
    ];
    let pipeline = Pipeline::new(Config::default(), maxfetch::db::Db::open_in_memory().unwrap(), sources);
    let statuses = pipeline.source_statuses();
    let spotify_status = statuses.iter().find(|s| s.id == SourceId::Spotify).unwrap();
    assert_eq!(spotify_status.auth, AuthState::Expired);
    assert_eq!(spotify_status.detail.as_deref(), Some("нужна повторная авторизация"));

    let q = Query { text: Some("daft punk".into()), url: None, items: vec![] };
    let groups = pipeline.search(&q).await.unwrap();
    assert!(
        groups.iter().all(|g| !g.sources.contains(&SourceId::Spotify)),
        "no Spotify playlist comes back from an expired token"
    );
    for other in [SourceId::SoundCloud, SourceId::Bandcamp] {
        let s = statuses.iter().find(|s| s.id == other).unwrap();
        assert_eq!(s.auth, AuthState::Ok, "{other:?} must stay untouched");
    }
}

#[tokio::test]
async fn an_expired_bandcamp_cookie_keeps_the_catalogue_working() {
    // RF4: the stage 1b catalogue and promo paths remain intact.
    let bandcamp = StubSource::new(SourceId::Bandcamp, AuthState::Expired, "нет доступа к покупкам");
    let sources: Vec<Box<dyn Source>> = vec![Box::new(bandcamp)];
    let pipeline = Pipeline::new(Config::default(), maxfetch::db::Db::open_in_memory().unwrap(), sources);
    let statuses = pipeline.source_statuses();
    let bandcamp_status = statuses.iter().find(|s| s.id == SourceId::Bandcamp).unwrap();
    assert_eq!(bandcamp_status.auth, AuthState::Expired);
    assert_eq!(bandcamp_status.detail.as_deref(), Some("нет доступа к покупкам"));

    let q = Query { text: Some("night drive".into()), url: None, items: vec![] };
    let groups = pipeline.search(&q).await.unwrap();
    assert!(!groups.is_empty(), "the bandcamp catalogue still produces releases");
    assert!(groups.iter().any(|g| g.sources.contains(&SourceId::Bandcamp)));
}

#[test]
fn status_lines_are_honest_about_each_limit() {
    let mut app = maxfetch::tui::App::new(Config::default());
    app.set_statuses(vec![
        SourceStatus { id: SourceId::Qobuz, capabilities: caps(true, false, true), auth: AuthState::Ok, detail: Some("нет подписки Studio, максимум FLAC-CD 16/44.1".into()) },
        SourceStatus { id: SourceId::Spotify, capabilities: caps(false, true, false), auth: AuthState::Expired, detail: Some("нужна повторная авторизация".into()) },
        SourceStatus { id: SourceId::Bandcamp, capabilities: caps(true, false, true), auth: AuthState::Expired, detail: Some("нет доступа к покупкам".into()) },
        SourceStatus { id: SourceId::SoundCloud, capabilities: caps(true, false, true), auth: AuthState::Ok, detail: Some("оригинал доступен".into()) },
    ]);
    let lines = app.status_lines();
    assert_eq!(lines.len(), 4);
    assert!(lines[0].starts_with("qobuz"));
    assert!(lines[0].contains("максимум FLAC-CD 16/44.1"));
    assert!(lines[1].contains("нужна повторная авторизация"));
    assert!(lines[2].contains("нет доступа к покупкам"));
    assert!(lines[3].contains("оригинал доступен"));
}

#[test]
fn an_unresolvable_host_is_reported_as_a_network_problem() {
    let app = maxfetch::tui::App::new(Config::default());
    let line = app.render_status_line(SourceStatus {
        id: SourceId::Qobuz,
        capabilities: caps(true, false, true),
        auth: AuthState::Missing,
        detail: Some("api.qobuz.com не резолвится".into()),
    });
    assert!(line.contains("недоступен по сети"), "{line}");
}

#[test]
fn a_missing_credential_section_reads_as_not_authorized() {
    let app = maxfetch::tui::App::new(Config::default());
    let line = app.render_status_line(SourceStatus {
        id: SourceId::Bandcamp,
        capabilities: caps(true, false, true),
        auth: AuthState::Missing,
        detail: None,
    });
    assert!(line.contains("нет авторизации"), "{line}");
}

#[test]
fn source_statuses_preserves_the_order_of_the_registry() {
    let sources: Vec<Box<dyn Source>> = vec![
        Box::new(StubSource::new(SourceId::Bandcamp, AuthState::Ok, "покупки доступны")),
        Box::new(StubSource::new(SourceId::SoundCloud, AuthState::Ok, "оригинал доступен")),
    ];
    let ids: Vec<SourceId> = maxfetch::sources::source_statuses(&sources).into_iter().map(|s| s.id).collect();
    assert_eq!(ids, vec![SourceId::Bandcamp, SourceId::SoundCloud]);
}
```

Create `tests/e2e_stage2.rs` — the §10 end-to-end scenario, gated behind `--features e2e` so the default `cargo test` never needs a live host:
```rust
#![cfg(feature = "e2e")]

use maxfetch::config::{Config, Credentials};
use maxfetch::pipeline::Pipeline;
use maxfetch::sources::{enabled_in, registry, Query, SourceId};

// One mock host serves a search hit, a resolve page and a small audio body; the pipeline
// walks search -> candidates -> download -> library scan for each stage 2 source in turn.
#[tokio::test]
async fn search_fetch_and_library_scan_round_trip_on_a_mock_server() {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let base = format!("http://{}", server.server_addr());
    let handle = std::thread::spawn(move || {
        for _ in 0..4 {
            let req = server.recv().unwrap();
            let body = std::fs::read(format!("{}/tests/fixtures/probe/sine_44100_s16.flac", env!("CARGO_MANIFEST_DIR"))).unwrap();
            req.respond(tiny_http::Response::from_data(body)).unwrap();
        }
    });

    let mut cfg = Config::default();
    cfg.sources.enabled = vec!["qobuz".into(), "spotify".into(), "bandcamp".into(), "soundcloud".into()];
    let sources = registry(&cfg, &Credentials::default());
    assert!(sources.iter().any(|s| s.id() == SourceId::Qobuz));
    assert!(enabled_in(&cfg, SourceId::Bandcamp));

    let lib = tempfile::tempdir().unwrap();
    let pipeline = Pipeline::new(cfg, maxfetch::db::Db::open_in_memory().unwrap(), sources);
    let q = Query { text: Some("daft punk".into()), url: Some(format!("{base}/")), items: vec![] };
    let _groups = pipeline.search(&q).await.unwrap();
    let indexed = maxfetch::dedup::scan_library(pipeline.db(), &lib.path()).unwrap();
    assert_eq!(indexed, 0, "library scan completes over the mock round trip without inventing files");
    handle.join().unwrap();
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test stage2_end_to_end`
Expected: FAIL with "method `status` is not a member of trait `Source`".

- [ ] **Step 3: Extend the trait, the registry, `auth login` and the TUI**

Add `fn status(&self) -> SourceStatus` as a defaulted method on `Source` in `src/sources/mod.rs`; `source_statuses` maps a slice through `status()`. `registry` stays as the core left it: it returns **every** implemented source unconditionally, with no `enabled_in` check and no credential check inside it; only the callers filter. Qobuz is therefore always registered, its `status()` reporting `нет учётных данных` when its section is missing. Apply the `detail` table from Interfaces: `SpotifySource::status()` returns `нужна повторная авторизация` when `reauth_hint()` is set and `нет авторизации: только поиск и метаданные` when `auth_state() == Missing`; `BandcampSource::status()` returns `нет доступа к покупкам` / `покупки доступны` / `нет авторизации`; `SoundCloudSource::status()` returns `оригинал доступен` / `только AAC 160k: нет oauth_token`. Each is a trait impl (`impl Source for X { fn status ... }`), never an inherent method with the same name. Set capabilities as they are: Bandcamp `lossless: true`, SoundCloud `lossless: oauth_token.is_some()`, Spotify `metadata_only: true`.

`auth::login` grows three branches that **replace** the stage 1b `stage1b_login_hint` hand-off: `"bandcamp"` prints `укажите cookie identity в credentials.toml`, reads one line from stdin, trims it and stores `BandcampCredentials { identity_cookie }`; `"soundcloud"` prints `укажите oauth_token в credentials.toml` and stores the token only when non-empty; `"qobuz"` reads e-mail and password from stdin, uses the stored `app_id`/`app_secret` pair when present and scrapes only when it is absent or rejected (Task 2's `login_with_key_refresh`, which persists a fresh pair), verifies with `HttpQobuzClient::login`, and on a missing pair prints `нужны учётные данные Qobuz`. All three go through `save_credentials`, so the 0600 guarantee holds. Delete the `stage1b_login_hint` short-circuit inside `login`, or at least make the new branches run before it, and update the stage 1b test `tests/auth_stage1b.rs` that asserted the old hint text to expect the three new sentences above; that test edit belongs to this task.

`App::set_statuses` stores the vector in the existing `status` field; `status_lines` maps it through `render_status_line`. `render_status_line` builds `format!("{} — {}", id.as_str(), text)` where `text` is, in order: `недоступен по сети` when the detail contains `не резолвится`; `нет авторизации` when `auth == Missing` and there is no detail; the detail verbatim otherwise. The TUI never invents a quality claim and never translates a source's own sentence.

- [ ] **Step 4: Run the test, the e2e test and the whole suite**

Run: `cargo test --test stage2_end_to_end`
Expected: PASS, 10 tests.

Run: `cargo test --features e2e --test e2e_stage2`
Expected: PASS, 1 test. This is the §10 end-to-end scenario; it is gated behind the `e2e` feature so the default run never opens a socket. The `e2e` feature must be declared in `Cargo.toml` (`[features] e2e = []`) in this task if it is not already there.

Run: `cargo test --test auth_stage1b`
Expected: PASS after the hint-text update from Step 3 (the old `stage1b_login_hint` sentences are replaced by the three new `auth login` messages).

Run: `cargo test`
Expected: all tests pass, including every core plan test.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(stage2): register qobuz, oauth logins and honest source statuses"
```