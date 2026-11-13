//! Source contract and the static source registry.
//!
//! This module defines what a source is: its [`Capabilities`], its
//! [`AuthState`], the [`Query`] it can be asked, and the [`Source`] trait it
//! implements. [`registry`] answers "which sources exist in this build" and
//! stays empty until the stage plans register their implementations;
//! [`enabled_in`] is the single place where `config.sources.enabled` is turned
//! into a list of ready-to-use ids.

pub mod soulseek;

use std::path::Path;

use async_trait::async_trait;

use crate::config::{Config, Credentials};
use crate::fetch::ProgressSink;
use crate::model::{Candidate, Release, SourceId};

/// What a source can do, independent of its current configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    /// The source delivers audio files.
    pub audio: bool,
    /// The source only exposes catalogue metadata.
    pub metadata_only: bool,
    /// The source can deliver lossless audio.
    pub lossless: bool,
    /// The source cannot work without credentials.
    pub needs_auth: bool,
}

/// Current authorisation state of a source.
///
/// Obtained without any I/O: the TUI calls it on every redraw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthState {
    /// The source needs no authorisation.
    NotNeeded,
    /// Credentials are absent.
    Missing,
    /// Credentials are present and valid.
    Ok,
    /// The key or token was rejected.
    Expired,
}

/// A single explicitly addressed item.
#[derive(Clone, Debug, PartialEq)]
pub enum QueryItem {
    Track { source: SourceId, id: String },
    Album { source: SourceId, id: String },
    Playlist { source: SourceId, id: String },
}

/// A request addressed by free text, by URL, or by explicit items.
///
/// At most one of the three is expected to be populated; enforcing that is the
/// caller's job, not this type's.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Query {
    pub text: Option<String>,
    pub url: Option<String>,
    pub items: Vec<QueryItem>,
}

/// Reported state of one source, as surfaced to the UI.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceStatus {
    pub id: SourceId,
    pub capabilities: Capabilities,
    pub auth: AuthState,
    pub detail: Option<String>,
}

/// The contract every source implements.
#[async_trait]
pub trait Source: Send + Sync {
    /// Identifies the source; pure, no I/O.
    fn id(&self) -> SourceId;

    /// Static capabilities of the source.
    fn capabilities(&self) -> Capabilities;

    /// Current authorisation state; must not touch the network.
    fn auth_state(&self) -> AuthState;

    /// Searches by free text, URL or explicit items.
    async fn search(&self, q: &Query) -> anyhow::Result<Vec<Release>>;

    /// Resolves an explicit request into the releases it refers to.
    async fn resolve(&self, q: &Query) -> anyhow::Result<Vec<Release>>;

    /// Lists the downloadable candidates for `release`.
    async fn candidates(&self, release: &Release) -> anyhow::Result<Vec<Candidate>>;

    /// Downloads `candidate` into `dest`, reporting progress to `sink`.
    ///
    /// `dest` is the path already chosen by the caller; creating directories,
    /// `.part` suffixes and the atomic rename are the source's business.
    async fn download(
        &self,
        candidate: &Candidate,
        dest: &Path,
        sink: &dyn ProgressSink,
    ) -> anyhow::Result<()>;
}

/// Returns every source compiled into this build.
///
/// The registry never filters by configuration: it answers "what exists",
/// not "what is enabled". Selection happens in [`enabled_in`].
pub fn registry(_config: &Config, _credentials: &Credentials) -> Vec<Box<dyn Source>> {
    // Stage plans register their sources here, one `Box::new(...)` per source.
    // Do not filter by `config.sources.enabled`: selection happens in `enabled_in`.
    Vec::new()
}

/// Keeps the ids that `config.sources.enabled` lists, in the order of `ids`.
///
/// This is the only selection point; pipeline, TUI and CLI must call it instead
/// of comparing source names themselves. An empty configured list selects
/// nothing.
pub fn enabled_in(ids: &[SourceId], config: &Config) -> Vec<SourceId> {
    for name in &config.sources.enabled {
        if SourceId::from_str(name).is_none() {
            tracing::warn!(name = %name, "unknown source name in config.sources.enabled");
        }
    }

    ids.iter()
        .copied()
        .filter(|id| is_enabled(id, config))
        .collect()
}

/// Tells whether `id` appears in `config.sources.enabled`.
///
/// Case-sensitive, like [`SourceId::as_str`]; values in `config.toml` are
/// lower-case by convention.
pub fn is_enabled(id: &SourceId, config: &Config) -> bool {
    config.sources.enabled.iter().any(|e| e == id.as_str())
}