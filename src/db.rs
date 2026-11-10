use std::path::Path;

use anyhow::{Context, Result};
use chrono::{Duration, Utc};
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::matching::match_key;
use crate::model::{Container, Release, SourceId};

/// A track row in the local library state.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackRecord {
    pub id: i64,
    pub isrc: Option<String>,
    pub match_key: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub track_no: Option<u32>,
    pub duration_sec: Option<f64>,
    pub lossless: bool,
    pub container: Container,
    pub bit_depth: Option<u8>,
    pub sample_rate: Option<u32>,
    pub bitrate_kbps: Option<u32>,
    pub size_bytes: Option<u64>,
    pub path: String,
    pub source: SourceId,
    pub acquired_at: String,
    pub duplicate_of: Option<i64>,
}

/// A group of duplicate tracks: one canonical winner and the tracks that lost to it.
#[derive(Clone, Debug, PartialEq)]
pub struct DuplicateGroup {
    pub winner: TrackRecord,
    pub losers: Vec<TrackRecord>,
    /// True when every loser's quality is indistinguishable from the winner's.
    pub conflict: bool,
}

/// SQLite-backed library state.
pub struct Db {
    conn: Connection,
}

const SCHEMA_VERSION: i64 = 1;

const TRACK_COLUMNS: &str = "id, isrc, match_key, title, artist, album, track_no, \
    duration_sec, lossless, container, bit_depth, sample_rate, bitrate_kbps, \
    size_bytes, path, source_id, acquired_at, duplicate_of";

impl Db {
    /// Open the database file, creating parent directories and running migrations.
    pub fn open(path: &Path) -> Result<Db> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating parent directories for {}", path.display()))?;
            }
        }
        let conn = Connection::open(path)
            .with_context(|| format!("opening database {}", path.display()))?;
        let db = Db { conn };
        db.prepare()?;
        db.migrate()?;
        Ok(db)
    }

    /// Open an in-memory database, used by tests.
    pub fn open_in_memory() -> Result<Db> {
        let conn = Connection::open_in_memory().context("opening in-memory database")?;
        let db = Db { conn };
        db.prepare()?;
        db.migrate()?;
        Ok(db)
    }

    fn prepare(&self) -> Result<()> {
        self.conn
            .pragma_update(None, "foreign_keys", true)
            .context("enabling foreign_keys")?;
        // `PRAGMA journal_mode` returns a row, so it cannot go through `pragma_update`.
        let _: String = self
            .conn
            .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
            .context("enabling WAL")?;
        Ok(())
    }

    /// Create the schema (or upgrade it in place) and record the schema version.
    pub fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS tracks (
               id            INTEGER PRIMARY KEY,
               isrc          TEXT,
               match_key     TEXT NOT NULL,
               title         TEXT NOT NULL,
               artist        TEXT NOT NULL,
               album         TEXT,
               track_no      INTEGER,
               duration_sec  REAL,
               lossless      INTEGER NOT NULL,
               container     TEXT NOT NULL,
               bit_depth     INTEGER,
               sample_rate   INTEGER,
               bitrate_kbps  INTEGER,
               size_bytes    INTEGER,
               path          TEXT NOT NULL UNIQUE,
               source_id     TEXT NOT NULL,
               acquired_at   TEXT NOT NULL,
               duplicate_of  INTEGER REFERENCES tracks(id)
             );
             CREATE INDEX IF NOT EXISTS idx_tracks_isrc     ON tracks(isrc);
             CREATE INDEX IF NOT EXISTS idx_tracks_matchkey ON tracks(match_key);
             CREATE INDEX IF NOT EXISTS idx_tracks_dup      ON tracks(duplicate_of);
             CREATE TABLE IF NOT EXISTS releases (
               key        TEXT PRIMARY KEY,
               match_key  TEXT NOT NULL,
               source_id  TEXT NOT NULL,
               payload    TEXT NOT NULL,
               fetched_at TEXT NOT NULL,
               expires_at TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS downloads (
               id         INTEGER PRIMARY KEY,
               track_id   INTEGER REFERENCES tracks(id),
               source_id  TEXT NOT NULL,
               state      TEXT NOT NULL,
               reason     TEXT,
               started_at TEXT,
               ended_at   TEXT
             );
             CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )
        .context("creating schema")?;

        // In-place upgrade: an older `releases` table has no `expires_at` column.
        if !self.releases_has_expires_at()? {
            self.conn
                .execute(
                    "ALTER TABLE releases ADD COLUMN expires_at TEXT NOT NULL DEFAULT ''",
                    [],
                )
                .context("adding releases.expires_at column")?;
        }

        let recorded: Option<i64> = self
            .conn
            .query_row("SELECT version FROM schema_version LIMIT 1", [], |row| row.get(0))
            .optional()
            .context("reading schema version")?;
        if recorded.is_none() {
            self.conn
                .execute("INSERT INTO schema_version (version) VALUES (?1)", params![SCHEMA_VERSION])
                .context("recording schema version")?;
        }
        Ok(())
    }

    fn releases_has_expires_at(&self) -> Result<bool> {
        let mut stmt = self
            .conn
            .prepare("PRAGMA table_info(releases)")
            .context("inspecting releases table")?;
        let mut rows = stmt.query([]).context("reading releases columns")?;
        while let Some(row) = rows.next().context("reading releases column row")? {
            let name: String = row.get("name").context("reading releases column name")?;
            if name == "expires_at" {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Insert a track, returning its new row id. Fails on a duplicate `path`.
    pub fn insert_track(&self, record: &TrackRecord) -> Result<i64> {
        self.conn
            .execute(
                "INSERT INTO tracks (isrc, match_key, title, artist, album, track_no, \
                 duration_sec, lossless, container, bit_depth, sample_rate, bitrate_kbps, \
                 size_bytes, path, source_id, acquired_at, duplicate_of) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
                params![
                    record.isrc,
                    record.match_key,
                    record.title,
                    record.artist,
                    record.album,
                    record.track_no,
                    record.duration_sec,
                    record.lossless,
                    record.container.as_str(),
                    record.bit_depth,
                    record.sample_rate,
                    record.bitrate_kbps,
                    record.size_bytes.map(|v| v as i64),
                    record.path,
                    record.source.as_str(),
                    record.acquired_at,
                    record.duplicate_of,
                ],
            )
            .map_err(anyhow::Error::from)?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn find_by_isrc(&self, isrc: &str) -> Result<Vec<TrackRecord>> {
        self.query_tracks(&format!("SELECT {TRACK_COLUMNS} FROM tracks WHERE isrc = ?1"), params![isrc])
    }

    pub fn find_by_match_key(&self, key: &str) -> Result<Vec<TrackRecord>> {
        self.query_tracks(
            &format!("SELECT {TRACK_COLUMNS} FROM tracks WHERE match_key = ?1"),
            params![key],
        )
    }

    pub fn all_tracks(&self) -> Result<Vec<TrackRecord>> {
        self.query_tracks(&format!("SELECT {TRACK_COLUMNS} FROM tracks"), [])
    }

    fn query_tracks(&self, sql: &str, params: impl rusqlite::Params) -> Result<Vec<TrackRecord>> {
        let mut stmt = self.conn.prepare(sql).context("preparing track query")?;
        let rows = stmt
            .query_map(params, track_from_row)
            .context("querying tracks")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("reading tracks")
    }

    /// Mark `loser_id` as a duplicate of `winner_id`.
    pub fn set_duplicate_of(&self, loser_id: i64, winner_id: i64) -> Result<()> {
        self.conn
            .execute(
                "UPDATE tracks SET duplicate_of = ?1 WHERE id = ?2",
                params![winner_id, loser_id],
            )
            .context("setting duplicate_of")?;
        Ok(())
    }

    /// All duplicate groups, one per winner, with the losers that point at it.
    pub fn duplicate_groups(&self) -> Result<Vec<DuplicateGroup>> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {TRACK_COLUMNS} FROM tracks WHERE duplicate_of IS NOT NULL ORDER BY duplicate_of, id"
            ))
            .context("preparing duplicate query")?;
        let losers = stmt
            .query_map([], track_from_row)
            .context("querying duplicates")?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("reading duplicates")?;

        let mut groups: Vec<DuplicateGroup> = Vec::new();
        for loser in losers {
            let winner_id = loser.duplicate_of.expect("loser has duplicate_of");
            match groups.last_mut() {
                Some(group) if group.winner.id == winner_id => group.losers.push(loser),
                _ => {
                    let Some(winner) = self.track_by_id(winner_id)? else {
                        continue;
                    };
                    groups.push(DuplicateGroup {
                        winner,
                        losers: vec![loser],
                        conflict: false,
                    });
                }
            }
        }
        for group in &mut groups {
            group.conflict = group
                .losers
                .iter()
                .all(|loser| same_quality(&group.winner, loser));
        }
        Ok(groups)
    }

    fn track_by_id(&self, id: i64) -> Result<Option<TrackRecord>> {
        self.conn
            .query_row(
                &format!("SELECT {TRACK_COLUMNS} FROM tracks WHERE id = ?1"),
                params![id],
                track_from_row,
            )
            .optional()
            .context("loading track by id")
    }

    /// Delete a track row. Never touches the file on disk.
    pub fn delete_track(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM tracks WHERE id = ?1", params![id])
            .context("deleting track")?;
        Ok(())
    }

    /// Append a download journal entry.
    pub fn log_download(
        &self,
        source: SourceId,
        track_id: Option<i64>,
        state: &str,
        reason: Option<&str>,
    ) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO downloads (track_id, source_id, state, reason) VALUES (?1, ?2, ?3, ?4)",
                params![track_id, source.as_str(), state, reason],
            )
            .context("logging download")?;
        Ok(())
    }

    /// Upsert a release into the metadata cache, refreshing its 7-day TTL.
    pub fn upsert_release(&self, release: &Release) -> Result<()> {
        let key = match_key(release);
        let payload = serde_json::to_string(release).context("serializing release")?;
        let now = Utc::now();
        let fetched_at = now.to_rfc3339();
        let expires_at = (now + Duration::days(7)).to_rfc3339();
        self.conn
            .execute(
                "INSERT INTO releases (key, match_key, source_id, payload, fetched_at, expires_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
                 ON CONFLICT(key) DO UPDATE SET \
                   source_id = excluded.source_id, \
                   payload = excluded.payload, \
                   fetched_at = excluded.fetched_at, \
                   expires_at = excluded.expires_at",
                params![key, key, release.source.as_str(), payload, fetched_at, expires_at],
            )
            .context("upserting release")?;
        Ok(())
    }

    /// Read a cached release by key, or `None` when it is absent.
    pub fn get_release(&self, key: &str) -> Result<Option<Release>> {
        let payload: Option<String> = self
            .conn
            .query_row("SELECT payload FROM releases WHERE key = ?1", params![key], |row| row.get(0))
            .optional()
            .context("reading cached release")?;
        match payload {
            Some(json) => Ok(Some(serde_json::from_str(&json).context("deserializing release")?)),
            None => Ok(None),
        }
    }

    /// True only when a cache row exists and its `expires_at` is later than now.
    pub fn release_is_fresh(&self, key: &str) -> Result<bool> {
        let expires_at: Option<String> = self
            .conn
            .query_row("SELECT expires_at FROM releases WHERE key = ?1", params![key], |row| row.get(0))
            .optional()
            .context("reading release expiry")?;
        Ok(match expires_at {
            // RFC 3339 UTC timestamps compare lexicographically.
            Some(expires) => expires.as_str() > Utc::now().to_rfc3339().as_str(),
            None => false,
        })
    }
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

/// Two tracks have indistinguishable quality when lossless, bit depth, sample
/// rate and bitrate all match.
fn same_quality(a: &TrackRecord, b: &TrackRecord) -> bool {
    a.lossless == b.lossless
        && a.bit_depth == b.bit_depth
        && a.sample_rate == b.sample_rate
        && a.bitrate_kbps == b.bitrate_kbps
}