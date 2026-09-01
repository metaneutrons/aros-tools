use std::fs::{self, File};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

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
fn failed_patch_never_exposes_the_prepared_archive_tree() {
    let (root, digest) = fixture();
    let cache = root.path().join("transaction-cache");
    let patch_cache = root.path().join("patch-cache");
    let ports = root.path().join("transaction-ports");
    let origin = root.path().join("origin");
    fs::create_dir(&ports).unwrap();
    fs::write(ports.join("preexisting.txt"), b"must survive\n").unwrap();
    fs::write(origin.join("broken.patch"), b"this is not a patch\n").unwrap();

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
            "--base",
            patch_cache.to_str().unwrap(),
            "--patch-origins",
            origin.to_str().unwrap(),
            "--patches",
            "broken.patch:fixture-src:-p0",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(
        fs::read_to_string(ports.join("preexisting.txt")).unwrap(),
        "must survive\n"
    );
    assert!(!ports.join("fixture-src").exists());
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
    assert!(value["diagnostics"][0]["hint"]
        .as_str()
        .is_some_and(|hint| !hint.trim().is_empty()));
}

#[test]
fn help_is_nonempty_and_documents_the_fetch_contract() {
    let output = Command::new(env!("CARGO_BIN_EXE_aros-fetch"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(!help.trim().is_empty());
    for required in [
        "Usage: aros-fetch",
        "--archive <ARCHIVE>",
        "--checksums <CHECKSUMS>",
        "CHECKSUM CONTRACT:",
        "OBSERVABILITY:",
        "AROS_FETCH_LOG_FILE",
    ] {
        assert!(help.contains(required), "help omits {required:?}:\n{help}");
    }
}

#[test]
fn stale_legacy_marker_never_authorizes_a_skip() {
    let (root, digest) = fixture();
    let cache = root.path().join("stale-cache");
    let base = root.path().join("stale-base");
    let destination = root.path().join("stale-destination");
    let origin = root.path().join("origin");
    let arguments = [
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
        destination.to_str().unwrap(),
        "--base",
        base.to_str().unwrap(),
    ];
    let first = Command::new(env!("CARGO_BIN_EXE_aros-fetch"))
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(base.join(".fixture.tar.gz.unpacked").is_file());
    fs::remove_dir_all(destination.join("fixture-src")).unwrap();
    let retry = Command::new(env!("CARGO_BIN_EXE_aros-fetch"))
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        retry.status.success(),
        "{}",
        String::from_utf8_lossy(&retry.stderr)
    );
    assert_eq!(
        fs::read_to_string(destination.join("fixture-src/value.txt")).unwrap(),
        "trusted payload\n"
    );
}

#[cfg(unix)]
#[test]
fn indeterminate_exchange_retry_uses_internal_receipt() {
    let (root, digest) = fixture();
    let cache = root.path().join("retry-cache");
    let base = root.path().join("retry-base");
    let destination = root.path().join("retry-destination");
    let origin = root.path().join("origin");
    let checksum = format!("fixture.tar.gz=sha256:{digest}");
    let arguments = [
        "--archive",
        "fixture",
        "--suffixes",
        "tar.gz",
        "--archive-origins",
        origin.to_str().unwrap(),
        "--checksums",
        checksum.as_str(),
        "--location",
        cache.to_str().unwrap(),
        "--destination",
        destination.to_str().unwrap(),
        "--base",
        base.to_str().unwrap(),
        "--diagnostic-format",
        "json",
    ];
    let first = Command::new(env!("CARGO_BIN_EXE_aros-fetch"))
        .args(arguments)
        .env(
            "AROS_PUBLICATION_TEST_FAIL_AT",
            "prepared-tree-after-exchange-before-sync",
        )
        .output()
        .unwrap();
    assert!(!first.status.success());
    let diagnostic: serde_json::Value = serde_json::from_slice(&first.stderr).unwrap();
    assert_eq!(
        diagnostic["diagnostics"][0]["context"]["commit_state"],
        "indeterminate"
    );
    assert!(destination.join("fixture-src/value.txt").is_file());
    let retry = Command::new(env!("CARGO_BIN_EXE_aros-fetch"))
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        retry.status.success(),
        "{}",
        String::from_utf8_lossy(&retry.stderr)
    );
}

#[cfg(unix)]
#[test]
fn concurrent_destination_mutation_fails_tree_cas_without_data_loss() {
    let (root, digest) = fixture();
    let cache = root.path().join("cas-cache");
    let base = root.path().join("cas-base");
    let destination = root.path().join("cas-destination");
    let origin = root.path().join("origin");
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("concurrent.txt"), b"before\n").unwrap();
    let binary = env!("CARGO_BIN_EXE_aros-fetch");
    let checksum = format!("fixture.tar.gz=sha256:{digest}");
    let cache_for_thread = cache;
    let base_for_thread = base;
    let destination_for_thread = destination.clone();
    let origin_for_thread = origin;
    let worker = thread::spawn(move || {
        Command::new(binary)
            .args([
                "--archive",
                "fixture",
                "--suffixes",
                "tar.gz",
                "--archive-origins",
                origin_for_thread.to_str().unwrap(),
                "--checksums",
                &checksum,
                "--location",
                cache_for_thread.to_str().unwrap(),
                "--destination",
                destination_for_thread.to_str().unwrap(),
                "--base",
                base_for_thread.to_str().unwrap(),
                "--force",
                "--diagnostic-format",
                "json",
            ])
            .env(
                "AROS_PUBLICATION_TEST_PAUSE_AT",
                "prepared-tree-before-content-cas",
            )
            .env("AROS_PUBLICATION_TEST_PAUSE_MS", "500")
            .output()
            .unwrap()
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let staged = fs::read_dir(root.path())
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".aros-fetch-publish-")
            });
        if staged {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "publication staging never appeared"
        );
        thread::sleep(Duration::from_millis(10));
    }
    fs::write(destination.join("concurrent.txt"), b"concurrent\n").unwrap();
    let output = worker.join().unwrap();
    assert!(!output.status.success());
    let diagnostic: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(
        diagnostic["diagnostics"][0]["context"]["commit_state"],
        "rolled_back"
    );
    assert_eq!(
        fs::read_to_string(destination.join("concurrent.txt")).unwrap(),
        "concurrent\n"
    );
    assert!(!destination.join("fixture-src").exists());
}

#[test]
fn post_commit_log_failure_cannot_reverse_success() {
    let (root, digest) = fixture();
    let cache = root.path().join("log-cache");
    let base = root.path().join("log-base");
    let destination = root.path().join("log-destination");
    let origin = root.path().join("origin");
    let log = root.path().join("fetch.log");
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
            destination.to_str().unwrap(),
            "--base",
            base.to_str().unwrap(),
            "--log-level",
            "info",
            "--log-file",
            log.to_str().unwrap(),
        ])
        .env("AROS_FETCH_TEST_LOG_FAIL_AT", "archive.extracted")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(destination.join("fixture-src/value.txt").is_file());
}

#[test]
fn terminal_log_failure_after_publication_is_typed_committed() {
    let (root, digest) = fixture();
    let cache = root.path().join("terminal-log-cache");
    let base = root.path().join("terminal-log-base");
    let destination = root.path().join("terminal-log-destination");
    let origin = root.path().join("origin");
    let log = root.path().join("terminal-fetch.log");
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
            destination.to_str().unwrap(),
            "--base",
            base.to_str().unwrap(),
            "--log-level",
            "info",
            "--log-file",
            log.to_str().unwrap(),
            "--diagnostic-format",
            "json",
        ])
        .env("AROS_FETCH_TEST_LOG_FAIL_AT", "invocation.complete")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let diagnostic: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(
        diagnostic["diagnostics"][0]["context"]["commit_state"],
        "committed"
    );
    assert!(destination.join("fixture-src/value.txt").is_file());
}

#[test]
fn same_patch_name_from_different_origins_never_aliases_shared_cache() {
    let (root, digest) = fixture();
    let origin_a = root.path().join("patch-a");
    let origin_b = root.path().join("patch-b");
    fs::create_dir(&origin_a).unwrap();
    fs::create_dir(&origin_b).unwrap();
    let patch = |replacement: &str| {
        format!("--- value.txt\n+++ value.txt\n@@ -1 +1 @@\n-trusted payload\n+{replacement}\n")
    };
    fs::write(origin_a.join("change.patch"), patch("origin-a")).unwrap();
    fs::write(origin_b.join("change.patch"), patch("origin-b")).unwrap();
    let origin_a_retry = origin_a.clone();
    let shared_base = root.path().join("shared-patch-cache");
    let archive_origin = root.path().join("origin");
    let spawn = |label: &'static str, patch_origin: std::path::PathBuf| {
        let destination = root.path().join(format!("destination-{label}"));
        let location = root.path().join(format!("cache-{label}"));
        let shared_base = shared_base.clone();
        let archive_origin = archive_origin.clone();
        let checksum = format!("fixture.tar.gz=sha256:{digest}");
        (
            destination.clone(),
            thread::spawn(move || {
                Command::new(env!("CARGO_BIN_EXE_aros-fetch"))
                    .args([
                        "--archive",
                        "fixture",
                        "--suffixes",
                        "tar.gz",
                        "--archive-origins",
                        archive_origin.to_str().unwrap(),
                        "--checksums",
                        &checksum,
                        "--location",
                        location.to_str().unwrap(),
                        "--destination",
                        destination.to_str().unwrap(),
                        "--base",
                        shared_base.to_str().unwrap(),
                        "--patch-origins",
                        patch_origin.to_str().unwrap(),
                        "--patches",
                        "change.patch:fixture-src:-p0",
                    ])
                    .output()
                    .unwrap()
            }),
        )
    };
    let (destination_a, worker_a) = spawn("a", origin_a);
    let (destination_b, worker_b) = spawn("b", origin_b);
    let output_a = worker_a.join().unwrap();
    let output_b = worker_b.join().unwrap();
    assert!(
        output_a.status.success(),
        "{}",
        String::from_utf8_lossy(&output_a.stderr)
    );
    assert!(
        output_b.status.success(),
        "{}",
        String::from_utf8_lossy(&output_b.stderr)
    );
    assert_eq!(
        fs::read_to_string(destination_a.join("fixture-src/value.txt")).unwrap(),
        "origin-a\n"
    );
    assert_eq!(
        fs::read_to_string(destination_b.join("fixture-src/value.txt")).unwrap(),
        "origin-b\n"
    );
    let retry = Command::new(env!("CARGO_BIN_EXE_aros-fetch"))
        .args([
            "--archive",
            "fixture",
            "--suffixes",
            "tar.gz",
            "--archive-origins",
            archive_origin.to_str().unwrap(),
            "--checksums",
            &format!("fixture.tar.gz=sha256:{digest}"),
            "--location",
            root.path().join("cache-a").to_str().unwrap(),
            "--destination",
            destination_a.to_str().unwrap(),
            "--base",
            shared_base.to_str().unwrap(),
            "--patch-origins",
            origin_a_retry.to_str().unwrap(),
            "--patches",
            "change.patch:fixture-src:-p0",
        ])
        .output()
        .unwrap();
    assert!(
        retry.status.success(),
        "{}",
        String::from_utf8_lossy(&retry.stderr)
    );
}

#[cfg(unix)]
#[test]
fn symlinked_internal_receipt_is_rejected_fail_closed() {
    use std::os::unix::fs::symlink;

    let (root, digest) = fixture();
    let cache = root.path().join("symlink-cache");
    let base = root.path().join("symlink-base");
    let destination = root.path().join("symlink-destination");
    let origin = root.path().join("origin");
    let checksum = format!("fixture.tar.gz=sha256:{digest}");
    let arguments = [
        "--archive",
        "fixture",
        "--suffixes",
        "tar.gz",
        "--archive-origins",
        origin.to_str().unwrap(),
        "--checksums",
        checksum.as_str(),
        "--location",
        cache.to_str().unwrap(),
        "--destination",
        destination.to_str().unwrap(),
        "--base",
        base.to_str().unwrap(),
        "--diagnostic-format",
        "json",
    ];
    let first = Command::new(env!("CARGO_BIN_EXE_aros-fetch"))
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let receipt = fs::read_dir(destination.join(".aros-fetch/receipts"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let external = root.path().join("external-receipt.json");
    fs::copy(&receipt, &external).unwrap();
    fs::remove_file(&receipt).unwrap();
    symlink(&external, &receipt).unwrap();

    let retry = Command::new(env!("CARGO_BIN_EXE_aros-fetch"))
        .args(arguments)
        .output()
        .unwrap();
    assert!(!retry.status.success());
    let diagnostic: serde_json::Value = serde_json::from_slice(&retry.stderr).unwrap();
    assert_eq!(diagnostic["diagnostics"][0]["code"], "AF0701");
    assert_eq!(
        fs::read_to_string(destination.join("fixture-src/value.txt")).unwrap(),
        "trusted payload\n"
    );
}

#[test]
fn compressed_patch_cache_publishes_payload_and_receipt_as_one_tree() {
    let (root, digest) = fixture();
    let patch_origin = root.path().join("compressed-patch-origin");
    fs::create_dir(&patch_origin).unwrap();
    let archive_file = File::create(patch_origin.join("change.patch.tar.gz")).unwrap();
    let mut archive = tar::Builder::new(GzEncoder::new(archive_file, Compression::default()));
    let patch = b"--- value.txt\n+++ value.txt\n@@ -1 +1 @@\n-trusted payload\n+compressed\n";
    let mut header = tar::Header::new_gnu();
    header.set_size(patch.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    archive
        .append_data(&mut header, "change.patch", &patch[..])
        .unwrap();
    archive.into_inner().unwrap().finish().unwrap();

    let cache = root.path().join("compressed-cache");
    let base = root.path().join("compressed-base");
    let destination = root.path().join("compressed-destination");
    let origin = root.path().join("origin");
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
            destination.to_str().unwrap(),
            "--base",
            base.to_str().unwrap(),
            "--patch-origins",
            patch_origin.to_str().unwrap(),
            "--patches",
            "change.patch:fixture-src:-p0",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(destination.join("fixture-src/value.txt")).unwrap(),
        "compressed\n"
    );
    let declaration_root = fs::read_dir(base.join(".aros-fetch/patch-cache"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let payload_root = fs::read_dir(declaration_root.join("payloads"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let members = fs::read_dir(payload_root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        members,
        ["payload.patch", "receipt.json"]
            .into_iter()
            .map(std::ffi::OsString::from)
            .collect()
    );
}

#[test]
fn parallel_contracts_for_same_exact_candidate_share_one_cache_lock() {
    let (root, _digest) = fixture();
    let origin = root.path().join("origin");
    fs::copy(origin.join("fixture.tar.gz"), origin.join("foo.tar.gz")).unwrap();
    let cache = root.path().join("candidate-cache");
    let spawn = |archive: &'static str, suffix: &'static str, label: &'static str| {
        let origin = origin.clone();
        let cache = cache.clone();
        let destination = root.path().join(format!("candidate-destination-{label}"));
        let base = root.path().join(format!("candidate-base-{label}"));
        thread::spawn(move || {
            Command::new(env!("CARGO_BIN_EXE_aros-fetch"))
                .args([
                    "--archive",
                    archive,
                    "--suffixes",
                    suffix,
                    "--archive-origins",
                    origin.to_str().unwrap(),
                    "--location",
                    cache.to_str().unwrap(),
                    "--destination",
                    destination.to_str().unwrap(),
                    "--base",
                    base.to_str().unwrap(),
                ])
                .output()
                .unwrap()
        })
    };
    let first = spawn("foo", "tar.gz", "a");
    let second = spawn("foo.tar", "gz", "b");
    let first = first.join().unwrap();
    let second = second.join().unwrap();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert!(cache.join("foo.tar.gz").is_file());
    assert_eq!(
        fs::read_dir(&cache)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name() == "foo.tar.gz")
            .count(),
        1
    );
}

#[test]
fn changed_local_patch_is_reimported_into_a_new_content_namespace() {
    let (root, digest) = fixture();
    let patch_origin = root.path().join("mutable-patch-origin");
    fs::create_dir(&patch_origin).unwrap();
    let patch = |replacement: &str| {
        format!("--- value.txt\n+++ value.txt\n@@ -1 +1 @@\n-trusted payload\n+{replacement}\n")
    };
    fs::write(patch_origin.join("change.patch"), patch("patch-a")).unwrap();
    let cache = root.path().join("mutable-cache");
    let base = root.path().join("mutable-base");
    let destination = root.path().join("mutable-destination");
    let origin = root.path().join("origin");
    let checksum = format!("fixture.tar.gz=sha256:{digest}");
    let run = |force: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_aros-fetch"));
        command.args([
            "--archive",
            "fixture",
            "--suffixes",
            "tar.gz",
            "--archive-origins",
            origin.to_str().unwrap(),
            "--checksums",
            checksum.as_str(),
            "--location",
            cache.to_str().unwrap(),
            "--destination",
            destination.to_str().unwrap(),
            "--base",
            base.to_str().unwrap(),
            "--patch-origins",
            patch_origin.to_str().unwrap(),
            "--patches",
            "change.patch:fixture-src:-p0",
        ]);
        if force {
            command.arg("--force");
        }
        command.output().unwrap()
    };
    let first = run(false);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert_eq!(
        fs::read_to_string(destination.join("fixture-src/value.txt")).unwrap(),
        "patch-a\n"
    );
    fs::write(patch_origin.join("change.patch"), patch("patch-b")).unwrap();
    let second = run(true);
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(
        fs::read_to_string(destination.join("fixture-src/value.txt")).unwrap(),
        "patch-b\n"
    );
    let declaration = fs::read_dir(base.join(".aros-fetch/patch-cache"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(
        fs::read_dir(declaration.join("payloads"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .count(),
        2
    );
}

#[cfg(unix)]
#[test]
fn local_patch_change_before_commit_fails_completion_revalidation() {
    let (root, digest) = fixture();
    let patch_origin = root.path().join("revalidation-patch-origin");
    fs::create_dir(&patch_origin).unwrap();
    let patch_path = patch_origin.join("change.patch");
    fs::write(
        &patch_path,
        b"--- value.txt\n+++ value.txt\n@@ -1 +1 @@\n-trusted payload\n+prepared\n",
    )
    .unwrap();
    let cache = root.path().join("revalidation-cache");
    let base = root.path().join("revalidation-base");
    let destination = root.path().join("revalidation-destination");
    let origin = root.path().join("origin");
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("preexisting"), b"preserved").unwrap();
    let checksum = format!("fixture.tar.gz=sha256:{digest}");
    let root_path = root.path().to_path_buf();
    let worker = thread::spawn(move || {
        Command::new(env!("CARGO_BIN_EXE_aros-fetch"))
            .args([
                "--archive",
                "fixture",
                "--suffixes",
                "tar.gz",
                "--archive-origins",
                origin.to_str().unwrap(),
                "--checksums",
                &checksum,
                "--location",
                cache.to_str().unwrap(),
                "--destination",
                destination.to_str().unwrap(),
                "--base",
                base.to_str().unwrap(),
                "--patch-origins",
                patch_origin.to_str().unwrap(),
                "--patches",
                "change.patch:fixture-src:-p0",
                "--diagnostic-format",
                "json",
            ])
            .env("AROS_FETCH_TEST_PAUSE_AT", "before-payload-revalidation")
            .env("AROS_FETCH_TEST_PAUSE_MS", "500")
            .output()
            .unwrap()
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while !fs::read_dir(&root_path)
        .unwrap()
        .filter_map(Result::ok)
        .any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".aros-fetch-publish-")
        })
    {
        assert!(
            Instant::now() < deadline,
            "publication staging never appeared"
        );
        thread::sleep(Duration::from_millis(10));
    }
    fs::write(&patch_path, b"changed during transaction\n").unwrap();
    let output = worker.join().unwrap();
    assert!(!output.status.success());
    let diagnostic: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(
        diagnostic["diagnostics"][0]["context"]["commit_state"],
        "rolled_back"
    );
    assert_eq!(
        fs::read(root_path.join("revalidation-destination/preexisting")).unwrap(),
        b"preserved"
    );
    assert!(!root_path
        .join("revalidation-destination/fixture-src")
        .exists());
}

#[test]
fn tar_hardlink_archive_is_rejected_before_publication() {
    let root = tempfile::tempdir().unwrap();
    let origin = root.path().join("hardlink-origin");
    fs::create_dir(&origin).unwrap();
    let archive_file = File::create(origin.join("bomb.tar.gz")).unwrap();
    let mut archive = tar::Builder::new(GzEncoder::new(archive_file, Compression::default()));
    let contents = b"one logical payload\n";
    let mut regular = tar::Header::new_gnu();
    regular.set_size(contents.len() as u64);
    regular.set_mode(0o644);
    regular.set_cksum();
    archive
        .append_data(&mut regular, "bomb-src/value", &contents[..])
        .unwrap();
    let mut hardlink = tar::Header::new_gnu();
    hardlink.set_entry_type(tar::EntryType::Link);
    hardlink.set_size(0);
    hardlink.set_mode(0o644);
    hardlink.set_link_name("bomb-src/value").unwrap();
    hardlink.set_cksum();
    archive
        .append_data(&mut hardlink, "bomb-src/alias", std::io::empty())
        .unwrap();
    archive.into_inner().unwrap().finish().unwrap();
    let destination = root.path().join("destination");
    let output = Command::new(env!("CARGO_BIN_EXE_aros-fetch"))
        .args([
            "--archive",
            "bomb",
            "--suffixes",
            "tar.gz",
            "--archive-origins",
            origin.to_str().unwrap(),
            "--location",
            root.path().join("cache").to_str().unwrap(),
            "--destination",
            destination.to_str().unwrap(),
            "--base",
            root.path().join("base").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!destination.join("bomb-src").exists());
    assert!(String::from_utf8_lossy(&output.stderr).contains("hard link"));
}

#[test]
fn zip_declared_size_bomb_is_bounded_and_rejected() {
    use std::io::Cursor;
    use zip::write::SimpleFileOptions;

    let root = tempfile::tempdir().unwrap();
    let origin = root.path().join("zip-bomb-origin");
    fs::create_dir(&origin).unwrap();
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .start_file(
            "bomb-src/value",
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated),
        )
        .unwrap();
    writer.write_all(&vec![b'z'; 1024 * 1024]).unwrap();
    let mut bytes = writer.finish().unwrap().into_inner();
    let local = bytes
        .windows(4)
        .position(|window| window == b"PK\x03\x04")
        .unwrap();
    bytes[local + 22..local + 26].copy_from_slice(&1_u32.to_le_bytes());
    let central = bytes
        .windows(4)
        .position(|window| window == b"PK\x01\x02")
        .unwrap();
    bytes[central + 24..central + 28].copy_from_slice(&1_u32.to_le_bytes());
    fs::write(origin.join("bomb.zip"), bytes).unwrap();
    let destination = root.path().join("destination");
    let output = Command::new(env!("CARGO_BIN_EXE_aros-fetch"))
        .args([
            "--archive",
            "bomb",
            "--suffixes",
            "zip",
            "--archive-origins",
            origin.to_str().unwrap(),
            "--location",
            root.path().join("cache").to_str().unwrap(),
            "--destination",
            destination.to_str().unwrap(),
            "--base",
            root.path().join("base").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!destination.join("bomb-src").exists());
}
