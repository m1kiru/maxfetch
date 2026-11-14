use maxfetch::model::Container;
use maxfetch::sources::qobuz::{
    QobuzFormat, best_format_within, ceiling_from_limits, quality_of, scrape_app_credentials,
};

fn fixture(rel: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/qobuz/{}",
        env!("CARGO_MANIFEST_DIR"),
        rel
    ))
    .unwrap()
}

#[test]
fn app_credentials_are_scraped_from_the_bundle() {
    let (app_id, app_secret) = scrape_app_credentials(&fixture("bundle.js")).unwrap();
    assert_eq!(app_id, "950096962");
    assert_eq!(app_secret, "a1b2c3d4e5f60718293a4b5c6d7e8f90");
}

#[test]
fn a_bundle_without_the_pair_yields_nothing() {
    assert!(scrape_app_credentials("var x = 1;").is_none());
}

#[test]
fn format_ids_map_to_the_quality_ladder() {
    let mp3 = quality_of(QobuzFormat::from_format_id(5).unwrap());
    assert!(!mp3.lossless);
    assert_eq!(mp3.container, Container::Mp3);
    assert_eq!(mp3.bitrate_kbps, Some(320));
    assert_eq!(
        mp3.sample_rate, None,
        "the spec pins no sample rate for MP3-320"
    );

    let cd = quality_of(QobuzFormat::from_format_id(6).unwrap());
    assert!(cd.lossless);
    assert_eq!((cd.bit_depth, cd.sample_rate), (Some(16), Some(44100)));

    let hi = quality_of(QobuzFormat::from_format_id(7).unwrap());
    assert_eq!((hi.bit_depth, hi.sample_rate), (Some(24), Some(96000)));

    let ultra = quality_of(QobuzFormat::from_format_id(27).unwrap());
    assert_eq!(
        (ultra.bit_depth, ultra.sample_rate),
        (Some(24), Some(192000))
    );

    assert!(QobuzFormat::from_format_id(9).is_none());
}

#[test]
fn without_studio_the_ceiling_is_flac_cd_and_says_so() {
    let ceiling = ceiling_from_limits(QobuzFormat::FlacHiRes192, false);
    assert_eq!(
        (
            ceiling.max_lossless.bit_depth,
            ceiling.max_lossless.sample_rate
        ),
        (Some(16), Some(44100))
    );
    let reason = ceiling.reason.as_ref().unwrap();
    assert!(reason.contains("нет подписки Studio"), "{reason}");
    assert!(reason.contains("FLAC-CD 16/44.1"), "{reason}");
    assert_eq!(
        best_format_within(&ceiling, QobuzFormat::FlacHiRes192),
        QobuzFormat::FlacCd
    );
    assert_eq!(
        best_format_within(&ceiling, QobuzFormat::FlacCd),
        QobuzFormat::FlacCd
    );
}

#[test]
fn with_studio_the_requested_format_is_kept() {
    let ceiling = ceiling_from_limits(QobuzFormat::FlacHiRes96, true);
    assert!(ceiling.reason.is_none());
    assert_eq!(
        best_format_within(&ceiling, QobuzFormat::FlacHiRes96),
        QobuzFormat::FlacHiRes96
    );
    assert_eq!(
        best_format_within(&ceiling, QobuzFormat::Mp3_320),
        QobuzFormat::Mp3_320
    );
}

#[test]
fn a_flac_cd_track_on_a_non_studio_account_is_not_capped() {
    let ceiling = ceiling_from_limits(QobuzFormat::FlacCd, false);
    assert_eq!(
        (
            ceiling.max_lossless.bit_depth,
            ceiling.max_lossless.sample_rate
        ),
        (Some(16), Some(44100))
    );
    assert!(
        ceiling.reason.is_none(),
        "nothing was capped, so there is nothing to report"
    );
}

#[test]
fn a_24_192_track_on_a_non_studio_account_is_capped_to_flac_cd() {
    let ceiling = ceiling_from_limits(QobuzFormat::FlacHiRes192, false);
    assert_eq!(
        (
            ceiling.max_lossless.bit_depth,
            ceiling.max_lossless.sample_rate
        ),
        (Some(16), Some(44100))
    );
    assert_eq!(
        best_format_within(&ceiling, QobuzFormat::FlacHiRes192),
        QobuzFormat::FlacCd
    );
}

#[test]
fn the_ladder_is_an_involution() {
    for id in [5u32, 6, 7, 27] {
        assert_eq!(QobuzFormat::from_format_id(id).unwrap().format_id(), id);
    }
}
