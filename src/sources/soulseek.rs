//! Soulseek peer model, attribute parsing, ranking and blocking client.
//!
//! The peer selection layer is pure: no network, no `soulseek_rs::Client`; it
//! only decides which peer to try and what the file claims to be. The client
//! layer sits behind [`Transfer`], so tests never open a socket.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

use anyhow::Context;
use soulseek_rs::{Client, ClientSettings, DownloadStatus};

use crate::config::{DownloadConfig, SoulseekCredentials};
use crate::fetch::ProgressSink;
use crate::model::{Candidate, Container, Locator, Quality, Release, SourceId};
use crate::sources::{AuthState, Capabilities, Query, Source};

/// Attribute code: bitrate in kbit/s, `0` for FLAC.
pub const ATTR_BITRATE: u32 = 0;
/// Attribute code: track length in seconds. Declared for completeness, unused.
pub const ATTR_DURATION: u32 = 1;
/// Attribute code: sample rate in Hz.
pub const ATTR_SAMPLE_RATE: u32 = 4;
/// Attribute code: bit depth.
pub const ATTR_BIT_DEPTH: u32 = 5;

/// One peer/file pair as returned by a search.
#[derive(Clone, Debug, PartialEq)]
pub struct PeerCandidate {
    pub username: String,
    pub filename: String,
    pub size: u64,
    pub slots: u8,
    pub speed: u32,
    pub quality: Quality,
}

/// Raw attribute lookup; `None` when the code is absent from the map.
pub fn attrib(attribs: &HashMap<u32, u32>, code: u32) -> Option<u32> {
    attribs.get(&code).copied()
}

/// `ext` is a lowercase extension without the dot.
pub fn lossless_extension(ext: &str) -> bool {
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "flac" | "wav" | "alac" | "aiff" | "ape" | "wv"
    )
}

/// Quality claimed by `name` plus `attribs`; `None` when the extension has no
/// `Container` mapping, i.e. the file cannot be laid out in the library.
pub fn quality_from_attributes(name: &str, attribs: &HashMap<u32, u32>) -> Option<Quality> {
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    let container = Container::from_ext(&ext)?;
    let lossless = container.is_lossless();
    let bitrate_kbps = if lossless {
        None
    } else {
        attrib(attribs, ATTR_BITRATE).filter(|kbps| *kbps > 0)
    };
    Some(Quality {
        lossless,
        container,
        bit_depth: attrib(attribs, ATTR_BIT_DEPTH).map(|depth| depth as u8),
        sample_rate: attrib(attribs, ATTR_SAMPLE_RATE),
        bitrate_kbps,
    })
}

/// One peer/file pair; `None` when [`quality_from_attributes`] yields `None`.
pub fn peer_from_file(
    username: &str,
    slots: u8,
    speed: u32,
    filename: &str,
    size: u64,
    attribs: &HashMap<u32, u32>,
) -> Option<PeerCandidate> {
    let quality = quality_from_attributes(filename, attribs)?;
    Some(PeerCandidate {
        username: username.into(),
        filename: filename.into(),
        size,
        slots,
        speed,
        quality,
    })
}

/// Drops `slots == 0`, then sorts best first: quality, slots, speed, size.
pub fn rank_peers(candidates: &[PeerCandidate]) -> Vec<PeerCandidate> {
    let mut kept: Vec<PeerCandidate> = candidates
        .iter()
        .filter(|candidate| candidate.slots > 0)
        .cloned()
        .collect();
    kept.sort_by(|a, b| {
        cmp_quality(&b.quality, &a.quality)
            .then_with(|| b.slots.cmp(&a.slots))
            .then_with(|| b.speed.cmp(&a.speed))
            .then_with(|| b.size.cmp(&a.size))
    });
    kept
}

/// Builds the pipeline candidate with [`Locator::Soulseek`].
pub fn into_candidate(peer: &PeerCandidate, release: &Release) -> Candidate {
    Candidate {
        source: SourceId::Soulseek,
        release: release.clone(),
        quality: peer.quality.clone(),
        size_bytes: Some(peer.size),
        locator: Locator::Soulseek {
            username: peer.username.clone(),
            filename: peer.filename.clone(),
            size: peer.size,
        },
    }
}

/// `"пир <username>: скорость <speed>, слотов <slots>"` (user-facing, RU).
pub fn describe_peer(peer: &PeerCandidate) -> String {
    format!(
        "пир {}: скорость {}, слотов {}",
        peer.username, peer.speed, peer.slots
    )
}

/// Mirrors `quality::compare_quality` for a bare [`Quality`]; `Greater` = better.
fn cmp_quality(a: &Quality, b: &Quality) -> Ordering {
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
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => Ordering::Greater,
        (None, Some(_)) => Ordering::Less,
        (None, None) => Ordering::Equal,
    }
}

/// Drains the crate's status channel until a terminal status arrives.
///
/// Blocking: `recv_timeout(stall_timeout)` is the whole stall guard, so a
/// peer that simply stops sending samples is detected here and not by the
/// caller. Progress samples are forwarded to `sink`.
///
/// Known limitation: `soulseek-rs-lib` 20.0.0 never publishes `Queued` or
/// `Paused` on a transfer's own status channel, so those arms only keep the
/// match exhaustive. A legitimate queue wait longer than `stall_timeout`
/// therefore looks like `transfer stalled`, and the caller fails over.
pub fn drain_transfer(
    events: Receiver<DownloadStatus>,
    stall_timeout: Duration,
    sink: &dyn ProgressSink,
) -> anyhow::Result<u64> {
    let mut total: u64 = 0;
    loop {
        match events.recv_timeout(stall_timeout) {
            Ok(DownloadStatus::InProgress {
                bytes_downloaded,
                total_bytes,
                speed_bytes_per_sec,
            }) => {
                sink.on_progress(bytes_downloaded, Some(total_bytes));
                total = total_bytes;
                tracing::trace!(speed = speed_bytes_per_sec, "soulseek transfer progress");
            }
            Ok(DownloadStatus::Queued | DownloadStatus::Paused { .. }) => {}
            Ok(DownloadStatus::Completed) => {
                sink.on_finished(total);
                return Ok(total);
            }
            Ok(DownloadStatus::Failed(Some(message))) => anyhow::bail!("{message}"),
            Ok(DownloadStatus::Failed(None)) => anyhow::bail!("transfer failed"),
            Ok(DownloadStatus::Cancelled) => anyhow::bail!("transfer cancelled"),
            Ok(DownloadStatus::TimedOut) => anyhow::bail!("transfer timed out"),
            Err(RecvTimeoutError::Timeout) => {
                anyhow::bail!("transfer stalled after {}s", stall_timeout.as_secs())
            }
            Err(RecvTimeoutError::Disconnected) => anyhow::bail!("peer disconnected"),
        }
    }
}

/// One upload slot on one peer; the only seam that touches the network.
pub trait Transfer: Send + Sync {
    /// Writes the complete file at `dest` and returns the byte count.
    ///
    /// Implementations must delete anything already at `dest` before they
    /// start. The crate can resume a `.part` file, but here a leftover file
    /// is never a valid resume point: [`download_ranked`] renames its own
    /// `.part` into place, so whatever sits at `dest` came from a different
    /// peer, a different size or a failed attempt. An interrupted transfer
    /// always restarts from zero.
    fn fetch(
        &self,
        peer: &PeerCandidate,
        dest: &Path,
        sink: &dyn ProgressSink,
    ) -> anyhow::Result<u64>;
}

/// Tries every ranked peer in order and returns the one that delivered.
pub fn download_ranked(
    transfer: &dyn Transfer,
    ranked: &[PeerCandidate],
    dest: &Path,
    sink: &dyn ProgressSink,
) -> anyhow::Result<PeerCandidate> {
    if ranked.is_empty() {
        anyhow::bail!("нет кандидатов со свободными слотами");
    }

    let part = dest.with_extension("part");
    let mut last_err: Option<(String, anyhow::Error)> = None;

    for peer in ranked {
        // The crate streams into `dir/<basename>.part` and renames it to
        // `dir/<basename>` only on a complete transfer, and it deliberately
        // keeps the `.part` when a transfer breaks off. Both must be gone
        // before the attempt: the crate resumes whatever `.part` it finds,
        // whoever wrote it, so bytes of another peer or another size would be
        // appended to instead of restarting from zero.
        remove_stale(&remote_path(&part, &peer.filename));
        match transfer.fetch(peer, &part, sink) {
            Ok(_bytes) => {
                let moved = std::fs::rename(&part, dest)
                    .with_context(|| format!("failed to move {} into place", part.display()));
                if let Err(err) = moved {
                    let _ = std::fs::remove_file(&part);
                    return Err(err);
                }
                return Ok(peer.clone());
            }
            Err(err) => {
                tracing::warn!(peer = %peer.username, error = %err, "soulseek peer failed");
                if part.exists() {
                    let _ = std::fs::remove_file(&part);
                }
                last_err = Some((peer.username.clone(), err));
            }
        }
    }

    match last_err {
        // The message stays a plain string so the peer's own text is visible
        // in `Display`; `anyhow::Context` would hide it behind the outer text.
        Some((username, err)) => Err(anyhow::anyhow!("peer {username} failed: {err}")),
        None => anyhow::bail!("нет кандидатов со свободными слотами"),
    }
}

/// `"Artist - Title.flac"` -> `(Some("Artist"), "Title")`;
/// `"Title.mp3"` -> `(None, "Title")`.
pub fn release_from_filename(filename: &str) -> (Option<String>, String) {
    let stem = filename.rsplit_once('.').map_or(filename, |(head, _)| head);
    match stem.split_once(" - ") {
        Some((artist, title)) => (Some(artist.trim().to_string()), title.trim().to_string()),
        None => (None, stem.to_string()),
    }
}

/// Last path component of a remote Soulseek path, both separators handled.
fn remote_basename(remote: &str) -> &str {
    remote.rsplit(['/', '\\']).next().unwrap_or(remote)
}

/// Where the crate itself writes the transfer for `filename`, given the path
/// handed to [`Transfer::fetch`]: the same directory, remote basename.
fn remote_path(dest: &Path, filename: &str) -> std::path::PathBuf {
    dest.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map_or_else(
            || std::path::PathBuf::from(remote_basename(filename)),
            |parent| parent.join(remote_basename(filename)),
        )
}

/// Deletes `path` and `path.part`, ignoring "not found" and other failures:
/// leftovers of a previous attempt are best-effort cleanup, not the goal.
fn remove_stale(path: &Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(format!("{}.part", path.display()));
}

/// Blocking real client. Constructed per `fetch` call, dropped after it.
struct LibSoulseekTransfer {
    credentials: SoulseekCredentials,
    download: DownloadConfig,
}

impl Transfer for LibSoulseekTransfer {
    fn fetch(
        &self,
        peer: &PeerCandidate,
        dest: &Path,
        sink: &dyn ProgressSink,
    ) -> anyhow::Result<u64> {
        // untested: requires network
        let _ = std::fs::remove_file(dest);

        let dir = dest
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map_or_else(
                || ".".to_string(),
                |parent| parent.to_string_lossy().into_owned(),
            );
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create download directory {dir}"))?;

        // The crate ignores `dest`: it streams into `dir/<basename>.part` and
        // renames that to `dir/<basename>` on success. It also resumes any
        // `.part` shorter than `peer.size`, so whatever an earlier attempt left
        // at either path has to go before `client.download` picks an offset --
        // otherwise bytes of a different peer or size are appended to.
        let written = Path::new(&dir).join(remote_basename(&peer.filename));
        remove_stale(&written);

        let settings = ClientSettings {
            username: self.credentials.username.clone(),
            password: self.credentials.password.clone(),
            enable_listen: false,
            ..ClientSettings::default()
        };
        let mut client = Client::with_settings(settings);
        client.connect().context("soulseek connect failed")?;
        if !client.login().context("soulseek login failed")? {
            anyhow::bail!("Soulseek отклонил логин");
        }

        // Space search and download so the server does not treat us as flooding.
        std::thread::sleep(Duration::from_millis(500));

        // `_download` is a snapshot of the client's download-store entry, not a
        // guard: there is no `Drop`-based cancellation in 20.0.0. A transfer
        // stops only through the store (`Client::cancel_download` ->
        // `cancel_by_file`), so this binding is kept for the token and status.
        let (_download, events) =
            client.download(peer.filename.clone(), peer.username.clone(), peer.size, dir)?;
        let bytes = drain_transfer(
            events,
            Duration::from_secs(self.download.stall_timeout_sec),
            sink,
        )?;

        if written.as_path() != dest {
            std::fs::rename(&written, dest)
                .with_context(|| format!("failed to move {} into place", written.display()))?;
        }
        Ok(bytes)
    }
}

/// Blocking search: connect, login, search, map files to peers.
fn search_peers(
    credentials: &SoulseekCredentials,
    download: &DownloadConfig,
    query: &str,
) -> anyhow::Result<Vec<PeerCandidate>> {
    // untested: requires network
    let settings = ClientSettings {
        username: credentials.username.clone(),
        password: credentials.password.clone(),
        enable_listen: false,
        ..ClientSettings::default()
    };
    let mut client = Client::with_settings(settings);
    client.connect().context("soulseek connect failed")?;
    if !client.login().context("soulseek login failed")? {
        anyhow::bail!("Soulseek отклонил логин");
    }

    let results = client.search(query, Duration::from_secs(download.timeout_sec))?;
    let mut peers = Vec::new();
    for result in &results {
        for file in &result.files {
            if let Some(peer) = peer_from_file(
                &result.username,
                result.slots,
                result.speed,
                &file.name,
                file.size,
                &file.attribs,
            ) {
                peers.push(peer);
            }
        }
    }
    Ok(peers)
}

/// Maps one search hit onto the pipeline's [`Release`].
fn release_from_peer(peer: &PeerCandidate) -> Release {
    let (artist, title) = release_from_filename(&peer.filename);
    Release {
        title,
        artist: artist.unwrap_or_default(),
        album: None,
        track_no: None,
        duration_sec: None,
        isrc: None,
        year: None,
        source: SourceId::Soulseek,
        source_item_id: format!("peer:{}:{}", peer.username, peer.filename),
    }
}

/// Soulseek as a [`Source`]: search on demand, download from ranked peers.
#[derive(Debug)]
pub struct SoulseekSource {
    credentials: SoulseekCredentials,
    download: DownloadConfig,
    peers: Mutex<Vec<PeerCandidate>>,
}

impl SoulseekSource {
    /// Never touches the network and never constructs a `Client`.
    pub fn new(credentials: SoulseekCredentials, download: DownloadConfig) -> SoulseekSource {
        SoulseekSource {
            credentials,
            download,
            peers: Mutex::new(Vec::new()),
        }
    }

    /// Snapshot of the peer set produced by the most recent search.
    pub fn peers(&self) -> Vec<PeerCandidate> {
        self.peers
            .lock()
            .map(|peers| peers.clone())
            .unwrap_or_default()
    }

    /// Replaces the snapshot; a poisoned mutex only loses the snapshot.
    fn store_peers(&self, peers: Vec<PeerCandidate>) {
        if let Ok(mut slot) = self.peers.lock() {
            *slot = peers;
        }
    }

    /// Blocking search behind [`tokio::task::block_in_place`].
    fn search_blocking(&self, query: &str) -> anyhow::Result<Vec<PeerCandidate>> {
        // untested: requires network
        let credentials = &self.credentials;
        let download = &self.download;
        let peers = tokio::task::block_in_place(|| search_peers(credentials, download, query))?;
        self.store_peers(peers.clone());
        Ok(peers)
    }
}

#[async_trait::async_trait]
impl Source for SoulseekSource {
    fn id(&self) -> SourceId {
        SourceId::Soulseek
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            audio: true,
            metadata_only: false,
            lossless: true,
            needs_auth: true,
        }
    }

    fn auth_state(&self) -> AuthState {
        if self.credentials.username.is_empty() || self.credentials.password.is_empty() {
            AuthState::Missing
        } else {
            AuthState::Ok
        }
    }

    async fn search(&self, q: &Query) -> anyhow::Result<Vec<Release>> {
        let query = q
            .text
            .as_deref()
            .or(q.url.as_deref())
            .unwrap_or_default()
            .trim();
        if query.is_empty() {
            anyhow::bail!("пустой поисковый запрос");
        }
        let peers = self.search_blocking(query)?;
        Ok(peers.iter().map(release_from_peer).collect())
    }

    async fn resolve(&self, q: &Query) -> anyhow::Result<Vec<Release>> {
        let query = q.url.as_deref().unwrap_or_default().trim();
        if query.is_empty() {
            anyhow::bail!("пустой поисковый запрос");
        }
        let peers = self.search_blocking(query)?;
        Ok(peers.iter().map(release_from_peer).collect())
    }

    async fn candidates(&self, release: &Release) -> anyhow::Result<Vec<Candidate>> {
        let query = format!("{} {}", release.artist, release.title);
        let peers = self.search_blocking(query.trim())?;
        Ok(rank_peers(&peers)
            .iter()
            .map(|peer| into_candidate(peer, release))
            .collect())
    }

    async fn download(
        &self,
        candidate: &Candidate,
        dest: &Path,
        sink: &dyn ProgressSink,
    ) -> anyhow::Result<()> {
        let Locator::Soulseek {
            username,
            filename,
            size,
        } = &candidate.locator
        else {
            anyhow::bail!("кандидат не принадлежит Soulseek");
        };

        let mut peers: Vec<PeerCandidate> = rank_peers(&self.peers())
            .into_iter()
            .filter(|peer| peer.filename == *filename)
            .collect();
        if peers.is_empty() {
            peers.push(PeerCandidate {
                username: username.clone(),
                filename: filename.clone(),
                size: *size,
                slots: 1,
                speed: 0,
                quality: candidate.quality.clone(),
            });
        }

        let transfer = LibSoulseekTransfer {
            credentials: self.credentials.clone(),
            download: self.download.clone(),
        };
        tokio::task::block_in_place(|| download_ranked(&transfer, &peers, dest, sink))?;
        Ok(())
    }
}
