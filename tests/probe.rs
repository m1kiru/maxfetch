use maxfetch::model::Container;
use maxfetch::probe::probe;
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/probe")
        .join(name)
}

#[test]
fn probe_reads_sample_rate_and_depth_of_flac() {
    let r = probe(&fixture("sine_44100_s16.flac")).unwrap();
    assert_eq!(r.container, Container::Flac);
    assert_eq!(r.sample_rate, Some(44100));
    assert_eq!(r.bit_depth, Some(16));
    assert_eq!(r.channels, Some(2));
    assert!(r.duration_sec.unwrap() > 0.9 && r.duration_sec.unwrap() < 1.1);
}

#[test]
fn probe_reports_actual_depth_not_claimed_depth() {
    let r = probe(&fixture("sine_48000_s24.flac")).unwrap();
    assert_eq!(r.sample_rate, Some(48000));
    assert_eq!(r.bit_depth, Some(24));
}

#[test]
fn probe_fails_on_a_missing_file() {
    assert!(probe(&fixture("nope.flac")).is_err());
}