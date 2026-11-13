mod support;

use maxfetch::model::Container;
use maxfetch::sources::bandcamp::{
    FREE_DOWNLOAD_FORMATS, format_quality, is_expired_download, parse_data_tralbum,
    parse_statdownload_result, retry_encoding, statdownload_url,
};
use support::fixture;

fn read(rel: &str) -> String {
    std::fs::read_to_string(fixture(rel)).unwrap()
}

#[test]
fn the_free_download_page_is_read_from_the_data_tralbum_script() {
    let page = parse_data_tralbum(&read("bandcamp/album_page.html")).unwrap();
    assert_eq!(
        page.free_download_page.as_deref(),
        Some("https://nightwave.bandcamp.com/album/night-drive/download")
    );
}

#[test]
fn a_page_without_a_data_tralbum_blob_has_no_free_download_page() {
    assert_eq!(parse_data_tralbum("<html></html>"), None);
}

#[test]
fn a_plain_download_url_is_rewritten_to_statdownload_with_a_nonce() {
    let url = statdownload_url(
        "https://nightwave.bandcamp.com/album/night-drive/download",
        12345,
    )
    .unwrap();
    assert_eq!(
        url,
        "https://nightwave.bandcamp.com/album/night-drive/statdownload?&.rand=12345"
    );
}

#[test]
fn a_download_url_that_already_has_a_query_keeps_it_before_the_nonce() {
    let url = statdownload_url(
        "https://nightwave.bandcamp.com/album/night-drive/download?from=embed",
        12345,
    )
    .unwrap();
    assert_eq!(
        url,
        "https://nightwave.bandcamp.com/album/night-drive/statdownload?from=embed&.rand=12345"
    );
}

#[test]
fn a_url_without_a_download_segment_is_rejected() {
    assert!(statdownload_url("https://nightwave.bandcamp.com/album/night-drive", 1).is_none());
}

#[test]
fn the_expired_403_body_is_recognised() {
    let body = read("bandcamp/statdownload_403.txt");
    assert!(is_expired_download(403, &body));
    assert!(!is_expired_download(200, &body));
    assert!(!is_expired_download(403, "Forbidden"));
}

#[test]
fn the_retry_url_is_extracted_from_the_statdl_result() {
    let parsed = parse_statdownload_result(&read("bandcamp/statdownload_result.txt")).unwrap();
    assert_eq!(
        parsed.retry_url.as_deref(),
        Some("https://nightwave.bandcamp.com/download/album?enc=flac&fsig=abc123&id=2814")
    );
    assert_eq!(parsed.err_type.as_deref(), Some("ExpiredFreeDownloadError"));
}

#[test]
fn a_body_without_statdl_result_yields_none() {
    assert!(parse_statdownload_result("<html>Download Expired</html>").is_none());
}

#[test]
fn the_encoding_is_read_from_the_retry_url() {
    assert_eq!(
        retry_encoding(
            "https://nightwave.bandcamp.com/download/album?enc=mp3-320&fsig=abc123&id=2814"
        ),
        "mp3-320"
    );
    assert_eq!(
        retry_encoding("https://nightwave.bandcamp.com/download/album?fsig=abc"),
        "flac"
    );
}

#[test]
fn flac_is_lossless_and_mp3_320_carries_its_bitrate() {
    let flac = format_quality("flac").unwrap();
    assert!(flac.lossless);
    assert_eq!(flac.container, Container::Flac);
    assert_eq!(flac.bit_depth, None);
    assert_eq!(flac.sample_rate, None);
    assert_eq!(flac.bitrate_kbps, None);

    let mp3 = format_quality("mp3-320").unwrap();
    assert!(!mp3.lossless);
    assert_eq!(mp3.container, Container::Mp3);
    assert_eq!(mp3.bitrate_kbps, Some(320));
}

#[test]
fn every_free_download_format_has_a_fixed_container_and_bitrate() {
    let flac = format_quality("flac").unwrap();
    assert!(flac.lossless);
    assert_eq!(flac.container, Container::Flac);

    let alac = format_quality("alac").unwrap();
    assert!(alac.lossless);
    assert_eq!(alac.container, Container::Alac);
    assert_eq!(alac.bitrate_kbps, None);

    let wav = format_quality("wav").unwrap();
    assert!(wav.lossless);
    assert_eq!(wav.container, Container::Wav);

    let vorbis = format_quality("vorbis").unwrap();
    assert!(!vorbis.lossless);
    assert_eq!(vorbis.container, Container::Opus);
    assert_eq!(vorbis.bitrate_kbps, None);

    let v0 = format_quality("mp3-v0").unwrap();
    assert!(!v0.lossless);
    assert_eq!(v0.container, Container::Mp3);
    assert_eq!(v0.bitrate_kbps, None);

    let aac = format_quality("aac-hi").unwrap();
    assert!(!aac.lossless);
    assert_eq!(aac.container, Container::M4aAac);
    assert_eq!(aac.bitrate_kbps, None);

    let mp3_128 = format_quality("mp3-128").unwrap();
    assert!(!mp3_128.lossless);
    assert_eq!(mp3_128.container, Container::Mp3);
    assert_eq!(mp3_128.bitrate_kbps, Some(128));

    assert_eq!(FREE_DOWNLOAD_FORMATS[0], "flac");
    assert_eq!(FREE_DOWNLOAD_FORMATS.len(), 8);
}

#[test]
fn the_format_list_keeps_lossless_keys_ahead_of_lossy_ones() {
    let first_lossy = FREE_DOWNLOAD_FORMATS
        .iter()
        .position(|f| !format_quality(f).unwrap().lossless)
        .unwrap();
    assert!(
        FREE_DOWNLOAD_FORMATS[..first_lossy]
            .iter()
            .all(|f| format_quality(f).unwrap().lossless)
    );
    assert!(
        FREE_DOWNLOAD_FORMATS[first_lossy..]
            .iter()
            .all(|f| !format_quality(f).unwrap().lossless)
    );
}

#[test]
fn unknown_format_keys_are_rejected() {
    assert_eq!(format_quality("zip"), None);
    assert_eq!(format_quality("aiff"), None);
}
