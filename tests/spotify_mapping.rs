use std::path::PathBuf;

use maxfetch::model::SourceId;
use maxfetch::sources::spotify::{
    SpotifyApi, UnconfiguredApi, duration_sec, isrc, parse_token_response, release_from_track,
};

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
    let rel = release_from_track(&track_json(), "4uLU6hMCjMI75M1A2tKUQC").unwrap();
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
    let body =
        serde_json::json!({"access_token": "BQD1", "token_type": "Bearer", "expires_in": 3600});
    assert_eq!(parse_token_response(&body).unwrap(), "BQD1");
}

#[test]
fn a_response_without_a_token_is_a_readable_error() {
    let err = parse_token_response(&serde_json::json!({"token_type": "Bearer"})).unwrap_err();
    assert!(format!("{err}").contains("access_token"));
}

#[test]
fn a_200_with_an_invalid_client_body_means_the_keys_were_rejected() {
    let body =
        serde_json::json!({"error": "invalid_client", "error_description": "Invalid client"});
    let err = parse_token_response(&body).unwrap_err();
    assert_eq!(
        format!("{err}"),
        maxfetch::sources::spotify::SPOTIFY_KEYS_REJECTED
    );
    assert_eq!(format!("{err}"), "ключи Spotify отклонены");

    let status = maxfetch::sources::SourceStatus {
        id: SourceId::Spotify,
        capabilities: maxfetch::sources::Capabilities {
            audio: false,
            metadata_only: true,
            lossless: false,
            needs_auth: true,
        },
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
