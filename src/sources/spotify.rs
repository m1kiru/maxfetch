//! Spotify as a metadata source: JSON mapping and the HTTP seam.
//!
//! The pure mappers take the JSON that the Spotify Web API returns, so they are
//! testable without a network. `SpotifyApi` is the one seam the real client and
//! the unconfigured stub share; `impl Source` arrives with Task 6.

use std::future::Future;
use std::sync::Arc;

use anyhow::anyhow;
use rspotify::clients::BaseClient as _;
use rspotify::model::{AlbumId, PlaylistId, SearchType, TrackId};

use crate::model::{Release, SourceId};

/// The one seam between the source and Spotify: one object-safe trait, two
/// implementations.
pub trait SpotifyApi: Send + Sync + std::fmt::Debug {
    fn search_tracks(&self, query: &str) -> anyhow::Result<Vec<serde_json::Value>>;
    fn track(&self, id: &str) -> anyhow::Result<serde_json::Value>;
    fn album(&self, id: &str) -> anyhow::Result<Vec<serde_json::Value>>;
    fn playlist_items(&self, id: &str) -> anyhow::Result<Vec<serde_json::Value>>;
}

/// The one string shared by the token parser, the log line and the UI wording.
pub const SPOTIFY_KEYS_REJECTED: &str = "ключи Spotify отклонены";

/// The ISRC of a track, from `external_ids.isrc`; an empty value is no value.
pub fn isrc(track: &serde_json::Value) -> Option<String> {
    track
        .get("external_ids")?
        .get("isrc")?
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// The duration in seconds, keeping the fraction that Spotify reports.
pub fn duration_sec(track: &serde_json::Value) -> Option<f64> {
    Some(track.get("duration_ms")?.as_f64()? / 1000.0)
}

/// Maps one track object to a [`Release`].
///
/// `source_item_id` is passed in rather than read from `id`: the id is only
/// validated, because a local or unavailable track has none and cannot be
/// downloaded. Nothing is invented: a missing album, track number or year stays
/// `None`.
pub fn release_from_track(
    track: &serde_json::Value,
    source_item_id: &str,
) -> anyhow::Result<Release> {
    let title = track
        .get("name")
        .and_then(|value| value.as_str())
        .ok_or_else(|| anyhow!("у трека Spotify нет поля name"))?;

    if track.get("id").and_then(|value| value.as_str()).is_none() {
        anyhow::bail!("у трека Spotify нет поля id: такой трек нельзя скачать");
    }

    let artist = track
        .get("artists")
        .and_then(|value| value.as_array())
        .and_then(|artists| artists.first())
        .and_then(|artist| artist.get("name"))
        .and_then(|value| value.as_str())
        .ok_or_else(|| anyhow!("у трека Spotify нет артиста (artists[0].name)"))?;

    let album = track
        .get("album")
        .and_then(|value| value.get("name"))
        .and_then(|value| value.as_str())
        .map(str::to_string);

    let track_no = track
        .get("track_number")
        .and_then(|value| value.as_u64())
        .and_then(|number| u32::try_from(number).ok());

    Ok(Release {
        title: title.to_string(),
        artist: artist.to_string(),
        album,
        track_no,
        duration_sec: duration_sec(track),
        isrc: isrc(track),
        year: year(track),
        source: SourceId::Spotify,
        source_item_id: source_item_id.to_string(),
    })
}

/// The release year, from the first four characters of `album.release_date`.
///
/// An absent, short, non-ASCII or non-numeric date yields `None`; the current
/// year is never substituted.
fn year(track: &serde_json::Value) -> Option<i32> {
    let date = track.get("album")?.get("release_date")?.as_str()?;
    date.get(..4)?.parse().ok()
}

/// Reads the `client_credentials` response body.
///
/// An `error` member is checked first, so a body such as
/// `{"error": "invalid_client"}` is never mistaken for a successful login, even
/// under HTTP 200.
pub fn parse_token_response(body: &serde_json::Value) -> anyhow::Result<String> {
    if body.get("error").is_some() {
        return Err(anyhow!("{SPOTIFY_KEYS_REJECTED}"));
    }

    match body.get("access_token").and_then(|value| value.as_str()) {
        Some(token) if !token.is_empty() => Ok(token.to_string()),
        _ => Err(anyhow!("в ответе Spotify нет поля access_token")),
    }
}

/// Errors on every call; used when no credentials are configured.
#[derive(Clone, Debug)]
pub struct UnconfiguredApi;

impl SpotifyApi for UnconfiguredApi {
    fn search_tracks(&self, _query: &str) -> anyhow::Result<Vec<serde_json::Value>> {
        anyhow::bail!("Spotify не настроен: нет client_id/client_secret")
    }

    fn track(&self, _id: &str) -> anyhow::Result<serde_json::Value> {
        anyhow::bail!("Spotify не настроен: нет client_id/client_secret")
    }

    fn album(&self, _id: &str) -> anyhow::Result<Vec<serde_json::Value>> {
        anyhow::bail!("Spotify не настроен: нет client_id/client_secret")
    }

    fn playlist_items(&self, _id: &str) -> anyhow::Result<Vec<serde_json::Value>> {
        anyhow::bail!("Spotify не настроен: нет client_id/client_secret")
    }
}

/// Holds what Task 6's `impl Source` will read.
#[derive(Clone, Debug)]
// read by `impl Source` in Task 6
#[allow(dead_code)]
pub struct SpotifySource {
    api: Arc<dyn SpotifyApi>,
    client_id: String,
    client_secret: String,
}

impl SpotifySource {
    pub fn new(
        api: Arc<dyn SpotifyApi>,
        client_id: String,
        client_secret: String,
    ) -> SpotifySource {
        SpotifySource {
            api,
            client_id,
            client_secret,
        }
    }
}

/// Real client over `rspotify` with the `client_credentials` flow.
#[derive(Clone, Debug)]
pub struct RspotifyApi {
    client: rspotify::ClientCredsSpotify,
}

impl RspotifyApi {
    pub fn new(client_id: String, client_secret: String) -> RspotifyApi {
        RspotifyApi {
            client: rspotify::ClientCredsSpotify::new(rspotify::Credentials::new(
                &client_id,
                &client_secret,
            )),
        }
    }

    /// Bridges the synchronous seam onto `rspotify`'s async client.
    ///
    /// `block_in_place` keeps the runtime from panicking, but it requires a
    /// multi-thread runtime, the same requirement `SoulseekSource` and
    /// `YoutubeSource` already carry.
    fn block<F: Future>(&self, future: F) -> F::Output {
        tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(future))
    }

    /// Makes sure the shared token slot is filled; `rspotify` keeps the token
    /// itself, so there is no second copy here.
    async fn ensure_token(&self) -> anyhow::Result<()> {
        let cached = match self.client.get_token().lock().await {
            Ok(slot) => slot.is_some(),
            Err(_) => false,
        };
        if cached {
            return Ok(());
        }
        self.request_token().await
    }

    /// Requests a fresh token, mapping a rejected exchange to the shared
    /// message so bad keys always read the same way.
    async fn request_token(&self) -> anyhow::Result<()> {
        match self.client.request_token().await {
            Ok(()) => Ok(()),
            Err(error) => Err(self.token_error(error)),
        }
    }

    /// Turns a token-exchange failure into a readable error.
    ///
    /// `rspotify` hands over the response, not its body, and the body can only
    /// be read asynchronously, hence the `&self` helper over [`Self::block`].
    fn token_error(&self, error: rspotify::ClientError) -> anyhow::Error {
        let detail = error.to_string();

        let rspotify::ClientError::Http(http) = error else {
            return anyhow!("{detail}");
        };
        let rspotify::http::HttpError::StatusCode(response) = *http else {
            return anyhow!("{detail}");
        };

        let Ok(text) = self.block(async { response.text().await }) else {
            return anyhow!("{detail}");
        };
        let Ok(body) = serde_json::from_str::<serde_json::Value>(&text) else {
            return anyhow!("{detail}");
        };

        match parse_token_response(&body) {
            Err(error) => error,
            Ok(_) => anyhow!("{detail}"),
        }
    }

    /// Runs one API call, retrying it once if the token has expired.
    ///
    /// `rspotify` refreshes on its own schedule but not on a 401, so exactly one
    /// retry after a fresh token is the whole policy.
    fn with_retry<T, F, Fut>(&self, call: F) -> anyhow::Result<T>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = rspotify::ClientResult<T>>,
    {
        self.block(async {
            self.ensure_token().await?;

            match call().await {
                Ok(value) => Ok(value),
                Err(error) if is_unauthorized(&error) => {
                    self.request_token().await?;
                    call().await.map_err(api_error)
                }
                Err(error) => Err(api_error(error)),
            }
        })
    }
}

impl SpotifyApi for RspotifyApi {
    fn search_tracks(&self, query: &str) -> anyhow::Result<Vec<serde_json::Value>> {
        let result = self.with_retry(|| {
            self.client.search_multiple(
                query,
                [SearchType::Track, SearchType::Album, SearchType::Artist],
                None,
                None,
                Some(10),
                None,
            )
        })?;

        match result.tracks {
            Some(page) => values(page.items),
            None => Ok(Vec::new()),
        }
    }

    fn track(&self, id: &str) -> anyhow::Result<serde_json::Value> {
        let track_id =
            TrackId::from_id(id).map_err(|error| anyhow!("неверный id трека Spotify: {error}"))?;
        let track = self.with_retry(|| self.client.track(track_id.clone(), None))?;
        Ok(serde_json::to_value(track)?)
    }

    fn album(&self, id: &str) -> anyhow::Result<Vec<serde_json::Value>> {
        let album_id = AlbumId::from_id(id)
            .map_err(|error| anyhow!("неверный id альбома Spotify: {error}"))?;
        let album = self.with_retry(|| self.client.album(album_id.clone(), None))?;
        values(album.tracks.items)
    }

    fn playlist_items(&self, id: &str) -> anyhow::Result<Vec<serde_json::Value>> {
        let playlist_id = PlaylistId::from_id(id)
            .map_err(|error| anyhow!("неверный id плейлиста Spotify: {error}"))?;
        let page = self.with_retry(|| {
            self.client
                .playlist_items_manual(playlist_id.clone(), None, None, Some(50), Some(0))
        })?;
        values(page.items)
    }
}

/// Serialises model objects back to the JSON the mappers understand.
fn values<T: serde::Serialize>(items: Vec<T>) -> anyhow::Result<Vec<serde_json::Value>> {
    items
        .into_iter()
        .map(|item| serde_json::to_value(item).map_err(anyhow::Error::from))
        .collect()
}

/// Whether the request failed because the token is no longer accepted.
fn is_unauthorized(error: &rspotify::ClientError) -> bool {
    match error {
        rspotify::ClientError::Http(http) => match http.as_ref() {
            rspotify::http::HttpError::StatusCode(response) => response.status().as_u16() == 401,
            _ => false,
        },
        _ => false,
    }
}

/// Wraps a failed API call with a user-facing message.
fn api_error(error: rspotify::ClientError) -> anyhow::Error {
    anyhow!("запрос к Spotify не удался: {error}")
}
