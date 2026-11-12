use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use maxfetch::fetch::{download_to_file, http_client, ProgressSink};

const PAYLOAD: &[u8] = b"maxfetch test payload";

/// Counts progress callbacks so tests can assert what the downloader reported.
struct Counter {
    transferred: AtomicU64,
    finished: AtomicU64,
}

impl Counter {
    fn new() -> Self {
        Self {
            transferred: AtomicU64::new(0),
            finished: AtomicU64::new(0),
        }
    }

    fn transferred(&self) -> u64 {
        self.transferred.load(Ordering::SeqCst)
    }

    fn finished(&self) -> u64 {
        self.finished.load(Ordering::SeqCst)
    }
}

impl ProgressSink for Counter {
    fn on_progress(&self, transferred: u64, _total: Option<u64>) {
        self.transferred.fetch_add(transferred, Ordering::SeqCst);
    }

    fn on_finished(&self, _transferred: u64) {
        self.finished.fetch_add(1, Ordering::SeqCst);
    }
}

/// Serves `payload` exactly once over a local HTTP/1.1 connection.
fn serve_once(payload: &'static [u8]) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
    let addr = listener.local_addr().expect("local addr");

    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept connection");
        let mut request = [0u8; 1024];
        let _ = stream.read(&mut request);

        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            payload.len()
        );
        let _ = stream.write_all(header.as_bytes());
        let _ = stream.write_all(payload);
        let _ = stream.flush();
    });

    format!("http://{addr}/file")
}

/// Declares the full `Content-Length` but sends only `sent` bytes before closing,
/// so the client must observe a truncated body instead of a complete transfer.
fn serve_truncated(payload: &'static [u8], sent: usize) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
    let addr = listener.local_addr().expect("local addr");

    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept connection");
        let mut request = [0u8; 1024];
        let _ = stream.read(&mut request);

        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            payload.len()
        );
        let _ = stream.write_all(header.as_bytes());
        let _ = stream.write_all(&payload[..sent.min(payload.len())]);
        let _ = stream.flush();
        drop(stream);
    });

    format!("http://{addr}/file")
}

fn part_path(dest: &PathBuf) -> PathBuf {
    dest.with_extension("part")
}

#[tokio::test]
async fn downloads_a_file_and_renames_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dest = dir.path().join("out.bin");
    let url = serve_once(PAYLOAD);
    let sink = Counter::new();

    download_to_file(&reqwest::Client::new(), &url, &dest, &sink)
        .await
        .expect("download succeeds");

    assert_eq!(std::fs::read(&dest).expect("read downloaded file"), PAYLOAD);
    assert!(!part_path(&dest).exists(), ".part file is renamed away");
    assert_eq!(sink.transferred(), PAYLOAD.len() as u64);
    assert_eq!(sink.finished(), 1);
}

#[tokio::test]
async fn failed_transfer_keeps_part_file_and_no_dest() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dest = dir.path().join("out.bin");
    let url = serve_truncated(PAYLOAD, 4);
    let sink = Counter::new();

    let result = download_to_file(&reqwest::Client::new(), &url, &dest, &sink).await;

    assert!(result.is_err(), "truncated transfer must fail: {result:?}");
    assert!(!dest.exists(), "destination must not appear on failure");
    assert!(
        part_path(&dest).exists(),
        "incomplete .part file must be kept on failure"
    );
    assert_eq!(sink.finished(), 0, "on_finished must not fire on failure");
}

#[tokio::test]
async fn http_client_builds_with_timeout() {
    let client = http_client(5).expect("client builds");
    let debug = format!("{client:?}");
    assert!(
        debug.contains("TotalTimeout: 5s"),
        "timeout should be configured: {debug}"
    );
    assert!(
        debug.contains("cookie_store"),
        "cookie store should be enabled: {debug}"
    );
}
