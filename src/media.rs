//! Media container helpers built on the external `ffmpeg` binary.
//!
//! Audio is only ever moved between containers with `-c copy`: nothing in this
//! module decodes or re-encodes audio, and nothing here inspects the result.
//! Metadata that does not fit into Matroska tags is stored in a JSON sidecar
//! file next to the media file.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::Context;

/// Number of trailing `stderr` lines kept in a failure message.
const STDERR_TAIL_LINES: usize = 20;

/// Number of characters kept from the trailing `stderr` block.
const STDERR_TAIL_CHARS: usize = 2000;

/// Copies the audio stream of `input` into the container of `output` without
/// re-encoding it.
///
/// The container is chosen by `ffmpeg` from the `output` extension; video
/// streams are dropped with `-vn` and every stream is copied with `-c copy`.
/// The parent directory of `output` is expected to exist. An existing `output`
/// is overwritten, and a partially written file is left in place on failure.
pub fn remux_to_opus(ffmpeg_path: &str, input: &Path, output: &Path) -> anyhow::Result<()> {
    let result = Command::new(ffmpeg_path)
        .args(["-nostdin", "-y", "-i"])
        .arg(input)
        .args(["-vn", "-c", "copy"])
        .arg(output)
        .output()
        .with_context(|| format!("running ffmpeg {ffmpeg_path}"))?;

    if !result.status.success() {
        let exit = match result.status.code() {
            Some(code) => format!("code {code}"),
            None => "a signal".to_string(),
        };
        anyhow::bail!(
            "ffmpeg exited with {exit} for {input:?}: {}",
            stderr_tail(&result.stderr)
        );
    }

    Ok(())
}

/// Reports whether `ffmpeg_path` launches and answers `-version`.
///
/// A missing or non-executable binary yields `false` instead of an error.
pub fn ffmpeg_available(ffmpeg_path: &str) -> bool {
    Command::new(ffmpeg_path)
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Writes `payload` as pretty JSON to `<path>.json`.
///
/// The suffix is appended to the whole file name, so `clip.webm` gets
/// `clip.webm.json` and never collides with the sidecar of `clip.opus`. The
/// media file itself is neither read nor checked for existence.
pub fn write_sidecar(path: &Path, payload: &serde_json::Value) -> anyhow::Result<()> {
    let mut raw = path.as_os_str().to_os_string();
    raw.push(".json");
    let sidecar = PathBuf::from(raw);

    let mut body = serde_json::to_string_pretty(payload).context("serializing sidecar")?;
    body.push('\n');

    std::fs::write(&sidecar, body)
        .with_context(|| format!("writing sidecar {}", sidecar.display()))?;
    Ok(())
}

/// Keeps the last [`STDERR_TAIL_LINES`] lines of `stderr`, capped at
/// [`STDERR_TAIL_CHARS`] characters.
fn stderr_tail(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = text.trim().lines().collect();
    let start = lines.len().saturating_sub(STDERR_TAIL_LINES);
    let tail = lines[start..].join("\n");
    let tail = tail.trim();

    let count = tail.chars().count();
    if count <= STDERR_TAIL_CHARS {
        return tail.to_string();
    }

    let skip = count - STDERR_TAIL_CHARS;
    tail.chars()
        .skip(skip)
        .collect::<String>()
        .trim_start()
        .to_string()
}