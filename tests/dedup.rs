use std::path::Path;

use maxfetch::config::Config;
use maxfetch::db::{Db, TrackRecord};
use maxfetch::dedup::{DedupOutcome, apply_deletion, plan_deletion, register, scan_library};
use maxfetch::model::{Candidate, Container, Locator, Quality, Release, SourceId};

const FIXTURE: &str = "tests/fixtures/probe/sine_44100_s16.flac";

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

/// A stored row, for tests that need to build database state directly.
fn row(path: &str, isrc: Option<&str>, depth: u8) -> TrackRecord {
    TrackRecord {
        id: 0,
        isrc: isrc.map(str::to_string),
        match_key: "daft punk|chain|ram".into(),
        title: "Chain".into(),
        artist: "Daft Punk".into(),
        album: Some("RAM".into()),
        track_no: Some(8),
        duration_sec: Some(369.0),
        lossless: true,
        container: Container::Flac,
        bit_depth: Some(depth),
        sample_rate: Some(44100),
        bitrate_kbps: None,
        size_bytes: None,
        path: path.into(),
        source: SourceId::Soulseek,
        acquired_at: "2026-11-10T00:00:00Z".into(),
        duplicate_of: None,
    }
}

fn fixture() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE)
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

    // The claimed quality must be better too, otherwise `compare_quality` returns
    // `Equal` and this test would silently exercise "the newer file wins".
    let mut better = candidate(Some("FR1"), "Get Lucky", "/m/high.flac");
    better.quality = quality(24);
    let outcome = register(
        &config,
        &db,
        &better,
        Path::new("/m/high.flac"),
        quality(24),
        Some(369.0),
    )
    .unwrap();
    match outcome {
        DedupOutcome::Duplicate {
            winner_id,
            loser_id,
        } => assert!(
            winner_id > loser_id,
            "the better file, registered second, must win: {outcome:?}"
        ),
        other => panic!("the better file must collide with the worse one: {other:?}"),
    }

    // Registering a worse file last must not steal the crown: quality decides,
    // not recency.
    register(
        &config,
        &db,
        &candidate(Some("FR1"), "Get Lucky", "/m/worse.flac"),
        Path::new("/m/worse.flac"),
        quality(16),
        Some(369.0),
    )
    .unwrap();

    let groups = db.duplicate_groups().unwrap();
    assert_eq!(groups.len(), 1);
    assert!(
        groups[0].winner.path.ends_with("high.flac"),
        "the 24 bit file must stay canonical, got {}",
        groups[0].winner.path
    );
    assert_eq!(groups[0].losers.len(), 2);
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
    let mut better = candidate(Some("FR2"), "Other", d.to_str().unwrap());
    better.quality = quality(24);
    register(&config, &db, &better, &d, quality(24), Some(369.0)).unwrap();

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
            quality(16),
            Some(369.0),
        )
        .unwrap();

        // No ISRC and a different album: only the artist+title+duration fallback
        // can connect the two files, and the allowance for it comes from Config.
        // The new file is genuinely better, so the fallback must also hand the
        // crown to it by quality instead of by recency.
        let mut second = candidate(None, "Get Lucky", "/m/b.flac");
        second.release.album = Some("Greatest Hits".into());
        second.quality = quality(24);
        register(
            &config,
            &db,
            &second,
            Path::new("/m/b.flac"),
            quality(24),
            Some(374.0),
        )
        .unwrap();

        let groups = db.duplicate_groups().unwrap();
        assert_eq!(groups.len(), expected, "with a {tolerance} s tolerance");
        if let Some(group) = groups.first() {
            assert!(
                group.winner.path.ends_with("b.flac"),
                "the 24 bit file must win through the fallback, got {}",
                group.winner.path
            );
        }
    }
}

#[test]
fn scan_library_indexes_unknown_files_and_never_touches_them() {
    let dir = tempfile::tempdir().unwrap();
    let nested = dir.path().join("Artist").join("Album");
    std::fs::create_dir_all(&nested).unwrap();
    let track = nested.join("Track.flac");
    std::fs::copy(fixture(), &track).unwrap();

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

#[test]
fn canonicalization_follows_the_whole_duplicate_chain() {
    let db = Db::open_in_memory().unwrap();
    let config = config(2);

    // A <- B <- C. Only the non-canonical `a` matches the ISRC of the new file,
    // so a single-step canonicalization would target the loser `b` and orphan
    // the real winner `c`, leaving two canonical rows behind.
    let a = db
        .insert_track(&row("/m/chain_a.flac", Some("FR3"), 24))
        .unwrap();
    let b = db
        .insert_track(&row("/m/chain_b.flac", Some("FR9"), 24))
        .unwrap();
    let c = db
        .insert_track(&row("/m/chain_c.flac", Some("FR9"), 16))
        .unwrap();
    db.set_duplicate_of(a, b).unwrap();
    db.set_duplicate_of(b, c).unwrap();

    let mut better = candidate(Some("FR3"), "Get Lucky", "/m/new.flac");
    better.quality = quality(24);
    let outcome = register(
        &config,
        &db,
        &better,
        Path::new("/m/new.flac"),
        quality(24),
        Some(369.0),
    )
    .unwrap();

    let canonicals: Vec<TrackRecord> = db
        .all_tracks()
        .unwrap()
        .into_iter()
        .filter(|track| track.duplicate_of.is_none())
        .collect();
    assert_eq!(
        canonicals.len(),
        1,
        "the chain must leave exactly one canonical row, got {canonicals:?}"
    );
    assert!(canonicals[0].path.ends_with("new.flac"));
    match outcome {
        DedupOutcome::Duplicate {
            winner_id,
            loser_id,
        } => {
            assert_eq!(winner_id, canonicals[0].id);
            // `c` was the real winner of the chain and must be the one dethroned.
            assert_eq!(loser_id, c);
        }
        other => panic!("the chain must collide with the new file: {other:?}"),
    }
}

#[test]
fn a_duplicate_cycle_does_not_hang_registration() {
    let db = Db::open_in_memory().unwrap();
    let config = config(2);
    let x = db.insert_track(&row("/m/x.flac", Some("FR7"), 16)).unwrap();
    let y = db.insert_track(&row("/m/y.flac", Some("FR8"), 16)).unwrap();
    db.set_duplicate_of(x, y).unwrap();
    db.set_duplicate_of(y, x).unwrap();

    let mut better = candidate(Some("FR7"), "Get Lucky", "/m/z.flac");
    better.quality = quality(24);
    let outcome = register(
        &config,
        &db,
        &better,
        Path::new("/m/z.flac"),
        quality(24),
        Some(369.0),
    )
    .unwrap();

    assert!(
        matches!(outcome, DedupOutcome::Inserted { .. }),
        "a corrupt cycle yields no canonical target, so the file is inserted: {outcome:?}"
    );
}

#[test]
#[cfg(unix)]
fn scan_library_keeps_going_past_unreadable_directories_and_broken_files() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let good = dir.path().join("Good").join("Track.flac");
    std::fs::create_dir_all(good.parent().unwrap()).unwrap();
    std::fs::copy(fixture(), &good).unwrap();

    let broken = dir.path().join("Broken").join("broken.flac");
    std::fs::create_dir_all(broken.parent().unwrap()).unwrap();
    std::fs::write(&broken, b"definitely not a flac stream").unwrap();

    let locked = dir.path().join("Locked");
    std::fs::create_dir_all(&locked).unwrap();
    std::fs::write(locked.join("Hidden.flac"), b"x").unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();

    let db = Db::open_in_memory().unwrap();
    let inserted = scan_library(&db, dir.path());

    // Restore the mode first: the temporary directory must stay removable.
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(
        inserted.unwrap(),
        1,
        "a broken file and an unreadable subdirectory must not abort the scan"
    );
    let tracks = db.all_tracks().unwrap();
    assert_eq!(tracks.len(), 1);
    assert!(tracks[0].path.ends_with("Track.flac"));
}

#[test]
#[cfg(unix)]
fn scan_library_reports_an_unreadable_root() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o000)).unwrap();

    let db = Db::open_in_memory().unwrap();
    let result = scan_library(&db, dir.path());

    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();

    let err = result.unwrap_err();
    assert!(
        format!("{err:#}").contains("scanning library root"),
        "an unreachable root is fatal: {err:#}"
    );
}
