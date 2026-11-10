use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use maxfetch::config::{Config, Credentials};

/// Serialises all environment mutation: the process environment is global,
/// so tests that set/remove XDG variables must never run concurrently.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn isolate_xdg(dir: &tempfile::TempDir) -> MutexGuard<'static, ()> {
    let guard = ENV_LOCK.lock().unwrap();
    let fake = dir.path().join("xdg");
    std::fs::create_dir_all(&fake).unwrap();
    // The returned guard is held by the caller for the whole test, so no
    // other test can mutate the environment (or read it via `dirs`) meanwhile.
    unsafe {
        std::env::set_var("XDG_DATA_HOME", &fake);
        std::env::set_var("XDG_CONFIG_HOME", &fake);
    }
    guard
}

#[test]
fn defaults_are_usable_when_no_file_exists() {
    let dir = tempfile::tempdir().unwrap();
    let _env = isolate_xdg(&dir);
    let cfg = Config::load_from(dir.path()).unwrap();
    assert_eq!(cfg.matching.duration_tolerance_sec, 2);
    assert!(cfg.sources.enabled.contains(&"soulseek".to_string()));
}

#[test]
fn config_round_trips_through_disk() {
    let dir = tempfile::tempdir().unwrap();
    let _env = isolate_xdg(&dir);
    let mut cfg = Config::load_from(dir.path()).unwrap();
    cfg.library.path = "/tmp/music".into();
    cfg.sources.enabled = vec!["soulseek".into()];
    cfg.save_to(dir.path()).unwrap();

    let raw = fs::read_to_string(dir.path().join("config.toml")).unwrap();
    assert!(raw.contains("[library]"));

    let again = Config::load_from(dir.path()).unwrap();
    assert_eq!(again.library.path, PathBuf::from("/tmp/music"));
    assert_eq!(again.sources.enabled, vec!["soulseek".to_string()]);
}

#[test]
fn credentials_are_written_with_mode_0600() {
    let dir = tempfile::tempdir().unwrap();
    let _env = isolate_xdg(&dir);
    let mut creds = Credentials::default();
    creds.soulseek = Some(Default::default());
    creds.save_to(dir.path()).unwrap();
    let path = dir.path().join("credentials.toml");
    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

#[test]
fn missing_credentials_sections_are_fine() {
    let dir = tempfile::tempdir().unwrap();
    let _env = isolate_xdg(&dir);
    let creds = Credentials::load_from(dir.path()).unwrap();
    assert!(creds.soulseek.is_none());
    assert!(creds.qobuz.is_none());
}

#[test]
fn wide_permissions_are_tightened() {
    let dir = tempfile::tempdir().unwrap();
    let _env = isolate_xdg(&dir);
    let creds = Credentials::default();
    creds.save_to(dir.path()).unwrap();
    let path = dir.path().join("credentials.toml");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    let changed = maxfetch::config::ensure_private(&path).unwrap();
    assert!(changed);
    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

#[test]
fn matching_duration_tolerance_can_be_overridden() {
    let dir = tempfile::tempdir().unwrap();
    let _env = isolate_xdg(&dir);
    let mut cfg = Config::load_from(dir.path()).unwrap();
    assert_eq!(cfg.matching.duration_tolerance_sec, 2);
    assert!(!cfg.matching.require_isrc);

    cfg.matching.duration_tolerance_sec = 5;
    cfg.matching.require_isrc = true;
    cfg.save_to(dir.path()).unwrap();

    let raw = fs::read_to_string(dir.path().join("config.toml")).unwrap();
    assert!(raw.contains("[matching]"));

    let again = Config::load_from(dir.path()).unwrap();
    assert_eq!(again.matching.duration_tolerance_sec, 5);
    assert!(again.matching.require_isrc);
}

#[test]
fn path_helpers_follow_xdg_config_home() {
    let dir = tempfile::tempdir().unwrap();
    let _env = isolate_xdg(&dir);
    let fake = dir.path().join("xdg");
    unsafe {
        std::env::set_var("XDG_CONFIG_HOME", &fake);
    }
    assert_eq!(
        maxfetch::config::config_dir(),
        fake.join("maxfetch")
    );
    assert_eq!(
        maxfetch::config::credentials_path(),
        fake.join("maxfetch").join("credentials.toml")
    );
}

#[test]
fn state_path_follows_xdg_data_home() {
    let dir = tempfile::tempdir().unwrap();
    let _env = isolate_xdg(&dir);
    let fake_data = dir.path().join("data");
    std::fs::create_dir_all(&fake_data).unwrap();
    unsafe {
        std::env::set_var("XDG_DATA_HOME", &fake_data);
    }
    assert_eq!(
        maxfetch::config::state_path(),
        fake_data.join("maxfetch").join("state.db")
    );
}

#[test]
fn path_helpers_fall_back_when_xdg_unset_or_empty() {
    let dir = tempfile::tempdir().unwrap();
    let _env = isolate_xdg(&dir);

    // Unset branch: neither variable set, helpers must fall back to home
    // directories and return absolute paths rather than panicking.
    unsafe {
        std::env::remove_var("XDG_CONFIG_HOME");
        std::env::remove_var("XDG_DATA_HOME");
    }
    let config_dir = maxfetch::config::config_dir();
    assert!(config_dir.is_absolute());
    assert!(config_dir.ends_with(PathBuf::from(".config/maxfetch")));
    let state_path = maxfetch::config::state_path();
    assert!(state_path.is_absolute());
    assert!(state_path.ends_with(PathBuf::from("maxfetch/state.db")));

    // Empty-string branch: treated the same as unset.
    unsafe {
        std::env::set_var("XDG_CONFIG_HOME", "");
        std::env::set_var("XDG_DATA_HOME", "");
    }
    let config_dir = maxfetch::config::config_dir();
    assert!(config_dir.is_absolute());
    assert!(config_dir.ends_with(PathBuf::from(".config/maxfetch")));
    let state_path = maxfetch::config::state_path();
    assert!(state_path.is_absolute());
    assert!(state_path.ends_with(PathBuf::from("maxfetch/state.db")));
}
