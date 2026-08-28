use std::fs::{self, File};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::thread;

use aros_common::sha256_file;
use flate2::write::GzEncoder;
use flate2::Compression;

fn fixture() -> (tempfile::TempDir, String) {
    let root = tempfile::tempdir().unwrap();
    let origin = root.path().join("origin");
    fs::create_dir(&origin).unwrap();
    let archive_path = origin.join("fixture.tar.gz");
    let file = File::create(&archive_path).unwrap();
    let mut archive = tar::Builder::new(GzEncoder::new(file, Compression::default()));
    let content = b"trusted payload\n";
    let mut header = tar::Header::new_gnu();
    header.set_size(content.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    archive
        .append_data(&mut header, "fixture-src/value.txt", &content[..])
        .unwrap();
    archive.into_inner().unwrap().finish().unwrap();
    let digest = sha256_file(&archive_path).unwrap().digest.to_string();
    (root, digest)
}

#[test]
fn legacy_contract_fetches_verifies_and_extracts_local_archive() {
    let (root, digest) = fixture();
    let cache = root.path().join("cache");
    let ports = root.path().join("ports");
    let origin = root.path().join("origin");
    let log = root.path().join("fetch.jsonl");
    let output = Command::new(env!("CARGO_BIN_EXE_aros-fetch"))
        .args([
            "-a",
            "fixture",
            "-s",
            "tar.gz",
            "-ao",
            origin.to_str().unwrap(),
            "-cs",
            &format!("fixture.tar.gz=sha256:{digest}"),
            "-l",
            cache.to_str().unwrap(),
            "-d",
            ports.to_str().unwrap(),
            "-b",
            ports.to_str().unwrap(),
            "-p",
            "::",
            "--log-level",
            "info",
            "--log-format",
            "jsonl",
            "--log-file",
            log.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(ports.join("fixture-src/value.txt")).unwrap(),
        "trusted payload\n"
    );
    let records = fs::read_to_string(log).unwrap();
    assert!(records.lines().all(|line| {
        serde_json::from_str::<serde_json::Value>(line).is_ok_and(|value| {
            value["schema"] == "aros-fetch-log-v1" && value.get("invocation").is_none()
        })
    }));
    assert!(!records.contains("timestamp"));
}

#[test]
fn tampered_cache_is_rejected_with_stable_json_diagnostic() {
    let (root, digest) = fixture();
    let cache = root.path().join("cache");
    let ports = root.path().join("ports");
    let origin = root.path().join("origin");
    fs::create_dir(&cache).unwrap();
    fs::write(cache.join("fixture.tar.gz"), b"tampered\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_aros-fetch"))
        .args([
            "--archive",
            "fixture",
            "--suffixes",
            "tar.gz",
            "--archive-origins",
            origin.to_str().unwrap(),
            "--checksums",
            &format!("fixture.tar.gz=sha256:{digest}"),
            "--location",
            cache.to_str().unwrap(),
            "--destination",
            ports.to_str().unwrap(),
            "--diagnostic-format",
            "json",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(value["schema"], "aros-tool-diagnostics-v1");
    assert_eq!(value["diagnostics"][0]["code"], "AF0401");
    assert_eq!(value["diagnostics"][0]["stage"], "integrity_validation");
}

#[test]
fn offline_miss_is_a_cache_diagnostic_and_never_attempts_network() {
    let root = tempfile::tempdir().unwrap();
    let cache = root.path().join("cache");
    let ports = root.path().join("ports");
    let output = Command::new(env!("CARGO_BIN_EXE_aros-fetch"))
        .args([
            "--archive",
            "fixture",
            "--suffixes",
            "tar.gz",
            "--archive-origins",
            "https://127.0.0.1:9",
            "--location",
            cache.to_str().unwrap(),
            "--destination",
            ports.to_str().unwrap(),
            "--offline",
            "--diagnostic-format",
            "json",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(value["diagnostics"][0]["code"], "AF0201");
}

#[test]
fn http_payload_is_streamed_verified_and_published() {
    let (root, digest) = fixture();
    let payload = fs::read(root.path().join("origin/fixture.tar.gz")).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 2048];
        let read = stream.read(&mut request).unwrap();
        assert!(String::from_utf8_lossy(&request[..read]).starts_with("GET /fixture.tar.gz "));
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            payload.len()
        )
        .unwrap();
        stream.write_all(&payload).unwrap();
    });
    let cache = root.path().join("http-cache");
    let ports = root.path().join("http-ports");
    let origin = format!("http://{address}");
    let contract = format!("fixture.tar.gz=sha256:{digest}");
    let output = Command::new(env!("CARGO_BIN_EXE_aros-fetch"))
        .args([
            "--archive",
            "fixture",
            "--suffixes",
            "tar.gz",
            "--archive-origins",
            &origin,
            "--checksums",
            &contract,
            "--location",
            cache.to_str().unwrap(),
            "--destination",
            ports.to_str().unwrap(),
            "--base",
            ports.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    server.join().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(ports.join("fixture-src/value.txt")).unwrap(),
        "trusted payload\n"
    );
}

#[test]
fn observability_configuration_has_its_own_stable_boundary() {
    let output = Command::new(env!("CARGO_BIN_EXE_aros-fetch"))
        .args([
            "--archive",
            "fixture",
            "--log-level",
            "info",
            "--diagnostic-format",
            "json",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(value["diagnostics"][0]["code"], "AF0002");
    assert_eq!(value["diagnostics"][0]["stage"], "fetch_observability");
}

#[test]
fn invalid_invocation_is_one_versioned_json_diagnostic() {
    let output = Command::new(env!("CARGO_BIN_EXE_aros-fetch"))
        .args(["--unknown-option", "--diagnostic-format", "json"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(value["schema"], "aros-tool-diagnostics-v1");
    assert_eq!(value["diagnostics"].as_array().unwrap().len(), 1);
    assert_eq!(value["diagnostics"][0]["code"], "AF0001");
}
