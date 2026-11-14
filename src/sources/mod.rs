//! Source contract and the static source registry.
//!
//! This module defines what a source is: its [`Capabilities`], its
//! [`AuthState`], the [`Query`] it can be asked, and the [`Source`] trait it
//! implements. [`registry`] answers "which sources exist in this build" and
//! registers the implementations this build carries; [`enabled_in`] is the
//! single place where `config.sources.enabled` is turned into a list of
//! ready-to-use ids.

pub mod bandcamp;
pub mod qobuz;
pub mod soulseek;
pub mod soundcloud;
pub mod spotify;
pub mod youtube;

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;

use crate::config::{Config, Credentials};
use crate::fetch::ProgressSink;
use crate::model::{Candidate, Release, SourceId};

use bandcamp::BandcampSource;
use soulseek::SoulseekSource;
use soundcloud::SoundCloudSource;
use spotify::{RspotifyApi, SpotifyApi, SpotifySource, UnconfiguredApi};
use youtube::YoutubeSource;

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

    /// Optional diagnosis to show next to the status; must not touch the network.
    ///
    /// Defaults to "nothing to add"; a source overrides it when the message
    /// depends on its own configuration (for example a configured binary path).
    fn detail(&self) -> Option<String> {
        None
    }

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
/// not "what is enabled". Selection happens in [`enabled_in`], so a
/// registered but disabled source stays visible and does not participate
/// in a run. The order is fixed — Soulseek, YouTube, Spotify, Bandcamp,
/// SoundCloud — so the CLI and the TUI render a stable list. No constructor
/// opens a socket or reads `credentials.toml`.
pub fn registry(config: &Config, credentials: &Credentials) -> Vec<Box<dyn Source>> {
    let mut sources: Vec<Box<dyn Source>> = Vec::with_capacity(5);

    sources.push(Box::new(SoulseekSource::new(
        credentials.soulseek.clone().unwrap_or_default(),
        config.download.clone(),
    )));
    sources.push(Box::new(YoutubeSource::new(&config.youtube, &config.media)));

    let api: Arc<dyn SpotifyApi> = match &credentials.spotify {
        Some(spotify) if !spotify.client_id.is_empty() && !spotify.client_secret.is_empty() => {
            Arc::new(RspotifyApi::new(
                spotify.client_id.clone(),
                spotify.client_secret.clone(),
            ))
        }
        _ => Arc::new(UnconfiguredApi),
    };
    let (client_id, client_secret) = match &credentials.spotify {
        Some(spotify) => (spotify.client_id.clone(), spotify.client_secret.clone()),
        None => (String::new(), String::new()),
    };
    sources.push(Box::new(SpotifySource::new(api, client_id, client_secret)));

    sources.push(Box::new(BandcampSource::new(config, credentials)));
    sources.push(Box::new(SoundCloudSource::new(config, credentials)));

    sources
}

/// One status per source, with the Russian diagnosis in `detail`.
///
/// Nothing here touches the network. `detail` comes from the source itself when
/// it has something configured to report (the `yt-dlp` path); otherwise a
/// [`AuthState::Missing`] source gets the hint about the credentials section.
pub fn source_statuses(sources: &[Box<dyn Source>]) -> Vec<SourceStatus> {
    sources
        .iter()
        .map(|source| {
            let auth = source.auth_state();
            SourceStatus {
                id: source.id(),
                capabilities: source.capabilities(),
                auth,
                detail: source
                    .detail()
                    .or_else(|| credential_hint(source.id(), auth)),
            }
        })
        .collect()
}

/// Points a source without credentials at the section it needs.
///
/// Derived from [`SourceId`] and [`AuthState`] alone, so it needs no `Config`.
fn credential_hint(id: SourceId, auth: AuthState) -> Option<String> {
    if auth != AuthState::Missing {
        return None;
    }
    match id {
        SourceId::Soulseek => Some(
            "нет данных для входа в Soulseek: секция [soulseek] в credentials.toml".to_string(),
        ),
        SourceId::Spotify => Some(
            "нет client_id/client_secret Spotify: секция [spotify] в credentials.toml".to_string(),
        ),
        _ => None,
    }
}

/// Why `source` is skipped in this run; `None` when it can be used.
pub fn skip_reason(source: &dyn Source) -> Option<String> {
    match source.auth_state() {
        AuthState::Missing => Some(format!(
            "источник {} пропущен: нет авторизации",
            source.id().as_str()
        )),
        AuthState::Expired => Some(format!(
            "источник {} пропущен: ключи протухли",
            source.id().as_str()
        )),
        AuthState::Ok | AuthState::NotNeeded => None,
    }
}

/// Keeps the ids that `config.sources.enabled` lists, in the order of `ids`.
///
/// This is the only selection point; pipeline, TUI and CLI must call it instead
/// of comparing source names themselves. An empty configured list keeps every
/// id, because "no list at all" means "everything enabled".
pub fn enabled_in(ids: &[SourceId], config: &Config) -> Vec<SourceId> {
    if config.sources.enabled.is_empty() {
        return ids.to_vec();
    }

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
