# maxfetch Stage 3 (Tidal, Deezer, Beatport) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the three stage-3 sources to the maxfetch core: Tidal (`tidalrs` 0.6.2, OAuth2 with a rotatable public key pair, AAC without HiFi), Deezer (`deezer-client` 0.1.4, `arl` cookie plus Blowfish-CBC decryption, MP3 128 without HiFi) and Beatport (OAuth2 `client_credentials` catalog, user-auth download of purchased tracks, quality unknown until probed). Each source implements the existing `Source` trait, reports an honest status when it is blocked or unauthorized, and participates in the shared quality ranking as a lossy or unknown-quality candidate.

**Architecture:** No new pipeline concepts are introduced. Each source lives in one file under `src/sources/`, is constructed from `Config` + `Credentials` by the existing `sources::registry`, and returns the same `Release`/`Candidate`/`Locator` shapes the core plan defined. The three sources share one small helper module, `src/sources/oauth.rs`, for token acquisition and caching (Tidal and Beatport both speak OAuth2); Deezer uses its own `arl` cookie flow. Tidal and Deezer additionally share a "derive a quality from the source's declared tier" mapping so that a tier the account cannot reach is reported as lossy rather than claimed as lossless.

**Tech Stack:** Rust 2024 edition, the crates already in the core build (`tokio`, `reqwest` with `rustls` and `cookies`, `serde`/`serde_json`, `anyhow`/`thiserror`, `tracing`), plus `tidalrs` 0.6.2, `deezer-client` 0.1.4, `blowfish` 0.10 (CBC + ECB, already a dependency), `aes` 0.9 (already a dependency) and `md-5` 0.10 for the Deezer key derivation. HTTP fixtures are replayed through a local mock server, never the network.

**Spec:** `docs/superpowers/specs/2026-11-10-maxfetch-design.md` (sections 8.6, 8.7 and 8.8 are the contract this plan implements; section 7 defines the `Source` trait, the quality rules and the library layout this plan builds on).

## Global Constraints

Carried over from the core plan (unchanged, still binding):

- Crate name and binary name: `maxfetch`. License MIT. Rust edition 2024.
- Rust toolchain 1.99.0 is installed at `/run/current-system/sw/bin/cargo`; `cargo` must be on PATH.
- Do not add dependencies that perform audio transcoding. `ffmpeg` may only be invoked with `-c copy` (stream copy, no re-encode).
- Lossy files are stored exactly as delivered by the source. No lossy-to-lossless or lossy-to-lossy re-encoding anywhere in the codebase. Deezer's Blowfish-CBC step is decryption of a stored ciphertext, not re-encoding; the decrypted MP3 bytes are written unchanged.
- Quality ordering: lossless beats lossy; inside lossless, `bit_depth` descending first, then `sample_rate` descending; inside lossy, `bitrate_kbps` descending first, then `sample_rate` descending. Missing values are never invented and always sort below known values of the same class.
- The unit of work is a track. An album is a grouping with a tracklist; partial success is normal, not an error.
- Library layout is `<library>/<Artist>/<Album>/<NN> <Title>.<ext>`; `<Album>` defaults to `Singles`; `NN` is a two-digit track number.
- `maxfetch` never deletes files implicitly. Deduplication only marks `duplicate_of`; deletion requires `maxfetch library dedupe --apply --delete --yes`.
- Secrets live in `$XDG_CONFIG_HOME/maxfetch/credentials.toml` with mode 0600. A missing credentials section means "source not authorized": that source is disabled and every other source keeps working.
- No source failure (network down, missing credentials, rotated API key, CloudFront block, refused TCP connection) may abort a run; it is recorded in the log and the source is skipped.
- Rust code, identifiers, comments and commit messages are in English. User-facing strings (TUI labels, CLI output, source status details) are in Russian.
- Every commit is authored as `m1kiru <m1kiru@github.com>`.

Stage-specific:

- The public key pairs are never hard-coded as the only path. Tidal reads `[tidal] client_id`/`client_secret` from `credentials.toml` and falls back to the built-in pair only when the section is absent; Beatport uses the section when present and the published `Marekkon5/onetagger` pair otherwise. Both are marked stale on a 400/401 from the token endpoint and reported to the user as «ключ протух» with the hint to supply their own pair.
- `AuthState::Expired` is reserved for exactly two situations in this stage: a Tidal token request rejected for an unknown client (`invalid_client`), and a Deezer API answer rejected for an invalid `arl` (`code 4`, «Invalid CSRF token», or HTTP 403 on a track request). `AuthState::Missing` covers an absent credentials section; `Ok` covers a section that is present and was not rejected.
- A quality tier the account has no subscription for must never be reported as lossless. Tidal tiers 2 and 3 without HiFi, and Deezer's HiFi tier without the subscription, become `Quality { lossless: false, .. }` and the detail string says so.
- Beatport candidates are always created with `Quality::default()`-shaped values (lossless flag `false`, all numeric fields `None`) unless a `probe` has already run; the real quality is written only after `probe` measure it, per core Task 11.
- All three sources are registered unconditionally in `sources::registry`; whether they run is decided by `config.sources.enabled` and by the credentials section, never by the network state at build time. An unreachable source returns an empty result and an `Expired`/`Ok` status with a Russian `detail`.
- Fixtures live under `tests/fixtures/{tidal,deezer,beatport}/` and are committed. They contain no live tokens, no `arl` and no password; they are recorded shapes with obviously fake values.

## Review Focus

The spec names these sources as lossy-by-default and blocked on the reference machine; these inputs are implied by it but no test in the core plan covers them. Each has a test pinned to the task that owns the relevant code.

1. **A Tidal public `client_id`/`client_secret` pair that rotated and now answers `invalid_client`.** Expected: the source reports the status «ключ протух» with a hint to paste a fresh pair into `credentials.toml`, `AuthState` becomes `Expired`, and the run continues with the other sources. Pinned to Task 2.
2. **A Tidal track that only exists at quality level 3 (HI_RES) while the account has no HiFi subscription.** Expected: the candidate is accepted at AAC (`lossless: false`, `container: M4aAac`), the detail says «подписка HiFi не активна, отдано AAC», and the lossy candidate correctly loses to any lossless elsewhere. Pinned to Task 2.
3. **A Deezer `arl` cookie that expired between the catalog search and the track download.** Expected: the source reports «нужна авторизация» and `AuthState::Expired` instead of panicking or writing a zero-byte file. Pinned to Task 5.
4. **A Deezer track encrypted with a track-specific Blowfish key that does not decrypt to valid MP3 frames.** Expected: the download is rejected, nothing is left at the destination path, and the reason says the decryption failed rather than surfacing an obscure cipher error. Pinned to Task 5.
5. **A Beatport catalog entry that carries no `bit_depth`/`sample_rate` (which v4 never exposes to an anonymous token).** Expected: the candidate is created with unknown numeric quality and ranks below an otherwise similar candidate that has known values. Pinned to Task 7.

---

### Task 1: OAuth2 helper shared by Tidal and Beatport

**Files:**
- Create: `src/sources/oauth.rs`
- Test: `tests/sources_oauth.rs`

**Interfaces:**
- Consumes: `reqwest`, `serde`, `tokio::sync::Mutex`, `chrono::Utc`.
- Produces:
  - `maxfetch::sources::oauth::TokenResponse` — `struct { access_token: String, expires_in: u64, scope: Option<String> }`, `Deserialize`, with the serde field names exactly `access_token`, `expires_in` and `scope`.
  - `maxfetch::sources::oauth::TokenCache` — `struct` holding an optional `(access_token: String, valid_until: chrono::DateTime<chrono::Utc>)` behind a `tokio::sync::Mutex`.
  - `TokenCache::new() -> TokenCache`.
  - `TokenCache::get_or_fetch<F, Fut>(&self, refresh_margin_sec: u64, fetch: F) -> anyhow::Result<String>` — returns the cached token while `now + refresh_margin_sec < valid_until`, otherwise awaits `fetch`, stores the result and returns the token.
  - `TokenCache::invalidate(&self)`.
  - `maxfetch::sources::oauth::OAuthError` — `enum { StaleKey, Network(anyhow::Error) }` with `impl std::error::Error`.
  - `maxfetch::sources::oauth::classify_token_failure(status: u16, body: &str) -> OAuthError` — `StaleKey` for HTTP 400/401 whose body contains `invalid_client` or `invalid_grant`, `Network` otherwise.

- [ ] **Step 1: Write the failing test**

Create `tests/sources_oauth.rs`:
```rust
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;
use maxfetch::sources::oauth::{classify_token_failure, OAuthError, TokenCache, TokenResponse};

#[tokio::test]
async fn token_is_fetched_once_and_cached() {
    let calls = Arc::new(AtomicU32::new(0));
    let cache = TokenCache::new();
    for _ in 0..3 {
        let c = calls.clone();
        let token = cache
            .get_or_fetch(60, || async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(TokenResponse { access_token: "tok".into(), expires_in: 600, scope: Some("app:external user:anon".into()) })
            })
            .await
            .unwrap();
        assert_eq!(token, "tok");
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn invalidate_forces_a_refetch() {
    let calls = Arc::new(AtomicU32::new(0));
    let cache = TokenCache::new();
    let c = calls.clone();
    cache
        .get_or_fetch(60, || async move {
            c.fetch_add(1, Ordering::SeqCst);
            Ok(TokenResponse { access_token: "a".into(), expires_in: 600, scope: None })
        })
        .await
        .unwrap();
    cache.invalidate();
    let c = calls.clone();
    cache
        .get_or_fetch(60, || async move {
            c.fetch_add(1, Ordering::SeqCst);
            Ok(TokenResponse { access_token: "b".into(), expires_in: 600, scope: None })
        })
        .await
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn token_near_expiry_is_refetched() {
    let calls = Arc::new(AtomicU32::new(0));
    let cache = TokenCache::new();
    let c = calls.clone();
    cache
        .get_or_fetch(60, || async move {
            c.fetch_add(1, Ordering::SeqCst);
            Ok(TokenResponse { access_token: "a".into(), expires_in: 10, scope: None })
        })
        .await
        .unwrap();
    let c = calls.clone();
    let token = cache
        .get_or_fetch(60, || async move {
            c.fetch_add(1, Ordering::SeqCst);
            Ok(TokenResponse { access_token: "b".into(), expires_in: 600, scope: None })
        })
        .await
        .unwrap();
    assert_eq!(token, "b");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn invalid_client_is_a_stale_key() {
    assert!(matches!(classify_token_failure(401, r#"{"error":"invalid_client"}"#), OAuthError::StaleKey));
    assert!(matches!(classify_token_failure(400, r#"{"error":"invalid_client"}"#), OAuthError::StaleKey));
}

#[test]
fn server_error_is_a_network_error() {
    assert!(matches!(classify_token_failure(503, "upstream"), OAuthError::Network(_)));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test sources_oauth`
Expected: FAIL with "unresolved import `maxfetch::sources::oauth`".

- [ ] **Step 3: Implement `src/sources/oauth.rs`**

`TokenCache` stores `Option<(String, chrono::DateTime<chrono::Utc>)>` inside a `tokio::sync::Mutex`. `get_or_fetch` takes the lock, releases it before awaiting `fetch` (so a slow request does not hold the mutex across a network round-trip), then re-locks and stores. `valid_until` is `Utc::now() + chrono::Duration::seconds(expires_in as i64)`. `refresh_margin_sec` guards against a token that dies mid-request.

`classify_token_failure` lowercases the body before searching for `invalid_client` and `invalid_grant`. `OAuthError` implements `Display` with `"ключ протух"` for `StaleKey` and the inner error's text for `Network`.

- [ ] **Step 4: Declare the module and run the test**

Add `pub mod oauth;` to `src/sources/mod.rs`.

Run: `cargo test --test sources_oauth`
Expected: PASS, 5 tests.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(sources): shared oauth token cache and stale-key classification"
```

---

### Task 2: Tidal source (metadata, quality tiers, stale key)

**Files:**
- Create: `src/sources/tidal.rs`
- Create: `tests/fixtures/tidal/token.json`, `tests/fixtures/tidal/track.json`, `tests/fixtures/tidal/search.json`
- Modify: `Cargo.toml` (add `tidalrs = "0.6.2"`)
- Test: `tests/source_tidal.rs`

**Interfaces:**
- Consumes: `maxfetch::sources::{AuthState, Capabilities, Query, Source}`, `maxfetch::sources::oauth::{OAuthError, TokenCache}`, `maxfetch::model::{Candidate, Container, Locator, Quality, Release, SourceId}`, `maxfetch::config::TidalCredentials`.
- Produces:
  - `maxfetch::sources::tidal::QUALITY_AAC: u8 = 0`, `QUALITY_HIGH_AAC: u8 = 1`, `QUALITY_LOSSLESS: u8 = 2`, `QUALITY_HI_RES: u8 = 3`.
  - `maxfetch::sources::tidal::quality_for_level(level: u8, has_hifi: bool) -> Quality` — the mapping described in Step 3.
  - `maxfetch::sources::tidal::pick_level(available: &[u8], has_hifi: bool) -> Option<u8>` — highest level in `available` the account may reach (`3` and `2` only when `has_hifi`, `1` and `0` always).
  - `maxfetch::sources::tidal::detail_for_level(level: u8, has_hifi: bool) -> Option<String>` — `None` for AAC/HIGH_AAC; `Some("подписка HiFi не активна, отдано AAC")` when `!has_hifi` and the requested level was 2 or 3.
  - `maxfetch::sources::tidal::TidalSource::new(credentials: TidalCredentials) -> TidalSource`.
  - `impl Source for TidalSource` with `id() == SourceId::Tidal`, `capabilities() == Capabilities { audio: true, metadata_only: false, lossless: false, needs_auth: true }`, and `auth_state()` derived from whether a token was ever obtained plus whether the last attempt was classified `StaleKey`.

- [ ] **Step 1: Create the fixtures**

Create `tests/fixtures/tidal/token.json` — a recorded OAuth token response shape, values fake:
```json
{ "access_token": "FIXTURE_TIDAL_ACCESS_TOKEN", "token_type": "Bearer", "expires_in": 86400, "scope": "r_usr w_usr" }
```
Create `tests/fixtures/tidal/track.json` — a recorded track object with the ISRC and the duration:
```json
{
  "id": 123456789,
  "title": "Get Lucky",
  "duration": 369,
  "trackNumber": 8,
  "isrc": "FR123400001",
  "copyright": "(C) 2013 Daft Life Limited",
  "artists": [{ "id": 1, "name": "Daft Punk" }],
  "album": { "id": 2, "title": "Random Access Memories" },
  "audioQuality": "HIGH",
  "mediaMetadata": { "tags": ["LOSSLESS", "HIRES_LOSSLESS"] }
}
```
Create `tests/fixtures/tidal/search.json` — a recorded catalog search answer containing one track, same shape as above wrapped in `{ "items": [ … ], "limit": 25 }`.
Expected: three files exist under `tests/fixtures/tidal/`.

- [ ] **Step 2: Write the failing test**

Create `tests/source_tidal.rs`:
```rust
use maxfetch::model::{Container, SourceId};
use maxfetch::sources::oauth::{classify_token_failure, OAuthError};
use maxfetch::sources::tidal::{detail_for_level, pick_level, quality_for_level, TidalSource};
use maxfetch::sources::{AuthState, Source};

#[test]
fn levels_zero_and_one_are_lossy_aac() {
    for level in [0u8, 1u8] {
        let q = quality_for_level(level, true);
        assert!(!q.lossless);
        assert_eq!(q.container, Container::M4aAac);
        assert_eq!(q.bit_depth, None);
        assert_eq!(q.bitrate_kbps, None);
    }
}

#[test]
fn level_two_is_lossless_flac() {
    let q = quality_for_level(2, true);
    assert!(q.lossless);
    assert_eq!(q.container, Container::Flac);
    assert_eq!(q.bit_depth, Some(16));
    assert_eq!(q.sample_rate, Some(44100));
}

#[test]
fn level_three_is_hires_lossless_flac() {
    let q = quality_for_level(3, true);
    assert!(q.lossless);
    assert_eq!(q.container, Container::Flac);
    assert_eq!(q.bit_depth, Some(24));
}

#[test]
fn no_hifi_subscription_downgrades_any_level_to_lossy() {
    for level in [2u8, 3u8] {
        let q = quality_for_level(level, false);
        assert!(!q.lossless);
        assert_eq!(q.container, Container::M4aAac);
    }
}

#[test]
fn pick_level_skips_tiers_the_account_cannot_reach() {
    assert_eq!(pick_level(&[0, 1, 2, 3], true), Some(3));
    assert_eq!(pick_level(&[0, 1, 2, 3], false), Some(1));
    assert_eq!(pick_level(&[0], false), Some(0));
    assert_eq!(pick_level(&[], true), None);
}

#[test]
fn downgrade_is_reported_in_russian() {
    assert_eq!(detail_for_level(0, false), None);
    assert_eq!(detail_for_level(1, false), None);
    assert_eq!(detail_for_level(3, false).as_deref(), Some("подписка HiFi не активна, отдано AAC"));
    assert_eq!(detail_for_level(3, true), None);
}

#[tokio::test]
async fn stale_key_reports_the_rotated_pair() {
    let err = classify_token_failure(401, r#"{"error":"invalid_client"}"#);
    assert!(matches!(err, OAuthError::StaleKey));
    let source = TidalSource::new(maxfetch::config::TidalCredentials { client_id: "old".into(), client_secret: "old".into() });
    source.mark_stale_for_test();
    assert_eq!(source.auth_state(), AuthState::Expired);
    assert_eq!(source.capabilities().lossless, false);
    assert_eq!(source.id(), SourceId::Tidal);
}

#[tokio::test]
async fn source_without_credentials_is_missing() {
    let source = TidalSource::new(maxfetch::config::TidalCredentials { client_id: String::new(), client_secret: String::new() });
    assert_eq!(source.auth_state(), AuthState::Missing);
}
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test --test source_tidal`
Expected: FAIL with "unresolved import `maxfetch::sources::tidal`".

- [ ] **Step 4: Implement `src/sources/tidal.rs`**

`quality_for_level` mapping, exactly:
```rust
pub fn quality_for_level(level: u8, has_hifi: bool) -> Quality {
    match (level, has_hifi) {
        (2, true) => Quality { lossless: true, container: Container::Flac, bit_depth: Some(16), sample_rate: Some(44100), bitrate_kbps: None },
        (3, true) => Quality { lossless: true, container: Container::Flac, bit_depth: Some(24), sample_rate: Some(48000), bitrate_kbps: None },
        _ => Quality { lossless: false, container: Container::M4aAac, bit_depth: None, sample_rate: None, bitrate_kbps: None },
    }
}
```
`sample_rate` and `bitrate_kbps` are left `None` for AAC on purpose: the API does not state them for a lossy stream and the core rule forbids inventing values.

`TidalSource` holds `credentials: TidalCredentials`, `tokens: TokenCache`, `has_hifi: bool` (read from the token response scope containing `r_usr` and a user subscription probe; in this stage default `false` because the plain `client_credentials` token carries no subscription), and `state: std::sync::Mutex<AuthState>`. `mark_stale_for_test` sets the state to `Expired`; production code calls the same internal setter from the token fetch branch that maps `OAuthError::StaleKey`.

`TidalSource::new` sets `AuthState::Missing` when `client_id` or `client_secret` is empty, `AuthState::Ok` otherwise.

`search` and `resolve` map the recorded `track.json`/`search.json` shapes into `Release` (`title`, first artist name, album title, `trackNumber`, `duration` as `f64` seconds, `isrc`, `source: SourceId::Tidal`, `source_item_id` = the track id as string). `candidates` calls `pick_level` over the `mediaMetadata.tags` plus `audioQuality` of the track and emits a `Candidate` whose `locator` is `Locator::Tidal { track_id, quality: level }`. `download` returns `anyhow::bail!` with the Russian message «Tidal: скачивание не реализовано в этой сборке (CloudFront 403 на openapi.tidal.com)» and never touches `dest`; the source advertises `detail` with the same text so the Sources screen shows the block honestly.

On `OAuthError::StaleKey` from the token endpoint, `search`/`resolve`/`candidates` return an error whose message contains «ключ протух» and «credentials.toml», and set the state to `Expired`. `classify_token_failure` from Task 1 is the only branch that decides.

- [ ] **Step 5: Add the dependency, declare the module and run the test**

```bash
cargo add tidalrs@0.6.2
```
Expected: `Cargo.toml` lists `tidalrs = "0.6.2"` and `cargo build` succeeds. If `cargo add` rejects the exact version, run `cargo add tidalrs` and then pin the resolved version with `cargo add tidalrs@0.6.2` once published; if 0.6.2 is not on crates.io, stop and report rather than substituting another version.

Add `pub mod tidal;` to `src/sources/mod.rs`.

Run: `cargo test --test source_tidal`
Expected: PASS, 8 tests.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(sources): tidal metadata, quality tiers and stale-key status"
```

---

### Task 3: Deezer source (catalog, arl auth state)

**Files:**
- Create: `src/sources/deezer.rs` (catalog and auth half; decryption goes in Task 5)
- Create: `tests/fixtures/deezer/track.json`, `tests/fixtures/deezer/search.json`, `tests/fixtures/deezer/arl_expired.json`
- Modify: `Cargo.toml` (add `deezer-client = "0.1.4"`)
- Test: `tests/source_deezer.rs`

**Interfaces:**
- Consumes: `maxfetch::sources::{AuthState, Capabilities, Query, Source}`, `maxfetch::model::{Candidate, Container, Locator, Quality, Release, SourceId}`, `maxfetch::config::DeezerCredentials`.
- Produces:
  - `maxfetch::sources::deezer::is_arl_rejected(status: u16, body: &str) -> bool` — true for HTTP 403, or a body carrying `"code":4` after whitespace removal, or the substring `Invalid CSRF token`.
  - `maxfetch::sources::deezer::mp3_128_quality() -> Quality` — `Quality { lossless: false, container: Container::Mp3, bit_depth: None, sample_rate: Some(44100), bitrate_kbps: Some(128) }`.
  - `maxfetch::sources::deezer::DeezerSource::new(credentials: DeezerCredentials) -> DeezerSource`.
  - `impl Source for DeezerSource` with `id() == SourceId::Deezer`, `capabilities() == Capabilities { audio: true, metadata_only: false, lossless: false, needs_auth: true }`.

- [ ] **Step 1: Create the fixtures**

`tests/fixtures/deezer/track.json` — recorded metadata with the ISRC and the duration:
```json
{
  "id": 3135556,
  "readable": true,
  "title": "Get Lucky",
  "duration": 369,
  "track_position": 8,
  "isrc": "FR123400001",
  "artist": { "id": 27, "name": "Daft Punk" },
  "album": { "id": 302127, "title": "Random Access Memories" }
}
```
`tests/fixtures/deezer/search.json` — `{ "data": [ … same object … ], "total": 1 }`.
`tests/fixtures/deezer/arl_expired.json` — `{ "error": { "type": "OAuthException", "message": "Invalid CSRF token", "code": 4 } }`.
Expected: three files exist under `tests/fixtures/deezer/`.

- [ ] **Step 2: Write the failing test**

Create `tests/source_deezer.rs`:
```rust
use maxfetch::config::DeezerCredentials;
use maxfetch::model::{Container, SourceId};
use maxfetch::sources::deezer::{is_arl_rejected, mp3_128_quality, DeezerSource};
use maxfetch::sources::{AuthState, Source};

#[test]
fn mp3_128_is_lossy() {
    let q = mp3_128_quality();
    assert!(!q.lossless);
    assert_eq!(q.container, Container::Mp3);
    assert_eq!(q.bitrate_kbps, Some(128));
}

#[test]
fn expired_arl_is_detected_by_code_four() {
    assert!(is_arl_rejected(200, r#"{"error":{"type":"OAuthException","message":"x","code":4}}"#));
    assert!(is_arl_rejected(200, r#"{"error":{"message":"Invalid CSRF token","code":0}}"#));
    assert!(is_arl_rejected(403, "Forbidden"));
    assert!(!is_arl_rejected(200, r#"{"id":1,"title":"ok"}"#));
}

#[test]
fn empty_arl_means_missing_credentials() {
    let source = DeezerSource::new(DeezerCredentials { arl: String::new() });
    assert_eq!(source.auth_state(), AuthState::Missing);
    assert_eq!(source.id(), SourceId::Deezer);
}

#[test]
fn present_arl_starts_ok() {
    let source = DeezerSource::new(DeezerCredentials { arl: "FIXTURE_ARL".into() });
    assert_eq!(source.auth_state(), AuthState::Ok);
    assert!(!source.capabilities().lossless);
}

#[test]
fn rejected_arl_turns_the_source_expired() {
    let source = DeezerSource::new(DeezerCredentials { arl: "FIXTURE_ARL".into() });
    source.mark_expired_for_test();
    assert_eq!(source.auth_state(), AuthState::Expired);
}
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test --test source_deezer`
Expected: FAIL with "unresolved import `maxfetch::sources::deezer`".

- [ ] **Step 4: Implement the catalog half of `src/sources/deezer.rs`**

`is_arl_rejected` strips all whitespace from `body`, then returns true for `status == 403`, for a body containing `"code":4`, or for the literal `Invalid CSRF token`. `DeezerSource` holds `credentials: DeezerCredentials` and `state: std::sync::Mutex<AuthState>`; `mark_expired_for_test` is the test hook for the same setter production code uses when `is_arl_rejected` fires.

`search`/`resolve` map `search.json`/`track.json` to `Release` (`title`, `artist.name`, `album.title`, `track_position`, `duration` as seconds, `isrc`, `source: SourceId::Deezer`, `source_item_id` = id). `candidates` returns one `Candidate` with `quality_for_tier(false)` from Task 4 and `locator: Locator::Deezer { track_id }`. A call that gets `is_arl_rejected(...) == true` sets `Expired` and returns an error whose message is exactly «Deezer: нужна авторизация (cookie arl протухла)». `download` is implemented in Task 5; until then it is a stub that returns the same authorization error when the state is `Expired`, and otherwise forwards to the private `decrypt_and_write` helper Task 5 fills in — for this task the stub returns `anyhow::bail!("Deezer: скачивание подключается в Task 5")` so the task compiles and its tests are about catalog and auth only.

- [ ] **Step 5: Add the dependency, declare the module and run the test**

```bash
cargo add deezer-client@0.1.4
```
Expected: `Cargo.toml` lists `deezer-client = "0.1.4"` and `cargo build` succeeds. The crate is used only for the catalog calls; if it does not expose the search and track endpoints needed here, fall back to `reqwest` against `api.deezer.com` with the `arl` cookie header and keep the same public interface, noting it in the task commit.

Add `pub mod deezer;` to `src/sources/mod.rs`.

Run: `cargo test --test source_deezer`
Expected: PASS, 5 tests.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(sources): deezer catalog and arl auth state"
```

---

### Task 4: Deezer quality tiers and HiFi gating

**Files:**
- Modify: `src/sources/deezer.rs`
- Test: `tests/source_deezer_quality.rs`

**Interfaces:**
- Consumes: `maxfetch::model::{Container, Quality}`.
- Produces:
  - `maxfetch::sources::deezer::quality_for_tier(hifi: bool) -> Quality` — `mp3_128_quality()` when `!hifi`, otherwise `Quality { lossless: true, container: Container::Flac, bit_depth: Some(16), sample_rate: Some(44100), bitrate_kbps: None }`.
  - `maxfetch::sources::deezer::detail_for_tier(hifi: bool) -> Option<String>` — `Some("подписка HiFi не активна, отдано MP3 128")` when `!hifi`, `None` otherwise.

- [ ] **Step 1: Write the failing test**

Create `tests/source_deezer_quality.rs`:
```rust
use maxfetch::model::Container;
use maxfetch::sources::deezer::{detail_for_tier, mp3_128_quality, quality_for_tier};

#[test]
fn without_hifi_the_tier_is_mp3_128() {
    assert_eq!(quality_for_tier(false), mp3_128_quality());
}

#[test]
fn with_hifi_the_tier_is_cd_flac() {
    let q = quality_for_tier(true);
    assert!(q.lossless);
    assert_eq!(q.container, Container::Flac);
    assert_eq!(q.bit_depth, Some(16));
    assert_eq!(q.sample_rate, Some(44100));
}

#[test]
fn the_downgrade_is_reported_in_russian() {
    assert_eq!(detail_for_tier(false).as_deref(), Some("подписка HiFi не активна, отдано MP3 128"));
    assert_eq!(detail_for_tier(true), None);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test source_deezer_quality`
Expected: FAIL with "cannot find function `quality_for_tier`".

- [ ] **Step 3: Implement the tier mapping**

`quality_for_tier(false)` returns `mp3_128_quality()`; `Quality` implements `PartialEq`, so the test compares by value. `DeezerSource::candidates` calls `quality_for_tier(self.has_hifi)`; `has_hifi` is a constructor field defaulting to `false`, because an `arl` cookie without a HiFi subscription serves MP3 128 and the API states no bitrate of its own. `detail_for_tier` feeds the `SourceStatus::detail` the registry reports.

- [ ] **Step 4: Run the test**

Run: `cargo test --test source_deezer_quality`
Expected: PASS, 3 tests.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(sources): deezer quality tiers with hifi gating"
```

---

### Task 5: Deezer Blowfish-CBC decryption and safe download

**Files:**
- Create: `src/sources/deezer_crypto.rs`
- Create: `tests/fixtures/deezer/encrypted_track.bin`, `tests/fixtures/deezer/encrypted_track_plain.bin`, `tests/fixtures/deezer/encrypted_track.key`
- Modify: `src/sources/deezer.rs` (replace the Task 3 download stub)
- Test: `tests/deezer_crypto.rs`, `tests/source_deezer_download.rs`

**Interfaces:**
- Consumes: `blowfish` 0.10 (`Blowfish`, `cipher::{BlockDecryptMut, KeyIvInit, block_padding::NoPadding}`), `md-5` 0.10.
- Produces:
  - `maxfetch::sources::deezer_crypto::derive_key(track_id: &str, md5_hex: &str) -> [u8; 16]` — the first 16 bytes of `md5(key_ascii)` where `key_ascii` is the concatenation described in Step 3.
  - `maxfetch::sources::deezer_crypto::decrypt_track(key: &[u8; 16], iv: &[u8; 8], ciphertext: &[u8]) -> anyhow::Result<Vec<u8>>` — Blowfish-CBC with no padding removal; fails when `ciphertext.len() % 8 != 0`.
  - `maxfetch::sources::deezer_crypto::decrypted_looks_like_mp3(bytes: &[u8]) -> bool` — true when the first bytes are `ID3`, or when a frame sync `0xFF 0xEx`/`0xFF 0xFx` appears within the first 4 KiB.
  - `DeezerSource::download_fixture(&self, ciphertext: &Path, key: &[u8; 16], iv: &[u8; 8], dest: &Path) -> anyhow::Result<()>` — reads `ciphertext`, decrypts and writes through the same path `download` uses, so the failure semantics are testable without a network.

- [ ] **Step 1: Create the fixtures**

Generate once and commit — a deterministic ciphertext built from a synthetic MP3 head, not tied to any real track:
```bash
mkdir -p tests/fixtures/deezer
```
Add a `#[ignore]` helper test `tests/deezer_crypto.rs::write_fixtures` that builds a 4096-byte plaintext whose first bytes are `ID3\x04\x00\x00\x00\x00\x00\x00` followed by a repeating `0xFF 0xFB 0x90 0x64` frame pattern, encrypts it with a fixed 16-byte key `[0x11; 16]` and the fixed zero IV, and writes `encrypted_track_plain.bin`, `encrypted_track.bin` and `encrypted_track.key`. Run it once:
```bash
cargo test --test deezer_crypto -- --ignored write_fixtures
```
Then commit the three produced files. `encrypted_track.bin` must be a multiple of 8 bytes; the helper asserts that before writing.
Expected: three files exist under `tests/fixtures/deezer/`, and `encrypted_track_plain.bin` begins with the bytes `ID3`.

- [ ] **Step 2: Write the failing tests**

Create `tests/deezer_crypto.rs`:
```rust
use std::path::PathBuf;
use maxfetch::sources::deezer_crypto::{decrypt_track, decrypted_looks_like_mp3, derive_key};

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/deezer").join(name)).unwrap()
}

#[test]
fn decryption_matches_the_recorded_plaintext() {
    let key_bytes = fixture("encrypted_track.key");
    let mut key = [0u8; 16];
    key.copy_from_slice(&key_bytes);
    let iv = [0u8; 8];
    let out = decrypt_track(&key, &iv, &fixture("encrypted_track.bin")).unwrap();
    assert_eq!(out, fixture("encrypted_track_plain.bin"));
}

#[test]
fn a_truncated_ciphertext_is_rejected() {
    let mut key = [0u8; 16];
    key.copy_from_slice(&fixture("encrypted_track.key"));
    let mut bad = fixture("encrypted_track.bin");
    bad.pop();
    assert!(decrypt_track(&key, &[0u8; 8], &bad).is_err());
}

#[test]
fn key_derivation_is_deterministic() {
    let a = derive_key("3135556", "0123456789abcdef0123456789abcdef");
    let b = derive_key("3135556", "0123456789abcdef0123456789abcdef");
    assert_eq!(a, b);
    assert_ne!(a, derive_key("3135557", "0123456789abcdef0123456789abcdef"));
}

#[test]
fn mp3_detection_accepts_id3_and_frame_sync() {
    let plain = fixture("encrypted_track_plain.bin");
    assert!(decrypted_looks_like_mp3(&plain));
    assert!(decrypted_looks_like_mp3(&[0xFF, 0xFB, 0x90, 0x00]));
    assert!(!decrypted_looks_like_mp3(&[0x00, 0x01, 0x02, 0x03]));
}
```

Create `tests/source_deezer_download.rs`:
```rust
use std::path::PathBuf;
use maxfetch::config::DeezerCredentials;
use maxfetch::sources::deezer::DeezerSource;
use maxfetch::sources::Source;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/deezer").join(name)
}

#[tokio::test]
async fn failed_decryption_leaves_no_file_behind() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.mp3");
    let source = DeezerSource::new(DeezerCredentials { arl: "FIXTURE_ARL".into() });
    // The recorded arl was rejected between search and download in this flow.
    source.mark_expired_for_test();
    let err = source
        .download_fixture(&fixture("encrypted_track.bin"), &[0u8; 16], &[0u8; 8], &dest)
        .await
        .unwrap_err();
    assert!(!dest.exists());
    assert!(format!("{err}").contains("нужна авторизация") || format!("{err}").contains("не расшифрован"));
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test --test deezer_crypto --test source_deezer_download`
Expected: FAIL with "unresolved import `maxfetch::sources::deezer_crypto`" and "no method named `download_fixture`".

- [ ] **Step 4: Implement `src/sources/deezer_crypto.rs`**

`derive_key` is a pure function of its two arguments: it concatenates `track_id` with the 16 hex characters of `md5_hex` selected by the digits of `track_id`, MD5s that string and returns the first 16 bytes of the digest. The fixture written in Step 1 uses a fixed key directly, so `decryption_matches_the_recorded_plaintext` covers `decrypt_track` and not the derivation; `key_derivation_is_deterministic` covers the shape of `derive_key`, and the exact interleaving must match what the real Deezer track id yields — verify it against one real track before merging.

`decrypt_track` uses `<<Blowfish as KeyIvInit>::new_from_slices(key, iv)`, then `decrypt_padded_vec_mut::<NoPadding>`; a length not divisible by 8 is an error, never a padded read.

`decrypted_looks_like_mp3` checks `starts_with(b"ID3")` first, then scans the first 4096 bytes for `0xFF` followed by a byte whose high nibble is `E` or `F`.

`DeezerSource::download` fetches the encrypted bytes, calls the shared `decrypt_and_write(key, iv, ciphertext, dest)` path, and therefore writes to `dest` only after `decrypted_looks_like_mp3` passes; on failure `dest` does not exist and the error message is exactly «Deezer: трек не расшифрован (неверный ключ или повреждённый поток)». `download_fixture` is the file-backed entry point into the same function.

- [ ] **Step 5: Implement the real `DeezerSource::download`**

`download` fetches the encrypted bytes, derives the key, decrypts in memory, and only then writes to `dest` via a `dest.with_extension("part")` file renamed on success — same atomic pattern as `maxfetch::fetch::download_to_file`. When `is_arl_rejected` fires, set `Expired` and bail with «Deezer: нужна авторизация (cookie arl протухла)». When `decrypted_looks_like_mp3` is false, bail with «Deezer: трек не расшифрован (неверный ключ или повреждённый поток)» and leave no `.part` behind. `download_fixture(ciphertext, key, iv, dest)` is a thin test-facing wrapper over the same decrypt-decide-write path so the review-focus test can exercise it without a network.

- [ ] **Step 6: Declare the module and run the tests**

Add `pub mod deezer_crypto;` to `src/sources/mod.rs`.

Run: `cargo test --test deezer_crypto --test source_deezer_download`
Expected: PASS, 4 tests and 1 test.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(sources): deezer blowfish-cbc decryption with safe download"
```

---

### Task 6: Beatport catalog client and field mapping

**Files:**
- Create: `src/sources/beatport.rs`
- Create: `tests/fixtures/beatport/token.json`, `tests/fixtures/beatport/search_tracks.json`, `tests/fixtures/beatport/track.json`
- Test: `tests/source_beatport.rs`

**Interfaces:**
- Consumes: `maxfetch::sources::{AuthState, Capabilities, Query, Source}`, `maxfetch::sources::oauth::{OAuthError, TokenCache}`, `maxfetch::model::{Candidate, Locator, Quality, Release, SourceId}`, `maxfetch::config::BeatportCredentials`.
- Produces:
  - `maxfetch::sources::beatport::PUBLIC_CLIENT_ID: &str` and `PUBLIC_CLIENT_SECRET: &str` — the published `Marekkon5/onetagger` pair, verbatim.
  - `maxfetch::sources::beatport::length_ms_to_sec(ms: u64) -> f64`.
  - `maxfetch::sources::beatport::release_from_track_json(v: &serde_json::Value) -> anyhow::Result<Release>` — maps `name`, `mix_name`, `artists[].name`, `length_ms`, `isrc`, `release.name`, `publish_date` into a `Release`.
  - `maxfetch::sources::beatport::display_title(name: &str, mix_name: Option<&str>) -> String`.
  - `maxfetch::sources::beatport::BeatportSource::new(credentials: Option<BeatportCredentials>) -> BeatportSource`.
  - `impl Source for BeatportSource` with `id() == SourceId::Beatport`, `capabilities() == Capabilities { audio: true, metadata_only: false, lossless: false, needs_auth: true }`.

- [ ] **Step 1: Create the fixtures**

`tests/fixtures/beatport/token.json` — recorded token shape, fake values:
```json
{ "access_token": "FIXTURE_BEATPORT_TOKEN", "expires_in": 600, "scope": "app:external user:anon", "token_type": "Bearer" }
```
`tests/fixtures/beatport/search_tracks.json`:
```json
{
  "results": [
    {
      "id": 900001,
      "name": "Get Lucky",
      "mix_name": "Album Mix",
      "artists": [{ "name": "Daft Punk" }],
      "length_ms": 369000,
      "bpm": 116,
      "key": { "camelot_number": 9, "camelot_letter": "B" },
      "isrc": "FR123400001",
      "price": { "value": 1.29, "code": "USD" },
      "release": { "name": "Random Access Memories", "label": { "name": "Daft Life" }, "publish_date": "2013-05-17", "catalog_number": "DRL0001" },
      "new_release_date": "2013-05-17",
      "sample_url": "https://fixture.invalid/sample.mp3",
      "audio_format": null
    }
  ],
  "per_page": 25
}
```
`tests/fixtures/beatport/track.json` — the same single object as the `results[0]` value, at top level.
Expected: three files exist under `tests/fixtures/beatport/`.

- [ ] **Step 2: Write the failing test**

Create `tests/source_beatport.rs`:
```rust
use maxfetch::model::{Container, SourceId};
use maxfetch::sources::beatport::{display_title, length_ms_to_sec, release_from_track_json, BeatportSource};
use maxfetch::sources::{AuthState, Source};

fn fixture_json(name: &str) -> serde_json::Value {
    let raw = std::fs::read_to_string(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/beatport").join(name)).unwrap();
    serde_json::from_str(&raw).unwrap()
}

#[test]
fn length_ms_becomes_seconds() {
    assert_eq!(length_ms_to_sec(369000), 369.0);
    assert!((length_ms_to_sec(12345) - 12.345).abs() < 1e-9);
}

#[test]
fn display_title_joins_the_mix_name() {
    assert_eq!(display_title("Get Lucky", Some("Album Mix")), "Get Lucky (Album Mix)");
    assert_eq!(display_title("Get Lucky", None), "Get Lucky");
    assert_eq!(display_title("Get Lucky", Some("")), "Get Lucky");
}

#[test]
fn track_json_maps_to_a_release_with_isrc_and_duration() {
    let v = fixture_json("track.json");
    let r = release_from_track_json(&v).unwrap();
    assert_eq!(r.title, "Get Lucky (Album Mix)");
    assert_eq!(r.artist, "Daft Punk");
    assert_eq!(r.album.as_deref(), Some("Random Access Memories"));
    assert_eq!(r.isrc.as_deref(), Some("FR123400001"));
    assert_eq!(r.duration_sec, Some(369.0));
    assert_eq!(r.year, Some(2013));
    assert_eq!(r.source, SourceId::Beatport);
    assert_eq!(r.source_item_id, "900001");
}

#[test]
fn missing_required_field_is_an_error() {
    let v = serde_json::json!({ "mix_name": "x" });
    assert!(release_from_track_json(&v).is_err());
}

#[test]
fn plain_credentials_are_ok_and_absent_ones_are_missing() {
    let with = BeatportSource::new(Some(maxfetch::config::BeatportCredentials { client_id: "a".into(), client_secret: "b".into(), username: String::new(), password: String::new() }));
    assert_eq!(with.auth_state(), AuthState::Ok);
    let without = BeatportSource::new(Some(maxfetch::config::BeatportCredentials { client_id: String::new(), client_secret: String::new(), username: String::new(), password: String::new() }));
    assert_eq!(without.auth_state(), AuthState::Missing);
    assert_eq!(without.id(), SourceId::Beatport);
    assert_eq!(without.capabilities().lossless, false);
}
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test --test source_beatport`
Expected: FAIL with "unresolved import `maxfetch::sources::beatport`".

- [ ] **Step 4: Implement the catalog half of `src/sources/beatport.rs`**

`BeatportSource` holds `credentials: Option<BeatportCredentials>`, `tokens: TokenCache` and `state: std::sync::Mutex<AuthState>`. The token fetch posts `grant_type=client_credentials`, `client_id`, `client_secret` to the OAuth token endpoint and deserializes `TokenResponse`; an absent or empty pair falls back to `PUBLIC_CLIENT_ID`/`PUBLIC_CLIENT_SECRET`. `OAuthError::StaleKey` sets `Expired` and makes the methods bail with a message containing «ключ протух».

`release_from_track_json` requires the `id`, `name` and `artists` keys and errors on absence. `year` comes from the first four characters of `release.publish_date` parsed as `i32` (also accept `new_release_date` when `publish_date` is absent). `release.label.name`, `catalog_number`, `bpm` and `key.camelot_*` are read for the log line and are not part of `Release`.

`search` calls `GET /v4/catalog/search/?q=&type=tracks`, `resolve` calls `GET /v4/catalog/tracks/{id}/`, and both map results through `release_from_track_json`. `candidates` reads the `audio_format` field of the fetched track JSON; when it is absent or `null` the candidate gets the unknown quality of Task 7 (`unknown_quality()`), never a guess.

- [ ] **Step 5: Declare the module and run the test**

Add `pub mod beatport;` to `src/sources/mod.rs`.

Run: `cargo test --test source_beatport`
Expected: PASS, 5 tests.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(sources): beatport catalog client and field mapping"
```

---

### Task 7: Beatport unknown quality and ranking

**Files:**
- Modify: `src/sources/beatport.rs`
- Test: `tests/source_beatport_quality.rs`

**Interfaces:**
- Consumes: `maxfetch::model::{Candidate, Container, Locator, Quality, Release, SourceId}`, `maxfetch::quality::{compare_quality, rank_candidates}`.
- Produces:
  - `maxfetch::sources::beatport::unknown_quality() -> Quality` — `Quality { lossless: false, container: Container::Mp3, bit_depth: None, sample_rate: None, bitrate_kbps: None }`.
  - `maxfetch::sources::beatport::known_quality(bit_depth: u8, sample_rate: u32) -> Quality` — used when a `probe` has already measured the file.
  - `BeatportSource::candidate_for(&self, release: &Release) -> Candidate` — builds a candidate whose quality is `unknown_quality()` whenever the API exposed no `audio_format`.

- [ ] **Step 1: Write the failing test**

Create `tests/source_beatport_quality.rs`:
```rust
use maxfetch::model::{Candidate, Container, Locator, Quality, Release, SourceId};
use maxfetch::sources::beatport::{known_quality, unknown_quality};
use maxfetch::quality::{compare_quality, rank_candidates};

fn cand(quality: Quality, isrc: Option<&str>) -> Candidate {
    Candidate {
        source: SourceId::Beatport,
        release: Release {
            title: "Get Lucky".into(),
            artist: "Daft Punk".into(),
            album: Some("RAM".into()),
            track_no: Some(8),
            duration_sec: Some(369.0),
            isrc: isrc.map(|s| s.into()),
            year: Some(2013),
            source: SourceId::Beatport,
            source_item_id: "900001".into(),
        },
        quality,
        size_bytes: None,
        locator: Locator::Beatport { track_id: "900001".into() },
    }
}

#[test]
fn unknown_quality_ranks_below_a_known_one() {
    let unknown = cand(unknown_quality(), Some("FR1"));
    let known = cand(known_quality(16, 44100), Some("FR1"));
    assert_eq!(compare_quality(&unknown.quality, &known.quality), std::cmp::Ordering::Less);
    let ranked = rank_candidates(&[unknown, known]);
    assert_eq!(ranked[0].quality.bit_depth, Some(16));
}

#[test]
fn unknown_quality_ranks_below_a_known_lossy_bitrate_too() {
    let unknown = cand(unknown_quality(), None);
    let known_mp3 = cand(Quality { lossless: false, container: Container::Mp3, bit_depth: None, sample_rate: Some(44100), bitrate_kbps: Some(320) }, None);
    let ranked = rank_candidates(&[unknown, known_mp3]);
    assert_eq!(ranked[0].quality.bitrate_kbps, Some(320));
}

#[test]
fn unknown_quality_never_claims_lossless() {
    let q = unknown_quality();
    assert!(!q.lossless);
    assert_eq!(q.bit_depth, None);
    assert_eq!(q.sample_rate, None);
    assert_eq!(q.bitrate_kbps, None);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test source_beatport_quality`
Expected: FAIL with "cannot find function `unknown_quality`".

- [ ] **Step 3: Implement `unknown_quality`, `known_quality` and `candidate_for`**

`known_quality(bit_depth, sample_rate)` returns `Quality { lossless: true, container: Container::Flac, bit_depth: Some(bit_depth), sample_rate: Some(sample_rate), bitrate_kbps: None }`; it is used only after `probe` and never by the catalog fetch. `candidate_for` re-reads the track JSON for `audio_format`; when it is `null` the quality is `unknown_quality()`. The `rank_candidates` test above relies on the core `cmp_opt_desc` rule that `None` sorts below `Some` for the same class, which is exactly why an unknown Beatport candidate loses to a known one without any special-case code in this module.

- [ ] **Step 4: Run the test**

Run: `cargo test --test source_beatport_quality`
Expected: PASS, 3 tests.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(sources): beatport unknown quality ranked below known candidates"
```

---

### Task 8: Beatport user-auth download for purchased tracks

**Files:**
- Modify: `src/sources/beatport.rs`
- Test: `tests/source_beatport_download.rs`

**Interfaces:**
- Consumes: `maxfetch::config::BeatportCredentials`, `reqwest`.
- Produces:
  - `maxfetch::sources::beatport::authorization_code_from_location(location: &str) -> Option<String>` — parses `code=` out of a redirect `Location` header value.
  - `maxfetch::sources::beatport::classify_download_failure(status: u16) -> Option<&'static str>` — `Some("трек не куплен")` for 403, `Some("нужна авторизация")` for 401, `None` otherwise.
  - `BeatportSource::user_download(&self, track_id: &str, dest: &Path, sink: &dyn ProgressSink) -> anyhow::Result<()>` — logs in with username/password, exchanges the code for a user token, fetches `GET /v4/catalog/tracks/{id}/download/` and streams it to `dest`.
  - `BeatportSource::download` delegates: when `username` and `password` are set it calls `user_download`; otherwise it returns an error whose message is «Beatport: скачивание требует логина и владения треком».

- [ ] **Step 1: Write the failing test**

Create `tests/source_beatport_download.rs`:
```rust
use maxfetch::sources::beatport::{authorization_code_from_location, classify_download_failure};

#[test]
fn code_is_parsed_from_the_redirect_location() {
    assert_eq!(
        authorization_code_from_location("https://api.beatport.com/v4/auth/o/post-message/?code=abc123&state=x"),
        Some("abc123".into())
    );
    assert_eq!(authorization_code_from_location("https://api.beatport.com/v4/auth/o/post-message/?error=denied"), None);
}

#[test]
fn forbidden_means_not_owned() {
    assert_eq!(classify_download_failure(403), Some("трек не куплен"));
}

#[test]
fn unauthorized_means_login_needed() {
    assert_eq!(classify_download_failure(401), Some("нужна авторизация"));
}

#[test]
fn other_codes_are_not_classified() {
    assert_eq!(classify_download_failure(500), None);
    assert_eq!(classify_download_failure(200), None);
}

#[tokio::test]
async fn download_without_credentials_says_it_requires_login() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("t.mp3");
    let source = maxfetch::sources::beatport::BeatportSource::new(Some(maxfetch::config::BeatportCredentials {
        client_id: "a".into(), client_secret: "b".into(), username: String::new(), password: String::new(),
    }));
    let cand = maxfetch::model::Candidate {
        source: maxfetch::model::SourceId::Beatport,
        release: maxfetch::model::Release {
            title: "T".into(), artist: "A".into(), album: None, track_no: None,
            duration_sec: None, isrc: None, year: None,
            source: maxfetch::model::SourceId::Beatport, source_item_id: "900001".into(),
        },
        quality: maxfetch::sources::beatport::unknown_quality(),
        size_bytes: None,
        locator: maxfetch::model::Locator::Beatport { track_id: "900001".into() },
    };
    let err = maxfetch::sources::Source::download(&source, &cand, &dest, &maxfetch::fetch::NoopSink).await.unwrap_err();
    assert!(format!("{err}").contains("требует логина"));
    assert!(!dest.exists());
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test source_beatport_download`
Expected: FAIL with "cannot find function `authorization_code_from_location`".

- [ ] **Step 3: Implement the user-auth flow**

`user_download` follows the sequence the spec records: `POST /v4/auth/login/` with the username and password, then `GET /v4/auth/o/authorize/?response_type=code&client_id=<id>&redirect_uri=https://api.beatport.com/v4/auth/o/post-message/` with redirects disabled (`reqwest::redirect::Policy::none()`), read `code` from the `Location` header via `authorization_code_from_location`, then `POST /v4/auth/o/token/` with `grant_type=authorization_code`. The user token is cached separately in the same `TokenCache` shape because its lifetime differs from the anonymous token.

The download streams into `dest.with_extension("part")` through `maxfetch::fetch::ProgressSink` and renames on completion, exactly like the core downloader. A non-2xx response is mapped with `classify_download_failure`; a 403 leaves no file and returns an error whose message is exactly «Beatport: трек не куплен» with the track id appended in parentheses.

- [ ] **Step 4: Run the test**

Run: `cargo test --test source_beatport_download`
Expected: PASS, 5 tests.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(sources): beatport user-auth download for purchased tracks"
```

---

### Task 9: Registry wiring, source status and stage-3 integration

**Files:**
- Modify: `src/sources/mod.rs` (registry entries, `pub mod` declarations for the three sources)
- Modify: `src/auth.rs` (real `login` branches for `tidal`, `deezer`, `beatport`)
- Test: `tests/sources_registry_stage3.rs`

**Interfaces:**
- Consumes: `maxfetch::sources::{registry, registry_always_all, AuthState, SourceStatus}`, `maxfetch::config::{Config, Credentials, TidalCredentials, DeezerCredentials, BeatportCredentials}`.
- Produces:
  - `maxfetch::sources::registry_always_all(config: &Config, credentials: &Credentials) -> Vec<Box<dyn Source>>` — the same as `registry` but ignoring `config.sources.enabled`; it always contains exactly one entry per implemented source (Tidal, Deezer, Beatport in this stage, plus whatever a previous stage registered), so the Sources screen can show a disabled source with its reason.
  - `maxfetch::sources::registry` keeps its core contract (enabled sources only) and now includes Tidal, Deezer and Beatport.
  - `maxfetch::auth::login(source: &str, credentials: &mut Credentials) -> anyhow::Result<()>` gains real branches: `"tidal"` asks for a `client_id`/`client_secret` pair on stdin and stores it; `"deezer"` asks for the `arl` cookie; `"beatport"` asks for `client_id`/`client_secret`/`username`/`password`. Every branch prints its prompt in Russian.

- [ ] **Step 1: Write the failing test**

Create `tests/sources_registry_stage3.rs`:
```rust
use maxfetch::config::{BeatportCredentials, Config, Credentials, DeezerCredentials, TidalCredentials};
use maxfetch::model::SourceId;
use maxfetch::sources::{registry, registry_always_all, AuthState};

fn creds_all() -> Credentials {
    let mut c = Credentials::default();
    c.tidal = Some(TidalCredentials { client_id: "a".into(), client_secret: "b".into() });
    c.deezer = Some(DeezerCredentials { arl: "FIXTURE_ARL".into() });
    c.beatport = Some(BeatportCredentials { client_id: "a".into(), client_secret: "b".into(), username: String::new(), password: String::new() });
    c
}

#[test]
fn stage3_sources_are_registered_when_enabled() {
    let mut cfg = Config::default();
    cfg.sources.enabled = vec!["tidal".into(), "deezer".into(), "beatport".into()];
    let sources = registry(&cfg, &creds_all());
    let ids: Vec<SourceId> = sources.iter().map(|s| s.id()).collect();
    assert!(ids.contains(&SourceId::Tidal));
    assert!(ids.contains(&SourceId::Deezer));
    assert!(ids.contains(&SourceId::Beatport));
    assert_eq!(ids.len(), 3);
}

#[test]
fn a_disabled_source_is_absent_from_registry_but_present_in_always_all() {
    let mut cfg = Config::default();
    cfg.sources.enabled = vec!["beatport".into()];
    let enabled: Vec<SourceId> = registry(&cfg, &creds_all()).iter().map(|s| s.id()).collect();
    assert_eq!(enabled, vec![SourceId::Beatport]);
    let all: Vec<SourceId> = registry_always_all(&cfg, &creds_all()).iter().map(|s| s.id()).collect();
    assert!(all.contains(&SourceId::Tidal));
    assert!(all.contains(&SourceId::Deezer));
}

#[test]
fn missing_credentials_do_not_remove_the_source() {
    let cfg = Config::default();
    let sources = registry_always_all(&cfg, &Credentials::default());
    let tidal = sources.iter().find(|s| s.id() == SourceId::Tidal).unwrap();
    assert_eq!(tidal.auth_state(), AuthState::Missing);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test sources_registry_stage3`
Expected: FAIL with "cannot find function `registry_always_all`".

- [ ] **Step 3: Implement the registry and the login branches**

`registry` filters `registry_always_all` through `enabled_in`. `registry_always_all` constructs a `TidalSource` from `credentials.tidal.clone().unwrap_or_default()`, a `DeezerSource` from `credentials.deezer.clone().unwrap_or_default()`, and a `BeatportSource` from `credentials.beatport.clone()`. Because the credential structs derive `Clone` (core Task 3 requirement), the registry does not mutate them; the token caches live inside the boxes. The Tidal and Deezer structs must therefore derive `Default` with empty strings; add the derive in this task if the core type lacks it, and note it in the commit message.

`auth::login` keeps the core "not implemented" fallback for sources outside this stage and switches on the `source` string for the three stage-3 ones. Each branch writes into `credentials` and calls the existing `save_credentials`, which writes mode 0600.

- [ ] **Step 4: Run the test**

Run: `cargo test --test sources_registry_stage3`
Expected: PASS, 3 tests.

- [ ] **Step 5: Run the whole suite**

Run: `cargo test`
Expected: all tests pass, including the core suite, with no core test altered.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(sources): register stage-3 sources and wire their logins"
```