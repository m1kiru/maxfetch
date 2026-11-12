use maxfetch::media::{ffmpeg_available, remux_to_opus, write_sidecar};
use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/probe/sine_44100_s16.flac")
}

/// Encodes the FLAC fixture into an Opus/WebM file. `libopus` is acceptable
/// here because this is test input preparation, not the production remux path.
fn webm_input(dir: &Path) -> PathBuf {
    let webm = dir.join("clip.webm");
    let status = Command::new("ffmpeg")
        .args(["-nostdin", "-y", "-i"])
        .arg(fixture())
        .args(["-vn", "-c", "copy", "-c:a", "libopus"])
        .arg(&webm)
        .status()
        .expect("spawn ffmpeg to build test input");
    assert!(status.success(), "preparing webm input failed");
    webm
}

#[test]
fn ffmpeg_is_detected_on_this_machine() {
    assert!(ffmpeg_available("ffmpeg"));
}

#[test]
fn missing_ffmpeg_binary_is_not_available() {
    assert!(!ffmpeg_available("maxfetch-no-such-ffmpeg-binary"));
}

#[test]
fn remux_copies_the_stream_without_reencoding() {
    let dir = tempfile::tempdir().expect("tempdir");
    let webm = webm_input(dir.path());
    let out = dir.path().join("clip.opus");

    remux_to_opus("ffmpeg", &webm, &out).expect("remux should succeed");

    assert!(out.exists());
    assert!(std::fs::metadata(&out).expect("metadata").len() > 1000);
    let bytes = std::fs::read(&out).expect("read remuxed output");
    assert_eq!(&bytes[..4], b"OggS", "output must be an Ogg container");
}

#[test]
fn sidecar_json_is_written_next_to_the_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let media = dir.path().join("clip.webm");
    std::fs::write(&media, b"x").expect("write media placeholder");

    write_sidecar(&media, &serde_json::json!({"source": "youtube"})).expect("write sidecar");

    let body = std::fs::read_to_string(dir.path().join("clip.webm.json")).expect("read sidecar");
    assert!(body.contains("youtube"));
    assert!(!dir.path().join("clip.json").exists());
}

#[test]
fn remux_fails_with_stderr_tail() {
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = dir.path().join("missing.webm");
    let out = dir.path().join("out.opus");

    let err = remux_to_opus("ffmpeg", &missing, &out).expect_err("remux of missing input must fail");
    assert!(err.to_string().contains("ffmpeg"), "error was: {err}");
}