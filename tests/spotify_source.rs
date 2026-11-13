use std::sync::Arc;

use maxfetch::model::{Release, SourceId};
use maxfetch::sources::spotify::{
    SpotifySource, UnconfiguredApi, adopt_reference, playlist_requires_auth_message,
};
use maxfetch::sources::{AuthState, Query, QueryItem, Source};

fn rel(
    source: SourceId,
    title: &str,
    artist: &str,
    album: Option<&str>,
    isrc: Option<&str>,
) -> Release {
    Release {
        title: title.into(),
        artist: artist.into(),
        album: album.map(|s| s.into()),
        track_no: None,
        duration_sec: Some(369.0),
        isrc: isrc.map(|s| s.into()),
        year: None,
        source,
        source_item_id: "x".into(),
    }
}

fn source() -> SpotifySource {
    SpotifySource::new(Arc::new(UnconfiguredApi), "id".into(), "secret".into())
}

#[tokio::test]
async fn a_playlist_request_without_user_auth_returns_a_friendly_error() {
    let q = Query {
        text: None,
        url: None,
        items: vec![QueryItem::Playlist {
            source: SourceId::Spotify,
            id: "37i9dQ".into(),
        }],
    };
    let err = source().resolve(&q).await.unwrap_err();
    let msg = format!("{err}");
    assert_eq!(msg, playlist_requires_auth_message());
    assert_eq!(msg, "нужна авторизация Spotify, она появится на этапе 2");
    assert!(!msg.contains("403"));
}

#[tokio::test]
async fn spotify_lists_no_candidates_and_refuses_to_download() {
    let r = rel(
        SourceId::Spotify,
        "Get Lucky",
        "Daft Punk",
        Some("RAM"),
        Some("USQX91300108"),
    );
    assert!(source().candidates(&r).await.unwrap().is_empty());
    let cand = maxfetch::model::Candidate {
        source: SourceId::Spotify,
        release: r,
        quality: maxfetch::model::Quality::default(),
        size_bytes: None,
        locator: maxfetch::model::Locator::Spotify {
            track_id: "4uLU6hMCjMI75M1A2tKUQC".into(),
        },
    };
    let err = source()
        .download(
            &cand,
            std::path::Path::new("/tmp/x"),
            &maxfetch::fetch::NoopSink,
        )
        .await
        .unwrap_err();
    assert!(format!("{err}").contains("аудио"));
}

#[test]
fn a_matching_isrc_adopts_the_spotify_description() {
    let mut target = rel(
        SourceId::Soulseek,
        "get lucky",
        "daft punk",
        None,
        Some("USQX91300108"),
    );
    let reference = rel(
        SourceId::Spotify,
        "Get Lucky",
        "Daft Punk",
        Some("Random Access Memories"),
        Some("USQX91300108"),
    );
    assert!(adopt_reference(&mut target, &reference));
    assert_eq!(target.title, "Get Lucky");
    assert_eq!(target.artist, "Daft Punk");
    assert_eq!(target.album.as_deref(), Some("Random Access Memories"));
    assert_eq!(target.isrc.as_deref(), Some("USQX91300108"));
    assert_eq!(target.duration_sec, Some(369.0));
}

#[test]
fn conflicting_isrcs_are_never_adopted() {
    let mut target = rel(
        SourceId::Soulseek,
        "Other",
        "Someone",
        None,
        Some("USQX91300108"),
    );
    let reference = rel(
        SourceId::Spotify,
        "Get Lucky",
        "Daft Punk",
        Some("RAM"),
        Some("GB0000000001"),
    );
    assert!(!adopt_reference(&mut target, &reference));
    assert_eq!(target.artist, "Someone");
}

#[test]
fn two_descriptions_without_isrc_match_by_key() {
    let mut target = rel(
        SourceId::Soulseek,
        "Get Lucky",
        "Daft Punk",
        Some("RAM"),
        None,
    );
    let reference = rel(
        SourceId::Spotify,
        "Get Lucky",
        "Daft Punk",
        Some("RAM"),
        None,
    );
    assert!(adopt_reference(&mut target, &reference));
    assert_eq!(target.source, SourceId::Soulseek);
}

#[test]
fn credentials_decide_the_auth_state() {
    assert_eq!(source().auth_state(), AuthState::Ok);
    let empty = SpotifySource::new(Arc::new(UnconfiguredApi), String::new(), String::new());
    assert_eq!(empty.auth_state(), AuthState::Missing);
}
