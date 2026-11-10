# maxfetch Stage 1 (Soulseek, YouTube, Spotify) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the three stage-1 sources to the `maxfetch` core: a Soulseek client that selects peers by quality, free slots and speed and falls over to the next candidate on a stall; a YouTube source that drives `yt-dlp` and remuxes WebM/Opus to `.opus` without re-encoding; a Spotify catalog source that supplies metadata and ISRC and refuses playlists with a readable message. Then register all three in the source registry and expose them in the CLI and the TUI.

**Architecture:** `src/sources/soulseek.rs` holds the peer model, the attribute-to-quality mapping and the ranked-download loop as pure functions, with the blocking `soulseek-rs-lib` calls isolated behind a `Transfer` trait and `tokio::task::spawn_blocking`. `src/youtube.rs` parses `yt-dlp -J` JSON into `Release` values and runs the subprocess; the audio path is download → `maxfetch::media::remux_to_opus` (stream copy) → tags, degrading to `.webm` plus a sidecar JSON when `ffmpeg` is unavailable. `src/sources/spotify.rs` maps Spotify JSON to `Release` and implements `Source` with `metadata_only: true`; its HTTP surface sits behind a `SpotifyApi` trait so the token exchange and the mapping are testable offline. `src/sources/mod.rs::registry` stops being empty.

**Tech Stack:** Rust 2024 edition, the core crate's existing dependencies, plus `soulseek-rs-lib` 20.0.0 (MIT, blocking API) and `rspotify` 0.16.1. External binaries: `yt-dlp` (configurable, `/run/current-system/sw/bin/yt-dlp` on this machine) and `ffmpeg`. Tests use fake binaries, canned JSON fixtures and channel-driven fakes; no test touches the network.

**Spec:** `docs/superpowers/specs/2026-11-10-maxfetch-design.md` (sections 7.3, 8.1, 8.3, 8.4 and 9 are the contract; section 7.4 fixes the ranking rule this stage must not violate).

## Global Constraints

- Crate name and binary name: `maxfetch`. License MIT. Rust edition 2024.
- Rust toolchain 1.99.0 is installed at `/run/current-system/sw/bin/cargo`; `cargo` must be on PATH.
- Do not add dependencies that perform audio transcoding. `ffmpeg` may only be invoked with `-c copy` (stream copy, no re-encode).
- Lossy files are stored exactly as delivered by the source. No lossy-to-lossless or lossy-to-lossy re-encoding anywhere in the codebase.
- Quality ordering: lossless beats lossy; inside lossless, `bit_depth` descending first, then `sample_rate` descending; inside lossy, `bitrate_kbps` descending first, then `sample_rate` descending. Missing values are never invented and always sort below known values of the same class.
- The unit of work is a track. An album is a grouping with a tracklist; partial success is normal, not an error.
- Library layout is `<library>/<Artist>/<Album>/<NN> <Title>.<ext>`; `<Album>` defaults to `Singles`; `NN` is a two-digit track number.
- `maxfetch` never deletes files implicitly. Deduplication only marks `duplicate_of` in the database; deletion requires the explicit command `maxfetch library dedupe --apply --delete --yes`.
- Soulseek is a download-only client: no shared directories, no incoming port, no UPnP/NAT-PMP. There is no "what to share" section in the settings, and `Client` is never configured to serve files or to listen.
- Soulseek peer selection: candidates with `slots == 0` are dropped before ranking; the order is file quality first, then free slots, then peer speed, then file size. A transfer that produces no bytes for `config.download.stall_timeout_sec` (default 30) or a peer that disconnects falls over to the next candidate in the ranked list.
- Spotify in stage 1 is `client_credentials` only. Spotify never yields audio; its candidates list is always empty and its `download` always fails with a Russian message.
- Secrets live in `$XDG_CONFIG_HOME/maxfetch/credentials.toml` with mode 0600; the file is created with 0600 and a wider mode produces a warning at startup.
- All source sections in `credentials.toml` are optional. A missing section means "source not authorized": the source reports `AuthState::Missing` and is skipped, and every other source keeps working.
- No source failure (network down, missing credentials, stalled peer, missing `yt-dlp`) may abort a run; it is recorded in the log and the source or the peer is skipped.
- `maxfetch auth login` for soulseek, youtube and spotify remains the core build's stub in this stage; this plan only adds credential-presence reporting.
- Rust code, identifiers, comments and commit messages are in English. User-facing strings (TUI labels, CLI output, source status, error messages shown to the user) are in Russian.
- Every commit is authored as `m1kiru <m1kiru@github.com>`.
- No test may reach the network. Soulseek is exercised through pure functions and a `Transfer` fake, YouTube through `tests/fixtures/bin/yt-dlp` and canned JSON, Spotify through the `SpotifyApi` seam and JSON fixtures.

## Review Focus

The spec is a vision document; these inputs are implied by it but not covered by any explicit requirement. Each is now pinned to a covering test in the task that owns the relevant code, and each records the decision the plan takes.

1. **A peer that stalls after a few megabytes.** The failover test only feeds a transfer that fails immediately; a real stall is a silent std mpsc `Receiver<DownloadStatus>`, so the `stall_timeout_sec` path depends on `recv_timeout` firing between `InProgress` samples, not on the ranked loop. Pinned to Task 2 (`drain_transfer`).
2. **A FLAC whose `attribs` map carries none of keys 0, 4 and 5.** Tested by `a_flac_without_bitrate_rate_and_depth_ranks_below_a_known_flac`: such a release lands in the "unknown quality" class and must rank below a FLAC with a known depth or rate, never above it. Decision: never invent a value; the entry keeps every numeric field `None` and `compare_quality` puts it last. Pinned to Task 1 (`quality_from_attributes`).
3. **A YouTube title that is not a track title** — `"Artist - Topic"`, a 3-hour DJ set, a video with no `track` field and an `uploader` that is a radio station. Decisions: (a) a `- Topic` channel is a loader, so the suffix is stripped and `Topic` never survives as an artist name; (b) anything longer than `MAX_TRACK_SECONDS` (900 s) is a DJ set or a radio broadcast and is rejected by `parse_video` with an error naming the duration; (c) a radio-station uploader with no `track` field is used verbatim as the artist, with the video title as the title and no album, because guessing a split out of a station name would invent data. Three tests pin all three cases. Pinned to Task 3 (`parse_video`).
4. **The Spotify token endpoint answering 200 with `{"error":"invalid_client"}` in the body.** The mapping test only covers a body that lacks `access_token`; a credential-shaped error body must surface as "ключи Spotify отклонены", never as a raw response dump. The covering test asserts exactly that string (equivalently `AuthState::Expired` on the source). Pinned to Task 5 (`parse_token_response`).
5. **A WebM whose audio stream is not Opus** (an AAC `m4a` fetched under a `.webm` name, or a remux that exits non-zero on an unusual codec). Two tests cover it: one feeds an AAC stream under a `.webm` name and asserts the probe-derived container is the real codec while the file keeps `.webm`, gets no tags, gets a sidecar and the `UNTAGGED` badge; one keeps `ffmpeg` present but makes the remux fail and asserts the same fallback with no panic. Pinned to Task 4 (`finalize`).
6. **Resuming an interrupted transfer from an offset.** The Soulseek protocol as exposed by `soulseek_rs` 20.0.0 has no offset resume. Decision: an unfinished download is deleted and restarted from zero; a transfer never leaves a `.part` file behind and never appends to one. Pinned to Task 2 (`Transfer::fetch`, `download_ranked`).
7. **Peer availability in the Downloads screen.** A peer with no free slots is dropped before ranking, but a transfer already running against a peer that fills up must still be visible as unusable. Decision: the Downloads screen shows the peer name, its free slots and the transfer speed for every active transfer, and marks the peer as unavailable when `slots == 0`. Pinned to Task 8.

---

### Task 1: Soulseek peer model, attribute parsing and ranking

**Files:**
- Modify: `Cargo.toml` (add `soulseek-rs-lib` 20.0.0)
- Create: `src/sources/soulseek.rs`
- Modify: `src/sources/mod.rs` (declare the module)
- Test: `tests/soulseek_selection.rs`

**Interfaces:**
- Consumes: `maxfetch::model::{Candidate, Container, Locator, Quality, Release}`, `maxfetch::quality::compare_quality` (core Tasks 1). Core Task 1 already gives `Quality`, `Container`, `Release`, `Locator` and `Candidate` the derives `Clone + Debug + PartialEq`; that is what makes the `assert_eq!` comparisons in the tests below legal, and this task adds no derives of its own.
- Produces:
  - `maxfetch::sources::soulseek::PeerCandidate` — `struct { username: String, filename: String, size: u64, slots: u8, speed: u32, quality: Quality }`, `Clone + Debug + PartialEq`. `slots` and `speed` keep the crate's own widths (`SearchResult::{slots, speed}`); `filename` is copied from `soulseek_rs::File::name`.
  - `maxfetch::sources::soulseek::ATTR_BITRATE/ATTR_DURATION/ATTR_SAMPLE_RATE/ATTR_BIT_DEPTH` — `const ...: u32 = 0 / 1 / 4 / 5`, matching the codes the spec fixes in section 8.1 and the crate writes into `File::attribs`. There is no VBR code in the contract.
  - `maxfetch::sources::soulseek::attrib(attribs: &std::collections::HashMap<u32, u32>, code: u32) -> Option<u32>` — the raw lookup; `None` when the code is absent.
  - `maxfetch::sources::soulseek::lossless_extension(ext: &str) -> bool` — lowercase extension without the leading dot; true for `flac`, `wav`, `alac`, `aiff`, `ape`, `wv`.
  - `maxfetch::sources::soulseek::quality_from_attributes(name: &str, attribs: &std::collections::HashMap<u32, u32>) -> Option<Quality>` — the parameter is exactly `&HashMap<u32, u32>` taken from `File::attribs`; `None` when the extension has no `Container` mapping; `bitrate_kbps` is `None` for every lossless container and equals `attribs[ATTR_BITRATE]` (when it is above zero) for lossy ones.
  - `maxfetch::sources::soulseek::peer_from_file(username: &str, slots: u8, speed: u32, filename: &str, size: u64, attribs: &std::collections::HashMap<u32, u32>) -> Option<PeerCandidate>` — `None` when `quality_from_attributes` returns `None`, so a peer/file pair the library cannot lay out is dropped here and never becomes a candidate.
  - `maxfetch::sources::soulseek::rank_peers(candidates: &[PeerCandidate]) -> Vec<PeerCandidate>` — drops `slots == 0`, sorts best first by quality, then slots, then speed, then size.
  - `maxfetch::sources::soulseek::into_candidate(peer: &PeerCandidate, release: &Release) -> Candidate` — builds a `Candidate` with `Locator::Soulseek { username, filename, size }`.
  - `maxfetch::sources::soulseek::describe_peer(peer: &PeerCandidate) -> String` — `"пир <username>: скорость <speed>, слотов <slots>"`.

- [ ] **Step 1: Write the failing test**

Create `tests/soulseek_selection.rs`:
```rust
use std::collections::HashMap;
use maxfetch::model::{Container, Quality, Release, SourceId};
use maxfetch::sources::soulseek::{
    attrib, describe_peer, into_candidate, lossless_extension, peer_from_file, quality_from_attributes,
    rank_peers, PeerCandidate, ATTR_BITRATE, ATTR_BIT_DEPTH, ATTR_DURATION, ATTR_SAMPLE_RATE,
};

fn peer(user: &str, slots: u8, speed: u32, quality: Quality, size: u64) -> PeerCandidate {
    PeerCandidate { username: user.into(), filename: "f.flac".into(), size, slots, speed, quality }
}

fn flac(depth: u8, rate: u32) -> Quality {
    Quality { lossless: true, container: Container::Flac, bit_depth: Some(depth), sample_rate: Some(rate), bitrate_kbps: None }
}

#[test]
fn zero_slot_peer_is_dropped() {
    let busy = peer("busy", 0, 4, flac(24, 96000), 900);
    let free = peer("free", 1, 1, flac(16, 44100), 100);
    let ranked = rank_peers(&[busy, free]);
    assert_eq!(ranked.len(), 1);
    assert_eq!(ranked[0].username, "free");
}

#[test]
fn flac_with_zero_bitrate_ranks_by_depth_then_rate() {
    let deep = quality_from_attributes("a.flac", &HashMap::from([(ATTR_BITRATE, 0), (ATTR_SAMPLE_RATE, 96000), (ATTR_BIT_DEPTH, 24)])).unwrap();
    let shallow = quality_from_attributes("b.flac", &HashMap::from([(ATTR_BITRATE, 0), (ATTR_SAMPLE_RATE, 192000), (ATTR_BIT_DEPTH, 16)])).unwrap();
    assert_eq!(deep.bit_depth, Some(24));
    assert_eq!(deep.sample_rate, Some(96000));
    assert_eq!(deep.bitrate_kbps, None);
    let ranked = rank_peers(&[
        peer("shallow", 2, 4, shallow, 200),
        peer("deep", 1, 1, deep, 100),
    ]);
    assert_eq!(ranked[0].username, "deep");
}

#[test]
fn flac_attributes_of_a_twenty_four_forty_eight_come_out_intact() {
    let q = quality_from_attributes("song.FLAC", &HashMap::from([(4u32, 48000u32), (5u32, 24u32)])).unwrap();
    assert!(q.lossless);
    assert_eq!(q.bit_depth, Some(24));
    assert_eq!(q.sample_rate, Some(48000));
    assert_eq!(q.bitrate_kbps, None);
    assert_eq!(attrib(&HashMap::from([(5u32, 24u32)]), ATTR_BIT_DEPTH), Some(24));
    assert_eq!(attrib(&HashMap::from([(5u32, 24u32)]), ATTR_SAMPLE_RATE), None);
}

#[test]
fn lossless_container_never_reports_a_bitrate() {
    let q = quality_from_attributes("a.flac", &HashMap::from([(ATTR_BITRATE, 1411), (ATTR_SAMPLE_RATE, 44100), (ATTR_BIT_DEPTH, 16)])).unwrap();
    assert_eq!(q.bitrate_kbps, None);
}

#[test]
fn lossy_attributes_yield_an_mp3_with_bitrate() {
    let q = quality_from_attributes("a.MP3", &HashMap::from([(ATTR_BITRATE, 320), (ATTR_SAMPLE_RATE, 44100)])).unwrap();
    assert!(!q.lossless);
    assert_eq!(q.container, Container::Mp3);
    assert_eq!(q.bitrate_kbps, Some(320));
}

#[test]
fn quality_outranks_free_slots() {
    let ranked = rank_peers(&[
        peer("many-slots", 9, 4, flac(16, 44100), 300),
        peer("one-slot", 1, 1, flac(24, 96000), 200),
    ]);
    assert_eq!(ranked[0].username, "one-slot");
}

#[test]
fn free_slots_outrank_peer_speed() {
    let ranked = rank_peers(&[
        peer("slow", 2, 1, flac(16, 44100), 100),
        peer("fast", 1, 4, flac(16, 44100), 100),
    ]);
    assert_eq!(ranked[0].username, "slow");
}

#[test]
fn peer_speed_breaks_ties() {
    let ranked = rank_peers(&[
        peer("slow", 1, 1, flac(16, 44100), 100),
        peer("fast", 1, 4, flac(16, 44100), 100),
    ]);
    assert_eq!(ranked[0].username, "fast");
}

#[test]
fn file_size_breaks_remaining_ties() {
    let ranked = rank_peers(&[
        peer("small", 1, 4, flac(16, 44100), 100),
        peer("large", 1, 4, flac(16, 44100), 900),
    ]);
    assert_eq!(ranked[0].username, "large");
}

#[test]
fn lossy_peer_never_beats_lossless() {
    let mp3 = Quality { lossless: false, container: Container::Mp3, bit_depth: None, sample_rate: Some(44100), bitrate_kbps: Some(320) };
    let ranked = rank_peers(&[peer("mp3", 9, 4, mp3, 300), peer("flac", 1, 1, flac(16, 44100), 100)]);
    assert_eq!(ranked[0].username, "flac");
}

#[test]
fn ape_and_wv_are_lossless_but_have_no_container() {
    assert!(lossless_extension("ape"));
    assert!(lossless_extension("wv"));
    assert!(lossless_extension("FLAC"));
    assert!(!lossless_extension("opus"));
    assert!(quality_from_attributes("x.ape", &HashMap::from([(ATTR_SAMPLE_RATE, 44100)])).is_none());
}

#[test]
fn a_file_the_library_cannot_lay_out_becomes_no_peer() {
    let attribs = HashMap::from([(ATTR_SAMPLE_RATE, 44100)]);
    assert!(peer_from_file("alice", 1, 4, "x.ape", 10, &attribs).is_none());
    let flac_attribs = HashMap::from([(ATTR_SAMPLE_RATE, 96000), (ATTR_BIT_DEPTH, 24)]);
    let p = peer_from_file("alice", 0, 4, "a.flac", 10, &flac_attribs).unwrap();
    assert_eq!(p.size, 10);
    assert_eq!(p.slots, 0);
    assert_eq!(p.quality.bit_depth, Some(24));
}

#[test]
fn a_flac_without_bitrate_rate_and_depth_ranks_below_a_known_flac() {
    let unknown = quality_from_attributes("unknown.flac", &HashMap::from([(ATTR_DURATION, 240)])).unwrap();
    assert_eq!(unknown.bit_depth, None);
    assert_eq!(unknown.sample_rate, None);
    assert_eq!(unknown.bitrate_kbps, None);
    let ranked = rank_peers(&[
        peer("unknown", 9, 4, unknown, 900),
        peer("known", 1, 1, flac(16, 44100), 100),
    ]);
    assert_eq!(ranked[0].username, "known");
}

#[test]
fn describe_peer_names_the_slots_and_the_speed() {
    assert_eq!(describe_peer(&peer("alice", 2, 4, flac(16, 44100), 1)), "пир alice: скорость 4, слотов 2");
}

#[test]
fn into_candidate_carries_the_soulseek_locator() {
    let p = peer("alice", 2, 4, flac(16, 44100), 1234);
    let release = Release {
        title: "Get Lucky".into(),
        artist: "Daft Punk".into(),
        album: Some("RAM".into()),
        track_no: Some(8),
        duration_sec: Some(369.0),
        isrc: None,
        year: None,
        source: SourceId::Soulseek,
        source_item_id: "peer:alice:f.flac".into(),
    };
    let c = into_candidate(&p, &release);
    assert_eq!(c.size_bytes, Some(1234));
    assert!(matches!(c.locator, maxfetch::model::Locator::Soulseek { .. }));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test soulseek_selection`
Expected: FAIL with "unresolved import `maxfetch::sources::soulseek`".

- [ ] **Step 3: Implement `src/sources/soulseek.rs`**

Add the dependency first: `cargo add soulseek-rs-lib@20.0.0`. The crate is imported as `use soulseek_rs::{Client, DownloadStatus, File, SearchResult};`. There is no typed attribute enum and no public response wrapper in this version: attribute data arrives only as `File::attribs: HashMap<u32, u32>`, keyed by the numeric codes the protocol uses, and `File` exposes `name: String`, `size: u64` and `attribs`.

`quality_from_attributes` takes the extension of `name` lowercased and without the leading dot, maps it through `Container::from_ext`, and returns `None` when that mapping is missing even if `lossless_extension` is true (the `Container` enum has no `Ape`/`Wv` variant, so those files cannot be laid out in the library and are skipped at selection time). Bit depth and sample rate come from `attrib(attribs, ATTR_BIT_DEPTH)` and `attrib(attribs, ATTR_SAMPLE_RATE)`; the bitrate comes from `attrib(attribs, ATTR_BITRATE)` and is kept only when the container is lossy and the value is above zero. `ATTR_DURATION` is declared for completeness and is ignored by this function; the attribute codes follow spec section 8.1 exactly (`0` bitrate, `1` length, `4` sample rate, `5` bit depth) and there is no VBR code. A FLAC whose map carries none of `0`, `4` and `5` therefore yields a `Quality` with every numeric field `None`; `compare_quality` keeps such an unknown-quality entry below every FLAC with a known depth or rate.

`peer_from_file` builds `PeerCandidate { username, filename, size, slots, speed, quality }` from a `SearchResult`'s `username`, `slots`, `speed` and one of its `files`, returning `None` when the derived quality is `None`.

`rank_peers` filters `slots > 0`, then sorts with `compare_quality` on the quality first (`Greater` means better, so compare the second against the first), then compares `slots`, `speed` and `size` descending.

- [ ] **Step 4: Declare the module and run the test**

Add `pub mod soulseek;` to `src/sources/mod.rs`.

Run: `cargo test --test soulseek_selection`
Expected: PASS, 15 tests.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit --author="m1kiru <m1kiru@github.com>" -m "feat(soulseek): peer model, attribute parsing and ranking"
```

---

### Task 2: Soulseek search, stalled-transfer detection and peer failover

**Files:**
- Modify: `src/sources/soulseek.rs`
- Test: `tests/soulseek_client.rs`

**Interfaces:**
- Consumes: `PeerCandidate`, `rank_peers`, `quality_from_attributes`, `peer_from_file`, `into_candidate` (Task 1); `maxfetch::fetch::ProgressSink`; `maxfetch::config::{DownloadConfig, SoulseekCredentials}` (core Tasks 3, 8); `maxfetch::sources::{AuthState, Capabilities, Source}` (core Task 10).
- **Verified crate API (source of truth for this task):** `soulseek_rs` 20.0.0 exposes `Client::new(username, password) -> Client`, `Client::connect(&mut self) -> Result<()>` and `Client::login(&self) -> Result<bool>`. This plan follows that crate API; spec section 8.1 still describes the older `Client::new` → `connect` → `login(user, pass)` shape and will be aligned to the crate by a separate edit that does not touch this file.
- Produces:
  - `maxfetch::sources::soulseek::drain_transfer(events: std::sync::mpsc::Receiver<soulseek_rs::DownloadStatus>, stall_timeout: std::time::Duration, sink: &dyn ProgressSink) -> anyhow::Result<u64>` — a blocking `recv()`/`recv_timeout()` loop over the std mpsc receiver the crate hands out. `DownloadStatus::InProgress { bytes_downloaded, total_bytes, speed_bytes_per_sec }` is the only progress sample: it is forwarded to `sink` (so the TUI gets speed and ETA through the sink and never polls the crate) and remembered; `is_terminal()` ends the loop — `Completed` calls `sink.on_finished` and returns the last `total_bytes`, `Failed(Some(msg))` returns that message, `Failed(None)` returns `"transfer failed"`, `Cancelled` returns `"transfer cancelled"`, `TimedOut` returns `"transfer timed out"`. `Err(RecvTimeoutError::Timeout)` returns `"transfer stalled after <n>s"` and `Err(RecvTimeoutError::Disconnected)` without a terminal status returns `"peer disconnected"`. `Queued` and `Paused { .. }` keep waiting.
  - `maxfetch::sources::soulseek::Transfer` — `trait { fn fetch(&self, peer: &PeerCandidate, dest: &Path, sink: &dyn ProgressSink) -> anyhow::Result<u64> }`, `Send + Sync`. The implementation writes the complete file at `dest` and returns the byte count; it owns the blocking download and the `drain_transfer` call. The Soulseek protocol as exposed by `soulseek_rs` has no offset resume, so `fetch` deletes any partial file left at `dest` before it starts: an interrupted transfer always restarts from zero. The trait makes that a requirement on every implementation, the tests' fakes included.
  - `maxfetch::sources::soulseek::download_ranked(transfer: &dyn Transfer, ranked: &[PeerCandidate], dest: &Path, sink: &dyn ProgressSink) -> anyhow::Result<PeerCandidate>` — for each peer in order, fetches into `dest.with_extension("part")` and renames to `dest` on success; on any `TimedOut`/`Failed`/stall/cancel error it removes the `.part` file, logs the peer with `tracing::warn!` and falls over to the next peer. Returns the peer that delivered the file; returns `Err` only when the list is empty or every peer failed.
  - `maxfetch::sources::soulseek::release_from_filename(filename: &str) -> (Option<String>, String)` — splits `"Artist - Title.flac"` into artist and title (title keeps no extension, artist is `None` when the stem has no ` - `).
  - `maxfetch::sources::soulseek::SoulseekSource` — `struct` with `SoulseekSource::new(credentials: SoulseekCredentials, download: DownloadConfig) -> SoulseekSource` (never touches the network, never constructs a `Client`), plus `pub fn peers(&self) -> Vec<PeerCandidate>`.
  - `impl Source for SoulseekSource`: `id` → `SourceId::Soulseek`; `capabilities` → `Capabilities { audio: true, metadata_only: false, lossless: true, needs_auth: true }`; `auth_state` → `AuthState::Missing` when username or password is empty, else `AuthState::Ok`; `search` splits the query text into the Soulseek search string; `resolve` returns the search results for a URL-shaped query; `candidates` ranks the peers found for the release; `download` runs `download_ranked` over the peers for the candidate's locator.

- [ ] **Step 1: Write the failing test**

Create `tests/soulseek_client.rs`:
```rust
use std::path::Path;
use std::time::Duration;
use maxfetch::config::{DownloadConfig, SoulseekCredentials};
use maxfetch::fetch::NoopSink;
use maxfetch::sources::soulseek::{download_ranked, drain_transfer, release_from_filename, PeerCandidate, SoulseekSource, Transfer};
use maxfetch::sources::{AuthState, Source};
use maxfetch::model::{Container, Quality};
use soulseek_rs::DownloadStatus;

fn peer(user: &str) -> PeerCandidate {
    PeerCandidate {
        username: user.into(),
        filename: "Daft Punk - Get Lucky.flac".into(),
        size: 4,
        slots: 1,
        speed: 4,
        quality: Quality { lossless: true, container: Container::Flac, bit_depth: Some(16), sample_rate: Some(44100), bitrate_kbps: None },
    }
}

struct Script {
    /// One entry per attempt: `true` succeeds, `false` fails.
    script: std::sync::Mutex<Vec<bool>>,
}

impl Transfer for Script {
    fn fetch(&self, peer: &PeerCandidate, dest: &Path, _sink: &dyn maxfetch::fetch::ProgressSink) -> anyhow::Result<u64> {
        let ok = self.script.lock().unwrap().remove(0);
        if ok {
            let body = peer.username.clone();
            std::fs::write(dest, &body)?;
            Ok(body.len() as u64)
        } else {
            anyhow::bail!("peer {} hung up", peer.username)
        }
    }
}

#[test]
fn a_silent_transfer_is_reported_as_stalled() {
    let (tx, rx) = std::sync::mpsc::channel::<DownloadStatus>();
    tx.send(DownloadStatus::InProgress { bytes_downloaded: 1024, total_bytes: 8192, speed_bytes_per_sec: 100.0 }).unwrap();
    let err = drain_transfer(rx, Duration::from_millis(50), &NoopSink).unwrap_err();
    assert!(format!("{err}").contains("stalled"));
    drop(tx);
}

#[test]
fn a_completed_transfer_returns_the_total_byte_count() {
    let (tx, rx) = std::sync::mpsc::channel::<DownloadStatus>();
    tx.send(DownloadStatus::Queued).unwrap();
    tx.send(DownloadStatus::InProgress { bytes_downloaded: 1024, total_bytes: 2048, speed_bytes_per_sec: 512.0 }).unwrap();
    tx.send(DownloadStatus::Completed).unwrap();
    assert_eq!(drain_transfer(rx, Duration::from_secs(5), &NoopSink).unwrap(), 2048);
}

#[test]
fn a_timed_out_status_is_an_error_that_names_the_reason() {
    let (tx, rx) = std::sync::mpsc::channel::<DownloadStatus>();
    tx.send(DownloadStatus::TimedOut).unwrap();
    let err = drain_transfer(rx, Duration::from_secs(5), &NoopSink).unwrap_err();
    assert!(format!("{err}").contains("timed out"));
}

#[test]
fn a_failed_status_carries_the_peer_message() {
    let (tx, rx) = std::sync::mpsc::channel::<DownloadStatus>();
    tx.send(DownloadStatus::Failed(Some("no such file".into()))).unwrap();
    let err = drain_transfer(rx, Duration::from_secs(5), &NoopSink).unwrap_err();
    assert!(format!("{err}").contains("no such file"));
}

#[test]
fn a_dropped_sender_is_a_disconnect() {
    let (tx, rx) = std::sync::mpsc::channel::<DownloadStatus>();
    drop(tx);
    let err = drain_transfer(rx, Duration::from_secs(5), &NoopSink).unwrap_err();
    assert!(format!("{err}").contains("disconnected"));
}

#[test]
fn a_failing_peer_falls_over_to_the_next_one() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("track.flac");
    let script = Script { script: std::sync::Mutex::new(vec![false, true]) };
    let ranked = vec![peer("bad"), peer("good")];
    let chosen = download_ranked(&script, &ranked, &dest, &NoopSink).unwrap();
    assert_eq!(chosen.username, "good");
    assert_eq!(std::fs::read_to_string(&dest).unwrap(), "good");
    assert!(!dir.path().join("track.part").exists());
}

#[test]
fn every_peer_failing_is_an_error_with_the_partial_file_removed() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("track.flac");
    let script = Script { script: std::sync::Mutex::new(vec![false, false]) };
    let err = download_ranked(&script, &[peer("a"), peer("b")], &dest, &NoopSink).unwrap_err();
    assert!(format!("{err}").contains("hung up"));
    assert!(!dir.path().join("track.part").exists());
    assert!(!dest.exists());
}

struct HalfWriter;

impl Transfer for HalfWriter {
    fn fetch(&self, peer: &PeerCandidate, dest: &Path, _sink: &dyn maxfetch::fetch::ProgressSink) -> anyhow::Result<u64> {
        std::fs::write(dest, vec![0u8; 4096])?;
        anyhow::bail!("peer {} dropped after 4096 bytes", peer.username)
    }
}

#[test]
fn an_interrupted_transfer_leaves_no_partial_file_behind() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("track.flac");
    let err = download_ranked(&HalfWriter, &[peer("half")], &dest, &NoopSink).unwrap_err();
    assert!(format!("{err}").contains("4096"));
    assert!(!dir.path().join("track.part").exists());
    assert!(!dest.exists());
}

#[test]
fn an_empty_peer_list_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let script = Script { script: std::sync::Mutex::new(vec![]) };
    assert!(download_ranked(&script, &[], &dir.path().join("x.flac"), &NoopSink).is_err());
}

#[test]
fn filenames_are_split_into_artist_and_title() {
    assert_eq!(release_from_filename("Daft Punk - Get Lucky.flac"), (Some("Daft Punk".to_string()), "Get Lucky".to_string()));
    assert_eq!(release_from_filename("Get Lucky.mp3"), (None, "Get Lucky".to_string()));
}

#[test]
fn a_source_without_credentials_is_missing() {
    let empty = SoulseekSource::new(SoulseekCredentials::default(), DownloadConfig::default());
    assert_eq!(empty.auth_state(), AuthState::Missing);
    let filled = SoulseekSource::new(
        SoulseekCredentials { username: "u".into(), password: "p".into() },
        DownloadConfig::default(),
    );
    assert_eq!(filled.auth_state(), AuthState::Ok);
}
```
Import `SoulseekSource` from `maxfetch::sources::soulseek` alongside the rest.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test soulseek_client`
Expected: FAIL with "unresolved import `download_ranked`".

- [ ] **Step 3: Implement the client layer**

`drain_transfer` is a blocking loop over the std mpsc `Receiver<DownloadStatus>`: `events.recv_timeout(stall_timeout)`; `Ok(DownloadStatus::InProgress { bytes_downloaded, total_bytes, speed_bytes_per_sec })` calls `sink.on_progress(bytes_downloaded, Some(total_bytes))`, remembers `total_bytes`, logs the speed at `trace!` level and continues; `Ok(DownloadStatus::Queued)` and `Ok(DownloadStatus::Paused { .. })` continue; any status where `is_terminal()` holds ends the loop — `Completed` calls `sink.on_finished(total)` and returns the remembered total, `Failed(Some(msg))` returns `anyhow::bail!("{msg}")`, `Failed(None)` returns `anyhow::bail!("transfer failed")`, `Cancelled` returns `anyhow::bail!("transfer cancelled")`, `TimedOut` returns `anyhow::bail!("transfer timed out")`; `Err(RecvTimeoutError::Timeout)` returns `anyhow::bail!("transfer stalled after {}s", stall_timeout.as_secs())`; `Err(RecvTimeoutError::Disconnected)` returns `anyhow::bail!("peer disconnected")`.

`download_ranked` iterates the ranked list, calls `transfer.fetch(peer, &part, sink)`, renames `part` to `dest` on success, and on failure removes `part` when it exists. Every failure is logged with `tracing::warn!(peer = %peer.username, error = %err, "soulseek peer failed")`; a `TimedOut`/`Failed`/stall error is treated exactly like any other error, which is what makes the fallover to the next candidate unconditional. With an empty list it bails with "нет кандидатов со свободными слотами". When the final attempt fails, the last error is returned with the peer name in the context.

`LibSoulseekTransfer` (private to `src/sources/soulseek.rs`) implements `Transfer` on top of `soulseek-rs-lib` 20.0.0. The trait method is synchronous, so `fetch` does all its work inside `tokio::task::spawn_blocking(...)` and the async `Source::download` awaits that handle; no `block_on` is used anywhere. The exact sequence is: `Client::new(username, password)` (the two-argument constructor; `Client::with_settings(settings)` is the alternative when non-default settings are needed and its fields are not used by this plan), `client.connect()?`, `client.login()?` (returns `Result<bool>`; `false` means bad credentials and is surfaced as "Soulseek отклонил логин"). Search is `client.search(&query, timeout)? -> Vec<SearchResult>`, and every `SearchResult { username, slots, speed, files, .. }` contributes one `PeerCandidate` per `File` through `peer_from_file(&sr.username, sr.slots, sr.speed, &file.name, file.size, &file.attribs)`, dropping the `None`s. Download is `client.download(filename.clone(), username.clone(), size, download_dir)? -> (Download, std::sync::mpsc::Receiver<DownloadStatus>)`; the receiver is passed straight to `drain_transfer` with `Duration::from_secs(download_config.stall_timeout_sec)` (the `DownloadConfig` field, not anything on `Download`) as the stall budget, and the returned `Download` value is kept alive for as long as the loop runs. The `Client` is never given shared directories, never binds a listen port and never calls UPnP/NAT-PMP. Searches are spaced by a short sleep to avoid flood disconnects. The client is constructed and dropped inside each `fetch` call, i.e. inside its own `spawn_blocking` closure, so no `Client` is shared across threads.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test --test soulseek_client`
Expected: PASS, 11 tests.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit --author="m1kiru <m1kiru@github.com>" -m "feat(soulseek): blocking client, stall detection and peer failover"
```

---

### Task 3: YouTube JSON parsing, metadata mapping and binary detection

**Files:**
- Create: `src/youtube.rs`
- Modify: `src/lib.rs` (declare the module)
- Create: `tests/fixtures/youtube/playlist.json`, `tests/fixtures/youtube/video.json`
- Create: `tests/fixtures/bin/yt-dlp` (executable, mode 755)
- Test: `tests/youtube_parse.rs`

**Interfaces:**
- Consumes: `maxfetch::model::{Container, Quality, Release, SourceId}` (core Task 1); `maxfetch::sources::Query` (core Task 10).
- Produces:
  - `maxfetch::youtube::YoutubeVideo` — `struct { video_id: String, title: String, duration_sec: Option<f64>, url: String }`, `Clone + Debug + PartialEq`.
  - `maxfetch::youtube::YoutubeTrackMeta` — `struct { title: String, artist: String, album: Option<String> }`, `Clone + Debug + PartialEq`.
  - `maxfetch::youtube::MAX_TRACK_SECONDS` — `const ...: u64 = 900`. Anything longer is a DJ set or a radio broadcast, not a track.
  - `maxfetch::youtube::parse_flat_playlist(json: &serde_json::Value) -> anyhow::Result<Vec<YoutubeVideo>>` — reads `entries[]` (`id`, `title`, `duration`), skips entries without an `id`, and builds `url` as `https://www.youtube.com/watch?v=<id>`.
  - `maxfetch::youtube::parse_video(json: &serde_json::Value) -> anyhow::Result<YoutubeTrackMeta>` — `title` from `track` when present, else from `title`; `artist` from `artist` when present, else from `uploader`, else from `channel`, with a trailing ` - Topic` stripped (a `- Topic` channel is the label's loader, so its `Topic` suffix is never an artist name and `Topic` alone is never returned); `album` from `album`. An error when the duration exceeds `MAX_TRACK_SECONDS` (`"слишком длинное видео (<n> с): DJ-сет или радиопередача"`), and a fallback to `"Unknown Artist"` when the stripped artist would be empty.
  - `maxfetch::youtube::bestaudio_quality(json: &serde_json::Value) -> Quality` — picks the `formats[]` entry with the highest `abr`, maps `asr` to `sample_rate` and `abr` to `bitrate_kbps` (rounded), and always returns `lossless: false, container: Container::Opus` when the winning entry's `acodec` is `opus`, `Container::M4aAac` otherwise.
  - `maxfetch::youtube::run_json(yt_dlp_path: &str, args: &[&str]) -> anyhow::Result<serde_json::Value>` — runs the binary and parses stdout; a spawn failure becomes `anyhow::Error` mentioning `yt-dlp`.
  - `maxfetch::youtube::binary_status(path: &str) -> Option<String>` — `None` when `<path> --version` exits 0, otherwise `Some("yt-dlp не найден: <path>")`.
  - `maxfetch::youtube::is_playlist_url(url: &str) -> bool` — true for a URL containing `list=`.

- [ ] **Step 1: Create the fixtures and the failing test**

`tests/fixtures/youtube/playlist.json`:
```json
{"_type":"playlist","id":"PL1","title":"Mix","entries":[
  {"_type":"url","id":"abc123","title":"Daft Punk - Get Lucky","duration":369.0},
  {"_type":"url","id":"def456","title":"Daft Punk - One More Time","duration":320.0},
  {"_type":"url","title":"no id, must be skipped"}
]}
```

`tests/fixtures/youtube/video.json`:
```json
{"_type":"video","id":"abc123","title":"Daft Punk - Get Lucky (Official Video)","track":"Get Lucky","artist":"Daft Punk","album":"Random Access Memories","duration":369.0,"uploader":"Daft Punk","channel":"Daft Punk","formats":[
  {"format_id":"251","ext":"webm","acodec":"opus","abr":160.0,"asr":48000},
  {"format_id":"140","ext":"m4a","acodec":"mp4a.40.2","abr":128.0,"asr":44100}
]}
```

`tests/fixtures/bin/yt-dlp` (make it executable with `chmod +x`):
```bash
#!/usr/bin/env bash
set -euo pipefail
cat "$(cd "$(dirname "$0")/.." && pwd)/youtube/playlist.json"
```

Create `tests/youtube_parse.rs`:
```rust
use std::path::PathBuf;
use maxfetch::model::Container;
use maxfetch::youtube::{bestaudio_quality, binary_status, is_playlist_url, parse_flat_playlist, parse_video, run_json};

fn fixture_json(name: &str) -> serde_json::Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/youtube").join(name);
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn fake_ytdlp() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/bin/yt-dlp")
}

#[test]
fn a_flat_playlist_yields_videos_with_ids_and_durations() {
    let videos = parse_flat_playlist(&fixture_json("playlist.json")).unwrap();
    assert_eq!(videos.len(), 2);
    assert_eq!(videos[0].video_id, "abc123");
    assert_eq!(videos[0].title, "Daft Punk - Get Lucky");
    assert_eq!(videos[0].duration_sec, Some(369.0));
    assert_eq!(videos[0].url, "https://www.youtube.com/watch?v=abc123");
}

#[test]
fn track_fields_become_the_tag_metadata() {
    let meta = parse_video(&fixture_json("video.json")).unwrap();
    assert_eq!(meta.title, "Get Lucky");
    assert_eq!(meta.artist, "Daft Punk");
    assert_eq!(meta.album.as_deref(), Some("Random Access Memories"));
}

#[test]
fn a_video_without_track_fields_falls_back_to_the_uploader() {
    let json: serde_json::Value = serde_json::from_str(r#"{"title":"Some Clip","uploader":"Chan","formats":[]}"#).unwrap();
    let meta = parse_video(&json).unwrap();
    assert_eq!(meta.title, "Some Clip");
    assert_eq!(meta.artist, "Chan");
    assert_eq!(meta.album, None);
}

#[test]
fn a_topic_channel_is_a_loader_not_an_artist() {
    let json: serde_json::Value = serde_json::from_str(r#"{"title":"Get Lucky","channel":"Daft Punk - Topic","uploader":"Daft Punk - Topic","duration":369}"#).unwrap();
    let meta = parse_video(&json).unwrap();
    assert_eq!(meta.artist, "Daft Punk");
    assert!(!meta.artist.contains("Topic"));
}

#[test]
fn a_three_hour_dj_set_is_rejected() {
    let json: serde_json::Value = serde_json::from_str(r#"{"title":"Boiler Room Set","channel":"Some DJ","duration":10800}"#).unwrap();
    let err = parse_video(&json).unwrap_err();
    assert!(format!("{err}").contains("DJ"));
    assert_eq!(maxfetch::youtube::MAX_TRACK_SECONDS, 900);
}

#[test]
fn a_radio_station_uploader_becomes_the_artist_without_inventing_data() {
    let json: serde_json::Value = serde_json::from_str(r#"{"title":"Midnight Hour","uploader":"Radio Nimbus","duration":210}"#).unwrap();
    let meta = parse_video(&json).unwrap();
    assert_eq!(meta.title, "Midnight Hour");
    assert_eq!(meta.artist, "Radio Nimbus");
    assert_eq!(meta.album, None);
}

#[test]
fn bestaudio_is_opus_at_160_kbps_and_is_lossy() {
    let q = bestaudio_quality(&fixture_json("video.json"));
    assert!(!q.lossless);
    assert_eq!(q.container, Container::Opus);
    assert_eq!(q.sample_rate, Some(48000));
    assert_eq!(q.bitrate_kbps, Some(160));
}

#[test]
fn the_fake_binary_answers_with_the_canned_json() {
    let v = run_json(fake_ytdlp().to_str().unwrap(), &["-J", "--flat-playlist", "https://www.youtube.com/playlist?list=PL1"]).unwrap();
    assert_eq!(v["entries"].as_array().unwrap().len(), 3);
}

#[test]
fn a_missing_binary_is_an_error_not_a_panic() {
    let err = run_json("/nonexistent/yt-dlp", &["-J", "x"]).unwrap_err();
    assert!(format!("{err}").contains("yt-dlp"));
    assert!(binary_status("/nonexistent/yt-dlp").unwrap().contains("yt-dlp"));
    assert!(binary_status("/bin/sh").is_none());
}

#[test]
fn playlist_urls_are_recognized() {
    assert!(is_playlist_url("https://www.youtube.com/playlist?list=PL1"));
    assert!(is_playlist_url("https://www.youtube.com/watch?v=x&list=PL1"));
    assert!(!is_playlist_url("https://www.youtube.com/watch?v=x"));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test youtube_parse`
Expected: FAIL with "unresolved import `maxfetch::youtube`".

- [ ] **Step 3: Implement `src/youtube.rs`**

`run_json` spawns `std::process::Command::new(yt_dlp_path).args(args).output()`, propagates a spawn error as `anyhow::Error` whose message names `yt-dlp`, fails on a non-zero exit including the tail of stderr, and parses stdout with `serde_json::from_slice`. `binary_status` runs `Command::new(path).arg("--version")` and returns the Russian message on a spawn error or a non-zero status. `bestaudio_quality` iterates `formats`, ignores entries without `abr`, keeps the highest, and reads `asr`/`abr` as `f64` then converts. `parse_video` reads the string fields defensively: a missing `track` falls back to `title`, a missing `artist` to `uploader` then `channel`; a trailing ` - Topic` (case-insensitive) is stripped from whichever artist source won, and `Topic` alone falls through to `"Unknown Artist"`; a missing title is an error mentioning the video id when present. A `duration` above `MAX_TRACK_SECONDS` (900) is an error naming the video as a DJ set or radio broadcast; a missing `duration` is not a rejection, it just leaves the length unknown.

- [ ] **Step 4: Declare the module and run the test**

Add `pub mod youtube;` to `src/lib.rs`.

Run: `cargo test --test youtube_parse`
Expected: PASS, 10 tests.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit --author="m1kiru <m1kiru@github.com>" -m "feat(youtube): yt-dlp json parsing and metadata mapping"
```

---

### Task 4: YouTube audio download, remux and the untagged sidecar

**Files:**
- Modify: `src/youtube.rs`
- Modify: `src/sources/mod.rs` (declare the source module)
- Create: `src/sources/youtube.rs`
- Test: `tests/youtube_audio.rs`

**Interfaces:**
- Consumes: `maxfetch::media::{remux_to_opus, write_sidecar}` (core Task 9); `maxfetch::probe::probe` (core Task 6); `maxfetch::fetch::ProgressSink`; `YoutubeTrackMeta`, `parse_video`, `parse_flat_playlist`, `run_json`, `bestaudio_quality` (Task 3); `maxfetch::sources::{Capabilities, Source}` (core Task 10); `maxfetch::config::YoutubeConfig`, `MediaConfig` (core Task 3).
- Produces:
  - `maxfetch::youtube::FinalizedAudio` — `struct { path: PathBuf, container: Container, untagged: bool, sidecar: Option<PathBuf> }`. `container` is always the container `maxfetch::probe::probe` reports for the finished file, never a guess from the path. `untagged` is true exactly on the fallback path (`.webm` bytes plus a sidecar JSON plus the `UNTAGGED` badge) and false when the remux produced a tagged `.opus`.
  - `maxfetch::youtube::download_audio(yt_dlp_path: &str, video_id: &str, url: &str, work_dir: &Path, sink: &dyn ProgressSink) -> anyhow::Result<PathBuf>` — runs `yt-dlp -f bestaudio -o <work_dir>/<video_id>.%(ext)s <url>` and returns the single file created in `work_dir`.
  - `maxfetch::youtube::finalize(ffmpeg_path: &str, downloaded: &Path, dest: &Path, video_id: &str, meta: &YoutubeTrackMeta) -> anyhow::Result<FinalizedAudio>` — remuxes to `dest` with `maxfetch::media::remux_to_opus`; on success `untagged` is `false` and `sidecar` is `None`. On a remux failure the downloaded bytes are copied to `dest` and `sidecar_payload` is written to `<dest>.json` through `write_sidecar`; `untagged` is then `true`. In both branches `container` comes from `probe(dest)`: an AAC stream delivered under a `.webm` name reports its real container, not `Webm`, and never `Opus`.
  - `maxfetch::youtube::sidecar_payload(video_id: &str, meta: &YoutubeTrackMeta) -> serde_json::Value` — keys `source` (`"youtube"`), `video_id`, `title`, `artist`, `album`.
  - `maxfetch::youtube::untagged(path: &Path) -> bool` — true when `path` exists and `<path>.json` exists next to it.
  - `maxfetch::youtube::move_sidecar(temp_media: &Path, final_media: &Path) -> anyhow::Result<Option<PathBuf>>` — renames `<temp_media>.json` to `<final_media>.json` and returns the new path, or `None` when there is no sidecar to move.
  - `maxfetch::sources::youtube::split_artist_title(filename: &str) -> Option<(String, String)>` — the one splitter this task uses. Input is a file name of the shape `<artist> - <title>.<ext>`; the stem is taken first, then split on the first ` - ` separator (case-insensitive ` - ` with optional surrounding spaces is not required: the separator is the literal ` - `). `Some((artist, title))` with both sides trimmed and non-empty when a separator exists; `None` when the stem has no ` - ` or either side would be empty. This replaces every "`release_from_filename`-style splitting" reference in this task; `soulseek::release_from_filename` stays a Soulseek-only helper with its own fallback contract.
  - `maxfetch::sources::youtube::YoutubeSource` — `struct` with `new(config: &YoutubeConfig, media: &MediaConfig) -> YoutubeSource`; `impl Source` with `id` → `SourceId::Youtube`, `capabilities` → `Capabilities { audio: true, metadata_only: false, lossless: false, needs_auth: false }`, `auth_state` → `AuthState::NotNeeded`, `search` → one `Release` per playlist entry or the single video, `resolve` → the same for a playlist URL, `candidates` → one `Candidate` with the `bestaudio_quality` result and `Locator::Youtube { url, video_id }`, `download` → `download_audio` then `finalize` into `dest`.

- [ ] **Step 1: Write the failing test**

Create `tests/youtube_audio.rs`:
```rust
use std::path::PathBuf;
use maxfetch::model::Container;
use maxfetch::sources::youtube::split_artist_title;
use maxfetch::youtube::{finalize, move_sidecar, sidecar_payload, untagged, YoutubeTrackMeta};

fn fixture_flac() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/probe/sine_44100_s16.flac")
}

fn meta() -> YoutubeTrackMeta {
    YoutubeTrackMeta { title: "Get Lucky".into(), artist: "Daft Punk".into(), album: Some("Random Access Memories".into()) }
}

/// Builds an Opus-in-WebM file without re-encoding the fixture container.
fn webm_fixture(dir: &std::path::Path) -> PathBuf {
    let out = dir.join("abc123.webm");
    let status = std::process::Command::new("ffmpeg")
        .args(["-nostdin", "-y", "-i"]).arg(fixture_flac())
        .args(["-vn", "-c:a", "libopus", "-b:a", "160k"]).arg(&out)
        .status().unwrap();
    assert!(status.success());
    out
}

#[test]
fn a_remuxed_youtube_file_is_opus_and_carries_no_sidecar() {
    let dir = tempfile::tempdir().unwrap();
    let src = webm_fixture(dir.path());
    let dest = dir.path().join("out.bin");
    let done = finalize("ffmpeg", &src, &dest, "abc123", &meta()).unwrap();
    assert!(!done.untagged);
    assert_eq!(done.container, Container::Opus);
    assert!(done.sidecar.is_none());
    assert!(!untagged(&dest));
    let probed = maxfetch::probe::probe(&dest).unwrap();
    assert_eq!(probed.container, Container::Opus);
}

/// AAC bytes under a `.webm` name: `ffmpeg` cannot mux AAC into a real WebM, so
/// the m4a is produced first and then renamed to the misleading extension.
fn aac_fixture(dir: &std::path::Path) -> PathBuf {
    let m4a = dir.join("aac.m4a");
    let status = std::process::Command::new("ffmpeg")
        .args(["-nostdin", "-y", "-i"]).arg(fixture_flac())
        .args(["-vn", "-c:a", "aac", "-b:a", "128k"]).arg(&m4a)
        .status().unwrap();
    assert!(status.success());
    let out = dir.join("abc123.webm");
    std::fs::rename(&m4a, &out).unwrap();
    out
}

#[test]
fn an_aac_stream_under_a_webm_name_reports_its_real_container() {
    let dir = tempfile::tempdir().unwrap();
    let src = aac_fixture(dir.path());
    let dest = dir.path().join("out.webm");
    let done = finalize("ffmpeg", &src, &dest, "abc123", &meta()).unwrap();
    assert!(done.untagged);
    assert_eq!(done.container, Container::M4aAac);
    assert_ne!(done.container, Container::Webm);
    assert_ne!(done.container, Container::Opus);
    assert_eq!(dest.extension().and_then(|e| e.to_str()), Some("webm"));
    assert!(done.sidecar.is_some());
    assert!(untagged(&dest));
}

#[test]
fn a_remux_failing_on_an_unusual_input_falls_back_without_panicking() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("abc123.webm");
    std::fs::write(&src, b"not a container ffmpeg can demux").unwrap();
    let dest = dir.path().join("out.webm");
    let done = finalize("ffmpeg", &src, &dest, "abc123", &meta()).unwrap();
    assert!(done.untagged);
    assert_ne!(done.container, Container::Opus);
    assert!(done.sidecar.is_some());
    assert!(untagged(&dest));
}

#[test]
fn a_channel_name_is_split_only_when_the_title_carries_a_separator() {
    assert_eq!(split_artist_title("Daft Punk - Get Lucky.webm"), Some(("Daft Punk".into(), "Get Lucky".into())));
    assert_eq!(split_artist_title("Get Lucky.webm"), None);
    assert_eq!(split_artist_title(" - Get Lucky.webm"), None);
}

#[test]
fn a_missing_ffmpeg_leaves_webm_and_writes_the_sidecar() {
    let dir = tempfile::tempdir().unwrap();
    let src = webm_fixture(dir.path());
    let dest = dir.path().join("out.bin");
    let done = finalize("/nonexistent/ffmpeg", &src, &dest, "abc123", &meta()).unwrap();
    assert!(done.untagged);
    assert_eq!(done.container, Container::Webm);
    assert_eq!(done.sidecar.as_deref(), Some(dir.path().join("out.bin.json").as_path()));
    assert!(untagged(&dest));
    let raw = std::fs::read_to_string(dir.path().join("out.bin.json")).unwrap();
    assert!(raw.contains("youtube"));
    assert!(raw.contains("abc123"));
    assert!(raw.contains("Get Lucky"));
}

#[test]
fn the_sidecar_payload_names_the_source_and_the_video() {
    let v = sidecar_payload("abc123", &meta());
    assert_eq!(v["source"], "youtube");
    assert_eq!(v["video_id"], "abc123");
    assert_eq!(v["artist"], "Daft Punk");
    assert_eq!(v["album"], "Random Access Memories");
}

#[test]
fn the_sidecar_follows_the_file_on_relocation() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path().join("tmp.bin");
    std::fs::write(&tmp, b"x").unwrap();
    std::fs::write(dir.path().join("tmp.bin.json"), b"{}").unwrap();
    let final_path = dir.path().join("01 Get Lucky.webm");
    let moved = move_sidecar(&tmp, &final_path).unwrap();
    assert_eq!(moved.as_deref(), Some(final_path.with_extension("webm.json").as_path()));
    assert!(final_path.with_extension("webm.json").exists());
    assert!(!dir.path().join("tmp.bin.json").exists());
}

#[test]
fn relocation_without_a_sidecar_is_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path().join("tmp.opus");
    std::fs::write(&tmp, b"x").unwrap();
    assert!(move_sidecar(&tmp, &dir.path().join("01 Get Lucky.opus")).unwrap().is_none());
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test youtube_audio`
Expected: FAIL with "unresolved import `finalize`".

- [ ] **Step 3: Implement the audio path and the source**

`download_audio` builds the output template with `format!("{}/{}.%(ext)s", work_dir.display(), video_id)`, runs `yt-dlp -f bestaudio -o <template> <url>` through `std::process::Command`, fails on a non-zero exit with the tail of stderr, then scans `work_dir` for the file whose stem equals `video_id` and returns it (exactly one such file; otherwise an error listing what was found).

`finalize` calls `maxfetch::media::remux_to_opus(ffmpeg_path, downloaded, dest)`; on `Ok` it removes the source file and returns `untagged: false`. On `Err` it logs `tracing::warn!`, copies `downloaded` to `dest`, writes `sidecar_payload` with `write_sidecar(dest, ...)` and returns `untagged: true`. The `Container` in both branches is decided by probing `dest` (`maxfetch::probe::probe`), never by the file extension, so a non-Opus stream reports the container it actually is; nothing in this path hardcodes `Container::Webm`. A probe failure on the finished file is returned as an error naming the destination rather than defaulting to `Webm`.

`YoutubeSource::download` creates a temporary work directory under `std::env::temp_dir()`, downloads, finalizes into `dest`, and removes the work directory; a missing `yt-dlp` surfaces as a clear error mentioning the configured path. `search` for a playlist URL runs `run_json(path, &["-J", "--flat-playlist", url])` and maps every entry to a `Release` through `split_artist_title(&video.title)`, falling back to the video title as the title and `"Unknown Artist"` as the artist; `resolve` for `Query::url` does the same for a playlist and `-J <url>` for a single video.

- [ ] **Step 4: Declare the module and run the test**

Add `pub mod youtube;` to `src/sources/mod.rs` (`src/lib.rs` already declares the top-level module in Task 3).

Run: `cargo test --test youtube_audio`
Expected: PASS, 8 tests.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit --author="m1kiru <m1kiru@github.com>" -m "feat(youtube): audio download, stream-copy remux and sidecar fallback"
```

---

### Task 5: Spotify JSON mapping and the HTTP seam

**Files:**
- Modify: `Cargo.toml` (add `rspotify` 0.16.1)
- Create: `src/sources/spotify.rs`
- Create: `tests/fixtures/spotify/track.json`
- Test: `tests/spotify_mapping.rs`

**Interfaces:**
- Consumes: `maxfetch::model::{Release, SourceId}` (core Task 1); `maxfetch::config::SpotifyCredentials` (core Task 3).
- Produces:
  - `maxfetch::sources::spotify::SpotifyApi` — `trait { fn search_tracks(&self, query: &str) -> anyhow::Result<Vec<serde_json::Value>>; fn track(&self, id: &str) -> anyhow::Result<serde_json::Value>; fn album(&self, id: &str) -> anyhow::Result<Vec<serde_json::Value>>; fn playlist_items(&self, id: &str) -> anyhow::Result<Vec<serde_json::Value>>; }`, `Send + Sync`.
  - `maxfetch::sources::spotify::UnconfiguredApi` — an implementation whose every method returns an error mentioning that Spotify is not configured; it must never panic.
  - `maxfetch::sources::spotify::isrc(track: &serde_json::Value) -> Option<String>` — `external_ids.isrc`.
  - `maxfetch::sources::spotify::duration_sec(track: &serde_json::Value) -> Option<f64>` — `duration_ms / 1000.0`.
  - `maxfetch::sources::spotify::track_to_release(track: &serde_json::Value, source_item_id: &str) -> anyhow::Result<Release>` — `title` from `name`, `artist` from `artists[0].name`, `album` from `album.name`, `track_no` from `track_number`, `year` from the first four characters of `album.release_date`, `duration_sec` and `isrc` from the helpers above, `source: SourceId::Spotify`. This field list is wider than spec section 8.4, which names only `external_ids.isrc` and `duration_ms`; the real Spotify API returns all of them, so the plan keeps the wider mapping and spec section 8.4 will be extended to this list by a separate edit (that edit does not touch this file).
  - `maxfetch::sources::spotify::parse_token_response(body: &serde_json::Value) -> anyhow::Result<String>` — the `access_token` string; a body without it is an error whose message is in Russian and names the missing field. A body that carries `"error": "invalid_client"` is always an error with the fixed message `"ключи Spotify отклонены"`, even when the HTTP status was 200.
  - `maxfetch::sources::spotify::SPOTIFY_KEYS_REJECTED` — `const ...: &str = "ключи Spotify отклонены"`, the one string shared by the token parser, the log line and the TUI/AuthState wording.
  - `maxfetch::sources::spotify::SpotifySource` — `struct` with `new(api: std::sync::Arc<dyn SpotifyApi>, client_id: String, client_secret: String) -> SpotifySource`.
  - `maxfetch::sources::spotify::RspotifyApi` — `struct` with `RspotifyApi::new(client_id: String, client_secret: String) -> RspotifyApi`; implements `SpotifyApi` on top of `rspotify` 0.16.1 with the `client_credentials` flow and the three working endpoints.

- [ ] **Step 1: Write the fixture and the failing test**

`tests/fixtures/spotify/track.json`:
```json
{"id":"4uLU6hMCjMI75M1A2tKUQC","name":"Get Lucky","artists":[{"name":"Daft Punk"}],
 "album":{"name":"Random Access Memories","release_date":"2013-05-17"},
 "duration_ms":369000,"track_number":8,"external_ids":{"isrc":"USQX91300108"}}
```

Create `tests/spotify_mapping.rs`:
```rust
use std::path::PathBuf;
use maxfetch::model::SourceId;
use maxfetch::sources::spotify::{duration_sec, isrc, parse_token_response, track_to_release, UnconfiguredApi, SpotifyApi};

fn track_json() -> serde_json::Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/spotify/track.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn the_isrc_comes_from_external_ids() {
    assert_eq!(isrc(&track_json()).as_deref(), Some("USQX91300108"));
    assert_eq!(isrc(&serde_json::json!({"external_ids": {}})), None);
}

#[test]
fn the_duration_is_converted_from_milliseconds() {
    assert_eq!(duration_sec(&track_json()), Some(369.0));
    assert_eq!(duration_sec(&serde_json::json!({})), None);
}

#[test]
fn a_track_maps_to_a_spotify_release() {
    let rel = track_to_release(&track_json(), "4uLU6hMCjMI75M1A2tKUQC").unwrap();
    assert_eq!(rel.title, "Get Lucky");
    assert_eq!(rel.artist, "Daft Punk");
    assert_eq!(rel.album.as_deref(), Some("Random Access Memories"));
    assert_eq!(rel.track_no, Some(8));
    assert_eq!(rel.year, Some(2013));
    assert_eq!(rel.duration_sec, Some(369.0));
    assert_eq!(rel.isrc.as_deref(), Some("USQX91300108"));
    assert_eq!(rel.source, SourceId::Spotify);
}

#[test]
fn the_token_is_read_from_the_credentials_response() {
    let body = serde_json::json!({"access_token": "BQD1", "token_type": "Bearer", "expires_in": 3600});
    assert_eq!(parse_token_response(&body).unwrap(), "BQD1");
}

#[test]
fn a_response_without_a_token_is_a_readable_error() {
    let err = parse_token_response(&serde_json::json!({"token_type": "Bearer"})).unwrap_err();
    assert!(format!("{err}").contains("access_token"));
}

#[test]
fn a_200_with_an_invalid_client_body_means_the_keys_were_rejected() {
    let body = serde_json::json!({"error": "invalid_client", "error_description": "Invalid client"});
    let err = parse_token_response(&body).unwrap_err();
    assert_eq!(format!("{err}"), maxfetch::sources::spotify::SPOTIFY_KEYS_REJECTED);
    assert_eq!(format!("{err}"), "ключи Spotify отклонены");

    let status = maxfetch::sources::SourceStatus {
        id: SourceId::Spotify,
        capabilities: maxfetch::sources::Capabilities { audio: false, metadata_only: true, lossless: false, needs_auth: true },
        auth: maxfetch::sources::AuthState::Expired,
        detail: Some(maxfetch::sources::spotify::SPOTIFY_KEYS_REJECTED.into()),
    };
    assert_eq!(status.auth, maxfetch::sources::AuthState::Expired);
    assert_eq!(status.detail.as_deref(), Some("ключи Spotify отклонены"));
}

#[test]
fn the_unconfigured_api_errors_instead_of_panicking() {
    assert!(UnconfiguredApi.track("x").is_err());
    assert!(UnconfiguredApi.search_tracks("x").is_err());
    assert!(UnconfiguredApi.playlist_items("x").is_err());
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test spotify_mapping`
Expected: FAIL with "unresolved import `maxfetch::sources::spotify`".

- [ ] **Step 3: Implement the mapping and the seam**

Add the dependency: `cargo add rspotify@0.16.1`.

`track_to_release` returns an error naming the missing field when `id` or `name` is absent, ignores entries without `artists`, and never invents a year: an unparsable `release_date` yields `None`. `duration_sec` divides by 1000.0 keeping the fraction. `parse_token_response` checks the body for an `error` member first and returns `anyhow!(SPOTIFY_KEYS_REJECTED)` for `invalid_client` (and for any other credential-shaped error) before it ever looks for `access_token`, so an HTTP 200 carrying an error body never counts as a successful login; the caller records `AuthState::Expired` on the source and logs the same string with `tracing::warn!`. `RspotifyApi` performs the `client_credentials` exchange with `rspotify`'s credentials flow and keeps the resulting token in a `Mutex`, refreshing it when a request answers 401. `search_tracks` uses `/v1/search?q=&type=track,album,artist`, `track` uses `/v1/tracks/{id}`, `album` uses `/v1/albums/{id}`, and `playlist_items` uses `/v1/playlists/{id}/items` — this method is only reachable from the stage-2 user-auth flow, and in stage 1 its error is replaced by the friendly message in Task 6 before it is ever called.

- [ ] **Step 4: Declare the module and run the test**

Add `pub mod spotify;` to `src/sources/mod.rs`.

Run: `cargo test --test spotify_mapping`
Expected: PASS, 7 tests.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit --author="m1kiru <m1kiru@github.com>" -m "feat(spotify): json mapping and http seam"
```

---

### Task 6: Spotify source behaviour, playlist refusal and reference adoption

**Files:**
- Modify: `src/sources/spotify.rs`
- Test: `tests/spotify_source.rs`

**Interfaces:**
- Consumes: `SpotifyApi`, `track_to_release`, `SpotifySource` (Task 5); `maxfetch::match::match_key` (core Task 2); `maxfetch::sources::{AuthState, Capabilities, Query, QueryItem, Source}` (core Task 10).
- Produces:
  - `maxfetch::sources::spotify::playlist_requires_auth_message() -> &'static str` — exactly `"нужна авторизация Spotify, она появится на этапе 2"`.
  - `maxfetch::sources::spotify::adopt_reference(target: &mut Release, reference: &Release) -> bool` — overwrites `target.title`, `target.artist`, `target.album` and `target.isrc` from `reference` when `target.isrc` equals `reference.isrc`, or when both ISRCs are `None` and `match_key(target) == match_key(reference)`; returns whether anything was adopted. `duration_sec`, `track_no` and `year` are never touched.
  - `impl Source for SpotifySource`: `id` → `SourceId::Spotify`; `capabilities` → `Capabilities { audio: false, metadata_only: true, lossless: false, needs_auth: true }`; `auth_state` → `Missing` when either credential string is empty, else `Ok`; `search` → `track_to_release` for every `search_tracks` hit; `resolve` → the track/album for `QueryItem::Track`/`QueryItem::Album`, and `Err(playlist_requires_auth_message())` for `QueryItem::Playlist { source: SourceId::Spotify, .. }` **before** calling the API; `candidates` → `Vec::new()`; `download` → `Err` with `"Spotify не отдаёт аудио: источник только для метаданных"`.

- [ ] **Step 1: Write the failing test**

Create `tests/spotify_source.rs`:
```rust
use std::sync::Arc;
use maxfetch::model::{Release, SourceId};
use maxfetch::sources::spotify::{adopt_reference, playlist_requires_auth_message, SpotifySource, UnconfiguredApi};
use maxfetch::sources::{AuthState, Query, QueryItem, Source};

fn rel(source: SourceId, title: &str, artist: &str, album: Option<&str>, isrc: Option<&str>) -> Release {
    Release {
        title: title.into(),
        artist: artist.into(),
        album: album.map(|s| s.into()),
        track_no: None,
        duration_sec: Some(369.0),
        isrc: isrc.map(|s| s.into()),
        year: None,
        source,
        source_item_id: "x".into(),
    }
}

fn source() -> SpotifySource {
    SpotifySource::new(Arc::new(UnconfiguredApi), "id".into(), "secret".into())
}

#[tokio::test]
async fn a_playlist_request_without_user_auth_returns_a_friendly_error() {
    let q = Query { text: None, url: None, items: vec![QueryItem::Playlist { source: SourceId::Spotify, id: "37i9dQ".into() }] };
    let err = source().resolve(&q).await.unwrap_err();
    let msg = format!("{err}");
    assert_eq!(msg, playlist_requires_auth_message());
    assert_eq!(msg, "нужна авторизация Spotify, она появится на этапе 2");
    assert!(!msg.contains("403"));
}

#[tokio::test]
async fn spotify_lists_no_candidates_and_refuses_to_download() {
    let r = rel(SourceId::Spotify, "Get Lucky", "Daft Punk", Some("RAM"), Some("USQX91300108"));
    assert!(source().candidates(&r).await.unwrap().is_empty());
    let cand = maxfetch::model::Candidate {
        source: SourceId::Spotify,
        release: r,
        quality: maxfetch::model::Quality::default(),
        size_bytes: None,
        locator: maxfetch::model::Locator::Spotify { track_id: "4uLU6hMCjMI75M1A2tKUQC".into() },
    };
    let err = source().download(&cand, std::path::Path::new("/tmp/x"), &maxfetch::fetch::NoopSink).await.unwrap_err();
    assert!(format!("{err}").contains("аудио"));
}

#[test]
fn a_matching_isrc_adopts_the_spotify_description() {
    let mut target = rel(SourceId::Soulseek, "get lucky", "daft punk", None, Some("USQX91300108"));
    let reference = rel(SourceId::Spotify, "Get Lucky", "Daft Punk", Some("Random Access Memories"), Some("USQX91300108"));
    assert!(adopt_reference(&mut target, &reference));
    assert_eq!(target.title, "Get Lucky");
    assert_eq!(target.artist, "Daft Punk");
    assert_eq!(target.album.as_deref(), Some("Random Access Memories"));
    assert_eq!(target.isrc.as_deref(), Some("USQX91300108"));
    assert_eq!(target.duration_sec, Some(369.0));
}

#[test]
fn conflicting_isrcs_are_never_adopted() {
    let mut target = rel(SourceId::Soulseek, "Other", "Someone", None, Some("USQX91300108"));
    let reference = rel(SourceId::Spotify, "Get Lucky", "Daft Punk", Some("RAM"), Some("GB0000000001"));
    assert!(!adopt_reference(&mut target, &reference));
    assert_eq!(target.artist, "Someone");
}

#[test]
fn two_descriptions_without_isrc_match_by_key() {
    let mut target = rel(SourceId::Soulseek, "Get Lucky", "Daft Punk", Some("RAM"), None);
    let reference = rel(SourceId::Spotify, "Get Lucky", "Daft Punk", Some("RAM"), None);
    assert!(adopt_reference(&mut target, &reference));
    assert_eq!(target.source, SourceId::Soulseek);
}

#[test]
fn credentials_decide_the_auth_state() {
    assert_eq!(source().auth_state(), AuthState::Ok);
    let empty = SpotifySource::new(Arc::new(UnconfiguredApi), String::new(), String::new());
    assert_eq!(empty.auth_state(), AuthState::Missing);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test spotify_source`
Expected: FAIL with "no method named `resolve`".

- [ ] **Step 3: Implement the trait**

`resolve` inspects `q.items` first: a `QueryItem::Playlist { source: SourceId::Spotify, .. }` returns `Err(anyhow!("{}", playlist_requires_auth_message()))` without touching `self.api`; `QueryItem::Track` calls `track` and `track_to_release`; `QueryItem::Album` calls `album` and maps every track of it. `search` maps every hit of `search_tracks` and drops entries that fail to map, logging them with `tracing::warn!`. `adopt_reference` implements exactly the two acceptance conditions and leaves `source` and `source_item_id` of the target alone.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test --test spotify_source`
Expected: PASS, 6 tests.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit --author="m1kiru <m1kiru@github.com>" -m "feat(spotify): catalog source, playlist refusal and reference adoption"
```

---

### Task 7: Registering the three sources and their status

**Files:**
- Modify: `src/sources/mod.rs`
- Test: `tests/sources_registry.rs`

**Interfaces:**
- Consumes: `SoulseekSource` (Task 2), `YoutubeSource` (Task 4), `SpotifySource`/`RspotifyApi`/`UnconfiguredApi` (Tasks 5, 6); `maxfetch::config::{Config, Credentials}`; `Capabilities`, `AuthState`, `Source`, `SourceStatus` (core Task 10).
- Produces:
  - `maxfetch::sources::registry(config: &Config, credentials: &Credentials) -> Vec<Box<dyn Source>>` — now returns three sources, always in the order Soulseek, YouTube, Spotify, regardless of `config.sources.enabled` (the pipeline filters on `enabled_in`). Construction never opens a socket: `SoulseekSource` connects on its first call, YouTube only stores the configured binary paths, Spotify builds `RspotifyApi` when both credentials are present and `UnconfiguredApi` otherwise.
  - `maxfetch::sources::source_checks(sources: &[Box<dyn Source>]) -> Vec<SourceStatus>` — one `SourceStatus` per source, with `detail` holding the Russian diagnosis: the Soulseek/Spotify credential hint when `auth_state` is `Missing`, the `youtube::binary_status` message when `yt-dlp` is absent, and `None` otherwise.
  - `maxfetch::sources::enabled_in(ids: &[SourceId], config: &Config) -> Vec<SourceId>` — returns `ids` filtered by `config.sources.enabled`, preserving the input order. An empty `enabled` list means "everything enabled". This is the single filter the pipeline and the TUI use, so `registry` stays a pure constructor and no consumer re-implements the string matching. Its per-id helper is `is_enabled(id: &SourceId, config: &Config) -> bool` (core Task 10); `enabled_in` is the only selection function, and there is no second name for it.
  - `maxfetch::sources::skip_reason(source: &dyn Source) -> Option<String>` — `Some("источник <id> пропущен: нет авторизации")` for `AuthState::Missing`, `Some("источник <id> пропущен: ключи протухли")` for `Expired`, `None` for `Ok` and `NotNeeded`.

- [ ] **Step 1: Write the failing test**

Create `tests/sources_registry.rs`:
```rust
use maxfetch::config::{Config, Credentials};
use maxfetch::model::SourceId;
use maxfetch::sources::{enabled_in, registry, skip_reason, source_checks, AuthState, Source};

#[test]
fn the_registry_holds_the_three_stage_one_sources() {
    let sources = registry(&Config::default(), &Credentials::default());
    let ids: Vec<SourceId> = sources.iter().map(|s| s.id()).collect();
    assert_eq!(ids, vec![SourceId::Soulseek, SourceId::Youtube, SourceId::Spotify]);
}

#[test]
fn a_disabled_source_is_still_registered_and_only_filtered_on_use() {
    let mut cfg = Config::default();
    cfg.sources.enabled = vec!["soulseek".into()];
    let sources = registry(&cfg, &Credentials::default());
    assert_eq!(sources.len(), 3);
    let ids: Vec<SourceId> = sources.iter().map(|s| s.id()).collect();
    assert_eq!(enabled_in(&ids, &cfg), vec![SourceId::Soulseek]);
    assert_eq!(enabled_in(&[], &cfg), Vec::<SourceId>::new());

    cfg.sources.enabled = vec!["youtube".into(), "spotify".into()];
    assert_eq!(enabled_in(&ids, &cfg), vec![SourceId::Youtube, SourceId::Spotify]);

    // An empty enabled list means "everything enabled", in the input order.
    cfg.sources.enabled = Vec::new();
    assert_eq!(enabled_in(&ids, &cfg), ids);
    assert_eq!(registry(&cfg, &Credentials::default()).len(), 3);
}

#[test]
fn authorization_follows_the_credentials_file() {
    let sources = registry(&Config::default(), &Credentials::default());
    let soulseek = sources.iter().find(|s| s.id() == SourceId::Soulseek).unwrap();
    let spotify = sources.iter().find(|s| s.id() == SourceId::Spotify).unwrap();
    let youtube = sources.iter().find(|s| s.id() == SourceId::Youtube).unwrap();
    assert_eq!(soulseek.auth_state(), AuthState::Missing);
    assert_eq!(spotify.auth_state(), AuthState::Missing);
    assert_eq!(youtube.auth_state(), AuthState::NotNeeded);

    let mut creds = Credentials::default();
    creds.soulseek = Some(Default::default());
    creds.spotify = Some(Default::default());
    let sources = registry(&Config::default(), &creds);
    assert!(sources.iter().all(|s| s.auth_state() != AuthState::Missing));
}

#[test]
fn capabilities_describe_each_source() {
    let sources = registry(&Config::default(), &Credentials::default());
    let caps = |id: SourceId| sources.iter().find(|s| s.id() == id).unwrap().capabilities();
    assert_eq!(caps(SourceId::Soulseek), maxfetch::sources::Capabilities { audio: true, metadata_only: false, lossless: true, needs_auth: true });
    assert_eq!(caps(SourceId::Youtube), maxfetch::sources::Capabilities { audio: true, metadata_only: false, lossless: false, needs_auth: false });
    assert_eq!(caps(SourceId::Spotify), maxfetch::sources::Capabilities { audio: false, metadata_only: true, lossless: false, needs_auth: true });
}

#[test]
fn a_missing_yt_dlp_is_reported_in_the_source_detail() {
    let mut cfg = Config::default();
    cfg.youtube.yt_dlp_path = "/nonexistent/yt-dlp".into();
    let statuses = source_checks(&registry(&cfg, &Credentials::default()));
    let youtube = statuses.iter().find(|s| s.id == SourceId::Youtube).unwrap();
    assert!(youtube.detail.as_deref().unwrap().contains("yt-dlp"));
    assert_eq!(youtube.auth, AuthState::NotNeeded);
}

#[test]
fn unauthorized_sources_get_a_skip_reason() {
    let sources = registry(&Config::default(), &Credentials::default());
    let soulseek = sources.iter().find(|s| s.id() == SourceId::Soulseek).unwrap();
    let youtube = sources.iter().find(|s| s.id() == SourceId::Youtube).unwrap();
    let reason = skip_reason(soulseek.as_ref()).unwrap();
    assert!(reason.contains("нет авторизации"));
    assert!(skip_reason(youtube.as_ref()).is_none());
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test sources_registry`
Expected: FAIL: `the_registry_holds_the_three_stage_one_sources` sees an empty vector.

- [ ] **Step 3: Implement the registry, the checks and the skip rule**

`registry` replaces the empty `Vec` and the marker comment of core Task 10 with the three pushes described in the Interfaces block, in a fixed order so the TUI and the CLI render a stable list. `enabled_in` is the one filter both `Pipeline::search` and the sources screen call: it keeps the input order, treats an empty `config.sources.enabled` as "all enabled", and compares the configured names against `SourceId::as_str()`; an unknown name logs `tracing::warn!` and is ignored rather than silently disabling everything. `source_checks` builds `SourceStatus { id, capabilities, auth, detail }` per source; the detail for a `Missing` Soulseek is `"нет данных для входа в Soulseek: секция [soulseek] в credentials.toml"`, for a `Missing` Spotify `"нет client_id/client_secret Spotify: секция [spotify] в credentials.toml"`, and the `youtube::binary_status` message is appended when `yt-dlp` is absent even though `auth_state` is `NotNeeded`. `Capabilities` and `SourceStatus` derive `PartialEq` (add the derive to the core definitions if it is missing; that is the only change this stage makes to them).

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test --test sources_registry`
Expected: PASS, 6 tests.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit --author="m1kiru <m1kiru@github.com>" -m "feat(sources): register soulseek, youtube and spotify"
```

---

### Task 8: CLI and TUI wiring, pipeline skip and the sidecar hop

**Files:**
- Modify: `src/auth.rs`
- Modify: `src/cli.rs`
- Modify: `src/pipeline.rs`
- Modify: `src/tui/views/sources.rs`, `src/tui/views/candidates.rs`, `src/tui/views/downloads.rs`
- Test: `tests/stage1_wiring.rs`

**Interfaces:**
- Consumes: `maxfetch::auth::auth_status` (core Task 15), `maxfetch::sources::{enabled_in, registry, source_checks, skip_reason, SourceStatus}` (Task 7), `maxfetch::youtube::{move_sidecar, untagged}` (Task 4), `maxfetch::sources::soulseek::{describe_peer, PeerCandidate}` (Task 1), `maxfetch::pipeline::Pipeline` (core Task 11), `maxfetch::cli::{Cli, run_with}` (core Task 13), `maxfetch::tui::App` (core Task 14).
- Produces:
  - `maxfetch::auth::describe_status(status: &SourceStatus) -> String` — `"<id>: доступен"` for `Ok`/`NotNeeded`, `"<id>: нет авторизации"` for `Missing`, `"<id>: ключ протух"` for `Expired`, with the status detail appended after `" — "` when it is present.
  - `maxfetch::cli::playlist_query(url: &str) -> Query` — `Query { text: None, url: Some(url), items: vec![] }`, used by `fetch --playlist`.
  - `Pipeline::search` (modified) — calls `sources::skip_reason` before each source, logs the reason with `tracing::warn!` and skips that source instead of querying it.
  - `Pipeline::download` (modified) — for `SourceId::Youtube` candidates, after moving the file into the library, calls `maxfetch::youtube::move_sidecar(&temporary, &final_path)` and logs the relocation.
  - `maxfetch::tui::App::source_rows(&self) -> Vec<String>` — one `auth::describe_status` line per entry of `self.status`.
  - `maxfetch::tui::App::candidate_badge(&self, candidate: &Candidate, downloaded: Option<&Path>) -> Option<&'static str>` — `Some("UNTAGGED")` when `downloaded` is `Some(path)` and `maxfetch::youtube::untagged(path)` holds, `Some("LOSSY")` for any other candidate whose `quality.lossless` is false, `None` for lossless.
  - `maxfetch::tui::transfer_line(peer: &PeerCandidate, title: &str) -> String` — the one line the Downloads screen prints per active transfer: `"<title>: пир <username>, слотов <slots>, скорость <speed>"`. When `peer.slots == 0` it returns `"<title>: пир <username> недоступен (0 слотов)"` instead, so a transfer whose peer has no free slots is visibly unusable even though it is still running. Lives in `src/tui/views/downloads.rs` and is re-exported from `maxfetch::tui`; it takes the same `PeerCandidate` the ranked loop chose, so no second source of truth for slots or speed.

- [ ] **Step 1: Write the failing test**

Create `tests/stage1_wiring.rs`:
```rust
use clap::Parser;
use maxfetch::auth::{describe_status, login};
use maxfetch::cli::{playlist_query, Cli};
use maxfetch::config::{Config, Credentials};
use maxfetch::model::SourceId;
use maxfetch::sources::{registry, source_checks, AuthState, Capabilities};

#[test]
fn source_status_lines_are_russian() {
    let mut cfg = Config::default();
    cfg.youtube.yt_dlp_path = "/nonexistent/yt-dlp".into();
    let statuses = source_checks(&registry(&cfg, &Credentials::default()));
    let soulseek = statuses.iter().find(|s| s.id == SourceId::Soulseek).unwrap();
    let youtube = statuses.iter().find(|s| s.id == SourceId::Youtube).unwrap();
    assert_eq!(describe_status(soulseek), "soulseek: нет авторизации — нет данных для входа в Soulseek: секция [soulseek] в credentials.toml");
    assert!(describe_status(youtube).starts_with("youtube: доступен — yt-dlp не найден"));
    let ok = maxfetch::sources::SourceStatus {
        id: SourceId::Soulseek,
        capabilities: Capabilities { audio: true, metadata_only: false, lossless: true, needs_auth: true },
        auth: AuthState::Ok,
        detail: None,
    };
    assert_eq!(describe_status(&ok), "soulseek: доступен");
}

#[test]
fn a_playlist_argument_becomes_a_url_query() {
    let q = playlist_query("https://www.youtube.com/playlist?list=PL1");
    assert_eq!(q.url.as_deref(), Some("https://www.youtube.com/playlist?list=PL1"));
    assert!(q.items.is_empty());
    assert!(q.text.is_none());
}

#[tokio::test]
async fn sources_check_lists_the_three_sources_in_russian() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::load_from(dir.path()).unwrap();
    let creds = Credentials::default();
    let cli = Cli::parse_from(["maxfetch", "sources", "check"]);
    let mut out = Vec::new();
    maxfetch::cli::run_with(cli, cfg, creds, &mut out).await.unwrap();
    let text = String::from_utf8_lossy(&out);
    assert!(text.contains("soulseek: нет авторизации"));
    assert!(text.contains("spotify: нет авторизации"));
    assert!(text.contains("youtube:"));
}

#[tokio::test]
async fn auth_login_stays_a_stub_for_the_stage_one_sources() {
    let mut creds = Credentials::default();
    for source in ["soulseek", "youtube", "spotify"] {
        let err = login(source, &mut creds).await.unwrap_err();
        assert!(format!("{err}").contains("not implemented"), "{source}");
    }
}

#[test]
fn the_sources_screen_starts_with_no_status_lines() {
    let app = maxfetch::tui::App::new(Config::default());
    assert!(app.source_rows().is_empty());
}

#[test]
fn an_active_transfer_names_its_peer_slots_and_speed() {
    let free = maxfetch::sources::soulseek::PeerCandidate {
        username: "alice".into(), filename: "f.flac".into(), size: 1, slots: 2, speed: 4,
        quality: maxfetch::model::Quality { lossless: true, container: maxfetch::model::Container::Flac, bit_depth: Some(16), sample_rate: Some(44100), bitrate_kbps: None },
    };
    assert_eq!(maxfetch::tui::transfer_line(&free, "Get Lucky"), "Get Lucky: пир alice, слотов 2, скорость 4");

    let busy = maxfetch::sources::soulseek::PeerCandidate { slots: 0, ..free };
    let line = maxfetch::tui::transfer_line(&busy, "Get Lucky");
    assert!(line.contains("недоступен"), "{line}");
    assert!(line.contains("alice"), "{line}");
}

#[test]
fn an_untagged_download_gets_its_own_badge() {
    let dir = tempfile::tempdir().unwrap();
    let webm = dir.path().join("clip.webm");
    std::fs::write(&webm, b"x").unwrap();
    let app = maxfetch::tui::App::new(Config::default());
    let lossy = maxfetch::model::Candidate {
        source: SourceId::Youtube,
        release: maxfetch::model::Release {
            title: "Get Lucky".into(),
            artist: "Daft Punk".into(),
            album: None,
            track_no: None,
            duration_sec: Some(369.0),
            isrc: None,
            year: None,
            source: SourceId::Youtube,
            source_item_id: "abc123".into(),
        },
        quality: maxfetch::model::Quality { lossless: false, container: maxfetch::model::Container::Opus, bit_depth: None, sample_rate: Some(48000), bitrate_kbps: Some(160) },
        size_bytes: Some(1),
        locator: maxfetch::model::Locator::Youtube { url: "https://www.youtube.com/watch?v=abc123".into(), video_id: "abc123".into() },
    };
    assert_eq!(app.candidate_badge(&lossy, Some(&webm)), Some("LOSSY"));
    std::fs::write(dir.path().join("clip.webm.json"), b"{}").unwrap();
    assert_eq!(app.candidate_badge(&lossy, Some(&webm)), Some("UNTAGGED"));
    assert_eq!(app.candidate_badge(&lossy, None), Some("LOSSY"));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test stage1_wiring`
Expected: FAIL with "unresolved import `maxfetch::auth::describe_status`".

- [ ] **Step 3: Implement the wiring**

`describe_status` formats exactly as above; `Missing` and `Expired` carry the core's Russian wording so the Sources screen and `sources check` share one string. `cli::run_with` gains the three stage-1 behaviours: `SourcesCommand::Check` prints `describe_status` for every `source_checks` entry, `AuthCommand::Status` prints the same lines plus the note `"вход для soulseek, youtube и spotify появится в следующем этапе"`, and `Command::Fetch { playlist: Some(url), .. }` sends `playlist_query(url)` through the pipeline. `AuthCommand::Login` keeps calling `auth::login`, which is untouched and still returns the core build's "not implemented" error.

`Pipeline::search` gains the skip gate: it first narrows the registry through `sources::enabled_in(&ids, &config)`, then for each source `match sources::skip_reason(source.as_ref()) { Some(reason) => { tracing::warn!(%reason, "source skipped"); continue }, None => {} }` before spawning the query; a source disabled in the config is never queried and its skip reason is logged at `debug` instead. `Pipeline::download` keeps its core order (download, probe, reject, tag, move, record) and inserts the sidecar hop for `SourceId::Youtube` between the move and the database insert.

The TUI: the Sources screen renders `app.source_rows()` from `Pipeline::source_status()` and binds `c` to a re-check; the Candidates screen renders `candidate_badge` in the quality column; the Downloads screen prints `tui::transfer_line(&peer, &title)` for every active transfer, taking the `PeerCandidate` that the ranked loop selected so the peer name, the free slots and the speed come from the same struct, and showing the "недоступен" variant as soon as the peer's slots reach zero. The Settings screen has no Soulseek sharing section, matching the download-only constraint.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test --test stage1_wiring`
Expected: PASS, 7 tests.

- [ ] **Step 5: Run the whole suite**

Run: `cargo test`
Expected: every core test and every stage-1 test passes; nothing requires the network.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit --author="m1kiru <m1kiru@github.com>" -m "feat(stage1): wire soulseek, youtube and spotify into cli, tui and pipeline"
```
