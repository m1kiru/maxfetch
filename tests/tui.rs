use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use maxfetch::config::Config;
use maxfetch::matching::ReleaseGroup;
use maxfetch::model::{Release, SourceId};
use maxfetch::sources::{AuthState, Capabilities, SourceStatus};
use maxfetch::tui::{App, DownloadItem, LogEntry, Screen, ScrollState};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

#[test]
fn tab_switches_between_screens() {
    let mut app = App::new(Config::default());
    assert_eq!(app.screen, Screen::Search);
    app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
        .unwrap();
    assert_eq!(app.screen, Screen::Results);
}

#[test]
fn q_quits() {
    let mut app = App::new(Config::default());
    app.on_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE))
        .unwrap();
    assert!(app.should_quit);
}

#[test]
fn settings_screen_can_change_the_library_path() {
    let mut app = App::new(Config::default());
    app.screen = Screen::Settings;
    app.on_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE))
        .unwrap();
    app.set_library_path("/tmp/newlib").unwrap();
    assert_eq!(app.config.library.path, PathBuf::from("/tmp/newlib"));
}

#[test]
fn scroll_state_tracks_and_clamps_the_offset() {
    let mut scroll = ScrollState::new();
    assert_eq!(scroll.position(), 0);
    scroll.scroll_by(3, 10);
    assert_eq!(scroll.position(), 3);
    scroll.scroll_by(-10, 10);
    assert_eq!(scroll.position(), 0);
    scroll.scroll_by(100, 10);
    assert_eq!(scroll.position(), 9);
    scroll.reset();
    assert_eq!(scroll.position(), 0);
}

#[test]
fn changing_screens_resets_the_scroll() {
    let mut app = App::new(Config::default());
    app.screen = Screen::Downloads;
    app.downloads.push(DownloadItem {
        title: "Get Lucky".into(),
        source: SourceId::Soulseek,
        state: "качается".into(),
        transferred: 10,
        total: Some(100),
        speed: Some(10),
        detail: None,
    });
    app.downloads.push(DownloadItem {
        title: "One More Time".into(),
        source: SourceId::Qobuz,
        state: "в очереди".into(),
        transferred: 0,
        total: None,
        speed: None,
        detail: None,
    });
    app.scroll_downloads(1);
    assert_eq!(
        app.downloads_scroll.position(),
        1,
        "the downloads list scrolls with the list length"
    );
    app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
        .unwrap();
    assert_eq!(
        app.downloads_scroll.position(),
        0,
        "changing the screen resets the scroll"
    );
}

#[test]
fn shift_tab_moves_backwards_and_wraps() {
    let mut app = App::new(Config::default());
    assert_eq!(app.screen, Screen::Search);
    app.on_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT))
        .unwrap();
    assert_eq!(app.screen, Screen::Log);
    app.on_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT))
        .unwrap();
    assert_eq!(app.screen, Screen::Settings);
}

#[test]
fn a_new_search_resets_the_results_scroll() {
    let mut app = App::new(Config::default());
    app.results_scroll.scroll_by(3, 10);
    assert_eq!(app.results_scroll.position(), 3);
    // A new result set replaces the list and starts reading from the top.
    app.groups.clear();
    app.selected = 0;
    app.results_scroll.reset();
    assert_eq!(app.results_scroll.position(), 0);
}

#[test]
fn every_screen_renders_without_panicking() {
    let mut app = App::new(Config::default());
    app.groups.push(ReleaseGroup {
        primary: sample_release(),
        members: vec![sample_release()],
        sources: vec![SourceId::Soulseek],
    });
    app.downloads.push(DownloadItem {
        title: "Get Lucky".into(),
        source: SourceId::Soulseek,
        state: "качается".into(),
        transferred: 10,
        total: Some(100),
        speed: Some(10),
        detail: Some("пир: peer, слоты: 3".into()),
    });
    app.log.push(LogEntry {
        level: "ошибка".into(),
        message: "источник недоступен".into(),
    });
    app.status.push(SourceStatus {
        id: SourceId::Soulseek,
        capabilities: Capabilities {
            audio: true,
            metadata_only: false,
            lossless: true,
            needs_auth: false,
        },
        auth: AuthState::NotNeeded,
        detail: None,
    });

    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    for screen in [
        Screen::Search,
        Screen::Results,
        Screen::Candidates,
        Screen::Downloads,
        Screen::Sources,
        Screen::Settings,
        Screen::Log,
    ] {
        let mut app = app.clone();
        app.screen = screen;
        terminal
            .draw(|frame| maxfetch::tui::render(frame, &app))
            .unwrap();
    }
}

#[test]
fn sources_screen_shows_the_russian_auth_state() {
    let mut app = App::new(Config::default());
    app.screen = Screen::Sources;
    app.status = vec![SourceStatus {
        id: SourceId::Qobuz,
        capabilities: Capabilities {
            audio: true,
            metadata_only: false,
            lossless: true,
            needs_auth: true,
        },
        auth: AuthState::Missing,
        detail: None,
    }];

    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal
        .draw(|frame| maxfetch::tui::render(frame, &app))
        .unwrap();
    let text: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(
        text.contains("нет авторизации"),
        "the sources screen must render the Russian auth label, buffer was: {text:?}"
    );
}

#[test]
fn set_library_path_rejects_an_empty_path() {
    let mut app = App::new(Config::default());
    let before = app.config.library.path.clone();
    assert!(app.set_library_path("   ").is_err());
    assert_eq!(app.config.library.path, before);
}

fn sample_release() -> Release {
    Release {
        title: "Get Lucky".into(),
        artist: "Daft Punk".into(),
        album: None,
        track_no: None,
        duration_sec: None,
        isrc: None,
        year: None,
        source: SourceId::Soulseek,
        source_item_id: "r".into(),
    }
}
