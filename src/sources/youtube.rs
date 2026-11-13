//! YouTube as a `Source`: playlist/video search over `yt-dlp` and audio download.

use std::path::Path;

use async_trait::async_trait;

use anyhow::Context;

use crate::config::{MediaConfig, YoutubeConfig};
use crate::fetch::ProgressSink;
use crate::model::{Candidate, Locator, Release, SourceId};
use crate::sources::{AuthState, Capabilities, Query, Source};
use crate::youtube::{
    FinalizedAudio, YoutubeTrackMeta, YoutubeVideo, bestaudio_quality, download_audio, finalize,
    is_playlist_url, parse_flat_playlist, parse_video, run_json,
};

/// Splits `<artist> - <title>.<ext>` on the first literal ` - ` separator.
///
/// The extension is dropped first, both sides are trimmed, and an empty side
/// rejects the whole split. This is the only splitter YouTube uses;
/// `soulseek::release_from_filename` keeps its own fallback contract.
pub fn split_artist_title(filename: &str) -> Option<(String, String)> {
    let stem = match filename.rsplit_once('.') {
        Some((stem, _extension)) => stem,
        None => filename,
    };
    let (artist, title) = stem.split_once(" - ")?;
    let artist = artist.trim();
    let title = title.trim();
    if artist.is_empty() || title.is_empty() {
        None
    } else {
        Some((artist.to_string(), title.to_string()))
    }
}

/// YouTube as a [`Source`].
#[derive(Clone, Debug)]
pub struct YoutubeSource {
    yt_dlp_path: String,
    ffmpeg_path: String,
}

impl YoutubeSource {
    /// Copies the configured binary paths; nothing is read from disk.
    pub fn new(config: &YoutubeConfig, media: &MediaConfig) -> YoutubeSource {
        YoutubeSource {
            yt_dlp_path: config.yt_dlp_path.clone(),
            ffmpeg_path: media.ffmpeg_path.clone(),
        }
    }

    /// Resolves one URL — playlist or single video — into releases.
    fn releases_for_url(&self, url: &str) -> anyhow::Result<Vec<Release>> {
        if is_playlist_url(url) {
            let json = run_json(&self.yt_dlp_path, &["-J", "--flat-playlist", url])?;
            let videos = parse_flat_playlist(&json)?;
            return Ok(videos.iter().map(release_from_video).collect());
        }
        let json = run_json(&self.yt_dlp_path, &["-J", url])?;
        Ok(vec![release_from_json(&json)?])
    }
}

#[async_trait]
impl Source for YoutubeSource {
    fn id(&self) -> SourceId {
        SourceId::Youtube
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            audio: true,
            metadata_only: false,
            lossless: false,
            needs_auth: false,
        }
    }

    fn auth_state(&self) -> AuthState {
        AuthState::NotNeeded
    }

    async fn search(&self, q: &Query) -> anyhow::Result<Vec<Release>> {
        let url = q
            .url
            .as_deref()
            .map(str::trim)
            .filter(|url| !url.is_empty());
        // untested: no plan test covers a text query
        let Some(url) = url else {
            anyhow::bail!("YouTube не умеет искать по тексту: нужна ссылка");
        };
        self.releases_for_url(url)
    }

    async fn resolve(&self, q: &Query) -> anyhow::Result<Vec<Release>> {
        let url = q
            .url
            .as_deref()
            .map(str::trim)
            .filter(|url| !url.is_empty());
        let Some(url) = url else {
            anyhow::bail!("пустая ссылка YouTube");
        };
        self.releases_for_url(url)
    }

    async fn candidates(&self, release: &Release) -> anyhow::Result<Vec<Candidate>> {
        let url = format!("https://www.youtube.com/watch?v={}", release.source_item_id);
        let json = run_json(&self.yt_dlp_path, &["-J", &url])?;
        // `bestaudio_quality` has no `Result` and never fails; an item without
        // usable formats still yields the default lossy quality.
        let quality = bestaudio_quality(&json);
        Ok(vec![Candidate {
            source: SourceId::Youtube,
            release: release.clone(),
            quality,
            // The size is unknown until the format is chosen, so it is not invented.
            size_bytes: None,
            locator: Locator::Youtube {
                url,
                video_id: release.source_item_id.clone(),
            },
        }])
    }

    async fn download(
        &self,
        candidate: &Candidate,
        dest: &Path,
        sink: &dyn ProgressSink,
    ) -> anyhow::Result<()> {
        let Locator::Youtube { url, video_id } = &candidate.locator else {
            anyhow::bail!("кандидат не принадлежит YouTube");
        };

        let work_dir =
            std::env::temp_dir().join(format!("maxfetch-youtube-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&work_dir)
            .map_err(|error| anyhow::anyhow!("creating {}: {error}", work_dir.display()))?;

        let meta = YoutubeTrackMeta {
            title: candidate.release.title.clone(),
            artist: candidate.release.artist.clone(),
            album: candidate.release.album.clone(),
        };

        // `yt-dlp` and `ffmpeg` are blocking, so they run on the runtime's
        // blocking pool the same way `SoulseekSource` does it.
        let result = tokio::task::block_in_place(|| -> anyhow::Result<FinalizedAudio> {
            let downloaded = download_audio(&self.yt_dlp_path, video_id, url, &work_dir, sink)?;
            finalize(&self.ffmpeg_path, &downloaded, dest, video_id, &meta)
        });

        // The work directory is removed on both paths; a cleanup failure must
        // not mask the original outcome.
        let _ = std::fs::remove_dir_all(&work_dir);

        result.with_context(|| format!("скачивание с YouTube не удалось: {url}"))?;
        Ok(())
    }
}

/// Maps a flat playlist entry to a [`Release`].
fn release_from_video(video: &YoutubeVideo) -> Release {
    let (artist, title) = match split_artist_title(&video.title) {
        Some((artist, title)) => (artist, title),
        None => ("Unknown Artist".to_string(), video.title.clone()),
    };
    Release {
        title,
        artist,
        album: None,
        track_no: None,
        duration_sec: video.duration_sec,
        isrc: None,
        year: None,
        source: SourceId::Youtube,
        source_item_id: video.video_id.clone(),
    }
}

/// Maps a single `yt-dlp -J` video object to a [`Release`].
fn release_from_json(json: &serde_json::Value) -> anyhow::Result<Release> {
    let meta = parse_video(json)?;
    let video_id = json
        .get("id")
        .and_then(|value| value.as_str())
        .filter(|id| !id.is_empty())
        .ok_or_else(|| anyhow::anyhow!("yt-dlp вернул видео без id"))?;
    Ok(Release {
        title: meta.title,
        artist: meta.artist,
        album: meta.album,
        track_no: None,
        // The private `number` helper of `youtube` is not reachable here.
        duration_sec: json.get("duration").and_then(|value| value.as_f64()),
        isrc: None,
        year: None,
        source: SourceId::Youtube,
        source_item_id: video_id.to_string(),
    })
}
