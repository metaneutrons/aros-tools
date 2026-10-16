//! ZIP dependency compatibility through the real offline extraction CLI.

use std::fs;
use std::io::{Cursor, Write};
use std::process::{Command, Output};

use aros_common::sha256_file;
use zip::write::SimpleFileOptions;
use zip::CompressionMethod;

const CONTENT: &[u8] = b"trusted ZIP payload\n";

fn archive(method: CompressionMethod) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .start_file(
            "source/value.txt",
            SimpleFileOptions::default().compression_method(method),
        )
        .unwrap();
    writer.write_all(CONTENT).unwrap();
    writer.finish().unwrap().into_inner()
}

fn run(root: &std::path::Path, bytes: &[u8]) -> Output {
    let cache = root.join("cache");
    fs::create_dir(&cache).unwrap();
    let path = cache.join("fixture.zip");
    fs::write(&path, bytes).unwrap();
    let digest = sha256_file(&path).unwrap().digest;
    let destination = root.join("destination");
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("sentinel"), b"unchanged").unwrap();
    Command::new(env!("CARGO_BIN_EXE_aros-fetch"))
        .args([
            "--archive",
            "fixture",
            "--suffixes",
            "zip",
            "--offline",
            "--require-checksums",
            "--checksums",
            &format!("fixture.zip=sha256:{digest}"),
            "--location",
        ])
        .arg(cache)
        .arg("--destination")
        .arg(destination)
        .arg("--base")
        .arg(root.join("base"))
        .output()
        .unwrap()
}

fn assert_rejected(bytes: &[u8], reason: &str) {
    let root = tempfile::tempdir().unwrap();
    let result = run(root.path(), bytes);
    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("AF0501"), "{stderr}");
    assert!(stderr.contains(reason), "{stderr}");
    let destination = root.path().join("destination");
    assert_eq!(
        fs::read(destination.join("sentinel")).unwrap(),
        b"unchanged"
    );
    assert_eq!(fs::read_dir(destination).unwrap().count(), 1);
    assert!(!root.path().join("my-escape.txt").exists());
}

#[test]
fn stored_deflate_and_bzip2_archives_extract_identical_payloads() {
    for method in [
        CompressionMethod::Stored,
        CompressionMethod::Deflated,
        CompressionMethod::Bzip2,
    ] {
        let root = tempfile::tempdir().unwrap();
        let result = run(root.path(), &archive(method));
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            fs::read(root.path().join("destination/source/value.txt")).unwrap(),
            CONTENT
        );
    }
}

#[test]
fn escaping_filename_is_rejected_without_publication() {
    let mut bytes = archive(CompressionMethod::Stored);
    let central = bytes.windows(4).position(|w| w == b"PK\x01\x02").unwrap();
    // Replace the same-length filename in both headers. The fallible ZIP 9
    // name API must not change the existing enclosed-path rejection policy.
    // One parent leaves private extraction staging but stays inside this test's
    // own root even if a future regression allows the unsafe member through.
    let escaping = b"../my-escape.txt";
    assert_eq!(escaping.len(), b"source/value.txt".len());
    bytes[30..30 + escaping.len()].copy_from_slice(escaping);
    bytes[central + 46..central + 46 + escaping.len()].copy_from_slice(escaping);
    assert_rejected(&bytes, "escapes the extraction root");
}

#[test]
fn corrupt_payload_crc_is_rejected_without_publication() {
    let mut bytes = archive(CompressionMethod::Stored);
    let offset = bytes
        .windows(CONTENT.len())
        .position(|w| w == CONTENT)
        .unwrap();
    bytes[offset] ^= 1;
    // The outer archive SHA is deliberately correct for these corrupt bytes;
    // rejection must come from ZIP's member integrity check.
    assert_rejected(&bytes, "Invalid checksum");
}
