# maxfetch Core Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the `maxfetch` core: the crate skeleton, the domain model, quality ranking, cross-source matching, SQLite state, config/credentials, library layout, file probe, tag writing, the downloader, the `Source` trait with its registry, the fetch pipeline, deduplication, the headless CLI and the TUI shell.

**Architecture:** A single Rust crate `maxfetch`. Domain types live in `src/model.rs`; every source implements `src/sources/mod.rs::Source` and is registered in a static registry. The pipeline (`src/pipeline.rs`) is source-agnostic: it queries all enabled sources in parallel, groups releases across sources, ranks candidates by quality, downloads the best candidate, probes the real file, tags it and records it in SQLite. Both a headless CLI (`src/cli.rs`) and the TUI (`src/tui/`) drive the same pipeline.

**Tech Stack:** Rust 2024 edition, `tokio` (async runtime), `reqwest` (HTTP, rustls), `rusqlite` (state, WAL), `symphonia` (probe), `lofty` (tags), `ratatui` + `crossterm` (TUI), `clap` (CLI), `tracing` (logs), `anyhow`/`thiserror` (errors), `serde`/`serde_json`/`toml` (serialization).

**Spec:** `docs/superpowers/specs/2026-11-10-maxfetch-design.md` (sections 7 and 9 are the contract this plan implements; sections 8.1-8.9 describe the sources that later stage plans add).

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

## Review Focus

The spec is a vision document; these inputs are implied by it but not covered by any explicit requirement. Each has a test pinned to the task that owns the relevant code.

1. **A "24/96 FLAC" that is really a 16/44.1 transcode.** The source claims more quality than the file delivers. Expected: the file is rejected after probing, the next-best candidate is tried, and the database only ever stores the quality measured from the file. Pinned to Task 6 (probe) and Task 11 (pipeline).
2. **The same ISRC arriving from two different albums** (compilation vs. studio album). Expected: deduplication is global, the loser is marked `duplicate_of`, its file stays on disk untouched, and no album ends up with a hole in its tracklist. Pinned to Task 12 (dedup).
3. **One source is unreachable or unauthorized while the others work.** Expected: the run continues, the failing source is reported as skipped with a reason, and the remaining sources still deliver the track. Pinned to Task 11 (pipeline).
4. **Only lossy exists anywhere** for a track. Expected: the lossy file is kept as-is, never re-encoded, and carries the honest "источник lossy" marker in the tag comment, the database and the TUI badge. Pinned to Task 7 (tags) and Task 1 (ranking).
5. **A Bandcamp album ZIP that contains 12 tracks but only 3 are wanted/downloaded.** Expected: the archive is unpacked, each track becomes its own candidate, the missing 9 are simply absent from the result, and the run is reported per track rather than failing the whole album. Pinned to Task 11 (pipeline).

---

### Task 1: Crate skeleton, domain model, quality ranking

**Files:**
- Create: `Cargo.toml`, `src/lib.rs`, `src/main.rs`
- Create: `src/model.rs`
- Create: `src/quality.rs`
- Test: `tests/quality_ranking.rs`

**Interfaces:**
- Consumes: nothing (first task).
- Produces:
  - `maxfetch::model::SourceId` — `enum { Soulseek, Bandcamp, Youtube, Spotify, SoundCloud, Qobuz, Tidal, Deezer, Beatport }`, `Copy + Eq + Hash + Serialize + Deserialize`, with `SourceId::as_str(&self) -> &'static str` and `SourceId::from_str(&str) -> Option<SourceId>` (accepts the lowercase forms used in config and in the CLI).
  - `maxfetch::model::Container` — `enum { Flac, Wav, Aiff, Alac, M4aAac, Mp3, Opus, Webm }`, with `Container::as_str(&self) -> &'static str`, `Container::from_ext(&str) -> Option<Container>` (lowercase, `"m4a"` and `"mp4"` both map to `M4aAac`), `Container::is_lossless(&self) -> bool` (true for `Flac`, `Wav`, `Aiff`, `Alac`).
  - `maxfetch::model::Quality` — `struct { lossless: bool, container: Container, bit_depth: Option<u8>, sample_rate: Option<u32>, bitrate_kbps: Option<u32> }`, `Clone + Debug + PartialEq + Serialize + Deserialize + Default`.
  - `maxfetch::model::Release` — `struct { title: String, artist: String, album: Option<String>, track_no: Option<u32>, duration_sec: Option<f64>, isrc: Option<String>, year: Option<i32>, source: SourceId, source_item_id: String }`.
  - `maxfetch::model::Locator` — `enum { Soulseek { username: String, filename: String, size: u64 }, Bandcamp { url: String, encoding: String }, Youtube { url: String, video_id: String }, Spotify { track_id: String }, SoundCloud { track_id: String }, Qobuz { track_id: String, format_id: u32 }, Tidal { track_id: String, quality: u8 }, Deezer { track_id: String }, Beatport { track_id: String } }`.
  - `maxfetch::model::Candidate` — `struct { source: SourceId, release: Release, quality: Quality, size_bytes: Option<u64>, locator: Locator }`.
  - `maxfetch::quality::compare_quality(&Quality, &Quality) -> std::cmp::Ordering` — `Less` when the first is worse.
  - `maxfetch::quality::rank_candidates(&[Candidate]) -> Vec<Candidate>` — returns a new sorted vector, best first, with lossy candidates removed whenever at least one lossless candidate is present.

- [ ] **Step 1: Create the crate and add dependencies**

Run:
```bash
cargo new --lib maxfetch --vcs none
cd maxfetch
cargo add anyhow async-trait chrono clap --features clap/derive dirs futures-util lofty ratatui serde --features serde/derive serde_json symphonia --features symphonia/all tokio --features tokio/full toml tracing tracing-subscriber --features tracing-subscriber/env-filter
cargo add crossterm --features event-stream
cargo add reqwest --features json,stream,rustls-tls,cookies
cargo add uuid --features v4
cargo add rusqlite --features bundled
cargo add --dev tempfile
```
Expected: `Cargo.toml` lists all of them and `cargo build` succeeds. If `cargo add` rejects a feature name (feature sets change between releases), run `cargo add <crate>` without features and then `cargo add <crate> --features <list>` with the names printed by `cargo add <crate> --help`.

- [ ] **Step 2: Write the failing ranking test**

Create `tests/quality_ranking.rs`:
```rust
use maxfetch::model::{Candidate, Container, Locator, Quality, Release, SourceId};
use maxfetch::quality::{compare_quality, rank_candidates};

fn release() -> Release {
    Release {
        title: "Get Lucky".into(),
        artist: "Daft Punk".into(),
        album: Some("Random Access Memories".into()),
        track_no: Some(8),
        duration_sec: Some(369.0),
        isrc: None,
        year: Some(2013),
        source: SourceId::Soulseek,
        source_item_id: "x".into(),
    }
}

fn cand(container: Container, lossless: bool, depth: Option<u8>, rate: Option<u32>, kbps: Option<u32>) -> Candidate {
    Candidate {
        source: SourceId::Soulseek,
        release: release(),
        quality: Quality { lossless, container, bit_depth: depth, sample_rate: rate, bitrate_kbps: kbps },
        size_bytes: None,
        locator: Locator::Soulseek { username: "peer".into(), filename: "f".into(), size: 0 },
    }
}

#[test]
fn lossless_beats_lossy() {
    let flac = cand(Container::Flac, true, Some(16), Some(44100), None);
    let mp3 = cand(Container::Mp3, false, None, Some(44100), Some(320));
    assert_eq!(compare_quality(&flac, &mp3), std::cmp::Ordering::Greater);
}

#[test]
fn bit_depth_outranks_sample_rate() {
    let deep = cand(Container::Flac, true, Some(24), Some(48000), None);
    let shallow = cand(Container::Flac, true, Some(16), Some(96000), None);
    assert_eq!(compare_quality(&deep, &shallow), std::cmp::Ordering::Greater);
}

#[test]
fn higher_sample_rate_breaks_equal_depth() {
    let a = cand(Container::Flac, true, Some(16), Some(96000), None);
    let b = cand(Container::Flac, true, Some(16), Some(44100), None);
    assert_eq!(compare_quality(&a, &b), std::cmp::Ordering::Greater);
}

#[test]
fn unknown_depth_sorts_below_known() {
    let known = cand(Container::Flac, true, Some(16), Some(44100), None);
    let unknown = cand(Container::Flac, true, None, Some(44100), None);
    assert_eq!(compare_quality(&known, &unknown), std::cmp::Ordering::Greater);
}

#[test]
fn lossy_ranks_by_bitrate_then_rate() {
    let hi = cand(Container::Mp3, false, None, Some(44100), Some(320));
    let lo = cand(Container::Mp3, false, None, Some(44100), Some(128));
    assert_eq!(compare_quality(&hi, &lo), std::cmp::Ordering::Greater);
    let hi48 = cand(Container::Opus, false, None, Some(48000), Some(128));
    let lo44 = cand(Container::Opus, false, None, Some(44100), Some(128));
    assert_eq!(compare_quality(&hi48, &lo44), std::cmp::Ordering::Greater);
}

#[test]
fn rank_drops_lossy_when_lossless_exists() {
    let flac = cand(Container::Flac, true, Some(16), Some(44100), None);
    let mp3 = cand(Container::Mp3, false, None, Some(44100), Some(320));
    let ranked = rank_candidates(&[mp3, flac]);
    assert_eq!(ranked.len(), 1);
    assert!(ranked[0].quality.lossless);
}

#[test]
fn rank_keeps_lossy_when_it_is_all_we_have() {
    let mp3 = cand(Container::Mp3, false, None, Some(44100), Some(320));
    let opus = cand(Container::Opus, false, None, Some(48000), Some(128));
    let ranked = rank_candidates(&[opus, mp3]);
    assert_eq!(ranked.len(), 2);
    assert_eq!(ranked[0].quality.bitrate_kbps, Some(320));
}
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test --test quality_ranking`
Expected: FAIL with "unresolved import `maxfetch::model`".

- [ ] **Step 4: Implement `src/model.rs`**

Create `src/model.rs` with the types listed in the Interfaces block. `SourceId::as_str` returns `"soulseek"`, `"bandcamp"`, `"youtube"`, `"spotify"`, `"soundcloud"`, `"qobuz"`, `"tidal"`, `"deezer"`, `"beatport"`; `from_str` parses exactly those strings. `Container::as_str` returns `"flac"`, `"wav"`, `"aiff"`, `"alac"`, `"m4a"`, `"mp3"`, `"opus"`, `"webm"`. `Quality::default` is `lossless: false, container: Container::Opus` with all three numeric fields `None`.

- [ ] **Step 5: Implement `src/quality.rs`**

```rust
use std::cmp::Ordering;
use crate::model::{Candidate, Quality};

pub fn compare_quality(a: &Quality, b: &Quality) -> Ordering {
    match (a.lossless, b.lossless) {
        (true, false) => return Ordering::Greater,
        (false, true) => return Ordering::Less,
        _ => {}
    }
    if a.lossless {
        cmp_opt_desc(a.bit_depth, b.bit_depth)
            .then_with(|| cmp_opt_desc(a.sample_rate, b.sample_rate))
    } else {
        cmp_opt_desc(a.bitrate_kbps, b.bitrate_kbps)
            .then_with(|| cmp_opt_desc(a.sample_rate, b.sample_rate))
    }
}

fn cmp_opt_desc<T: Ord>(a: Option<T>, b: Option<T>) -> Ordering {
    match (a, b) {
        (Some(x), Some(y)) => y.cmp(&x),
        (Some(_), None) => Ordering::Greater,
        (None, Some(_)) => Ordering::Less,
        (None, None) => Ordering::Equal,
    }
}

pub fn rank_candidates(candidates: &[Candidate]) -> Vec<Candidate> {
    let has_lossless = candidates.iter().any(|c| c.quality.lossless);
    let mut kept: Vec<Candidate> = candidates
        .iter()
        .filter(|c| !has_lossless || c.quality.lossless)
        .cloned()
        .collect();
    kept.sort_by(|a, b| compare_quality(&b.quality, &a.quality));
    kept
}
```

- [ ] **Step 6: Wire up `src/lib.rs` and `src/main.rs`**

`src/lib.rs` declares `pub mod model; pub mod quality;`. `src/main.rs` is `fn main() {}` for now.

- [ ] **Step 7: Run the test to verify it passes**

Run: `cargo test --test quality_ranking`
Expected: PASS, 7 tests.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat(core): domain model and quality ranking"
```

---

### Task 2: Name normalization and cross-source release grouping

**Files:**
- Create: `src/match.rs`
- Test: `tests/matching.rs`

**Interfaces:**
- Consumes: `maxfetch::model::Release`, `SourceId` (Task 1).
- Produces:
  - `maxfetch::match::norm(&str) -> String` — lowercase, Unicode NFC, drops bracketed groups whose content matches a version marker, drops punctuation, collapses whitespace.
  - `maxfetch::match::artist_title_key(&Release) -> String` — `norm(artist) + "|" + norm(title)`.
  - `maxfetch::match::match_key(&Release) -> String` — `norm(artist) + "|" + norm(title) + "|" + norm(album)`, with an empty album segment when `album` is `None`.
  - `maxfetch::match::is_known_version_marker(&str) -> bool`.
  - `maxfetch::match::group_releases(Vec<Release>, duration_tolerance_sec: f64) -> Vec<ReleaseGroup>`.
  - `maxfetch::match::ReleaseGroup` — `struct { primary: Release, members: Vec<Release>, sources: Vec<SourceId> }`.

- [ ] **Step 1: Write the failing test**

Create `tests/matching.rs`:
```rust
use maxfetch::match::{group_releases, match_key, norm};
use maxfetch::model::{Release, SourceId};

fn rel(source: SourceId, title: &str, artist: &str, album: Option<&str>, isrc: Option<&str>, dur: Option<f64>) -> Release {
    Release {
        title: title.into(),
        artist: artist.into(),
        album: album.map(|s| s.into()),
        track_no: None,
        duration_sec: dur,
        isrc: isrc.map(|s| s.into()),
        year: None,
        source,
        source_item_id: "id".into(),
    }
}

#[test]
fn norm_strips_case_punctuation_and_version_markers() {
    assert_eq!(norm("Get Lucky (feat. Pharrell Williams) [Official Audio]"), "get lucky feat pharrell williams");
    assert_eq!(norm("Around The World (2007 Remaster)"), "around the world");
    assert_eq!(norm("One More Time - 2011 Remaster"), "one more time");
    assert_eq!(norm("Daft Punk — Live At Home"), "daft punk live at home");
}

#[test]
fn match_key_includes_album_or_empty_slot() {
    let with = rel(SourceId::Spotify, "T", "A", Some("Alb"), None, None);
    let without = rel(SourceId::Soulseek, "T", "A", None, None, None);
    assert_eq!(match_key(&with), "a|t|alb");
    assert_eq!(match_key(&without), "a|t|");
}

#[test]
fn same_isrc_groups_across_sources() {
    let a = rel(SourceId::Spotify, "Get Lucky", "Daft Punk", Some("RAM"), Some("FR123400001"), Some(369.0));
    let b = rel(SourceId::Beatport, "Get Lucky", "Daft Punk", Some("RAM"), Some("FR123400001"), Some(369.0));
    let groups = group_releases(vec![a, b], 2.0);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].sources.len(), 2);
}

#[test]
fn different_isrc_never_groups_even_with_same_name() {
    let a = rel(SourceId::Spotify, "Get Lucky", "Daft Punk", Some("RAM"), Some("FR123400001"), Some(369.0));
    let b = rel(SourceId::Beatport, "Get Lucky", "Daft Punk", Some("RAM"), Some("FR999999999"), Some(369.0));
    assert_eq!(group_releases(vec![a, b], 2.0).len(), 2);
}

#[test]
fn same_name_without_isrc_groups() {
    let a = rel(SourceId::Bandcamp, "Get Lucky", "Daft Punk", Some("RAM"), None, Some(369.0));
    let b = rel(SourceId::Soulseek, "Get Lucky", "Daft Punk", Some("RAM"), None, Some(369.0));
    assert_eq!(group_releases(vec![a, b], 2.0).len(), 1);
}

#[test]
fn different_album_same_track_groups_by_duration() {
    let a = rel(SourceId::Spotify, "Get Lucky", "Daft Punk", Some("RAM"), None, Some(369.0));
    let b = rel(SourceId::Soulseek, "Get Lucky", "Daft Punk", Some("Greatest Hits"), None, Some(370.5));
    assert_eq!(group_releases(vec![a, b], 2.0).len(), 1);
}

#[test]
fn duration_outside_tolerance_does_not_group() {
    let a = rel(SourceId::Spotify, "Get Lucky", "Daft Punk", Some("RAM"), None, Some(369.0));
    let b = rel(SourceId::Soulseek, "Get Lucky", "Daft Punk", Some("Greatest Hits"), None, Some(400.0));
    assert_eq!(group_releases(vec![a, b], 2.0).len(), 2);
}

#[test]
fn primary_prefers_spotify_metadata() {
    let spotify = rel(SourceId::Spotify, "Get Lucky", "Daft Punk", Some("RAM"), Some("FR123400001"), Some(369.0));
    let soulseek = rel(SourceId::Soulseek, "get lucky ", "daft punk", None, None, None);
    let groups = group_releases(vec![soulseek, spotify], 2.0);
    assert_eq!(groups[0].primary.source, SourceId::Spotify);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test matching`
Expected: FAIL with "unresolved import `maxfetch::match`".

- [ ] **Step 3: Implement `src/match.rs`**

`norm` implementation: convert to NFC, lowercase, then repeatedly remove bracketed groups `(\[[^\]]*\]|\([^)]*\))` whose inner text satisfies `is_known_version_marker`, then remove the standalone suffix pattern ` - <digits> remaster` (case-insensitive), then replace every character that is not a letter, digit or whitespace with a space, then collapse whitespace and trim.

`is_known_version_marker` returns true when the lowercased text contains any of: `remaster`, `live`, `remix`, `radio edit`, `edit`, `mono`, `stereo`, `demo`, `instrumental`, `version`, `bonus`, `deluxe`, `anniversary`, `reissue`, `official audio`, `official video`, `explicit`, `feat`, `featuring`. Anything else inside brackets is kept.

`group_releases`: build the groups greedily. Two releases belong to the same group when `same_release(a, b, tol)`:

```rust
fn same_release(a: &Release, b: &Release, tol: f64) -> bool {
    match (&a.isrc, &b.isrc) {
        (Some(x), Some(y)) => x == y,
        _ => {
            if match_key(a) == match_key(b) { return true; }
            if artist_title_key(a) != artist_title_key(b) { return false; }
            match (a.duration_sec, b.duration_sec) {
                (Some(x), Some(y)) => (x - y).abs() <= tol,
                _ => false,
            }
        }
    }
}
```

The primary of a group is the member whose `source` is `SourceId::Spotify` when such a member exists, otherwise the member with the highest metadata completeness (count of `Some` fields among `album`, `track_no`, `duration_sec`, `isrc`, `year`); ties break on the lower `source_item_id` for determinism. `sources` is the sorted unique list of member sources.

- [ ] **Step 4: Declare the module and run the test**

Add `pub mod match;` to `src/lib.rs`.

Run: `cargo test --test matching`
Expected: PASS, 8 tests.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(core): name normalization and release grouping"
```

---

### Task 3: Configuration, credentials and XDG paths

**Files:**
- Create: `src/config.rs`
- Test: `tests/config.rs`

**Interfaces:**
- Consumes: `maxfetch::model::SourceId` (Task 1).
- Produces:
  - `maxfetch::config::Config` — `struct { library: LibraryConfig, sources: SourcesConfig, matching: MatchingConfig, download: DownloadConfig, youtube: YoutubeConfig, media: MediaConfig }`, `Serialize + Deserialize + Clone + Debug`, with `Config::default()`.
  - `LibraryConfig { path: PathBuf }` — default `~/Music/maxfetch`.
  - `SourcesConfig { enabled: Vec<String> }` — default `["soulseek", "bandcamp", "youtube", "spotify"]`.
  - `MatchingConfig { duration_tolerance_sec: f64, require_isrc: bool }` — defaults `2.0`, `false`.
  - `DownloadConfig { max_concurrency: usize, retry_attempts: u32, timeout_sec: u64, stall_timeout_sec: u64 }` — defaults `4`, `3`, `60`, `30`.
  - `YoutubeConfig { yt_dlp_path: String }` — default `"yt-dlp"`.
  - `MediaConfig { ffmpeg_path: String }` — default `"ffmpeg"`.
  - `maxfetch::config::Credentials` — `struct` with one optional sub-struct per source: `soulseek: Option<SoulseekCredentials { username, password }>`, `bandcamp: Option<BandcampCredentials { identity_cookie }>`, `spotify: Option<SpotifyCredentials { client_id, client_secret, refresh_token }>`, `soundcloud: Option<SoundCloudCredentials { client_id, oauth_token }>`, `qobuz: Option<QobuzCredentials { email, password, app_id, app_secret }>`, `tidal: Option<TidalCredentials { client_id, client_secret }>`, `deezer: Option<DeezerCredentials { arl }>`, `beatport: Option<BeatportCredentials { client_id, client_secret, username, password }>`.
  - `maxfetch::config::config_dir() -> PathBuf`, `credentials_path() -> PathBuf`, `state_path() -> PathBuf`.
  - `maxfetch::config::Config::load_from(dir: &Path) -> anyhow::Result<Config>` and `Config::save_to(&self, dir: &Path) -> anyhow::Result<()>`.
  - `maxfetch::config::Credentials::load_from(dir: &Path) -> anyhow::Result<Credentials>` and `Credentials::save_to(&self, dir: &Path) -> anyhow::Result<()>` (writes mode 0600).
  - `maxfetch::config::ensure_private(path: &Path) -> anyhow::Result<bool>` — chmods the file to 0600 and returns `true` if the mode had to be tightened.

- [ ] **Step 1: Write the failing test**

Create `tests/config.rs`:
```rust
use std::fs;
use std::os::unix::fs::PermissionsExt;
use maxfetch::config::{Config, Credentials};

#[test]
fn defaults_are_usable_when_no_file_exists() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::load_from(dir.path()).unwrap();
    assert_eq!(cfg.matching.duration_tolerance_sec, 2.0);
    assert!(cfg.sources.enabled.contains(&"soulseek".to_string()));
}

#[test]
fn config_round_trips_through_disk() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::load_from(dir.path()).unwrap();
    cfg.library.path = "/tmp/music".into();
    cfg.sources.enabled = vec!["soulseek".into()];
    cfg.save_to(dir.path()).unwrap();

    let raw = fs::read_to_string(dir.path().join("maxfetch").join("config.toml")).unwrap();
    assert!(raw.contains("[library]"));

    let again = Config::load_from(dir.path()).unwrap();
    assert_eq!(again.library.path, PathBuf::from("/tmp/music"));
    assert_eq!(again.sources.enabled, vec!["soulseek".to_string()]);
}

#[test]
fn credentials_are_written_with_mode_0600() {
    let dir = tempfile::tempdir().unwrap();
    let mut creds = Credentials::default();
    creds.soulseek = Some(Default::default());
    creds.save_to(dir.path()).unwrap();
    let path = dir.path().join("maxfetch").join("credentials.toml");
    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

#[test]
fn missing_credentials_sections_are_fine() {
    let dir = tempfile::tempdir().unwrap();
    let creds = Credentials::load_from(dir.path()).unwrap();
    assert!(creds.soulseek.is_none());
    assert!(creds.qobuz.is_none());
}

#[test]
fn wide_permissions_are_tightened() {
    let dir = tempfile::tempdir().unwrap();
    let creds = Credentials::default();
    creds.save_to(dir.path()).unwrap();
    let path = dir.path().join("maxfetch").join("credentials.toml");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    let changed = maxfetch::config::ensure_private(&path).unwrap();
    assert!(changed);
    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test config`
Expected: FAIL with "unresolved import `maxfetch::config`".

- [ ] **Step 3: Implement `src/config.rs`**

Use `toml` for both files. `config_dir()` returns `$XDG_CONFIG_HOME/maxfetch` when `XDG_CONFIG_HOME` is set and non-empty, otherwise `dirs::home_dir()/".config"/"maxfetch"`. `state_path()` returns `$XDG_DATA_HOME/maxfetch/state.db` when `XDG_DATA_HOME` is set and non-empty, otherwise `dirs::data_dir()/maxfetch/state.db`. `Config::load_from(dir)` reads `dir/config.toml`, returning `Config::default()` when the file does not exist; a parse error is propagated with the file path in the message. `Config::save_to(dir)` creates the directory and writes `dir/config.toml` with `toml::to_string_pretty`. `Credentials::save_to(dir)` writes `dir/credentials.toml` and immediately sets mode 0600 on unix. `ensure_private` returns `false` when the file is already 0600 or absent.

- [ ] **Step 4: Declare the module and run the test**

Add `pub mod config;` to `src/lib.rs`.

Run: `cargo test --test config`
Expected: PASS, 5 tests.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(core): config, credentials and XDG paths"
```

---

### Task 4: SQLite state, migrations and track repository

**Files:**
- Create: `src/db.rs`
- Test: `tests/db.rs`

**Interfaces:**
- Consumes: `maxfetch::model::{Container, SourceId}` (Task 1).
- Produces:
  - `maxfetch::db::Db` — wraps a `rusqlite::Connection`.
  - `Db::open(path: &Path) -> anyhow::Result<Db>` (opens the file, creates parent directories, sets `PRAGMA journal_mode=WAL`, `PRAGMA foreign_keys=ON`, runs migrations).
  - `Db::open_in_memory() -> anyhow::Result<Db>`.
  - `Db::migrate(&self) -> anyhow::Result<()>` — creates `schema_version`, `tracks`, `releases`, `downloads`, `settings` exactly as in spec section 7.9, plus the three indexes `idx_tracks_isrc`, `idx_tracks_matchkey`, `idx_tracks_dup`.
  - `maxfetch::db::TrackRecord` — `struct { id: i64, isrc: Option<String>, match_key: String, title: String, artist: String, album: Option<String>, track_no: Option<u32>, duration_sec: Option<f64>, lossless: bool, container: Container, bit_depth: Option<u8>, sample_rate: Option<u32>, bitrate_kbps: Option<u32>, size_bytes: Option<u64>, path: String, source: SourceId, acquired_at: String, duplicate_of: Option<i64> }`.
  - `Db::insert_track(&self, record: &TrackRecord) -> anyhow::Result<i64>` — inserts, returns the new row id, fails on a duplicate `path`.
  - `Db::find_by_isrc(&self, isrc: &str) -> anyhow::Result<Vec<TrackRecord>>`.
  - `Db::find_by_match_key(&self, key: &str) -> anyhow::Result<Vec<TrackRecord>>`.
  - `Db::all_tracks(&self) -> anyhow::Result<Vec<TrackRecord>>`.
  - `Db::set_duplicate_of(&self, loser_id: i64, winner_id: i64) -> anyhow::Result<()>`.
  - `Db::duplicate_groups(&self) -> anyhow::Result<Vec<DuplicateGroup>>`.
  - `maxfetch::db::DuplicateGroup` — `struct { winner: TrackRecord, losers: Vec<TrackRecord>, conflict: bool }`; `conflict` is true when the loser's quality is indistinguishable from the winner's (same lossless flag, same bit depth, same sample rate, same bitrate).
  - `Db::delete_track(&self, id: i64) -> anyhow::Result<()>` — deletes the row only, never touches the file.
  - `Db::log_download(&self, source: SourceId, track_id: Option<i64>, state: &str, reason: Option<&str>) -> anyhow::Result<()>`.

- [ ] **Step 1: Write the failing test**

Create `tests/db.rs`:
```rust
use maxfetch::db::{Db, TrackRecord};
use maxfetch::model::{Container, SourceId};

fn record(path: &str, isrc: Option<&str>, key: &str) -> TrackRecord {
    TrackRecord {
        id: 0,
        isrc: isrc.map(|s| s.into()),
        match_key: key.into(),
        title: "Get Lucky".into(),
        artist: "Daft Punk".into(),
        album: Some("RAM".into()),
        track_no: Some(8),
        duration_sec: Some(369.0),
        lossless: true,
        container: Container::Flac,
        bit_depth: Some(16),
        sample_rate: Some(44100),
        bitrate_kbps: None,
        size_bytes: Some(1234),
        path: path.into(),
        source: SourceId::Soulseek,
        acquired_at: "2026-11-10T00:00:00Z".into(),
        duplicate_of: None,
    }
}

#[test]
fn insert_and_read_back() {
    let db = Db::open_in_memory().unwrap();
    let id = db.insert_track(&record("/m/a.flac", Some("FR1"), "a|t|ram")).unwrap();
    assert!(id > 0);
    let found = db.find_by_isrc("FR1").unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].path, "/m/a.flac");
}

#[test]
fn duplicate_path_is_rejected() {
    let db = Db::open_in_memory().unwrap();
    db.insert_track(&record("/m/a.flac", None, "k")).unwrap();
    let err = db.insert_track(&record("/m/a.flac", None, "k")).unwrap_err();
    assert!(format!("{err}").to_lowercase().contains("unique"));
}

#[test]
fn duplicate_group_reports_winner_and_loser() {
    let db = Db::open_in_memory().unwrap();
    let a = db.insert_track(&record("/m/a.flac", Some("FR1"), "k")).unwrap();
    let b = db.insert_track(&record("/m/b.flac", Some("FR1"), "k")).unwrap();
    db.set_duplicate_of(b, a).unwrap();
    let groups = db.duplicate_groups().unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].winner.id, a);
    assert_eq!(groups[0].losers.len(), 1);
    assert_eq!(groups[0].losers[0].id, b);
}

#[test]
fn delete_track_removes_only_the_row() {
    let db = Db::open_in_memory().unwrap();
    let a = db.insert_track(&record("/m/a.flac", Some("FR1"), "k")).unwrap();
    db.delete_track(a).unwrap();
    assert!(db.all_tracks().unwrap().is_empty());
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test db`
Expected: FAIL with "unresolved import `maxfetch::db`".

- [ ] **Step 3: Implement `src/db.rs`**

Use `rusqlite` 0.41 with the `params![]` macro from `rusqlite::params`. Open with `Connection::open(path)`, create the parent directories first, then set `PRAGMA foreign_keys=ON` with `conn.pragma_update(None, "foreign_keys", true)` and WAL with `conn.query_row("PRAGMA journal_mode=WAL", [], |row| row.get::<_, String>(0))`. The WAL pragma is the one exception to `pragma_update`: `PRAGMA journal_mode=WAL` returns a row, and `pragma_update` runs the statement through `execute`, which fails with "Execute returned results" — so `query_row` is required here, and the returned mode string is ignored. Store `source` as the `SourceId::as_str()` string and `container` as `Container::as_str()`; convert back on read. Store `acquired_at` as an RFC 3339 UTC timestamp produced by `chrono::Utc::now().to_rfc3339()`. `duplicate_groups` selects every track with a non-null `duplicate_of`, loads the winners, and groups them by winner id; the `conflict` flag is computed as described in the Interfaces block.

- [ ] **Step 4: Declare the module and run the test**

Add `pub mod db;` to `src/lib.rs`.

Run: `cargo test --test db`
Expected: PASS, 4 tests.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(core): sqlite state and track repository"
```

---

### Task 5: Library paths and filename sanitization

**Files:**
- Create: `src/library.rs`
- Test: `tests/library.rs`

**Interfaces:**
- Consumes: `maxfetch::model::Release` (Task 1).
- Produces:
  - `maxfetch::library::sanitize_component(&str) -> String` — replaces `/ \ : * ? " < > |` with `_`, collapses whitespace runs to one space, trims leading/trailing spaces and dots, appends `_` to Windows-reserved names (`CON`, `PRN`, `AUX`, `NUL`, `COM1`..`COM9`, `LPT1`..`LPT9`, case-insensitive), and truncates the result to 240 bytes at a word boundary.
  - `maxfetch::library::relative_path(release: &Release, ext: &str) -> String` — `"<Artist>/<Album>/<NN> <Title>.<ext>"`, album defaulting to `Singles`, `NN` being a zero-padded two-digit track number or `01` when `track_no` is `None`.
  - `maxfetch::library::absolute_path(root: &Path, release: &Release, ext: &str) -> PathBuf`.

- [ ] **Step 1: Write the failing test**

Create `tests/library.rs`:
```rust
use maxfetch::library::{relative_path, sanitize_component};
use maxfetch::model::Release;

fn rel(artist: &str, album: Option<&str>, track_no: Option<u32>) -> Release {
    Release {
        title: "Get Lucky".into(),
        artist: artist.into(),
        album: album.map(|s| s.into()),
        track_no,
        duration_sec: Some(369.0),
        isrc: None,
        year: None,
        source: maxfetch::model::SourceId::Soulseek,
        source_item_id: "x".into(),
    }
}

#[test]
fn forbidden_characters_are_replaced() {
    assert_eq!(sanitize_component("AC/DC"), "AC_DC");
    assert_eq!(sanitize_component("A: B? C* D"), "A_ B_ C_ D");
}

#[test]
fn leading_and_trailing_dots_and_spaces_are_trimmed() {
    assert_eq!(sanitize_component("  ..Foo.  "), "Foo");
}

#[test]
fn windows_reserved_names_get_a_suffix() {
    assert_eq!(sanitize_component("CON"), "CON_");
    assert_eq!(sanitize_component("lpt1"), "lpt1_");
}

#[test]
fn whitespace_is_collapsed() {
    assert_eq!(sanitize_component("Get    Lucky"), "Get Lucky");
}

#[test]
fn relative_path_uses_artist_album_track_title() {
    assert_eq!(relative_path(&rel("Daft Punk", Some("RAM"), Some(8)), "flac"), "Daft Punk/RAM/08 Get Lucky.flac");
}

#[test]
fn missing_album_falls_back_to_singles() {
    assert_eq!(relative_path(&rel("Daft Punk", None, None), "flac"), "Daft Punk/Singles/01 Get Lucky.flac");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test library`
Expected: FAIL with "unresolved import `maxfetch::library`".

- [ ] **Step 3: Implement `src/library.rs`**

Truncation to 240 bytes must not cut a multi-byte character in half: cut at a byte offset that is a character boundary and, if possible, at the last space before it.

- [ ] **Step 4: Declare the module and run the test**

Add `pub mod library;` to `src/lib.rs`.

Run: `cargo test --test library`
Expected: PASS, 6 tests.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(core): library paths and filename sanitization"
```

---

### Task 6: Probing the real quality of a downloaded file

**Files:**
- Create: `src/probe.rs`
- Create: `tests/fixtures/probe/` — a tiny 44.1 kHz 16-bit FLAC and a 48 kHz 24-bit FLAC, generated once by a script and committed.
- Test: `tests/probe.rs`

**Interfaces:**
- Consumes: `maxfetch::model::Container` (Task 1).
- Produces:
  - `maxfetch::probe::ProbeResult` — `struct { container: Container, bit_depth: Option<u8>, sample_rate: Option<u32>, bitrate_kbps: Option<u32>, duration_sec: Option<f64>, channels: Option<u16> }`.
  - `maxfetch::probe::probe(path: &Path) -> anyhow::Result<ProbeResult>` — reads the container header only, never decodes audio.

  Field derivation, exactly as implemented in Step 4 (fields come from `AudioCodecParameters` and the track's time base, not from a convenience accessor):
  - `sample_rate` ← `track.codec_params.as_ref().and_then(|p| p.audio()).and_then(|a| a.sample_rate)`.
  - `bit_depth` ← the same `AudioCodecParameters`, `bits_per_sample` (falling back to `bits_per_coded_sample` when `bits_per_sample` is `None`).
  - `channels` ← `audio.channels.map(|c| c.count() as u16)`, because `Channels::count(&self) -> usize`.
  - `duration_sec` ← the track's own time base is read as `track.time_base`, falling back to `MediaInfo::time_base` when the track carries none; the duration is `track.duration` (or `MediaInfo::from_track(&track).duration`, the first non-`None`). One tick is `numer/denom` seconds, so `duration_sec = duration.get() as f64 * time_base.denom as f64 / time_base.numer as f64`. `TimeBase::calc_duration(duration)` returns the same span as a `Time`, not as bare seconds, so it still needs that conversion to a value in seconds; there is no ready-made "seconds" accessor. `duration_sec` is `None` when either the duration or the time base is absent, and it is never guessed.
  - `bitrate_kbps` ← derived from the file size and `duration_sec` (`size * 8 / duration_sec / 1000`), `None` when `duration_sec` is `None`.

- [ ] **Step 1: Create the fixtures**

Generate two FLAC files with `ffmpeg` (available on this machine) and commit them:

```bash
mkdir -p tests/fixtures/probe
ffmpeg -f lavfi -i "sine=frequency=440:duration=1" -c:a flac -ar 44100 -sample_fmt s16 tests/fixtures/probe/sine_44100_s16.flac -y
ffmpeg -f lavfi -i "sine=frequency=440:duration=1" -c:a flac -ar 48000 -sample_fmt s32 tests/fixtures/probe/sine_48000_s24.flac -y
```
Expected: two files exist, each roughly 20-40 KB.

- [ ] **Step 2: Write the failing test**

Create `tests/probe.rs`:
```rust
use std::path::PathBuf;
use maxfetch::model::Container;
use maxfetch::probe::probe;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/probe").join(name)
}

#[test]
fn probe_reads_sample_rate_and_depth_of_flac() {
    let r = probe(&fixture("sine_44100_s16.flac")).unwrap();
    assert_eq!(r.container, Container::Flac);
    assert_eq!(r.sample_rate, Some(44100));
    assert_eq!(r.bit_depth, Some(16));
    assert_eq!(r.channels, Some(2));
    assert!(r.duration_sec.unwrap() > 0.9 && r.duration_sec.unwrap() < 1.1);
}

#[test]
fn probe_reports_actual_depth_not_claimed_depth() {
    let r = probe(&fixture("sine_48000_s24.flac")).unwrap();
    assert_eq!(r.sample_rate, Some(48000));
    assert_eq!(r.bit_depth, Some(24));
}

#[test]
fn probe_fails_on_a_missing_file() {
    assert!(probe(&fixture("nope.flac")).is_err());
}
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test --test probe`
Expected: FAIL with "unresolved import `maxfetch::probe`".

- [ ] **Step 4: Implement `src/probe.rs`**

symphonia 0.6 split its docs into per-crate modules; the umbrella crate only re-exports them. The probe API is `symphonia::default::get_probe() -> &'static Probe`, and the only entry point is:

```rust
pub fn probe<'s>(&self, hint: &Hint, mss: MediaSourceStream<'s>, fmt_opts: FormatOptions, meta_opts: MetadataOptions) -> Result<Box<dyn FormatReader + 's>>
```

There is no `Probe::format` and no `Probe::new(format_hint)`. Build the stream and the hint explicitly:

```rust
let mut hint = Hint::new();
hint.with_extension(path.extension().and_then(|e| e.to_str()).unwrap_or(""));
let mss = MediaSourceStream::new(Box::new(File::open(path)?), MediaSourceStreamOptions::default());
let format = symphonia::default::get_probe()
    .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())?;
```

From the returned `Box<dyn FormatReader>` read `format.media_info()` (`MediaInfo`, which is `#[non_exhaustive]` and carries `time_base`, `duration`, `start_ts`) and `format.default_track(TrackType::Audio)` or the first entry of `format.tracks()`. Per track use `track.codec_params` (`Option<CodecParameters>`, with `is_audio()` / `audio()` / `audio_mut()`) and the `AudioCodecParameters` struct's plain fields — `codec`, `sample_rate`, `sample_format`, `bits_per_sample`, `bits_per_coded_sample`, `channels`. There is no `AudioCodecParametersBuilder` type, and channels are counted with `Channels::count(&self) -> usize` (`Channels` is `#[non_exhaustive]`: `Positioned`, `Discrete`, `Ambisonic`, `Custom`, `None`).

Duration comes from `Duration::get(self) -> u64`, in ticks of the track's own time base, read as `track.time_base` and falling back to `MediaInfo::time_base` when the track has none. `TimeBase { numer, denom }` are `NonZero<u32>` and one tick is `numer/denom` seconds, so `duration_sec = duration.get() as f64 * time_base.denom as f64 / time_base.numer as f64`. `time_base.calc_duration(duration)` returns the same span as a `Time` (`seconds as f64 + frac`), not as bare seconds, so it still needs that conversion; there is no ready-made "seconds" accessor. When either value is missing, `duration_sec` is `None` — never guess. Do not import `symphonia::core::formats::flac::read` or call a `read_properties` function: those do not exist. If direct FLAC access is ever needed, it is `symphonia_bundle_flac::FlacReader::try_new(mss, FormatOptions::default())`.

Map the probed format to a `Container`: FLAC → `Flac`, WAV → `Wav`, AIFF → `Aiff`, MP3 → `Mp3` (`MpaReader`; `Mp3Reader` is only a deprecated alias), Ogg/Opus → `Opus`, MP4/M4A → `M4aAac`, WebM/Matroska → `Webm`. FLAC bit depth comes from the codec parameters' `bits_per_sample` (the STREAMINFO value); when it is absent, fall back to `bits_per_coded_sample`. `symphonia::default::get_codecs() -> &'static CodecRegistry` is only needed if a codec is instantiated, which header-only probing never does.

- [ ] **Step 5: Declare the module and run the test**

Add `pub mod probe;` to `src/lib.rs`.

Run: `cargo test --test probe`
Expected: PASS, 3 tests.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(core): probe real file quality with symphonia"
```

---

### Task 7: Writing tags and the honest lossy marker

**Files:**
- Create: `src/tags.rs`
- Test: `tests/tags.rs`

**Interfaces:**
- Consumes: `maxfetch::model::{Container, Quality}` (Task 1).
- Produces:
  - `maxfetch::tags::TagMeta` — `struct { title: String, artist: String, album: Option<String>, track_no: Option<u32>, year: Option<i32>, isrc: Option<String>, quality: Quality, source: SourceId, acquired_at: String, quality_comment: String }`; `quality_comment` holds the raw `COMMENT` tag as read back from the file, empty when absent.
  - `maxfetch::tags::write_tags(path: &Path, meta: &TagMeta) -> anyhow::Result<()>` — writes the metadata into an existing file.
  - `maxfetch::tags::lossy_comment(source: SourceId, container: Container, acquired_at: &str) -> String` — returns exactly `maxfetch: source=lossy; container=<Container::as_str()>; source_id=<SourceId::as_str()>; acquired=<acquired_at>`.
  - `maxfetch::tags::read_tags(path: &Path) -> anyhow::Result<TagMeta>` — used by `library scan` and by tests.

  Tag type per container (lofty has no Matroska/WebM support at all): `TagType::VorbisComments` for `Flac`, `Opus` and Ogg/Vorbis; `TagType::Id3v2` for `Mp3`; `TagType::Mp4Ilst` for `M4aAac`; `TagType::RiffInfo` for `Wav`; `TagType::AiffText` for `Aiff`. For `Webm` there is no tag type: `write_tags` writes nothing and the file keeps only the JSON sidecar from Task 9. `Alac` inside MP4 uses `Mp4Ilst` as well.

- [ ] **Step 1: Write the failing test**

Create `tests/tags.rs`:
```rust
use std::path::PathBuf;
use maxfetch::model::{Container, Quality, SourceId};
use maxfetch::tags::{lossy_comment, read_tags, write_tags, TagMeta};

fn fixture() -> PathBuf {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/probe/sine_44100_s16.flac");
    let dst = std::env::temp_dir().join(format!("maxfetch-tag-{}.flac", std::process::id()));
    std::fs::copy(&src, &dst).unwrap();
    dst
}

#[test]
fn writes_core_fields_and_isrc() {
    let path = fixture();
    let meta = TagMeta {
        title: "Get Lucky".into(),
        artist: "Daft Punk".into(),
        album: Some("Random Access Memories".into()),
        track_no: Some(8),
        year: Some(2013),
        isrc: Some("FR123400001".into()),
        quality: Quality { lossless: true, container: Container::Flac, bit_depth: Some(16), sample_rate: Some(44100), bitrate_kbps: None },
        source: SourceId::Soulseek,
        acquired_at: "2026-11-10T00:00:00Z".into(),
    };
    write_tags(&path, &meta).unwrap();
    let back = read_tags(&path).unwrap();
    assert_eq!(back.title, "Get Lucky");
    assert_eq!(back.artist, "Daft Punk");
    assert_eq!(back.album.as_deref(), Some("Random Access Memories"));
    assert_eq!(back.track_no, Some(8));
    assert_eq!(back.year, Some(2013));
    assert_eq!(back.isrc.as_deref(), Some("FR123400001"));
}

#[test]
fn lossy_files_get_the_honest_marker() {
    let path = fixture();
    let meta = TagMeta {
        title: "Clip".into(),
        artist: "Someone".into(),
        album: None,
        track_no: None,
        year: None,
        isrc: None,
        quality: Quality { lossless: false, container: Container::Opus, bit_depth: None, sample_rate: Some(48000), bitrate_kbps: Some(160) },
        source: SourceId::Youtube,
        acquired_at: "2026-11-10T00:00:00Z".into(),
    };
    write_tags(&path, &meta).unwrap();
    let comment = read_tags(&path).unwrap().quality_comment;
    assert!(comment.contains("source=lossy"));
    assert!(comment.contains("container=opus"));
    assert!(comment.contains("source_id=youtube"));
}

#[test]
fn lossless_files_get_no_lossy_marker() {
    let path = fixture();
    let meta = TagMeta {
        title: "Clip".into(),
        artist: "Someone".into(),
        album: None,
        track_no: None,
        year: None,
        isrc: None,
        quality: Quality { lossless: true, container: Container::Flac, bit_depth: Some(16), sample_rate: Some(44100), bitrate_kbps: None },
        source: SourceId::Soulseek,
        acquired_at: "2026-11-10T00:00:00Z".into(),
    };
    write_tags(&path, &meta).unwrap();
    assert!(!read_tags(&path).unwrap().quality_comment.contains("source=lossy"));
}

#[test]
fn lossy_comment_format_is_exact() {
    assert_eq!(
        lossy_comment(SourceId::Youtube, Container::Opus, "2026-11-10T00:00:00Z"),
        "maxfetch: source=lossy; container=opus; source_id=youtube; acquired=2026-11-10T00:00:00Z"
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test tags`
Expected: FAIL with "unresolved import `maxfetch::tags`".

- [ ] **Step 3: Implement `src/tags.rs`**

Use `lofty` 0.25. Read with `lofty::read_from_path(path) -> Result<TaggedFile, FileParseError>`; that `TaggedFile` implements `TaggedFileExt`, so use `file_type() -> FileType` to choose the tag type and `tag(TagType)` / `tag_mut(TagType)` to reach the container's existing tag.

Writing:
1. Build the `Tag` for the container: `Tag::new(TagType::VorbisComments)` for FLAC/Opus/Ogg, `TagType::Id3v2` for MP3, `TagType::Mp4Ilst` for M4A/AAC/ALAC, `TagType::RiffInfo` for WAV, `TagType::AiffText` for AIFF.
2. Insert each field with `TagItem::new(ItemKey::X, ItemValue::Text(value))` and `tag.insert(item)`, using the exact `ItemKey` names: `TrackTitle`, `TrackArtist`, `AlbumTitle`, `AlbumArtist`, `TrackNumber`, `TrackTotal`, `DiscNumber`, `Year`, `RecordingDate`, `Comment`, `Isrc`, `Genre`, `Barcode`. `track_no` goes to `TrackNumber` as a string; `year` to `Year`; `isrc` to `Isrc`.
3. The lossy marker goes into `ItemKey::Comment` via the same `TagItem`/`ItemValue::Text` pair when `meta.quality.lossless` is false; for a lossless file call `tag.remove_key(ItemKey::Comment)` so no stale marker survives. (`TagItem::new` takes `ItemValue`; `ItemValue` is `Text`, `Locator` or `Binary` — only `Text` is used here.)
4. Persist with `TagExt::save_to_path(path, WriteOptions::new())`, or attach the tag to the `TaggedFile` with `TaggedFileExt::insert_tag(tag)` / `tag_mut` and call `AudioFile::save_to_path(&file, path, WriteOptions::new())`. `WriteOptions::new()` is fine as-is; `respect_read_only` / `use_id3v23` / `preferred_padding` / `remove_others` are optional refinements.

`read_tags` calls `lofty::read_from_path`, resolves the tag through `TaggedFileExt::tag(TagType)`, reads strings with `Tag::get_string(ItemKey::X) -> Option<&str>` (e.g. `TrackTitle`, `TrackArtist`, `AlbumTitle`, `TrackNumber`, `Year`, `Isrc`), and fills `quality_comment` from `Tag::get_string(ItemKey::Comment)` — empty string when absent. Parse `TrackNumber`/`Year` back into `u32`/`i32`.

For `Container::Webm`, lofty has no Matroska/WebM reader: `write_tags` returns `Ok(())` without touching the file and `read_tags` reports an empty `quality_comment`, so the `.webm` fallback relies on the sidecar written in Task 9. Do not look for a Matroska tag type; `FileType::Custom` is the closest lofty gets and it is not written.

- [ ] **Step 4: Declare the module and run the test**

Add `pub mod tags;` to `src/lib.rs`.

Run: `cargo test --test tags`
Expected: PASS, 4 tests.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(core): tag writing and lossy marker"
```

---

### Task 8: HTTP downloader with progress and atomic rename

**Files:**
- Create: `src/fetch.rs`
- Test: `tests/fetch.rs`

**Interfaces:**
- Consumes: nothing beyond the standard library and `reqwest`.
- Produces:
  - `maxfetch::fetch::ProgressSink` — `trait { fn on_progress(&self, transferred: u64, total: Option<u64>); fn on_finished(&self, transferred: u64); }`, `Send + Sync`.
  - `maxfetch::fetch::NoopSink` — a `ProgressSink` that does nothing.
  - `maxfetch::fetch::download_to_file(client: &reqwest::Client, url: &str, dest: &Path, sink: &dyn ProgressSink) -> anyhow::Result<()>` — streams the body into `dest.with_extension("part")` and renames it to `dest` only after a complete transfer; on error the `.part` file is left in place and the error is returned.
  - `maxfetch::fetch::http_client(timeout_sec: u64) -> anyhow::Result<reqwest::Client>` — a client with a browser-like User-Agent, cookies enabled and the given timeout.

- [ ] **Step 1: Write the failing test**

Create `tests/fetch.rs`:
```rust
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use maxfetch::fetch::{download_to_file, ProgressSink};

struct Counter(AtomicU64);
impl ProgressSink for Counter {
    fn on_progress(&self, transferred: u64, _total: Option<u64>) { self.0.fetch_add(transferred, Ordering::SeqCst); }
    fn on_finished(&self, _transferred: u64) {}
}

#[tokio::test]
async fn downloads_a_file_and_renames_it() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.bin");
    let sink = Arc::new(Counter(AtomicU64::new(0)));
    download_to_file(&reqwest::Client::new(), "file:///etc/hostname", &dest, sink.as_ref()).await.unwrap();
    assert!(dest.exists());
    assert!(!dir.path().join("out.part").exists());
}
```
If `reqwest` refuses the `file://` scheme, replace the body of the test with a tiny local HTTP server: spin up a `tokio::net::TcpListener` on port 0, write a fixed HTTP/1.1 response with `Content-Length`, and download from `http://127.0.0.1:<port>/file`.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test fetch`
Expected: FAIL with "unresolved import `maxfetch::fetch`".

- [ ] **Step 3: Implement `src/fetch.rs`**

Use `reqwest::Response::bytes_stream()` with the `stream` feature and `futures_util::StreamExt::next`. Compute `total` from the `Content-Length` header when present. Write into `dest` with the extension replaced by `part` (`dest.with_extension("part")`), `std::fs::File::create` it, and `tokio::fs::rename` it to `dest` at the end. Report progress through the sink after every chunk.

- [ ] **Step 4: Declare the module and run the test**

Add `pub mod fetch;` to `src/lib.rs`.

Run: `cargo test --test fetch`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(core): streaming http downloader with atomic rename"
```

---

### Task 9: Container remux without re-encoding

**Files:**
- Create: `src/media.rs`
- Test: `tests/media.rs`

**Interfaces:**
- Consumes: `maxfetch::config::MediaConfig` (Task 3).
- Produces:
  - `maxfetch::media::remux_to_opus(ffmpeg_path: &str, input: &Path, output: &Path) -> anyhow::Result<()>` — runs `ffmpeg -nostdin -y -i <input> -vn -c copy <output>`; the audio stream is copied, never re-encoded.
  - `maxfetch::media::ffmpeg_available(ffmpeg_path: &str) -> bool`.
  - `maxfetch::media::write_sidecar(path: &Path, payload: &serde_json::Value) -> anyhow::Result<()>` — writes `<path>.json` next to the media file.

- [ ] **Step 1: Write the failing test**

Create `tests/media.rs`:
```rust
use std::path::PathBuf;
use maxfetch::media::{ffmpeg_available, remux_to_opus, write_sidecar};

#[test]
fn ffmpeg_is_detected_on_this_machine() {
    assert!(ffmpeg_available("ffmpeg"));
}

#[test]
fn remux_copies_the_stream_without_reencoding() {
    let dir = tempfile::tempdir().unwrap();
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/probe/sine_44100_s16.flac");
    let webm = dir.path().join("clip.webm");
    let out = dir.path().join("clip.opus");

    std::process::Command::new("ffmpeg").args(["-nostdin", "-y", "-i"]).arg(&src).args(["-vn", "-c", "copy", "-c:a", "libopus"]).arg(&webm).status().unwrap();

    remux_to_opus("ffmpeg", &webm, &out).unwrap();
    assert!(out.exists());
    let size = std::fs::metadata(&out).unwrap().len();
    assert!(size > 1000);
}

#[test]
fn sidecar_json_is_written_next_to_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let media = dir.path().join("clip.webm");
    std::fs::write(&media, b"x").unwrap();
    write_sidecar(&media, &serde_json::json!({"source": "youtube"})).unwrap();
    let raw = std::fs::read_to_string(dir.path().join("clip.webm.json")).unwrap();
    assert!(raw.contains("youtube"));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test media`
Expected: FAIL with "unresolved import `maxfetch::media`".

- [ ] **Step 3: Implement `src/media.rs`**

`remux_to_opus` spawns `ffmpeg` and fails when the exit code is non-zero, including the tail of stderr in the error message. `ffmpeg_available` runs `ffmpeg -version` and checks the exit status.

- [ ] **Step 4: Declare the module and run the test**

Add `pub mod media;` to `src/lib.rs`.

Run: `cargo test --test media`
Expected: PASS, 3 tests.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(core): ffmpeg remux helper and sidecar metadata"
```

---

### Task 10: The `Source` trait, capabilities and the registry

**Files:**
- Create: `src/sources/mod.rs`
- Test: `tests/source_trait.rs`

**Interfaces:**
- Consumes: `maxfetch::model::{Candidate, Release, SourceId}`, `maxfetch::config::{Config, Credentials}` (Tasks 1 and 3).
- Produces:
  - `maxfetch::sources::Capabilities` — `struct { audio: bool, metadata_only: bool, lossless: bool, needs_auth: bool }`.
  - `maxfetch::sources::AuthState` — `enum { NotNeeded, Missing, Ok, Expired }`.
  - `maxfetch::sources::Query` — `struct { text: Option<String>, url: Option<String>, items: Vec<QueryItem> }` where `QueryItem` is `enum { Track { source: SourceId, id: String }, Album { source: SourceId, id: String }, Playlist { source: SourceId, id: String } }`.
  - `maxfetch::sources::SourceStatus` — `struct { id: SourceId, capabilities: Capabilities, auth: AuthState, detail: Option<String> }`.
  - `maxfetch::sources::Source` — `#[async_trait] pub trait Source: Send + Sync { fn id(&self) -> SourceId; fn capabilities(&self) -> Capabilities; fn auth_state(&self) -> AuthState; async fn search(&self, q: &Query) -> anyhow::Result<Vec<Release>>; async fn resolve(&self, q: &Query) -> anyhow::Result<Vec<Release>>; async fn candidates(&self, release: &Release) -> anyhow::Result<Vec<Candidate>>; async fn download(&self, candidate: &Candidate, dest: &Path, sink: &dyn ProgressSink) -> anyhow::Result<()>; }`.
  - `maxfetch::sources::registry(config: &Config, credentials: &Credentials) -> Vec<Box<dyn Source>>` — returns one boxed source per source id that has an implementation in this build; the list is empty in this task and grows in the stage plans.
  - `maxfetch::sources::enabled_in(config: &Config, id: SourceId) -> bool`.

- [ ] **Step 1: Write the failing test**

Create `tests/source_trait.rs`:
```rust
use maxfetch::config::{Config, Credentials};
use maxfetch::sources::{enabled_in, registry};
use maxfetch::model::SourceId;

#[test]
fn registry_is_empty_before_sources_are_added() {
    let cfg = Config::default();
    let creds = Credentials::default();
    assert!(registry(&cfg, &creds).is_empty());
}

#[test]
fn enabled_in_follows_the_configured_list() {
    let mut cfg = Config::default();
    cfg.sources.enabled = vec!["soulseek".into()];
    assert!(enabled_in(&cfg, SourceId::Soulseek));
    assert!(!enabled_in(&cfg, SourceId::Qobuz));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test source_trait`
Expected: FAIL with "unresolved import `maxfetch::sources`".

- [ ] **Step 3: Implement `src/sources/mod.rs`**

Define the trait exactly as in the Interfaces block, with `#[async_trait]`. `registry` returns an empty `Vec` for now; a comment marks the place where the stage plans register their sources. `enabled_in` compares the configured strings against `SourceId::as_str()`.

- [ ] **Step 4: Declare the module and run the test**

Add `pub mod sources;` to `src/lib.rs`.

Run: `cargo test --test source_trait`
Expected: PASS, 2 tests.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(core): source trait, capabilities and registry"
```

---

### Task 11: The fetch pipeline

**Files:**
- Create: `src/pipeline.rs`
- Test: `tests/pipeline.rs`

**Interfaces:**
- Consumes: `maxfetch::sources::Source`, `maxfetch::model::{Candidate, Release}`, `maxfetch::match::group_releases`, `maxfetch::quality::rank_candidates`, `maxfetch::probe::probe`, `maxfetch::tags::write_tags`, `maxfetch::library::{absolute_path, relative_path}`, `maxfetch::db::Db` (Tasks 1-10).
- Produces:
  - `maxfetch::pipeline::Pipeline` — `struct { config: Config, db: Db, sources: Vec<Box<dyn Source>> }` with `Pipeline::new(config, db, sources)`.
  - `Pipeline::search(&self, q: &Query) -> anyhow::Result<Vec<ReleaseGroup>>` — queries every enabled source in parallel, skips a failing source with a logged reason, groups the surviving releases.
  - `Pipeline::candidates(&self, group: &ReleaseGroup) -> anyhow::Result<Vec<Candidate>>` — asks each source for candidates of the group's primary release, ranks them.
  - `Pipeline::download(&self, candidate: &Candidate, sink: &dyn ProgressSink) -> anyhow::Result<DownloadResult>` — downloads to a temporary file, probes it, rejects it when the measured quality is worse than the claimed quality and no lossless alternative was expected, tags it, moves it to the library path and records it in the database.
  - `maxfetch::pipeline::DownloadResult` — `struct { path: PathBuf, quality: Quality, probe: ProbeResult, rejected: bool, reason: Option<String> }`.
  - `maxfetch::pipeline::Pipeline::source_status(&self) -> Vec<SourceStatus>`.
  - `maxfetch::pipeline::Pipeline::db(&self) -> &Db`.

- [ ] **Step 1: Write the failing test**

Create `tests/pipeline.rs` with a fake source implementing the trait:

```rust
use std::path::PathBuf;
use async_trait::async_trait;
use maxfetch::config::Config;
use maxfetch::db::Db;
use maxfetch::model::{Candidate, Container, Locator, Quality, Release, SourceId};
use maxfetch::pipeline::Pipeline;
use maxfetch::sources::{AuthState, Capabilities, Query, Source};

struct Fake {
    id: SourceId,
    releases: Vec<Release>,
    quality: Quality,
    bytes: Vec<u8>,
    fail_search: bool,
}

fn rel(source: SourceId) -> Release {
    Release {
        title: "Get Lucky".into(),
        artist: "Daft Punk".into(),
        album: Some("RAM".into()),
        track_no: Some(8),
        duration_sec: Some(369.0),
        isrc: Some("FR123400001".into()),
        year: Some(2013),
        source,
        source_item_id: "r1".into(),
    }
}

#[async_trait]
impl Source for Fake {
    fn id(&self) -> SourceId { self.id }
    fn capabilities(&self) -> Capabilities { Capabilities { audio: true, metadata_only: false, lossless: self.quality.lossless, needs_auth: false } }
    fn auth_state(&self) -> AuthState { AuthState::NotNeeded }
    async fn search(&self, _q: &Query) -> anyhow::Result<Vec<Release>> {
        if self.fail_search { anyhow::bail!("network down") } else { Ok(self.releases.clone()) }
    }
    async fn resolve(&self, _q: &Query) -> anyhow::Result<Vec<Release>> { Ok(self.releases.clone()) }
    async fn candidates(&self, release: &Release) -> anyhow::Result<Vec<Candidate>> {
        Ok(vec![Candidate {
            source: self.id,
            release: release.clone(),
            quality: self.quality.clone(),
            size_bytes: Some(self.bytes.len() as u64),
            locator: Locator::Soulseek { username: "peer".into(), filename: "f".into(), size: self.bytes.len() as u64 },
        }])
    }
    async fn download(&self, _c: &Candidate, dest: &PathBuf, _sink: &dyn maxfetch::fetch::ProgressSink) -> anyhow::Result<()> {
        std::fs::write(dest, &self.bytes)?;
        Ok(())
    }
}
```

Then the test bodies:

```rust
#[tokio::test]
async fn a_failing_source_does_not_break_the_run() {
    let cfg = Config::default();
    let db = Db::open_in_memory().unwrap();
    let bad = Fake { id: SourceId::Beatport, releases: vec![], quality: Quality::default(), bytes: vec![], fail_search: true };
    let good = Fake { id: SourceId::Soulseek, releases: vec![rel(SourceId::Soulseek)], quality: Quality { lossless: true, container: Container::Flac, bit_depth: Some(16), sample_rate: Some(44100), bitrate_kbps: None }, bytes: std::fs::read(fixture("sine_44100_s16.flac")).unwrap(), fail_search: false };
    let p = Pipeline::new(cfg, db, vec![Box::new(bad), Box::new(good)]);
    let groups = p.search(&Query { text: Some("daft punk get lucky".into()), url: None, items: vec![] }).await.unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].sources, vec![SourceId::Soulseek]);
}

#[tokio::test]
async fn download_probes_tags_and_records_the_track() {
    let cfg = Config::default();
    let db = Db::open_in_memory().unwrap();
    let fake = Fake { id: SourceId::Soulseek, releases: vec![rel(SourceId::Soulseek)], quality: Quality { lossless: true, container: Container::Flac, bit_depth: Some(16), sample_rate: Some(44100), bitrate_kbps: None }, bytes: std::fs::read(fixture("sine_44100_s16.flac")).unwrap(), fail_search: false };
    let p = Pipeline::new(cfg.clone(), db, vec![Box::new(fake)]);
    let groups = p.search(&Query { text: Some("x".into()), url: None, items: vec![] }).await.unwrap();
    let cands = p.candidates(&groups[0]).await.unwrap();
    let res = p.download(&cands[0], &maxfetch::fetch::NoopSink).await.unwrap();
    assert!(res.path.exists());
    assert!(!res.rejected);
    assert_eq!(res.quality.bit_depth, Some(16));
    let tracks = p.db().all_tracks().unwrap();
    assert_eq!(tracks.len(), 1);
    assert!(tracks[0].path.ends_with(".flac"));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test pipeline`
Expected: FAIL with "unresolved import `maxfetch::pipeline`".

- [ ] **Step 3: Implement `src/pipeline.rs`**

`search` runs `tokio::join!`-style concurrency over the enabled sources: spawn each source's `search`/`resolve` into a `tokio::task`, join them all, and collect the results. A source that returns an error is logged with `tracing::warn!` and contributes nothing; it never aborts the run.

`download` performs, in order: download into `<library>/.tmp/<uuid>.part`; probe the file; compare the measured quality with the claimed quality using `maxfetch::quality::compare_quality` — if the measured quality is worse AND the candidate claimed lossless, the result is `rejected: true` with a reason, the temporary file is kept for inspection and the next candidate must be tried by the caller; otherwise write the tags, compute `absolute_path(&config.library.path, &candidate.release, ext)` with the extension taken from the measured container, create the parent directories, move the file there, insert a `TrackRecord` with the **measured** quality, and return `DownloadResult`.

- [ ] **Step 4: Declare the module and run the test**

Add `pub mod pipeline;` to `src/lib.rs`.

Run: `cargo test --test pipeline`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(core): fetch pipeline with probe, tagging and recording"
```

---

### Task 12: Deduplication and the library commands

**Files:**
- Create: `src/dedup.rs`
- Test: `tests/dedup.rs`

**Interfaces:**
- Consumes: `maxfetch::db::{Db, TrackRecord}`, `maxfetch::model::{Candidate, Quality}`, `maxfetch::quality::compare_quality` (Tasks 1, 2, 4).
- Produces:
  - `maxfetch::dedup::DedupOutcome` — `enum { Inserted { id: i64 }, Duplicate { winner_id: i64, loser_id: i64 } }`.
  - `maxfetch::dedup::register(db: &Db, candidate: &Candidate, path: &Path, probe_quality: Quality, duration_sec: Option<f64>) -> anyhow::Result<DedupOutcome>` — finds an existing track by ISRC, then by `match_key`, then by artist+title with a duration tolerance of 2 seconds; when a loser is found it is marked `duplicate_of` and the file is left untouched.
  - `maxfetch::dedup::scan_library(db: &Db, root: &Path) -> anyhow::Result<usize>` — indexes existing files found under `root` that are not yet known to the database; it never renames, moves or deletes anything; returns the number of newly indexed files.
  - `maxfetch::dedup::plan_deletion(db: &Db) -> anyhow::Result<Vec<DeletionPlan>>` — returns the exact list of files that `--apply --delete` would remove, excluding entries whose group is flagged `conflict`.
  - `maxfetch::dedup::DeletionPlan` — `struct { id: i64, path: String, winner_path: String, reason: String }`.
  - `maxfetch::dedup::apply_deletion(db: &Db, plans: &[DeletionPlan]) -> anyhow::Result<usize>` — deletes the files listed in `plans` and their rows, and returns the number of removed files.

- [ ] **Step 1: Write the failing test**

Create `tests/dedup.rs`:
```rust
use maxfetch::db::Db;
use maxfetch::dedup::{apply_deletion, plan_deletion, register, scan_library};
use maxfetch::model::{Candidate, Container, Locator, Quality, Release, SourceId};

fn candidate(isrc: Option<&str>, title: &str, path: &str) -> Candidate {
    Candidate {
        source: SourceId::Soulseek,
        release: Release {
            title: title.into(),
            artist: "Daft Punk".into(),
            album: Some("RAM".into()),
            track_no: Some(8),
            duration_sec: Some(369.0),
            isrc: isrc.map(|s| s.into()),
            year: None,
            source: SourceId::Soulseek,
            source_item_id: "r".into(),
        },
        quality: Quality { lossless: true, container: Container::Flac, bit_depth: Some(16), sample_rate: Some(44100), bitrate_kbps: None },
        size_bytes: None,
        locator: Locator::Soulseek { username: "p".into(), filename: "f".into(), size: 0 },
    }
}

fn quality(depth: u8) -> Quality {
    Quality { lossless: true, container: Container::Flac, bit_depth: Some(depth), sample_rate: Some(44100), bitrate_kbps: None }
}

#[test]
fn same_isrc_marks_the_new_file_as_duplicate() {
    let db = Db::open_in_memory().unwrap();
    let first = register(&db, &candidate(Some("FR1"), "Get Lucky", "/m/a.flac"), std::path::Path::new("/m/a.flac"), quality(24), Some(369.0)).unwrap();
    let second = register(&db, &candidate(Some("FR1"), "Get Lucky", "/m/b.flac"), std::path::Path::new("/m/b.flac"), quality(16), Some(369.0)).unwrap();
    assert!(matches!(first, maxfetch::dedup::DedupOutcome::Inserted { .. }));
    assert!(matches!(second, maxfetch::dedup::DedupOutcome::Duplicate { .. }));
    let groups = db.duplicate_groups().unwrap();
    assert_eq!(groups.len(), 1);
}

#[test]
fn the_better_quality_wins() {
    let db = Db::open_in_memory().unwrap();
    register(&db, &candidate(Some("FR1"), "Get Lucky", "/m/low.flac"), std::path::Path::new("/m/low.flac"), quality(16), Some(369.0)).unwrap();
    register(&db, &candidate(Some("FR1"), "Get Lucky", "/m/high.flac"), std::path::Path::new("/m/high.flac"), quality(24), Some(369.0)).unwrap();
    let groups = db.duplicate_groups().unwrap();
    let winner = &groups[0].winner;
    assert!(winner.path.ends_with("high.flac"));
}

#[test]
fn dedup_never_deletes_files_by_itself() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.flac");
    let b = dir.path().join("b.flac");
    std::fs::write(&a, b"a").unwrap();
    std::fs::write(&b, b"b").unwrap();
    let db = Db::open_in_memory().unwrap();
    register(&db, &candidate(Some("FR1"), "Get Lucky", a.to_str().unwrap()), &a, quality(24), Some(369.0)).unwrap();
    register(&db, &candidate(Some("FR1"), "Get Lucky", b.to_str().unwrap()), &b, quality(16), Some(369.0)).unwrap();
    assert!(a.exists() && b.exists());
}

#[test]
fn deletion_requires_an_explicit_plan_and_leaves_conflicts_alone() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.flac");
    let b = dir.path().join("b.flac");
    std::fs::write(&a, b"a").unwrap();
    std::fs::write(&b, b"b").unwrap();
    let db = Db::open_in_memory().unwrap();
    register(&db, &candidate(Some("FR1"), "Get Lucky", a.to_str().unwrap()), &a, quality(16), Some(369.0)).unwrap();
    register(&db, &candidate(Some("FR1"), "Get Lucky", b.to_str().unwrap()), &b, quality(16), Some(369.0)).unwrap();

    let plans = plan_deletion(&db).unwrap();
    assert!(plans.is_empty(), "identical quality must be treated as a conflict");

    let c = dir.path().join("c.flac");
    std::fs::write(&c, b"c").unwrap();
    register(&db, &candidate(Some("FR2"), "Other", c.to_str().unwrap()), &c, quality(16), Some(369.0)).unwrap();
    register(&db, &candidate(Some("FR2"), "Other", dir.path().join("d.flac").to_str().unwrap()), &dir.path().join("d.flac"), quality(24), Some(369.0)).unwrap();
    let plans = plan_deletion(&db).unwrap();
    assert_eq!(plans.len(), 1);
    let removed = apply_deletion(&db, &plans).unwrap();
    assert_eq!(removed, 1);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test dedup`
Expected: FAIL with "unresolved import `maxfetch::dedup`".

- [ ] **Step 3: Implement `src/dedup.rs`**

`register` must run inside a single `BEGIN IMMEDIATE` transaction: look up by ISRC, then by `match_key`, then by artist+title within the duration tolerance; compare the found track's quality with the new one using `compare_quality`; the better one stays canonical and the other is marked `duplicate_of`. When the two qualities compare equal the group is a conflict and the loser is still marked, but `plan_deletion` must not list conflicts.

`scan_library` walks `root` recursively, reads the tags of every audio file with `maxfetch::tags::read_tags`, and inserts a row for the files that are unknown to the database. It never renames, moves or deletes.

`apply_deletion` deletes the file at `plan.path` and then the row with `plan.id`, and counts only the files that were actually removed.

- [ ] **Step 4: Declare the module and run the test**

Add `pub mod dedup;` to `src/lib.rs`.

Run: `cargo test --test dedup`
Expected: PASS, 4 tests.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(core): deduplication by isrc with explicit deletion"
```

---

### Task 13: Headless CLI

**Files:**
- Create: `src/cli.rs`
- Modify: `src/main.rs`
- Test: `tests/cli.rs`

**Interfaces:**
- Consumes: `maxfetch::cli::Cli`, `maxfetch::pipeline::Pipeline`, `maxfetch::config::{Config, Credentials}`, `maxfetch::db::Db`, `maxfetch::dedup` (Tasks 3, 4, 11, 12).
- Produces:
  - `maxfetch::cli::Cli` — a `clap::Parser` struct with the subcommands `search`, `fetch`, `library`, `auth`, `sources`, `config`.
  - `maxfetch::cli::run(cli: Cli) -> anyhow::Result<()>` — the whole CLI behaviour, so it can be called from `main` and from tests.
  - `maxfetch::cli::run_with(cli: Cli, config: Config, credentials: Credentials, out: &mut dyn std::io::Write) -> anyhow::Result<()>` — the testable entry point.

- [ ] **Step 1: Write the failing test**

Create `tests/cli.rs`:
```rust
use clap::Parser;
use maxfetch::cli::{Cli, run_with};

#[tokio::test]
async fn dry_run_downloads_nothing_and_reports_the_plan() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = maxfetch::config::Config::load_from(dir.path()).unwrap();
    cfg.library.path = dir.path().join("library");
    let creds = maxfetch::config::Credentials::default();
    let cli = Cli::parse_from(["maxfetch", "fetch", "--dry-run", "--library", cfg.library.path.to_str().unwrap(), "daft punk get lucky"]);
    let mut out = Vec::new();
    run_with(cli, cfg, creds, &mut out).await.unwrap();
    assert!(!cfg.library.path.exists() || std::fs::read_dir(&cfg.library.path).unwrap().count() == 0);
}

#[tokio::test]
async fn library_dedupe_without_flags_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = maxfetch::config::Config::load_from(dir.path()).unwrap();
    let creds = maxfetch::config::Credentials::default();
    let cli = Cli::parse_from(["maxfetch", "library", "dedupe"]);
    let mut out = Vec::new();
    run_with(cli, cfg, creds, &mut out).await.unwrap();
    assert!(String::from_utf8_lossy(&out).contains("нечего удалять") || String::from_utf8_lossy(&out).is_empty());
}

#[tokio::test]
async fn library_list_json_is_valid() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = maxfetch::config::Config::load_from(dir.path()).unwrap();
    let creds = maxfetch::config::Credentials::default();
    let cli = Cli::parse_from(["maxfetch", "library", "list", "--json"]);
    let mut out = Vec::new();
    run_with(cli, cfg, creds, &mut out).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert!(v.is_array());
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test cli`
Expected: FAIL with "unresolved import `maxfetch::cli`".

- [ ] **Step 3: Implement `src/cli.rs`**

Use `clap` derive. Structure:

```rust
#[derive(Parser)]
#[command(name = "maxfetch", version, about = "Скачивание треков в максимально возможном качестве")]
pub struct Cli {
    #[arg(long, global = true)] pub library: Option<PathBuf>,
    #[arg(long, global = true, value_delimiter = ',')] pub source: Vec<String>,
    #[arg(long, global = true)] pub dry_run: bool,
    #[arg(long, global = true)] pub json: bool,
    #[arg(long, global = true)] pub ndjson_progress: bool,
    #[arg(long, global = true)] pub no_tui: bool,
    #[command(subcommand)] pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    Search { query: String },
    Fetch { query: Option<String>, #[arg(long)] playlist: Option<String>, #[arg(long)] spotify_track: Option<String>, #[arg(long)] spotify_playlist: Option<String>, #[arg(long)] page: Option<String>, #[arg(long)] soundcloud: Option<String>, #[arg(long)] input: Option<String> },
    Library { #[command(subcommand)] command: LibraryCommand },
    Auth { #[command(subcommand)] command: AuthCommand },
    Sources { #[command(subcommand)] command: SourcesCommand },
    Config { #[command(subcommand)] command: ConfigCommand },
}
```

`LibraryCommand` has `Scan`, `Dedupe { #[arg(long)] apply: bool, #[arg(long)] delete: bool, #[arg(long)] yes: bool }` and `List`. `AuthCommand` has `Login { source: String }` and `Status`. `SourcesCommand` has `Check`. `ConfigCommand` has `Set { key: String, value: String }`.

`run_with` resolves the config (`--library` overrides `config.library.path` and is written back), opens the database at `state_path()`, builds the pipeline from the registry, and dispatches. `library dedupe` prints the plan and deletes nothing unless `--apply --delete` are both present; `--yes` is additionally required in the non-interactive path, otherwise the command prompts. All user-facing strings are in Russian.

- [ ] **Step 4: Wire up `src/main.rs` and run the test**

`src/main.rs` becomes:
```rust
use clap::Parser;
use maxfetch::cli::{Cli, run};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    run(Cli::parse()).await
}
```

Run: `cargo test --test cli`
Expected: PASS, 3 tests.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(core): headless cli"
```

---

### Task 14: TUI shell and screens

**Files:**
- Create: `src/tui/mod.rs`
- Create: `src/tui/views/mod.rs`, `src/tui/views/search.rs`, `results.rs`, `candidates.rs`, `downloads.rs`, `sources.rs`, `settings.rs`, `log.rs`
- Test: `tests/tui.rs`

**Interfaces:**
- Consumes: `maxfetch::pipeline::Pipeline`, `maxfetch::config::Config`, `maxfetch::model::SourceId` (Tasks 3, 11).
- Produces:
  - `maxfetch::tui::Screen` — `enum { Search, Results, Candidates, Downloads, Sources, Settings, Log }`.
  - `maxfetch::tui::App` — `struct { screen: Screen, query: String, groups: Vec<ReleaseGroup>, selected: usize, candidates: Vec<Candidate>, downloads: Vec<DownloadItem>, log: Vec<LogEntry>, config: Config, status: Vec<SourceStatus>, should_quit: bool }`.
  - `maxfetch::tui::App::new(config: Config) -> App`, `App::on_key(&mut self, key: KeyEvent) -> anyhow::Result<()>`, `App::set_library_path(&mut self, path: &str) -> anyhow::Result<()>`.
  - `maxfetch::tui::render(frame: &mut ratatui::Frame, app: &App)` — draws the current screen.
  - `maxfetch::tui::run(config: Config) -> anyhow::Result<()>` — the event loop.
  - `maxfetch::tui::DownloadItem` — `struct { title: String, source: SourceId, state: String, transferred: u64, total: Option<u64>, speed: Option<u64>, detail: Option<String> }`.
  - `maxfetch::tui::LogEntry` — `struct { level: String, message: String }`.

- [ ] **Step 1: Write the failing test**

Create `tests/tui.rs`:
```rust
use std::path::PathBuf;
use crossterm::event::KeyCode;
use maxfetch::config::Config;
use maxfetch::tui::{App, Screen};

#[test]
fn tab_switches_between_screens() {
    let mut app = App::new(Config::default());
    assert_eq!(app.screen, Screen::Search);
    app.on_key(crossterm::event::KeyEvent::new(KeyCode::Tab, crossterm::event::KeyModifiers::NONE)).unwrap();
    assert_eq!(app.screen, Screen::Results);
}

#[test]
fn q_quits() {
    let mut app = App::new(Config::default());
    app.on_key(crossterm::event::KeyEvent::new(KeyCode::Char('q'), crossterm::event::KeyModifiers::NONE)).unwrap();
    assert!(app.should_quit);
}

#[test]
fn settings_screen_can_change_the_library_path() {
    let mut app = App::new(Config::default());
    app.screen = Screen::Settings;
    app.on_key(crossterm::event::KeyEvent::new(KeyCode::Char('e'), crossterm::event::KeyModifiers::NONE)).unwrap();
    app.set_library_path("/tmp/newlib");
    assert_eq!(app.config.library.path, PathBuf::from("/tmp/newlib"));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test tui`
Expected: FAIL with "unresolved import `maxfetch::tui`".

- [ ] **Step 3: Implement the TUI**

Use `ratatui` 0.30 and `crossterm` 0.29 with the `event-stream` feature. `run` creates the terminal with `ratatui::init() -> DefaultTerminal` (which is `Terminal<CrosstermBackend<Stdout>>` and enters the alternate screen), spawns a task reading `crossterm::event::EventStream::new()` (the `Stream<Item = io::Result<Event>>` from `crossterm::event`, requiring `event-stream`), renders on a tick, and restores the terminal with `ratatui::restore()` on exit.

`render` receives `&mut ratatui::Frame` and must use `frame.area() -> Rect` (not `frame.size()`, which still exists but is the older accessor) to size the layout. Build layouts with `Layout::new(Direction::Vertical, constraints)` / `Layout::new(Direction::Horizontal, constraints)`; render lists with `List::new(items)` over `ListItem::new(text)`, tables with `Table::new(rows, widths)`, and text with `Line::raw`, `Line::styled`, `Line::spans`, `Span::raw` and `Span::styled`.

`crossterm` is also reachable through `ratatui::crossterm`, so the views may import from either path; the test in Step 1 imports `crossterm::event` directly, which works because it is a direct dependency.

Key bindings, exactly as in spec section 7.11: `Tab`/`Shift+Tab` change the screen, `/` focuses the search field, `Enter` expands the selected release into candidates, `Space` marks, `d` downloads the marked items, `a` downloads the best candidate of every marked release, `s` opens the settings, `?` shows the help overlay, `q` quits.

The Candidates screen shows, per candidate: the source badge, the measured-or-claimed quality (`24/48 FLAC`), the size, the `LOSSY` badge and the `conflict` badge. The Downloads screen shows a progress bar, the transfer speed, the ETA and the state of every item, and for Soulseek the peer name, its speed and its free slots. The Sources screen shows one line per source with its status (доступен / нет авторизации / ключ протух / недоступен по сети) and a «проверить» action bound to `c`. The Settings screen edits the library path, the per-source toggles, the limits and the `yt-dlp`/`ffmpeg` paths, and saves them to `config.toml` when leaving the screen.

- [ ] **Step 4: Declare the module and run the test**

Add `pub mod tui;` to `src/lib.rs`.

Run: `cargo test --test tui`
Expected: PASS, 3 tests.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(core): tui shell and screens"
```

---

### Task 15: Source status, authentication stubs and the README

**Files:**
- Create: `src/auth.rs`
- Create: `README.md`
- Test: `tests/auth.rs`

**Interfaces:**
- Consumes: `maxfetch::config::Credentials`, `maxfetch::sources::{registry, Source, SourceStatus}` (Tasks 3, 10).
- Produces:
  - `maxfetch::auth::auth_status(credentials: &Credentials) -> Vec<SourceStatus>` — reports, per source, whether credentials are present.
  - `maxfetch::auth::login(source: &str, credentials: &mut Credentials) -> anyhow::Result<()>` (async) — performs the interactive login of the given source and stores the result in the credentials file; in this task it returns a clear "not implemented in the core build" error for every source, and the stage plans replace that branch with the real flow.
  - `maxfetch::auth::save_credentials(credentials: &Credentials) -> anyhow::Result<()>` — writes the credentials file with mode 0600.

- [ ] **Step 1: Write the failing test**

Create `tests/auth.rs`:
```rust
use maxfetch::auth::{auth_status, login};
use maxfetch::config::Credentials;

#[test]
fn status_reports_missing_credentials() {
    let creds = Credentials::default();
    let statuses = auth_status(&creds);
    let soulseek = statuses.iter().find(|s| s.id == maxfetch::model::SourceId::Soulseek).unwrap();
    assert_eq!(soulseek.auth, maxfetch::sources::AuthState::Missing);
}

#[test]
fn status_reports_present_credentials() {
    let mut creds = Credentials::default();
    creds.soulseek = Some(Default::default());
    let statuses = auth_status(&creds);
    let soulseek = statuses.iter().find(|s| s.id == maxfetch::model::SourceId::Soulseek).unwrap();
    assert_eq!(soulseek.auth, maxfetch::sources::AuthState::Ok);
}

#[tokio::test]
async fn login_is_not_implemented_in_the_core_build() {
    let mut creds = Credentials::default();
    let err = login("spotify", &mut creds).await.unwrap_err();
    assert!(format!("{err}").contains("not implemented"));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test auth`
Expected: FAIL with "unresolved import `maxfetch::auth`".

- [ ] **Step 3: Implement `src/auth.rs`**

`auth_status` builds the list of all nine `SourceId`s and inspects the corresponding optional credential section: `Some(_)` → `AuthState::Ok`, `None` → `AuthState::Missing`. `login` currently returns `anyhow::bail!("login for {source} is not implemented in the core build")`.

- [ ] **Step 4: Write `README.md`**

Cover: what the tool does, the installation (`cargo install --path .`), the configuration (`config.toml` and `credentials.toml` with the 0600 requirement), the library layout, the CLI commands, the TUI keys, the source list with the current stage of each one, and the legal note:

> Скачивание с Qobuz, Tidal и Deezer нарушает их условия использования. Bandcamp и Beatport легальны для уже купленного контента. Soulseek находится в серой зоне. Ответственность за использование несёт пользователь.

- [ ] **Step 5: Declare the module and run the test**

Add `pub mod auth;` to `src/lib.rs`.

Run: `cargo test --test auth`
Expected: PASS, 3 tests.

- [ ] **Step 6: Run the whole suite**

Run: `cargo test`
Expected: all tests pass.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(core): source status, auth stubs and readme"
```
