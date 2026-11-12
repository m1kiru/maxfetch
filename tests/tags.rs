use maxfetch::model::{Container, Quality, SourceId};
use maxfetch::probe::probe;
use maxfetch::tags::{TagMeta, lossy_comment, read_tags, write_tags};
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/probe/sine_44100_s16.flac");
    let dst = std::env::temp_dir().join(format!(
        "maxfetch_tags_{}_{}.flac",
        std::process::id(),
        name
    ));
    std::fs::copy(&src, &dst).expect("copy fixture");
    dst
}

fn meta(quality: Quality, source: SourceId) -> TagMeta {
    TagMeta {
        title: "Sine Wave".to_string(),
        artist: "Test Artist".to_string(),
        album: Some("Test Album".to_string()),
        track_no: Some(7),
        year: Some(2026),
        isrc: Some("USUM71700001".to_string()),
        quality,
        source,
        acquired_at: "2026-11-10T00:00:00Z".to_string(),
        quality_comment: String::new(),
    }
}

fn lossless_flac() -> Quality {
    Quality {
        lossless: true,
        container: Container::Flac,
        bit_depth: Some(16),
        sample_rate: Some(44100),
        bitrate_kbps: None,
    }
}

#[test]
fn writes_core_fields_and_isrc() {
    let path = fixture("core");
    let m = meta(lossless_flac(), SourceId::Bandcamp);
    write_tags(&path, &m).unwrap();

    let back = read_tags(&path).unwrap();
    assert_eq!(back.title, "Sine Wave");
    assert_eq!(back.artist, "Test Artist");
    assert_eq!(back.album.as_deref(), Some("Test Album"));
    assert_eq!(back.track_no, Some(7));
    assert_eq!(back.year, Some(2026));
    assert_eq!(back.isrc.as_deref(), Some("USUM71700001"));

    // The audio itself must stay untouched.
    let p = probe(&path).unwrap();
    assert_eq!(p.container, Container::Flac);
    assert_eq!(p.sample_rate, Some(44100));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn lossy_files_get_the_honest_marker() {
    let path = fixture("lossy");
    let quality = Quality {
        lossless: false,
        container: Container::Opus,
        bit_depth: None,
        sample_rate: Some(48000),
        bitrate_kbps: Some(160),
    };
    let m = meta(quality, SourceId::Youtube);
    write_tags(&path, &m).unwrap();

    let comment = read_tags(&path).unwrap().quality_comment;
    assert!(comment.contains("source=lossy"), "comment was: {comment}");
    assert!(comment.contains("container=opus"), "comment was: {comment}");
    assert!(comment.contains("source_id=youtube"), "comment was: {comment}");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn lossless_files_get_no_lossy_marker() {
    let path = fixture("nolossy");
    let m = meta(lossless_flac(), SourceId::Bandcamp);
    write_tags(&path, &m).unwrap();

    let comment = read_tags(&path).unwrap().quality_comment;
    assert!(!comment.contains("source=lossy"), "comment was: {comment}");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn lossy_comment_format_is_exact() {
    assert_eq!(
        lossy_comment(SourceId::Youtube, Container::Opus, "2026-11-10T00:00:00Z"),
        "maxfetch: source=lossy; container=opus; source_id=youtube; acquired=2026-11-10T00:00:00Z"
    );
}
