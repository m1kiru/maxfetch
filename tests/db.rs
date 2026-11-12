use maxfetch::db::{Db, TrackRecord};
use maxfetch::model::{Container, Release, SourceId};

fn record(path: &str, isrc: Option<&str>, key: &str) -> TrackRecord {
    TrackRecord {
        id: 0,
        isrc: isrc.map(|s| s.into()),
        match_key: key.into(),
        title: "Get Lucky".into(),
        artist: "Daft Punk".into(),
        album: Some("RAM".into()),
        track_no: Some(8),
        duration_sec: Some(369.0),
        lossless: true,
        container: Container::Flac,
        bit_depth: Some(16),
        sample_rate: Some(44100),
        bitrate_kbps: None,
        size_bytes: Some(1234),
        path: path.into(),
        source: SourceId::Soulseek,
        acquired_at: "2026-11-10T00:00:00Z".into(),
        duplicate_of: None,
    }
}

#[test]
fn insert_and_read_back() {
    let db = Db::open_in_memory().unwrap();
    let id = db.insert_track(&record("/m/a.flac", Some("FR1"), "a|t|ram")).unwrap();
    assert!(id > 0);
    let found = db.find_by_isrc("FR1").unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].path, "/m/a.flac");
}

#[test]
fn duplicate_path_is_rejected() {
    let db = Db::open_in_memory().unwrap();
    db.insert_track(&record("/m/a.flac", None, "k")).unwrap();
    let err = db.insert_track(&record("/m/a.flac", None, "k")).unwrap_err();
    assert!(format!("{err}").to_lowercase().contains("unique"));
}

#[test]
fn duplicate_group_reports_winner_and_loser() {
    let db = Db::open_in_memory().unwrap();
    let a = db.insert_track(&record("/m/a.flac", Some("FR1"), "k")).unwrap();
    let b = db.insert_track(&record("/m/b.flac", Some("FR1"), "k")).unwrap();
    db.set_duplicate_of(b, a).unwrap();
    let groups = db.duplicate_groups().unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].winner.id, a);
    assert_eq!(groups[0].losers.len(), 1);
    assert_eq!(groups[0].losers[0].id, b);
}

#[test]
fn delete_track_removes_only_the_row() {
    let db = Db::open_in_memory().unwrap();
    let a = db.insert_track(&record("/m/a.flac", Some("FR1"), "k")).unwrap();
    db.delete_track(a).unwrap();
    assert!(db.all_tracks().unwrap().is_empty());
}

fn lossy24(path: &str) -> TrackRecord {
    let mut r = record(path, Some("FR1"), "k");
    r.bit_depth = Some(24);
    r.sample_rate = Some(96000);
    r
}

#[test]
fn conflict_is_true_when_any_loser_matches_winner_quality() {
    let db = Db::open_in_memory().unwrap();
    let winner = db.insert_track(&lossy24("/m/win.flac")).unwrap();
    // Проигравший 1: хуже по качеству (16-bit).
    let worse = db.insert_track(&record("/m/16.flac", Some("FR1"), "k")).unwrap();
    // Проигравший 2: неотличим от победителя (24-bit).
    let equal = db.insert_track(&lossy24("/m/24.flac")).unwrap();
    db.set_duplicate_of(worse, winner).unwrap();
    db.set_duplicate_of(equal, winner).unwrap();

    let groups = db.duplicate_groups().unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].losers.len(), 2);
    assert!(
        groups[0].conflict,
        "один неотличимый проигравший должен выставлять conflict=true"
    );
}

#[test]
fn conflict_is_false_when_all_losers_are_worse() {
    let db = Db::open_in_memory().unwrap();
    let winner = db.insert_track(&lossy24("/m/win.flac")).unwrap();
    let worse = db.insert_track(&record("/m/16.flac", Some("FR1"), "k")).unwrap();
    db.set_duplicate_of(worse, winner).unwrap();

    let groups = db.duplicate_groups().unwrap();
    assert_eq!(groups.len(), 1);
    assert!(!groups[0].conflict);
}

#[test]
fn indistinguishable_losers_lists_only_matching_quality() {
    let db = Db::open_in_memory().unwrap();
    let winner = db.insert_track(&lossy24("/m/win.flac")).unwrap();
    let worse = db.insert_track(&record("/m/16.flac", Some("FR1"), "k")).unwrap();
    let equal = db.insert_track(&lossy24("/m/24.flac")).unwrap();
    db.set_duplicate_of(worse, winner).unwrap();
    db.set_duplicate_of(equal, winner).unwrap();

    let groups = db.duplicate_groups().unwrap();
    let matched: Vec<i64> = groups[0].indistinguishable_losers().iter().map(|t| t.id).collect();
    assert_eq!(matched, vec![equal], "только совпавший по качеству файл");
}

#[test]
fn delete_track_with_download_row_nulls_the_reference() {
    let db = Db::open_in_memory().unwrap();
    let a = db.insert_track(&record("/m/a.flac", Some("FR1"), "k")).unwrap();
    db.log_download(SourceId::Soulseek, Some(a), "completed", None).unwrap();

    db.delete_track(a).unwrap();

    assert!(db.all_tracks().unwrap().is_empty());
    let track_id: Option<i64> = db
        .conn()
        .query_row("SELECT track_id FROM downloads", [], |row| row.get(0))
        .unwrap();
    assert_eq!(track_id, None, "downloads.track_id должен стать NULL");
}

#[test]
fn delete_track_clears_duplicate_of_on_losers() {
    let db = Db::open_in_memory().unwrap();
    let winner = db.insert_track(&lossy24("/m/win.flac")).unwrap();
    let loser = db.insert_track(&record("/m/16.flac", Some("FR1"), "k")).unwrap();
    db.set_duplicate_of(loser, winner).unwrap();

    db.delete_track(winner).unwrap();

    let remaining = db.all_tracks().unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].id, loser);
    assert_eq!(remaining[0].duplicate_of, None, "проигравший становится каноническим");
}

#[test]
fn transaction_is_atomic_when_rolled_back() {
    let db = Db::open_in_memory().unwrap();
    {
        let tx = db.transaction().unwrap();
        for path in ["/m/tx1.mp3", "/m/tx2.mp3"] {
            tx.execute(
                &format!(
                    "INSERT INTO tracks (match_key, title, artist, lossless, container, path, source_id, acquired_at) \
                     VALUES ('k', 't', 'a', 0, 'mp3', '{path}', 'soulseek', '2026-11-10T00:00:00Z')"
                ),
                [],
            )
            .unwrap();
        }
        // Drop без commit = откат.
    }
    assert!(db.all_tracks().unwrap().is_empty(), "откат должен убрать оба трека");
}

#[test]
fn release_cache_round_trips_and_expires() {
    let db = Db::open_in_memory().unwrap();
    let release = Release {
        title: "Get Lucky".into(),
        artist: "Daft Punk".into(),
        album: Some("RAM".into()),
        track_no: Some(8),
        duration_sec: Some(369.0),
        isrc: Some("FR1".into()),
        year: Some(2013),
        source: SourceId::Soulseek,
        source_item_id: "r1".into(),
    };
    let key = maxfetch::matching::match_key(&release);
    assert!(db.get_release(&key).unwrap().is_none());
    assert!(!db.release_is_fresh(&key).unwrap());

    db.upsert_release(&release).unwrap();
    let back = db.get_release(&key).unwrap().expect("cached release");
    assert_eq!(back.title, "Get Lucky");
    assert_eq!(back.source, SourceId::Soulseek);
    assert!(db.release_is_fresh(&key).unwrap());

    db.upsert_release(&release).unwrap();
    assert_eq!(db.get_release(&key).unwrap().unwrap().source_item_id, "r1");
}