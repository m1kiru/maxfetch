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
            detail: None,
        })
        .collect()
}

/// Maps the credential section of `id` onto an [`AuthState`].
///
/// Presence only: an empty section counts as configured, because validating
/// the contents would need I/O and this function must stay pure.
fn auth_state(id: SourceId, credentials: &Credentials) -> AuthState {
    let present = match id {
        SourceId::Soulseek => credentials.soulseek.is_some(),
        SourceId::Bandcamp => credentials.bandcamp.is_some(),
        SourceId::Spotify => credentials.spotify.is_some(),
        SourceId::SoundCloud => credentials.soundcloud.is_some(),
        SourceId::Qobuz => credentials.qobuz.is_some(),
        SourceId::Tidal => credentials.tidal.is_some(),
        SourceId::Deezer => credentials.deezer.is_some(),
        SourceId::Beatport => credentials.beatport.is_some(),
        // YouTube needs no account (spec section 6.6, stage 1).
        SourceId::Youtube => return AuthState::NotNeeded,
    };
    if present {
        AuthState::Ok
    } else {
        AuthState::Missing
    }
}

/// Performs the interactive login of `source` and stores the result.
///
/// The core build implements no source yet, so this always fails with a
/// "not implemented" error; the stage plans fill in the real flow.
pub async fn login(source: &str, credentials: &mut Credentials) -> Result<()> {
    let _ = credentials; // no store yet; the stage plans use it.
    anyhow::bail!("login for {source} is not implemented in the core build")
}

/// Writes `credentials` to `$XDG_CONFIG_HOME/maxfetch/credentials.toml` (0600).
pub fn save_credentials(credentials: &Credentials) -> Result<()> {
    credentials.save_to(&config::config_dir())
}
