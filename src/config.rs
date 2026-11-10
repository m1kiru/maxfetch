use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

fn default_library_path() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join("Music").join("maxfetch"))
        .unwrap_or_else(|| PathBuf::from("Music/maxfetch"))
}

fn default_sources_enabled() -> Vec<String> {
    ["soulseek", "bandcamp", "youtube", "spotify"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

fn default_duration_tolerance_sec() -> u32 {
    2
}

fn default_max_concurrency() -> usize {
    4
}

fn default_retry_attempts() -> u32 {
    3
}

fn default_timeout_sec() -> u64 {
    60
}

fn default_stall_timeout_sec() -> u64 {
    30
}

fn default_yt_dlp_path() -> String {
    "yt-dlp".to_string()
}

fn default_ffmpeg_path() -> String {
    "ffmpeg".to_string()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LibraryConfig {
    #[serde(default = "default_library_path")]
    pub path: PathBuf,
}

impl Default for LibraryConfig {
    fn default() -> Self {
        Self {
            path: default_library_path(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourcesConfig {
    #[serde(default = "default_sources_enabled")]
    pub enabled: Vec<String>,
}

impl Default for SourcesConfig {
    fn default() -> Self {
        Self {
            enabled: default_sources_enabled(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MatchingConfig {
    #[serde(default = "default_duration_tolerance_sec")]
    pub duration_tolerance_sec: u32,
    #[serde(default)]
    pub require_isrc: bool,
}

impl Default for MatchingConfig {
    fn default() -> Self {
        Self {
            duration_tolerance_sec: 2,
            require_isrc: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DownloadConfig {
    #[serde(default = "default_max_concurrency")]
    pub max_concurrency: usize,
    #[serde(default = "default_retry_attempts")]
    pub retry_attempts: u32,
    #[serde(default = "default_timeout_sec")]
    pub timeout_sec: u64,
    #[serde(default = "default_stall_timeout_sec")]
    pub stall_timeout_sec: u64,
}

impl Default for DownloadConfig {
    fn default() -> Self {
        Self {
            max_concurrency: 4,
            retry_attempts: 3,
            timeout_sec: 60,
            stall_timeout_sec: 30,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct YoutubeConfig {
    #[serde(default = "default_yt_dlp_path")]
    pub yt_dlp_path: String,
}

impl Default for YoutubeConfig {
    fn default() -> Self {
        Self {
            yt_dlp_path: "yt-dlp".to_string(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MediaConfig {
    #[serde(default = "default_ffmpeg_path")]
    pub ffmpeg_path: String,
}

impl Default for MediaConfig {
    fn default() -> Self {
        Self {
            ffmpeg_path: "ffmpeg".to_string(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub library: LibraryConfig,
    #[serde(default)]
    pub sources: SourcesConfig,
    #[serde(default)]
    pub matching: MatchingConfig,
    #[serde(default)]
    pub download: DownloadConfig,
    #[serde(default)]
    pub youtube: YoutubeConfig,
    #[serde(default)]
    pub media: MediaConfig,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SoulseekCredentials {
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BandcampCredentials {
    #[serde(default)]
    pub identity_cookie: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SpotifyCredentials {
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub client_secret: String,
    #[serde(default)]
    pub refresh_token: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SoundCloudCredentials {
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub oauth_token: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct QobuzCredentials {
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub app_id: String,
    #[serde(default)]
    pub app_secret: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TidalCredentials {
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub client_secret: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DeezerCredentials {
    #[serde(default)]
    pub arl: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BeatportCredentials {
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub client_secret: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Credentials {
    #[serde(default)]
    pub soulseek: Option<SoulseekCredentials>,
    #[serde(default)]
    pub bandcamp: Option<BandcampCredentials>,
    #[serde(default)]
    pub spotify: Option<SpotifyCredentials>,
    #[serde(default)]
    pub soundcloud: Option<SoundCloudCredentials>,
    #[serde(default)]
    pub qobuz: Option<QobuzCredentials>,
    #[serde(default)]
    pub tidal: Option<TidalCredentials>,
    #[serde(default)]
    pub deezer: Option<DeezerCredentials>,
    #[serde(default)]
    pub beatport: Option<BeatportCredentials>,
}

fn xdg_or_home(xdg: &str, home_fallback: PathBuf) -> PathBuf {
    match std::env::var(xdg) {
        Ok(v) if !v.is_empty() => PathBuf::from(v),
        _ => home_fallback,
    }
}

pub fn config_dir() -> PathBuf {
    let fallback = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/root"))
        .join(".config");
    xdg_or_home("XDG_CONFIG_HOME", fallback).join("maxfetch")
}

pub fn credentials_path() -> PathBuf {
    config_dir().join("credentials.toml")
}

pub fn state_path() -> PathBuf {
    let fallback = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("/root/.local/share"))
        .join("maxfetch")
        .join("state.db");
    match std::env::var("XDG_DATA_HOME") {
        Ok(v) if !v.is_empty() => PathBuf::from(v).join("maxfetch").join("state.db"),
        _ => fallback,
    }
}

impl Config {
    pub fn load_from(dir: &Path) -> anyhow::Result<Config> {
        let path = dir.join("config.toml");
        if !path.exists() {
            return Ok(Config::default());
        }
        let raw = fs::read_to_string(&path)?;
        toml::from_str(&raw)
            .map_err(|e| anyhow::anyhow!("failed to parse {}: {e}", path.display()))
    }

    pub fn save_to(&self, dir: &Path) -> anyhow::Result<()> {
        fs::create_dir_all(dir)?;
        let raw = toml::to_string_pretty(self)?;
        fs::write(dir.join("config.toml"), raw)?;
        Ok(())
    }
}

impl Credentials {
    pub fn load_from(dir: &Path) -> anyhow::Result<Credentials> {
        let path = dir.join("credentials.toml");
        if !path.exists() {
            return Ok(Credentials::default());
        }
        let raw = fs::read_to_string(&path)?;
        toml::from_str(&raw)
            .map_err(|e| anyhow::anyhow!("failed to parse {}: {e}", path.display()))
    }

    pub fn save_to(&self, dir: &Path) -> anyhow::Result<()> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;

        fs::create_dir_all(dir)?;
        let path = dir.join("credentials.toml");
        let raw = toml::to_string_pretty(self)?;
        // Create with 0600 from the start so secrets are never world-readable,
        // even briefly under a permissive umask.
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)?;
        file.write_all(raw.as_bytes())?;
        // Tighten pre-existing files that were created with loose permissions.
        ensure_private(&path)?;
        Ok(())
    }
}

/// Tighten `path` to mode 0600. Returns `true` if the mode had to change,
/// `false` when the file is already 0600 or absent.
pub fn ensure_private(path: &Path) -> anyhow::Result<bool> {
    use std::os::unix::fs::PermissionsExt;

    if !path.exists() {
        return Ok(false);
    }
    let mode = fs::metadata(path)?.permissions().mode() & 0o777;
    if mode == 0o600 {
        return Ok(false);
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(true)
}
