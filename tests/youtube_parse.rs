use maxfetch::model::Container;
use maxfetch::youtube::{
    bestaudio_quality, binary_status, is_playlist_url, parse_flat_playlist, parse_video, run_json,
};
use std::path::PathBuf;

fn fixture_json(name: &str) -> serde_json::Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/youtube")
        .join(name);
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn fake_ytdlp() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/bin/yt-dlp")
}

#[test]
fn a_flat_playlist_yields_videos_with_ids_and_durations() {
    let videos = parse_flat_playlist(&fixture_json("playlist.json")).unwrap();
    assert_eq!(videos.len(), 2);
    assert_eq!(videos[0].video_id, "abc123");
    assert_eq!(videos[0].title, "Daft Punk - Get Lucky");
    assert_eq!(videos[0].duration_sec, Some(369.0));
    assert_eq!(videos[0].url, "https://www.youtube.com/watch?v=abc123");
}

#[test]
fn track_fields_become_the_tag_metadata() {
    let meta = parse_video(&fixture_json("video.json")).unwrap();
    assert_eq!(meta.title, "Get Lucky");
    assert_eq!(meta.artist, "Daft Punk");
    assert_eq!(meta.album.as_deref(), Some("Random Access Memories"));
}

#[test]
fn a_video_without_track_fields_falls_back_to_the_uploader() {
    let json: serde_json::Value =
        serde_json::from_str(r#"{"title":"Some Clip","uploader":"Chan","formats":[]}"#).unwrap();
    let meta = parse_video(&json).unwrap();
    assert_eq!(meta.title, "Some Clip");
    assert_eq!(meta.artist, "Chan");
    assert_eq!(meta.album, None);
}

#[test]
fn a_topic_channel_is_a_loader_not_an_artist() {
    let json: serde_json::Value = serde_json::from_str(r#"{"title":"Get Lucky","channel":"Daft Punk - Topic","uploader":"Daft Punk - Topic","duration":369}"#).unwrap();
    let meta = parse_video(&json).unwrap();
    assert_eq!(meta.artist, "Daft Punk");
    assert!(!meta.artist.contains("Topic"));
}

#[test]
fn a_three_hour_dj_set_is_rejected() {
    let json: serde_json::Value =
        serde_json::from_str(r#"{"title":"Boiler Room Set","channel":"Some DJ","duration":10800}"#)
            .unwrap();
    let err = parse_video(&json).unwrap_err();
    assert!(format!("{err}").contains("DJ"));
    assert_eq!(maxfetch::youtube::MAX_TRACK_SECONDS, 900);
}

#[test]
fn a_radio_station_uploader_becomes_the_artist_without_inventing_data() {
    let json: serde_json::Value = serde_json::from_str(
        r#"{"title":"Midnight Hour","uploader":"Radio Nimbus","duration":210}"#,
    )
    .unwrap();
    let meta = parse_video(&json).unwrap();
    assert_eq!(meta.title, "Midnight Hour");
    assert_eq!(meta.artist, "Radio Nimbus");
    assert_eq!(meta.album, None);
}

#[test]
fn bestaudio_is_opus_at_160_kbps_and_is_lossy() {
    let q = bestaudio_quality(&fixture_json("video.json"));
    assert!(!q.lossless);
    assert_eq!(q.container, Container::Opus);
    assert_eq!(q.sample_rate, Some(48000));
    assert_eq!(q.bitrate_kbps, Some(160));
}

#[test]
fn the_fake_binary_answers_with_the_canned_json() {
    let v = run_json(
        fake_ytdlp().to_str().unwrap(),
        &[
            "-J",
            "--flat-playlist",
            "https://www.youtube.com/playlist?list=PL1",
        ],
    )
    .unwrap();
    assert_eq!(v["entries"].as_array().unwrap().len(), 3);
}

#[test]
fn a_missing_binary_is_an_error_not_a_panic() {
    let err = run_json("/nonexistent/yt-dlp", &["-J", "x"]).unwrap_err();
    assert!(format!("{err}").contains("yt-dlp"));
    assert!(
        binary_status("/nonexistent/yt-dlp")
            .unwrap()
            .contains("yt-dlp")
    );
    assert!(binary_status("/bin/sh").is_none());
}

#[test]
fn playlist_urls_are_recognized() {
    assert!(is_playlist_url("https://www.youtube.com/playlist?list=PL1"));
    assert!(is_playlist_url(
        "https://www.youtube.com/watch?v=x&list=PL1"
    ));
    assert!(!is_playlist_url("https://www.youtube.com/watch?v=x"));
}
