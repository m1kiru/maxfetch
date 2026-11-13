use std::path::PathBuf;

use maxfetch::model::Container;
use maxfetch::sources::youtube::split_artist_title;
use maxfetch::youtube::{YoutubeTrackMeta, finalize, move_sidecar, sidecar_payload, untagged};

fn fixture_flac() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/probe/sine_44100_s16.flac")
}

fn meta() -> YoutubeTrackMeta {
    YoutubeTrackMeta {
        title: "Get Lucky".into(),
        artist: "Daft Punk".into(),
        album: Some("Random Access Memories".into()),
    }
}

/// Builds an Opus-in-WebM file without re-encoding the fixture container.
fn webm_fixture(dir: &std::path::Path) -> PathBuf {
    let out = dir.join("abc123.webm");
    let status = std::process::Command::new("ffmpeg")
        .args(["-nostdin", "-y", "-i"])
        .arg(fixture_flac())
        .args(["-vn", "-c:a", "libopus", "-b:a", "160k"])
        .arg(&out)
        .status()
        .unwrap();
    assert!(status.success());
    out
}

#[test]
fn a_remuxed_youtube_file_is_opus_and_carries_no_sidecar() {
    let dir = tempfile::tempdir().unwrap();
    let src = webm_fixture(dir.path());
    let dest = dir.path().join("out.bin");
    let done = finalize("ffmpeg", &src, &dest, "abc123", &meta()).unwrap();
    assert!(!done.untagged);
    assert_eq!(done.container, Container::Opus);
    assert!(done.sidecar.is_none());
    assert!(!untagged(&dest));
    let probed = maxfetch::probe::probe(&dest).unwrap();
    assert_eq!(probed.container, Container::Opus);
}

/// AAC bytes under a `.webm` name: `ffmpeg` cannot mux AAC into a real WebM, so
/// the m4a is produced first and then renamed to the misleading extension.
fn aac_fixture(dir: &std::path::Path) -> PathBuf {
    let m4a = dir.join("aac.m4a");
    let status = std::process::Command::new("ffmpeg")
        .args(["-nostdin", "-y", "-i"])
        .arg(fixture_flac())
        .args(["-vn", "-c:a", "aac", "-b:a", "128k"])
        .arg(&m4a)
        .status()
        .unwrap();
    assert!(status.success());
    let out = dir.join("abc123.webm");
    std::fs::rename(&m4a, &out).unwrap();
    out
}

#[test]
fn an_aac_stream_under_a_webm_name_reports_its_real_container() {
    let dir = tempfile::tempdir().unwrap();
    let src = aac_fixture(dir.path());
    let dest = dir.path().join("out.webm");
    let done = finalize("ffmpeg", &src, &dest, "abc123", &meta()).unwrap();
    assert!(done.untagged);
    assert_eq!(done.container, Container::M4aAac);
    assert_ne!(done.container, Container::Webm);
    assert_ne!(done.container, Container::Opus);
    assert_eq!(dest.extension().and_then(|e| e.to_str()), Some("webm"));
    assert!(done.sidecar.is_some());
    assert!(untagged(&dest));
}

#[test]
fn a_remux_failing_on_an_unusual_input_falls_back_without_panicking() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("abc123.webm");
    std::fs::write(&src, b"not a container ffmpeg can demux").unwrap();
    let dest = dir.path().join("out.webm");
    let done = finalize("ffmpeg", &src, &dest, "abc123", &meta()).unwrap();
    assert!(done.untagged);
    assert_ne!(done.container, Container::Opus);
    assert!(done.sidecar.is_some());
    assert!(untagged(&dest));
}

#[test]
fn a_channel_name_is_split_only_when_the_title_carries_a_separator() {
    assert_eq!(
        split_artist_title("Daft Punk - Get Lucky.webm"),
        Some(("Daft Punk".into(), "Get Lucky".into()))
    );
    assert_eq!(split_artist_title("Get Lucky.webm"), None);
    assert_eq!(split_artist_title(" - Get Lucky.webm"), None);
}

#[test]
fn a_missing_ffmpeg_leaves_webm_and_writes_the_sidecar() {
    let dir = tempfile::tempdir().unwrap();
    let src = webm_fixture(dir.path());
    let dest = dir.path().join("out.bin");
    let done = finalize("/nonexistent/ffmpeg", &src, &dest, "abc123", &meta()).unwrap();
    assert!(done.untagged);
    assert_eq!(done.container, Container::Webm);
    assert_eq!(
        done.sidecar.as_deref(),
        Some(dir.path().join("out.bin.json").as_path())
    );
    assert!(untagged(&dest));
    let raw = std::fs::read_to_string(dir.path().join("out.bin.json")).unwrap();
    assert!(raw.contains("youtube"));
    assert!(raw.contains("abc123"));
    assert!(raw.contains("Get Lucky"));
}

#[test]
fn the_sidecar_payload_names_the_source_and_the_video() {
    let v = sidecar_payload("abc123", &meta());
    assert_eq!(v["source"], "youtube");
    assert_eq!(v["video_id"], "abc123");
    assert_eq!(v["artist"], "Daft Punk");
    assert_eq!(v["album"], "Random Access Memories");
}

#[test]
fn the_sidecar_follows_the_file_on_relocation() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path().join("tmp.bin");
    std::fs::write(&tmp, b"x").unwrap();
    std::fs::write(dir.path().join("tmp.bin.json"), b"{}").unwrap();
    let final_path = dir.path().join("01 Get Lucky.webm");
    let moved = move_sidecar(&tmp, &final_path).unwrap();
    assert_eq!(
        moved.as_deref(),
        Some(final_path.with_extension("webm.json").as_path())
    );
    assert!(final_path.with_extension("webm.json").exists());
    assert!(!dir.path().join("tmp.bin.json").exists());
}

#[test]
fn relocation_without_a_sidecar_is_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path().join("tmp.opus");
    std::fs::write(&tmp, b"x").unwrap();
    assert!(
        move_sidecar(&tmp, &dir.path().join("01 Get Lucky.opus"))
            .unwrap()
            .is_none()
    );
}
