use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use maxfetch::config::Config;
use maxfetch::matching::ReleaseGroup;
use maxfetch::model::{Candidate, Container, Locator, Quality, Release, SourceId};
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

// Fix round 1 ---------------------------------------------------------------

#[test]
fn scrolling_the_downloads_list_moves_the_visible_row() {
    let mut app = App::new(Config::default());
    app.screen = Screen::Downloads;
    for index in 0..30 {
        app.downloads
            .push(download_item(&format!("трек {index:02}")));
    }

    app.scroll_downloads(25);

    assert_eq!(
        app.downloads_selected,
        Some(25),
        "scrolling selects the row it moves to"
    );
    let text = draw_text(&app, 80, 24);
    assert!(
        text.contains("трек 25"),
        "the row scrolled to must be on screen, buffer was: {text:?}"
    );
    assert!(
        !text.contains("трек 00"),
        "the first row must have scrolled away, buffer was: {text:?}"
    );
}

#[test]
fn page_keys_move_the_results_selection() {
    let mut app = App::new(Config::default());
    app.screen = Screen::Results;
    app.groups = (0..30)
        .map(|index| release_group(&format!("группа {index:02}")))
        .collect();

    app.on_key(key(KeyCode::PageDown)).unwrap();
    assert_eq!(app.selected, 10, "PageDown moves the selection by a page");
    app.on_key(key(KeyCode::PageUp)).unwrap();
    assert_eq!(app.selected, 0, "PageUp walks the selection back");
    app.on_key(key(KeyCode::End)).unwrap();
    assert_eq!(app.selected, 29, "End selects the last result");
    app.on_key(key(KeyCode::Home)).unwrap();
    assert_eq!(app.selected, 0, "Home selects the first result");
}

#[test]
fn page_keys_move_the_candidates_selection() {
    let mut app = App::new(Config::default());
    app.screen = Screen::Candidates;
    app.candidates = (0..30)
        .map(|index| sample_candidate(&format!("c{index:02}")))
        .collect();

    app.on_key(key(KeyCode::PageDown)).unwrap();
    assert_eq!(app.selected, 10, "PageDown moves the candidate selection");
    app.on_key(key(KeyCode::End)).unwrap();
    assert_eq!(app.selected, 29, "End selects the last candidate");
}

#[test]
fn the_candidates_screen_names_the_expanded_group() {
    let mut app = App::new(Config::default());
    app.groups = vec![
        group_named("Daft Punk", "One More Time"),
        group_named("Daft Punk", "Get Lucky"),
    ];
    // The Candidates screen keeps the candidate cursor in `selected`, so only
    // `expanded` can say which release is on screen.
    app.expanded = Some(1);
    app.selected = 0;
    app.candidates = vec![sample_candidate("Get Lucky")];
    app.screen = Screen::Candidates;

    let text = draw_text(&app, 80, 24);

    assert!(
        text.contains("Get Lucky"),
        "the title must name the expanded group, buffer was: {text:?}"
    );
    assert!(
        !text.contains("One More Time"),
        "the first group must not leak into the title, buffer was: {text:?}"
    );
}

#[test]
fn the_candidates_screen_spells_the_container_as_in_the_spec() {
    let mut app = App::new(Config::default());
    app.groups = vec![group_named("Daft Punk", "Get Lucky")];
    app.expanded = Some(0);
    app.selected = 0;
    app.candidates = vec![Candidate {
        quality: Quality {
            lossless: false,
            container: Container::Opus,
            bit_depth: None,
            sample_rate: Some(48000),
            bitrate_kbps: Some(320),
        },
        ..sample_candidate("Get Lucky")
    }];
    app.screen = Screen::Candidates;

    let text = draw_text(&app, 90, 24);

    assert!(
        text.contains("320 kbps Opus"),
        "the container keeps its documented case, buffer was: {text:?}"
    );
}

#[test]
fn the_downloads_gauge_survives_a_transfer_above_the_total() {
    let mut app = App::new(Config::default());
    app.screen = Screen::Downloads;
    app.downloads.push(DownloadItem {
        title: "Get Lucky".into(),
        source: SourceId::Soulseek,
        state: "качается".into(),
        transferred: 200,
        total: Some(100),
        speed: Some(10),
        detail: None,
    });

    // `Gauge::ratio` asserts `0.0..=1.0`, so an over-long transfer must be clamped.
    let text = draw_text(&app, 80, 24);
    assert!(text.contains("Get Lucky"), "the row is drawn");
}

#[test]
fn the_results_scrollbar_appears_for_two_line_rows() {
    // Four groups are eight rows: on a five-row panel the list is clipped even
    // though the group count alone fits the height.
    let mut app = App::new(Config::default());
    app.screen = Screen::Results;
    app.groups = (0..4)
        .map(|index| release_group(&format!("группа {index}")))
        .collect();

    let text = draw_text(&app, 80, 10);

    assert!(
        text.contains('▲') || text.contains('█') || text.contains('║'),
        "the scrollbar must be drawn, buffer was: {text:?}"
    );
}

#[test]
fn settings_navigation_does_not_touch_the_results_scroll() {
    let mut app = App::new(Config::default());
    app.screen = Screen::Settings;
    // One-row viewport: the old `reveal_selection` would pin the offset to the
    // Settings cursor even though Settings draws no results panel.
    app.results_scroll.viewport = 1;
    app.selected = 0;

    app.on_key(key(KeyCode::Down)).unwrap();

    assert_eq!(app.selected, 1, "the Settings cursor moved");
    assert_eq!(
        app.results_scroll.position(),
        0,
        "Settings must not scroll the results panel"
    );
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn draw_text(app: &App, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| maxfetch::tui::render(frame, app))
        .unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

fn download_item(title: &str) -> DownloadItem {
    DownloadItem {
        title: title.into(),
        source: SourceId::Soulseek,
        state: "качается".into(),
        transferred: 0,
        total: None,
        speed: None,
        detail: None,
    }
}

fn release_group(title: &str) -> ReleaseGroup {
    group_named("Daft Punk", title)
}

fn group_named(artist: &str, title: &str) -> ReleaseGroup {
    ReleaseGroup {
        primary: Release {
            artist: artist.into(),
            ..sample_release_with(title)
        },
        members: Vec::new(),
        sources: vec![SourceId::Soulseek],
    }
}

fn sample_candidate(title: &str) -> Candidate {
    Candidate {
        source: SourceId::Soulseek,
        release: sample_release_with(title),
        quality: Quality {
            lossless: true,
            container: Container::Flac,
            bit_depth: Some(24),
            sample_rate: Some(48000),
            bitrate_kbps: None,
        },
        size_bytes: Some(1024),
        locator: Locator::Soulseek {
            username: "peer".into(),
            filename: "track.flac".into(),
            size: 1024,
        },
    }
}

fn sample_release_with(title: &str) -> Release {
    Release {
        title: title.into(),
        ..sample_release()
    }
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
