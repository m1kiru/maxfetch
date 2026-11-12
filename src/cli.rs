//! The headless CLI facade: one `clap` tree over the pipeline and the library.
//!
//! [`run`] is what `main` calls: it locates the configuration through the XDG
//! helpers in [`crate::config`], loads it and hands everything to
//! [`run_with`]. [`run_with`] is the testable core — the configuration, the
//! credentials and the output writer all come from the caller, so tests never
//! touch the real configuration directory or the real state database.
//!
//! Two rules run through the whole module:
//!
//! * nothing here deletes a file on its own: the only deletion path is
//!   [`crate::dedup::apply_deletion`], reached from
//!   `library dedupe --apply --delete` (plus `--yes` or a confirmation);
//! * `--json` and `--ndjson-progress` own stdout completely, so every line a
//!   machine consumer reads is data. Diagnostic messages, including the
//!   credentials-permission warning, go to stderr.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use crate::config::{self, Config, Credentials};
use crate::db::{Db, TrackRecord};
use crate::dedup::{self, DeletionPlan};
use crate::fetch::ProgressSink;
use crate::library::absolute_path;
use crate::model::{Quality, SourceId};
use crate::pipeline::Pipeline;
use crate::sources::{self, AuthState, Capabilities, Query};

/// Parsed command line of the `maxfetch` binary.
#[derive(Debug, Parser)]
#[command(
    name = "maxfetch",
    version,
    about = "Скачивание треков в максимально возможном качестве"
)]
pub struct Cli {
    /// Library root; overrides `library.path` in memory only.
    #[arg(long, global = true)]
    pub library: Option<PathBuf>,
    /// Sources to use, comma separated.
    #[arg(long, global = true, value_delimiter = ',')]
    pub source: Vec<String>,
    /// Print the plan, change nothing on disk.
    #[arg(long, global = true)]
    pub dry_run: bool,
    /// Machine-readable output.
    #[arg(long, global = true)]
    pub json: bool,
    /// One JSON progress event per line instead of human-readable output.
    #[arg(long, global = true)]
    pub ndjson_progress: bool,
    /// Accepted for scripting compatibility; the core build has no TUI.
    #[arg(long, global = true)]
    pub no_tui: bool,
    #[command(subcommand)]
    pub command: Command,
}

/// The six top-level commands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Search the sources for a track.
    Search {
        /// Free-text query.
        query: String,
    },
    /// Find and download tracks.
    Fetch {
        /// Free-text query.
        query: Option<String>,
        /// Playlist URL.
        #[arg(long)]
        playlist: Option<String>,
        /// Spotify track URL.
        #[arg(long)]
        spotify_track: Option<String>,
        /// Spotify playlist URL.
        #[arg(long)]
        spotify_playlist: Option<String>,
        /// Generic page URL.
        #[arg(long)]
        page: Option<String>,
        /// SoundCloud track URL.
        #[arg(long)]
        soundcloud: Option<String>,
        /// CSV file with `artist,title[,album,isrc]` rows.
        #[arg(long)]
        input: Option<String>,
    },
    /// Inspect the local library.
    Library {
        #[command(subcommand)]
        command: LibraryCommand,
    },
    /// Inspect or set up source credentials.
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
    /// Inspect the compiled-in sources.
    Sources {
        #[command(subcommand)]
        command: SourcesCommand,
    },
    /// Inspect or change the configuration.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
}

/// `maxfetch library …`
#[derive(Debug, Subcommand)]
pub enum LibraryCommand {
    /// Index audio files that the database does not know yet.
    Scan,
    /// Plan duplicate removal; deletes only with `--apply --delete`.
    Dedupe {
        /// Combine with `--delete` to actually remove files.
        #[arg(long)]
        apply: bool,
        /// Combine with `--apply` to actually remove files.
        #[arg(long)]
        delete: bool,
        /// Skip the interactive confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// List every indexed track.
    List,
}

/// `maxfetch auth …`
#[derive(Debug, Subcommand)]
pub enum AuthCommand {
    /// Log in to a source.
    Login {
        /// Source id, e.g. `soulseek`.
        source: String,
    },
    /// Show which credential sections are configured.
    Status,
}

/// `maxfetch sources …`
#[derive(Debug, Subcommand)]
pub enum SourcesCommand {
    /// Show the state of every compiled-in source.
    Check,
}

/// `maxfetch config …`
#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Set a configuration key.
    Set {
        /// Key to set; only `library` is supported.
        key: String,
        /// New value.
        value: String,
    },
}

/// One row of a `fetch --input` CSV file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackInput {
    pub artist: String,
    pub title: String,
    pub album: Option<String>,
    pub isrc: Option<String>,
}

/// One NDJSON progress event; `kind` serialises under the key `event`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ExportEvent {
    #[serde(rename = "event")]
    pub kind: String,
    pub detail: String,
}

/// The raw `fetch` arguments, kept together so the command body stays readable.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct FetchRequest {
    query: Option<String>,
    playlist: Option<String>,
    spotify_track: Option<String>,
    spotify_playlist: Option<String>,
    page: Option<String>,
    soundcloud: Option<String>,
    input: Option<String>,
}

/// What `fetch` was asked to work on.
#[derive(Clone, Debug, PartialEq)]
enum Request {
    /// Rows of a `--input` CSV file, in file order.
    Rows(Vec<TrackInput>),
    /// A single text or URL query.
    Query(Query),
}

/// Output switches shared by the commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OutputMode {
    json: bool,
    ndjson: bool,
}

impl OutputMode {
    /// True when stdout must carry machine data only.
    fn machine(&self) -> bool {
        self.json || self.ndjson
    }
}

/// Progress sink that records events as NDJSON lines in an in-memory buffer.
///
/// [`ProgressSink`] is `Send + Sync` and takes `&self`, while the command
/// writes through `&mut dyn Write`, which is neither. The shared buffer bridges
/// the two: the sink pushes finished lines, the command drains them into `out`
/// at step boundaries.
#[derive(Clone, Debug, Default)]
struct NdjsonSink {
    enabled: bool,
    buffer: Arc<Mutex<Vec<u8>>>,
}

impl NdjsonSink {
    fn new(enabled: bool) -> NdjsonSink {
        NdjsonSink {
            enabled,
            buffer: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn push(&self, kind: &str, detail: String) {
        if !self.enabled {
            return;
        }
        let line = ndjson_progress_line(&ExportEvent {
            kind: kind.to_string(),
            detail,
        });
        let mut buffer = self.buffer();
        buffer.extend_from_slice(line.as_bytes());
    }

    /// Writes everything buffered so far into `out` and flushes it.
    fn drain_into(&self, out: &mut dyn Write) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let bytes = {
            let mut buffer = self.buffer();
            std::mem::take(&mut *buffer)
        };
        if bytes.is_empty() {
            return Ok(());
        }
        out.write_all(&bytes)?;
        out.flush()?;
        Ok(())
    }

    /// The buffer, recovered even if another thread panicked while holding it.
    fn buffer(&self) -> std::sync::MutexGuard<'_, Vec<u8>> {
        match self.buffer.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

impl ProgressSink for NdjsonSink {
    fn on_progress(&self, transferred: u64, total: Option<u64>) {
        let detail = match total {
            Some(total) => format!("{transferred} из {total} байт"),
            None => format!("{transferred} байт"),
        };
        self.push("progress", detail);
    }

    fn on_finished(&self, transferred: u64) {
        self.push("finished", format!("{transferred} байт"));
    }
}

/// One command's view of its output: the pipeline, the writers and the mode.
struct Session<'a, 'w> {
    pipeline: &'a Pipeline,
    mode: OutputMode,
    sink: NdjsonSink,
    out: &'w mut dyn Write,
    /// Where NDJSON progress events go. `run` points this at stderr so that the
    /// payload on stdout stays clean (ruling 8); `run_with` passes `None`, which
    /// keeps the events in `out`.
    events: Option<&'w mut dyn Write>,
}

impl<'a, 'w> Session<'a, 'w> {
    fn new(
        pipeline: &'a Pipeline,
        mode: OutputMode,
        out: &'w mut dyn Write,
        events: Option<&'w mut dyn Write>,
    ) -> Session<'a, 'w> {
        Session {
            pipeline,
            mode,
            sink: NdjsonSink::new(mode.ndjson),
            out,
            events,
        }
    }

    /// A human-readable line; suppressed when stdout carries machine data.
    fn human(&mut self, text: &str) -> Result<()> {
        if self.mode.machine() {
            return Ok(());
        }
        writeln!(self.out, "{text}")?;
        Ok(())
    }

    /// A `plan` event, or the same information as a human-readable line.
    fn plan(&mut self, detail: &str) -> Result<()> {
        if self.mode.ndjson {
            self.event("plan", detail.to_string())?;
        } else {
            self.human(&format!("план: {detail}"))?;
        }
        Ok(())
    }

    /// Pushes one event onto the stream and flushes the buffer.
    fn event(&mut self, kind: &str, detail: String) -> Result<()> {
        self.sink.push(kind, detail);
        self.drain()
    }

    /// Flushes the event buffer into the event writer, or into `out` when no
    /// separate event channel was given.
    fn drain(&mut self) -> Result<()> {
        match self.events.as_mut() {
            Some(events) => self.sink.drain_into(&mut **events),
            None => self.sink.drain_into(&mut *self.out),
        }
    }
}

/// Runs the CLI with the on-disk configuration of this user.
///
/// Stdout carries the command payload only; the NDJSON progress stream goes to
/// stderr so that `--ndjson-progress` composes with `--json`.
pub async fn run(cli: Cli) -> Result<()> {
    let dir = config::config_dir();
    let config = Config::load_from(&dir)?;
    let credentials = Credentials::load_from(&dir)?;
    let mut stdout = std::io::stdout();
    let mut stderr = std::io::stderr();
    run_with_writers(cli, config, credentials, &mut stdout, Some(&mut stderr)).await
}

/// Runs the CLI against the given configuration, credentials and writer.
///
/// The credentials guard runs exactly once, here, so `main` and the tests share
/// one code path. With no separate event writer, NDJSON events share `out`.
pub async fn run_with(
    cli: Cli,
    config: Config,
    credentials: Credentials,
    out: &mut dyn Write,
) -> Result<()> {
    run_with_writers(cli, config, credentials, out, None).await
}

async fn run_with_writers<'w>(
    cli: Cli,
    mut config: Config,
    credentials: Credentials,
    out: &'w mut dyn Write,
    events: Option<&'w mut dyn Write>,
) -> Result<()> {
    // The guard speaks on stderr: `--json` and `--ndjson-progress` own stdout.
    let mut stderr = std::io::stderr();
    warn_if_credentials_too_open(&config::credentials_path(), &mut stderr)?;

    let Cli {
        library: library_override,
        source,
        dry_run,
        json,
        ndjson_progress,
        no_tui: _,
        command,
    } = cli;

    // In-memory overrides only: `config.toml` is never rewritten here.
    if let Some(library) = library_override {
        config.library.path = library;
    }
    if !source.is_empty() {
        let mut enabled = Vec::new();
        for name in &source {
            match SourceId::from_str(name) {
                Some(id) => enabled.push(id.as_str().to_string()),
                None => tracing::warn!(name = %name, "unknown source name in --source"),
            }
        }
        if !enabled.is_empty() {
            config.sources.enabled = enabled;
        }
    }

    // Everything below is synchronous on purpose: `Db` is not `Sync`, so these
    // calls cannot move to `spawn_blocking` without reworking the database
    // layer. A single-threaded CLI driver is acceptable here.
    let db = Db::open(&config::state_path())?;
    let pipeline = Pipeline::new(config.clone(), db, sources::registry(&config, &credentials));
    let library_root = config.library.path.clone();

    let mode = OutputMode {
        json,
        ndjson: ndjson_progress,
    };
    let mut session = Session::new(&pipeline, mode, out, events);

    match command {
        Command::Search { query } => search(&mut session, &query).await,
        Command::Fetch {
            query,
            playlist,
            spotify_track,
            spotify_playlist,
            page,
            soundcloud,
            input,
        } => {
            let request = FetchRequest {
                query,
                playlist,
                spotify_track,
                spotify_playlist,
                page,
                soundcloud,
                input,
            };
            fetch(&mut session, &library_root, request, dry_run).await
        }
        Command::Library { command } => library(&mut session, &library_root, command, dry_run),
        Command::Auth { command } => auth(&credentials, command, &mut session),
        Command::Sources { command } => {
            let SourcesCommand::Check = command;
            sources_check(&mut session)
        }
        Command::Config { command } => {
            let ConfigCommand::Set { key, value } = command;
            config_set(&mut config, &key, &value, &mut session)
        }
    }
}

/// Prints the credentials-permission warning when the guard had to tighten it.
///
/// [`config::ensure_private`] is idempotent and returns `true` only when the
/// mode was wider than `0600` and has just been narrowed, so `false` — already
/// `0600`, or no file at all — stays silent.
pub fn warn_if_credentials_too_open(credentials_path: &Path, out: &mut dyn Write) -> Result<()> {
    if config::ensure_private(credentials_path)? {
        writeln!(
            out,
            "Внимание: права файла {} были шире 0600, секреты были доступны другим пользователям — выставлены права 0600",
            credentials_path.display()
        )?;
        out.flush()?;
    }
    Ok(())
}

/// Serialises `event` as one NDJSON line.
///
/// Serialising a struct of two `String` fields cannot fail, so the impossible
/// error is a panic with a clear message instead of a `Result` threaded through
/// every [`ProgressSink`] method.
pub fn ndjson_progress_line(event: &ExportEvent) -> String {
    let mut line = serde_json::to_string(event).expect("serialising an ExportEvent cannot fail");
    line.push('\n');
    line
}

/// Reads a `fetch --input` CSV file.
///
/// A quoted field may contain commas. The first non-empty row is treated as a
/// header when it names both `artist` and `title`; otherwise the positional
/// order `artist,title,album,isrc` is assumed.
pub fn read_input_csv(path: &Path) -> Result<Vec<TrackInput>> {
    let raw =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;

    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_reader(raw.as_bytes());

    let mut rows: Vec<Vec<String>> = Vec::new();
    for record in reader.records() {
        let record = record.with_context(|| format!("parsing {}", path.display()))?;
        let fields: Vec<String> = record
            .iter()
            .map(|field| field.trim().to_string())
            .collect();
        // Blank lines are not tracks.
        if fields.iter().any(|field| !field.is_empty()) {
            rows.push(fields);
        }
    }

    let mut columns = Columns::positional();
    if let Some(first) = rows.first()
        && columns_is_header(first)
    {
        columns = Columns::from_header(first);
        rows.remove(0);
    }

    let mut inputs = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let line = index + 1;
        let artist = columns.value(row, Some(columns.artist)).unwrap_or_default();
        if artist.is_empty() {
            return Err(anyhow::anyhow!(
                "{}: row {line} has an empty artist",
                path.display()
            ));
        }
        let title = columns.value(row, Some(columns.title)).unwrap_or_default();
        if title.is_empty() {
            return Err(anyhow::anyhow!(
                "{}: row {line} has an empty title",
                path.display()
            ));
        }
        inputs.push(TrackInput {
            artist: artist.to_string(),
            title: title.to_string(),
            album: columns.value(row, columns.album).map(str::to_string),
            isrc: columns.value(row, columns.isrc).map(str::to_string),
        });
    }
    Ok(inputs)
}

/// True when `row` looks like a header naming both `artist` and `title`.
fn columns_is_header(row: &[String]) -> bool {
    let names = |name: &str| row.iter().any(|field| field.eq_ignore_ascii_case(name));
    names("artist") && names("title")
}

/// Column positions of a CSV file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Columns {
    artist: usize,
    title: usize,
    album: Option<usize>,
    isrc: Option<usize>,
}

impl Columns {
    fn positional() -> Columns {
        Columns {
            artist: 0,
            title: 1,
            album: Some(2),
            isrc: Some(3),
        }
    }

    fn from_header(row: &[String]) -> Columns {
        let find = |name: &str| {
            row.iter()
                .position(|field| field.eq_ignore_ascii_case(name))
        };
        Columns {
            artist: find("artist").unwrap_or(0),
            title: find("title").unwrap_or(1),
            album: find("album"),
            isrc: find("isrc"),
        }
    }

    /// The trimmed field at `index`, or `None` when absent or empty.
    fn value<'a>(&self, row: &'a [String], index: Option<usize>) -> Option<&'a str> {
        let field = row.get(index?)?.as_str();
        if field.is_empty() { None } else { Some(field) }
    }
}

async fn search(session: &mut Session<'_, '_>, text: &str) -> Result<()> {
    let query = Query {
        text: Some(text.to_string()),
        ..Query::default()
    };
    let groups = session.pipeline.search(&query).await?;

    if session.mode.json {
        let rows: Vec<serde_json::Value> = groups
            .iter()
            .map(|group| {
                serde_json::json!({
                    "title": group.primary.title,
                    "artist": group.primary.artist,
                    "album": group.primary.album,
                    "sources": group.sources.iter().map(|id| id.as_str()).collect::<Vec<_>>(),
                })
            })
            .collect();
        writeln!(session.out, "{}", serde_json::to_string(&rows)?)?;
        return Ok(());
    }

    if groups.is_empty() {
        session.human(&format!("ничего не найдено: {text}"))?;
        return Ok(());
    }
    for group in &groups {
        session.human(&format!(
            "{} — {}{}",
            group.primary.artist,
            group.primary.title,
            album_suffix(group.primary.album.as_deref())
        ))?;
    }
    Ok(())
}

async fn fetch(
    session: &mut Session<'_, '_>,
    library_root: &Path,
    request: FetchRequest,
    dry_run: bool,
) -> Result<()> {
    let request = if let Some(input) = request.input.as_deref() {
        // `--input` wins over the URL flags and the positional query.
        Request::Rows(read_input_csv(Path::new(input))?)
    } else {
        Request::Query(query_from_request(&request)?)
    };

    let mut planned = 0usize;
    let mut acquired: Vec<serde_json::Value> = Vec::new();
    let mut failed = 0usize;

    match request {
        Request::Rows(rows) => {
            for row in &rows {
                let label = format!(
                    "{} — {}{}",
                    row.artist,
                    row.title,
                    album_suffix(row.album.as_deref())
                );
                planned += 1;
                if dry_run {
                    // The plan never depends on search results: every row is
                    // planned even when the registry is empty and nothing can
                    // be found.
                    session.plan(&label)?;
                    continue;
                }
                let query = Query {
                    text: Some(format!("{} {}", row.artist, row.title)),
                    ..Query::default()
                };
                download_groups(session, &query, &mut acquired, &mut failed).await?;
            }
        }
        Request::Query(query) => {
            let label = query
                .text
                .clone()
                .or_else(|| query.url.clone())
                .unwrap_or_default();
            if dry_run {
                // Stated before any search: a dry run reports what it was asked
                // for even when no source can answer.
                planned += 1;
                session.plan(&label)?;
                let groups = session.pipeline.search(&query).await?;
                for group in &groups {
                    let candidates = session.pipeline.candidates(group).await?;
                    for candidate in &candidates {
                        planned += 1;
                        let path = absolute_path(
                            library_root,
                            &candidate.release,
                            candidate.quality.container.as_str(),
                        );
                        let detail = format!(
                            "{} — {} [{}] → {}",
                            candidate.release.artist,
                            candidate.release.title,
                            show_quality(&candidate.quality),
                            path.display()
                        );
                        session.plan(&detail)?;
                    }
                }
            } else {
                planned += 1;
                download_groups(session, &query, &mut acquired, &mut failed).await?;
            }
        }
    }

    write_fetch_summary(session, planned, &acquired, failed, dry_run)
}

/// Searches `query`, downloads the best candidate of every group and records
/// the outcome. A rejected download is "not acquired", never an error.
async fn download_groups(
    session: &mut Session<'_, '_>,
    query: &Query,
    acquired: &mut Vec<serde_json::Value>,
    failed: &mut usize,
) -> Result<()> {
    let groups = session.pipeline.search(query).await?;
    for group in &groups {
        let candidates = session.pipeline.candidates(group).await?;
        let result = session
            .pipeline
            .download(&candidates, &session.sink)
            .await?;
        session.drain()?;

        if result.rejected || result.path.as_os_str().is_empty() {
            let reason = result
                .reason
                .unwrap_or_else(|| "no usable candidate".to_string());
            *failed += 1;
            if session.mode.ndjson {
                session.event(
                    "failed",
                    format!(
                        "{} — {}: {reason}",
                        group.primary.artist, group.primary.title
                    ),
                )?;
            } else {
                session.human(&format!(
                    "пропущено: {} — {} ({reason})",
                    group.primary.artist, group.primary.title
                ))?;
            }
            continue;
        }

        session.human(&format!("добыто: {}", result.path.display()))?;
        acquired.push(serde_json::json!({
            "artist": group.primary.artist,
            "title": group.primary.title,
            "path": result.path,
        }));
    }
    Ok(())
}

fn write_fetch_summary(
    session: &mut Session<'_, '_>,
    planned: usize,
    acquired: &[serde_json::Value],
    failed: usize,
    dry_run: bool,
) -> Result<()> {
    let acquired_count = acquired.len();
    // The event stream and the JSON payload own different channels (`run` points
    // the events at stderr), so `--json --ndjson-progress` yields both.
    if session.mode.ndjson {
        session.event(
            "summary",
            format!("запланировано: {planned}, добыто: {acquired_count}, пропущено: {failed}"),
        )?;
    }
    if session.mode.json {
        let summary = serde_json::json!({
            "planned": planned,
            "acquired": acquired_count,
            "failed": failed,
            "tracks": acquired,
        });
        writeln!(session.out, "{}", serde_json::to_string(&summary)?)?;
        return Ok(());
    }
    // `human` is a no-op whenever stdout carries machine data, so the NDJSON-only
    // case stops here.
    if dry_run {
        session.human(&format!("запланировано: {planned}"))
    } else {
        session.human(&format!("добыто: {acquired_count}, пропущено: {failed}"))
    }
}

/// Builds the single text/URL query of a `fetch` run.
fn query_from_request(request: &FetchRequest) -> Result<Query> {
    let url = request
        .playlist
        .clone()
        .or_else(|| request.spotify_track.clone())
        .or_else(|| request.spotify_playlist.clone())
        .or_else(|| request.page.clone())
        .or_else(|| request.soundcloud.clone());
    if let Some(url) = url {
        return Ok(Query {
            url: Some(url),
            ..Query::default()
        });
    }
    match request.query.clone() {
        Some(text) => Ok(Query {
            text: Some(text),
            ..Query::default()
        }),
        None => Err(anyhow::anyhow!(
            "no query given: pass a text query, a URL flag or --input <csv>"
        )),
    }
}

fn library(
    session: &mut Session<'_, '_>,
    library_root: &Path,
    command: LibraryCommand,
    dry_run: bool,
) -> Result<()> {
    match command {
        LibraryCommand::Scan => {
            let inserted = dedup::scan_library(session.pipeline.db(), library_root)?;
            if session.mode.json {
                let value = serde_json::json!({ "indexed": inserted });
                writeln!(session.out, "{}", serde_json::to_string(&value)?)?;
            } else if inserted == 0 {
                session.human("новых файлов не найдено")?;
            } else {
                session.human(&format!("проиндексировано файлов: {inserted}"))?;
            }
            Ok(())
        }
        LibraryCommand::Dedupe { apply, delete, yes } => {
            let plans = dedup::plan_deletion(session.pipeline.db())?;

            if session.mode.json {
                let rows: Vec<serde_json::Value> = plans.iter().map(plan_value).collect();
                writeln!(session.out, "{}", serde_json::to_string(&rows)?)?;
            } else if plans.is_empty() {
                session.human("нечего удалять")?;
            } else {
                for plan in &plans {
                    session.human(&format!(
                        "{} ← оставлен {} ({})",
                        plan.path, plan.winner_path, plan.reason
                    ))?;
                }
            }

            // Deletion needs both flags, a real run, and either `--yes` or an
            // interactive confirmation. Every other combination stays a plan.
            if dry_run || !apply || !delete || plans.is_empty() {
                return Ok(());
            }
            if !yes && !confirm_deletion(session.out)? {
                session.human("удаление отменено")?;
                return Ok(());
            }

            let removed = dedup::apply_deletion(session.pipeline.db(), &plans)?;
            if session.mode.machine() {
                // stdout stays data; the count goes to stderr.
                eprintln!("удалено файлов: {removed}");
            } else {
                session.human(&format!("удалено файлов: {removed}"))?;
            }
            Ok(())
        }
        LibraryCommand::List => {
            let tracks = session.pipeline.db().all_tracks()?;
            if session.mode.json {
                let rows: Vec<serde_json::Value> = tracks.iter().map(track_value).collect();
                writeln!(session.out, "{}", serde_json::to_string(&rows)?)?;
                return Ok(());
            }
            if tracks.is_empty() {
                session.human("библиотека пуста")?;
                return Ok(());
            }
            for track in &tracks {
                let quality = track.quality();
                let duplicate = match track.duplicate_of {
                    Some(winner) => format!(" (проигравший, дубликат #{winner})"),
                    None => String::new(),
                };
                session.human(&format!(
                    "{} — {} [{}] → {}{}",
                    track.artist,
                    track.title,
                    show_quality(&quality),
                    track.path,
                    duplicate
                ))?;
            }
            Ok(())
        }
    }
}

/// Asks the user before deleting anything; reached only in the deletion branch.
fn confirm_deletion(out: &mut dyn Write) -> Result<bool> {
    write!(out, "Удалить перечисленные файлы? [y/N]: ")?;
    out.flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    let answer = answer.trim().to_lowercase();
    Ok(matches!(answer.as_str(), "y" | "yes" | "д" | "да"))
}

fn auth(
    credentials: &Credentials,
    command: AuthCommand,
    session: &mut Session<'_, '_>,
) -> Result<()> {
    match command {
        AuthCommand::Login { source } => {
            session.human(&format!(
                "источник {source} не поддерживается в этой сборке: авторизация появится позже"
            ))?;
            Ok(())
        }
        AuthCommand::Status => {
            for (name, configured) in credential_sections(credentials) {
                let state = if configured {
                    "настроено"
                } else {
                    "не настроено"
                };
                session.human(&format!("{name}: {state}"))?;
            }
            Ok(())
        }
    }
}

fn credential_sections(credentials: &Credentials) -> Vec<(&'static str, bool)> {
    vec![
        ("soulseek", credentials.soulseek.is_some()),
        ("bandcamp", credentials.bandcamp.is_some()),
        ("spotify", credentials.spotify.is_some()),
        ("soundcloud", credentials.soundcloud.is_some()),
        ("qobuz", credentials.qobuz.is_some()),
        ("tidal", credentials.tidal.is_some()),
        ("deezer", credentials.deezer.is_some()),
        ("beatport", credentials.beatport.is_some()),
    ]
}

fn sources_check(session: &mut Session<'_, '_>) -> Result<()> {
    let statuses = session.pipeline.source_statuses();
    if statuses.is_empty() {
        session.human("источников нет")?;
        return Ok(());
    }
    for status in &statuses {
        session.human(&format!(
            "{} — возможности: {}; авторизация: {}; {}",
            status.id.as_str(),
            describe_capabilities(&status.capabilities),
            describe_auth(status.auth),
            status.detail.as_deref().unwrap_or("подробностей нет")
        ))?;
    }
    Ok(())
}

fn config_set(
    config: &mut Config,
    key: &str,
    value: &str,
    session: &mut Session<'_, '_>,
) -> Result<()> {
    match key {
        "library" => {
            config.library.path = PathBuf::from(value);
            config.save_to(&config::config_dir())?;
            session.human(&format!("библиотека: {}", config.library.path.display()))?;
            Ok(())
        }
        other => Err(anyhow::anyhow!(
            "unsupported config key '{other}'; supported keys: library"
        )),
    }
}

/// Builds the JSON form of one library row by hand: `TrackRecord` is not
/// `Serialize`, and the database layer is not this task's to change.
fn track_value(track: &TrackRecord) -> serde_json::Value {
    serde_json::json!({
        "id": track.id,
        "artist": track.artist,
        "title": track.title,
        "album": track.album,
        "path": track.path,
        "quality": {
            "lossless": track.lossless,
            "container": track.container.as_str(),
            "bit_depth": track.bit_depth,
            "sample_rate": track.sample_rate,
            "bitrate_kbps": track.bitrate_kbps,
        },
        "duplicate_of": track.duplicate_of,
    })
}

fn plan_value(plan: &DeletionPlan) -> serde_json::Value {
    serde_json::json!({
        "id": plan.id,
        "path": plan.path,
        "winner_path": plan.winner_path,
        "reason": plan.reason,
    })
}

/// Human-readable quality, e.g. `flac 24 bit/96000 Hz`.
fn show_quality(quality: &Quality) -> String {
    let bit_depth = match quality.bit_depth {
        Some(depth) => format!("{depth} bit"),
        None => "-".to_string(),
    };
    let sample_rate = match quality.sample_rate {
        Some(rate) => format!("{rate} Hz"),
        None => "-".to_string(),
    };
    let bitrate = match quality.bitrate_kbps {
        Some(kbps) => format!(" {kbps} kbps"),
        None => String::new(),
    };
    format!(
        "{} {bit_depth}/{sample_rate}{bitrate}",
        quality.container.as_str()
    )
}

fn describe_capabilities(capabilities: &Capabilities) -> String {
    let mut parts = Vec::new();
    if capabilities.audio {
        parts.push("аудио");
    }
    if capabilities.metadata_only {
        parts.push("только метаданные");
    }
    if capabilities.lossless {
        parts.push("lossless");
    }
    if capabilities.needs_auth {
        parts.push("нужна авторизация");
    }
    if parts.is_empty() {
        return "нет".to_string();
    }
    parts.join(", ")
}

fn describe_auth(auth: AuthState) -> &'static str {
    match auth {
        AuthState::NotNeeded => "не требуется",
        AuthState::Missing => "не настроена",
        AuthState::Ok => "в порядке",
        AuthState::Expired => "истекла",
    }
}

fn album_suffix(album: Option<&str>) -> String {
    match album {
        Some(album) => format!(" [{album}]"),
        None => String::new(),
    }
}
