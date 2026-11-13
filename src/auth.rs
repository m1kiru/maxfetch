//! Per-source authorisation status and the login stubs.
//!
//! The core build ships no source implementations, so `login` is a stub: the
//! stage plans replace each branch with the real flow.

use anyhow::Result;

use crate::config::{self, Credentials};
use crate::model::SourceId;
use crate::sources::{AuthState, Capabilities, SourceStatus};

/// Every source this build knows, in the order of [`SourceId::as_str`].
const ALL: [SourceId; 9] = [
    SourceId::Soulseek,
    SourceId::Bandcamp,
    SourceId::Youtube,
    SourceId::Spotify,
    SourceId::SoundCloud,
    SourceId::Qobuz,
    SourceId::Tidal,
    SourceId::Deezer,
    SourceId::Beatport,
];

/// Static capabilities of `id`, until the registry starts returning sources.
///
/// Values come from the design spec, section 6.6 ("Состав источников").
const fn capabilities(id: SourceId) -> Capabilities {
    match id {
        SourceId::Soulseek => Capabilities {
            audio: true,
            metadata_only: false,
            lossless: true,
            needs_auth: true,
        },
        SourceId::Bandcamp => Capabilities {
            audio: true,
            metadata_only: false,
            lossless: true,
            needs_auth: true,
        },
        SourceId::Youtube => Capabilities {
            audio: true,
            metadata_only: false,
            lossless: false,
            needs_auth: false,
        },
        SourceId::Spotify => Capabilities {
            audio: false,
            metadata_only: true,
            lossless: false,
            needs_auth: true,
        },
        SourceId::SoundCloud => Capabilities {
            audio: true,
            metadata_only: false,
            lossless: true,
            needs_auth: false,
        },
        SourceId::Qobuz => Capabilities {
            audio: true,
            metadata_only: false,
            lossless: true,
            needs_auth: true,
        },
        SourceId::Tidal => Capabilities {
            audio: true,
            metadata_only: false,
            lossless: false,
            needs_auth: true,
        },
        SourceId::Deezer => Capabilities {
            audio: true,
            metadata_only: false,
            lossless: false,
            needs_auth: true,
        },
        SourceId::Beatport => Capabilities {
            audio: true,
            metadata_only: false,
            lossless: true,
            needs_auth: true,
        },
    }
}

/// Reports, per source, whether credentials are present. No I/O.
pub fn auth_status(credentials: &Credentials) -> Vec<SourceStatus> {
    ALL.iter()
        .map(|id| SourceStatus {
            id: *id,
            capabilities: capabilities(*id),
            auth: auth_state(*id, credentials),
            detail: stage1b_detail(*id).map(str::to_string),
        })
        .collect()
}

/// The Stage 1b detail line: both sources work without an account.
///
/// Bandcamp serves free promo downloads and SoundCloud serves AAC 160k over a
/// public `client_id`, so neither cares about its credentials section yet.
fn stage1b_detail(id: SourceId) -> Option<&'static str> {
    match id {
        SourceId::Bandcamp => Some("без учётной записи: только бесплатное промо"),
        SourceId::SoundCloud => Some("без учётной записи: только AAC 160k"),
        _ => None,
    }
}

/// One Russian line for a source: id, auth state and the detail when present.
///
/// The wording matches the Sources screen, so the CLI and the TUI show the same
/// string for the same source.
pub fn describe_status(status: &SourceStatus) -> String {
    let state = match status.auth {
        AuthState::Ok | AuthState::NotNeeded => "доступен",
        AuthState::Missing => "нет авторизации",
        AuthState::Expired => "ключ протух",
    };
    match status.detail.as_deref() {
        Some(detail) => format!("{}: {state} — {detail}", status.id.as_str()),
        None => format!("{}: {state}", status.id.as_str()),
    }
}

/// Maps the credential section of `id` onto an [`AuthState`].
///
/// Presence only: an empty section counts as configured, because validating
/// the contents would need I/O and this function must stay pure.
fn auth_state(id: SourceId, credentials: &Credentials) -> AuthState {
    let present = match id {
        SourceId::Soulseek => credentials.soulseek.is_some(),
        SourceId::Spotify => credentials.spotify.is_some(),
        SourceId::Qobuz => credentials.qobuz.is_some(),
        SourceId::Tidal => credentials.tidal.is_some(),
        SourceId::Deezer => credentials.deezer.is_some(),
        SourceId::Beatport => credentials.beatport.is_some(),
        // YouTube needs no account (spec section 6.6, stage 1); Bandcamp and
        // SoundCloud need none in Stage 1b either (spec sections 8.2 and 8.9).
        SourceId::Youtube | SourceId::Bandcamp | SourceId::SoundCloud => {
            return AuthState::NotNeeded;
        }
    };
    if present {
        AuthState::Ok
    } else {
        AuthState::Missing
    }
}

/// Explains why `source` cannot log in yet, or `None` when the generic
/// "not implemented" error applies.
///
/// The Stage 1b sources work without an account; their real login flows are
/// deferred to Stage 2, so the hint says so in Russian, like every CLI line.
pub fn stage1b_login_hint(source: &str) -> Option<&'static str> {
    match source {
        "bandcamp" => {
            Some("вход в Bandcamp появится в этапе 2: в этапе 1б доступно только бесплатное промо")
        }
        "soundcloud" => {
            Some("вход в SoundCloud появится в этапе 2: без него доступен только AAC 160k")
        }
        _ => None,
    }
}

/// Performs the interactive login of `source` and stores the result.
///
/// The Stage 1b sources answer with a Russian hint about Stage 2; everything
/// else still fails with the "not implemented" error until its stage plan
/// fills in the real flow.
pub async fn login(source: &str, credentials: &mut Credentials) -> Result<()> {
    if let Some(hint) = stage1b_login_hint(source) {
        anyhow::bail!(hint);
    }
    let _ = credentials; // no store yet; the stage plans use it.
    anyhow::bail!("login for {source} is not implemented in the core build")
}

/// Writes `credentials` to `$XDG_CONFIG_HOME/maxfetch/credentials.toml` (0600).
pub fn save_credentials(credentials: &Credentials) -> Result<()> {
    credentials.save_to(&config::config_dir())
}
