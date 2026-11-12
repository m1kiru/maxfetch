//! Streaming HTTP downloader.
//!
//! Downloads a response body to disk in chunks, reporting progress through a
//! [`ProgressSink`], and renames the temporary `.part` file into place only
//! after a complete transfer. An interrupted transfer never leaves a file
//! under the destination name.

use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use futures_util::StreamExt;
use tokio::io::AsyncWriteExt;

/// Browser-like User-Agent used for every request made through [`http_client`].
pub const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

/// Receives download progress notifications.
pub trait ProgressSink: Send + Sync {
    /// Called after every received chunk with the cumulative byte count and the
    /// advertised total size, when known.
    fn on_progress(&self, transferred: u64, total: Option<u64>);

    /// Called once, after the destination file has been renamed into place.
    fn on_finished(&self, transferred: u64);
}

/// A [`ProgressSink`] that ignores every notification.
#[derive(Clone, Debug, Default)]
pub struct NoopSink;

impl ProgressSink for NoopSink {
    fn on_progress(&self, _transferred: u64, _total: Option<u64>) {}

    fn on_finished(&self, _transferred: u64) {}
}

/// Streams the body of `url` into `dest`, reporting progress to `sink`.
///
/// Bytes are written to `dest.with_extension("part")` and renamed onto `dest`
/// only after the transfer completes. On failure the `.part` file is left in
/// place, `dest` does not appear and [`ProgressSink::on_finished`] is not
/// called.
pub async fn download_to_file(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    sink: &dyn ProgressSink,
) -> anyhow::Result<()> {
    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("request failed for {url}"))?
        .error_for_status()
        .with_context(|| format!("server returned an error for {url}"))?;

    let total = response.content_length();
    let part = dest.with_extension("part");

    if let Some(parent) = dest.parent()
        && !parent.as_os_str().is_empty()
    {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
    }

    let mut file = tokio::fs::File::create(&part)
        .await
        .with_context(|| format!("failed to create {}", part.display()))?;

    let mut transferred: u64 = 0;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.with_context(|| format!("failed to read body from {url}"))?;
        file.write_all(&bytes)
            .await
            .with_context(|| format!("failed to write {}", part.display()))?;
        transferred += bytes.len() as u64;
        sink.on_progress(transferred, total);
    }

    file.flush()
        .await
        .with_context(|| format!("failed to flush {}", part.display()))?;
    drop(file);

    tokio::fs::rename(&part, dest)
        .await
        .with_context(|| format!("failed to rename {} to {}", part.display(), dest.display()))?;

    sink.on_finished(transferred);
    Ok(())
}

/// Builds a `reqwest` client with a browser User-Agent, a cookie store and the
/// given request timeout.
pub fn http_client(timeout_sec: u64) -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(BROWSER_USER_AGENT)
        .cookie_store(true)
        .timeout(Duration::from_secs(timeout_sec))
        .build()
        .context("failed to build HTTP client")
}