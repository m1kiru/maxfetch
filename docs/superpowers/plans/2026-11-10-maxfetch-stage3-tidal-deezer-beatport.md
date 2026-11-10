# maxfetch Stage 3 (Tidal, Deezer, Beatport) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the three stage-3 sources to the maxfetch core: Tidal (`reqwest` against the endpoints, OAuth2 `client_credentials` with a user-supplied `client_id`/`client_secret` pair, AAC without HiFi), Deezer (`deezer-client` 0.1.4, `arl` cookie plus Blowfish-CBC decryption, MP3 128 without HiFi) and Beatport (OAuth2 `client_credentials` catalog with a user-supplied pair, user-auth download of purchased tracks, quality unknown until probed). Each source implements the existing `Source` trait, reports an honest status when it is blocked or unauthorized, and participates in the shared quality ranking as a lossy or unknown-quality candidate.

**Architecture:** No new pipeline concepts are introduced. Each source lives in one file under `src/sources/`, is constructed from `Config` + `Credentials` by the existing `sources::registry`, and returns the same `Release`/`Candidate`/`Locator` shapes the core plan defined. The three sources share one small helper module, `src/sources/oauth.rs`, for token acquisition and caching (Tidal and Beatport both speak OAuth2); Deezer uses its own `arl` cookie flow. Tidal and Deezer additionally share a "derive a quality from the source's declared tier" mapping so that a tier above what the account is entitled to is reported at the quality the account actually receives instead of at face value.

**Tech Stack:** Rust 2024 edition, the crates already in the core build (`tokio`, `reqwest` with `rustls` and `cookies`, `serde`/`serde_json`, `anyhow`/`thiserror`, `tracing`), plus `deezer-client` 0.1.4, `blowfish` 0.10 (CBC + ECB, already a dependency), `aes` 0.9 (already a dependency) and `md-5` 0.10 for the Deezer key derivation. `md-5` 0.10 is an extension of the crate list in spec §8.7 (which names only `deezer-client` 0.1.4, `blowfish` 0.10 and `aes` 0.9) and is required for the Deezer key derivation. Both Tidal and Beatport speak HTTP through `reqwest`, like every other HTTP source. HTTP fixtures are replayed through a local mock server, never the network.

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

- The public key pairs are never hard-coded. Tidal reads `[tidal] client_id`/`client_secret` from `credentials.toml`; Beatport reads `[beatport] client_id`/`client_secret` from the same file. There is no built-in fallback pair for either source: when the section is absent or either field is empty, the source is disabled with the status «нет учётных данных Tidal» / «нет учётных данных Beatport» and every other source keeps working. The user pastes the pair into `credentials.toml` (or through `maxfetch auth login <source>`) themselves; the plan names no published pair. A pair that the provider rejected is marked stale on a 400/401 from the token endpoint and reported to the user as «ключ протух» with the hint to supply a fresh pair.
- `AuthState::Expired` is reserved for exactly two situations in this stage: a Tidal token request rejected for an unknown client (`invalid_client`), and a Deezer API answer rejected for an invalid `arl` (`code 4`, «Invalid CSRF token», or HTTP 403 on a track request). `AuthState::Missing` covers an absent credentials section; `Ok` covers a section that is present and was not rejected.
- A quality tier the account has no subscription for must never be reported as lossless at face value. Tidal tiers 2 and 3 without HiFi are served at CD quality at best (HI_RES becomes `lossless: true` with `bit_depth: Some(16)`, `sample_rate: Some(44100)`; LOSSLESS becomes the same CD FLAC), and the detail string says the HiFi subscription is not active. Deezer's HiFi tier without the subscription becomes `Quality { lossless: false, .. }` (MP3 128) and the detail string says so.
- `md-5` 0.10 is an extension of the crate list in spec §8.7: the spec names `deezer-client` 0.1.4, `blowfish` 0.10 and `aes` 0.9 only, and the Deezer key derivation needs MD5. Spec §8.7 is amended with `md-5` 0.10 in a separate change; until then this plan is the place that records the extra crate.
- Beatport candidates are always created with `Quality::default()`-shaped values (lossless flag `false`, all numeric fields `None`) unless a `probe` has already run; the real quality is written only after `probe` measure it, per core Task 11.
- All three sources are registered unconditionally in `sources::registry`, and `registry` returns every implemented source without filtering; whether a source runs is decided by `config.sources.enabled` through the core `sources::enabled_in` and by the credentials section, never by the network state at build time. An unreachable source returns an empty result and an `Expired`/`Ok` status with a Russian `detail`.
- Fixtures live under `tests/fixtures/{tidal,deezer,beatport}/` and are committed. They contain no live tokens, no `arl` and no password; they are recorded shapes with obviously fake values.

## Review Focus

The spec names these sources as lossy-by-default and blocked on the reference machine; these inputs are implied by it but no test in the core plan covers them. Each has a test pinned to the task that owns the relevant code.

1. **A Tidal public `client_id`/`client_secret` pair that rotated and now answers `invalid_client`.** Expected: the source reports the status «ключ протух» with a hint to paste a fresh pair into `credentials.toml`, `AuthState` becomes `Expired`, and the run continues with the other sources. Pinned to Task 2.
2. **A Tidal track that only exists at quality level 3 (HI_RES) while the account has no HiFi subscription.** Expected: `pick_level` returns `3` (the best tier the track offers, unconditionally), `quality_for_level(3, false)` maps it to CD FLAC (`lossless: true`, `bit_depth: Some(16)`, `sample_rate: Some(44100)`) and the detail says «подписка HiFi не активна, отдано CD-качество», so the candidate is honest about the subscription while it is never claimed as HI_RES. Covered by the `no_hifi_subscription_downgrades_hi_res_to_cd` and `downgrade_is_reported_in_russian` tests in Task 2. Pinned to Task 2.
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
- Test: `tests/source_tidal.rs`

**Interfaces:**
- Consumes: `maxfetch::sources::{AuthState, Capabilities, Query, Source}`, `maxfetch::sources::oauth::{OAuthError, TokenCache, classify_token_failure}`, `maxfetch::model::{Candidate, Container, Locator, Quality, Release, SourceId}`, `maxfetch::config::TidalCredentials`.
- Produces:
  - `maxfetch::sources::tidal::QUALITY_AAC: u8 = 0`, `QUALITY_HIGH_AAC: u8 = 1`, `QUALITY_LOSSLESS: u8 = 2`, `QUALITY_HI_RES: u8 = 3`.
  - `maxfetch::sources::tidal::quality_for_level(level: u8, has_hifi: bool) -> Quality` — maps the level the source received to the quality the account actually gets, rules in Step 4.
  - `maxfetch::sources::tidal::pick_level(available: &[u8], has_hifi: bool) -> Option<u8>` — the best level present in `available`, unconditionally: `3` when `available` contains it, else `2`, else `1`, else `0`; `None` for an empty list. No tier is skipped for the account, the downgrade happens in `quality_for_level`.
  - `maxfetch::sources::tidal::detail_for_level(level: u8, has_hifi: bool) -> Option<String>` — `None` for levels 0 and 1 and for any level on a HiFi account; `Some("подписка HiFi не активна, отдано CD-качество")` for level 3 when `!has_hifi`; `None` for level 2 on a non-HiFi account, because level 2 is CD FLAC either way.
  - `maxfetch::sources::tidal::TidalSource::note_token_failure(&self, status: u16, body: &str) -> anyhow::Error` — the single production branch that runs `classify_token_failure`, moves the state to `Expired` on `StaleKey`, and returns the user-facing Russian error.
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
use maxfetch::model::Container;
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
fn no_hifi_subscription_serves_hi_res_at_cd_quality() {
    let q = quality_for_level(3, false);
    assert!(q.lossless);
    assert_eq!(q.container, Container::Flac);
    assert_eq!(q.bit_depth, Some(16));
    assert_eq!(q.sample_rate, Some(44100));
    assert_eq!(q.bitrate_kbps, None);
}

#[test]
fn no_hifi_subscription_keeps_lossless_at_cd_quality() {
    let q = quality_for_level(2, false);
    assert!(q.lossless);
    assert_eq!(q.container, Container::Flac);
    assert_eq!(q.bit_depth, Some(16));
    assert_eq!(q.sample_rate, Some(44100));
}

#[test]
fn pick_level_takes_the_best_available_tier() {
    assert_eq!(pick_level(&[0, 1, 2, 3], true), Some(3));
    assert_eq!(pick_level(&[0, 1, 2, 3], false), Some(3));
    assert_eq!(pick_level(&[0, 1], true), Some(1));
    assert_eq!(pick_level(&[0], false), Some(0));
    assert_eq!(pick_level(&[], true), None);
}

#[test]
fn downgrade_is_reported_in_russian() {
    assert_eq!(detail_for_level(0, false), None);
    assert_eq!(detail_for_level(1, false), None);
    assert_eq!(detail_for_level(2, false), None);
    assert_eq!(detail_for_level(3, false).as_deref(), Some("подписка HiFi не активна, отдано CD-качество"));
    assert_eq!(detail_for_level(2, true), None);
    assert_eq!(detail_for_level(3, true), None);
}

#[tokio::test]
async fn stale_key_reports_the_rotated_pair() {
    let source = TidalSource::new(maxfetch::config::TidalCredentials { client_id: "old".into(), client_secret: "old".into() });
    // The token endpoint answers the way a rotated pair does: 401 with an
    // invalid_client body, which `classify_token_failure` maps to `StaleKey`.
    let explain = source.note_token_failure(401, r#"{"error":"invalid_client"}"#).to_string();
    assert!(explain.contains("протух"), "{explain}");
    assert!(explain.contains("credentials.toml"), "{explain}");
    assert_eq!(source.auth_state(), AuthState::Expired);
}

#[tokio::test]
async fn a_transport_failure_keeps_the_key_and_never_names_the_host() {
    let source = TidalSource::new(maxfetch::config::TidalCredentials { client_id: "a".into(), client_secret: "b".into() });
    let explain = source.note_token_failure(503, "upstream").to_string();
    assert!(explain.contains("сервис Tidal недоступен"), "{explain}");
    assert!(!explain.contains("протух"), "{explain}");
    assert!(!explain.contains("openapi"), "{explain}");
    assert_eq!(source.auth_state(), AuthState::Ok);
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

`pick_level` returns the maximum of `available`, ignoring `has_hifi` entirely: the account entitlement is not a reason to skip a tier, because the source still receives that stream and simply gets a lower-resolution variant of it from the API. `has_hifi` is consumed only by the display mapping below.

`quality_for_level` maps the level the source received to the quality the account actually gets, so that a tier above the subscription is never reported at face value: level 0 and 1 are AAC (`lossless: false`, `container: Container::M4aAac`, all numeric fields `None`); level 2 is CD FLAC (`bit_depth: Some(16)`, `sample_rate: Some(44100)`) with and without HiFi; level 3 is HI_RES FLAC (`bit_depth: Some(24)`, `sample_rate: Some(48000)`) with HiFi, and without HiFi the provider truncates it to CD, so `quality_for_level(3, false)` is the same CD FLAC as level 2 (`bit_depth: Some(16)`, `sample_rate: Some(44100)`). `sample_rate` and `bitrate_kbps` are left `None` for AAC on purpose: the API does not state them for a lossy stream and the core rule forbids inventing values. The task tests above are the authoritative list of every branch of this mapping.

`detail_for_level(level, has_hifi)` returns `None` for levels 0 and 1, for any level when `has_hifi` is true, and for level 2 without HiFi (CD FLAC is what the subscription gives anyway); it returns `Some("подписка HiFi не активна, отдано CD-качество")` for level 3 without HiFi.

`TidalSource` holds `credentials: TidalCredentials`, `tokens: TokenCache`, `has_hifi: bool` (read from the token response scope containing `r_usr` and a user subscription probe; in this stage default `false` because the plain `client_credentials` token carries no subscription), and `state: std::sync::Mutex<AuthState>`.

`TidalSource::new` sets `AuthState::Missing` with the detail «нет учётных данных Tidal» when `client_id` or `client_secret` is empty; there is no built-in fallback pair, the keys come only from `[tidal]` in `credentials.toml`. With a non-empty pair the state starts as `AuthState::Ok`.

`note_token_failure(status: u16, body: &str) -> anyhow::Error` is the single place that turns a token-endpoint answer into an error and a state transition; production code calls it from the token fetch branch, and the tests above call it directly. It runs on `classify_token_failure(status, body)`: for `OAuthError::StaleKey` it sets the state to `Expired`, invalidates the token cache and returns an error whose message is «Tidal: ключ протух, вставьте новую пару client_id/client_secret в credentials.toml»; for `OAuthError::Network` it leaves the state at `Ok`, invalidates the cache and returns «сервис Tidal недоступен: <причина>». No user-facing string names a host.

`search` and `resolve` call the Tidal catalog endpoints through `reqwest` (no client crate) and map the recorded `track.json`/`search.json` shapes into `Release` (`title`, first artist name, album title, `trackNumber`, `duration` as `f64` seconds, `isrc`, `source: SourceId::Tidal`, `source_item_id` = the track id as string). `candidates` calls `pick_level` over the `mediaMetadata.tags` plus `audioQuality` of the track, then `quality_for_level(level, has_hifi)` and `detail_for_level(level, has_hifi)` and emits a `Candidate` whose `locator` is `Locator::Tidal { track_id, quality: level }`. `download` is implemented in Task 5; this task leaves the trait method unimplemented for Tidal in the sense that the `Source::download` body lands there, see the Interfaces note of Task 5.

On a token-endpoint failure `search`/`resolve`/`candidates` return the error from `note_token_failure`. `classify_token_failure` from Task 1 is the only branch that decides.

- [ ] **Step 5: Declare the module and run the test**

No dependency is added: Tidal is spoken to with `reqwest`, already in the core build, exactly like the other HTTP sources; `tidalrs` is not used.

Add `pub mod tidal;` to `src/sources/mod.rs`.

Run: `cargo test --test source_tidal`
Expected: PASS, 10 tests.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(sources): tidal metadata, quality tiers and stale-key status"
```

---

### Task 3: Deezer source (catalog, arl auth state)

**Files:**
- Create: `src/sources/deezer.rs` (catalog, search and auth half only; the decryption, the download path and the `Source` impl are Task 5)
- Create: `tests/fixtures/deezer/track.json`, `tests/fixtures/deezer/search.json`, `tests/fixtures/deezer/arl_expired.json`
- Modify: `Cargo.toml` (add `deezer-client = "0.1.4"`)
- Test: `tests/source_deezer.rs`

**Interfaces:**
- Consumes: `maxfetch::sources::{AuthState, Capabilities}`, `maxfetch::model::{Candidate, Container, Locator, Quality, Release, SourceId}`, `maxfetch::config::DeezerCredentials`.
- Produces:
  - `maxfetch::sources::deezer::is_arl_rejected(status: u16, body: &str) -> bool` — true for HTTP 403, or a body carrying `"code":4` after whitespace removal, or the substring `Invalid CSRF token`.
  - `maxfetch::sources::deezer::mp3_128_quality() -> Quality` — `Quality { lossless: false, container: Container::Mp3, bit_depth: None, sample_rate: Some(44100), bitrate_kbps: Some(128) }`.
  - `maxfetch::sources::deezer::DeezerSource::new(credentials: DeezerCredentials) -> DeezerSource` with inherent `id(&self) -> SourceId == SourceId::Deezer`, `capabilities(&self) -> Capabilities { audio: true, metadata_only: false, lossless: false, needs_auth: true }` and `auth_state(&self) -> AuthState`.
  - Not produced in this task: `impl Source for DeezerSource`, and therefore `search`, `resolve`, `candidates` and `download` as trait methods. They arrive in Task 5, when the download path exists, so that no trait method is ever a placeholder that only bails. This task covers the `DeezerSource` struct, its state, its inherent accessors, and the `search`/`resolve`/`candidates` logic as inherent methods that Task 5's `impl Source` forwards to.
  - `maxfetch::sources::deezer::DeezerSource::mark_expired_for_test(&self)` — sets the state to `Expired` through the same private setter production code uses when `is_arl_rejected` fires.
  - `maxfetch::config::DeezerCredentials` gains an optional `secret: Option<String>` with `#[serde(default)]`, so that `[deezer] secret` from `credentials.toml` carries the key-derivation secret used in Task 5. Existing `credentials.toml` files without the key still parse; an absent key leaves the encrypted download unable to decrypt, which Task 5 reports as «нет секрета расшифровки».

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
use maxfetch::sources::AuthState;

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
    let source = DeezerSource::new(DeezerCredentials { arl: String::new(), secret: None });
    assert_eq!(source.auth_state(), AuthState::Missing);
    assert_eq!(source.id(), SourceId::Deezer);
}

#[test]
fn present_arl_starts_ok() {
    let source = DeezerSource::new(DeezerCredentials { arl: "FIXTURE_ARL".into(), secret: Some("FIXTURE_SECRET".into()) });
    assert_eq!(source.auth_state(), AuthState::Ok);
    assert!(!source.capabilities().lossless);
}

#[test]
fn rejected_arl_turns_the_source_expired() {
    let source = DeezerSource::new(DeezerCredentials { arl: "FIXTURE_ARL".into(), secret: Some("FIXTURE_SECRET".into()) });
    source.mark_expired_for_test();
    assert_eq!(source.auth_state(), AuthState::Expired);
}
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test --test source_deezer`
Expected: FAIL with "unresolved import `maxfetch::sources::deezer`".

- [ ] **Step 4: Implement the catalog half of `src/sources/deezer.rs`**

`is_arl_rejected` strips all whitespace from `body`, then returns true for `status == 403`, for a body containing `"code":4`, or for the literal `Invalid CSRF token`. `DeezerSource` holds `credentials: DeezerCredentials` and `state: std::sync::Mutex<AuthState>`; `mark_expired_for_test` is the test hook for the same private setter production code uses when `is_arl_rejected` fires.

`DeezerSource` gets inherent `id`, `capabilities` and `auth_state` so the struct is usable before the trait impl exists. `search`/`resolve` (inherent methods at this stage) map `search.json`/`track.json` to `Release` (`title`, `artist.name`, `album.title`, `track_position`, `duration` as seconds, `isrc`, `source: SourceId::Deezer`, `source_item_id` = id). `candidates` returns one `Candidate` with `quality_for_tier(false)` from Task 4 and `locator: Locator::Deezer { track_id }`. A call that gets `is_arl_rejected(...) == true` sets `Expired` and returns an error whose message is exactly «Deezer: нужна авторизация (cookie arl протухла)».

The `Source` trait is not implemented in this task: the trait requires `download`, and Deezer's `download` only becomes meaningful together with the decryption path of Task 5. Task 3 therefore ships the struct, the auth state machine and the catalog logic; Task 5 adds `impl Source for DeezerSource` and forwards `search`/`resolve`/`candidates` to the inherent methods plus the real `download`.

- [ ] **Step 5: Add the dependency, declare the module and run the test**

```bash
cargo add deezer-client@0.1.4
```
Expected: `Cargo.toml` lists `deezer-client = "0.1.4"` and `cargo build` succeeds. Decision, not an option: `deezer-client` 0.1.4 is the crate for the Deezer catalog calls, exactly as spec §8.7 names it, and its public API is the one `search`/`resolve` call. The only Deezer call that does not go through the crate is the encrypted-track fetch in Task 5, which reads raw bytes with `reqwest` against the same origin and the same `arl` cookie; the catalog client is not replaced.

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
- Modify: `src/sources/deezer.rs` (add `impl Source` and the real `download`)
- Test: `tests/deezer_crypto.rs`, `tests/source_deezer_download.rs`

**Interfaces:**
- Consumes: `blowfish` 0.10 (`Blowfish`, `cipher::{BlockDecryptMut, KeyIvInit, block_padding::NoPadding}`), `md-5` 0.10, `maxfetch::fetch::ProgressSink`, `maxfetch::sources::{AuthState, Capabilities, Query, Source}`.
- Produces:
  - `maxfetch::sources::deezer_crypto::derive_key(track_id: &str, secret: &str) -> [u8; 16]` — the first 16 hexadecimal characters of `md5("<track_id><secret>")`, taken as 16 ASCII bytes; the exact rule is fixed in Step 4.
  - `maxfetch::sources::deezer_crypto::decrypt_track(key: &[u8; 16], iv: &[u8; 8], ciphertext: &[u8]) -> anyhow::Result<Vec<u8>>` — Blowfish-CBC with no padding removal; fails when `ciphertext.len() % 8 != 0`.
  - `maxfetch::sources::deezer_crypto::encrypt_track(key: &[u8; 16], iv: &[u8; 8], plaintext: &[u8]) -> anyhow::Result<Vec<u8>>` — the inverse, used only by the fixture-generating test.
  - `maxfetch::sources::deezer_crypto::decrypted_looks_like_mp3(bytes: &[u8]) -> bool` — true when the first bytes are `ID3`, or when a frame sync `0xFF 0xEx`/`0xFF 0xFx` appears within the first 4 KiB.
  - `DeezerSource::download_fixture(&self, ciphertext: &Path, key: &[u8; 16], iv: &[u8; 8], dest: &Path) -> anyhow::Result<()>` — reads `ciphertext`, decrypts and writes through the same path `download` uses, so the failure semantics are testable without a network.
  - `impl Source for DeezerSource` in this task, forwarding `search`/`resolve`/`candidates` to the inherent methods written in Task 3 and implementing `download` with the `arl`-and-decryption path described in Step 5.
  - `DeezerSource::download_rejected_for_test(&self, status: u16, body: &str, dest: &Path) -> anyhow::Result<()>` — drives the same arl-rejected branch the real `download` takes when the track fetch answers with a rejected `arl`, without a network.
  - `DeezerCredentials` gains an optional `secret: Option<String>` (`#[serde(default)]`) so that `[deezer] secret` from `credentials.toml` carries the key-derivation secret; an absent key means the encrypted download cannot be decrypted and the source reports it as such. Existing `credentials.toml` files without the key still parse.

- [ ] **Step 1: Create the fixtures**

Generate and commit a deterministic ciphertext built from a synthetic MP3 head, not tied to any real track. The fixtures are produced by an ordinary (not ignored) test, which both writes the three files and verifies that decrypting what it wrote yields the plaintext back. Chosen mechanism: the committed files live in `tests/fixtures/deezer/`, and the generating test rewrites them byte-for-byte every run, so the fixtures in git and the fixtures at test time are provably the same.

`tests/deezer_crypto.rs` holds the generator:

```rust
const FIXTURE_KEY: [u8; 16] = [0x11; 16];
const FIXTURE_IV: [u8; 8] = [0u8; 8];

fn synthetic_mp3() -> Vec<u8> {
    let mut plain = vec![b'I', b'D', b'3', 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
    while plain.len() < 4096 {
        plain.extend_from_slice(&[0xFF, 0xFB, 0x90, 0x64]);
    }
    plain.truncate(4096);
    plain
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/deezer")
}

#[test]
fn fixtures_are_reproducible_and_decrypt_back() {
    let dir = fixture_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let plain = synthetic_mp3();
    let ciphertext = encrypt_track(&FIXTURE_KEY, &FIXTURE_IV, &plain).unwrap();
    assert_eq!(ciphertext.len() % 8, 0);
    std::fs::write(dir.join("encrypted_track_plain.bin"), &plain).unwrap();
    std::fs::write(dir.join("encrypted_track.bin"), &ciphertext).unwrap();
    std::fs::write(dir.join("encrypted_track.key"), FIXTURE_KEY).unwrap();
    assert_eq!(decrypt_track(&FIXTURE_KEY, &FIXTURE_IV, &ciphertext).unwrap(), plain);
    assert!(plain.starts_with(b"ID3"));
}
```

The fixtures are generated by `cargo test --test deezer_crypto` itself, on any run; the three files are then committed once with the rest of the task. `encrypted_track.bin` must remain a multiple of 8 bytes, which the test asserts before writing.
Expected: three files exist under `tests/fixtures/deezer/`, and `encrypted_track_plain.bin` begins with the bytes `ID3`.

- [ ] **Step 2: Write the failing tests**

Create `tests/deezer_crypto.rs` (the generator of Step 1 plus the unit tests of the crypto module, in one file):
```rust
use std::path::PathBuf;
use maxfetch::sources::deezer_crypto::{decrypt_track, decrypted_looks_like_mp3, derive_key, encrypt_track};

const FIXTURE_KEY: [u8; 16] = [0x11; 16];
const FIXTURE_IV: [u8; 8] = [0u8; 8];

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(fixture_dir().join(name)).unwrap()
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/deezer")
}

fn synthetic_mp3() -> Vec<u8> { /* as written in Step 1 */ }

#[test]
fn fixtures_are_reproducible_and_decrypt_back() { /* as written in Step 1 */ }

#[test]
fn decryption_matches_the_recorded_plaintext() {
    let out = decrypt_track(&FIXTURE_KEY, &FIXTURE_IV, &fixture("encrypted_track.bin")).unwrap();
    assert_eq!(out, fixture("encrypted_track_plain.bin"));
}

#[test]
fn a_truncated_ciphertext_is_rejected() {
    let mut bad = fixture("encrypted_track.bin");
    bad.pop();
    assert!(decrypt_track(&FIXTURE_KEY, &FIXTURE_IV, &bad).is_err());
}

#[test]
fn key_derivation_is_the_md5_prefix_rule() {
    // md5("3135556FIXTURE_SECRET") = 819ac3ed4ad8b8af17a8860a756d9b0b
    assert_eq!(derive_key("3135556", "FIXTURE_SECRET"), *b"819ac3ed4ad8b8af");
    assert_ne!(derive_key("3135557", "FIXTURE_SECRET"), *b"819ac3ed4ad8b8af");
    assert_ne!(derive_key("3135556", "OTHER_SECRET"), *b"819ac3ed4ad8b8af");
}

#[test]
fn mp3_detection_accepts_id3_and_frame_sync() {
    let plain = fixture("encrypted_track_plain.bin");
    assert!(decrypted_looks_like_mp3(&plain));
    assert!(decrypted_looks_like_mp3(&[0xFF, 0xFB, 0x90, 0x00]));
    assert!(!decrypted_looks_like_mp3(&[0x00, 0x01, 0x02, 0x03]));
}
```

Create `tests/source_deezer_download.rs` — two tests, each pinned to exactly one cause:
```rust
use std::path::PathBuf;
use maxfetch::config::DeezerCredentials;
use maxfetch::model::{Candidate, Locator, Quality, Release, SourceId};
use maxfetch::sources::deezer::DeezerSource;
use maxfetch::sources::{AuthState, Source};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/deezer").join(name)
}

fn candidate() -> Candidate {
    Candidate {
        source: SourceId::Deezer,
        release: Release {
            title: "Get Lucky".into(), artist: "Daft Punk".into(),
            album: Some("Random Access Memories".into()), track_no: Some(8),
            duration_sec: Some(369.0), isrc: Some("FR123400001".into()), year: Some(2013),
            source: SourceId::Deezer, source_item_id: "3135556".into(),
        },
        quality: maxfetch::sources::deezer::mp3_128_quality(),
        size_bytes: None,
        locator: Locator::Deezer { track_id: "3135556".into() },
    }
}

// Review Focus 3: the arl expired between the catalog search and the download.
#[tokio::test]
async fn expired_arl_asks_for_authorization_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.mp3");
    let source = DeezerSource::new(DeezerCredentials { arl: "FIXTURE_ARL".into(), secret: Some("FIXTURE_SECRET".into()) });
    source.mark_expired_for_test();
    let err = source.download(&candidate(), &dest, &maxfetch::fetch::NoopSink).await.unwrap_err();
    let text = format!("{err}");
    assert!(text.contains("нужна авторизация"), "{text}");
    assert!(!dest.exists());
    assert_eq!(source.auth_state(), AuthState::Expired);
}

// Review Focus 4, first half: a wrong key for a genuine ciphertext.
#[tokio::test]
async fn undecryptable_stream_reports_decryption_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.mp3");
    let source = DeezerSource::new(DeezerCredentials { arl: "FIXTURE_ARL".into(), secret: Some("FIXTURE_SECRET".into()) });
    let mut ciphertext = std::fs::read(fixture("encrypted_track.bin")).unwrap();
    for byte in ciphertext.iter_mut().take(64) {
        *byte ^= 0xAA;
    }
    let path = dir.path().join("broken.bin");
    std::fs::write(&path, &ciphertext).unwrap();
    let err = source.download_fixture(&path, &[0x22; 16], &[0u8; 8], &dest).await.unwrap_err();
    let text = format!("{err}");
    assert!(text.contains("не расшифрован"), "{text}");
    assert!(!text.contains("нужна авторизация"), "{text}");
    assert!(!dest.exists());
    assert!(!dest.with_extension("part").exists());
    assert_eq!(source.auth_state(), AuthState::Ok);
}

// Review Focus 4, second half: the fetch itself is rejected by Deezer's arl check.
#[tokio::test]
async fn rejected_arl_during_download_says_authorization_not_decryption() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.mp3");
    let source = DeezerSource::new(DeezerCredentials { arl: "FIXTURE_ARL".into(), secret: Some("FIXTURE_SECRET".into()) });
    let err = source
        .download_rejected_for_test(200, r#"{"error":{"type":"OAuthException","message":"Invalid CSRF token","code":4}}"#, &dest)
        .await
        .unwrap_err();
    let text = format!("{err}");
    assert!(text.contains("нужна авторизация"), "{text}");
    assert!(!text.contains("не расшифрован"), "{text}");
    assert!(!dest.exists());
    assert_eq!(source.auth_state(), AuthState::Expired);
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test --test deezer_crypto --test source_deezer_download`
Expected: FAIL with "unresolved import `maxfetch::sources::deezer_crypto`" and "no method named `download_fixture`".

- [ ] **Step 4: Implement `src/sources/deezer_crypto.rs` and the download path**

`derive_key` is fixed, not a matter of reading a real track: the key is the first 16 hexadecimal characters of `md5("<track_id><secret>")`, taken as 16 ASCII bytes, where `<track_id>` is the numeric Deezer track id from `Locator::Deezer { track_id }` and `<secret>` is `credentials.deezer.secret` read from `[deezer] secret` in `credentials.toml`. `derive_key("3135556", "FIXTURE_SECRET")` therefore equals `*b"819ac3ed4ad8b8af"` (md5 of `3135556FIXTURE_SECRET` is `819ac3ed4ad8b8af17a8860a756d9b0b`) — the test above asserts exactly that vector, so the rule cannot drift. When `secret` is absent or empty, `download` bails before fetching with «Deezer: нет секрета расшифровки (секция [deezer] в credentials.toml)» and leaves no file.

The IV is derived from the key: Blowfish has an 8-byte block, so the 16-byte key is passed to `KeyIvInit::new_from_slices(key, &key[..8])` — the same 16 bytes are the key, and their first 8 bytes serve as the CBC IV. This is the pinned choice for this stage; `key_derivation_is_the_md5_prefix_rule` and `decryption_matches_the_recorded_plaintext` lock both sides of it.

`decrypt_track` uses `<<Blowfish as KeyIvInit>::new_from_slices(key, iv)`, then `decrypt_padded_vec_mut::<NoPadding>`; a length not divisible by 8 is an error, never a padded read. `encrypt_track` is the mirror image with `encrypt_padded_vec_mut::<NoPadding>` and exists only so the fixture test can build its own ciphertext.

`decrypted_looks_like_mp3` checks `starts_with(b"ID3")` first, then scans the first 4096 bytes for `0xFF` followed by a byte whose high nibble is `E` or `F`.

`download` and `download_fixture` share one decrypt-decide-write path, in this order: (1) when the state is `Expired`, bail with «Deezer: нужна авторизация (cookie arl протухла)» before touching the network or `dest`; (2) fetch the ciphertext (or read it from disk in `download_fixture`) and run `is_arl_rejected(status, body)` on the answer — when it is true, set the state to `Expired` and bail with the same authorization message, so a rejected `arl` is never reported as a decryption failure; (3) derive or take the key, decrypt in memory; (4) when `decrypted_looks_like_mp3` is false, bail with «Deezer: трек не расшифрован (неверный ключ или повреждённый поток)»; (5) only now write to `dest.with_extension("part")` and rename it to `dest`, the same atomic pattern as `maxfetch::fetch::download_to_file`. Every bail path leaves neither `dest` nor a `.part` behind.

`download_rejected_for_test(status, body, dest)` enters that path at step (2) with the given status and body, which is how the review-focus test covers a rejected `arl` without a network.

`impl Source for DeezerSource` lands here, forwarding `search`/`resolve`/`candidates` to the inherent methods written in Task 3 and wiring `download` as above.

- [ ] **Step 5: Declare the module and run the tests**

Add `pub mod deezer_crypto;` to `src/sources/mod.rs`.

Run: `cargo test --test deezer_crypto --test source_deezer_download`
Expected: PASS, 5 tests and 3 tests.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(sources): deezer blowfish-cbc decryption with safe download"
```

---

### Task 6: Beatport catalog client and field mapping

**Files:**
- Create: `src/sources/beatport.rs`
- Create: `tests/fixtures/beatport/token.json`, `tests/fixtures/beatport/search_tracks.json`, `tests/fixtures/beatport/track.json`, `tests/fixtures/beatport/release.json`, `tests/fixtures/beatport/release_tracks.json`
- Test: `tests/source_beatport.rs`

**Interfaces:**
- Consumes: `maxfetch::sources::{AuthState, Capabilities, Query, Source}`, `maxfetch::sources::oauth::{OAuthError, TokenCache, classify_token_failure}`, `maxfetch::model::{Candidate, Locator, Quality, Release, SourceId}`, `maxfetch::config::BeatportCredentials`.
- Produces:
  - `maxfetch::sources::beatport::length_ms_to_sec(ms: u64) -> f64`.
  - `maxfetch::sources::beatport::release_from_track_json(v: &serde_json::Value) -> anyhow::Result<Release>` — maps `name`, `mix_name`, `artists[].name`, `length_ms`, `isrc`, `release.name`, `publish_date` into a `Release`.
  - `maxfetch::sources::beatport::release_meta_from_json(v: &serde_json::Value) -> anyhow::Result<ReleaseMeta>` — reads `GET /v4/catalog/releases/{id}/`: `name`, `label.name`, `catalog_number`, `publish_date`, `track_count`, `id`.
  - `maxfetch::sources::beatport::ReleaseMeta` — `struct { id: String, name: String, label: Option<String>, catalog_number: Option<String>, publish_date: Option<String>, track_count: Option<usize> }`, `Debug + Clone + PartialEq`.
  - `maxfetch::sources::beatport::release_tracks_from_json(v: &serde_json::Value) -> anyhow::Result<Vec<Release>>` — reads `GET /v4/catalog/releases/{id}/tracks/` (`results[]`), mapping each element through `release_from_track_json`, and fills `album` from the release name and `track_no` from `number` when the track object omits them.
  - `maxfetch::sources::beatport::display_title(name: &str, mix_name: Option<&str>) -> String`.
  - `maxfetch::sources::beatport::BeatportSource::new(credentials: Option<BeatportCredentials>) -> BeatportSource`.
  - `maxfetch::sources::beatport::BeatportSource::resolve_release(&self, release_id: &str) -> anyhow::Result<(ReleaseMeta, Vec<Release>)>` — calls `/v4/catalog/releases/{id}/` for the metadata and `/v4/catalog/releases/{id}/tracks/` for the tracklist, so album-level output exists instead of only track search.
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
`tests/fixtures/beatport/release.json` — the `GET /v4/catalog/releases/{id}/` shape:
```json
{
  "id": 1001,
  "name": "Random Access Memories",
  "label": { "name": "Daft Life" },
  "catalog_number": "DRL0001",
  "publish_date": "2013-05-17",
  "track_count": 2
}
```
`tests/fixtures/beatport/release_tracks.json` — the `GET /v4/catalog/releases/{id}/tracks/` shape, two elements of the same track object with `number` added:
```json
{
  "results": [
    { "id": 900001, "number": 8, "name": "Get Lucky", "mix_name": "Album Mix", "artists": [{ "name": "Daft Punk" }], "length_ms": 369000, "isrc": "FR123400001", "release": { "name": "Random Access Memories", "publish_date": "2013-05-17" } },
    { "id": 900002, "number": 9, "name": "Doin' it Right", "mix_name": null, "artists": [{ "name": "Daft Punk" }], "length_ms": 251000, "isrc": "FR123400002", "release": { "name": "Random Access Memories", "publish_date": "2013-05-17" } }
  ],
  "per_page": 25
}
```
Expected: five files exist under `tests/fixtures/beatport/`.

- [ ] **Step 2: Write the failing test**

Create `tests/source_beatport.rs`:
```rust
use maxfetch::model::{Container, SourceId};
use maxfetch::sources::beatport::{display_title, length_ms_to_sec, release_from_track_json, release_meta_from_json, release_tracks_from_json, BeatportSource};
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
fn release_json_maps_to_release_metadata() {
    let meta = release_meta_from_json(&fixture_json("release.json")).unwrap();
    assert_eq!(meta.id, "1001");
    assert_eq!(meta.name, "Random Access Memories");
    assert_eq!(meta.label.as_deref(), Some("Daft Life"));
    assert_eq!(meta.catalog_number.as_deref(), Some("DRL0001"));
    assert_eq!(meta.publish_date.as_deref(), Some("2013-05-17"));
    assert_eq!(meta.track_count, Some(2));
}

#[test]
fn release_tracks_json_maps_to_an_album_tracklist() {
    let tracks = release_tracks_from_json(&fixture_json("release_tracks.json")).unwrap();
    assert_eq!(tracks.len(), 2);
    assert_eq!(tracks[0].title, "Get Lucky (Album Mix)");
    assert_eq!(tracks[0].track_no, Some(8));
    assert_eq!(tracks[0].album.as_deref(), Some("Random Access Memories"));
    assert_eq!(tracks[0].source_item_id, "900001");
    assert_eq!(tracks[1].title, "Doin' it right");
    assert_eq!(tracks[1].track_no, Some(9));
    assert_eq!(tracks[1].isrc.as_deref(), Some("FR123400002"));
    assert_eq!(tracks[1].duration_sec, Some(251.0));
    assert!(tracks.iter().all(|t| t.source == SourceId::Beatport));
}

#[test]
fn release_json_without_a_name_is_an_error() {
    let v = serde_json::json!({ "id": 1001 });
    assert!(release_meta_from_json(&v).is_err());
    let v = serde_json::json!({ "results": [{ "mix_name": "x" }] });
    assert!(release_tracks_from_json(&v).is_err());
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

`BeatportSource` holds `credentials: Option<BeatportCredentials>`, `tokens: TokenCache` and `state: std::sync::Mutex<AuthState>`. The token fetch posts `grant_type=client_credentials`, `client_id`, `client_secret` to the OAuth token endpoint and deserializes `TokenResponse`. The pair comes only from `[beatport] client_id`/`client_secret` in `credentials.toml`; there is no built-in or published pair in the code, and the user pastes the pair there themselves. When the section is absent or either field is empty, `new` sets `AuthState::Missing` with the detail «нет учётных данных Beatport» and `search`/`resolve`/`resolve_release` bail with the same sentence without any network call. `classify_token_failure` on a 400/401 sets `Expired` and makes the methods bail with a message containing «ключ протух» and the hint to put a fresh pair into `credentials.toml`.

`release_from_track_json` requires the `id`, `name` and `artists` keys and errors on absence. `year` comes from the first four characters of `release.publish_date` parsed as `i32` (also accept `new_release_date` when `publish_date` is absent). `release.label.name`, `catalog_number`, `bpm` and `key.camelot_*` are read for the log line and are not part of `Release`.

`release_meta_from_json` requires `id` and `name`; `label.name`, `catalog_number`, `publish_date` and `track_count` are optional and pass through as `None` when the API omits them. `release_tracks_from_json` reads the `results` array and maps each element with `release_from_track_json`; when the element carries `number` it becomes `track_no`, and when it lacks `release.name` the album name of the enclosing release is used — otherwise the album comes from the track object itself.

`search` calls `GET /v4/catalog/search/?q=&type=tracks`, `resolve` calls `GET /v4/catalog/tracks/{id}/`, and both map results through `release_from_track_json`. `resolve_release(release_id)` calls `GET /v4/catalog/releases/{id}/` for the metadata and `GET /v4/catalog/releases/{id}/tracks/` for the tracklist, returning the pair, which is how album-level output is produced from an album id. `candidates` reads the `audio_format` field of the fetched track JSON; when it is absent or `null` the candidate gets the unknown quality of Task 7 (`unknown_quality()`), never a guess.

- [ ] **Step 5: Declare the module and run the test**

Add `pub mod beatport;` to `src/sources/mod.rs`.

Run: `cargo test --test source_beatport`
Expected: PASS, 8 tests.

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
- Consumes: `maxfetch::config::BeatportCredentials`, `maxfetch::fetch::ProgressSink`, `reqwest`.
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
- Consumes: `maxfetch::sources::{registry, enabled_in, AuthState, Source, SourceStatus}`, `maxfetch::config::{Config, Credentials, TidalCredentials, DeezerCredentials, BeatportCredentials}`.
- Produces:
  - `maxfetch::sources::registry(config: &Config, credentials: &Credentials) -> Vec<Box<dyn Source>>` — returns every registered source and does no filtering at all: Soulseek, YouTube, Spotify, Bandcamp, SoundCloud, Qobuz, Tidal, Deezer, Beatport, in that order, regardless of `config.sources.enabled`. Construction never opens a socket; the token caches live inside the boxes.
  - `maxfetch::sources::enabled_in(ids: &[SourceId], config: &Config) -> Vec<SourceId>` — the single filtering step, called by the pipeline and by the TUI before they use the registry: it returns the entries of `ids` whose `is_enabled(id, config)` is true, preserving the input order. It is defined once in core Task 10 and this stage does not redefine or wrap it; there is no `enabled_ids` and no second selection function.
  - `maxfetch::sources::statuses(sources: &[Box<dyn Source>]) -> Vec<SourceStatus>` — one status per entry, with the Russian `detail` for a source without usable credentials («нет учётных данных Tidal», «нет учётных данных Beatport», «Deezer: нужна авторизация (cookie arl протухла)») or `None`.
  - Derive list the stage-3 credential structs must satisfy, because the registry clones them out of `Credentials` instead of mutating it: `TidalCredentials { client_id: String, client_secret: String }`, `DeezerCredentials { arl: String, secret: Option<String> }` and `BeatportCredentials { client_id: String, client_secret: String, username: String, password: String }` each derive `Clone + Debug + Default + PartialEq + Serialize + Deserialize`. `Clone` is what the registry needs (`credentials.tidal.clone().unwrap_or_default()`), `Default` is what `unwrap_or_default` needs, and `PartialEq` is what the round-trip tests in `tests/config.rs` and the stage-3 tests compare by value. Core Task 3 declares `Clone` on `Config` only; Task 9 adds these derives to the three structs if they are missing, and says so in the commit message. Tidal and Deezer already need the empty-string default for `AuthState::Missing`.
  - `maxfetch::auth::login(source: &str, credentials: &mut Credentials) -> anyhow::Result<()>` gains real branches: `"tidal"` asks for a `client_id`/`client_secret` pair on stdin and stores it; `"deezer"` asks for the `arl` cookie and the `[deezer] secret` used for key derivation; `"beatport"` asks for `client_id`/`client_secret`/`username`/`password`. Every branch prints its prompt in Russian and explains that the pair is the user's own.

- [ ] **Step 1: Write the failing test**

Create `tests/sources_registry_stage3.rs`:
```rust
use maxfetch::config::{BeatportCredentials, Config, Credentials, DeezerCredentials, TidalCredentials};
use maxfetch::model::SourceId;
use maxfetch::sources::{enabled_in, registry, AuthState};

fn creds_all() -> Credentials {
    let mut c = Credentials::default();
    c.tidal = Some(TidalCredentials { client_id: "a".into(), client_secret: "b".into() });
    c.deezer = Some(DeezerCredentials { arl: "FIXTURE_ARL".into(), secret: Some("FIXTURE_SECRET".into()) });
    c.beatport = Some(BeatportCredentials { client_id: "a".into(), client_secret: "b".into(), username: String::new(), password: String::new() });
    c
}

fn all_ids() -> Vec<SourceId> {
    vec![
        SourceId::Soulseek,
        SourceId::Youtube,
        SourceId::Spotify,
        SourceId::Bandcamp,
        SourceId::SoundCloud,
        SourceId::Qobuz,
        SourceId::Tidal,
        SourceId::Deezer,
        SourceId::Beatport,
    ]
}

#[test]
fn the_registry_holds_every_source_and_filters_nothing() {
    let mut cfg = Config::default();
    cfg.sources.enabled = vec!["tidal".into(), "deezer".into(), "beatport".into()];
    let ids: Vec<SourceId> = registry(&cfg, &creds_all()).iter().map(|s| s.id()).collect();
    assert_eq!(ids, all_ids());

    // The configured list does not change what the registry returns.
    cfg.sources.enabled = vec!["beatport".into()];
    let again: Vec<SourceId> = registry(&cfg, &creds_all()).iter().map(|s| s.id()).collect();
    assert_eq!(again, all_ids());
}

#[test]
fn enabled_in_is_the_only_filter() {
    let mut cfg = Config::default();
    cfg.sources.enabled = vec!["tidal".into(), "beatport".into()];
    assert_eq!(enabled_in(&all_ids(), &cfg), vec![SourceId::Tidal, SourceId::Beatport]);

    cfg.sources.enabled = vec!["deezer".into()];
    assert_eq!(enabled_in(&all_ids(), &cfg), vec![SourceId::Deezer]);

    // An empty enabled list means "everything enabled", in the input order.
    cfg.sources.enabled = Vec::new();
    assert_eq!(enabled_in(&all_ids(), &cfg), all_ids());
}

#[test]
fn missing_credentials_do_not_remove_the_source() {
    let cfg = Config::default();
    let sources = registry(&cfg, &Credentials::default());
    assert_eq!(sources.len(), all_ids().len());
    let tidal = sources.iter().find(|s| s.id() == SourceId::Tidal).unwrap();
    assert_eq!(tidal.auth_state(), AuthState::Missing);
    let beatport = sources.iter().find(|s| s.id() == SourceId::Beatport).unwrap();
    assert_eq!(beatport.auth_state(), AuthState::Missing);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test sources_registry_stage3`
Expected: FAIL with "unresolved import `maxfetch::sources::enabled_in`" (the registry entries for Tidal, Deezer and Beatport do not exist yet).

- [ ] **Step 3: Implement the registry and the login branches**

`registry` builds the `Vec<Box<dyn Source>>` in the fixed order Soulseek, YouTube, Spotify, Bandcamp, SoundCloud, Qobuz, Tidal, Deezer, Beatport and returns it as is; it never looks at `config.sources.enabled`. `TidalSource` is built from `credentials.tidal.clone().unwrap_or_default()`, `DeezerSource` from `credentials.deezer.clone().unwrap_or_default()`, `BeatportSource` from `credentials.beatport.clone()`. Cloning is why the three structs need the derives listed in Interfaces; the registry never mutates `Credentials`.

`enabled_in` is the filter the pipeline and the TUI apply on top of the registry result, so a disabled source is still visible in the Sources screen with its reason and simply does not participate in a run. It is the one defined in core Task 10: this stage adds no wrapper and no second name, and the pipeline calls it with the ids collected from `registry`.

`auth::login` keeps the core "not implemented" fallback for sources outside this stage and switches on the `source` string for the three stage-3 ones. Each branch writes into `credentials` and calls the existing `save_credentials`, which writes mode 0600. The Tidal and Beatport branches tell the user, in Russian, that the pair is theirs to supply: nothing is prefilled with a published key.

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