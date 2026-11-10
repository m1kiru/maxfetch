use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use maxfetch::config::{Config, Credentials};

fn isolate_xdg(dir: &tempfile::TempDir) {
    let fake = dir.path().join("xdg");
    std::fs::create_dir_all(&fake).unwrap();
    // SAFETY: tests run in the same process; each test points XDG at its own
    // tempdir before touching any path helper, so no cross-test reads occur.
    unsafe {
        std::env::set_var("XDG_DATA_HOME", &fake);
        std::env::set_var("XDG_CONFIG_HOME", &fake);
    }
}

#[test]
fn defaults_are_usable_when_no_file_exists() {
    let dir = tempfile::tempdir().unwrap();
    isolate_xdg(&dir);
    let cfg = Config::load_from(dir.path()).unwrap();
    assert_eq!(cfg.matching.duration_tolerance_sec, 2);
    assert!(cfg.sources.enabled.contains(&"soulseek".to_string()));
}

#[test]
fn config_round_trips_through_disk() {
    let dir = tempfile::tempdir().unwrap();
    isolate_xdg(&dir);
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
    isolate_xdg(&dir);
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
    isolate_xdg(&dir);
    let creds = Credentials::load_from(dir.path()).unwrap();
    assert!(creds.soulseek.is_none());
    assert!(creds.qobuz.is_none());
}

#[test]
fn wide_permissions_are_tightened() {
    let dir = tempfile::tempdir().unwrap();
    isolate_xdg(&dir);
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
    isolate_xdg(&dir);
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
