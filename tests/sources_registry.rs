use std::sync::Arc;

use maxfetch::config::{Config, Credentials, SoulseekCredentials, SpotifyCredentials};
use maxfetch::model::SourceId;
use maxfetch::sources::spotify::{RspotifyApi, SpotifySource, UnconfiguredApi};
use maxfetch::sources::{AuthState, enabled_in, registry, skip_reason, source_statuses};

#[test]
fn the_registry_holds_the_three_stage_one_sources() {
    let sources = registry(&Config::default(), &Credentials::default());
    let ids: Vec<SourceId> = sources.iter().map(|s| s.id()).collect();
    assert_eq!(
        ids,
        vec![SourceId::Soulseek, SourceId::Youtube, SourceId::Spotify]
    );
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
    assert_eq!(
        enabled_in(&ids, &cfg),
        vec![SourceId::Youtube, SourceId::Spotify]
    );

    // An empty enabled list means "everything enabled", in the input order.
    cfg.sources.enabled = Vec::new();
    assert_eq!(enabled_in(&ids, &cfg), ids);
    assert_eq!(registry(&cfg, &Credentials::default()).len(), 3);
}

#[test]
fn authorization_follows_the_credentials_file() {
    let sources = registry(&Config::default(), &Credentials::default());
    let soulseek = sources
        .iter()
        .find(|s| s.id() == SourceId::Soulseek)
        .unwrap();
    let spotify = sources
        .iter()
        .find(|s| s.id() == SourceId::Spotify)
        .unwrap();
    let youtube = sources
        .iter()
        .find(|s| s.id() == SourceId::Youtube)
        .unwrap();
    assert_eq!(soulseek.auth_state(), AuthState::Missing);
    assert_eq!(spotify.auth_state(), AuthState::Missing);
    assert_eq!(youtube.auth_state(), AuthState::NotNeeded);

    let creds = Credentials {
        soulseek: Some(SoulseekCredentials {
            username: "user".into(),
            password: "pass".into(),
        }),
        spotify: Some(SpotifyCredentials {
            client_id: "id".into(),
            client_secret: "secret".into(),
            refresh_token: String::new(),
        }),
        ..Credentials::default()
    };
    let sources = registry(&Config::default(), &creds);
    assert!(sources.iter().all(|s| s.auth_state() != AuthState::Missing));
}

#[test]
fn capabilities_describe_each_source() {
    let sources = registry(&Config::default(), &Credentials::default());
    let caps = |id: SourceId| {
        sources
            .iter()
            .find(|s| s.id() == id)
            .unwrap()
            .capabilities()
    };
    assert_eq!(
        caps(SourceId::Soulseek),
        maxfetch::sources::Capabilities {
            audio: true,
            metadata_only: false,
            lossless: true,
            needs_auth: true
        }
    );
    assert_eq!(
        caps(SourceId::Youtube),
        maxfetch::sources::Capabilities {
            audio: true,
            metadata_only: false,
            lossless: false,
            needs_auth: false
        }
    );
    assert_eq!(
        caps(SourceId::Spotify),
        maxfetch::sources::Capabilities {
            audio: false,
            metadata_only: true,
            lossless: false,
            needs_auth: true
        }
    );
}

#[test]
fn a_missing_yt_dlp_is_reported_in_the_source_detail() {
    let mut cfg = Config::default();
    cfg.youtube.yt_dlp_path = "/nonexistent/yt-dlp".into();
    let statuses = source_statuses(&registry(&cfg, &Credentials::default()));
    let youtube = statuses.iter().find(|s| s.id == SourceId::Youtube).unwrap();
    assert!(youtube.detail.as_deref().unwrap().contains("yt-dlp"));
    assert_eq!(youtube.auth, AuthState::NotNeeded);
}

#[test]
fn unauthorized_sources_get_a_skip_reason() {
    let sources = registry(&Config::default(), &Credentials::default());
    let soulseek = sources
        .iter()
        .find(|s| s.id() == SourceId::Soulseek)
        .unwrap();
    let youtube = sources
        .iter()
        .find(|s| s.id() == SourceId::Youtube)
        .unwrap();
    let reason = skip_reason(soulseek.as_ref()).unwrap();
    assert!(reason.contains("нет авторизации"));
    assert!(skip_reason(youtube.as_ref()).is_none());
}

#[test]
fn spotify_debug_masks_the_client_secret() {
    let unconfigured = SpotifySource::new(
        Arc::new(UnconfiguredApi),
        "client-id".into(),
        "top-secret".into(),
    );
    let text = format!("{unconfigured:?}");
    assert!(text.contains("client-id"), "{text}");
    assert!(!text.contains("top-secret"), "{text}");

    // `RspotifyApi` wraps `rspotify::ClientCredsSpotify`, whose derived `Debug`
    // prints the secret, so a configured source must stay safe to log too.
    let configured = SpotifySource::new(
        Arc::new(RspotifyApi::new("client-id".into(), "top-secret".into())),
        "client-id".into(),
        "top-secret".into(),
    );
    let text = format!("{configured:?}");
    assert!(text.contains("client-id"), "{text}");
    assert!(!text.contains("top-secret"), "{text}");
}
