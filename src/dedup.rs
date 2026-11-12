//! Deduplication: the only place where `tracks` gains new rows.
//!
//! The stage contract has two halves:
//!
//! * [`register`] is the single writer. It looks for an existing track by ISRC,
//!   then by `match_key`, then by artist+title within the configured duration
//!   tolerance, compares the measured quality and marks the loser with
//!   `duplicate_of`. Files are never read, renamed or deleted here.
//! * [`plan_deletion`] is read-only and [`apply_deletion`] is the only function
//!   in the crate that removes files. It is reached solely through the explicit
//!   `library dedupe --apply --delete --yes` command.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::config::Config;
use crate::db::{Db, TrackRecord};
use crate::matching::{artist_title_key, match_key};
use crate::model::{Candidate, Container, Locator, Quality, Release, SourceId};
use crate::probe;
use crate::quality::compare_quality;
use crate::tags;

/// What [`register`] did with a newly downloaded file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DedupOutcome {
    /// Nothing collided; the file is the only copy of its track.
    Inserted { id: i64 },
    /// A collision was found; `winner_id` and `loser_id` reflect the final
    /// distribution, so the new row can be either the winner or the loser.
    Duplicate { winner_id: i64, loser_id: i64 },
}

/// One file that [`apply_deletion`] may remove.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeletionPlan {
    /// Row id of the losing track.
    pub id: i64,
    /// File belonging to the losing track.
    pub path: String,
    /// File that won and therefore stays.
    pub winner_path: String,
    /// Short English diagnostic, printed by the CLI layer.
    pub reason: String,
}

/// Register a downloaded file, resolving duplicates inside one transaction.
///
/// `probe_quality` is the *measured* quality of the file on disk; the claim of
/// the source is never used. A missing file at `path` is tolerated (the tests
/// register paths that do not exist), in which case `size_bytes` stays `None`.
pub fn register(
    config: &Config,
    db: &Db,
    candidate: &Candidate,
    path: &Path,
    probe_quality: Quality,
    duration_sec: Option<f64>,
) -> Result<DedupOutcome> {
    let tx = db.transaction()?;
    let record = record_from(candidate, path, &probe_quality, duration_sec);
    let target = find_target(config, db, &tx, &record)?;

    let Some(target) = target else {
        let id = db.insert_track(&record)?;
        db.log_download(candidate.source, Some(id), "acquired", None)?;
        tx.commit()?;
        return Ok(DedupOutcome::Inserted { id });
    };

    let existing = as_candidate(target.quality());
    // `Greater` means the new file is better than the stored one.
    let order = compare_quality(candidate, &existing);

    let new_id = db.insert_track(&record)?;
    db.log_download(candidate.source, Some(new_id), "acquired", None)?;

    let outcome = match order {
        // At equal quality the newer file wins (spec §7.6).
        Ordering::Greater | Ordering::Equal => {
            db.set_duplicate_of(target.id, new_id)?;
            DedupOutcome::Duplicate {
                winner_id: new_id,
                loser_id: target.id,
            }
        }
        Ordering::Less => {
            db.set_duplicate_of(new_id, target.id)?;
            DedupOutcome::Duplicate {
                winner_id: target.id,
                loser_id: new_id,
            }
        }
    };
    tx.commit()?;
    Ok(outcome)
}

/// Index audio files under `root` that the database does not know yet.
///
/// Only inserts rows: no deduplication, no `duplicate_of`, no file moves. The
/// number of inserted rows is returned.
pub fn scan_library(db: &Db, root: &Path) -> Result<usize> {
    let known: HashSet<String> = db.all_tracks()?.into_iter().map(|row| row.path).collect();
    let mut inserted = 0;
    let mut pending: Vec<PathBuf> = vec![root.to_path_buf()];

    while let Some(dir) = pending.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) if dir == root => {
                return Err(err)
                    .with_context(|| format!("scanning library root {}", dir.display()));
            }
            Err(err) => {
                // A subdirectory that vanished or became unreadable must not
                // abort the whole pass; only the root is fatal.
                tracing::warn!(
                    path = %dir.display(),
                    error = %err,
                    "skipping unreadable library directory"
                );
                continue;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(err) => {
                    tracing::warn!(
                        path = %dir.display(),
                        error = %err,
                        "skipping unreadable library entry"
                    );
                    continue;
                }
            };
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(err) => {
                    tracing::warn!(
                        path = %entry.path().display(),
                        error = %err,
                        "skipping entry with unknown file type"
                    );
                    continue;
                }
            };
            // Symlinks are skipped as both files and directories: following them
            // can loop forever or escape the library entirely.
            if file_type.is_symlink() {
                continue;
            }
            let path = entry.path();
            if file_type.is_dir() {
                if entry.file_name() == ".tmp" {
                    continue;
                }
                pending.push(path);
                continue;
            }
            if !file_type.is_file() || !is_audio(&path) {
                continue;
            }
            let path_str = path.to_string_lossy().into_owned();
            if known.contains(&path_str) {
                continue;
            }
            let meta = match tags::read_tags(&path) {
                Ok(meta) => meta,
                Err(err) => {
                    // One broken file must never abort the whole indexing pass.
                    tracing::warn!(path = %path.display(), error = %err, "skipping unreadable tags");
                    continue;
                }
            };
            let metadata = std::fs::metadata(&path).ok();
            let probed = probe::probe(&path).ok();
            let (measured, duration_sec) = match &probed {
                Some(result) => (
                    Quality {
                        lossless: result.container.is_lossless(),
                        container: result.container,
                        bit_depth: result.bit_depth,
                        sample_rate: result.sample_rate,
                        bitrate_kbps: result.bitrate_kbps,
                    },
                    result.duration_sec,
                ),
                // Without a probe the guess from the tags is all there is.
                None => (meta.quality.clone(), None),
            };
            let release = Release {
                title: meta.title.clone(),
                artist: meta.artist.clone(),
                album: meta.album.clone(),
                track_no: meta.track_no,
                duration_sec,
                isrc: meta.isrc.clone(),
                year: meta.year,
                source: meta.source,
                source_item_id: String::new(),
            };
            let record = TrackRecord {
                id: 0,
                isrc: normalize_isrc(meta.isrc.as_deref()),
                match_key: match_key(&release),
                title: meta.title,
                artist: meta.artist,
                album: meta.album,
                track_no: meta.track_no,
                duration_sec,
                lossless: measured.lossless,
                container: measured.container,
                bit_depth: measured.bit_depth,
                sample_rate: measured.sample_rate,
                bitrate_kbps: measured.bitrate_kbps,
                size_bytes: metadata.as_ref().map(|m| m.len()),
                path: path_str,
                source: meta.source,
                acquired_at: acquired_at(metadata.as_ref()),
                duplicate_of: None,
            };
            db.insert_track(&record)?;
            inserted += 1;
        }
    }
    Ok(inserted)
}

/// Every file that the explicit deletion command may remove.
///
/// Read-only and deterministic. A group is skipped as a whole when quality is
/// ambiguous: `duplicate_groups` reports `conflict` when any loser is
/// indistinguishable by `same_quality`, and an extra `compare_quality` check
/// covers the gap where `compare_quality` calls a lossless pair `Equal` while
/// `same_quality` does not (it ignores `bitrate_kbps` for lossless).
pub fn plan_deletion(db: &Db) -> Result<Vec<DeletionPlan>> {
    let mut plans = Vec::new();
    for group in db.duplicate_groups()? {
        if group.conflict || !group.indistinguishable_losers().is_empty() {
            continue;
        }
        let winner = as_candidate(group.winner.quality());
        let any_equal = group.losers.iter().any(|loser| {
            compare_quality(&as_candidate(loser.quality()), &winner) == Ordering::Equal
        });
        if any_equal {
            continue;
        }
        for loser in &group.losers {
            plans.push(DeletionPlan {
                id: loser.id,
                path: loser.path.clone(),
                winner_path: group.winner.path.clone(),
                reason: "lower quality than winner".to_string(),
            });
        }
    }
    Ok(plans)
}

/// Remove the files of `plans` and their rows. The only file deletion in the crate.
///
/// The file goes first and the row second, so a failure leaves the database
/// pointing at a file that still exists. Every plan is its own transaction, so
/// one failure does not roll back deletions that already succeeded. Repeating
/// the call is safe: an already missing file is not an error and is not counted.
pub fn apply_deletion(db: &Db, plans: &[DeletionPlan]) -> Result<usize> {
    let mut removed = 0;
    for plan in plans {
        match std::fs::remove_file(&plan.path) {
            Ok(()) => removed += 1,
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => {
                return Err(err).with_context(|| format!("deleting {}", plan.path));
            }
        }
        let tx = db.transaction()?;
        db.delete_track(plan.id)?;
        tx.commit()?;
    }
    Ok(removed)
}

/// The best existing track the new file collides with, if any.
fn find_target(
    config: &Config,
    db: &Db,
    tx: &Connection,
    record: &TrackRecord,
) -> Result<Option<TrackRecord>> {
    let mut found: Vec<TrackRecord> = Vec::new();
    if let Some(isrc) = record.isrc.as_deref() {
        found = db.find_by_isrc(isrc)?;
    }
    if found.is_empty() {
        found = db.find_by_match_key(&record.match_key)?;
    }
    if found.is_empty() && record.isrc.is_none() {
        // There is no index for artist+title, so this rare fallback walks the
        // whole library. It only applies when neither side has an ISRC.
        let key = artist_title_key(&release_of(record));
        let tolerance = config.matching.duration_tolerance_sec as f64;
        for row in db.all_tracks()? {
            if row.isrc.is_some() || artist_title_key(&release_of(&row)) != key {
                continue;
            }
            if let (Some(existing), Some(new)) = (row.duration_sec, record.duration_sec)
                && (existing - new).abs() <= tolerance
            {
                found.push(row);
            }
        }
    }
    if found.is_empty() {
        return Ok(None);
    }

    // Lookups return losers as well, so every row is first replaced by its
    // canonical winner. The walk follows the whole `duplicate_of` chain: a
    // single step would stop on an intermediate loser of a chain A<-B<-C,
    // dethrone C and leave two canonical rows behind.
    let mut canonical: Vec<TrackRecord> = Vec::new();
    for row in found {
        let Some(row) = canonical_track(tx, row)? else {
            // A dangling reference or a cycle has no canonical row at all.
            continue;
        };
        if !canonical.iter().any(|seen| seen.id == row.id) {
            canonical.push(row);
        }
    }
    Ok(canonical
        .into_iter()
        .max_by(|a, b| compare_quality(&as_candidate(a.quality()), &as_candidate(b.quality()))))
}

fn record_from(
    candidate: &Candidate,
    path: &Path,
    probe_quality: &Quality,
    duration_sec: Option<f64>,
) -> TrackRecord {
    let release = &candidate.release;
    TrackRecord {
        id: 0,
        isrc: normalize_isrc(release.isrc.as_deref()),
        match_key: match_key(release),
        title: release.title.clone(),
        artist: release.artist.clone(),
        album: release.album.clone(),
        track_no: release.track_no,
        duration_sec: duration_sec.or(release.duration_sec),
        lossless: probe_quality.lossless,
        container: probe_quality.container,
        bit_depth: probe_quality.bit_depth,
        sample_rate: probe_quality.sample_rate,
        bitrate_kbps: probe_quality.bitrate_kbps,
        size_bytes: std::fs::metadata(path).ok().map(|m| m.len()),
        path: path.to_string_lossy().into_owned(),
        source: candidate.source,
        acquired_at: Utc::now().to_rfc3339(),
        duplicate_of: None,
    }
}

/// A blank release wrapper: `compare_quality` reads only `.quality`, but the
/// signature needs a whole [`Candidate`]. Placeholders never leave this module.
fn as_candidate(quality: Quality) -> Candidate {
    Candidate {
        source: SourceId::Soulseek,
        release: Release {
            title: String::new(),
            artist: String::new(),
            album: None,
            track_no: None,
            duration_sec: None,
            isrc: None,
            year: None,
            source: SourceId::Soulseek,
            source_item_id: String::new(),
        },
        quality,
        size_bytes: None,
        locator: Locator::Soulseek {
            username: String::new(),
            filename: String::new(),
            size: 0,
        },
    }
}

/// A [`Release`] reconstructed from a stored row, for `match_key`/`artist_title_key`.
fn release_of(record: &TrackRecord) -> Release {
    Release {
        title: record.title.clone(),
        artist: record.artist.clone(),
        album: record.album.clone(),
        track_no: record.track_no,
        duration_sec: record.duration_sec,
        isrc: record.isrc.clone(),
        year: None,
        source: record.source,
        source_item_id: String::new(),
    }
}

fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .and_then(Container::from_ext)
        .is_some()
}

/// An ISRC is stored only when it is non-empty after trimming.
fn normalize_isrc(isrc: Option<&str>) -> Option<String> {
    isrc.map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn acquired_at(metadata: Option<&std::fs::Metadata>) -> String {
    metadata
        .and_then(|meta| meta.modified().ok())
        .map(|modified| DateTime::<Utc>::from(modified).to_rfc3339())
        .unwrap_or_else(|| Utc::now().to_rfc3339())
}

const TRACK_COLUMNS: &str = "id, isrc, match_key, title, artist, album, track_no, \
    duration_sec, lossless, container, bit_depth, sample_rate, bitrate_kbps, \
    size_bytes, path, source_id, acquired_at, duplicate_of";

/// Follow a row's `duplicate_of` chain to the canonical row it belongs to.
///
/// Returns `None` when the chain leaves the table or loops back on itself: such
/// a chain has no canonical row, so it cannot be a dedup target.
fn canonical_track(conn: &Connection, row: TrackRecord) -> Result<Option<TrackRecord>> {
    let mut current = row;
    let mut seen: HashSet<i64> = HashSet::new();
    seen.insert(current.id);
    while let Some(winner_id) = current.duplicate_of {
        if !seen.insert(winner_id) {
            tracing::warn!(
                track_id = winner_id,
                "duplicate_of chain loops, dropping it"
            );
            return Ok(None);
        }
        match track_by_id(conn, winner_id)? {
            Some(winner) => current = winner,
            None => return Ok(None),
        }
    }
    Ok(Some(current))
}

/// Read one row by id. `Db::track_by_id` is private, and `db.rs` is not this
/// module's to change, so the documented `Connection` access is used instead.
fn track_by_id(conn: &Connection, id: i64) -> Result<Option<TrackRecord>> {
    conn.query_row(
        &format!("SELECT {TRACK_COLUMNS} FROM tracks WHERE id = ?1"),
        params![id],
        track_from_row,
    )
    .optional()
    .context("loading track by id")
}

fn track_from_row(row: &Row<'_>) -> rusqlite::Result<TrackRecord> {
    let container_str: String = row.get("container")?;
    let container = Container::from_ext(&container_str).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            format!("unknown container {container_str:?}").into(),
        )
    })?;
    let source_str: String = row.get("source_id")?;
    let source = SourceId::from_str(&source_str).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            format!("unknown source {source_str:?}").into(),
        )
    })?;
    Ok(TrackRecord {
        id: row.get("id")?,
        isrc: row.get("isrc")?,
        match_key: row.get("match_key")?,
        title: row.get("title")?,
        artist: row.get("artist")?,
        album: row.get("album")?,
        track_no: row.get("track_no")?,
        duration_sec: row.get("duration_sec")?,
        lossless: row.get::<_, i64>("lossless")? != 0,
        container,
        bit_depth: row.get("bit_depth")?,
        sample_rate: row.get("sample_rate")?,
        bitrate_kbps: row.get("bitrate_kbps")?,
        size_bytes: row.get::<_, Option<i64>>("size_bytes")?.map(|v| v as u64),
        path: row.get("path")?,
        source,
        acquired_at: row.get("acquired_at")?,
        duplicate_of: row.get("duplicate_of")?,
    })
}
