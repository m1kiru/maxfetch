use std::path::Path;

use maxfetch::config::Config;
use maxfetch::db::Db;
use maxfetch::dedup::{DedupOutcome, apply_deletion, plan_deletion, register, scan_library};
use maxfetch::model::{Candidate, Container, Locator, Quality, Release, SourceId};

fn quality(depth: u8) -> Quality {
    Quality {
        lossless: true,
        container: Container::Flac,
        bit_depth: Some(depth),
        sample_rate: Some(44100),
        bitrate_kbps: None,
    }
}

fn candidate(isrc: Option<&str>, title: &str, path: &str) -> Candidate {
    Candidate {
        source: SourceId::Soulseek,
        release: Release {
            title: title.into(),
            artist: "Daft Punk".into(),
            album: Some("RAM".into()),
            track_no: Some(8),
            duration_sec: Some(369.0),
            isrc: isrc.map(str::to_string),
            year: None,
            source: SourceId::Soulseek,
            source_item_id: "r".into(),
        },
        quality: quality(16),
        size_bytes: None,
        locator: Locator::Soulseek {
            username: "p".into(),
            filename: path.into(),
            size: 0,
        },
    }
}

fn config(tolerance: u32) -> Config {
    let mut config = Config::default();
    config.matching.duration_tolerance_sec = tolerance;
    config
}

#[test]
fn same_isrc_marks_the_new_file_as_duplicate() {
    let db = Db::open_in_memory().unwrap();
    let config = config(2);

    let first = register(
        &config,
        &db,
        &candidate(Some("FR1"), "Get Lucky", "/m/a.flac"),
        Path::new("/m/a.flac"),
        quality(24),
        Some(369.0),
    )
    .unwrap();
    assert!(
        matches!(first, DedupOutcome::Inserted { .. }),
        "the first file has nothing to collide with: {first:?}"
    );

    // Same ISRC, different album: dedup must not depend on `Release::album`
    // (a compilation and the studio album hold the same recording).
    let mut other_album = candidate(Some("FR1"), "Get Lucky", "/m/b.flac");
    other_album.release.album = Some("Now 50".into());
    let second = register(
        &config,
        &db,
        &other_album,
        Path::new("/m/b.flac"),
        quality(16),
        Some(369.0),
    )
    .unwrap();
    assert!(
        matches!(second, DedupOutcome::Duplicate { .. }),
        "an ISRC match must be reported as a duplicate: {second:?}"
    );
    assert_eq!(db.duplicate_groups().unwrap().len(), 1);
}

#[test]
fn the_better_quality_wins() {
    let db = Db::open_in_memory().unwrap();
    let config = config(2);

    register(
        &config,
        &db,
        &candidate(Some("FR1"), "Get Lucky", "/m/low.flac"),
        Path::new("/m/low.flac"),
        quality(16),
        Some(369.0),
    )
    .unwrap();
    register(
        &config,
        &db,
        &candidate(Some("FR1"), "Get Lucky", "/m/high.flac"),
        Path::new("/m/high.flac"),
        quality(24),
        Some(369.0),
    )
    .unwrap();

    let groups = db.duplicate_groups().unwrap();
    assert_eq!(groups.len(), 1);
    assert!(
        groups[0].winner.path.ends_with("high.flac"),
        "the 24 bit file must become canonical, got {}",
        groups[0].winner.path
    );
}

#[test]
fn dedup_never_deletes_files_by_itself() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.flac");
    let b = dir.path().join("b.flac");
    std::fs::write(&a, b"a").unwrap();
    std::fs::write(&b, b"b").unwrap();

    let db = Db::open_in_memory().unwrap();
    let config = config(2);
    register(
        &config,
        &db,
        &candidate(Some("FR1"), "Get Lucky", a.to_str().unwrap()),
        &a,
        quality(24),
        Some(369.0),
    )
    .unwrap();
    register(
        &config,
        &db,
        &candidate(Some("FR1"), "Get Lucky", b.to_str().unwrap()),
        &b,
        quality(16),
        Some(369.0),
    )
    .unwrap();

    assert!(a.exists(), "register must never delete the winner");
    assert!(b.exists(), "register must never delete the loser");
}

#[test]
fn deletion_requires_an_explicit_plan_and_leaves_conflicts_alone() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.flac");
    let b = dir.path().join("b.flac");
    std::fs::write(&a, b"a").unwrap();
    std::fs::write(&b, b"b").unwrap();

    let db = Db::open_in_memory().unwrap();
    let config = config(2);
    register(
        &config,
        &db,
        &candidate(Some("FR1"), "Get Lucky", a.to_str().unwrap()),
        &a,
        quality(16),
        Some(369.0),
    )
    .unwrap();
    register(
        &config,
        &db,
        &candidate(Some("FR1"), "Get Lucky", b.to_str().unwrap()),
        &b,
        quality(16),
        Some(369.0),
    )
    .unwrap();

    assert!(
        plan_deletion(&db).unwrap().is_empty(),
        "identical quality must be treated as a conflict"
    );

    let c = dir.path().join("c.flac");
    let d = dir.path().join("d.flac");
    std::fs::write(&c, b"c").unwrap();
    register(
        &config,
        &db,
        &candidate(Some("FR2"), "Other", c.to_str().unwrap()),
        &c,
        quality(16),
        Some(369.0),
    )
    .unwrap();
    // The winner's file deliberately does not exist: a missing winner file must
    // not stand in the way of deleting the clearly worse copy.
    register(
        &config,
        &db,
        &candidate(Some("FR2"), "Other", d.to_str().unwrap()),
        &d,
        quality(24),
        Some(369.0),
    )
    .unwrap();

    let plans = plan_deletion(&db).unwrap();
    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0].path, c.to_str().unwrap());
    assert_eq!(plans[0].winner_path, d.to_str().unwrap());
    assert!(c.exists(), "planning must not delete anything");

    assert_eq!(apply_deletion(&db, &plans).unwrap(), 1);
    assert!(!c.exists(), "the planned file is gone");
    assert!(plan_deletion(&db).unwrap().is_empty());
}

#[test]
fn the_duration_tolerance_comes_from_the_config() {
    for (tolerance, expected) in [(2u32, 0usize), (10, 1)] {
        let db = Db::open_in_memory().unwrap();
        let config = config(tolerance);

        register(
            &config,
            &db,
            &candidate(None, "Get Lucky", "/m/a.flac"),
            Path::new("/m/a.flac"),
            quality(24),
            Some(369.0),
        )
        .unwrap();

        // No ISRC and a different album: only the artist+title+duration fallback
        // can connect the two files, and the allowance for it comes from Config.
        let mut second = candidate(None, "Get Lucky", "/m/b.flac");
        second.release.album = Some("Greatest Hits".into());
        register(
            &config,
            &db,
            &second,
            Path::new("/m/b.flac"),
            quality(16),
            Some(374.0),
        )
        .unwrap();

        assert_eq!(
            db.duplicate_groups().unwrap().len(),
            expected,
            "with a {tolerance} s tolerance"
        );
    }
}

#[test]
fn scan_library_indexes_unknown_files_and_never_touches_them() {
    let dir = tempfile::tempdir().unwrap();
    let nested = dir.path().join("Artist").join("Album");
    std::fs::create_dir_all(&nested).unwrap();
    let track = nested.join("Track.flac");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("probe")
        .join("sine_44100_s16.flac");
    std::fs::copy(&fixture, &track).unwrap();

    let db = Db::open_in_memory().unwrap();
    assert_eq!(scan_library(&db, dir.path()).unwrap(), 1);
    assert!(track.exists(), "scanning must never touch the files");

    let tracks = db.all_tracks().unwrap();
    assert_eq!(tracks.len(), 1);
    assert!(tracks[0].path.ends_with("Track.flac"));
    assert_eq!(tracks[0].bit_depth, Some(16));

    assert_eq!(scan_library(&db, dir.path()).unwrap(), 0);
    assert_eq!(db.all_tracks().unwrap().len(), 1);
}
