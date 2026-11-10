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