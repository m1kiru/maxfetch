//! End-to-end tests for the headless CLI facade (`src/cli.rs`).
//!
//! Every test isolates the process environment (`XDG_CONFIG_HOME`,
//! `XDG_DATA_HOME`, `XDG_STATE_HOME`) into a `tempfile` directory so that no
//! test ever touches the real `~/.config/maxfetch` or
//! `~/.local/share/maxfetch`; the `ENV_LOCK` mutex serialises the global
//! environment mutation across tests in this binary (the same pattern as
//! `tests/config.rs`).
//!
//! The environment guard is deliberately held across the `run_with().await`
//! point: releasing it earlier would let a concurrent test repoint the XDG
//! roots while this one is running. The tests are single-threaded
//! (`#[tokio::test]` without a multi-thread runtime), so no other test can make
//! progress on the same thread meanwhile.
#![allow(clippy::await_holding_lock)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::sync::{Mutex, MutexGuard};

use clap::Parser;

use maxfetch::cli::{Cli, read_input_csv, run_with, warn_if_credentials_too_open};
use maxfetch::config::{Config, Credentials};

/// Serialises all environment mutation: the process environment is global,
/// so tests that set/remove XDG variables must never run concurrently.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Points all three XDG roots into `dir` and returns the lock held for the
/// whole test.
fn isolate_xdg(dir: &tempfile::TempDir) -> MutexGuard<'static, ()> {
    let guard = ENV_LOCK.lock().unwrap();
    let fake = dir.path().join("xdg");
    fs::create_dir_all(&fake).unwrap();
    // The returned guard is held by the caller for the whole test, so no
    // other test can mutate the environment (or read it via `dirs`) meanwhile.
    unsafe {
        std::env::set_var("XDG_DATA_HOME", &fake);
        std::env::set_var("XDG_CONFIG_HOME", &fake);
        std::env::set_var("XDG_STATE_HOME", &fake);
    }
    guard
}

/// Isolated default config + credentials; the returned guard keeps the env
/// pinned for the duration of the test.
fn isolated(dir: &tempfile::TempDir) -> (MutexGuard<'static, ()>, Config, Credentials) {
    let env = isolate_xdg(dir);
    let cfg = Config::load_from(dir.path()).unwrap();
    let creds = Credentials::load_from(dir.path()).unwrap();
    (env, cfg, creds)
}

/// Parses `args` (argv without the program name) into a [`Cli`].
fn parse(args: &[&str]) -> Cli {
    let mut argv = vec!["maxfetch"];
    argv.extend_from_slice(args);
    Cli::try_parse_from(argv).expect("clap parse failed")
}

/// Runs the real binary with all three XDG roots pointed into `dir` and returns
/// its captured `(stdout, stderr)`.
///
/// The child gets the isolated environment through `Command::env`, so the
/// process-global environment (and therefore the `ENV_LOCK` protocol) is
/// untouched and this test can run in parallel with the others.
fn run_binary(args: &[&str], dir: &tempfile::TempDir) -> (String, String) {
    let fake = dir.path().join("xdg");
    fs::create_dir_all(&fake).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_maxfetch"))
        .args(args)
        .env("XDG_CONFIG_HOME", &fake)
        .env("XDG_DATA_HOME", &fake)
        .env("XDG_STATE_HOME", &fake)
        .output()
        .expect("running the maxfetch binary failed");
    assert!(
        output.status.success(),
        "binary exited with {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    (
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

/// Ruling 8: progress events belong on stderr, so stdout stays a pure payload.
///
/// `run_with` keeps both channels in `out` (the mandatory
/// `ndjson_progress_writes_one_json_object_per_line` test relies on that), but
/// the binary spawned through `run` must split them.
#[test]
fn run_sends_ndjson_events_to_stderr() {
    let dir = tempfile::tempdir().unwrap();
    let (stdout, stderr) = run_binary(
        &[
            "fetch",
            "--dry-run",
            "--ndjson-progress",
            "daft punk get lucky",
        ],
        &dir,
    );

    assert!(
        stdout.trim().is_empty(),
        "ndjson events must not land on stdout: {stdout}"
    );

    let lines: Vec<&str> = stderr
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    assert!(
        !lines.is_empty(),
        "the event stream on stderr must not be empty"
    );
    for line in &lines {
        let value: serde_json::Value = serde_json::from_str(line)
            .unwrap_or_else(|err| panic!("stderr line is not JSON: {line:?}: {err}"));
        assert!(
            value.get("event").is_some(),
            "stderr line lacks the `event` key: {line}"
        );
    }
    let last: serde_json::Value = serde_json::from_str(lines.last().unwrap()).unwrap();
    assert_eq!(last["event"], "summary");
}

/// The `--json` payload stays on stdout while the event stream owns stderr.
#[test]
fn run_keeps_the_json_payload_on_stdout() {
    let dir = tempfile::tempdir().unwrap();
    let (stdout, stderr) = run_binary(&["library", "list", "--json"], &dir);

    let value: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout must be one JSON array");
    assert!(value.is_array(), "whole stdout must be one JSON array");
    assert!(
        stderr.trim().is_empty(),
        "no events are produced by `library list`: {stderr}"
    );
}

/// With separate channels the two flags compose: one JSON payload on stdout and
/// the event stream on stderr, each independently parseable.
#[test]
fn run_composes_json_payload_with_ndjson_events() {
    let dir = tempfile::tempdir().unwrap();
    let (stdout, stderr) = run_binary(
        &[
            "fetch",
            "--dry-run",
            "--json",
            "--ndjson-progress",
            "daft punk get lucky",
        ],
        &dir,
    );

    let payload: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout must be one JSON object");
    assert!(payload.is_object(), "expected a JSON object: {stdout}");
    assert!(payload.get("planned").is_some(), "missing summary fields");

    let lines: Vec<&str> = stderr
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    assert!(!lines.is_empty(), "the event stream must not be empty");
    for line in &lines {
        let value: serde_json::Value = serde_json::from_str(line)
            .unwrap_or_else(|err| panic!("stderr line is not JSON: {line:?}: {err}"));
        assert!(value.get("event").is_some(), "line lacks `event`: {line}");
    }
}

#[tokio::test]
async fn dry_run_downloads_nothing_and_reports_the_plan() {
    let dir = tempfile::tempdir().unwrap();
    let (_env, mut cfg, creds) = isolated(&dir);
    let library = dir.path().join("library");
    cfg.library.path = library.clone();
    // The plan must not be tied to search results: with an empty registry the
    // query itself is still reported.
    let cli = parse(&[
        "fetch",
        "--dry-run",
        "--library",
        library.to_str().unwrap(),
        "daft punk get lucky",
    ]);
    let mut out = Vec::new();
    run_with(cli, cfg, creds, &mut out).await.unwrap();
    let text = String::from_utf8(out).unwrap();
    assert!(
        text.to_lowercase().contains("get lucky"),
        "plan not printed: {text}"
    );
    if library.exists() {
        let entries = fs::read_dir(&library).unwrap().count();
        assert_eq!(entries, 0, "dry run must not create library files");
    }
    assert!(
        !library.join(".tmp").exists(),
        "dry run must not create .tmp"
    );
}

#[tokio::test]
async fn library_dedupe_without_flags_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (_env, cfg, creds) = isolated(&dir);
    let cli = parse(&["library", "dedupe"]);
    let mut out = Vec::new();
    run_with(cli, cfg, creds, &mut out).await.unwrap();
    let text = String::from_utf8(out).unwrap();
    assert!(
        text.contains("нечего удалять"),
        "expected a dry-run plan, got: {text}"
    );
}

#[tokio::test]
async fn library_list_json_is_valid() {
    let dir = tempfile::tempdir().unwrap();
    let (_env, cfg, creds) = isolated(&dir);
    let cli = parse(&["library", "list", "--json"]);
    let mut out = Vec::new();
    run_with(cli, cfg, creds, &mut out).await.unwrap();
    let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert!(value.is_array(), "whole stdout must be one JSON array");
}

#[tokio::test]
async fn ndjson_progress_writes_one_json_object_per_line() {
    let dir = tempfile::tempdir().unwrap();
    let (_env, cfg, creds) = isolated(&dir);
    let cli = parse(&[
        "fetch",
        "--dry-run",
        "--ndjson-progress",
        "daft punk get lucky",
    ]);
    let mut out = Vec::new();
    run_with(cli, cfg, creds, &mut out).await.unwrap();
    let text = String::from_utf8(out).unwrap();
    assert!(!text.is_empty(), "the event stream must never be empty");
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    assert!(!lines.is_empty(), "the event stream must never be empty");
    for line in &lines {
        let value: serde_json::Value = serde_json::from_str(line)
            .unwrap_or_else(|err| panic!("line is not JSON: {line:?}: {err}"));
        assert!(
            value.get("event").is_some(),
            "line lacks the `event` key: {line}"
        );
    }
    // The stream is closed by a summary event even when nothing was found.
    let last: serde_json::Value = serde_json::from_str(lines.last().unwrap()).unwrap();
    assert_eq!(last["event"], "summary");
}

#[tokio::test]
async fn fetch_input_reads_a_csv_and_queues_every_row() {
    let dir = tempfile::tempdir().unwrap();
    let (_env, mut cfg, creds) = isolated(&dir);
    cfg.library.path = dir.path().join("library");
    let csv_path = dir.path().join("input.csv");
    // `album` of the first row contains a comma inside quotes: the CSV parser
    // must keep the field intact and split on unquoted commas only.
    fs::write(
        &csv_path,
        "artist,title,album,isrc\n\
         Daft Punk,Get Lucky,\"Random Access Memories, Deluxe\",FR123400001\n\
         Daft Punk,One More Time,,\n",
    )
    .unwrap();

    let rows = read_input_csv(&csv_path).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].title, "Get Lucky");
    assert_eq!(
        rows[0].album.as_deref(),
        Some("Random Access Memories, Deluxe")
    );
    assert_eq!(rows[0].isrc.as_deref(), Some("FR123400001"));
    assert!(rows[1].album.is_none());
    assert!(rows[1].isrc.is_none());

    let cli = parse(&["fetch", "--dry-run", "--input", csv_path.to_str().unwrap()]);
    let mut out = Vec::new();
    run_with(cli, cfg, creds, &mut out).await.unwrap();
    let text = String::from_utf8(out).unwrap();
    assert!(
        text.contains("Get Lucky"),
        "row 1 missing from plan: {text}"
    );
    assert!(
        text.contains("One More Time"),
        "row 2 missing from plan: {text}"
    );
}

#[test]
fn wide_credentials_mode_prints_a_warning() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("credentials.toml");
    fs::write(&path, "[soulseek]\nusername = \"u\"\npassword = \"p\"\n").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

    let mut out = Vec::new();
    warn_if_credentials_too_open(&path, &mut out).unwrap();
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("0600"), "warning must mention 0600: {text}");
    assert!(
        text.contains(path.to_str().unwrap()),
        "warning must mention the path"
    );
    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "the guard must tighten the mode to 0600");
}

#[test]
fn a_missing_credentials_file_is_silent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("credentials.toml");
    let mut out = Vec::new();
    warn_if_credentials_too_open(&path, &mut out).unwrap();
    assert!(out.is_empty(), "no warning expected, got: {:?}", out);
}

#[tokio::test]
async fn dedupe_apply_requires_both_flags() {
    for flags in [&["--apply"][..], &["--delete"][..]] {
        let dir = tempfile::tempdir().unwrap();
        let (_env, cfg, creds) = isolated(&dir);
        let mut args = vec!["library", "dedupe"];
        args.extend_from_slice(flags);
        let cli = parse(&args);
        let mut out = Vec::new();
        run_with(cli, cfg, creds, &mut out).await.unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("нечего удалять"),
            "flags {flags:?} must stay a dry-run, got: {text}"
        );
    }
}

#[test]
fn csv_without_header_uses_the_positional_order() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("input.csv");
    fs::write(&path, "Radiohead,Karma Police,OK Computer,GBB971200101\n").unwrap();
    let rows = read_input_csv(&path).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].artist, "Radiohead");
    assert_eq!(rows[0].title, "Karma Police");
    assert_eq!(rows[0].album.as_deref(), Some("OK Computer"));
    assert_eq!(rows[0].isrc.as_deref(), Some("GBB971200101"));
}
