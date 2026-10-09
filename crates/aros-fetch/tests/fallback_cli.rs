//! A declared source that answers with the wrong bytes must not end the
//! fetch while another declared source still has the archive.

use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::Path;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use aros_common::sha256_bytes;
use flate2::write::GzEncoder;
use flate2::Compression;

const HTML_REDIRECT_PAGE: &[u8] = b"<html>\r\n<head><title>301 Moved Permanently</title></head>\r\n<body><center><h1>301 Moved Permanently</h1></center></body>\r\n</html>\r\n";

fn archive(content: &[u8]) -> Vec<u8> {
    let mut archive = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
    let mut header = tar::Header::new_gnu();
    header.set_size(content.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    archive
        .append_data(&mut header, "fixture-src/value.txt", content)
        .unwrap();
    archive.into_inner().unwrap().finish().unwrap()
}

fn response(status: &str, body: &[u8]) -> Vec<u8> {
    let mut response = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    response
}

/// Answers the n-th connection with `responses[n]` (the last one repeats)
/// until stopped, and reports how many requests arrived.
struct Server {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: JoinHandle<usize>,
}

impl Server {
    fn start(responses: Vec<Vec<u8>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            let mut served = 0;
            while !stopped.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).unwrap();
                        let mut request = [0_u8; 2048];
                        let _ = stream.read(&mut request).unwrap();
                        let answer = &responses[served.min(responses.len() - 1)];
                        stream.write_all(answer).unwrap();
                        served += 1;
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("test server accept failed: {error}"),
                }
            }
            served
        });
        Self {
            address,
            stop,
            thread,
        }
    }

    fn origin(&self) -> String {
        format!("http://{}", self.address)
    }

    fn requests(self) -> usize {
        self.stop.store(true, Ordering::SeqCst);
        self.thread.join().unwrap()
    }
}

fn fetch(root: &Path, origins: &[String], checksum: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_aros-fetch"));
    command.args(["--archive", "fixture", "--suffixes", "tar.gz"]);
    command.args(["--archive-origins", &origins.join(" ")]);
    if let Some(digest) = checksum {
        command.args(["--checksums", &format!("fixture.tar.gz=sha256:{digest}")]);
    }
    command
        .args(["--location", root.join("cache").to_str().unwrap()])
        .args(["--destination", root.join("ports").to_str().unwrap()])
        .args(["--base", root.join("ports").to_str().unwrap()])
        .args(["--diagnostic-format", "json"])
        .output()
        .unwrap()
}

fn assert_extracted(root: &Path, output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(root.join("ports/fixture-src/value.txt")).unwrap(),
        "trusted payload\n"
    );
    let cached = fs::read(root.join("cache/fixture.tar.gz")).unwrap();
    assert_eq!(cached, archive(b"trusted payload\n"));
}

#[test]
fn an_html_page_from_one_source_falls_back_to_the_next() {
    let root = tempfile::tempdir().unwrap();
    let page = Server::start(vec![response("200 OK", HTML_REDIRECT_PAGE)]);
    let mirror = Server::start(vec![response("200 OK", &archive(b"trusted payload\n"))]);
    let output = fetch(root.path(), &[page.origin(), mirror.origin()], None);
    assert_eq!(page.requests(), 1);
    assert_eq!(mirror.requests(), 1);
    assert_extracted(root.path(), &output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(
            "Failed     fixture.tar.gz from declared origin 1: payload is not a gzip archive"
        ),
        "{stdout}"
    );
}

#[test]
fn a_digest_mismatch_from_one_source_falls_back_to_the_next() {
    let root = tempfile::tempdir().unwrap();
    let trusted = archive(b"trusted payload\n");
    let digest = sha256_bytes(&trusted).to_string();
    let stale = Server::start(vec![response("200 OK", &archive(b"stale payload\n"))]);
    let mirror = Server::start(vec![response("200 OK", &trusted)]);
    let output = fetch(
        root.path(),
        &[stale.origin(), mirror.origin()],
        Some(&digest),
    );
    assert_eq!(stale.requests(), 1);
    assert_eq!(mirror.requests(), 1);
    assert_extracted(root.path(), &output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("SHA-256 mismatch"), "{stdout}");
}

#[test]
fn a_digest_mismatch_from_every_source_stays_an_integrity_failure() {
    let root = tempfile::tempdir().unwrap();
    let digest = sha256_bytes(&archive(b"trusted payload\n")).to_string();
    let stale = Server::start(vec![response("200 OK", &archive(b"stale payload\n"))]);
    let down = Server::start(vec![response("404 Not Found", b"")]);
    let output = fetch(root.path(), &[stale.origin(), down.origin()], Some(&digest));
    stale.requests();
    down.requests();
    assert!(!output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(value["diagnostics"][0]["code"], "AF0401");
    assert!(!root.path().join("cache/fixture.tar.gz").exists());
    assert!(!root.path().join("ports/fixture-src").exists());
}

#[test]
fn a_definitive_http_error_is_not_retried() {
    let root = tempfile::tempdir().unwrap();
    let missing = Server::start(vec![response("404 Not Found", b"")]);
    let output = fetch(root.path(), &[missing.origin()], None);
    assert!(!output.status.success());
    assert_eq!(missing.requests(), 1);
}

#[test]
fn a_transient_http_error_is_retried_after_a_pause() {
    let root = tempfile::tempdir().unwrap();
    let busy = Server::start(vec![
        response("503 Service Unavailable", b""),
        response("200 OK", &archive(b"trusted payload\n")),
    ]);
    let output = fetch(root.path(), &[busy.origin()], None);
    assert_eq!(busy.requests(), 2);
    assert_extracted(root.path(), &output);
}
