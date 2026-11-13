//! Stage-1 wiring: one Russian status line for CLI and TUI, the playlist query
//! and the two shared TUI formatters.
//!
//! `run_with` touches `state_path()` and `credentials_path()`, so the CLI test
//! pins every XDG root into a `tempfile` directory exactly like `tests/cli.rs`.

#![allow(clippy::await_holding_lock)]

use std::fs;
use std::sync::{Mutex, MutexGuard};

use clap::Parser;

use maxfetch::auth::{describe_status, login};
use maxfetch::cli::{Cli, playlist_query};
use maxfetch::config::{Config, Credentials};
use maxfetch::model::SourceId;
use maxfetch::sources::{AuthState, Capabilities, registry, source_statuses};

/// Serialises the XDG mutation of the one test that runs the CLI.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Points all three XDG roots into `dir` and returns the lock held for the
/// whole test.
fn isolate_xdg(dir: &tempfile::TempDir) -> MutexGuard<'static, ()> {
    let guard = ENV_LOCK.lock().unwrap();
    let fake = dir.path().join("xdg");
    fs::create_dir_all(&fake).unwrap();
    // The returned guard is held by the caller for the whole test, so no other
    // test can mutate the environment (or read it via `dirs`) meanwhile.
    unsafe {
        std::env::set_var("XDG_DATA_HOME", &fake);
        std::env::set_var("XDG_CONFIG_HOME", &fake);
        std::env::set_var("XDG_STATE_HOME", &fake);
    }
    guard
}

#[test]
fn source_statuses_are_rendered_in_russian() {
    let mut cfg = Config::default();
    cfg.youtube.yt_dlp_path = "/nonexistent/yt-dlp".into();
    let statuses = source_statuses(&registry(&cfg, &Credentials::default()));
    let soulseek = statuses
        .iter()
        .find(|s| s.id == SourceId::Soulseek)
        .unwrap();
    let youtube = statuses.iter().find(|s| s.id == SourceId::Youtube).unwrap();
    assert_eq!(
        describe_status(soulseek),
        "soulseek: нет авторизации — нет данных для входа в Soulseek: секция [soulseek] в credentials.toml"
    );
    assert!(describe_status(youtube).starts_with("youtube: доступен — yt-dlp не найден"));
    let ok = maxfetch::sources::SourceStatus {
        id: SourceId::Soulseek,
        capabilities: Capabilities {
            audio: true,
            metadata_only: false,
            lossless: true,
            needs_auth: true,
        },
        auth: AuthState::Ok,
        detail: None,
    };
    assert_eq!(describe_status(&ok), "soulseek: доступен");
}

#[test]
fn a_playlist_argument_becomes_a_url_query() {
    let q = playlist_query("https://www.youtube.com/playlist?list=PL1");
    assert_eq!(
        q.url.as_deref(),
        Some("https://www.youtube.com/playlist?list=PL1")
    );
    assert!(q.items.is_empty());
    assert!(q.text.is_none());
}

#[tokio::test]
async fn sources_check_lists_the_three_sources_in_russian() {
    let dir = tempfile::tempdir().unwrap();
    let _env = isolate_xdg(&dir);
    let cfg = Config::load_from(dir.path()).unwrap();
    let creds = Credentials::default();
    let cli = Cli::parse_from(["maxfetch", "sources", "check"]);
    let mut out = Vec::new();
    maxfetch::cli::run_with(cli, cfg, creds, &mut out)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&out);
    assert!(text.contains("soulseek: нет авторизации"));
    assert!(text.contains("spotify: нет авторизации"));
    assert!(text.contains("youtube:"));
}

#[tokio::test]
async fn auth_login_stays_a_stub_for_the_stage_one_sources() {
    let mut creds = Credentials::default();
    for source in ["soulseek", "youtube", "spotify"] {
        let err = login(source, &mut creds).await.unwrap_err();
        assert!(format!("{err}").contains("not implemented"), "{source}");
    }
}

#[test]
fn the_sources_screen_starts_with_no_status_lines() {
    let app = maxfetch::tui::App::new(Config::default());
    assert!(app.source_rows().is_empty());
}

#[test]
fn an_active_transfer_names_its_peer_slots_and_speed() {
    let free = maxfetch::sources::soulseek::PeerCandidate {
        username: "alice".into(),
        filename: "f.flac".into(),
        size: 1,
        slots: 2,
        speed: 4,
        quality: maxfetch::model::Quality {
            lossless: true,
            container: maxfetch::model::Container::Flac,
            bit_depth: Some(16),
            sample_rate: Some(44100),
            bitrate_kbps: None,
        },
    };
    assert_eq!(
        maxfetch::tui::transfer_line(&free, "Get Lucky"),
        "Get Lucky: пир alice, слотов 2, скорость 4"
    );

    let busy = maxfetch::sources::soulseek::PeerCandidate { slots: 0, ..free };
    let line = maxfetch::tui::transfer_line(&busy, "Get Lucky");
    assert!(line.contains("недоступен"), "{line}");
    assert!(line.contains("alice"), "{line}");
}

#[test]
fn an_untagged_download_gets_its_own_badge() {
    let dir = tempfile::tempdir().unwrap();
    let webm = dir.path().join("clip.webm");
    std::fs::write(&webm, b"x").unwrap();
    let app = maxfetch::tui::App::new(Config::default());
    let lossy = maxfetch::model::Candidate {
        source: SourceId::Youtube,
        release: maxfetch::model::Release {
            title: "Get Lucky".into(),
            artist: "Daft Punk".into(),
            album: None,
            track_no: None,
            duration_sec: Some(369.0),
            isrc: None,
            year: None,
            source: SourceId::Youtube,
            source_item_id: "abc123".into(),
        },
        quality: maxfetch::model::Quality {
            lossless: false,
            container: maxfetch::model::Container::Opus,
            bit_depth: None,
            sample_rate: Some(48000),
            bitrate_kbps: Some(160),
        },
        size_bytes: Some(1),
        locator: maxfetch::model::Locator::Youtube {
            url: "https://www.youtube.com/watch?v=abc123".into(),
            video_id: "abc123".into(),
        },
    };
    assert_eq!(app.candidate_badge(&lossy, Some(&webm)), Some("LOSSY"));
    std::fs::write(dir.path().join("clip.webm.json"), b"{}").unwrap();
    assert_eq!(app.candidate_badge(&lossy, Some(&webm)), Some("UNTAGGED"));
    assert_eq!(app.candidate_badge(&lossy, None), Some("LOSSY"));
}
