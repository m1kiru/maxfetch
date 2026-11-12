//! Authorisation status and login-stub tests (`src/auth.rs`).
//!
//! `auth_status` is a pure function over `Credentials`: no test here touches
//! the network or the real `$XDG_CONFIG_HOME` except
//! `save_credentials_writes_a_private_file`, which isolates all three XDG roots
//! into a `tempfile` directory while `ENV_LOCK` serialises the mutation.

use std::os::unix::fs::PermissionsExt;
use std::sync::{Mutex, MutexGuard};

use maxfetch::auth::{auth_status, login, save_credentials};
use maxfetch::config::{self, Credentials, SoulseekCredentials};
use maxfetch::model::SourceId;
use maxfetch::sources::AuthState;

/// Serialises all environment mutation: the process environment is global,
/// so tests that set/remove XDG variables must never run concurrently.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Points all three XDG roots into `dir` and returns the lock held for the
/// whole test.
fn isolate_xdg(dir: &tempfile::TempDir) -> MutexGuard<'static, ()> {
    let guard = ENV_LOCK.lock().unwrap();
    let fake = dir.path().join("xdg");
    std::fs::create_dir_all(&fake).unwrap();
    unsafe {
        std::env::set_var("XDG_DATA_HOME", &fake);
        std::env::set_var("XDG_CONFIG_HOME", &fake);
        std::env::set_var("XDG_STATE_HOME", &fake);
    }
    guard
}

#[test]
fn status_reports_missing_credentials() {
    let creds = Credentials::default();
    let statuses = auth_status(&creds);
    let soulseek = statuses
        .iter()
        .find(|s| s.id == SourceId::Soulseek)
        .unwrap();
    assert_eq!(soulseek.auth, AuthState::Missing);
}

#[test]
fn status_reports_present_credentials() {
    let mut creds = Credentials::default();
    creds.soulseek = Some(Default::default());
    let statuses = auth_status(&creds);
    let soulseek = statuses
        .iter()
        .find(|s| s.id == SourceId::Soulseek)
        .unwrap();
    assert_eq!(soulseek.auth, AuthState::Ok);
}

#[tokio::test]
async fn login_is_not_implemented_in_the_core_build() {
    let mut creds = Credentials::default();
    let err = login("spotify", &mut creds).await.unwrap_err();
    assert!(format!("{err}").contains("not implemented"));
}

#[test]
fn status_covers_every_source_once() {
    let statuses = auth_status(&Credentials::default());
    assert_eq!(statuses.len(), 9);

    let expected = [
        SourceId::Soulseek,
        SourceId::Bandcamp,
        SourceId::Youtube,
        SourceId::Spotify,
        SourceId::SoundCloud,
        SourceId::Qobuz,
        SourceId::Tidal,
        SourceId::Deezer,
        SourceId::Beatport,
    ];
    let ids: Vec<SourceId> = statuses.iter().map(|s| s.id).collect();
    assert_eq!(ids, expected, "one status per source, in as_str order");

    let mut unique = ids.clone();
    unique.sort_by_key(|id| id.as_str());
    unique.dedup();
    assert_eq!(unique.len(), ids.len(), "no source is reported twice");
}

#[test]
fn status_reports_youtube_as_not_needed() {
    let statuses = auth_status(&Credentials::default());
    let youtube = statuses.iter().find(|s| s.id == SourceId::Youtube).unwrap();
    assert_eq!(youtube.auth, AuthState::NotNeeded);
}

#[test]
fn status_ignores_empty_credential_sections() {
    let mut creds = Credentials::default();
    creds.qobuz = Some(Default::default());
    let statuses = auth_status(&creds);

    let qobuz = statuses.iter().find(|s| s.id == SourceId::Qobuz).unwrap();
    assert_eq!(
        qobuz.auth,
        AuthState::Ok,
        "an empty section is still a section"
    );

    let tidal = statuses.iter().find(|s| s.id == SourceId::Tidal).unwrap();
    assert_eq!(tidal.auth, AuthState::Missing);
}

#[test]
fn save_credentials_writes_a_private_file() {
    let dir = tempfile::tempdir().unwrap();
    let _env = isolate_xdg(&dir);

    let creds = Credentials {
        soulseek: Some(SoulseekCredentials {
            username: "u".into(),
            password: "p".into(),
        }),
        ..Credentials::default()
    };
    save_credentials(&creds).unwrap();

    let path = config::credentials_path();
    assert_eq!(
        path,
        dir.path().join("xdg").join("maxfetch/credentials.toml")
    );

    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "credentials must be private from creation");

    let loaded = Credentials::load_from(&config::config_dir()).unwrap();
    let loaded = loaded.soulseek.expect("the section round-trips");
    assert_eq!(loaded.username, "u");
    assert_eq!(loaded.password, "p");
}
