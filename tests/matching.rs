use maxfetch::matching::{group_releases, match_key, norm};
use maxfetch::model::{Release, SourceId};

fn rel(source: SourceId, title: &str, artist: &str, album: Option<&str>, isrc: Option<&str>, dur: Option<f64>) -> Release {
    Release {
        title: title.into(),
        artist: artist.into(),
        album: album.map(|s| s.into()),
        track_no: None,
        duration_sec: dur,
        isrc: isrc.map(|s| s.into()),
        year: None,
        source,
        source_item_id: "id".into(),
    }
}

#[test]
fn norm_strips_case_punctuation_and_version_markers() {
    assert_eq!(norm("Get Lucky (feat. Pharrell Williams) [Official Audio]"), "get lucky feat pharrell williams");
    assert_eq!(norm("Around The World (2007 Remaster)"), "around the world");
    assert_eq!(norm("One More Time - 2011 Remaster"), "one more time");
    assert_eq!(norm("Daft Punk — Live At Home"), "daft punk live at home");
}

#[test]
fn match_key_includes_album_or_empty_slot() {
    let with = rel(SourceId::Spotify, "T", "A", Some("Alb"), None, None);
    let without = rel(SourceId::Soulseek, "T", "A", None, None, None);
    assert_eq!(match_key(&with), "a|t|alb");
    assert_eq!(match_key(&without), "a|t|");
}

#[test]
fn same_isrc_groups_across_sources() {
    let a = rel(SourceId::Spotify, "Get Lucky", "Daft Punk", Some("RAM"), Some("FR123400001"), Some(369.0));
    let b = rel(SourceId::Beatport, "Get Lucky", "Daft Punk", Some("RAM"), Some("FR123400001"), Some(369.0));
    let groups = group_releases(vec![a, b], 2.0);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].sources.len(), 2);
}

#[test]
fn different_isrc_never_groups_even_with_same_name() {
    let a = rel(SourceId::Spotify, "Get Lucky", "Daft Punk", Some("RAM"), Some("FR123400001"), Some(369.0));
    let b = rel(SourceId::Beatport, "Get Lucky", "Daft Punk", Some("RAM"), Some("FR999999999"), Some(369.0));
    assert_eq!(group_releases(vec![a, b], 2.0).len(), 2);
}

#[test]
fn same_name_without_isrc_groups() {
    let a = rel(SourceId::Bandcamp, "Get Lucky", "Daft Punk", Some("RAM"), None, Some(369.0));
    let b = rel(SourceId::Soulseek, "Get Lucky", "Daft Punk", Some("RAM"), None, Some(369.0));
    assert_eq!(group_releases(vec![a, b], 2.0).len(), 1);
}

#[test]
fn different_album_same_track_groups_by_duration() {
    let a = rel(SourceId::Spotify, "Get Lucky", "Daft Punk", Some("RAM"), None, Some(369.0));
    let b = rel(SourceId::Soulseek, "Get Lucky", "Daft Punk", Some("Greatest Hits"), None, Some(370.5));
    assert_eq!(group_releases(vec![a, b], 2.0).len(), 1);
}

#[test]
fn duration_outside_tolerance_does_not_group() {
    let a = rel(SourceId::Spotify, "Get Lucky", "Daft Punk", Some("RAM"), None, Some(369.0));
    let b = rel(SourceId::Soulseek, "Get Lucky", "Daft Punk", Some("Greatest Hits"), None, Some(400.0));
    assert_eq!(group_releases(vec![a, b], 2.0).len(), 2);
}

#[test]
fn primary_prefers_spotify_metadata() {
    let spotify = rel(SourceId::Spotify, "Get Lucky", "Daft Punk", Some("RAM"), Some("FR123400001"), Some(369.0));
    let soulseek = rel(SourceId::Soulseek, "get lucky ", "daft punk", None, None, Some(369.0));
    let groups = group_releases(vec![soulseek, spotify], 2.0);
    assert_eq!(groups[0].primary.source, SourceId::Spotify);
}
