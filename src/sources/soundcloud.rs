//! SoundCloud scraping and parsing: the pure seam between Task 6 and the
//! later `SoundCloudSource`.
//!
//! Everything here is stateless: a scraper that extracts the public
//! `client_id` from the web page bundles the way `yt-dlp` does (§8.9), a
//! credentials-file cache around it, and a thin typed layer over the
//! `GET /users/{id}` profile endpoint. No retry logic and no 401/403
//! re-scraping lives here; the `SoundCloudSource` itself arrives in Task 8
//! and wraps these helpers with `http_client` and its retry policy.

use std::path::Path;
use std::sync::OnceLock;

use anyhow::Context;
use regex::Regex;
use serde_json::Value;

use crate::config::{Credentials, SoundCloudCredentials};

/// The one `client_id` pattern shared by the scraper and its documentation.
pub const CLIENT_ID_REGEX: &str = r#"client_id\s*:\s*"([0-9a-zA-Z]{32})""#;

fn client_id_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX
        .get_or_init(|| Regex::new(CLIENT_ID_REGEX).expect("CLIENT_ID_REGEX must be a valid regex"))
}

/// The first 32-character `client_id` literal in `js`, if any.
pub fn find_client_id(js: &str) -> Option<String> {
    client_id_regex()
        .captures(js)
        .and_then(|captures| captures.get(1))
        .map(|hit| hit.as_str().to_string())
}

/// Every `<script src=...>` target, in document order.
pub fn script_urls(html: &str) -> Vec<String> {
    let mut urls = Vec::new();
    let mut rest = html;
    while let Some((_, tail)) = rest.split_once("<script") {
        let tag = tag_end(tail);
        if let Some(url) = script_src(tag) {
            urls.push(url);
        }
        rest = tail;
    }
    urls
}

/// The text of the `<script ...>` tag: from the match to its closing `>`.
fn tag_end(tail_after_open: &str) -> &str {
    match tail_after_open.find('>') {
        Some(end) => &tail_after_open[..end],
        None => tail_after_open,
    }
}

/// The value of the first `src="..."` attribute inside one tag, if any.
fn script_src(tag: &str) -> Option<String> {
    let mut rest = tag;
    while let Some((_, tail)) = rest.split_once("src=") {
        if let Some(value) = quoted(tail) {
            return Some(value);
        }
        rest = tail;
    }
    None
}

/// A `"..."` value at the start of `tail`, if the quote is there and closed.
fn quoted(tail: &str) -> Option<String> {
    let tail = tail.strip_prefix('"')?;
    tail.find('"').map(|end| tail[..end].to_string())
}

/// Web, API and script-CDN roots of SoundCloud.
#[derive(Clone, Debug, PartialEq)]
pub struct SoundCloudEndpoints {
    pub web_base: String,
    pub api_base: String,
    pub script_base: String,
}

impl SoundCloudEndpoints {
    pub fn production() -> SoundCloudEndpoints {
        SoundCloudEndpoints {
            web_base: "https://soundcloud.com".to_string(),
            api_base: "https://api-v2.soundcloud.com".to_string(),
            script_base: "https://a-v2.sndcdn.com".to_string(),
        }
    }
}

/// Fetches the web page and pulls the `client_id` out of its bundles.
///
/// Bundles are walked in reverse order of appearance, matching the way
/// `yt-dlp` looks for the id in the newest chunk first. A transport failure on
/// one bundle only logs and the walk continues; only the home page failing is
/// a hard error, because there is no way forward without it.
pub async fn scrape_client_id(
    client: &reqwest::Client,
    endpoints: &SoundCloudEndpoints,
) -> anyhow::Result<String> {
    let home_url = format!("{}/", endpoints.web_base);
    let home = client
        .get(&home_url)
        .send()
        .await
        .with_context(|| format!("не удалось скачать страницу {home_url}"))?
        .error_for_status()
        .with_context(|| format!("страница {home_url} ответила ошибкой"))?
        .text()
        .await
        .with_context(|| format!("не удалось прочитать тело страницы {home_url}"))?;

    for url in script_urls(&home).into_iter().rev() {
        let path = bundle_path(&url);
        let bundle_url = format!("{}{}", endpoints.script_base, path);
        let body = match client.get(&bundle_url).send().await {
            Ok(response) => match response.text().await {
                Ok(text) => text,
                Err(err) => {
                    tracing::debug!("soundcloud: скрипт {bundle_url} недоступен: {err}");
                    continue;
                }
            },
            Err(err) => {
                tracing::debug!("soundcloud: скрипт {bundle_url} недоступен: {err}");
                continue;
            }
        };
        if let Some(client_id) = find_client_id(&body) {
            return Ok(client_id);
        }
    }

    anyhow::bail!("soundcloud: client_id not found in the page bundles")
}

/// Everything from the first `/` after scheme and host: the path of a URL.
fn bundle_path(url: &str) -> &str {
    let after_scheme = url.split("://").nth(1).unwrap_or(url);
    match after_scheme.find('/') {
        Some(start) => &after_scheme[start..],
        None => "/",
    }
}

/// The `client_id` cached in `credentials.toml`, if a non-empty one is there.
///
/// A missing, unreadable or unparsable file simply means "no cache".
pub fn cached_client_id(dir: &Path) -> Option<String> {
    let client_id = Credentials::load_from(dir).ok()?.soundcloud?.client_id;
    if client_id.is_empty() {
        None
    } else {
        Some(client_id)
    }
}

/// Writes `client_id` into the `[soundcloud]` section of `credentials.toml`,
/// leaving every other credential, including `oauth_token`, untouched.
pub fn store_client_id(dir: &Path, client_id: &str) -> anyhow::Result<()> {
    let mut creds = Credentials::load_from(dir)?;
    let section = creds
        .soundcloud
        .get_or_insert_with(SoundCloudCredentials::default);
    section.client_id = client_id.to_string();
    creds.save_to(dir)
}

/// The cached id when there is one, a fresh scrape otherwise (then cached).
pub async fn ensure_client_id(
    dir: &Path,
    client: &reqwest::Client,
    endpoints: &SoundCloudEndpoints,
) -> anyhow::Result<String> {
    if let Some(client_id) = cached_client_id(dir) {
        return Ok(client_id);
    }
    let client_id = scrape_client_id(client, endpoints).await?;
    store_client_id(dir, &client_id)?;
    Ok(client_id)
}

/// The §8.9 `GET /users/{id}` profile that `yt-dlp` cannot give us.
#[derive(Clone, Debug, PartialEq)]
pub struct SoundCloudUser {
    pub id: u64,
    pub username: String,
    pub permalink: String,
    pub followers_count: u64,
    pub city: Option<String>,
    pub country: Option<String>,
}

/// Parses one SoundCloud user profile JSON body.
///
/// Extra fields are ignored on purpose: the real API returns far more than
/// the five fields this build reads.
pub fn parse_user(json: &str) -> anyhow::Result<SoundCloudUser> {
    let value: Value = serde_json::from_str(json)?;

    let id = value
        .get("id")
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow::anyhow!("у профиля SoundCloud нет поля id"))?;
    let username = value
        .get("username")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("у профиля SoundCloud нет поля username"))?
        .to_string();
    let permalink = value
        .get("permalink")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("у профиля SoundCloud нет поля permalink"))?
        .to_string();

    Ok(SoundCloudUser {
        id,
        username,
        permalink,
        followers_count: value
            .get("followers_count")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        city: value
            .get("city")
            .and_then(Value::as_str)
            .map(str::to_string),
        country: value
            .get("country")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

/// One `GET /users/{id}?client_id=...` call, parsed into a [`SoundCloudUser`].
pub async fn get_user(
    client: &reqwest::Client,
    endpoints: &SoundCloudEndpoints,
    client_id: &str,
    user_id: u64,
) -> anyhow::Result<SoundCloudUser> {
    let url = format!(
        "{}/users/{}?client_id={}",
        endpoints.api_base, user_id, client_id
    );
    let body = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("не удалось скачать профиль {url}"))?
        .error_for_status()
        .with_context(|| format!("профиль {url} ответил ошибкой"))?
        .text()
        .await
        .with_context(|| format!("не удалось прочитать тело профиля {url}"))?;
    parse_user(&body)
}
