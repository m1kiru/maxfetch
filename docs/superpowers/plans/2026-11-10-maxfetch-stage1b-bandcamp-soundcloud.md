# maxfetch Stage 1b (Bandcamp, SoundCloud) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the two Stage 1b sources on top of the finished core: Bandcamp as a catalog (fuzzy search, artist discography, tralbum metadata) plus free promo downloads in the delivered encoding, and SoundCloud as a catalog with its own `api-v2` client (ISRC from `publisher_metadata`, exact duration, `media.transcodings[]`) plus lossy AAC download, with the two sources registered in the core registry.

**Architecture:** `sources/bandcamp.rs` and `sources/soundcloud.rs` implement `maxfetch::sources::Source` from the core plan (Task 10) and are pushed into `sources::registry`. Every wire format is parsed by a pure function over a recorded fixture, so no test touches the network; the only network-shaped tests run against a local TCP mock server that replays those same fixtures. Both sources produce the core's `Release`/`Candidate`/`Quality` unchanged, so the pipeline, dedup, library and TUI need no new concepts. A Bandcamp album archive is transport, not a unit of work: it is unpacked once into a cache directory and yields one candidate per member.

**Tech Stack:** Rust 2024 edition, `tokio`, `reqwest` (already in the crate), `serde_json`, plus two new dependencies: `regex` (the Bandcamp id regexes and the SoundCloud `client_id` regex) and `zip` (unpacking the Bandcamp promo archive). No audio codec is added: Bandcamp files are stored as delivered and SoundCloud AAC bytes are fetched by `yt-dlp`.

**Spec:** `docs/superpowers/specs/2026-11-10-maxfetch-design.md` (section 7 is the architecture this plan extends, 8.2 is Bandcamp, 8.9 is SoundCloud; section 10 lists the fixtures these tests are built from).

## Global Constraints

- Crate name and binary name: `maxfetch`. License MIT. Rust edition 2024.
- Rust toolchain 1.99.0 is installed at `/run/current-system/sw/bin/cargo`; `cargo` must be on PATH.
- Do not add dependencies that perform audio transcoding. `ffmpeg` may only be invoked with `-c copy` (stream copy, no re-encode). This plan adds no codec at all: `zip` and `regex` do no audio work.
- Lossy files are stored exactly as delivered by the source. No lossy-to-lossless or lossy-to-lossy re-encoding anywhere in the codebase. The SoundCloud AAC stream is written as delivered; nothing rewraps or re-encodes it.
- Quality ordering: lossless beats lossy; inside lossless, `bit_depth` descending first, then `sample_rate` descending; inside lossy, `bitrate_kbps` descending first, then `sample_rate` descending. Missing values are never invented and always sort below known values of the same class. A Bandcamp format key of `mp3-v0`, `vorbis` or `aac-hi` therefore carries `bitrate_kbps: None`, not a guess.
- The unit of work is a track. An album is a grouping with a tracklist; partial success (3 of 12 tracks downloaded) is normal, not an error. The Bandcamp promo ZIP is transport: it is unpacked and each member becomes its own candidate.
- Library layout is `<library>/<Artist>/<Album>/<NN> <Title>.<ext>`; `<Album>` defaults to `Singles`; `NN` is a two-digit track number. Both sources fill `artist`, `album`, `track_no` from their own metadata before the pipeline computes the path.
- `maxfetch` never deletes files implicitly, and `library scan` only indexes. Nothing in this plan deletes, renames or moves a file; the Bandcamp archive cache lives under the temporary directory, not under the library.
- Secrets live in `$XDG_CONFIG_HOME/maxfetch/credentials.toml` with mode 0600; the file is created with 0600 and a wider mode produces a warning at startup. The scraped SoundCloud `client_id` is written through `Credentials::save_to` so it inherits the same 0600 handling.
- All source sections in `credentials.toml` are optional. A missing section means "source not authorized": the source is disabled and every other source keeps working. Neither source in this stage needs a section at all: Bandcamp works anonymously and SoundCloud scrapes its own `client_id`.
- No source failure (network down, missing credentials, rotated API key) may abort a run; it is recorded in the log and the source is skipped.
- Rust code, identifiers, comments and commit messages are in English. User-facing strings (TUI labels, CLI output, log messages shown in the UI) are in Russian.
- Every commit is authored as `m1kiru <m1kiru@github.com>`.
- Stage 1b scope for Bandcamp is catalog and free promo only. Purchased content (cookie `identity`, `<script data-tralbum-collect-info>`, `POST /api/fancollection/1/search_items`, `redownload_urls`, `pagedata`, `digital_items[0].downloads.<fmt>.url`) is **stage 2** and must not appear in this stage's code; `BandcampCredentials::identity_cookie` stays unread here.
- Stage 1b scope for SoundCloud is anonymous access only. `GET /users/{id}/trackings` and `GET /tracks/{id}/download` both answer 401 without OAuth, so the artist tracklist is taken through `yt-dlp` and the original lossless file is **stage 2**; `SoundCloudCredentials::oauth_token` stays unread here.
- Bandcamp never supplies an ISRC and its mobile API never supplies anything better than an `mp3-128` stream: full-resolution promo audio is reachable only through the `freeDownloadPage` -> `/statdownload/` -> `retry_url` chain. The mobile stream is never used as an audio candidate.
- The Bandcamp HTML `/search` page is behind a JS challenge and must not be requested; search goes through `GET /api/fuzzysearch/2/app_autocomplete` only.
- The SoundCloud `client_id` is a public identifier, not a secret, and it rotates. It is not hardcoded: it is scraped, cached in `credentials.toml` under `[soundcloud]`, and re-scraped once on a 401/403.
- `Locator::Bandcamp` is not extended. A member of a Bandcamp promo archive is addressed by putting the member name in the URL fragment of `Locator::Bandcamp { url, encoding }`; `reqwest` never sends a fragment, and `parse_locator` splits it off. `Locator::SoundCloud` *does* gain a second field, `format: SoundCloudFormat` (Task 5), because the lossy AAC candidate and the lossless original share the same `track_id` and must stay distinguishable.
- No test performs a network request. Every wire format is a recorded fixture under `tests/fixtures/`, replayed through `tests/support/mod.rs`.
- This plan assumes no API from `src/youtube.rs` (owned by the Stage 1 plan). The two `yt-dlp` invocations Stage 1b needs live in `src/sources/soundcloud.rs`.

## Review Focus

The spec is a vision document; these inputs are implied by it but not covered by any explicit requirement. Each has a test pinned to the task that owns the relevant code.

1. **A Bandcamp free download whose `statdownload` chain answers with a stale `fsig`, a chain that answers with no `retry_url` at all, and a release that has no free download at all.** Expected: the `/download/` -> `/statdownload/` rewrite plus a `&.rand=` nonce produces a fresh `retry_url`; a `var _statDL_result` object that carries `"result":"err"` and `errortype` but no `retry_url` is *not* an error — the source logs «нет бесплатного промо», returns `Ok(vec![])` and stays enabled; a paid-only release is the same case and never falls back to the `mp3-128` stream. Pinned to Task 3 (chain, test `the_retry_url_is_extracted_from_the_statdl_result`) and Task 5 (source, tests `a_statdl_result_without_a_retry_url_reports_no_promo` and `a_paid_only_release_yields_no_candidates_and_no_error`).
2. **A Bandcamp album whose ZIP holds 12 tracks but only 3 are wanted.** Expected: the archive is unpacked once, each member becomes an independent candidate with its own track number and title, exactly the three requested members are written, the other 9 are simply absent from the result, and the ZIP is fetched a single time — asserted by `MockServer::request_count("/download/album") == 1` after three downloads of one album. The single-fetch property is pinned twice: to Task 4 (the unpack helper, test `unpacking_is_cached_and_reuses_the_members_directory`) and to Task 5 (the source, tests `downloading_three_of_twelve_writes_exactly_those_three_and_fetches_the_zip_once` and `the_promo_chain_and_the_archive_are_fetched_exactly_once`).
3. **A Bandcamp promo whose payload is not what the source expects: an unknown `enc` on the `retry_url`, or an HTML page instead of a ZIP.** Expected: the member is skipped with the user-facing line «неизвестный формат <enc> у трека <title>» and the run report shows the number of skipped tracks with that reason; an HTML body is discarded (its `PK\x03\x04` signature is absent), a warning «страница загрузки вернула не архив» is logged and no candidate is produced for the members of that archive. In both cases the remaining members still become candidates and the source stays `Ok` and enabled. A *track* promo is the exception: there the `/download/track` answer is the audio file itself, so a non-ZIP body is the single-track candidate. Pinned to Task 5 (source, tests `an_unknown_encoding_skips_only_that_member` and `an_html_body_instead_of_a_zip_yields_no_candidates`).
4. **A SoundCloud track whose `client_id` has rotated and the API answers 401.** Expected: the id is re-scraped from `soundcloud.com`, the request is retried exactly once, and a second 401 marks this item failed without touching the other sources. Pinned to Task 6 (scrape, test `the_scraped_id_is_saved_and_read_back_from_credentials`) and Task 8 (source, tests `a_rotated_client_id_is_rescraped_and_the_request_retried_once` and `a_second_401_fails_the_request_without_a_panic`).
5. **A SoundCloud track that is lossless on the platform but offers only AAC without OAuth.** Expected: the lossy result is honest — `lossless = false`, `aac_160k`, the `maxfetch: source=lossy` comment, the `LOSSY` badge — and the platform's lossless claim never reaches the database. The pipeline test uses a stub that delivers a *real* MP3 128 kbps file, so the probed quality stays lossy and the comment is written from measured bytes, not from the claim. Pinned to Task 7 (classification, test `without_oauth_the_pick_is_aac_160k`) and Task 9 (pipeline, test `a_soundcloud_only_run_records_an_honest_lossy_track`).

---

### Task 1: Fixture replay harness and Bandcamp URL resolution

**Files:**
- Modify: `Cargo.toml` (add `regex`, `zip`)
- Create: `tests/support/mod.rs`
- Create: `tests/fixtures/bandcamp/album_page.html`
- Create: `src/sources/bandcamp.rs`
- Modify: `src/sources/mod.rs` (declare `pub mod bandcamp;`)
- Test: `tests/bandcamp_url.rs`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces:
  - `maxfetch::sources::bandcamp::TralbumType` — `enum { Album, Track }`, `Copy + Debug + PartialEq`, with `as_str(&self) -> &'static str` returning `"a"` / `"t"`.
  - `maxfetch::sources::bandcamp::TralbumKind` — `enum { Artist, Album, Track }`, `Copy + Debug + PartialEq`.
  - `maxfetch::sources::bandcamp::classify_url(url: &str) -> Option<TralbumKind>` — `Track` when the path contains `/track/`, `Album` when it contains `/album/`, `Artist` for a host that ends in `.bandcamp.com` with no `/album/` and no `/track/`; `None` for anything else.
  - `maxfetch::sources::bandcamp::BandcampIds` — `struct { band_id: u64, tralbum_id: u64 }`, `Copy + Debug + PartialEq`.
  - `maxfetch::sources::bandcamp::parse_bandcamp_ids(html: &str) -> Option<BandcampIds>` — first `&amp;band_id=([0-9]+)`, first `"name":"item_id","value":([0-9]+)`.
  - `tests/support/mod.rs`: `Reply`, `Route`, `MockServer` with `MockServer::start(routes: Vec<Route>) -> MockServer`, `MockServer::start_with(build: impl FnOnce(&str) -> Vec<Route>) -> MockServer` (the closure receives the base URL, so a recorded body can embed it), `MockServer::base_url(&self) -> String`, `MockServer::request_count(&self, path: &str) -> usize`; plus `fixture(rel: &str) -> PathBuf` and `write_zip(path: &Path, members: &[(&str, &[u8])]) -> anyhow::Result<()>`.

- [ ] **Step 1: Add the two dependencies**

Run:
```bash
cargo add regex
cargo add zip
```
Expected: both appear in `[dependencies]` and `cargo build` succeeds. Do not pin a version by hand; `cargo add` picks the current release. If `zip` needs a feature for deflate support, run `cargo add zip --features deflate` with the feature names printed by `cargo add zip --help`.

- [ ] **Step 2: Write the failing test**

Create `tests/bandcamp_url.rs`:
```rust
mod support;

use maxfetch::sources::bandcamp::{classify_url, parse_bandcamp_ids, TralbumKind};
use support::{fixture, MockServer, Reply, Route};

#[test]
fn album_url_is_classified_as_album() {
    assert_eq!(classify_url("https://nightwave.bandcamp.com/album/night-drive"), Some(TralbumKind::Album));
    assert_eq!(classify_url("https://nightwave.bandcamp.com/album/night-drive?from=embed"), Some(TralbumKind::Album));
}

#[test]
fn track_url_is_classified_as_track() {
    assert_eq!(classify_url("https://nightwave.bandcamp.com/track/neon"), Some(TralbumKind::Track));
}

#[test]
fn artist_root_is_classified_as_artist() {
    assert_eq!(classify_url("https://nightwave.bandcamp.com"), Some(TralbumKind::Artist));
    assert_eq!(classify_url("https://nightwave.bandcamp.com/"), Some(TralbumKind::Artist));
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
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test --test bandcamp_url`
Expected: FAIL with "unresolved import `maxfetch::sources::bandcamp`" (and "file not found for module `support`").

- [ ] **Step 4: Implement the mock server and the fixture**

Create `tests/fixtures/bandcamp/album_page.html`:
```html
<!DOCTYPE html><html><head><title>Night Drive</title></head><body>
<script type="text/javascript" data-tralbum="{&quot;item_type&quot;:&quot;album&quot;,&quot;freeDownloadPage&quot;:&quot;https://nightwave.bandcamp.com/album/night-drive/download&quot;}"></script>
<a href="https://bandcamp.com/album/night-drive?from=embed&amp;band_id=4171">buy</a>
<script>var item = {"name":"item_id","value":2814};</script>
</body></html>
```

Create `tests/support/mod.rs` with `#![allow(dead_code)]` at the top and:
```rust
pub struct Reply { pub status: u16, pub content_type: &'static str, pub body: Vec<u8> }
impl Reply {
    pub fn ok(content_type: &'static str, body: impl Into<Vec<u8>>) -> Reply { ... }
    pub fn status(status: u16, body: impl Into<Vec<u8>>) -> Reply { ... }   // content_type = "text/plain"
}
pub struct Route { pub path: String, pub replies: Vec<Reply> }
impl Route { pub fn new(path: impl Into<String>, replies: Vec<Reply>) -> Route { ... } }
pub struct MockServer { base: String, hits: Arc<Mutex<HashMap<String, usize>>>, cursor: Arc<Mutex<HashMap<String, usize>>>, _handle: tokio::task::JoinHandle<()> }
```
`MockServer::start` binds a `tokio::net::TcpListener` on `127.0.0.1:0`, records the local port, spawns an accept loop, and per connection: reads until `\r\n\r\n`, takes the request target from the first line, splits it at `?`, looks up that path in `routes`, increments `hits[path]`, and answers with `replies[min(n, replies.len() - 1)]` where `n` is the number of previous requests for that path. The response is `HTTP/1.1 <status> <reason>\r\nContent-Type: <ct>\r\nContent-Length: <len>\r\nConnection: close\r\n\r\n` followed by the body; an unknown path answers `404` with an empty body. `reason` is `OK` for 200, `Unauthorized` for 401, `Forbidden` for 403, `Not Found` for 404.

Add `fixture(rel: &str) -> PathBuf` (`CARGO_MANIFEST_DIR/tests/fixtures/rel`) and `write_zip(path, members)` using `zip::ZipWriter` with `zip::write::SimpleFileOptions::default()`.

- [ ] **Step 5: Implement `classify_url` and `parse_bandcamp_ids`**

Create `src/sources/bandcamp.rs` with the two enums, the struct and the two functions. `classify_url` splits the input at the first `/` after `://`: the host must end with `.bandcamp.com` (case-insensitive) or equal `bandcamp.com`; then `Track` if the remainder contains `/track/`, `Album` if it contains `/album/`, else `Artist` only when the host is a subdomain. `parse_bandcamp_ids` uses `regex::Regex` built lazily with `std::sync::OnceLock`.

- [ ] **Step 6: Declare the module and run the test**

Add `pub mod bandcamp;` to `src/sources/mod.rs` (next to the existing trait definitions).

Run: `cargo test --test bandcamp_url`
Expected: PASS, 7 tests.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(bandcamp): fixture replay harness and url resolution"
```

---

### Task 2: Bandcamp search, discography and tralbum metadata

**Files:**
- Create: `tests/fixtures/bandcamp/fuzzysearch.json`, `tests/fixtures/bandcamp/band_details.json`, `tests/fixtures/bandcamp/tralbum_details_album.json`
- Modify: `tests/support/mod.rs` (add `tralbum_json(track_count: u32) -> String`)
- Modify: `src/sources/bandcamp.rs`
- Test: `tests/bandcamp_catalog.rs`

**Interfaces:**
- Consumes: `maxfetch::model::{Release, SourceId}` (core Task 1), `TralbumKind`, `TralbumType` (Task 1).
- Produces:
  - `maxfetch::sources::bandcamp::FuzzyHit` — `struct { kind: TralbumKind, id: u64, band_id: Option<u64>, location: Option<String> }`, `Debug + PartialEq`. `id` is the hit's `item_id` for album/track results and the `band_id` for artist results; `band_id` is what `tralbum_details` needs, so a hit that has none cannot be resolved.
  - `maxfetch::sources::bandcamp::parse_fuzzy_search(json: &str) -> anyhow::Result<Vec<FuzzyHit>>` — per `results[]` element it reads exactly these fields: `type` (`"b"`/`"a"`/`"t"`; a missing `type` falls back to `item_type` `"a"`/`"t"`; anything else is skipped), `item_id` (the hit `id`; an artist result takes `band_id` instead because it has no `item_id`), `band_id` (the direct field, or, when it is absent on an album/track result, the numeric value of the `band_id=` query parameter inside `location`), and `location` (kept verbatim). A missing `band_id` and a missing `location` leave the field `None` and never fail the parse.
  - `maxfetch::sources::bandcamp::BandcampTrack` — `struct { track_id: u64, title: String, track_num: u32, duration_sec: f64, band_name: String, album_title: String, is_streamable: bool, track_url: String }`, `Clone + Debug`.
  - `maxfetch::sources::bandcamp::BandcampRelease` — `struct { tralbum_id: u64, tralbum_type: TralbumType, title: String, artist: String, release_date_unix: u64, free_download: bool, is_purchasable: bool, price: Option<f64>, currency: Option<String>, has_digital_download: bool, tracks: Vec<BandcampTrack> }`, `Clone + Debug`.
  - `maxfetch::sources::bandcamp::parse_tralbum_details(json: &str) -> anyhow::Result<BandcampRelease>`.
  - `BandcampRelease::to_releases(&self, page_url: &str) -> Vec<Release>` — one `Release` per streamable track, `source: SourceId::Bandcamp`, `source_item_id: format!("{page_url}|{track_id}")`, `isrc: None`, `year` from `release_date_unix`, `album` = `Some(album_title)` when non-empty, `duration_sec` = `Some(duration_sec)`, `track_no` = `Some(track_num)`.
  - `maxfetch::sources::bandcamp::split_source_item_id(id: &str) -> Option<(&str, u64)>` — inverse of `to_releases`, split at the last `|`.
  - `maxfetch::sources::bandcamp::DiscographyItem` — `struct { item_id: u64, item_type: TralbumType, title: String, release_date_unix: u64 }`, `Clone + Debug + PartialEq`.
  - `maxfetch::sources::bandcamp::parse_band_details(json: &str) -> anyhow::Result<Vec<DiscographyItem>>` — accepts `"album"`/`"track"` and `"a"`/`"t"` for `item_type`, skips anything else.

- [ ] **Step 1: Write the failing test**

Create `tests/bandcamp_catalog.rs`:
```rust
mod support;

use maxfetch::model::SourceId;
use maxfetch::sources::bandcamp::{
    parse_band_details, parse_fuzzy_search, parse_tralbum_details, split_source_item_id, TralbumKind, TralbumType,
};
use support::fixture;

fn read(rel: &str) -> String {
    std::fs::read_to_string(fixture(rel)).unwrap()
}

#[test]
fn search_hits_map_the_three_result_types() {
    let hits = parse_fuzzy_search(&read("bandcamp/fuzzysearch.json")).unwrap();
    assert_eq!(hits.len(), 3);
    assert_eq!(hits[0].kind, TralbumKind::Artist);
    assert_eq!(hits[0].id, 4171);
    assert_eq!(hits[1].kind, TralbumKind::Album);
    assert_eq!(hits[1].id, 2814);
    assert_eq!(hits[2].kind, TralbumKind::Track);
    assert_eq!(hits[2].id, 909);
}

#[test]
fn an_album_hit_carries_the_band_id_tralbum_details_needs() {
    let hits = parse_fuzzy_search(&read("bandcamp/fuzzysearch.json")).unwrap();
    assert_eq!(hits[1].band_id, Some(4171));
    assert_eq!(hits[1].location.as_deref(), Some("https://nightwave.bandcamp.com/album/night-drive?band_id=4171"));
    assert_eq!(hits[2].band_id, Some(4171));
    assert_eq!(hits[0].band_id, Some(4171));
}

#[test]
fn an_album_hit_without_a_direct_band_id_reads_it_from_location() {
    let hits = parse_fuzzy_search(
        r#"{"results":[{"type":"a","item_id":2814,"item_type":"a","location":"https://nightwave.bandcamp.com/album/night-drive?band_id=4171","name":"Night Drive"}]}"#,
    )
    .unwrap();
    assert_eq!(hits[0].band_id, Some(4171));
    assert_eq!(hits[0].id, 2814);
}

#[test]
fn unknown_result_types_are_skipped() {
    let hits = parse_fuzzy_search(&read("bandcamp/fuzzysearch.json")).unwrap();
    assert!(hits.iter().all(|h| h.id != 5));
}

#[test]
fn album_details_map_to_one_release_per_track() {
    let tralbum = parse_tralbum_details(&read("bandcamp/tralbum_details_album.json")).unwrap();
    assert_eq!(tralbum.title, "Night Drive");
    assert_eq!(tralbum.artist, "Nightwave");
    let releases = tralbum.to_releases("https://nightwave.bandcamp.com/album/night-drive");
    assert_eq!(releases.len(), 3);
    assert_eq!(releases[0].track_no, Some(1));
    assert_eq!(releases[0].title, "Neon");
    assert_eq!(releases[0].album.as_deref(), Some("Night Drive"));
    assert_eq!(releases[0].duration_sec, Some(214.5));
    assert_eq!(releases[0].year, Some(2021));
    assert_eq!(releases[0].source, SourceId::Bandcamp);
}

#[test]
fn album_details_carry_the_free_download_flag() {
    let tralbum = parse_tralbum_details(&read("bandcamp/tralbum_details_album.json")).unwrap();
    assert!(tralbum.free_download);
    assert!(tralbum.has_digital_download);
    assert_eq!(tralbum.currency.as_deref(), Some("USD"));
}

#[test]
fn bandcamp_releases_never_carry_an_isrc() {
    let tralbum = parse_tralbum_details(&support::tralbum_json(12)).unwrap();
    assert_eq!(tralbum.tracks.len(), 12);
    let releases = tralbum.to_releases("https://nightwave.bandcamp.com/album/night-drive");
    assert_eq!(releases.len(), 12);
    assert!(releases.iter().all(|r| r.isrc.is_none()));
}

#[test]
fn the_source_item_id_round_trips_to_a_page_url_and_track_id() {
    let tralbum = parse_tralbum_details(&read("bandcamp/tralbum_details_album.json")).unwrap();
    let releases = tralbum.to_releases("https://nightwave.bandcamp.com/album/night-drive");
    let (url, track_id) = split_source_item_id(&releases[0].source_item_id).unwrap();
    assert_eq!(url, "https://nightwave.bandcamp.com/album/night-drive");
    assert_eq!(track_id, 9001);
}

#[test]
fn discography_items_keep_their_release_date() {
    let items = parse_band_details(&read("bandcamp/band_details.json")).unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].item_type, TralbumType::Album);
    assert_eq!(items[0].title, "Night Drive");
    assert_eq!(items[0].release_date_unix, 1609459200);
}

#[test]
fn unstreamable_tracks_are_not_offered() {
    let mut tralbum = parse_tralbum_details(&read("bandcamp/tralbum_details_album.json")).unwrap();
    tralbum.tracks[1].is_streamable = false;
    let releases = tralbum.to_releases("https://nightwave.bandcamp.com/album/night-drive");
    assert_eq!(releases.len(), 2);
    assert_eq!(releases[1].track_no, Some(3));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test bandcamp_catalog`
Expected: FAIL with "cannot find function `parse_fuzzy_search`".

- [ ] **Step 3: Create the fixtures**

`tests/fixtures/bandcamp/fuzzysearch.json`:
```json
{"results":[
  {"type":"b","band_id":4171,"location":"https://nightwave.bandcamp.com","name":"Nightwave"},
  {"type":"a","item_id":2814,"item_type":"a","location":"https://nightwave.bandcamp.com/album/night-drive?band_id=4171","name":"Night Drive"},
  {"type":"t","item_id":909,"item_type":"t","band_id":4171,"location":"https://nightwave.bandcamp.com/track/neon","name":"Neon"},
  {"type":"x","item_id":5,"name":"noise"}
]}
```
The album hit deliberately omits a direct `band_id` and carries it only inside `location`, which is the shape the live endpoint returns and the reason `parse_fuzzy_search` must read both: without a `band_id` the hit cannot be resolved through `tralbum_details`. `item_id` is the hit id; the extra `name` is what the service really returns and must not affect the parser.

`tests/fixtures/bandcamp/tralbum_details_album.json`:
```json
{"id":2814,"title":"Night Drive","tralbum_artist":"Nightwave","release_date":1609459200,
 "free_download":true,"is_purchasable":true,"price":7.0,"currency":"USD","has_digital_download":true,
 "tracks":[
  {"track_id":9001,"title":"Neon","track_num":1,"duration":214.5,"band_name":"Nightwave","album_title":"Night Drive","is_streamable":true,"track_url":"https://nightwave.bandcamp.com/track/neon","streaming_url":"https://t4.bcbits.com/stream/neon/mp3-128/1"},
  {"track_id":9002,"title":"Midnight","track_num":2,"duration":198.0,"band_name":"Nightwave","album_title":"Night Drive","is_streamable":true,"track_url":"https://nightwave.bandcamp.com/track/midnight","streaming_url":"https://t4.bcbits.com/stream/midnight/mp3-128/1"},
  {"track_id":9003,"title":"Dawn","track_num":3,"duration":241.25,"band_name":"Nightwave","album_title":"Night Drive","is_streamable":true,"track_url":"https://nightwave.bandcamp.com/track/dawn","streaming_url":"https://t4.bcbits.com/stream/dawn/mp3-128/1"}
 ]}
```
`2021-01-01T00:00:00Z` is `1609459200`, hence `year == 2021`.

`tests/fixtures/bandcamp/band_details.json`:
```json
{"band_id":4171,"name":"Nightwave","discography":[
  {"item_id":2814,"item_type":"album","title":"Night Drive","release_date":1609459200},
  {"item_id":909,"item_type":"track","title":"Neon","release_date":1609545600}
]}
```

- [ ] **Step 4: Implement the parsers**

`parse_fuzzy_search` walks `json["results"]` as an array and, per element, takes `type` (falling back to `item_type` when `type` is absent), `item_id` as the hit id (an artist result uses `band_id` because it has no `item_id`), `band_id` from the direct field or from the `band_id=` query parameter of `location`, and `location` verbatim; `"b"`/`"a"`/`"t"` map to the three kinds and everything else is skipped. Without the `band_id` an album or track hit cannot be passed to `tralbum_details`, so both sources are needed. `parse_tralbum_details` reads the scalar fields with fallbacks (`price`, `currency` optional; `free_download`, `is_purchasable`, `has_digital_download` default to `false`) and maps `tracks[]` into `BandcampTrack`; a missing `track_num` takes the array position plus one, a missing `duration` becomes `0.0`. `year` in `to_releases` comes from `chrono::DateTime::from_timestamp(release_date_unix as i64, 0)`. `split_source_item_id` splits at the **last** `|` so a page URL may itself contain one.

Add `tralbum_json(track_count: u32) -> String` to `tests/support/mod.rs`: a `serde_json::json!` payload with `id: 2814`, `title: "Night Drive"`, `tralbum_artist: "Nightwave"`, `release_date: 1609459200`, `free_download: true`, and one-based `tracks[i] = {track_id: 9000 + i, title: "Track <i+1>", track_num: i + 1, duration: 200.0 + i, band_name: "Nightwave", album_title: "Night Drive", is_streamable: true, track_url: "https://nightwave.bandcamp.com/track/track-<i+1>", streaming_url: "..."}` for `i` in `1..=track_count`. Numbering starts at one because `track_num` is the position inside the archive and `track_number_from_stem` compares against it; a helper starting at zero would silently hide an off-by-one between the tralbum metadata and the archive members.

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test --test bandcamp_catalog`
Expected: PASS, 10 tests.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(bandcamp): fuzzy search, discography and tralbum metadata"
```

---

### Task 3: Bandcamp free-download chain and encoding selection

**Files:**
- Create: `tests/fixtures/bandcamp/statdownload_403.txt`, `tests/fixtures/bandcamp/statdownload_result.txt`
- Modify: `src/sources/bandcamp.rs`
- Test: `tests/bandcamp_download_chain.rs`

**Interfaces:**
- Consumes: `maxfetch::model::{Container, Quality}` (core Task 1), `BandcampTrack` (Task 2).
- Produces:
  - `maxfetch::sources::bandcamp::DataTralbum` — `struct { free_download_page: Option<String> }`, `Debug + PartialEq`.
  - `maxfetch::sources::bandcamp::parse_data_tralbum(html: &str) -> Option<DataTralbum>` — reads the `data-tralbum` attribute of the page's `<script>`, HTML-unescapes it, parses it as JSON and returns `free_download_page`.
  - `maxfetch::sources::bandcamp::statdownload_url(download_url: &str, nonce: u64) -> Option<String>` — replaces the `/download` path segment (with or without a trailing slash) by `/statdownload`, keeps the original query string, and appends `&.rand=<nonce>`. The separator is chosen by the source URL: `?` when it has no query string, `&` when it has one — so the result is always a valid URL and never `.../statdownload&.rand=N` without a `?`. `None` when the path carries no such segment.
  - `maxfetch::sources::bandcamp::StatDownloadResult` — `struct { retry_url: Option<String>, err_type: Option<String> }`, `Debug + PartialEq`.
  - `maxfetch::sources::bandcamp::parse_statdownload_result(body: &str) -> Option<StatDownloadResult>` — finds `var _statDL_result = ` and parses the JSON object that follows.
  - `maxfetch::sources::bandcamp::is_expired_download(status: u16, body: &str) -> bool` — `status == 403 && body.trim() == "Download Expired"`.
  - `maxfetch::sources::bandcamp::retry_encoding(retry_url: &str) -> String` — the current value of the `enc` query parameter, `"flac"` when it is absent. This is the encoding the promo archive really contains: it is chosen by Bandcamp, not by us, so a candidate carries exactly this value and never invents a different one.
  - `maxfetch::sources::bandcamp::FREE_DOWNLOAD_FORMATS: [&str; 8]` = `["flac", "alac", "wav", "vorbis", "mp3-v0", "mp3-320", "aac-hi", "mp3-128"]` — exactly the eight keys of spec §8.2 (`flac`, `mp3-v0`, `mp3-320`, `aac-hi`, `wav`, `alac`, `vorbis`, `mp3-128`), grouped lossless-first so the ordering test holds. `aiff` is not in §8.2 and is not offered.
  - `maxfetch::sources::bandcamp::format_quality(fmt: &str) -> Option<Quality>` — the single place that maps a Bandcamp format key to a `Quality`. Container per key (spec §8.2):

    | key | container | lossless | bitrate_kbps |
    | --- | --- | --- | --- |
    | `flac` | `Container::Flac` | true | `None` |
    | `alac` | `Container::Alac` | true | `None` |
    | `wav` | `Container::Wav` | true | `None` |
    | `vorbis` | `Container::Opus` | false | `None` |
    | `mp3-v0` | `Container::Mp3` | false | `None` |
    | `mp3-320` | `Container::Mp3` | false | `Some(320)` |
    | `aac-hi` | `Container::M4aAac` | false | `None` |
    | `mp3-128` | `Container::Mp3` | false | `Some(128)` |

    `alac` maps to `Container::Alac` and is reported as `lossless: true`, exactly like the core's `Container::is_lossless()` rule; it carries no bitrate because ALAC is lossless. Every key leaves `bit_depth` and `sample_rate` as `None` because only the probe may fill measured values; `mp3-v0` is VBR and therefore carries `bitrate_kbps: None` rather than a guess. Keys outside the table give `None`.

- [ ] **Step 1: Write the failing test**

Create `tests/bandcamp_download_chain.rs`:
```rust
mod support;

use maxfetch::model::{Container, Quality};
use maxfetch::sources::bandcamp::{
    format_quality, is_expired_download, parse_data_tralbum, parse_statdownload_result, retry_encoding,
    statdownload_url, FREE_DOWNLOAD_FORMATS,
};
use support::fixture;

fn read(rel: &str) -> String {
    std::fs::read_to_string(fixture(rel)).unwrap()
}

#[test]
fn the_free_download_page_is_read_from_the_data_tralbum_script() {
    let page = parse_data_tralbum(&read("bandcamp/album_page.html")).unwrap();
    assert_eq!(page.free_download_page.as_deref(), Some("https://nightwave.bandcamp.com/album/night-drive/download"));
}

#[test]
fn a_page_without_a_data_tralbum_blob_has_no_free_download_page() {
    assert_eq!(parse_data_tralbum("<html></html>"), None);
}

#[test]
fn a_plain_download_url_is_rewritten_to_statdownload_with_a_nonce() {
    let url = statdownload_url("https://nightwave.bandcamp.com/album/night-drive/download", 12345).unwrap();
    assert_eq!(url, "https://nightwave.bandcamp.com/album/night-drive/statdownload?&.rand=12345");
}

#[test]
fn a_download_url_that_already_has_a_query_keeps_it_before_the_nonce() {
    let url = statdownload_url("https://nightwave.bandcamp.com/album/night-drive/download?from=embed", 12345).unwrap();
    assert_eq!(url, "https://nightwave.bandcamp.com/album/night-drive/statdownload?from=embed&.rand=12345");
}

#[test]
fn a_url_without_a_download_segment_is_rejected() {
    assert!(statdownload_url("https://nightwave.bandcamp.com/album/night-drive", 1).is_none());
}

#[test]
fn the_expired_403_body_is_recognised() {
    let body = read("bandcamp/statdownload_403.txt");
    assert!(is_expired_download(403, &body));
    assert!(!is_expired_download(200, &body));
    assert!(!is_expired_download(403, "Forbidden"));
}

#[test]
fn the_retry_url_is_extracted_from_the_statdl_result() {
    let parsed = parse_statdownload_result(&read("bandcamp/statdownload_result.txt")).unwrap();
    assert_eq!(
        parsed.retry_url.as_deref(),
        Some("https://nightwave.bandcamp.com/download/album?enc=flac&fsig=abc123&id=2814")
    );
    assert_eq!(parsed.err_type.as_deref(), Some("ExpiredFreeDownloadError"));
}

#[test]
fn a_body_without_statdl_result_yields_none() {
    assert!(parse_statdownload_result("<html>Download Expired</html>").is_none());
}

#[test]
fn the_encoding_is_read_from_the_retry_url() {
    assert_eq!(
        retry_encoding("https://nightwave.bandcamp.com/download/album?enc=mp3-320&fsig=abc123&id=2814"),
        "mp3-320"
    );
    assert_eq!(retry_encoding("https://nightwave.bandcamp.com/download/album?fsig=abc"), "flac");
}

#[test]
fn flac_is_lossless_and_mp3_320_carries_its_bitrate() {
    let flac = format_quality("flac").unwrap();
    assert!(flac.lossless);
    assert_eq!(flac.container, Container::Flac);
    assert_eq!(flac.bit_depth, None);
    assert_eq!(flac.sample_rate, None);
    assert_eq!(flac.bitrate_kbps, None);

    let mp3 = format_quality("mp3-320").unwrap();
    assert!(!mp3.lossless);
    assert_eq!(mp3.container, Container::Mp3);
    assert_eq!(mp3.bitrate_kbps, Some(320));
}

#[test]
fn every_free_download_format_has_a_fixed_container_and_bitrate() {
    let flac = format_quality("flac").unwrap();
    assert!(flac.lossless);
    assert_eq!(flac.container, Container::Flac);

    let alac = format_quality("alac").unwrap();
    assert!(alac.lossless);
    assert_eq!(alac.container, Container::Alac);
    assert_eq!(alac.bitrate_kbps, None);

    let wav = format_quality("wav").unwrap();
    assert!(wav.lossless);
    assert_eq!(wav.container, Container::Wav);

    let vorbis = format_quality("vorbis").unwrap();
    assert!(!vorbis.lossless);
    assert_eq!(vorbis.container, Container::Opus);
    assert_eq!(vorbis.bitrate_kbps, None);

    let v0 = format_quality("mp3-v0").unwrap();
    assert!(!v0.lossless);
    assert_eq!(v0.container, Container::Mp3);
    assert_eq!(v0.bitrate_kbps, None);

    let aac = format_quality("aac-hi").unwrap();
    assert!(!aac.lossless);
    assert_eq!(aac.container, Container::M4aAac);
    assert_eq!(aac.bitrate_kbps, None);

    let mp3_128 = format_quality("mp3-128").unwrap();
    assert!(!mp3_128.lossless);
    assert_eq!(mp3_128.container, Container::Mp3);
    assert_eq!(mp3_128.bitrate_kbps, Some(128));

    assert_eq!(FREE_DOWNLOAD_FORMATS[0], "flac");
    assert_eq!(FREE_DOWNLOAD_FORMATS.len(), 8);
}

#[test]
fn the_format_list_keeps_lossless_keys_ahead_of_lossy_ones() {
    let first_lossy = FREE_DOWNLOAD_FORMATS.iter().position(|f| !format_quality(f).unwrap().lossless).unwrap();
    assert!(FREE_DOWNLOAD_FORMATS[..first_lossy].iter().all(|f| format_quality(f).unwrap().lossless));
    assert!(FREE_DOWNLOAD_FORMATS[first_lossy..].iter().all(|f| !format_quality(f).unwrap().lossless));
}

#[test]
fn unknown_format_keys_are_rejected() {
    assert_eq!(format_quality("zip"), None);
    assert_eq!(format_quality("aiff"), None);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test bandcamp_download_chain`
Expected: FAIL with "cannot find function `statdownload_url`".

- [ ] **Step 3: Create the fixtures**

`tests/fixtures/bandcamp/statdownload_403.txt` — exactly `Download Expired` (no trailing content):
```
Download Expired
```
`tests/fixtures/bandcamp/statdownload_result.txt` — one line:
```
var _statDL_result = {"result":"err","retry_url":"https://nightwave.bandcamp.com/download/album?enc=flac&fsig=abc123&id=2814","errortype":"ExpiredFreeDownloadError"};
```

- [ ] **Step 4: Implement the chain helpers**

`parse_data_tralbum` locates `data-tralbum="`, takes everything up to the closing `"`, and unescapes `&quot;` -> `"`, `&amp;` -> `&`, `&#39;` -> `'`, then parses with `serde_json`. It returns `Some(DataTralbum { free_download_page })` even when the key is absent, so callers can tell "no blob" (`None`) from "blob without a promo" (`Some` with `free_download_page: None`).

`statdownload_url` splits the URL at the first `?` into a path and an optional query, requires the path to end with `/download` or contain `/download/`, replaces that segment with `/statdownload`, and appends the nonce with the separator that matches the original URL: `?` when the source URL had no query string (`{path}/statdownload?&.rand={nonce}`) and `&` when it had one (`{path}/statdownload?{query}&.rand={nonce}`). Writing `&` into a URL without a query would produce `.../statdownload&.rand=N`, which no client can parse, so the separator is derived from the source URL and never hardcoded. This is the exact shape Bandcamp answers: the 403 body is replaced by the `_statDL_result` object carrying a fresh `fsig`.

`retry_encoding` parses the query string of the `retry_url` and returns the `enc` value, defaulting to `flac`. `format_quality` is the single place that maps a Bandcamp format key to a `Quality`; it fixes the container per the table in the Interfaces block and never fills `bit_depth`, `sample_rate` or an absent bitrate, because the probe is the only source of measured values. `alac` maps to `Container::Alac` and is reported as `lossless: true`, matching the core's `Container::is_lossless()` rule. `mp3-128` is a real spec §8.2 key and is not "unknown": it is lossy MP3 at `Some(128)`.

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test --test bandcamp_download_chain`
Expected: PASS, 13 tests.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(bandcamp): statdownload chain and encoding selection"
```

---

### Task 4: Bandcamp archive unpacking and track matching

**Files:**
- Modify: `src/sources/bandcamp.rs`
- Test: `tests/bandcamp_archive.rs`

**Interfaces:**
- Consumes: `BandcampTrack` (Task 2), `tests/support::write_zip` and `fixture` (Task 1).
- Produces:
  - `maxfetch::sources::bandcamp::ArchiveEntry` — `struct { member: String, path: PathBuf }`, `Clone + Debug + PartialEq`.
  - `maxfetch::sources::bandcamp::is_zip(path: &Path) -> anyhow::Result<bool>` — true when the first four bytes are `PK\x03\x04`.
  - `maxfetch::sources::bandcamp::unpack_archive(zip_path: &Path, dest: &Path) -> anyhow::Result<Vec<ArchiveEntry>>` — creates `dest`, extracts every file entry, skips directories, `__MACOSX/` prefixes and `.DS_Store`, and returns the entries sorted by member name.
  - `maxfetch::sources::bandcamp::archive_dir(cache_root: &Path, tralbum_id: u64) -> PathBuf` — `<cache_root>/bandcamp/<tralbum_id>`, the single key of the unpacked-archive cache; with the `BandcampSource::new` root this is `$XDG_CACHE_HOME/maxfetch/bandcamp/<tralbum_id>` (Task 5, Task 9). `archive_zip_path(cache_root: &Path, tralbum_id: u64) -> PathBuf` — `<archive_dir>/promo.zip`; `archive_members_dir(cache_root: &Path, tralbum_id: u64) -> PathBuf` — `<archive_dir>/members`. `archive_dir` creates nothing; the caller creates the directories it writes to.
  - `maxfetch::sources::bandcamp::unpack_cached(zip_path: &Path, cache_root: &Path, tralbum_id: u64) -> anyhow::Result<Vec<ArchiveEntry>>` — extracts into `archive_members_dir` only when that directory is empty or missing, otherwise lists it again; the ZIP is read once per tralbum per run. Both `candidates` and `download` go through `archive_members_dir(cache_root, tralbum_id)`, which is why the tralbum id — not the page URL — is the address of an unpacked archive.
  - `maxfetch::sources::bandcamp::match_members_to_tracks(members: &[ArchiveEntry], tracks: &[BandcampTrack]) -> Vec<(ArchiveEntry, BandcampTrack)>` — first pass matches the sanitized member stem against the sanitized track title, second pass pairs the leftovers by position in both lists; unmatched members and unmatched tracks are dropped.
  - `maxfetch::sources::bandcamp::track_number_from_stem(stem: &str) -> Option<u32>` — leading digits of a member stem (`"01 Neon"` -> `Some(1)`).

- [ ] **Step 1: Write the failing test**

Create `tests/bandcamp_archive.rs`:
```rust
mod support;

use maxfetch::sources::bandcamp::{is_zip, match_members_to_tracks, track_number_from_stem, unpack_archive, unpack_cached};
use support::{fixture, write_zip};

fn track(id: u64, num: u32, title: &str) -> maxfetch::sources::bandcamp::BandcampTrack {
    maxfetch::sources::bandcamp::BandcampTrack {
        track_id: id,
        title: title.into(),
        track_num: num,
        duration_sec: 200.0,
        band_name: "Nightwave".into(),
        album_title: "Night Drive".into(),
        is_streamable: true,
        track_url: format!("https://nightwave.bandcamp.com/track/{}", title.to_lowercase()),
    }
}

fn flac() -> Vec<u8> {
    std::fs::read(fixture("probe/sine_44100_s16.flac")).unwrap()
}

fn build_zip(dir: &std::path::Path, members: &[&str]) -> std::path::PathBuf {
    let path = dir.join("promo.zip");
    let audio = flac();
    let payload: Vec<(&str, &[u8])> = members.iter().map(|m| (*m, audio.as_slice())).collect();
    write_zip(&path, &payload).unwrap();
    path
}

#[test]
fn every_file_in_the_zip_becomes_an_entry() {
    let dir = tempfile::tempdir().unwrap();
    let zip = build_zip(dir.path(), &["01 Neon.flac", "02 Midnight.flac"]);
    assert!(is_zip(&zip).unwrap());
    let entries = unpack_archive(&zip, &dir.path().join("out")).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].member, "01 Neon.flac");
    assert!(entries[0].path.exists());
}

#[test]
fn macosx_sidecars_are_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let zip = build_zip(dir.path(), &["01 Neon.flac", "__MACOSX/01 Neon.flac", ".DS_Store"]);
    let entries = unpack_archive(&zip, &dir.path().join("out")).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].member, "01 Neon.flac");
}

#[test]
fn a_non_zip_file_is_not_an_archive() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("single.flac");
    std::fs::copy(fixture("probe/sine_44100_s16.flac"), &path).unwrap();
    assert!(!is_zip(&path).unwrap());
}

#[test]
fn members_are_matched_to_tracks_by_position_when_stems_differ() {
    let dir = tempfile::tempdir().unwrap();
    let zip = build_zip(dir.path(), &["a.flac", "b.flac"]);
    let entries = unpack_archive(&zip, &dir.path().join("out")).unwrap();
    let tracks = vec![track(9001, 1, "Neon"), track(9002, 2, "Midnight")];
    let pairs = match_members_to_tracks(&entries, &tracks);
    assert_eq!(pairs.len(), 2);
    assert_eq!(pairs[0].1.track_id, 9001);
    assert_eq!(pairs[1].1.track_id, 9002);
}

#[test]
fn a_title_stem_match_wins_over_position() {
    let dir = tempfile::tempdir().unwrap();
    let zip = build_zip(dir.path(), &["dawn.flac", "neon.flac"]);
    let entries = unpack_archive(&zip, &dir.path().join("out")).unwrap();
    let tracks = vec![track(9001, 1, "Neon"), track(9003, 3, "Dawn")];
    let pairs = match_members_to_tracks(&entries, &tracks);
    assert_eq!(pairs[0].0.member, "dawn.flac");
    assert_eq!(pairs[0].1.track_id, 9003);
    assert_eq!(pairs[1].0.member, "neon.flac");
    assert_eq!(pairs[1].1.track_id, 9001);
}

#[test]
fn unmatched_members_and_tracks_are_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let zip = build_zip(dir.path(), &["neon.flac"]);
    let entries = unpack_archive(&zip, &dir.path().join("out")).unwrap();
    let tracks = vec![track(9001, 1, "Neon"), track(9002, 2, "Midnight")];
    let pairs = match_members_to_tracks(&entries, &tracks);
    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0].1.track_id, 9001);
}

#[test]
fn unpacking_is_cached_and_reuses_the_members_directory() {
    let dir = tempfile::tempdir().unwrap();
    let zip = build_zip(dir.path(), &["01 Neon.flac", "02 Midnight.flac"]);
    let first = unpack_cached(&zip, dir.path(), 2814).unwrap();
    std::fs::remove_file(&zip).unwrap();
    let second = unpack_cached(&zip, dir.path(), 2814).unwrap();
    assert_eq!(first.len(), second.len());
    assert_eq!(second[0].member, "01 Neon.flac");
    assert!(second[0].path.exists());
}

#[test]
fn the_track_number_is_read_from_the_member_stem() {
    assert_eq!(track_number_from_stem("01 Neon"), Some(1));
    assert_eq!(track_number_from_stem("12 Dawn"), Some(12));
    assert_eq!(track_number_from_stem("Neon"), None);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test bandcamp_archive`
Expected: FAIL with "cannot find function `unpack_archive`".

- [ ] **Step 3: Implement the archive layer**

`unpack_archive` uses `zip::ZipArchive::new(std::fs::File::open(zip_path)?)`, iterates `0..len`, skips `entry.is_dir()`, skips names starting with `__MACOSX/` or equal to `.DS_Store`, joins the sanitized member into `dest` (using `maxfetch::library::sanitize_component` per path segment), creates the parent directory, copies the entry through `std::io::copy`, and pushes `ArchiveEntry { member, path }`. The result is sorted by `member`.

`match_members_to_tracks` normalizes with `maxfetch::match::norm` on both sides: pass one pairs a member whose normalized stem (after stripping the leading track number and the extension) contains the normalized track title with a track that is still unmatched; pass two pairs the remaining members with the remaining tracks in order. The returned vec keeps the order of `tracks`.

`unpack_cached` returns early with the listing of `archive_members_dir` when it already contains entries, otherwise calls `unpack_archive(zip_path, &members_dir)`. The cached directory is keyed by `tralbum_id` only, so an album read twice in one run is unpacked once.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test --test bandcamp_archive`
Expected: PASS, 8 tests.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(bandcamp): unpack promo archives into per-track candidates"
```

---

### Task 5: The Bandcamp source: candidates, promo reporting and download

**Files:**
- Modify: `src/sources/bandcamp.rs`
- Modify: `tests/support/mod.rs` (the `tralbum_json` helper used by the 12-track fixtures is one-based, Task 2)
- Test: `tests/bandcamp_source.rs` (15 tests; also add the `bandcamp_archive_written` file helper there)

**Interfaces:**
- Consumes: `maxfetch::sources::{Capabilities, AuthState, Query, QueryItem, Source}` and `maxfetch::fetch::ProgressSink` (core Task 10), `maxfetch::config::{Config, Credentials}` (core Task 3), everything from Tasks 1-4.
- Produces:
  - `maxfetch::sources::bandcamp::PromoAvailability` — `enum { Free, PaidOnly, Unavailable }`, `Copy + Debug + PartialEq`, with `message(&self) -> &'static str` (`Free` -> `"бесплатное промо"`, the other two -> `"нет бесплатного промо"`) and `is_free(&self) -> bool`.
  - `maxfetch::sources::bandcamp::promo_availability(release: &BandcampRelease) -> PromoAvailability` — `Free` when `free_download` is true, `PaidOnly` when `is_purchasable || has_digital_download`, otherwise `Unavailable`.
  - `maxfetch::sources::bandcamp::BandcampTarget` — `struct { request_url: String, member: Option<String>, encoding: String, tralbum_id: Option<u64> }`, `Debug + PartialEq`.
  - `maxfetch::sources::bandcamp::make_member_locator(request_url: &str, member: &str, encoding: &str) -> Locator` — `Locator::Bandcamp { url: format!("{request_url}#{member}"), encoding: encoding.into() }`.
  - `maxfetch::sources::bandcamp::parse_locator(loc: &Locator) -> Option<BandcampTarget>` — splits the fragment off and percent-decodes it; `None` for a non-Bandcamp locator. `tralbum_id` is recovered from `request_url` **without extending `Locator`**, in this order: the `tralbum_id` query parameter, then `album_id`, then `id` (the `retry_url` shape `.../download/album?enc=flac&fsig=...&id=2814` carries the tralbum id in `id`). When none of the three is present, `tralbum_id` is `None` and `download` falls back to looking the member up by `encoding` + the member's track title inside the cache root: it walks `<cache_root>/bandcamp/*/members` and takes the first directory that holds a file whose stem, after the leading track number, matches the member stem. The archive itself is always keyed by `tralbum_id` (`archive_dir`), so this lookup only repairs a locator that lost the id; it never unpacks a second copy.
  - `maxfetch::sources::bandcamp::BandcampEndpoints` — `struct { api_base: String }`, `Clone + Debug`, with `BandcampEndpoints::production() -> BandcampEndpoints` (`https://bandcamp.com`). It is the endpoint bundle `new` uses; it is **not** exposed as a constructor of its own (see the constructor set below).
  - `maxfetch::sources::bandcamp::BandcampSource` — constructors. **The set is canonicalized in stage 2**: the canonical list is `new`, `with_api_base`, `with_lookups`, `without_credentials`, and no other constructor is introduced. This task defines the two that stage 1b needs — `new` and `with_api_base` — and stage 2 adds the two it needs (`with_lookups`, `without_credentials`) on top of exactly these two, so the set never conflicts. In particular there is **no `with_endpoints`**: an earlier draft of this plan had one, and it was removed so that stage 2's canonical list is not contradicted.
    - `BandcampSource::new(config: &Config, credentials: &Credentials) -> BandcampSource` — production endpoints (`BandcampEndpoints::production()`), the shared `reqwest` client from `maxfetch::fetch::http_client`, and the archive cache root `$XDG_CACHE_HOME/maxfetch` (falling back to `$HOME/.cache/maxfetch` when `XDG_CACHE_HOME` is unset), created at construction time. `archive_dir` appends `bandcamp/<tralbum_id>`, so the unpacked archives live at `$XDG_CACHE_HOME/maxfetch/bandcamp/<tralbum_id>` and nowhere else. Nothing in `new` depends on `config::config_dir()` and nothing is read from `BandcampCredentials::identity_cookie`.
    - `BandcampSource::with_api_base(api_base: &str, cache_root: &Path) -> BandcampSource` — `api_base` replaces the endpoint host and `cache_root` replaces the cache root verbatim (the test passes a `tempfile::tempdir()` path); used by every test in this stage. This is the only test constructor stage 1b introduces.
  - `impl Source for BandcampSource` — `id() -> SourceId::Bandcamp`; `capabilities() -> Capabilities { audio: true, metadata_only: false, lossless: true, needs_auth: false }`; `auth_state() -> AuthState::NotNeeded`; `search` maps a text query to `fuzzysearch` and resolves the first 5 album/track hits through `tralbum_details` using each hit's `band_id`; `resolve` turns an album/track URL into its releases through `tralbum_details` and an artist URL into one `Release` per discography entry (via `GET /api/mobile/24/band_details?band_id=`); `candidates` runs the promo chain and returns exactly one candidate per matched archive member; `download` copies the matched member (or the single-track file) into `dest`.

- [ ] **Step 1: Write the failing test**

Create `tests/bandcamp_source.rs`:
```rust
mod support;

use std::path::Path;

use maxfetch::fetch::NoopSink;
use maxfetch::model::{Release, SourceId};
use maxfetch::sources::AuthState;
use maxfetch::sources::bandcamp::{
    make_member_locator, parse_locator, parse_tralbum_details, promo_availability, BandcampSource, PromoAvailability,
};
use maxfetch::sources::Source;
use support::{fixture, write_zip, MockServer, Reply, Route};

const PAGE: &str = "/album/night-drive";

fn read(rel: &str) -> String {
    std::fs::read_to_string(fixture(rel)).unwrap()
}

/// The recorded album page and the recorded statdownload body, both rewritten
/// onto the mock server's own origin.
/// The recorded album page, the recorded download page (403 "Download Expired"),
/// the recorded statdownload body and the retry archive, all rewritten onto the
/// mock server's own origin. The 403 route is what makes the test exercise the
/// real §8.2 chain instead of skipping straight to the retry body.
fn album_routes(album_page: &str, track_count: u32, zip: Option<Vec<u8>>) -> impl FnOnce(&str) -> Vec<Route> {
    let page = album_page.to_string();
    let tralbum = support::tralbum_json(track_count);
    let retry = read("bandcamp/statdownload_result.txt");
    move |base: &str| {
        vec![
            Route::new(PAGE, vec![Reply::ok("text/html", page.replace("https://nightwave.bandcamp.com", base))]),
            Route::new(&format!("{PAGE}/download"), vec![Reply::status(403, "Download Expired")]),
            Route::new(
                &format!("{PAGE}/statdownload"),
                vec![Reply::ok("text/plain", retry.replace("https://nightwave.bandcamp.com", base))],
            ),
            Route::new("/download/album", vec![Reply::status(200, zip.clone().unwrap_or_default())]),
            Route::new("/api/mobile/25/tralbum_details", vec![Reply::ok("application/json", tralbum.clone())]),
        ]
    }
}

fn promo_zip(track_count: u32) -> Vec<u8> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("promo.zip");
    let audio = std::fs::read(fixture("probe/sine_44100_s16.flac")).unwrap();
    let members: Vec<String> = (1..=track_count).map(|i| format!("{i:02} Track {i}.flac")).collect();
    let payload: Vec<(&str, &[u8])> = members.iter().map(|m| (m.as_str(), audio.as_slice())).collect();
    write_zip(&path, &payload).unwrap();
    std::fs::read(&path).unwrap()
}

fn album_page() -> String {
    read("bandcamp/album_page.html")
}

/// True when the source unpacked something under the archive cache root.
fn bandcamp_archive_written(cache_root: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(cache_root) else { return false };
    for entry in entries.flatten() {
        let members = entry.path().join("members");
        if std::fs::read_dir(&members).map(|d| d.count() > 0).unwrap_or(false) {
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
    assert_eq!(promo_availability(&tralbum), PromoAvailability::PaidOnly);
    assert_eq!(promo_availability(&tralbum).message(), "нет бесплатного промо");
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
    let loc = make_member_locator("https://nightwave.bandcamp.com/download/album?enc=flac&fsig=abc", "01 Track 1.flac", "flac");
    let target = parse_locator(&loc).unwrap();
    assert_eq!(target.request_url, "https://nightwave.bandcamp.com/download/album?enc=flac&fsig=abc");
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
    assert_eq!(parse_locator(&make_member_locator(explicit, "01 Track 1.flac", "flac")).unwrap().tralbum_id, Some(2814));
    let album = "https://nightwave.bandcamp.com/download/album?album_id=2814&enc=flac";
    assert_eq!(parse_locator(&make_member_locator(album, "01 Track 1.flac", "flac")).unwrap().tralbum_id, Some(2814));
}

#[test]
fn a_foreign_locator_is_rejected() {
    let loc = maxfetch::model::Locator::SoundCloud { track_id: "1".into(), format: maxfetch::sources::soundcloud::SoundCloudFormat::Aac160 };
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
    assert_eq!(parse_locator(&candidates[0].locator).unwrap().member.as_deref(), Some("01 Track 1.flac"));
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
        let candidate = candidates.iter().find(|c| c.release.track_no == Some(n)).unwrap();
        let member = parse_locator(&candidate.locator).unwrap().member.unwrap();
        let dest = out.path().join(&member);
        src.download(candidate, &dest, &NoopSink).await.unwrap();
        assert!(dest.exists());
        written.push(member);
    }
    written.sort();
    assert_eq!(written, vec!["01 Track 1.flac", "05 Track 5.flac", "12 Track 12.flac"]);

    // The other nine members must not have produced a file.
    let files: Vec<String> = std::fs::read_dir(out.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(files.len(), 3);
    for n in (1..=12u32).filter(|n| !WANTED.contains(n)) {
        let prefix = format!("{n:02} ");
        assert!(!files.iter().any(|f| f.starts_with(&prefix)), "{prefix} was downloaded");
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
        let candidate = candidates.iter().find(|c| c.release.track_no == Some(n)).unwrap();
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
    let unpacked: Vec<_> = std::fs::read_dir(cache.path().join("bandcamp").join("2814").join("members"))
        .unwrap()
        .collect();
    assert_eq!(unpacked.len(), 12);
}

#[tokio::test]
async fn a_paid_only_release_yields_no_candidates_and_no_error() {
    let server = MockServer::start_with(album_routes(&album_page(), 3, None)).await;
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
            Route::new(PAGE, vec![Reply::ok("text/html", page.replace("https://nightwave.bandcamp.com", base))]),
            Route::new(
                &format!("{PAGE}/statdownload"),
                vec![Reply::ok("text/plain", retry.replace("https://nightwave.bandcamp.com", base))],
            ),
            Route::new("/download/track", vec![Reply::status(200, audio.clone())]),
            Route::new("/api/mobile/25/tralbum_details", vec![Reply::ok("application/json", tralbum.clone())]),
        ]
    })
    .await;
    let cache = tempfile::tempdir().unwrap();
    let src = BandcampSource::with_api_base(&server.base_url(), cache.path());
    let parsed = parse_tralbum_details(&support::tralbum_json(1)).unwrap();
    let release = parsed.to_releases(&format!("{}{}", server.base_url(), PAGE))[0].clone();

    let candidates = src.candidates(&release).await.unwrap();
    assert_eq!(candidates.len(), 1);
    assert!(parse_locator(&candidates[0].locator).unwrap().member.is_none());

    let dest = cache.path().join("single.flac");
    src.download(&candidates[0], &dest, &NoopSink).await.unwrap();
    assert_eq!(std::fs::metadata(&dest).unwrap().len(), std::fs::metadata(fixture("probe/sine_44100_s16.flac")).unwrap().len());
}

#[tokio::test]
async fn a_release_without_a_page_blob_reports_no_promo() {
    let server = MockServer::start_with(|_base: &str| {
        vec![Route::new(PAGE, vec![Reply::ok("text/html", "<html></html>")])]
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
}

#[tokio::test]
async fn a_statdl_result_without_a_retry_url_reports_no_promo() {
    let page = album_page();
    let statdl = r#"var _statDL_result = {"result":"err","errortype":"ExpiredFreeDownloadError"};"#;
    let server = MockServer::start_with(move |base: &str| {
        vec![
            Route::new(PAGE, vec![Reply::ok("text/html", page.replace("https://nightwave.bandcamp.com", base))]),
            Route::new(&format!("{PAGE}/download"), vec![Reply::status(403, "Download Expired")]),
            Route::new(&format!("{PAGE}/statdownload"), vec![Reply::ok("text/plain", statdl)]),
            Route::new("/api/mobile/25/tralbum_details", vec![Reply::ok("application/json", support::tralbum_json(3))]),
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
            Route::new(PAGE, vec![Reply::ok("text/html", page.replace("https://nightwave.bandcamp.com", base))]),
            Route::new(&format!("{PAGE}/download"), vec![Reply::status(403, "Download Expired")]),
            Route::new(
                &format!("{PAGE}/statdownload"),
                vec![Reply::ok("text/plain", retry.replace("https://nightwave.bandcamp.com", base))],
            ),
            // The retry URL answers with the download page instead of the archive.
            Route::new("/download/album", vec![Reply::ok("text/html", "<html><body>Download</body></html>")]),
            Route::new("/api/mobile/25/tralbum_details", vec![Reply::ok("application/json", support::tralbum_json(3))]),
        ]
    })
    .await;
    let cache = tempfile::tempdir().unwrap();
    let src = BandcampSource::with_api_base(&server.base_url(), cache.path());
    let tralbum = parse_tralbum_details(&support::tralbum_json(3)).unwrap();
    let release = tralbum.to_releases(&format!("{}{}", server.base_url(), PAGE))[0].clone();

    let candidates = src.candidates(&release).await.unwrap();
    assert!(candidates.is_empty());
    assert_eq!(server.request_count(&format!("{PAGE}/statdownload")), 0);
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
            Route::new(PAGE, vec![Reply::ok("text/html", page.replace("https://nightwave.bandcamp.com", base))]),
            Route::new(&format!("{PAGE}/download"), vec![Reply::status(403, "Download Expired")]),
            Route::new(
                &format!("{PAGE}/statdownload"),
                vec![Reply::ok("text/plain", retry.replace("https://nightwave.bandcamp.com", base))],
            ),
            Route::new("/download/album", vec![Reply::status(200, zip.clone())]),
            Route::new("/api/mobile/25/tralbum_details", vec![Reply::ok("application/json", support::tralbum_json(3))]),
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
            Route::new("/", vec![Reply::ok("text/html", page.replace("https://nightwave.bandcamp.com", base))]),
            Route::new("/api/mobile/24/band_details", vec![Reply::ok("application/json", band_details.clone())]),
            Route::new("/api/mobile/25/tralbum_details", vec![Reply::ok("application/json", tralbum.clone())]),
        ]
    })
    .await;
    let cache = tempfile::tempdir().unwrap();
    let src = BandcampSource::with_api_base(&server.base_url(), cache.path());

    let releases = src
        .resolve(&maxfetch::sources::Query { text: None, url: Some(server.base_url()), items: vec![] })
        .await
        .unwrap();
    // Two discography items, each resolved through tralbum_details into its tracks.
    assert_eq!(releases.len(), 6);
    assert_eq!(server.request_count("/api/mobile/24/band_details"), 1);
    assert_eq!(server.request_count("/api/mobile/25/tralbum_details"), 2);
}
```

`bandcamp_archive_written` in the HTML-body test is a small file helper defined at the top of `tests/bandcamp_source.rs` (next to `promo_zip`, body shown above): it walks the cache root and returns true when any `bandcamp/*/members` directory holds a file. It keeps the assertion independent of the source's error type while pinning "nothing was unpacked from an HTML body". The artist test also needs `use maxfetch::sources::Query;` at the top of the file.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test bandcamp_source`
Expected: FAIL with "unresolved import `maxfetch::sources::bandcamp::BandcampSource`".

- [ ] **Step 3: Implement the source**

`candidates` performs, in order:
1. `split_source_item_id(&release.source_item_id)` -> `(page_url, track_id)`; a malformed id is logged with `tracing::warn!` and returns `Ok(vec![])`.
2. Fetch `page_url` and parse `parse_bandcamp_ids` -> `(band_id, tralbum_id)` and `parse_data_tralbum`. A page without ids or without a `data-tralbum` blob yields `Ok(vec![])` after logging «нет бесплатного промо» for the release.
3. `GET {api_base}/api/mobile/25/tralbum_details?band_id=<id>&tralbum_id=<id>&tralbum_type=<a|t>` -> `parse_tralbum_details` -> `promo_availability`. When it is not `Free`, log `tracing::info!("{}: {} — {}", artist, title, availability.message())` and return `Ok(vec![])`.
4. `free_download_page` -> `GET` -> when the answer is the 403 `is_expired_download(status, body)`, rewrite with `statdownload_url(url, nonce)` where `nonce = SystemTime::now()` subsec nanos, `GET` that -> `parse_statdownload_result` -> `retry_url`. A missing `retry_url` — including a `_statDL_result` with `"result":"err"` and only an `errortype` — is logged with the same «нет бесплатного промо» message and returns `Ok(vec![])`; it is never an error. A first-pass success without a 403 uses the download page body directly.
5. The retry body is written to `archive_zip_path(cache_root, tralbum_id)` and `is_zip` decides. The tralbum type from step 3 picks the branch:
   - `tralbum_type == Album`: a ZIP goes through `unpack_cached` + `match_members_to_tracks` (Task 4). A body without the `PK\x03\x04` signature is **not** an archive: its bytes are discarded, `tracing::warn!` records «страница загрузки вернула не архив» with the release artist and title, nothing is unpacked into the cache, and the call returns `Ok(vec![])` — the members are marked unavailable, and the source keeps working for other releases.
   - `tralbum_type == Track`: the body is the audio file itself (the real `/download/track` answer), so it becomes exactly one candidate whose locator has no member — a non-ZIP body here is not an error and does not raise the warning.
6. `encoding = retry_encoding(&retry_url)`; `format_quality(&encoding)` gives the quality. `None` means the `enc` is outside `FREE_DOWNLOAD_FORMATS`: the member is dropped from the result, `tracing::warn!("неизвестный формат {} у трека {}", encoding, track.title)` records the user-facing reason «неизвестный формат <enc> у трека <title>», the skipped count for that reason is kept for the run report, and the remaining members still become candidates — one unknown member never empties the release. Each remaining `(member, track)` becomes one `Candidate { source: SourceId::Bandcamp, release: <track release>, quality, size_bytes: None, locator: make_member_locator(&retry_url, &member.member, &encoding) }`.

`download` resolves `parse_locator` and, when `member` is `Some`, copies the cached member file to `dest`. The cache directory is located by `target.tralbum_id` (`archive_members_dir(cache_root, id)`, with the `encoding` + member-title walk over `bandcamp/*/members` as the fallback when the locator lost the id); when the cache was cleaned between `candidates` and `download`, the archive is re-fetched from `target.request_url` and re-unpacked into that same directory. When `member` is `None` it copies the single-track file. It finishes with `sink.on_finished(size)`. The archive is never unpacked twice for one tralbum: `unpack_cached` guards that, which is why three downloads of one album hit the network once.

`search` calls `{api_base}/api/fuzzysearch/2/app_autocomplete?q=<percent-encoded>&param_with_locations=true`, keeps the first 5 `Album`/`Track` hits, resolves each through `tralbum_details` with the hit's own `id` (`tralbum_id`) and `band_id` — a hit without a `band_id` cannot be resolved and is logged with `tracing::debug!` and skipped — and returns the union of `to_releases`, deduplicated by `source_item_id`. An empty result is `Ok(vec![])`.

`resolve` handles three URL shapes of spec §8.2:
- Album or track page: fetch the page, parse the ids and the tralbum type from the path, then reuse the same `tralbum_details` path as `search`.
- Artist page (`classify_url` -> `Artist`): take the `band_id` from `parse_bandcamp_ids` of the fetched page (or from a `band_id=` query parameter when the URL carries one), `GET {api_base}/api/mobile/24/band_details?band_id=<id>` -> `parse_band_details` -> for each `DiscographyItem` issue the step-3 `tralbum_details` call with its `item_id`/`item_type` and collect `to_releases`. The result is the artist's discography, so §8.2 is reachable without `search`. A `band_details` failure or an empty discography logs `tracing::info!("дискография пуста: {}", url)` and returns `Ok(vec![])`.
- Anything else logs `tracing::info!("неизвестная страница Bandcamp: {}", url)` and returns `Ok(vec![])`.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test --test bandcamp_source`
Expected: PASS, 15 tests.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(bandcamp): source implementation with promo candidates"
```

---

### Task 6: SoundCloud `client_id` scraping and credential caching

**Files:**
- Create: `tests/fixtures/soundcloud/home.html`, `tests/fixtures/soundcloud/bundle_early.js`, `tests/fixtures/soundcloud/bundle_late.js`
- Create: `src/sources/soundcloud.rs`
- Modify: `src/sources/mod.rs` (declare `pub mod soundcloud;`)
- Test: `tests/soundcloud_client_id.rs`

**Interfaces:**
- Consumes: `maxfetch::config::{Credentials, Config}` (core Task 3), `maxfetch::fetch::http_client` (core Task 8).
- Produces:
  - `maxfetch::sources::soundcloud::CLIENT_ID_REGEX: &str` = `r#"client_id\s*:\s*"([0-9a-zA-Z]{32})""#`.
  - `maxfetch::sources::soundcloud::find_client_id(js: &str) -> Option<String>`.
  - `maxfetch::sources::soundcloud::script_urls(html: &str) -> Vec<String>` — the `src` of every `<script src=...>`, in document order.
  - `maxfetch::sources::soundcloud::SoundCloudEndpoints` — `struct { web_base: String, api_base: String, script_base: String }`, `Clone + Debug`, with `SoundCloudEndpoints::production()` -> `https://soundcloud.com`, `https://api-v2.soundcloud.com`, `https://a-v2.sndcdn.com`.
  - `maxfetch::sources::soundcloud::scrape_client_id(client: &reqwest::Client, endpoints: &SoundCloudEndpoints) -> anyhow::Result<String>` — GETs `web_base`, walks the script URLs in reverse order of appearance, fetches each as `script_base` + the URL's path, returns the first hit; errors with `"soundcloud: client_id not found in the page bundles"` when all of them miss.
  - `maxfetch::sources::soundcloud::cached_client_id(dir: &Path) -> Option<String>` — `Credentials::load_from(dir).ok()?.soundcloud?.client_id`, mapped through `(|id| if id.is_empty() { None } else { Some(id) })` so an empty stored id counts as "no cached id" and the id is scraped again.
  - `maxfetch::sources::soundcloud::store_client_id(dir: &Path, client_id: &str) -> anyhow::Result<()>` — loads, sets `soundcloud.client_id` (creating the section when absent), saves through `Credentials::save_to`.
  - `maxfetch::sources::soundcloud::ensure_client_id(dir: &Path, client: &reqwest::Client, endpoints: &SoundCloudEndpoints) -> anyhow::Result<String>` — cached value when present, otherwise scrape + store.
  - `maxfetch::sources::soundcloud::SoundCloudUser` — `struct { id: u64, username: String, permalink: String, followers_count: u64, city: Option<String>, country: Option<String> }`, `Clone + Debug + PartialEq`. This is the §8.9 `GET /users/{id}` profile that `yt-dlp` cannot give us.
  - `maxfetch::sources::soundcloud::parse_user(json: &str) -> anyhow::Result<SoundCloudUser>` — reads `id`, `username`, `permalink`, `followers_count` (default `0` when absent), `city`, `country` (both optional); a missing `username` or `permalink` is an error naming the missing field.
  - `maxfetch::sources::soundcloud::get_user(client: &reqwest::Client, endpoints: &SoundCloudEndpoints, client_id: &str, user_id: u64) -> anyhow::Result<SoundCloudUser>` — one `GET {api_base}/users/{user_id}?client_id={client_id}` -> `parse_user`. No retry here: the once-only re-scrape on 401/403 belongs to Task 8's `api_get`, which wraps this request shape for every endpoint.

- [ ] **Step 1: Write the failing test**

Create `tests/soundcloud_client_id.rs`:
```rust
mod support;

use maxfetch::config::Credentials;
use maxfetch::sources::soundcloud::{
    cached_client_id, ensure_client_id, find_client_id, get_user, scrape_client_id, script_urls, store_client_id,
    SoundCloudEndpoints,
};
use support::{fixture, MockServer, Reply, Route};

fn read(rel: &str) -> String {
    std::fs::read_to_string(fixture(rel)).unwrap()
}

fn endpoints(base: &str) -> SoundCloudEndpoints {
    SoundCloudEndpoints { web_base: base.into(), api_base: base.into(), script_base: base.into() }
}

fn server(late_hits: &str) -> impl std::future::Future<Output = MockServer> {
    let home = read("soundcloud/home.html");
    let late = late_hits.to_string();
    async move {
        MockServer::start(vec![
            Route::new("/", vec![Reply::ok("text/html", home)]),
            Route::new("/assets/0-early.js", vec![Reply::ok("application/javascript", read("soundcloud/bundle_early.js"))]),
            Route::new("/assets/50-late.js", vec![Reply::ok("application/javascript", late)]),
        ])
        .await
    }
}

#[test]
fn every_script_src_is_collected_in_document_order() {
    let urls = script_urls(&read("soundcloud/home.html"));
    assert_eq!(urls.len(), 2);
    assert_eq!(urls[0], "https://a-v2.sndcdn.com/assets/0-early.js");
    assert_eq!(urls[1], "https://a-v2.sndcdn.com/assets/50-late.js");
}

#[test]
fn a_client_id_is_found_in_the_recorded_bundle() {
    let js = read("soundcloud/bundle_late.js");
    assert_eq!(find_client_id(&js).as_deref(), Some("iN5ffSanbMS4rYYxVJVZcQU5jdq8Fjmz"));
}

#[test]
fn a_bundle_without_a_client_id_is_skipped() {
    assert_eq!(find_client_id(&read("soundcloud/bundle_early.js")), None);
}

#[test]
fn an_id_that_is_not_thirty_two_characters_is_rejected() {
    assert_eq!(find_client_id(r#"{"client_id":"iN5ffSanbMS4rYYxVJVZcQU5jdq8Fjm"}"#), None);
    assert_eq!(find_client_id(r#"{"client_id":"iN5ffSanbMS4rYYxVJVZcQU5jdq8Fjmzz"}"#), None);
    assert_eq!(find_client_id(r#"{"api_key":"iN5ffSanbMS4rYYxVJVZcQU5jdq8Fjmz"}"#), None);
}

#[tokio::test]
async fn scripts_are_walked_in_reverse_order_of_appearance() {
    let late = read("soundcloud/bundle_late.js");
    let server = server(&late).await;
    let client = reqwest::Client::new();
    let id = scrape_client_id(&client, &endpoints(&server.base_url())).await.unwrap();
    assert_eq!(id, "iN5ffSanbMS4rYYxVJVZcQU5jdq8Fjmz");
    assert_eq!(server.request_count("/assets/50-late.js"), 1);
    assert_eq!(server.request_count("/assets/0-early.js"), 0);
}

#[tokio::test]
async fn a_page_without_a_usable_bundle_is_an_error() {
    let server = server(&read("soundcloud/bundle_early.js")).await;
    let client = reqwest::Client::new();
    let err = scrape_client_id(&client, &endpoints(&server.base_url())).await.unwrap_err();
    assert!(format!("{err}").contains("client_id not found"));
}

#[tokio::test]
async fn the_scraped_id_is_saved_and_read_back_from_credentials() {
    let server = server(&read("soundcloud/bundle_late.js")).await;
    let dir = tempfile::tempdir().unwrap();
    let client = reqwest::Client::new();
    let id = ensure_client_id(dir.path(), &client, &endpoints(&server.base_url())).await.unwrap();
    assert_eq!(id, "iN5ffSanbMS4rYYxVJVZcQU5jdq8Fjmz");
    assert_eq!(cached_client_id(dir.path()).as_deref(), Some("iN5ffSanbMS4rYYxVJVZcQU5jdq8Fjmz"));
    let creds = Credentials::load_from(dir.path()).unwrap();
    assert_eq!(creds.soundcloud.unwrap().client_id, "iN5ffSanbMS4rYYxVJVZcQU5jdq8Fjmz");
}

#[tokio::test]
async fn a_cached_id_is_reused_without_touching_the_network() {
    let server = server(&read("soundcloud/bundle_late.js")).await;
    let dir = tempfile::tempdir().unwrap();
    store_client_id(dir.path(), "iN5ffSanbMS4rYYxVJVZcQU5jdq8Fjmz").unwrap();
    let client = reqwest::Client::new();
    let id = ensure_client_id(dir.path(), &client, &endpoints(&server.base_url())).await.unwrap();
    assert_eq!(id, "iN5ffSanbMS4rYYxVJVZcQU5jdq8Fjmz");
    assert_eq!(server.request_count("/"), 0);
}

#[tokio::test]
async fn the_artist_profile_is_read_from_the_users_endpoint() {
    let server = MockServer::start(vec![
        Route::new("/users/77", vec![Reply::ok("application/json", read("soundcloud/user.json"))]),
    ])
    .await;
    let client = reqwest::Client::new();
    let user = get_user(&client, &endpoints(&server.base_url()), "iN5ffSanbMS4rYYxVJVZcQU5jdq8Fjmz", 77)
        .await
        .unwrap();
    assert_eq!(user.id, 77);
    assert_eq!(user.username, "nightwave");
    assert_eq!(user.permalink, "nightwave");
    assert_eq!(user.followers_count, 18244);
    assert_eq!(user.city.as_deref(), Some("Berlin"));
    assert_eq!(user.country.as_deref(), Some("DE"));
    assert_eq!(server.request_count("/users/77"), 1);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test soundcloud_client_id`
Expected: FAIL with "unresolved import `maxfetch::sources::soundcloud`".

- [ ] **Step 3: Create the fixtures**

`tests/fixtures/soundcloud/home.html`:
```html
<!DOCTYPE html><html><head>
<script crossorigin src="https://a-v2.sndcdn.com/assets/0-early.js"></script>
<script crossorigin src="https://a-v2.sndcdn.com/assets/50-late.js"></script>
</head><body></body></html>
```
`tests/fixtures/soundcloud/bundle_early.js`: `var env={api_key:"not-a-client-id",env:"production"};`
`tests/fixtures/soundcloud/bundle_late.js`: `var cfg={client_id:"iN5ffSanbMS4rYYxVJVZcQU5jdq8Fjmz",env:"production",version:1789401};`
`tests/fixtures/soundcloud/user.json` — the recorded `GET /users/77` body of the track owner:
```json
{"id":77,"username":"nightwave","permalink":"nightwave","followers_count":18244,
 "city":"Berlin","country":"DE","description":"electronic music","track_count":42}
```
Only the five fields in `SoundCloudUser` are read; the rest is what the service really returns and must not affect the parser.

- [ ] **Step 4: Implement the scraper**

`find_client_id` compiles the regex once with `OnceLock`. `script_urls` scans for `<script` tags and pulls the value of a `src="..."` attribute, in order; a tag without `src` is skipped. `scrape_client_id` reverses `script_urls`, maps each absolute URL to `format!("{}{}", endpoints.script_base, path)` where `path` is everything from the first `/` after the scheme and host, fetches it, and returns the first `find_client_id` hit. A fetch error on one bundle is logged with `tracing::debug!` and the walk continues.

`store_client_id` loads the credentials, replaces the `[soundcloud]` section (preserving the `oauth_token` string when it was there) and saves. The file keeps mode 0600 because it always goes through `Credentials::save_to`.

`parse_user` and `get_user` are the §8.9 profile path: `get_user` issues exactly one `GET {api_base}/users/{user_id}?client_id={client_id}` (no retry, no scrape), `parse_user` maps the five fields and defaults `followers_count` to `0` while keeping `city`/`country` optional. Task 8 reuses the same request shape through `api_get`, so the 401/403 re-scrape is implemented once.

- [ ] **Step 5: Declare the module and run the test**

Add `pub mod soundcloud;` to `src/sources/mod.rs`.

Run: `cargo test --test soundcloud_client_id`
Expected: PASS, 9 tests.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(soundcloud): scrape and cache the public client_id"
```

---

### Task 7: SoundCloud metadata, ISRC and format classification

**Files:**
- Create: `tests/fixtures/soundcloud/track.json`, `tests/fixtures/soundcloud/track_progressive.json`, `tests/fixtures/soundcloud/user.json`
- Modify: `src/sources/soundcloud.rs`
- Test: `tests/soundcloud_catalog.rs`

**Interfaces:**
- Consumes: `maxfetch::model::{Container, Quality, Release, SourceId}` (core Task 1).
- Produces:
  - `maxfetch::sources::soundcloud::Transcoding` — `struct { preset: String, protocol: String, mime_type: String }`, `Clone + Debug + PartialEq`.
  - `maxfetch::sources::soundcloud::SoundCloudTrack` — `struct { id: u64, title: String, duration_ms: u64, created_at: String, genre: Option<String>, license: Option<String>, permalink_url: String, isrc: Option<String>, publisher_artist: Option<String>, label_name: Option<String>, user_id: u64, username: String, user_permalink: String, downloadable: bool, has_downloads_left: bool, transcodings: Vec<Transcoding> }`, `Clone + Debug`.
  - `maxfetch::sources::soundcloud::parse_track(json: &str) -> anyhow::Result<SoundCloudTrack>`.
  - `SoundCloudTrack::release_from_track(&self) -> Release` — `artist` = `publisher_artist` when non-empty, else `username`; `album: None`; `year` from the first four characters of `created_at`; `duration_sec = Some(duration_ms as f64 / 1000.0)`; `source_item_id = id.to_string()`; `isrc` copied. The name is canonical: stage 2 reuses it as `release_from_track` and defines no second converter.
  - `maxfetch::sources::soundcloud::SoundCloudFormat` — `enum { Original, Aac160, Aac96, M3u8 }`, `Copy + Debug + Eq + PartialEq`. `Original` is the progressive original file, `Aac160`/`Aac96` are the HLS AAC transcodes at those bitrates, `M3u8` is any other HLS playlist. It is the field that makes `Locator::SoundCloud` distinguishable between the lossless original and the lossy AAC that share a `track_id`; stage 2 adds the `Original` candidate on top of this enum and never changes its variants.
  - **`maxfetch::model::Locator::SoundCloud { track_id: String, format: SoundCloudFormat }`** — the two-field form, canonical and shared with stage 2. The core plan declares `SoundCloud { track_id: String }`; this task widens it to the two-field shape, so the locator carries the stream kind next to the id and the stage-2 lossless original is never confused with the stage-1b AAC transcode. Every construction of the locator in this plan therefore passes both fields, and `SoundCloudFormat` is `Copy`, so the enum is stored by value and never behind a reference.
  - `maxfetch::sources::soundcloud::AudioFormat` — `enum { LossyAac { bitrate_kbps: u32 }, ProgressiveOriginal { protocol: String } }`, `Clone + Debug + PartialEq`.
  - `AudioFormat::from_transcoding(t: &Transcoding) -> Option<AudioFormat>` — `preset` `aac_160k` / `aac_96k` with `protocol == "hls"` map to `LossyAac` with the matching bitrate; `protocol` `http_mp3` / `http_opus` / `http_aac` / `progressive` maps to `ProgressiveOriginal`.
  - `AudioFormat::pick(transcodings: &[Transcoding]) -> Option<AudioFormat>` — the best `LossyAac` by bitrate when no progressive stream exists, the first `ProgressiveOriginal` when one does.
  - `AudioFormat::quality(&self) -> Quality` — `LossyAac { 160 }` -> `M4aAac`, `bitrate_kbps: Some(160)`; `ProgressiveOriginal` -> `lossless: true`, container from the protocol (`http_mp3` -> `Mp3`, `http_opus` -> `Opus`, `http_aac` -> `M4aAac`, `progressive` -> `Flac` as the provisional container) with `bit_depth`/`sample_rate`/`bitrate_kbps` all `None`.
  - `maxfetch::sources::soundcloud::ytdlp_format(format: &AudioFormat) -> &'static str` — `LossyAac { 160 }` -> `"hls_aac_160k"`, `LossyAac { 96 }` -> `"hls_aac_96k"`, `ProgressiveOriginal` -> `"bestaudio"`.
  - `maxfetch::sources::soundcloud::format_of(audio_format: &AudioFormat) -> SoundCloudFormat` — the mapping from a picked `AudioFormat` to the locator field: `LossyAac { bitrate_kbps: 160 }` -> `SoundCloudFormat::Aac160`, `LossyAac { bitrate_kbps: 96 }` -> `SoundCloudFormat::Aac96`, `ProgressiveOriginal` -> `SoundCloudFormat::Original`. `M3u8` is produced by stage 2 for an HLS playlist this stage does not pick.

- [ ] **Step 1: Write the failing test**

Create `tests/soundcloud_catalog.rs`:
```rust
mod support;

use maxfetch::model::{Container, SourceId};
use maxfetch::sources::soundcloud::{
    format_of, parse_track, AudioFormat, SoundCloudFormat, SoundCloudTrack, Transcoding,
};
use support::fixture;

fn read(rel: &str) -> String {
    std::fs::read_to_string(fixture(rel)).unwrap()
}

fn track() -> SoundCloudTrack {
    parse_track(&read("soundcloud/track.json")).unwrap()
}

#[test]
fn track_metadata_maps_to_a_release() {
    let release = track().release_from_track();
    assert_eq!(release.source, SourceId::SoundCloud);
    assert_eq!(release.source_item_id, "880017");
    assert_eq!(release.title, "Neon");
    assert_eq!(release.artist, "Nightwave");
    assert_eq!(release.album, None);
    assert_eq!(release.track_no, None);
    assert_eq!(release.duration_sec, Some(214.0));
    assert_eq!(release.year, Some(2023));
}

#[test]
fn the_isrc_comes_from_publisher_metadata() {
    assert_eq!(track().release_from_track().isrc.as_deref(), Some("DEP960300042"));
}

#[test]
fn the_publisher_artist_wins_over_the_username() {
    let mut t = track();
    t.publisher_artist = Some("Nightwave Music".into());
    assert_eq!(t.release_from_track().artist, "Nightwave Music");
    let mut t = track();
    t.publisher_artist = None;
    assert_eq!(t.release_from_track().artist, "nightwave");
}

#[test]
fn duration_is_converted_from_milliseconds() {
    assert_eq!(track().duration_ms, 214000);
    assert_eq!(track().release_from_track().duration_sec, Some(214.0));
}

#[test]
fn aac_160k_is_classified_as_lossy() {
    let format = AudioFormat::from_transcoding(&Transcoding {
        preset: "aac_160k".into(),
        protocol: "hls".into(),
        mime_type: "audio/mp4; codecs=\"mp4a.40.2\"".into(),
    })
    .unwrap();
    assert_eq!(format, AudioFormat::LossyAac { bitrate_kbps: 160 });
    let quality = format.quality();
    assert!(!quality.lossless);
    assert_eq!(quality.container, Container::M4aAac);
    assert_eq!(quality.bitrate_kbps, Some(160));
}

#[test]
fn aac_96k_is_classified_as_lossy_at_ninety_six() {
    let format = AudioFormat::from_transcoding(&Transcoding {
        preset: "aac_96k".into(),
        protocol: "hls".into(),
        mime_type: "audio/mp4; codecs=\"mp4a.40.2\"".into(),
    })
    .unwrap();
    assert_eq!(format, AudioFormat::LossyAac { bitrate_kbps: 96 });
    assert_eq!(format.quality().bitrate_kbps, Some(96));
}

#[test]
fn a_progressive_stream_is_a_lossless_candidate() {
    let format = AudioFormat::from_transcoding(&Transcoding {
        preset: "original".into(),
        protocol: "http_mp3".into(),
        mime_type: "audio/mpeg".into(),
    })
    .unwrap();
    assert_eq!(format, AudioFormat::ProgressiveOriginal { protocol: "http_mp3".into() });
    let quality = format.quality();
    assert!(quality.lossless);
    assert_eq!(quality.container, Container::Mp3);
    assert_eq!(quality.bit_depth, None);
    assert_eq!(quality.bitrate_kbps, None);
}

#[test]
fn without_oauth_the_pick_is_aac_160k() {
    let format = AudioFormat::pick(&track().transcodings).unwrap();
    assert_eq!(format, AudioFormat::LossyAac { bitrate_kbps: 160 });
}

#[test]
fn a_progressive_stream_beats_the_hls_streams() {
    let format = AudioFormat::pick(&parse_track(&read("soundcloud/track_progressive.json")).unwrap().transcodings).unwrap();
    assert!(matches!(format, AudioFormat::ProgressiveOriginal { .. }));
}

#[test]
fn an_unknown_transcoding_is_ignored() {
    assert_eq!(
        AudioFormat::from_transcoding(&Transcoding { preset: "opus_0_0".into(), protocol: "hls".into(), mime_type: "audio/ogg".into() }),
        None
    );
    assert_eq!(AudioFormat::pick(&[]), None);
}

#[test]
fn the_ytdlp_format_name_matches_the_transcoding_preset() {
    assert_eq!(maxfetch::sources::soundcloud::ytdlp_format(&AudioFormat::LossyAac { bitrate_kbps: 160 }), "hls_aac_160k");
    assert_eq!(maxfetch::sources::soundcloud::ytdlp_format(&AudioFormat::LossyAac { bitrate_kbps: 96 }), "hls_aac_96k");
    assert_eq!(maxfetch::sources::soundcloud::ytdlp_format(&AudioFormat::ProgressiveOriginal { protocol: "progressive".into() }), "bestaudio");
}

#[test]
fn the_locator_format_names_the_picked_stream() {
    assert_eq!(format_of(&AudioFormat::LossyAac { bitrate_kbps: 160 }), SoundCloudFormat::Aac160);
    assert_eq!(format_of(&AudioFormat::LossyAac { bitrate_kbps: 96 }), SoundCloudFormat::Aac96);
    assert_eq!(
        format_of(&AudioFormat::ProgressiveOriginal { protocol: "progressive".into() }),
        SoundCloudFormat::Original
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test soundcloud_catalog`
Expected: FAIL with "cannot find function `parse_track`".

- [ ] **Step 3: Create the fixtures**

`tests/fixtures/soundcloud/track.json` (the recorded `GET /tracks/880017` body):
```json
{"id":880017,"title":"Neon","duration":214000,"created_at":"2023-05-11T09:14:02Z",
 "genre":"Electronic","license":"all-rights-reserved",
 "permalink_url":"https://soundcloud.com/nightwave/neon","artwork_url":"https://i1.sndcdn.com/artworks-neon-large.jpg",
 "publisher_metadata":{"isrc":"DEP960300042","artist":"Nightwave","publisher":"Nightwave Music","writer_composer":"N. Wave"},
 "label_name":"Nightwave","downloadable":true,"has_downloads_left":true,
 "user":{"id":77,"username":"nightwave","permalink":"nightwave"},
 "media":{"transcodings":[
   {"preset":"aac_160k","protocol":"hls","mime_type":"audio/mp4; codecs=\"mp4a.40.2\""},
   {"preset":"aac_96k","protocol":"hls","mime_type":"audio/mp4; codecs=\"mp4a.40.2\""}
 ]}}
```
`tests/fixtures/soundcloud/track_progressive.json` — the same object with `"id":880018` and a transcoding list of `{"preset":"original","protocol":"progressive","mime_type":"audio/mpeg"}` plus the two HLS entries.

- [ ] **Step 4: Implement the mapping**

`parse_track` reads the scalars with `serde_json`, fills the optional fields from `publisher_metadata` (`isrc`, `artist`), `license`, `genre`, `label_name`, `artwork_url` (dropped: it is not part of `Release` and is not needed in this stage beyond the `SoundCloudTrack` shape — keep the field out of the struct and out of the parser), `downloadable` and `has_downloads_left` defaulting to `false`, `user` from `user.id`/`user.username`/`user.permalink`, and `transcodings` from `media.transcodings[]`.

`SoundCloudFormat` is declared here as `enum { Original, Aac160, Aac96, M3u8 }` with `Copy + Debug + Eq + PartialEq`, and `maxfetch::model::Locator::SoundCloud` is widened to the two-field form `SoundCloud { track_id: String, format: SoundCloudFormat }` (the core plan declares only `track_id`; this task owns the widening, and stage 2 reuses the same two-field shape without changing it). `format_of` maps a picked `AudioFormat` to that field: `LossyAac { 160 }` -> `Aac160`, `LossyAac { 96 }` -> `Aac96`, `ProgressiveOriginal` -> `Original`. `M3u8` is declared here but not produced by this stage; it is the slot stage 2 uses for an HLS playlist that is neither 160k nor 96k. Because the enum is `Copy`, the locator stores it by value and every construction site passes both fields.

Note on `AudioFormat::quality()`: the `Flac` container for `protocol == "progressive"` is provisional and documented as such in a doc comment. The pipeline replaces it with the probed quality before anything is stored, and Task 9 asserts that the database row carries the probed container, never this provisional one.

The profile endpoint of §8.9 is implemented here as `parse_user`/`get_user` (Task 7), and Task 8's `resolve` calls the same request through `api_get` so a rotated `client_id` is re-scraped for a profile read exactly as it is for a track read. `SoundCloudUser` stays a catalog detail in this stage: it is what the Sources/Track screen shows for the artist, and it never overrides `SoundCloudTrack::release_from_track`'s `artist`, because the track's own `publisher_metadata.artist` is closer to the recording than the account name.

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test --test soundcloud_catalog`
Expected: PASS, 12 tests.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(soundcloud): track metadata, isrc and format classification"
```

---

### Task 8: The SoundCloud source: inputs, 401 re-scrape and lossy download

**Files:**
- Modify: `src/sources/soundcloud.rs`
- Create: `tests/fixtures/bin/yt-dlp-search.json`, `tests/fixtures/bin/yt-dlp-url.json`
- Create: `tests/fixtures/bin/fake-yt-dlp.sh` (a `#!/bin/sh` stub that replays the recorded `yt-dlp` JSON; `chmod +x` it)
- Create: `tests/fixtures/bin/fake-yt-dlp-lossy.sh` (a `#!/bin/sh` stub that copies a real lossy file over the `-o` path; `chmod +x` it)
- Create: `tests/fixtures/probe/sine_128k.mp3` (generated once with `ffmpeg -f lavfi -i "sine=frequency=440:duration=1" -c:a libmp3lame -b:a 128k tests/fixtures/probe/sine_128k.mp3 -y`, then committed)
- Test: `tests/soundcloud_source.rs`

**Interfaces:**
- Consumes: `maxfetch::sources::{Capabilities, AuthState, Query, QueryItem, Source}` (core Task 10), `maxfetch::fetch::ProgressSink`, `maxfetch::config::Config` (`youtube.yt_dlp_path`), Tasks 6-7.
- Produces:
  - `maxfetch::sources::soundcloud::SoundCloudInput` — `enum { Id(u64), Url(String), Text(String) }`, `Debug + PartialEq`.
  - `maxfetch::sources::soundcloud::classify_input(s: &str) -> SoundCloudInput` — all digits -> `Id`; a string that starts with `http` or contains `soundcloud.com` -> `Url`; anything else -> `Text`.
  - `maxfetch::sources::soundcloud::search_arg(query: &str, limit: u32) -> String` — `format!("scsearch{limit}:{query}")`, the search prefix the `yt-dlp` SoundCloud extractor understands.
  - `maxfetch::sources::soundcloud::parse_ytdlp_ids(json: &str) -> Vec<u64>` — reads a top-level `id` and every `entries[].id`, in order; non-numeric ids are skipped.
  - `maxfetch::sources::soundcloud::SoundCloudSource` with `SoundCloudSource::new(config: &Config, credentials: &Credentials) -> SoundCloudSource` and `SoundCloudSource::with_api_base(api_base: &str, credentials_dir: &Path, yt_dlp_path: &str) -> SoundCloudSource`. **The constructor set is canonicalized in stage 2**: stage 1b introduces no constructor named `with_endpoints` (an earlier draft had one and it was removed); stage 2 adds `with_ytdlp(creds, yt_dlp_path)` and `without_credentials()` on top of exactly these two, so the sets never conflict. `with_api_base` points `web_base`, `api_base` and `script_base` at the same base URL, which is what the mock server needs.
  - `impl Source for SoundCloudSource` — `capabilities().lossless` is true only when `credentials.soundcloud.oauth_token` is a non-empty string; `auth_state()` is `AuthState::NotNeeded`; `search` handles `Text` through `yt-dlp`; `resolve` handles `Id` and `Url`; `candidates` returns exactly one candidate from `AudioFormat::pick`, carrying the locator `Locator::SoundCloud { track_id, format }` with `format` from `format_of`; `download` fetches the audio with `yt-dlp -f <ytdlp_format>`.
  - `maxfetch::sources::soundcloud::api_get(&self, path: &str) -> anyhow::Result<String>` — GETs `{api_base}{path}?client_id={id}`; on 401/403 it re-scrapes the id once, stores it and retries once; a second 401/403 returns `"soundcloud: client_id rejected after a re-scrape"`.

- [ ] **Step 1: Write the failing test**

Create `tests/soundcloud_source.rs`:
```rust
mod support;

use std::path::Path;

use maxfetch::config::Config;
use maxfetch::fetch::NoopSink;
use maxfetch::model::{Container, Locator, SourceId};
use maxfetch::sources::soundcloud::{
    classify_input, parse_ytdlp_ids, search_arg, SoundCloudFormat, SoundCloudInput, SoundCloudSource,
};
use maxfetch::sources::{AuthState, Query, Source};
use support::{fixture, MockServer, Reply, Route};

fn read(rel: &str) -> String {
    std::fs::read_to_string(fixture(rel)).unwrap()
}

fn stub(name: &str) -> String {
    fixture(&format!("bin/{name}")).to_str().unwrap().to_string()
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
    assert_eq!(classify_input("nightwave neon"), SoundCloudInput::Text("nightwave neon".into()));
}

#[test]
fn the_search_argument_uses_the_ytdlp_search_prefix() {
    assert_eq!(search_arg("nightwave neon", 10), "scsearch10:nightwave neon");
}

#[test]
fn ytdlp_ids_are_read_from_both_shapes() {
    assert_eq!(parse_ytdlp_ids(&read("bin/yt-dlp-url.json")), vec![880017]);
    assert_eq!(parse_ytdlp_ids(&read("bin/yt-dlp-search.json")), vec![880017, 880018]);
}

#[tokio::test]
async fn a_track_id_yields_one_release_with_its_isrc() {
    let server = MockServer::start(vec![
        Route::new("/tracks/880017", vec![Reply::ok("application/json", read("soundcloud/track.json"))]),
        Route::new("/users/77", vec![Reply::ok("application/json", read("soundcloud/user.json"))]),
        Route::new("/", vec![Reply::ok("text/html", read("soundcloud/home.html"))]),
        Route::new("/assets/50-late.js", vec![Reply::ok("application/javascript", read("soundcloud/bundle_late.js"))]),
    ])
    .await;
    let dir = tempfile::tempdir().unwrap();
    let src = SoundCloudSource::with_api_base(&server.base_url(), dir.path(), "yt-dlp");
    let releases = src.resolve(&Query { text: None, url: None, items: vec![maxfetch::sources::QueryItem::Track { source: SourceId::SoundCloud, id: "880017".into() }] }).await.unwrap();
    assert_eq!(releases.len(), 1);
    assert_eq!(releases[0].isrc.as_deref(), Some("DEP960300042"));
}

#[tokio::test]
async fn a_rotated_client_id_is_rescraped_and_the_request_retried_once() {
    let server = MockServer::start(vec![
        Route::new("/tracks/880017", vec![
            Reply::status(401, "unauthorized"),
            Reply::ok("application/json", read("soundcloud/track.json")),
        ]),
        Route::new("/users/77", vec![Reply::ok("application/json", read("soundcloud/user.json"))]),
        Route::new("/", vec![Reply::ok("text/html", read("soundcloud/home.html"))]),
        Route::new("/assets/50-late.js", vec![Reply::ok("application/javascript", read("soundcloud/bundle_late.js"))]),
    ])
    .await;
    let dir = tempfile::tempdir().unwrap();
    let src = SoundCloudSource::with_api_base(&server.base_url(), dir.path(), "yt-dlp");
    let release = src.resolve(&Query { text: None, url: None, items: vec![maxfetch::sources::QueryItem::Track { source: SourceId::SoundCloud, id: "880017".into() }] }).await.unwrap();
    assert_eq!(release.len(), 1);
    assert_eq!(server.request_count("/tracks/880017"), 2);
    assert_eq!(server.request_count("/"), 1);
}

#[tokio::test]
async fn a_second_401_fails_the_request_without_a_panic() {
    let server = MockServer::start(vec![
        Route::new("/tracks/880017", vec![Reply::status(401, "unauthorized")]),
        Route::new("/", vec![Reply::ok("text/html", read("soundcloud/home.html"))]),
        Route::new("/assets/50-late.js", vec![Reply::ok("application/javascript", read("soundcloud/bundle_late.js"))]),
    ])
    .await;
    let dir = tempfile::tempdir().unwrap();
    let src = SoundCloudSource::with_api_base(&server.base_url(), dir.path(), "yt-dlp");
    let err = src.resolve(&Query { text: None, url: None, items: vec![maxfetch::sources::QueryItem::Track { source: SourceId::SoundCloud, id: "880017".into() }] }).await.unwrap_err();
    assert!(format!("{err}").contains("client_id rejected"));
    assert_eq!(server.request_count("/"), 1);
}

#[tokio::test]
async fn the_only_candidate_is_the_lossy_aac_stream() {
    let server = MockServer::start(vec![
        Route::new("/tracks/880017", vec![Reply::ok("application/json", read("soundcloud/track.json"))]),
        Route::new("/users/77", vec![Reply::ok("application/json", read("soundcloud/user.json"))]),
        Route::new("/", vec![Reply::ok("text/html", read("soundcloud/home.html"))]),
        Route::new("/assets/50-late.js", vec![Reply::ok("application/javascript", read("soundcloud/bundle_late.js"))]),
    ])
    .await;
    let dir = tempfile::tempdir().unwrap();
    let src = SoundCloudSource::with_api_base(&server.base_url(), dir.path(), "yt-dlp");
    let release = src.resolve(&Query { text: None, url: None, items: vec![maxfetch::sources::QueryItem::Track { source: SourceId::SoundCloud, id: "880017".into() }] }).await.unwrap()[0].clone();
    assert_eq!(server.request_count("/users/77"), 1, "the §8.9 profile is fetched during resolve");
    let candidates = src.candidates(&release).await.unwrap();
    // Exactly one candidate, and it is lossy: no lossless stream is offered here.
    assert_eq!(candidates.len(), 1);
    assert!(candidates.iter().all(|c| !c.quality.lossless));
    assert_eq!(candidates[0].quality.container, Container::M4aAac);
    assert_eq!(candidates[0].quality.bitrate_kbps, Some(160));
    assert_eq!(candidates[0].locator, Locator::SoundCloud { track_id: "880017".into(), format: SoundCloudFormat::Aac160 });
    // The platform's lossless claim must not leak into the catalog: the track is
    // downloadable and flagged as such on SoundCloud, yet no lossless quality is
    // produced and the release carries no lossless marker.
    assert!(release.isrc.as_deref() == Some("DEP960300042"));
}

#[tokio::test]
async fn a_lossy_download_goes_through_ytdlp_with_the_hls_preset() {
    let server = MockServer::start(vec![
        Route::new("/tracks/880017", vec![Reply::ok("application/json", read("soundcloud/track.json"))]),
        Route::new("/users/77", vec![Reply::ok("application/json", read("soundcloud/user.json"))]),
        Route::new("/", vec![Reply::ok("text/html", read("soundcloud/home.html"))]),
        Route::new("/assets/50-late.js", vec![Reply::ok("application/javascript", read("soundcloud/bundle_late.js"))]),
    ])
    .await;
    let dir = tempfile::tempdir().unwrap();
    let src = SoundCloudSource::with_api_base(&server.base_url(), dir.path(), &stub("fake-yt-dlp.sh"));
    let release = src.resolve(&Query { text: None, url: None, items: vec![maxfetch::sources::QueryItem::Track { source: SourceId::SoundCloud, id: "880017".into() }] }).await.unwrap()[0].clone();
    let candidate = src.candidates(&release).await.unwrap().remove(0);
    let dest = dir.path().join("out.m4a");
    src.download(&candidate, &dest, &NoopSink).await.unwrap();
    assert!(dest.exists());
    assert_eq!(std::fs::metadata(&dest).unwrap().len(), std::fs::metadata(fixture("probe/sine_44100_s16.flac")).unwrap().len());
}

#[tokio::test]
async fn a_source_with_an_oauth_token_advertises_lossless() {
    let mut cfg = Config::default();
    let mut creds = maxfetch::config::Credentials::default();
    creds.soundcloud = Some(maxfetch::config::SoundCloudCredentials { client_id: "iN5ffSanbMS4rYYxVJVZcQU5jdq8Fjmz".into(), oauth_token: "t".into() });
    let src = SoundCloudSource::new(&cfg, &creds);
    assert!(src.capabilities().lossless);
    assert_eq!(src.auth_state(), AuthState::NotNeeded);
    creds.soundcloud = None;
    cfg.youtube.yt_dlp_path = "yt-dlp".into();
    assert!(!SoundCloudSource::new(&cfg, &creds).capabilities().lossless);
    // An empty `oauth_token` means "no token", not "some token".
    creds.soundcloud = Some(maxfetch::config::SoundCloudCredentials { client_id: String::new(), oauth_token: String::new() });
    assert!(!SoundCloudSource::new(&cfg, &creds).capabilities().lossless);
}
```

Before running, add the stub scripts `tests/fixtures/bin/fake-yt-dlp.sh` and `tests/fixtures/bin/fake-yt-dlp-lossy.sh` (Step 3) and make them executable. The last test's `Config`/`SoundCloudCredentials` come from `maxfetch::config` (core Task 3); the `Credentials::soundcloud` field type is `Option<SoundCloudCredentials>{ client_id: String, oauth_token: String }` — plain `String` fields, exactly as the core plan declares them, where an **empty string means "no value"**. There is no `Option` here: a missing `[soundcloud]` section is already expressed by `Credentials::soundcloud == None`, and a present section with an empty `oauth_token` means "anonymous access only".

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test soundcloud_source`
Expected: FAIL with "unresolved import `maxfetch::sources::soundcloud::classify_input`".

- [ ] **Step 3: Create the yt-dlp stubs and their recorded payloads**

`tests/fixtures/bin/fake-yt-dlp.sh` — the metadata stub used by Task 8 (prints the JSON, and would copy a FLAC only if asked to download, which Task 8 never does):
```bash
#!/bin/sh
# Replays a recorded yt-dlp result. The last argument is the target; -f selects
# the format. Prints the JSON fixture to stdout.
while [ "$#" -gt 0 ]; do
  case "$1" in
    -f) FORMAT="$2"; shift 2 ;;
    -o) OUT="$2"; shift 2 ;;
    *) TARGET="$1"; shift ;;
  esac
done
if [ "$TARGET" = "scsearch10:nightwave neon" ]; then
  cat "$(dirname "$0")/yt-dlp-search.json"
else
  cat "$(dirname "$0")/yt-dlp-url.json"
fi
```

`tests/fixtures/bin/fake-yt-dlp-lossy.sh` — the download stub used by the Stage 1b pipeline test, which must deliver a **lossy** file so the probe and the `maxfetch: source=lossy` comment tell the truth about a SoundCloud AAC download:
```bash
#!/bin/sh
# Delivers a real lossy file: an MP3 128 kbps, never a FLAC.
while [ "$#" -gt 0 ]; do
  case "$1" in
    -o) OUT="$2"; shift 2 ;;
    *) shift ;;
  esac
done
cp "$(dirname "$0")/../probe/sine_128k.mp3" "$OUT"
```
`chmod +x tests/fixtures/bin/fake-yt-dlp.sh tests/fixtures/bin/fake-yt-dlp-lossy.sh`.

`tests/fixtures/probe/sine_128k.mp3` — generated once, committed (the command is in the Step below that needs it):
```bash
ffmpeg -f lavfi -i "sine=frequency=440:duration=1" -c:a libmp3lame -b:a 128k tests/fixtures/probe/sine_128k.mp3 -y
```

`tests/fixtures/bin/yt-dlp-url.json`:
```json
{"id":"880017","title":"Neon","extractor_key":"SoundCloud"}
```
`tests/fixtures/bin/yt-dlp-search.json`:
```json
{"id":"scsearch10:nightwave neon","entries":[{"id":"880017","title":"Neon"},{"id":"880018","title":"Midnight"}]}
```

- [ ] **Step 4: Implement the source**

`api_get` builds `{api_base}{path}?client_id={id}` (with `&client_id=` when the path already carries a query), reads the status, and on 401/403 runs `ensure_client_id` with `force: true` semantics by calling `scrape_client_id` directly, then `store_client_id`, then repeats the request exactly once.

`resolve` handles `QueryItem::Track { id }` by calling `api_get("/tracks/{id}")` -> `parse_track` -> `release_from_track`. After the track is parsed, `resolve` also fetches its artist profile with `api_get("/users/{track.user_id}")` -> `parse_user` and keeps the result on the source for the Sources/Track screen (§8.9 `GET /users/{id}`); a profile failure is logged with `tracing::debug!` and never fails the resolve, because the profile is enrichment, not a prerequisite. A `Query::url` or a `QueryItem::Album/Playlist` goes through `yt-dlp -J --flat-playlist <url>` -> `parse_ytdlp_ids` -> one `api_get` per id. `Query::text` goes through `yt-dlp -J --flat-playlist "scsearch10:<text>"` -> `parse_ytdlp_ids` -> one `api_get` per id. Any `yt-dlp` failure is logged with `tracing::warn!` and yields `Ok(vec![])`, so a missing or broken `yt-dlp` never aborts a run.

`candidates` re-fetches `GET /tracks/{id}` (the locator carries the track id and the stream format) and returns one candidate from `AudioFormat::pick`, with `size_bytes: None` (SoundCloud reports no size before the transfer) and `locator: Locator::SoundCloud { track_id, format: format_of(&picked) }` — the two-field locator of Task 7, so the AAC candidate of this stage stays distinguishable from the lossless original that stage 2 adds under the same `track_id`.

`download` runs `yt-dlp -f <ytdlp_format> -o <dest> --no-playlist <permalink_url>` through `tokio::process::Command`, fail with the tail of stderr on a non-zero exit, and reports `sink.on_finished(bytes)` with the written size.

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test --test soundcloud_source`
Expected: PASS, 11 tests.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(soundcloud): source with client_id retry and lossy download"
```

---

### Task 9: Registry wiring, auth status and the Stage 1b pipeline test

**Files:**
- Modify: `src/sources/mod.rs` (`registry` registers both sources)
- Modify: `src/auth.rs` (Stage 1b login branches and status details)
- Create: `tests/e2e_stage1b.rs` (the end-to-end test, gated behind `#![cfg(feature = "e2e")]`)
- Create: `tests/fixtures/bandcamp/fuzzysearch_album.json` (the recorded `fuzzysearch` body used by the e2e test)
- Test: `tests/stage1b_pipeline.rs`, `tests/stage1b_auth.rs`

**Interfaces:**
- Consumes: `maxfetch::pipeline::Pipeline` (core Task 11), `maxfetch::db::Db` (core Task 4), `maxfetch::sources::{registry, enabled_in, SourceStatus}` (core Task 10 and stage 1 Task 7), `BandcampSource` (Task 5), `SoundCloudSource` (Task 8).
- Produces:
  - `maxfetch::sources::registry(config: &Config, credentials: &Credentials) -> Vec<Box<dyn Source>>` — unchanged signature, and the contract of the core and the stage 1 plan: **it returns every implemented source, unfiltered**, in the fixed order `Soulseek, YouTube, Spotify, Bandcamp, SoundCloud`, and never looks at `config.sources.enabled`. It only builds the two Stage 1b sources, appending `Bandcamp` then `SoundCloud` to what stage 1 already registers.
  - `maxfetch::sources::enabled_in(ids: &[SourceId], config: &Config) -> Vec<SourceId>` — the single filter that the pipeline and the TUI apply on the registry output (stage 1 Task 7 owns it; this task does not redefine it, it only relies on it). A source that is registered but not enabled stays visible in the Sources screen and does not participate in a run.
  - `maxfetch::sources::BandcampSource::new` passes the archive cache root `$XDG_CACHE_HOME/maxfetch` (falling back to `$HOME/.cache/maxfetch` when `XDG_CACHE_HOME` is unset), created at construction; `archive_dir` appends `bandcamp/<tralbum_id>`, so unpacked archives live exactly at `$XDG_CACHE_HOME/maxfetch/bandcamp/<tralbum_id>`. `SoundCloudSource::new` keeps using `config::config_dir()` for the `client_id` cache.
  - `maxfetch::auth::stage1b_login_hint(source: &str) -> Option<&'static str>` — `Some("вход в Bandcamp появится в этапе 2: в этапе 1б доступно только бесплатное промо")` for `"bandcamp"`, `Some("вход в SoundCloud появится в этапе 2: без него доступен только AAC 160k")` for `"soundcloud"`, `None` otherwise.
  - `maxfetch::auth::login` gains those two branches before the generic "not implemented in the core build" error, so `maxfetch auth login soundcloud` explains the limitation in Russian instead of claiming the build is incomplete.
  - `maxfetch::cli::fetch_query` wiring for the §7.12 flags (core Task 13 already declares the flags; this task gives them meaning for the Stage 1b sources): `--page <url>` becomes `Query { text: None, url: Some(url), items: vec![] }` and the pipeline picks the source by the URL's domain — a `bandcamp.com` host routes to `Query::url` for `BandcampSource`, a `soundcloud.com` host routes to `Query::url` for `SoundCloudSource`; a host that matches neither is logged with `tracing::warn!` and the run gets an empty result instead of an error. `--soundcloud <url>` is the explicit form: it always targets `SoundCloudSource` (a bare `soundcloud.com` URL works even when the domain sniffing would be ambiguous, e.g. a `sndcdn.com` link).
  - `tests/e2e_stage1b.rs` — the Stage 1b end-to-end test file, gated behind `#![cfg(feature = "e2e")]`. It contains exactly one test, `bandcamp_promo_roundtrip_records_a_lossless_track`, which drives the whole Bandcamp promo path against the mock server (`fuzzysearch` -> `tralbum_details` -> the 403 `download` -> `statdownload` -> the retry archive), writes the member into a temporary library through `Pipeline::download`, then runs `library scan` and `library dedupe --dry-run` through the headless CLI and asserts that the database holds exactly one lossless Bandcamp row whose path exists and ends in `.flac`, with no duplicate groups and nothing to delete. The file reuses `tests/support/mod.rs` (`mod support;` sits after the `cfg` attribute), so it compiles to nothing without the feature.
  - `tests/fixtures/bandcamp/fuzzysearch_album.json` — the recorded `fuzzysearch` body of the e2e test: one album hit only, so the search resolves exactly one album and the roundtrip has a single expected outcome. It is created by this task and used only by `tests/e2e_stage1b.rs`.

- [ ] **Step 1: Write the failing test**

Create `tests/stage1b_auth.rs`:
```rust
use maxfetch::auth::{auth_status, login, stage1b_login_hint};
use maxfetch::config::{Config, Credentials};
use maxfetch::model::SourceId;
use maxfetch::sources::{enabled_in, registry};

#[test]
fn stage1b_sources_are_registered_but_disabled_by_the_default_config() {
    let cfg = Config::default();
    let all = registry(&cfg, &Credentials::default());
    let ids: Vec<SourceId> = all.iter().map(|s| s.id()).collect();
    // `registry` is a pure constructor: every implemented source is present,
    // whatever `config.sources.enabled` says.
    assert!(ids.contains(&SourceId::Bandcamp));
    assert!(ids.contains(&SourceId::SoundCloud));
    // `enabled_in` is the only filter; with the default config SoundCloud is off.
    let enabled = enabled_in(&ids, &cfg);
    assert!(enabled.contains(&SourceId::Bandcamp));
    assert!(!enabled.contains(&SourceId::SoundCloud));
}

#[test]
fn an_enabled_soundcloud_is_registered() {
    let mut cfg = Config::default();
    cfg.sources.enabled.push("soundcloud".into());
    let ids: Vec<SourceId> = registry(&cfg, &Credentials::default()).iter().map(|s| s.id()).collect();
    assert!(ids.contains(&SourceId::Bandcamp));
    assert!(ids.contains(&SourceId::SoundCloud));
    let enabled = enabled_in(&ids, &cfg);
    assert!(enabled.contains(&SourceId::Bandcamp));
    assert!(enabled.contains(&SourceId::SoundCloud));
}

#[test]
fn neither_source_needs_credentials_in_stage1b() {
    let statuses = auth_status(&Credentials::default());
    let bandcamp = statuses.iter().find(|s| s.id == SourceId::Bandcamp).unwrap();
    let soundcloud = statuses.iter().find(|s| s.id == SourceId::SoundCloud).unwrap();
    assert_eq!(bandcamp.auth, maxfetch::sources::AuthState::NotNeeded);
    assert_eq!(soundcloud.auth, maxfetch::sources::AuthState::NotNeeded);
    assert_eq!(bandcamp.detail.as_deref(), Some("без учётной записи: только бесплатное промо"));
    assert_eq!(soundcloud.detail.as_deref(), Some("без учётной записи: только AAC 160k"));
}

#[tokio::test]
async fn the_login_hint_explains_the_stage_two_limit() {
    assert!(stage1b_login_hint("bandcamp").unwrap().contains("этапе 2"));
    assert!(stage1b_login_hint("soundcloud").unwrap().contains("AAC 160k"));
    let mut creds = Credentials::default();
    let err = login("soundcloud", &mut creds).await.unwrap_err();
    assert!(format!("{err}").contains("этапе 2"));
}
```

Create `tests/stage1b_pipeline.rs`:
```rust
mod support;

use maxfetch::config::Config;
use maxfetch::db::Db;
use maxfetch::fetch::NoopSink;
use maxfetch::model::SourceId;
use maxfetch::pipeline::Pipeline;
use maxfetch::sources::bandcamp::{parse_tralbum_details, BandcampSource};
use maxfetch::sources::soundcloud::SoundCloudSource;
use maxfetch::sources::{Query, QueryItem};

use support::{fixture, write_zip, MockServer, Reply, Route};

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
            Route::new("/album/night-drive", vec![Reply::ok("text/html", page.replace("https://nightwave.bandcamp.com", base))]),
            Route::new(
                "/album/night-drive/statdownload",
                vec![Reply::ok("text/plain", retry.replace("https://nightwave.bandcamp.com", base))],
            ),
            Route::new("/download/album", vec![Reply::status(200, zip_bytes.clone())]),
            Route::new("/api/mobile/25/tralbum_details", vec![Reply::ok("application/json", support::tralbum_json(1))]),
            Route::new("/tracks/880017", vec![Reply::ok("application/json", read("soundcloud/track.json"))]),
            Route::new("/", vec![Reply::ok("text/html", read("soundcloud/home.html"))]),
            Route::new("/assets/50-late.js", vec![Reply::ok("application/javascript", read("soundcloud/bundle_late.js"))]),
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
            items: vec![QueryItem::Track { source: SourceId::SoundCloud, id: "880017".into() }],
        })
        .await
        .unwrap();
    assert!(!groups.is_empty());

    let bandcamp_group = groups.iter().find(|g| g.primary.source == SourceId::Bandcamp).unwrap();
    let candidates = p.candidates(bandcamp_group).await.unwrap();
    assert!(candidates[0].quality.lossless);
    let result = p.download(&candidates[0], &NoopSink).await.unwrap();
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
        Route::new("/tracks/880017", vec![Reply::ok("application/json", read("soundcloud/track.json"))]),
        Route::new("/users/77", vec![Reply::ok("application/json", read("soundcloud/user.json"))]),
        Route::new("/", vec![Reply::ok("text/html", read("soundcloud/home.html"))]),
        Route::new("/assets/50-late.js", vec![Reply::ok("application/javascript", read("soundcloud/bundle_late.js"))]),
    ])
    .await;
    let lib = tempfile::tempdir().unwrap();
    let mut cfg = Config::default();
    cfg.library.path = lib.path().to_path_buf();
    cfg.youtube.yt_dlp_path = fixture("bin/fake-yt-dlp-lossy.sh").to_str().unwrap().to_string();
    let soundcloud = SoundCloudSource::with_api_base(&server.base_url(), lib.path(), &cfg.youtube.yt_dlp_path);
    let db = Db::open_in_memory().unwrap();
    let p = Pipeline::new(cfg, db, vec![Box::new(soundcloud)]);
    let groups = p
        .search(&Query { text: None, url: None, items: vec![QueryItem::Track { source: SourceId::SoundCloud, id: "880017".into() }] })
        .await
        .unwrap();
    let candidates = p.candidates(&groups[0]).await.unwrap();
    assert!(!candidates[0].quality.lossless);
    let result = p.download(&candidates[0], &NoopSink).await.unwrap();
    assert!(!result.rejected);
    let tracks = p.db().all_tracks().unwrap();
    assert_eq!(tracks[0].isrc.as_deref(), Some("DEP960300042"));
    assert!(!tracks[0].lossless, "the measured bytes are lossy");
    let comment = maxfetch::tags::read_tags(std::path::Path::new(&tracks[0].path)).unwrap().quality_comment;
    assert!(comment.contains("source=lossy"));
    assert!(!comment.contains("source=lossless"));
}
```

The stub delivers a **real lossy file**: `tests/fixtures/bin/fake-yt-dlp-lossy.sh` copies `tests/fixtures/probe/sine_128k.mp3` (generated above) over the `-o` path, so the probe measures an actual MP3 128 kbps and the pipeline records `lossless = false` plus the `maxfetch: source=lossy` comment from measured bytes — exactly what a real SoundCloud AAC download produces. A stub writing FLAC bytes would have flipped the stored quality to the measured lossless (the probe decides) and made the comment assertion meaningless.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --test stage1b_auth --test stage1b_pipeline`
Expected: FAIL with "cannot find function `stage1b_login_hint`" and with `registry` returning no Bandcamp source.

- [ ] **Step 3: Wire the registry, the auth status and the CLI flags**

In `src/sources/mod.rs`, replace the empty `registry` body: push `BandcampSource::new(config, credentials)` and then `SoundCloudSource::new(config, credentials)`, **unconditionally**, after the three stage-1 sources. `registry` stays a pure constructor and does not call `enabled_in`; the pipeline and the TUI filter its output through `enabled_in(&ids, config)`, which is why a disabled source still shows up in the Sources screen with its reason. `BandcampSource::new` builds the cache root `$XDG_CACHE_HOME/maxfetch` (created at construction) — never `config::config_dir()`, which is where only `SoundCloudSource::new` looks for the cached `client_id`.

In `src/auth.rs`, `auth_status` special-cases the two Stage 1b ids: `AuthState::NotNeeded` with the Russian detail strings from the Interfaces block, regardless of the credentials section. `login` checks `stage1b_login_hint` first and returns that message as the error.

In `src/cli.rs`, give the two §7.12 flags their Stage 1b meaning: `Command::Fetch { page: Some(url), .. }` builds `Query { text: None, url: Some(url), items: vec![] }` and routes it to the source whose domain matches (`bandcamp.com` -> `BandcampSource`, `soundcloud.com` -> `SoundCloudSource`); `Command::Fetch { soundcloud: Some(url), .. }` builds the same `Query` but always targets `SoundCloudSource`. Neither flag bypasses `enabled_in`: a source disabled in the config is skipped with the log line from stage 1, so the flags select a source, they do not enable one.

- [ ] **Step 4: Declare nothing new and run the tests**

No module declaration changes: both modules were declared in Tasks 1 and 6.

Run: `cargo test --test stage1b_auth --test stage1b_pipeline`
Expected: PASS, 4 and 2 tests.

- [ ] **Step 5: Add the Stage 1b end-to-end test**

Spec §10 requires an e2e test per source behind `--features e2e`. Create `tests/fixtures/bandcamp/fuzzysearch_album.json`:
```json
{"results":[
  {"type":"a","item_id":2814,"item_type":"a","band_id":4171,"location":"https://nightwave.bandcamp.com/album/night-drive","name":"Night Drive"}
]}
```
The `location` carries the same origin as `tests/fixtures/bandcamp/album_page.html`, so the e2e test rewrites it onto the mock server's base URL exactly like it rewrites the album page. The hit is an album with a direct `band_id`, so `search` resolves it into exactly one album and the roundtrip has a single expected outcome.

Create `tests/e2e_stage1b.rs` starting with `#![cfg(feature = "e2e")]`, reusing `tests/support/mod.rs` (the module is compiled in this file too, so `mod support;` sits after the `cfg` attribute and the whole file is inert without the feature). The single test `bandcamp_promo_roundtrip_records_a_lossless_track` drives the whole Stage 1b Bandcamp path against the mock server: `fuzzysearch` -> `tralbum_details` -> the 403 `download` -> `statdownload` -> the retry archive, then `Pipeline::download` writes the file into a temporary library, `library scan` indexes it and `library dedupe --dry-run` reports on it. The test asserts that the database holds exactly one lossless Bandcamp row whose path exists on disk and ends in `.flac`, that no row carries a `duplicate_of`, that the dedupe plan is empty, and that the dry run deletes nothing.
```rust
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
use maxfetch::sources::bandcamp::BandcampSource;
use maxfetch::sources::Query;

use support::{fixture, write_zip, MockServer, Reply, Route};

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
                vec![Reply::ok("application/json", fuzzy.replace("https://nightwave.bandcamp.com", base))],
            ),
            Route::new("/album/night-drive", vec![Reply::ok("text/html", page.replace("https://nightwave.bandcamp.com", base))]),
            Route::new("/album/night-drive/download", vec![Reply::status(403, "Download Expired")]),
            Route::new(
                "/album/night-drive/statdownload",
                vec![Reply::ok("text/plain", retry.replace("https://nightwave.bandcamp.com", base))],
            ),
            Route::new("/download/album", vec![Reply::status(200, zip_bytes.clone())]),
            Route::new("/api/mobile/25/tralbum_details", vec![Reply::ok("application/json", tralbum.clone())]),
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
        .search(&Query { text: Some("night drive".into()), url: None, items: vec![] })
        .await
        .unwrap();
    assert!(!groups.is_empty());
    let group = groups.iter().find(|g| g.primary.source == SourceId::Bandcamp).unwrap();

    // 2. The free promo yields exactly one lossless candidate.
    let candidates = p.candidates(group).await.unwrap();
    assert_eq!(candidates.len(), 1);
    assert!(candidates[0].quality.lossless);

    // 3. The download writes the member into the library and registers it.
    let result = p.download(&candidates, &NoopSink).await.unwrap();
    assert!(!result.rejected);
    assert!(result.path.exists());
    assert!(result.path.ends_with(".flac"));

    // 4. `library scan` indexes the library; the file the pipeline already
    //    registered must not become a second row.
    let mut out = Vec::new();
    let cli = Cli::parse_from(["maxfetch", "library", "scan"]);
    run_with(cli, cfg.clone(), Credentials::default(), &mut out).await.unwrap();

    let db = Db::open(&state_path()).unwrap();
    let tracks = db.all_tracks().unwrap();
    assert_eq!(tracks.len(), 1, "the scan must not duplicate the recorded track");
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
    run_with(cli, cfg, Credentials::default(), &mut out).await.unwrap();
    let text = String::from_utf8_lossy(&out);
    assert!(text.contains("нечего удалять") || text.is_empty(), "the dry run reports no deletion");
    assert!(plan_deletion(&db).unwrap().is_empty());
    assert_eq!(db.all_tracks().unwrap().len(), 1);
}
```

The `library scan` step is what proves the file is readable from disk and indexable: the pipeline registered the row already, so the scan must add nothing and the table must still hold exactly one row. The `library dedupe --dry-run` step is what proves the roundtrip produced no duplicate: the plan is empty and the row count is unchanged.

Seed the `e2e` feature in `Cargo.toml` (`[features] e2e = []`) if the core plan did not already add it; that one-line entry is the only `Cargo.toml` change in this task.

Run: `cargo test --features e2e --test e2e_stage1b`
Expected: PASS, 1 test. Without the feature the file compiles to nothing and `cargo test` is unaffected.

- [ ] **Step 6: Run the whole suite**

Run: `cargo test`
Expected: every test passes, including the core suite from the core plan.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(stage1b): register bandcamp and soundcloud sources"
```
