mod support;

use maxfetch::model::{Container, SourceId};
use maxfetch::sources::soundcloud::{
    AudioFormat, SoundCloudFormat, SoundCloudTrack, Transcoding, format_of, parse_track,
};
use support::fixture;

fn read(rel: &str) -> String {
    std::fs::read_to_string(fixture(rel)).unwrap()
}

fn track() -> SoundCloudTrack {
    parse_track(&read("soundcloud/track.json")).unwrap()
}

#[test]
fn track_metadata_maps_to_a_release() {
    let release = track().release_from_track();
    assert_eq!(release.source, SourceId::SoundCloud);
    assert_eq!(release.source_item_id, "880017");
    assert_eq!(release.title, "Neon");
    assert_eq!(release.artist, "Nightwave");
    assert_eq!(release.album, None);
    assert_eq!(release.track_no, None);
    assert_eq!(release.duration_sec, Some(214.0));
    assert_eq!(release.year, Some(2023));
}

#[test]
fn the_isrc_comes_from_publisher_metadata() {
    assert_eq!(
        track().release_from_track().isrc.as_deref(),
        Some("DEP960300042")
    );
}

#[test]
fn the_publisher_artist_wins_over_the_username() {
    let mut t = track();
    t.publisher_artist = Some("Nightwave Music".into());
    assert_eq!(t.release_from_track().artist, "Nightwave Music");
    let mut t = track();
    t.publisher_artist = None;
    assert_eq!(t.release_from_track().artist, "nightwave");
}

#[test]
fn duration_is_converted_from_milliseconds() {
    assert_eq!(track().duration_ms, 214000);
    assert_eq!(track().release_from_track().duration_sec, Some(214.0));
}

#[test]
fn aac_160k_is_classified_as_lossy() {
    let format = AudioFormat::from_transcoding(&Transcoding {
        preset: "aac_160k".into(),
        protocol: "hls".into(),
        mime_type: "audio/mp4; codecs=\"mp4a.40.2\"".into(),
    })
    .unwrap();
    assert_eq!(format, AudioFormat::LossyAac { bitrate_kbps: 160 });
    let quality = format.quality();
    assert!(!quality.lossless);
    assert_eq!(quality.container, Container::M4aAac);
    assert_eq!(quality.bitrate_kbps, Some(160));
}

#[test]
fn aac_96k_is_classified_as_lossy_at_ninety_six() {
    let format = AudioFormat::from_transcoding(&Transcoding {
        preset: "aac_96k".into(),
        protocol: "hls".into(),
        mime_type: "audio/mp4; codecs=\"mp4a.40.2\"".into(),
    })
    .unwrap();
    assert_eq!(format, AudioFormat::LossyAac { bitrate_kbps: 96 });
    assert_eq!(format.quality().bitrate_kbps, Some(96));
}

#[test]
fn a_progressive_stream_is_a_lossless_candidate() {
    let format = AudioFormat::from_transcoding(&Transcoding {
        preset: "original".into(),
        protocol: "http_mp3".into(),
        mime_type: "audio/mpeg".into(),
    })
    .unwrap();
    assert_eq!(
        format,
        AudioFormat::ProgressiveOriginal {
            protocol: "http_mp3".into()
        }
    );
    let quality = format.quality();
    assert!(quality.lossless);
    assert_eq!(quality.container, Container::Mp3);
    assert_eq!(quality.bit_depth, None);
    assert_eq!(quality.bitrate_kbps, None);
}

#[test]
fn without_oauth_the_pick_is_aac_160k() {
    let format = AudioFormat::pick(&track().transcodings).unwrap();
    assert_eq!(format, AudioFormat::LossyAac { bitrate_kbps: 160 });
}

#[test]
fn a_progressive_stream_beats_the_hls_streams() {
    let format = AudioFormat::pick(
        &parse_track(&read("soundcloud/track_progressive.json"))
            .unwrap()
            .transcodings,
    )
    .unwrap();
    assert!(matches!(format, AudioFormat::ProgressiveOriginal { .. }));
}

#[test]
fn an_unknown_transcoding_is_ignored() {
    assert_eq!(
        AudioFormat::from_transcoding(&Transcoding {
            preset: "opus_0_0".into(),
            protocol: "hls".into(),
            mime_type: "audio/ogg".into()
        }),
        None
    );
    assert_eq!(AudioFormat::pick(&[]), None);
}

#[test]
fn the_ytdlp_format_name_matches_the_transcoding_preset() {
    assert_eq!(
        maxfetch::sources::soundcloud::ytdlp_format(&AudioFormat::LossyAac { bitrate_kbps: 160 }),
        "hls_aac_160k"
    );
    assert_eq!(
        maxfetch::sources::soundcloud::ytdlp_format(&AudioFormat::LossyAac { bitrate_kbps: 96 }),
        "hls_aac_96k"
    );
    assert_eq!(
        maxfetch::sources::soundcloud::ytdlp_format(&AudioFormat::ProgressiveOriginal {
            protocol: "progressive".into()
        }),
        "bestaudio"
    );
}

#[test]
fn the_locator_format_names_the_picked_stream() {
    assert_eq!(
        format_of(&AudioFormat::LossyAac { bitrate_kbps: 160 }),
        SoundCloudFormat::Aac160
    );
    assert_eq!(
        format_of(&AudioFormat::LossyAac { bitrate_kbps: 96 }),
        SoundCloudFormat::Aac96
    );
    assert_eq!(
        format_of(&AudioFormat::ProgressiveOriginal {
            protocol: "progressive".into()
        }),
        SoundCloudFormat::Original
    );
}
