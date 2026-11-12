use anyhow::{Result, anyhow};
use std::fs::File;
use std::path::Path;
use symphonia::core::codecs::CodecParameters;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::well_known::{
    FORMAT_ID_AIFF, FORMAT_ID_FLAC, FORMAT_ID_ISOMP4, FORMAT_ID_MKV, FORMAT_ID_MP1, FORMAT_ID_MP2,
    FORMAT_ID_MP3, FORMAT_ID_OGG, FORMAT_ID_WAVE,
};
use symphonia::core::formats::{FormatId, FormatOptions, FormatReader, Track, TrackType};
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;

use crate::model::Container;

/// What a downloaded file actually contains, read from its container header.
#[derive(Clone, Debug, PartialEq)]
pub struct ProbeResult {
    pub container: Container,
    pub bit_depth: Option<u8>,
    pub sample_rate: Option<u32>,
    pub bitrate_kbps: Option<u32>,
    pub duration_sec: Option<f64>,
    pub channels: Option<u16>,
}

/// Reads the container header of `path` and reports the real quality of the audio it holds.
///
/// Only the header is read: samples are never decoded. Claimed quality of the source that
/// produced the file is irrelevant here, so the result reflects the file as it is on disk.
pub fn probe(path: &Path) -> Result<ProbeResult> {
    let mut hint = Hint::new();
    hint.with_extension(path.extension().and_then(|e| e.to_str()).unwrap_or(""));

    let mss = MediaSourceStream::new(
        Box::new(File::open(path)?),
        MediaSourceStreamOptions::default(),
    );

    let format = symphonia::default::get_probe().probe(
        &hint,
        mss,
        FormatOptions::default(),
        MetadataOptions::default(),
    )?;

    let container = container_of(format.format_info().format)?;
    let audio = audio_track(format.as_ref())
        .ok_or_else(|| anyhow!("no audio track in {}", path.display()))?;
    let codec_params = audio.codec_params.as_ref().and_then(CodecParameters::audio);

    let sample_rate = codec_params.and_then(|p| p.sample_rate);
    let bit_depth = codec_params
        .and_then(|p| p.bits_per_sample.or(p.bits_per_coded_sample))
        .and_then(|bits| u8::try_from(bits).ok());
    let channels = codec_params
        .and_then(|p| p.channels.as_ref())
        .map(|c| c.count() as u16);

    let duration_sec = duration_sec(format.as_ref(), audio);
    let bitrate_kbps = match duration_sec {
        Some(secs) if secs > 0.0 => {
            let size_bytes = std::fs::metadata(path)?.len() as f64;
            Some((size_bytes * 8.0 / secs / 1000.0) as u32)
        }
        _ => None,
    };

    Ok(ProbeResult {
        container,
        bit_depth,
        sample_rate,
        bitrate_kbps,
        duration_sec,
        channels,
    })
}

/// Picks the default audio track, falling back to the first audio track available.
fn audio_track(format: &dyn FormatReader) -> Option<&Track> {
    format.default_track(TrackType::Audio).or_else(|| {
        format
            .tracks()
            .iter()
            .find(|t| t.codec_params.as_ref().and_then(CodecParameters::audio).is_some())
    })
}

/// Maps the probed container format to a [`Container`]. The probed format wins over the file
/// extension, so a mislabelled file is reported as what it really is. Formats whose quality
/// cannot be honestly described as one of our containers are rejected instead of guessed.
fn container_of(format: FormatId) -> Result<Container> {
    let container = match format {
        FORMAT_ID_FLAC => Container::Flac,
        FORMAT_ID_WAVE => Container::Wav,
        FORMAT_ID_AIFF => Container::Aiff,
        FORMAT_ID_MP1 | FORMAT_ID_MP2 | FORMAT_ID_MP3 => Container::Mp3,
        FORMAT_ID_OGG => Container::Opus,
        FORMAT_ID_ISOMP4 => Container::M4aAac,
        FORMAT_ID_MKV => Container::Webm,
        other => anyhow::bail!("unsupported container format: {other}"),
    };
    Ok(container)
}

/// Converts the track duration in time base ticks to seconds. One tick is `numer/denom` seconds,
/// which is how `TimeBase::calc_duration` converts as well.
fn duration_sec(format: &dyn FormatReader, track: &Track) -> Option<f64> {
    let media_info = format.media_info();
    let ticks = track.duration.map(|d| d.get()).or(media_info.duration.map(|d| d.get()))?;
    let time_base = track.time_base.or(media_info.time_base)?;
    Some(ticks as f64 * time_base.numer.get() as f64 / time_base.denom.get() as f64)
}
