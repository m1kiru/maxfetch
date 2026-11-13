use crate::sources::soundcloud::SoundCloudFormat;
use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SourceId {
    Soulseek,
    Bandcamp,
    Youtube,
    Spotify,
    SoundCloud,
    Qobuz,
    Tidal,
    Deezer,
    Beatport,
}

impl SourceId {
    pub fn as_str(&self) -> &'static str {
        match self {
            SourceId::Soulseek => "soulseek",
            SourceId::Bandcamp => "bandcamp",
            SourceId::Youtube => "youtube",
            SourceId::Spotify => "spotify",
            SourceId::SoundCloud => "soundcloud",
            SourceId::Qobuz => "qobuz",
            SourceId::Tidal => "tidal",
            SourceId::Deezer => "deezer",
            SourceId::Beatport => "beatport",
        }
    }

    pub fn from_str(s: &str) -> Option<SourceId> {
        match s {
            "soulseek" => Some(SourceId::Soulseek),
            "bandcamp" => Some(SourceId::Bandcamp),
            "youtube" => Some(SourceId::Youtube),
            "spotify" => Some(SourceId::Spotify),
            "soundcloud" => Some(SourceId::SoundCloud),
            "qobuz" => Some(SourceId::Qobuz),
            "tidal" => Some(SourceId::Tidal),
            "deezer" => Some(SourceId::Deezer),
            "beatport" => Some(SourceId::Beatport),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Container {
    Flac,
    Wav,
    Aiff,
    Alac,
    M4aAac,
    Mp3,
    Opus,
    Webm,
}

impl Container {
    pub fn as_str(&self) -> &'static str {
        match self {
            Container::Flac => "flac",
            Container::Wav => "wav",
            Container::Aiff => "aiff",
            Container::Alac => "alac",
            Container::M4aAac => "m4a",
            Container::Mp3 => "mp3",
            Container::Opus => "opus",
            Container::Webm => "webm",
        }
    }

    pub fn from_ext(ext: &str) -> Option<Container> {
        match ext.to_lowercase().as_str() {
            "flac" => Some(Container::Flac),
            "wav" => Some(Container::Wav),
            "aiff" => Some(Container::Aiff),
            "alac" => Some(Container::Alac),
            "m4a" | "mp4" => Some(Container::M4aAac),
            "mp3" => Some(Container::Mp3),
            "opus" => Some(Container::Opus),
            "webm" => Some(Container::Webm),
            _ => None,
        }
    }

    pub fn is_lossless(&self) -> bool {
        matches!(
            self,
            Container::Flac | Container::Wav | Container::Aiff | Container::Alac
        )
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Quality {
    pub lossless: bool,
    pub container: Container,
    pub bit_depth: Option<u8>,
    pub sample_rate: Option<u32>,
    pub bitrate_kbps: Option<u32>,
}

impl Default for Quality {
    fn default() -> Self {
        Quality {
            lossless: false,
            container: Container::Opus,
            bit_depth: None,
            sample_rate: None,
            bitrate_kbps: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Release {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub track_no: Option<u32>,
    pub duration_sec: Option<f64>,
    pub isrc: Option<String>,
    pub year: Option<i32>,
    pub source: SourceId,
    pub source_item_id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Locator {
    Soulseek {
        username: String,
        filename: String,
        size: u64,
    },
    Bandcamp {
        url: String,
        encoding: String,
    },
    Youtube {
        url: String,
        video_id: String,
    },
    Spotify {
        track_id: String,
    },
    SoundCloud {
        track_id: String,
        format: SoundCloudFormat,
    },
    Qobuz {
        track_id: String,
        format_id: u32,
    },
    Tidal {
        track_id: String,
        quality: u8,
    },
    Deezer {
        track_id: String,
    },
    Beatport {
        track_id: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub source: SourceId,
    pub release: Release,
    pub quality: Quality,
    pub size_bytes: Option<u64>,
    pub locator: Locator,
}
