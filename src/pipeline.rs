//! The fetch pipeline: the single place where the domain meets the sources.
//!
//! A [`Pipeline`] owns the configuration, the library database and the set of
//! sources. It searches (first through the release cache, then through the
//! sources), ranks the downloadable candidates and downloads them one by one,
//! verifying the measured quality of every file against what the source
//! claimed.
//!
//! Three rules follow from the stage contract:
//!
//! * one failing source never fails the run — it is logged and skipped;
//! * a partial success (three of twelve tracks) is a normal outcome reported in
//!   [`DownloadResult`], not an error;
//! * nothing here transcodes or re-containerises: the measured file is moved
//!   into the library as-is.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use anyhow::Context;

use crate::config::Config;
use crate::db::Db;
use crate::dedup;
use crate::fetch::ProgressSink;
use crate::library::absolute_path;
use crate::matching::{ReleaseGroup, group_releases, match_key};
use crate::model::{Candidate, Container, Quality, Release, SourceId};
use crate::probe::{ProbeResult, probe};
use crate::quality::{compare_quality, rank_candidates};
use crate::sources::{Query, Source, SourceStatus, enabled_in};
use crate::tags::{TagMeta, write_tags};

/// Search, candidate ranking and verified downloading over the enabled sources.
pub struct Pipeline {
    config: Config,
    db: Db,
    sources: Vec<Box<dyn Source>>,
}

/// Outcome of one [`Pipeline::download`] call.
#[derive(Clone, Debug, PartialEq)]
pub struct DownloadResult {
    /// Final library path; empty when every candidate was rejected.
    pub path: PathBuf,
    /// Measured quality of the accepted file.
    pub quality: Quality,
    /// Raw probe result of the accepted file.
    pub probe: ProbeResult,
    /// True when at least one candidate failed or lied about its quality.
    pub rejected: bool,
    /// Why the last candidate was dropped, if any.
    pub reason: Option<String>,
    /// How many candidates were actually tried.
    pub attempts: usize,
}

impl Pipeline {
    pub fn new(config: Config, db: Db, sources: Vec<Box<dyn Source>>) -> Pipeline {
        Pipeline {
            config,
            db,
            sources,
        }
    }

    /// The library database, for callers that record or read tracks.
    pub fn db(&self) -> &Db {
        &self.db
    }

    /// One status per registered source, in registration order.
    ///
    /// Every source is reported — enabled or not — because the UI shows what
    /// exists next to what is switched on. Per-source detail is not collected
    /// yet.
    pub fn source_statuses(&self) -> Vec<SourceStatus> {
        self.sources
            .iter()
            .map(|source| SourceStatus {
                id: source.id(),
                capabilities: source.capabilities(),
                auth: source.auth_state(),
                detail: None,
            })
            .collect()
    }

    /// Sources that survived [`enabled_in`], in registration order.
    fn enabled_sources(&self) -> Vec<&dyn Source> {
        let ids: Vec<SourceId> = self.sources.iter().map(|source| source.id()).collect();
        let enabled = enabled_in(&ids, &self.config);
        self.sources
            .iter()
            .filter(|source| enabled.contains(&source.id()))
            .map(|source| source.as_ref())
            .collect()
    }

    /// Searches the sources, grouped and merged across sources.
    ///
    /// A text query is answered from the release cache when a row for it is
    /// still fresh; a file/URL query always goes to the sources. Each result
    /// group is written back to the cache, and a text query additionally stores
    /// one row under the key of the query itself so that the next identical
    /// search hits the cache instead of the network.
    pub async fn search(&self, q: &Query) -> anyhow::Result<Vec<ReleaseGroup>> {
        let text = q.text.clone();
        let cache_key = text.as_deref().map(query_key);

        if let Some(key) = cache_key.as_deref()
            && self.db.release_is_fresh(key)?
            && let Some(cached) = self.db.get_release(key)?
        {
            let source = cached.source;
            return Ok(vec![ReleaseGroup {
                primary: cached.clone(),
                members: vec![cached],
                sources: vec![source],
            }]);
        }

        let by_text = text.is_some();
        let sources = self.enabled_sources();
        let results = futures_util::future::join_all(sources.iter().map(|source| async move {
            let id = source.id();
            let outcome = if by_text {
                source.search(q).await
            } else {
                source.resolve(q).await
            };
            (id, outcome)
        }))
        .await;

        let mut releases = Vec::new();
        for (id, outcome) in results {
            match outcome {
                Ok(found) => releases.extend(found),
                Err(err) => {
                    tracing::warn!(source = %id.as_str(), error = %err, "source search failed");
                }
            }
        }

        let groups = group_releases(releases, self.config.matching.duration_tolerance_sec as f64);

        for group in &groups {
            self.db.upsert_release(&group.primary)?;
        }
        if let Some(text) = text.as_deref()
            && let Some(first) = groups.first()
        {
            let query_release = Release {
                title: text.to_string(),
                artist: String::new(),
                album: None,
                ..first.primary.clone()
            };
            self.db.upsert_release(&query_release)?;
        }

        Ok(groups)
    }

    /// Collects candidates for `group` from every enabled source and ranks them.
    ///
    /// The group's primary release is the one handed to the sources. A source
    /// that fails is logged and skipped.
    pub async fn candidates(&self, group: &ReleaseGroup) -> anyhow::Result<Vec<Candidate>> {
        let sources = self.enabled_sources();
        let primary = &group.primary;
        let results = futures_util::future::join_all(sources.iter().map(|source| async move {
            let id = source.id();
            (id, source.candidates(primary).await)
        }))
        .await;

        let mut all = Vec::new();
        for (id, outcome) in results {
            match outcome {
                Ok(found) => all.extend(found),
                Err(err) => {
                    tracing::warn!(source = %id.as_str(), error = %err, "source candidates failed");
                }
            }
        }

        Ok(rank_candidates(&all))
    }

    /// Downloads the first candidate that survives verification.
    ///
    /// The candidates are tried in the order given (see
    /// [`rank_candidates`](crate::quality::rank_candidates)). Every attempt is
    /// downloaded into `library/.tmp` under a fresh name, probed, and compared
    /// against the claim: a file that measures worse than the source promised is
    /// deleted and the next candidate is tried. A file that is at least as good
    /// as promised is moved into the library, tagged with the *measured*
    /// quality, and recorded in the database.
    ///
    /// Reaching the end of the list is not an error — the result then carries
    /// `rejected: true` and an empty path.
    pub async fn download(
        &self,
        candidates: &[Candidate],
        sink: &dyn ProgressSink,
    ) -> anyhow::Result<DownloadResult> {
        let tmp_dir = self.config.library.path.join(".tmp");
        tokio::fs::create_dir_all(&tmp_dir)
            .await
            .with_context(|| format!("creating {}", tmp_dir.display()))?;

        let mut attempts = 0usize;
        let mut rejected = false;
        let mut reason: Option<String> = None;
        let mut last_probe: Option<ProbeResult> = None;

        for candidate in candidates {
            attempts += 1;

            let source = self
                .sources
                .iter()
                .find(|source| source.id() == candidate.source)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "no source registered for a candidate from {}",
                        candidate.source.as_str()
                    )
                })?;

            // A fresh temporary name per attempt: `fetch::download_to_file` writes
            // to a fixed `.part` sibling, so two attempts sharing a destination
            // would overwrite each other. On a `<uuid>.part` path that internal
            // rename is a self-rename, which is harmless.
            let tmp_path = tmp_dir.join(format!("{}.part", uuid::Uuid::new_v4()));

            if let Err(err) = source.download(candidate, &tmp_path, sink).await {
                rejected = true;
                reason = Some(format!("download failed: {err}"));
                discard(&tmp_path).await;
                continue;
            }

            let probe_input = tmp_path.clone();
            let probed = tokio::task::spawn_blocking(move || probe(&probe_input))
                .await
                .context("probing the downloaded file")?;
            let probed = match probed {
                Ok(result) => result,
                Err(err) => {
                    rejected = true;
                    reason = Some(format!("probe failed: {err}"));
                    discard(&tmp_path).await;
                    continue;
                }
            };

            let measured = Quality {
                lossless: probed.container.is_lossless(),
                container: probed.container,
                bit_depth: probed.bit_depth,
                sample_rate: probed.sample_rate,
                bitrate_kbps: probed.bitrate_kbps,
            };
            last_probe = Some(probed.clone());

            // `compare_quality` compares candidates, so the honest measurement is
            // wrapped in a copy of the candidate that carried the claim.
            let measured_candidate = Candidate {
                quality: measured.clone(),
                ..candidate.clone()
            };
            if compare_quality(&measured_candidate, candidate) == Ordering::Less {
                rejected = true;
                reason = Some(format!(
                    "claimed {} bit / {} Hz, measured {} bit / {} Hz",
                    show(candidate.quality.bit_depth),
                    show(candidate.quality.sample_rate),
                    show(measured.bit_depth),
                    show(measured.sample_rate),
                ));
                discard(&tmp_path).await;
                continue;
            }

            let final_path = absolute_path(
                &self.config.library.path,
                &candidate.release,
                measured.container.as_str(),
            );
            if let Some(parent) = final_path.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            tokio::fs::rename(&tmp_path, &final_path)
                .await
                .with_context(|| {
                    format!("moving {} to {}", tmp_path.display(), final_path.display())
                })?;

            let acquired_at = chrono::Utc::now().to_rfc3339();
            let meta = TagMeta {
                title: candidate.release.title.clone(),
                artist: candidate.release.artist.clone(),
                album: candidate.release.album.clone(),
                track_no: candidate.release.track_no,
                year: candidate.release.year,
                isrc: candidate.release.isrc.clone(),
                quality: measured.clone(),
                source: candidate.source,
                acquired_at: acquired_at.clone(),
                quality_comment: String::new(),
            };
            // Tagging touches the file header, so it is blocking but short.
            let tagged = final_path.clone();
            tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
                write_tags(&tagged, &meta)?;
                Ok(())
            })
            .await
            .context("tagging the downloaded file")??;

            // The single writer of `tracks`: it deduplicates, sets `duplicate_of`
            // and journals the download inside one IMMEDIATE transaction.
            dedup::register(
                &self.config,
                &self.db,
                candidate,
                &final_path,
                measured.clone(),
                candidate.release.duration_sec,
            )?;

            return Ok(DownloadResult {
                path: final_path,
                quality: measured,
                probe: probed,
                rejected,
                reason,
                attempts,
            });
        }

        Ok(DownloadResult {
            path: PathBuf::new(),
            quality: Quality::default(),
            probe: last_probe.unwrap_or_else(neutral_probe),
            rejected: true,
            reason,
            attempts,
        })
    }

    /// Convenience wrapper for a single candidate, with identical semantics.
    pub async fn download_one(
        &self,
        candidate: &Candidate,
        sink: &dyn ProgressSink,
    ) -> anyhow::Result<DownloadResult> {
        self.download(std::slice::from_ref(candidate), sink).await
    }
}

/// The release cache key of a free-text query.
///
/// [`match_key`] of a release with an empty artist and album: the query text is
/// the title, so the key never collides with a real release.
fn query_key(text: &str) -> String {
    match_key(&Release {
        title: text.to_string(),
        artist: String::new(),
        album: None,
        track_no: None,
        duration_sec: None,
        isrc: None,
        year: None,
        source: SourceId::Soulseek,
        source_item_id: String::new(),
    })
}

/// Formats an optional measurement for the rejection reason.
fn show<T: std::fmt::Display>(value: Option<T>) -> String {
    value.map_or_else(|| "unknown".to_string(), |value| value.to_string())
}

/// Best-effort removal of a temporary file.
///
/// This is the only deletion the pipeline performs; library files are never
/// removed.
async fn discard(path: &Path) {
    match tokio::fs::remove_file(path).await {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            tracing::warn!(path = %path.display(), error = %err, "failed to remove temporary file");
        }
    }
}

/// Placeholder probe result for "nothing was measured"; consistent with
/// [`Quality::default`].
fn neutral_probe() -> ProbeResult {
    ProbeResult {
        container: Container::Opus,
        bit_depth: None,
        sample_rate: None,
        bitrate_kbps: None,
        duration_sec: None,
        channels: None,
    }
}
