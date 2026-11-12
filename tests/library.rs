use std::path::{Path, PathBuf};

use maxfetch::library::{absolute_path, relative_path, sanitize_component};
use maxfetch::model::{Release, SourceId};

fn release() -> Release {
    Release {
        title: "Get Lucky".into(),
        artist: "Daft Punk".into(),
        album: Some("Random Access Memories".into()),
        track_no: Some(8),
        duration_sec: Some(369.0),
        isrc: None,
        year: Some(2013),
        source: SourceId::Soulseek,
        source_item_id: "x".into(),
    }
}

#[test]
fn forbidden_characters_are_replaced() {
    let input = r#"a/b\c:d*e?f"g<h>i|j"#;
    let out = sanitize_component(input);
    assert!(!out.contains('/'));
    assert!(!out.contains('\\'));
    assert!(!out.contains(':'));
    assert!(!out.contains('*'));
    assert!(!out.contains('?'));
    assert!(!out.contains('"'));
    assert!(!out.contains('<'));
    assert!(!out.contains('>'));
    assert!(!out.contains('|'));
    assert_eq!(out, "a_b_c_d_e_f_g_h_i_j");
}

#[test]
fn leading_and_trailing_dots_and_spaces_are_trimmed() {
    assert_eq!(sanitize_component("  ..hello..  "), "hello");
    assert_eq!(sanitize_component("...  ."), "");
    assert_eq!(sanitize_component(".track name."), "track name");
}

#[test]
fn windows_reserved_names_get_a_suffix() {
    for name in ["CON", "con", "Prn", "AUX", "NUL", "COM1", "com9", "LPT1", "lpt9"] {
        assert_eq!(sanitize_component(name), format!("{name}_"));
    }
    assert_eq!(sanitize_component("COM0"), "COM0");
    assert_eq!(sanitize_component("CONSOLE"), "CONSOLE");
}

#[test]
fn whitespace_is_collapsed() {
    assert_eq!(sanitize_component("a   b\t\tc\n d"), "a b c d");
    assert_eq!(sanitize_component("   lead   and   trail   "), "lead and trail");
}

#[test]
fn relative_path_uses_artist_album_track_title() {
    let r = release();
    assert_eq!(
        relative_path(&r, "flac"),
        "Daft Punk/Random Access Memories/08 Get Lucky.flac"
    );
}

#[test]
fn missing_album_falls_back_to_singles() {
    let mut r = release();
    r.album = None;
    assert_eq!(relative_path(&r, "opus"), "Daft Punk/Singles/08 Get Lucky.opus");

    let mut r2 = release();
    r2.album = None;
    r2.track_no = None;
    let joined: PathBuf = absolute_path(Path::new("/music"), &r2, "mp3");
    assert_eq!(joined, Path::new("/music/Daft Punk/Singles/01 Get Lucky.mp3"));
}