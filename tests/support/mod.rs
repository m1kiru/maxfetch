#![allow(dead_code)]

//! Shared test support: a local fixture replay server plus fixture/ZIP helpers.
//!
//! [`MockServer`] serves recorded bodies from memory over a real loopback
//! socket, so tests exercise the same HTTP client code that production uses
//! without touching the network.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// A canned HTTP response handed out by [`MockServer`].
pub struct Reply {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

impl Reply {
    /// A `200 OK` response with the given content type and body.
    pub fn ok(content_type: &'static str, body: impl Into<Vec<u8>>) -> Reply {
        Reply {
            status: 200,
            content_type,
            body: body.into(),
        }
    }

    /// A response with the given status; the body is served as `text/plain`.
    pub fn status(status: u16, body: impl Into<Vec<u8>>) -> Reply {
        Reply {
            status,
            content_type: "text/plain",
            body: body.into(),
        }
    }
}

/// A path with the ordered replies it serves.
pub struct Route {
    pub path: String,
    pub replies: Vec<Reply>,
}

impl Route {
    pub fn new(path: impl Into<String>, replies: Vec<Reply>) -> Route {
        Route {
            path: path.into(),
            replies,
        }
    }
}

/// A loopback HTTP server that replays recorded fixtures.
///
/// Requests are handled one at a time, so request counters are deterministic.
/// The server lives until the test process exits; `_handle` is kept only to
/// document that ownership.
pub struct MockServer {
    base: String,
    hits: Arc<Mutex<HashMap<String, usize>>>,
    cursor: Arc<Mutex<HashMap<String, usize>>>,
    _handle: tokio::task::JoinHandle<()>,
}

impl MockServer {
    /// Starts a server that answers the given routes.
    pub async fn start(routes: Vec<Route>) -> MockServer {
        start_impl(routes).await
    }

    /// Starts a server whose routes are built from the final base URL, so a
    /// recorded body can embed the server address.
    pub async fn start_with(build: impl FnOnce(&str) -> Vec<Route>) -> MockServer {
        let listener = bind().await;
        let base = base_of(&listener);
        let routes = build(&base);
        start_impl_on(listener, base, routes).await
    }

    /// The server address, without a trailing slash.
    pub fn base_url(&self) -> String {
        self.base.clone()
    }

    /// How many requests the server received for `path` (query string ignored).
    pub fn request_count(&self, path: &str) -> usize {
        self.hits.lock().unwrap().get(path).copied().unwrap_or(0)
    }
}

async fn bind() -> tokio::net::TcpListener {
    tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind the mock server to a loopback port")
}

fn base_of(listener: &tokio::net::TcpListener) -> String {
    format!(
        "http://{}",
        listener.local_addr().expect("read the mock server address")
    )
}

async fn start_impl(routes: Vec<Route>) -> MockServer {
    let listener = bind().await;
    let base = base_of(&listener);
    start_impl_on(listener, base, routes).await
}

async fn start_impl_on(
    listener: tokio::net::TcpListener,
    base: String,
    routes: Vec<Route>,
) -> MockServer {
    let hits: Arc<Mutex<HashMap<String, usize>>> = Arc::new(Mutex::new(HashMap::new()));
    let cursor: Arc<Mutex<HashMap<String, usize>>> = Arc::new(Mutex::new(HashMap::new()));

    let loop_hits = Arc::clone(&hits);
    let loop_cursor = Arc::clone(&cursor);
    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _peer)) = listener.accept().await else {
                break;
            };
            serve(&mut stream, &routes, &loop_hits, &loop_cursor).await;
        }
    });

    MockServer {
        base,
        hits,
        cursor,
        _handle: handle,
    }
}

async fn serve(
    stream: &mut TcpStream,
    routes: &[Route],
    hits: &Mutex<HashMap<String, usize>>,
    cursor: &Mutex<HashMap<String, usize>>,
) {
    let Some(request_line) = read_request_line(stream).await else {
        return;
    };

    let target = request_line.split_whitespace().nth(1).unwrap_or("/");
    let path = target.split('?').next().unwrap_or("/");
    let path = if path.is_empty() { "/" } else { path };

    // Critical sections stay clear of `.await` so the loop cannot deadlock and
    // clippy's `await_holding_lock` stays quiet.
    let index = {
        let mut cursor = cursor.lock().unwrap();
        let index = cursor.get(path).copied().unwrap_or(0);
        cursor.insert(path.to_string(), index + 1);
        index
    };
    {
        let mut hits = hits.lock().unwrap();
        *hits.entry(path.to_string()).or_insert(0) += 1;
    }

    let found = routes
        .iter()
        .find(|route| route.path == path)
        .and_then(|route| {
            let last = route.replies.len().checked_sub(1)?;
            route.replies.get(index.min(last))
        });

    let (status, content_type, body): (u16, &'static str, &[u8]) = match found {
        Some(reply) => (reply.status, reply.content_type, reply.body.as_slice()),
        None => (404, "text/plain", b""),
    };
    let reason = match status {
        200 => "OK",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        _ => "",
    };

    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let mut response = head.into_bytes();
    response.extend_from_slice(body);

    let _ = stream.write_all(&response).await;
    let _ = stream.shutdown().await;
}

/// Reads the request line, stopping at the blank line that ends the headers or
/// at a 64 KiB safety cap.
async fn read_request_line(stream: &mut TcpStream) -> Option<String> {
    const MAX_HEADER_BYTES: usize = 64 * 1024;

    let mut buf = Vec::new();
    let mut chunk = [0u8; 2048];
    loop {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..read]);
        if buf.windows(4).any(|window| window == b"\r\n\r\n") || buf.len() >= MAX_HEADER_BYTES {
            break;
        }
    }

    let head = String::from_utf8_lossy(&buf);
    head.lines().next().map(str::to_string)
}

/// Absolute path to a file under `tests/fixtures`.
pub fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(rel)
}

/// Writes a ZIP archive with the given members, deflated by default.
///
/// Parent directories are not created; callers keep the archive in a temp dir.
pub fn write_zip(path: &Path, members: &[(&str, &[u8])]) -> anyhow::Result<()> {
    let file = std::fs::File::create(path)?;
    let mut writer = zip::ZipWriter::new(file);
    for (name, bytes) in members {
        writer.start_file(*name, zip::write::SimpleFileOptions::default())?;
        writer.write_all(bytes)?;
    }
    writer.finish()?;
    Ok(())
}

/// Builds a valid tralbum_details payload with `track_count` streamable tracks.
///
/// Track numbers are one-based: `track_num` is the position inside the archive,
/// so a helper starting at zero would hide an off-by-one between the tralbum
/// metadata and the archive members.
pub fn tralbum_json(track_count: u32) -> String {
    let tracks: Vec<serde_json::Value> = (0..track_count)
        .map(|i| {
            serde_json::json!({
                "track_id": 9001 + i,
                "title": format!("Track {}", i + 1),
                "track_num": i + 1,
                "duration": 200.0 + (i + 1) as f64,
                "band_name": "Nightwave",
                "album_title": "Night Drive",
                "is_streamable": true,
                "track_url": format!("https://nightwave.bandcamp.com/track/track-{}", i + 1),
                "streaming_url": "https://t4.bcbits.com/stream/x/mp3-128/1",
            })
        })
        .collect();

    serde_json::json!({
        "id": 2814,
        "title": "Night Drive",
        "tralbum_artist": "Nightwave",
        "release_date": 1609459200,
        "free_download": true,
        "tracks": tracks,
    })
    .to_string()
}
