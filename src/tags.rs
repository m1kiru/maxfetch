use anyhow::Result;
use lofty::config::WriteOptions;
use lofty::file::{FileType, TaggedFile, TaggedFileExt};
use lofty::tag::{ItemKey, ItemValue, Tag, TagExt, TagItem, TagType};
use std::path::Path;

use crate::model::{Container, Quality, SourceId};

/// Metadata attached to a downloaded audio file.
///
/// `quality_comment` is the raw `COMMENT` tag as read back from the file. On
/// write it is not copied verbatim: [`write_tags`] builds the comment itself
/// via [`lossy_comment`] (or removes it for lossless files).
#[derive(Clone, Debug, PartialEq)]
pub struct TagMeta {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub track_no: Option<u32>,
    pub year: Option<i32>,
    pub isrc: Option<String>,
    pub quality: Quality,
    pub source: SourceId,
    pub acquired_at: String,
    pub quality_comment: String,
}

/// The single source of truth for the honest lossy-origin marker.
pub fn lossy_comment(source: SourceId, container: Container, acquired_at: &str) -> String {
    format!(
        "maxfetch: source=lossy; container={}; source_id={}; acquired={}",
        container.as_str(),
        source.as_str(),
        acquired_at
    )
}

fn tag_type_for_container(container: Container) -> Option<TagType> {
    Some(match container {
        Container::Flac | Container::Opus => TagType::VorbisComments,
        Container::Mp3 => TagType::Id3v2,
        Container::M4aAac | Container::Alac => TagType::Mp4Ilst,
        Container::Wav => TagType::RiffInfo,
        Container::Aiff => TagType::AiffText,
        Container::Webm => return None,
    })
}

fn text_item(key: ItemKey, value: String) -> TagItem {
    TagItem::new(key, ItemValue::Text(value))
}

/// Writes title/artist/album/track number/year/ISRC into the file's tag, plus
/// the lossy-origin marker for lossy files (removed for lossless ones).
///
/// WebM/Matroska is not supported by lofty: the file is left untouched.
pub fn write_tags(path: &Path, meta: &TagMeta) -> Result<()> {
    let Some(tag_type) = tag_type_for_container(meta.quality.container) else {
        return Ok(());
    };

    // Parse the file first so we fail early on unreadable input.
    let _existing = lofty::read_from_path(path)?;
    let mut tag = Tag::new(tag_type);

    tag.insert(text_item(ItemKey::TrackTitle, meta.title.clone()));
    tag.insert(text_item(ItemKey::TrackArtist, meta.artist.clone()));
    if let Some(album) = &meta.album {
        tag.insert(text_item(ItemKey::AlbumTitle, album.clone()));
    }
    if let Some(track_no) = meta.track_no {
        tag.insert(text_item(ItemKey::TrackNumber, track_no.to_string()));
    }
    if let Some(year) = meta.year {
        tag.insert(text_item(ItemKey::Year, year.to_string()));
    }
    if let Some(isrc) = &meta.isrc {
        tag.insert(text_item(ItemKey::Isrc, isrc.clone()));
    }

    if meta.quality.lossless {
        tag.remove_key(ItemKey::Comment);
    } else {
        let comment = lossy_comment(meta.source, meta.quality.container, &meta.acquired_at);
        tag.insert(text_item(ItemKey::Comment, comment));
    }

    tag.save_to_path(path, WriteOptions::new())?;
    Ok(())
}

fn guessed_quality(file_type: FileType) -> Quality {
    let (lossless, container) = match file_type {
        FileType::Flac => (true, Container::Flac),
        FileType::Wav => (true, Container::Wav),
        FileType::Aiff => (true, Container::Aiff),
        FileType::Mp4 => (true, Container::Alac),
        FileType::Opus => (false, Container::Opus),
        FileType::Vorbis => (false, Container::Opus),
        _ => (false, Container::Mp3),
    };
    Quality {
        lossless,
        container,
        bit_depth: None,
        sample_rate: None,
        bitrate_kbps: None,
    }
}

fn webm_placeholder() -> TagMeta {
    TagMeta {
        title: String::new(),
        artist: String::new(),
        album: None,
        track_no: None,
        year: None,
        isrc: None,
        quality: Quality {
            lossless: false,
            container: Container::Webm,
            bit_depth: None,
            sample_rate: None,
            bitrate_kbps: None,
        },
        source: SourceId::Soulseek,
        acquired_at: String::new(),
        quality_comment: String::new(),
    }
}

/// Reads tags back into [`TagMeta`]. The container is inferred from the actual
/// file contents, not passed in.
///
/// Only the text fields survive the round trip; the full [`Quality`] and
/// [`SourceId`] come from `probe` and the sidecar instead.
pub fn read_tags(path: &Path) -> Result<TagMeta> {
    let is_webm = path
        .extension()
        .and_then(|ext| ext.to_str())
        .and_then(Container::from_ext)
        == Some(Container::Webm);
    if is_webm {
        return Ok(webm_placeholder());
    }

    let file = lofty::read_from_path(path)?;
    let file_type = file.file_type();
    let tag_type = match file_type {
        FileType::Flac | FileType::Vorbis | FileType::Opus => TagType::VorbisComments,
        FileType::Mpeg => TagType::Id3v2,
        FileType::Mp4 => TagType::Mp4Ilst,
        FileType::Wav => TagType::RiffInfo,
        FileType::Aiff => TagType::AiffText,
        _ => file.primary_tag_type(),
    };
    let tag = tag_type_opt(&file, tag_type);

    let string_of = |key: ItemKey| -> String {
        tag.and_then(|t| t.get_string(key)).unwrap_or("").to_string()
    };
    let option_of = |key: ItemKey| -> Option<String> {
        tag.and_then(|t| t.get_string(key)).map(str::to_string)
    };

    let track_no = tag
        .and_then(|t| t.get_string(ItemKey::TrackNumber))
        .and_then(|s| s.trim().parse::<u32>().ok());
    let year = tag
        .and_then(|t| t.get_string(ItemKey::Year))
        .and_then(|s| s.trim().parse::<i32>().ok());

    Ok(TagMeta {
        title: string_of(ItemKey::TrackTitle),
        artist: string_of(ItemKey::TrackArtist),
        album: option_of(ItemKey::AlbumTitle),
        track_no,
        year,
        isrc: option_of(ItemKey::Isrc),
        quality: guessed_quality(file_type),
        source: SourceId::Soulseek,
        acquired_at: String::new(),
        quality_comment: string_of(ItemKey::Comment),
    })
}

fn tag_type_opt(file: &TaggedFile, tag_type: TagType) -> Option<&Tag> {
    file.tag(tag_type).or_else(|| file.primary_tag())
}