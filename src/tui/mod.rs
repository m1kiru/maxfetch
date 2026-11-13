//! Interactive terminal shell over the fetch pipeline.
//!
//! [`App`] is input state only: it never touches the network or the disk, so
//! every screen can be rendered and every key pressed from a test. Work that
//! needs the pipeline is expressed as an [`Action`] and performed by the
//! [`run`] event loop, which owns the [`Pipeline`].

mod views;

pub use views::downloads::transfer_line;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, anyhow};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures_util::StreamExt;
use ratatui::crossterm::event::EventStream;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, Paragraph, Tabs};
use ratatui::{DefaultTerminal, Frame};

use crate::config::{self, Config, Credentials};
use crate::db::Db;
use crate::fetch::NoopSink;
use crate::matching::ReleaseGroup;
use crate::model::{Candidate, SourceId};
use crate::pipeline::Pipeline;
use crate::sources::{self, Query, SourceStatus};

/// How many log lines the in-memory journal keeps.
const LOG_LIMIT: usize = 500;

/// The screens of the shell, in `Tab` order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Search,
    Results,
    Candidates,
    Downloads,
    Sources,
    Settings,
    Log,
}

impl Screen {
    /// Every screen, in `Tab` order.
    const ALL: [Screen; 7] = [
        Screen::Search,
        Screen::Results,
        Screen::Candidates,
        Screen::Downloads,
        Screen::Sources,
        Screen::Settings,
        Screen::Log,
    ];

    /// Russian tab title (user-facing).
    pub fn title(self) -> &'static str {
        match self {
            Screen::Search => "Поиск",
            Screen::Results => "Результаты",
            Screen::Candidates => "Кандидаты",
            Screen::Downloads => "Загрузки",
            Screen::Sources => "Источники",
            Screen::Settings => "Настройки",
            Screen::Log => "Журнал",
        }
    }

    /// Position of the screen in [`Screen::ALL`].
    fn ordinal(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }

    /// Next screen, wrapping around (the `Tab` order).
    pub fn next(self) -> Screen {
        Self::ALL[(self.ordinal() + 1) % Self::ALL.len()]
    }

    /// Previous screen, wrapping around (`Shift+Tab`).
    pub fn previous(self) -> Screen {
        Self::ALL[(self.ordinal() + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

/// Scroll offset of one panel, clamped against the length of its list.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScrollState {
    pub offset: usize,
    /// Panel height in rows; written by the event loop, read by the views.
    pub viewport: usize,
}

impl ScrollState {
    /// A scroll state at the top of an empty list.
    pub fn new() -> ScrollState {
        ScrollState {
            offset: 0,
            viewport: 0,
        }
    }

    /// Returns to the top of the list; the viewport is left alone.
    pub fn reset(&mut self) {
        self.offset = 0;
    }

    /// Moves by `delta` rows, clamped to `0..=len - 1`.
    pub fn scroll_by(&mut self, delta: isize, len: usize) {
        let limit = len.saturating_sub(1);
        let next = if delta.is_negative() {
            self.offset.saturating_sub(delta.unsigned_abs())
        } else {
            self.offset.saturating_add(delta as usize)
        };
        self.offset = next.min(limit);
    }

    /// Offset as the `u16` the ratatui scrollbar state takes.
    pub fn position(&self) -> u16 {
        u16::try_from(self.offset).unwrap_or(u16::MAX)
    }
}

/// One row of the Downloads screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DownloadItem {
    pub title: String,
    pub source: SourceId,
    pub state: String,
    pub transferred: u64,
    pub total: Option<u64>,
    pub speed: Option<u64>,
    pub detail: Option<String>,
}

/// One line of the in-memory journal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogEntry {
    pub level: String,
    pub message: String,
}

/// Work the event loop must perform; `on_key` is synchronous, the pipeline is not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Search,
    Expand,
    CheckSources,
    DownloadMarked,
    DownloadBest,
    SaveConfig,
}

/// Everything the shell knows: the screens read it, the event loop fills it.
#[derive(Clone, Debug)]
pub struct App {
    pub screen: Screen,
    pub query: String,
    pub groups: Vec<ReleaseGroup>,
    pub selected: usize,
    /// Index into `groups` of the release shown on the Candidates screen.
    ///
    /// `selected` is the candidate cursor while Candidates is on screen, so the
    /// expanded release needs an index of its own.
    pub expanded: Option<usize>,
    pub candidates: Vec<Candidate>,
    pub downloads: Vec<DownloadItem>,
    pub log: Vec<LogEntry>,
    pub config: Config,
    pub status: Vec<SourceStatus>,
    pub results_scroll: ScrollState,
    pub downloads_scroll: ScrollState,
    /// Highlighted Downloads row; `None` until the list has an item to show.
    pub downloads_selected: Option<usize>,
    pub should_quit: bool,
    pub(crate) marked: BTreeSet<usize>,
    pub(crate) pending: Vec<Action>,
    pub(crate) editing: bool,
    pub(crate) dirty: bool,
    pub(crate) help: bool,
}

impl App {
    /// A fresh shell on the Search screen; performs no I/O.
    pub fn new(config: Config) -> App {
        App {
            screen: Screen::Search,
            query: String::new(),
            groups: Vec::new(),
            selected: 0,
            expanded: None,
            candidates: Vec::new(),
            downloads: Vec::new(),
            log: Vec::new(),
            config,
            status: Vec::new(),
            results_scroll: ScrollState::new(),
            downloads_scroll: ScrollState::new(),
            downloads_selected: None,
            should_quit: false,
            marked: BTreeSet::new(),
            pending: Vec::new(),
            editing: false,
            dirty: false,
            help: false,
        }
    }

    /// One `auth::describe_status` line per entry of `self.status`.
    pub fn source_rows(&self) -> Vec<String> {
        self.status
            .iter()
            .map(crate::auth::describe_status)
            .collect()
    }

    /// `UNTAGGED` for a delivered YouTube file that kept its sidecar, `LOSSY`
    /// for another lossy candidate, `None` for a lossless one.
    ///
    /// The final path of a download is only known to the caller that performed
    /// it, so `downloaded` is `None` wherever the badge is drawn from the
    /// candidate alone.
    pub fn candidate_badge(
        &self,
        candidate: &Candidate,
        downloaded: Option<&Path>,
    ) -> Option<&'static str> {
        if let Some(path) = downloaded
            && crate::youtube::untagged(path)
        {
            return Some("UNTAGGED");
        }
        if !candidate.quality.lossless {
            Some("LOSSY")
        } else {
            None
        }
    }

    /// Handles one key press; performs no I/O and no network access.
    pub fn on_key(&mut self, key: KeyEvent) -> Result<()> {
        if key.kind != KeyEventKind::Press {
            return Ok(());
        }

        // Global keys come first: they work on every screen.
        match key.code {
            KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.switch_screen(self.screen.previous());
                return Ok(());
            }
            KeyCode::BackTab => {
                self.switch_screen(self.screen.previous());
                return Ok(());
            }
            KeyCode::Tab => {
                self.switch_screen(self.screen.next());
                return Ok(());
            }
            KeyCode::Char('q') => {
                self.should_quit = true;
                return Ok(());
            }
            KeyCode::Char('?') => {
                self.help = !self.help;
                return Ok(());
            }
            KeyCode::Char('s') => {
                self.switch_screen(Screen::Settings);
                return Ok(());
            }
            _ => {}
        }

        if self.editing {
            match key.code {
                KeyCode::Char(c) => self.query.push(c),
                KeyCode::Backspace => {
                    self.query.pop();
                }
                KeyCode::Esc => {
                    self.editing = false;
                    self.query.clear();
                }
                KeyCode::Enter => self.commit_edit(),
                _ => {}
            }
            return Ok(());
        }

        match self.screen {
            Screen::Search => match key.code {
                KeyCode::Char('/') => self.editing = true,
                KeyCode::Char(c) => {
                    self.query.push(c);
                    self.editing = true;
                }
                KeyCode::Backspace => {
                    self.query.pop();
                }
                KeyCode::Enter => self.pending.push(Action::Search),
                _ => {}
            },
            Screen::Results => match key.code {
                KeyCode::Up => self.move_results_selection(-1),
                KeyCode::Down => self.move_results_selection(1),
                KeyCode::PageUp => {
                    let step = self.page_step();
                    self.scroll_results(-step);
                }
                KeyCode::PageDown => {
                    let step = self.page_step();
                    self.scroll_results(step);
                }
                KeyCode::Home => {
                    let len = self.groups.len() as isize;
                    self.scroll_results(-len);
                }
                KeyCode::End => {
                    let len = self.groups.len() as isize;
                    self.scroll_results(len);
                }
                KeyCode::Enter => self.pending.push(Action::Expand),
                KeyCode::Char(' ') => self.toggle_mark(),
                KeyCode::Char('d') => self.pending.push(Action::DownloadMarked),
                KeyCode::Char('a') => self.pending.push(Action::DownloadBest),
                _ => {}
            },
            Screen::Candidates => match key.code {
                KeyCode::Up => self.move_selection(self.candidates.len(), -1),
                KeyCode::Down => self.move_selection(self.candidates.len(), 1),
                KeyCode::PageUp => {
                    let step = self.page_step();
                    self.scroll_results(-step);
                }
                KeyCode::PageDown => {
                    let step = self.page_step();
                    self.scroll_results(step);
                }
                KeyCode::Home => {
                    let len = self.candidates.len() as isize;
                    self.scroll_results(-len);
                }
                KeyCode::End => {
                    let len = self.candidates.len() as isize;
                    self.scroll_results(len);
                }
                KeyCode::Enter | KeyCode::Char('d') => {
                    self.pending.push(Action::DownloadMarked);
                }
                KeyCode::Esc => {
                    // Collapsing forgets which release was open.
                    self.expanded = None;
                    self.switch_screen(Screen::Results);
                }
                _ => {}
            },
            Screen::Downloads => match key.code {
                KeyCode::Up => self.scroll_downloads(-1),
                KeyCode::Down => self.scroll_downloads(1),
                KeyCode::PageUp => {
                    let step = self.page_step();
                    self.scroll_downloads(-step);
                }
                KeyCode::PageDown => {
                    let step = self.page_step();
                    self.scroll_downloads(step);
                }
                KeyCode::Home => {
                    let len = self.downloads.len() as isize;
                    self.scroll_downloads(-len);
                }
                KeyCode::End => {
                    let len = self.downloads.len() as isize;
                    self.scroll_downloads(len);
                }
                _ => {}
            },
            Screen::Sources => {
                if key.code == KeyCode::Char('c') {
                    self.pending.push(Action::CheckSources);
                }
            }
            Screen::Settings => match key.code {
                KeyCode::Char('e') => {
                    self.query = self.config.library.path.display().to_string();
                    self.editing = true;
                }
                KeyCode::Up => self.move_settings_selection(-1),
                KeyCode::Down => self.move_settings_selection(1),
                KeyCode::Char(' ') => self.toggle_source(),
                _ => {}
            },
            Screen::Log => match key.code {
                KeyCode::Up => self.move_selection(self.log.len(), -1),
                KeyCode::Down => self.move_selection(self.log.len(), 1),
                _ => {}
            },
        }

        Ok(())
    }

    /// Points the library at `path`; the file is written when Settings is left.
    pub fn set_library_path(&mut self, path: &str) -> Result<()> {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            return Err(anyhow!("library path must not be empty"));
        }
        self.config.library.path = PathBuf::from(trimmed);
        self.dirty = true;
        Ok(())
    }

    /// Scrolls the results panel by moving its selection: `ratatui` derives the
    /// first visible row from the selected index, so an offset alone is ignored.
    pub fn scroll_results(&mut self, delta: isize) {
        if self.screen == Screen::Candidates {
            self.move_selection(self.candidates.len(), delta);
        } else {
            self.move_results_selection(delta);
        }
    }

    /// Scrolls the downloads panel by moving its highlight.
    pub fn scroll_downloads(&mut self, delta: isize) {
        let len = self.downloads.len();
        if len == 0 {
            self.downloads_selected = None;
            self.downloads_scroll.offset = 0;
            return;
        }
        let current = self.downloads_selected.unwrap_or(0);
        let next = if delta.is_negative() {
            current.saturating_sub(delta.unsigned_abs())
        } else {
            current.saturating_add(delta as usize)
        };
        let next = next.min(len - 1);
        self.downloads_selected = Some(next);
        // The gauge and the scrollbar follow the highlight rather than their own
        // offset, so both stay on the row the user is looking at.
        self.downloads_scroll.offset = next;
    }

    /// Records a journal line; the newest entry stays first.
    pub(crate) fn log_line(&mut self, level: &str, message: impl Into<String>) {
        self.log.insert(
            0,
            LogEntry {
                level: level.to_string(),
                message: message.into(),
            },
        );
        self.log.truncate(LOG_LIMIT);
    }

    /// Changes the screen and starts both panels from the top.
    fn switch_screen(&mut self, screen: Screen) {
        if self.screen == Screen::Settings && screen != Screen::Settings && self.dirty {
            self.pending.push(Action::SaveConfig);
        }
        self.screen = screen;
        self.results_scroll.reset();
        self.downloads_scroll.reset();
        // Both panels start from the top; the Downloads highlight follows the
        // offset so the gauge and the list cannot disagree.
        self.downloads_selected = if self.downloads.is_empty() {
            None
        } else {
            Some(0)
        };
    }

    /// Appends a Downloads row and keeps the panel highlight inside the list.
    fn push_download(&mut self, item: DownloadItem) {
        self.downloads.push(item);
        let len = self.downloads.len();
        let selected = self.downloads_selected.unwrap_or(len - 1).min(len - 1);
        self.downloads_selected = Some(selected);
        // Keep the public scroll state a mirror of the highlight: the list itself
        // derives its first visible row from the selection.
        self.downloads_scroll.offset = selected;
    }

    /// `Space` on the Results screen: mark or unmark the selected group.
    fn toggle_mark(&mut self) {
        if !self.marked.remove(&self.selected) {
            self.marked.insert(self.selected);
        }
    }

    /// Ends the text-editing mode: commit on Settings, search everywhere else.
    fn commit_edit(&mut self) {
        self.editing = false;
        if self.screen != Screen::Settings {
            self.pending.push(Action::Search);
            return;
        }
        // The new path becomes an unsaved change; `switch_screen` writes it when
        // Settings is left, so Enter alone never touches the real config file.
        let path = self.query.clone();
        match self.set_library_path(&path) {
            Ok(()) => self.query.clear(),
            Err(err) => {
                tracing::warn!(error = %err, "library path rejected");
                self.log_line("ошибка", err.to_string());
            }
        }
    }

    /// Moves the selection inside `0..len`.
    ///
    /// The panels derive their first visible row from this index while rendering
    /// (`ratatui` re-computes it from `selected`), so navigation touches no
    /// scroll state — Settings and Log therefore cannot move the results panel.
    fn move_selection(&mut self, len: usize, delta: isize) {
        if len == 0 {
            self.selected = 0;
            return;
        }
        let next = if delta.is_negative() {
            self.selected.saturating_sub(delta.unsigned_abs())
        } else {
            self.selected.saturating_add(delta as usize)
        };
        self.selected = next.min(len - 1);
    }

    /// `Up`/`Down` on Results: moving the cursor forgets the opened release.
    fn move_results_selection(&mut self, delta: isize) {
        self.move_selection(self.groups.len(), delta);
        self.expanded = None;
    }

    /// Rows per `PageUp`/`PageDown`: one screen less an overlap row, else ten.
    fn page_step(&self) -> isize {
        match self.results_scroll.viewport {
            viewport if viewport > 1 => (viewport - 1) as isize,
            _ => 10,
        }
    }

    /// Moves the Settings selection over the source toggles.
    fn move_settings_selection(&mut self, delta: isize) {
        let len = source_names(&self.config).len();
        self.move_selection(len, delta);
    }

    /// `Space` on the Settings screen: flip one source in `config.sources.enabled`.
    fn toggle_source(&mut self) {
        let names = source_names(&self.config);
        let Some(name) = names.get(self.selected).cloned() else {
            return;
        };
        match self.config.sources.enabled.iter().position(|n| *n == name) {
            Some(index) => {
                self.config.sources.enabled.remove(index);
            }
            None => self.config.sources.enabled.push(name),
        }
        self.dirty = true;
    }
}

/// Source names offered as toggles on the Settings screen.
///
/// `SourceId::ALL` arrives with the authentication stage; until then the list
/// is every id this build already knows plus any extra names the config
/// carries. Selection itself stays in [`sources::enabled_in`].
pub(crate) fn source_names(config: &Config) -> Vec<String> {
    const KNOWN: [&str; 9] = [
        "soulseek",
        "bandcamp",
        "youtube",
        "spotify",
        "soundcloud",
        "qobuz",
        "tidal",
        "deezer",
        "beatport",
    ];
    let mut names: Vec<String> = KNOWN.iter().map(|name| (*name).to_string()).collect();
    for name in &config.sources.enabled {
        if !names.contains(name) {
            names.push(name.clone());
        }
    }
    names
}

/// Draws the tabs, the current screen, the footer and the help overlay.
pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let [tabs_area, body, footer_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(2),
    ])
    .areas(area);

    let titles: Vec<Line> = Screen::ALL
        .iter()
        .map(|screen| Line::from(screen.title()))
        .collect();
    let tabs = Tabs::new(titles)
        .select(app.screen.ordinal())
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    frame.render_widget(tabs, tabs_area);

    match app.screen {
        Screen::Search => {
            let [input_area, rest] =
                Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).areas(body);
            let field = if app.editing {
                format!("Запрос: {}_", app.query)
            } else {
                format!("Запрос: {}", app.query)
            };
            let input = Paragraph::new(field).block(Block::bordered().title("Поиск"));
            frame.render_widget(input, input_area);
            views::search::render(frame, rest, app);
        }
        Screen::Results => views::results::render(frame, body, app),
        Screen::Candidates => views::candidates::render(frame, body, app),
        Screen::Downloads => views::downloads::render(frame, body, app),
        Screen::Sources => views::sources::render(frame, body, app),
        Screen::Settings => views::settings::render(frame, body, app),
        Screen::Log => views::log::render(frame, body, app),
    }

    let footer = vec![
        Line::from(
            "Tab/Shift+Tab — экраны · / — поиск · Enter — раскрыть · Space — отметить · d — скачать · a — лучшее · s — настройки · ? — помощь · q — выход",
        ),
        Line::from(format!(
            "Библиотека: {} · источников включено: {}",
            app.config.library.path.display(),
            app.config.sources.enabled.len()
        )),
    ];
    frame.render_widget(Paragraph::new(footer), footer_area);

    if app.help {
        render_help(frame, area);
    }
}

/// The `?` overlay: every key on one card.
fn render_help(frame: &mut Frame, area: Rect) {
    let [_, middle, _] = Layout::vertical([
        Constraint::Percentage(15),
        Constraint::Percentage(70),
        Constraint::Percentage(15),
    ])
    .areas(area);
    let [_, card, _] = Layout::horizontal([
        Constraint::Percentage(15),
        Constraint::Percentage(70),
        Constraint::Percentage(15),
    ])
    .areas(middle);

    let keys = vec![
        Line::from("Tab / Shift+Tab — переключение экранов"),
        Line::from("/ — ввод запроса, Enter — поиск или раскрытие"),
        Line::from("Space — отметить релиз, d — скачать отмеченное"),
        Line::from("a — скачать лучшее, c — проверить источники"),
        Line::from("e — правка пути библиотеки, s — настройки"),
        Line::from("? — эта справка, q — выход"),
    ];
    frame.render_widget(Clear, card);
    frame.render_widget(
        Paragraph::new(keys).block(Block::bordered().title("Помощь")),
        card,
    );
}

/// Runs the shell until the user quits.
pub async fn run(config: Config) -> Result<()> {
    // These calls stay blocking on purpose: `Db` is not `Sync`, so they cannot
    // move to `spawn_blocking` without reworking the database layer.
    let credentials = Credentials::load_from(&config::config_dir())?;
    let db = Db::open(&config::state_path())?;
    let pipeline = Pipeline::new(config.clone(), db, sources::registry(&config, &credentials));
    let mut app = App::new(config);
    app.status = pipeline.source_statuses();

    // `try_init` reports failures instead of panicking; `restore` runs on both
    // the success and the error path so the terminal never stays in raw mode.
    let mut terminal = ratatui::try_init()?;
    let outcome = event_loop(&mut terminal, &mut app, &pipeline).await;
    ratatui::restore();
    outcome
}

async fn event_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    pipeline: &Pipeline,
) -> Result<()> {
    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(200));

    loop {
        terminal.draw(|frame| render(frame, app))?;
        // The panels do not know their height until the terminal is created, so
        // the event loop writes it: tabs take one row, the footer takes two.
        let viewport = terminal.size()?.height.saturating_sub(3) as usize;
        app.results_scroll.viewport = viewport;
        app.downloads_scroll.viewport = viewport;

        for action in std::mem::take(&mut app.pending) {
            apply_action(app, pipeline, action).await;
        }

        tokio::select! {
            event = events.next() => match event {
                Some(Ok(Event::Key(key))) => app.on_key(key)?,
                Some(Ok(_)) => {}
                Some(Err(err)) => tracing::warn!(error = %err, "terminal event stream failed"),
                None => break,
            },
            _ = tick.tick() => {}
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}

/// Performs one queued action; failures are logged, never fatal.
async fn apply_action(app: &mut App, pipeline: &Pipeline, action: Action) {
    match action {
        Action::Search => {
            let query = Query {
                text: Some(app.query.clone()),
                url: None,
                items: Vec::new(),
            };
            match pipeline.search(&query).await {
                Ok(groups) => {
                    let found = groups.len();
                    app.groups = groups;
                    app.selected = 0;
                    app.expanded = None;
                    app.candidates.clear();
                    app.marked.clear();
                    app.results_scroll.reset();
                    app.log_line("поиск", format!("найдено групп: {found}"));
                }
                Err(err) => {
                    tracing::warn!(error = %err, "search failed");
                    app.log_line("ошибка", err.to_string());
                }
            }
        }
        Action::Expand => {
            let Some(group) = app.groups.get(app.selected).cloned() else {
                app.log_line("ошибка", "нечего раскрывать");
                return;
            };
            let index = app.selected;
            match pipeline.candidates(&group).await {
                Ok(candidates) => {
                    let found = candidates.len();
                    app.candidates = candidates;
                    // The screen keeps its own index of the open release: the
                    // candidate cursor takes over `selected` from here on.
                    app.expanded = Some(index);
                    app.selected = 0;
                    app.results_scroll.reset();
                    app.screen = Screen::Candidates;
                    app.log_line("кандидаты", format!("кандидатов: {found}"));
                }
                Err(err) => {
                    tracing::warn!(error = %err, "candidates failed");
                    app.log_line("ошибка", err.to_string());
                }
            }
        }
        Action::CheckSources => {
            app.status = pipeline.source_statuses();
            let found = app.status.len();
            app.log_line("источники", format!("проверено источников: {found}"));
        }
        Action::DownloadMarked => download(app, pipeline, false).await,
        Action::DownloadBest => download(app, pipeline, true).await,
        Action::SaveConfig => match app.config.save_to(&config::config_dir()) {
            Ok(()) => {
                app.dirty = false;
                app.log_line("настройки", "конфигурация сохранена");
            }
            Err(err) => {
                tracing::warn!(error = %err, "saving the config failed");
                app.log_line("ошибка", err.to_string());
            }
        },
    }
}

/// Downloads the marked groups, or the selection, or the candidates on screen.
///
/// `best_only` never narrows the candidate list: [`Pipeline::download`] verifies
/// every candidate in the order it is given and falls back to the next one, so a
/// truncated list would turn recoverable failures into "не добыто". The flag only
/// decides which *groups* are fetched — the selection alone instead of the marks.
async fn download(app: &mut App, pipeline: &Pipeline, best_only: bool) {
    if app.screen == Screen::Candidates {
        // `selected` is the candidate cursor here; the open release lives in
        // `expanded`, and every candidate of that group is a fallback.
        let Some(group) = app.expanded.and_then(|index| app.groups.get(index)) else {
            app.log_line("ошибка", "нечего качать");
            return;
        };
        let title = format!("{} — {}", group.primary.artist, group.primary.title);
        let source = group.primary.source;
        let candidates = app.candidates.clone();
        fetch_and_store(app, pipeline, title, source, candidates).await;
        return;
    }

    let indices: Vec<usize> = if best_only || app.marked.is_empty() {
        vec![app.selected]
    } else {
        app.marked.iter().copied().collect()
    };

    for index in indices {
        let Some(group) = app.groups.get(index).cloned() else {
            continue;
        };
        let title = format!("{} — {}", group.primary.artist, group.primary.title);
        let source = group.primary.source;
        let candidates = match pipeline.candidates(&group).await {
            Ok(candidates) => candidates,
            Err(err) => {
                tracing::warn!(error = %err, "candidates failed");
                app.log_line("ошибка", err.to_string());
                continue;
            }
        };
        fetch_and_store(app, pipeline, title, source, candidates).await;
    }
}

/// Runs one verified download and turns the outcome into a Downloads row.
async fn fetch_and_store(
    app: &mut App,
    pipeline: &Pipeline,
    title: String,
    source: SourceId,
    candidates: Vec<Candidate>,
) {
    if candidates.is_empty() {
        app.push_download(DownloadItem {
            title: title.clone(),
            source,
            state: "не добыто".to_string(),
            transferred: 0,
            total: None,
            speed: None,
            detail: Some("кандидатов нет".to_string()),
        });
        app.log_line("загрузка", format!("{title}: кандидатов нет"));
        return;
    }

    let source = candidates.first().map(|c| c.source).unwrap_or(source);
    match pipeline.download(&candidates, &NoopSink).await {
        Ok(result) => {
            // `rejected` is sticky in `Pipeline::download`: it stays set after an
            // attempt fails even when a later candidate succeeds. A delivered
            // file is the only proof of success, so the path decides.
            let failed = result.path.as_os_str().is_empty();
            let total = if failed {
                None
            } else {
                std::fs::metadata(&result.path).ok().map(|meta| meta.len())
            };
            let (state, detail) = if failed {
                (
                    "не добыто".to_string(),
                    Some(
                        result
                            .reason
                            .clone()
                            .unwrap_or_else(|| "все кандидаты отбракованы".to_string()),
                    ),
                )
            } else {
                (
                    "готово".to_string(),
                    Some(result.path.display().to_string()),
                )
            };
            app.push_download(DownloadItem {
                title: title.clone(),
                source,
                state: state.clone(),
                transferred: total.unwrap_or(0),
                total,
                speed: None,
                detail: detail.clone(),
            });
            let message = match detail {
                Some(detail) => format!("{title}: {state} ({detail})"),
                None => format!("{title}: {state}"),
            };
            app.log_line("загрузка", message);
        }
        Err(err) => {
            tracing::warn!(error = %err, "download failed");
            app.push_download(DownloadItem {
                title: title.clone(),
                source,
                state: "ошибка".to_string(),
                transferred: 0,
                total: None,
                speed: None,
                detail: Some(err.to_string()),
            });
            app.log_line("ошибка", format!("{title}: {err}"));
        }
    }
}
#[cfg(test)]
mod tests {
    //! Unit tests for the parts `on_key`/`render` cannot reach: the async
    //! download path, which needs a [`Pipeline`] and a real source.

    use std::path::{Path, PathBuf};

    use async_trait::async_trait;

    use super::*;
    use crate::db::Db;
    use crate::fetch::ProgressSink;
    use crate::matching::ReleaseGroup;
    use crate::model::{Container, Locator, Quality, Release};
    use crate::sources::{AuthState, Capabilities, Source};

    /// The first candidate of the group fails, the second is accepted, so
    /// "не добыто" means the list was truncated to one entry.
    struct Flaky {
        requested: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl Flaky {
        fn new() -> Flaky {
            Flaky {
                requested: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            }
        }
    }

    #[async_trait]
    impl Source for Flaky {
        fn id(&self) -> SourceId {
            SourceId::Soulseek
        }

        fn capabilities(&self) -> Capabilities {
            Capabilities {
                audio: true,
                metadata_only: false,
                lossless: true,
                needs_auth: false,
            }
        }

        fn auth_state(&self) -> AuthState {
            AuthState::NotNeeded
        }

        async fn search(&self, _q: &Query) -> anyhow::Result<Vec<Release>> {
            Ok(Vec::new())
        }

        async fn resolve(&self, _q: &Query) -> anyhow::Result<Vec<Release>> {
            Ok(Vec::new())
        }

        async fn candidates(&self, release: &Release) -> anyhow::Result<Vec<Candidate>> {
            Ok(vec![
                candidate(release, "first.flac", 24, 96_000),
                candidate(release, "second.flac", 16, 44_100),
            ])
        }

        async fn download(
            &self,
            candidate: &Candidate,
            dest: &Path,
            _sink: &dyn ProgressSink,
        ) -> anyhow::Result<()> {
            if let Ok(mut log) = self.requested.lock() {
                log.push(match &candidate.locator {
                    Locator::Soulseek { filename, .. } => filename.clone(),
                    _ => "other".to_string(),
                });
            }
            if let Locator::Soulseek { filename, .. } = &candidate.locator {
                if filename == "first.flac" {
                    anyhow::bail!("peer went away");
                }
            }
            let bytes = std::fs::read(fixture())?;
            std::fs::write(dest, bytes)?;
            Ok(())
        }
    }

    fn fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/probe/sine_44100_s16.flac")
    }

    fn candidate(release: &Release, filename: &str, depth: u8, rate: u32) -> Candidate {
        Candidate {
            source: SourceId::Soulseek,
            release: release.clone(),
            quality: Quality {
                lossless: true,
                container: Container::Flac,
                bit_depth: Some(depth),
                sample_rate: Some(rate),
                bitrate_kbps: None,
            },
            size_bytes: Some(1),
            locator: Locator::Soulseek {
                username: "peer".to_string(),
                filename: filename.to_string(),
                size: 1,
            },
        }
    }

    fn group(title: &str) -> ReleaseGroup {
        ReleaseGroup {
            primary: Release {
                title: title.to_string(),
                artist: "Daft Punk".to_string(),
                album: None,
                track_no: None,
                duration_sec: None,
                isrc: None,
                year: None,
                source: SourceId::Soulseek,
                source_item_id: format!("{title}-1"),
            },
            members: Vec::new(),
            sources: vec![SourceId::Soulseek],
        }
    }

    fn config_in(root: &Path) -> Config {
        let mut config = Config::default();
        config.library.path = root.join("library");
        config.sources.enabled = vec!["soulseek".to_string()];
        config
    }

    fn pipeline(source: Flaky, config: &Config) -> Pipeline {
        Pipeline::new(
            config.clone(),
            Db::open_in_memory().expect("in-memory database"),
            vec![Box::new(source)],
        )
    }

    /// `a` must hand the whole ranked list to the pipeline so a failed first
    /// candidate falls back instead of reporting "не добыто".
    #[tokio::test]
    async fn best_download_keeps_every_candidate_for_the_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(dir.path());
        let source = Flaky::new();
        let requested = std::sync::Arc::clone(&source.requested);
        let pipeline = pipeline(source, &config);

        let mut app = App::new(config);
        app.groups = vec![group("Get Lucky")];
        app.selected = 0;

        download(&mut app, &pipeline, true).await;

        let attempted: Vec<String> = requested.lock().map(|log| log.clone()).unwrap_or_default();
        assert_eq!(app.downloads.len(), 1, "one row per group");
        assert_eq!(
            app.downloads[0].state, "готово",
            "the second candidate must be tried after the first fails: {attempted:?} detail={:?}",
            app.downloads[0].detail,
        );
        assert_eq!(attempted.len(), 2, "both candidates were attempted");
        assert_eq!(
            app.log.first().map(|entry| entry.level.as_str()),
            Some("загрузка"),
        );
    }

    /// The Candidates screen keeps the candidate cursor in `selected`, so the
    /// download must address the group named by `expanded`.
    #[tokio::test]
    async fn the_candidates_screen_downloads_the_expanded_group() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(dir.path());
        let pipeline = pipeline(Flaky::new(), &config);

        let mut app = App::new(config);
        app.groups = vec![group("One More Time"), group("Get Lucky")];
        app.expanded = Some(1);
        app.selected = 0;
        app.candidates = vec![
            candidate(&group("Get Lucky").primary, "second.flac", 16, 44_100),
            candidate(&group("Another").primary, "second.flac", 16, 44_100),
        ];
        app.screen = Screen::Candidates;

        download(&mut app, &pipeline, false).await;

        assert_eq!(app.downloads.len(), 1);
        assert_eq!(
            app.downloads[0].title, "Daft Punk — Get Lucky",
            "the row must be named after the expanded release",
        );
    }
}
