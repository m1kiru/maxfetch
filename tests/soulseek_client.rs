use std::path::Path;
use std::time::Duration;

use maxfetch::config::{DownloadConfig, SoulseekCredentials};
use maxfetch::fetch::NoopSink;
use maxfetch::model::{Container, Quality};
use maxfetch::sources::soulseek::{
    PeerCandidate, SoulseekSource, Transfer, download_ranked, drain_transfer, release_from_filename,
};
use maxfetch::sources::{AuthState, Source};
use soulseek_rs::DownloadStatus;

fn peer(user: &str) -> PeerCandidate {
    PeerCandidate {
        username: user.into(),
        filename: "Daft Punk - Get Lucky.flac".into(),
        size: 4,
        slots: 1,
        speed: 4,
        quality: Quality {
            lossless: true,
            container: Container::Flac,
            bit_depth: Some(16),
            sample_rate: Some(44100),
            bitrate_kbps: None,
        },
    }
}

struct Script {
    /// One entry per attempt: `true` succeeds, `false` fails.
    script: std::sync::Mutex<Vec<bool>>,
}

impl Transfer for Script {
    fn fetch(
        &self,
        peer: &PeerCandidate,
        dest: &Path,
        _sink: &dyn maxfetch::fetch::ProgressSink,
    ) -> anyhow::Result<u64> {
        let ok = self.script.lock().unwrap().remove(0);
        if ok {
            let body = peer.username.clone();
            std::fs::write(dest, &body)?;
            Ok(body.len() as u64)
        } else {
            anyhow::bail!("peer {} hung up", peer.username)
        }
    }
}

#[test]
fn a_silent_transfer_is_reported_as_stalled() {
    let (tx, rx) = std::sync::mpsc::channel::<DownloadStatus>();
    tx.send(DownloadStatus::InProgress {
        bytes_downloaded: 1024,
        total_bytes: 8192,
        speed_bytes_per_sec: 100.0,
    })
    .unwrap();
    let err = drain_transfer(rx, Duration::from_millis(50), &NoopSink).unwrap_err();
    assert!(format!("{err}").contains("stalled"));
    drop(tx);
}

#[test]
fn a_completed_transfer_returns_the_total_byte_count() {
    let (tx, rx) = std::sync::mpsc::channel::<DownloadStatus>();
    tx.send(DownloadStatus::Queued).unwrap();
    tx.send(DownloadStatus::InProgress {
        bytes_downloaded: 1024,
        total_bytes: 2048,
        speed_bytes_per_sec: 512.0,
    })
    .unwrap();
    tx.send(DownloadStatus::Completed).unwrap();
    assert_eq!(
        drain_transfer(rx, Duration::from_secs(5), &NoopSink).unwrap(),
        2048
    );
}

#[test]
fn a_timed_out_status_is_an_error_that_names_the_reason() {
    let (tx, rx) = std::sync::mpsc::channel::<DownloadStatus>();
    tx.send(DownloadStatus::TimedOut).unwrap();
    let err = drain_transfer(rx, Duration::from_secs(5), &NoopSink).unwrap_err();
    assert!(format!("{err}").contains("timed out"));
}

#[test]
fn a_failed_status_carries_the_peer_message() {
    let (tx, rx) = std::sync::mpsc::channel::<DownloadStatus>();
    tx.send(DownloadStatus::Failed(Some("no such file".into())))
        .unwrap();
    let err = drain_transfer(rx, Duration::from_secs(5), &NoopSink).unwrap_err();
    assert!(format!("{err}").contains("no such file"));
}

#[test]
fn a_dropped_sender_is_a_disconnect() {
    let (tx, rx) = std::sync::mpsc::channel::<DownloadStatus>();
    drop(tx);
    let err = drain_transfer(rx, Duration::from_secs(5), &NoopSink).unwrap_err();
    assert!(format!("{err}").contains("disconnected"));
}

#[test]
fn a_failing_peer_falls_over_to_the_next_one() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("track.flac");
    let script = Script {
        script: std::sync::Mutex::new(vec![false, true]),
    };
    let ranked = vec![peer("bad"), peer("good")];
    let chosen = download_ranked(&script, &ranked, &dest, &NoopSink).unwrap();
    assert_eq!(chosen.username, "good");
    assert_eq!(std::fs::read_to_string(&dest).unwrap(), "good");
    assert!(!dir.path().join("track.part").exists());
}

#[test]
fn every_peer_failing_is_an_error_with_the_partial_file_removed() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("track.flac");
    let script = Script {
        script: std::sync::Mutex::new(vec![false, false]),
    };
    let err = download_ranked(&script, &[peer("a"), peer("b")], &dest, &NoopSink).unwrap_err();
    assert!(format!("{err}").contains("hung up"));
    assert!(!dir.path().join("track.part").exists());
    assert!(!dest.exists());
}

struct HalfWriter;

impl Transfer for HalfWriter {
    fn fetch(
        &self,
        peer: &PeerCandidate,
        dest: &Path,
        _sink: &dyn maxfetch::fetch::ProgressSink,
    ) -> anyhow::Result<u64> {
        std::fs::write(dest, vec![0u8; 4096])?;
        anyhow::bail!("peer {} dropped after 4096 bytes", peer.username)
    }
}

#[test]
fn an_interrupted_transfer_leaves_no_partial_file_behind() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("track.flac");
    let err = download_ranked(&HalfWriter, &[peer("half")], &dest, &NoopSink).unwrap_err();
    assert!(format!("{err}").contains("4096"));
    assert!(!dir.path().join("track.part").exists());
    assert!(!dest.exists());
}

struct StaleChecker {
    /// Whether `<basename>` and `<basename>.part` were both absent on entry.
    clean_on_entry: std::sync::Mutex<Vec<bool>>,
}

impl Transfer for StaleChecker {
    fn fetch(
        &self,
        _peer: &PeerCandidate,
        dest: &Path,
        _sink: &dyn maxfetch::fetch::ProgressSink,
    ) -> anyhow::Result<u64> {
        let dir = dest.parent().unwrap();
        let finished = dir.join("Daft Punk - Get Lucky.flac");
        let resumed = dir.join("Daft Punk - Get Lucky.flac.part");
        self.clean_on_entry
            .lock()
            .unwrap()
            .push(!finished.exists() && !resumed.exists());
        std::fs::write(dest, b"ok")?;
        Ok(2)
    }
}

#[test]
fn every_attempt_removes_the_crates_leftover_files_first() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("track.flac");
    let finished = dir.path().join("Daft Punk - Get Lucky.flac");
    let resumed = dir.path().join("Daft Punk - Get Lucky.flac.part");
    std::fs::write(&finished, [7u8; 4]).unwrap();
    std::fs::write(&resumed, vec![0u8; 4096]).unwrap();
    let checker = StaleChecker {
        clean_on_entry: std::sync::Mutex::new(Vec::new()),
    };
    let chosen = download_ranked(&checker, &[peer("good")], &dest, &NoopSink).unwrap();
    assert_eq!(chosen.username, "good");
    assert_eq!(checker.clean_on_entry.lock().unwrap().clone(), vec![true]);
    assert_eq!(std::fs::read_to_string(&dest).unwrap(), "ok");
    assert!(!finished.exists());
    assert!(!resumed.exists());
}

#[test]
fn a_failed_rename_does_not_leave_the_partial_file_behind() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("track.flac");
    // A directory squatting on `dest` makes the final rename fail.
    std::fs::create_dir(&dest).unwrap();
    let script = Script {
        script: std::sync::Mutex::new(vec![true]),
    };
    assert!(download_ranked(&script, &[peer("good")], &dest, &NoopSink).is_err());
    assert!(!dir.path().join("track.part").exists());
}

#[test]
fn an_empty_peer_list_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let script = Script {
        script: std::sync::Mutex::new(vec![]),
    };
    assert!(download_ranked(&script, &[], &dir.path().join("x.flac"), &NoopSink).is_err());
}

#[test]
fn filenames_are_split_into_artist_and_title() {
    assert_eq!(
        release_from_filename("Daft Punk - Get Lucky.flac"),
        (Some("Daft Punk".to_string()), "Get Lucky".to_string())
    );
    assert_eq!(
        release_from_filename("Get Lucky.mp3"),
        (None, "Get Lucky".to_string())
    );
}

#[test]
fn a_source_without_credentials_is_missing() {
    let empty = SoulseekSource::new(SoulseekCredentials::default(), DownloadConfig::default());
    assert_eq!(empty.auth_state(), AuthState::Missing);
    let filled = SoulseekSource::new(
        SoulseekCredentials {
            username: "u".into(),
            password: "p".into(),
        },
        DownloadConfig::default(),
    );
    assert_eq!(filled.auth_state(), AuthState::Ok);
}
