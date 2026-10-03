//! The hidden MetaMake bridge is exercised as the source-owned make target sees it.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use aros_common::sha256_bytes;
use aros_toolchain::metamake_fetch::SourceUseLedger;
use serde_json::json;

#[test]
fn bridge_resolves_one_locked_archive_and_forwards_only_a_private_verified_source() {
    let temporary = tempfile::tempdir().unwrap();
    let cache = temporary.path().join("cache");
    fs::create_dir(&cache).unwrap();
    let payload = b"locked payload\n";
    fs::write(cache.join("llvm-11.0.0.src.tar.xz"), payload).unwrap();
    let lock = temporary.path().join("sources.json");
    fs::write(
        &lock,
        serde_json::to_vec(&json!({
            "schema": "aros-toolchain-source-lock-v2", "family": "llvm", "version": "11.0.0",
            "sources": [{
                "component": "llvm", "version": "11.0.0", "purpose": "toolchain-component",
                "filename": "llvm-11.0.0.src.tar.xz", "url": "https://example.invalid/llvm.tar.xz",
                "sha256": sha256_bytes(payload), "size": payload.len()
            }],
            "host_python_packages": [{
                "name": "mako", "version": "1.3.10", "filename": "mako.tar.gz",
                "url": "https://example.invalid/mako.tar.gz", "sha256": sha256_bytes(payload), "size": payload.len(),
                "source_root": "mako", "python_path": "."
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    let ledger_path = temporary.path().join("usage.log");
    SourceUseLedger::create(&ledger_path).unwrap();
    let marker = temporary.path().join("forwarded.txt");
    let upstream = temporary.path().join("fetch.sh");
    write_executable(
        &upstream,
        "#!/bin/sh\nset -eu\ntest \"$AROS_FETCH_OFFLINE\" = 1\ntest \"$AROS_FETCH_REQUIRE_CHECKSUMS\" = 1\nprintf '%s\\n' \"$*\" > \"$AROS_BRIDGE_MARKER\"\nwhile test $# -gt 0; do\n if test \"$1\" = -l; then location=$2; fi\n shift\ndone\ncp \"$location/llvm-11.0.0.src.tar.xz\" \"$AROS_BRIDGE_MARKER.payload\"\nif test \"$AROS_TEST_CORRUPT\" = 1; then printf corrupt > \"$AROS_TEST_ORIGINAL\"; fi\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_aros"))
        .current_dir(temporary.path())
        .env("AROS_TOOLCHAIN_FETCH_LOCK", &lock)
        .env("AROS_TOOLCHAIN_FETCH_CACHE", &cache)
        .env("AROS_TOOLCHAIN_FETCH_LEDGER", &ledger_path)
        .env("AROS_TOOLCHAIN_FETCH_UPSTREAM", &upstream)
        .env("AROS_BRIDGE_MARKER", &marker)
        .env("AROS_TEST_CORRUPT", "0")
        .args([
            "toolchain",
            "__metamake-fetch",
            "-a",
            "llvm-11.0.0.src",
            "-s",
            "tar.xz",
            "-l",
            cache.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(ledger_path).unwrap(),
        "llvm-11.0.0.src.tar.xz\n"
    );
    assert_private_arguments(
        &marker,
        &cache,
        "llvm-11.0.0.src",
        "llvm-11.0.0.src.tar.xz",
        payload,
    );
    assert_eq!(
        fs::read(marker.with_extension("txt.payload")).unwrap(),
        payload
    );

    // Mutation after authorization cannot redirect the helper to new cache
    // bytes. The helper sees the private original, and no successful source
    // use is recorded after post-consumption revalidation fails.
    let failed_ledger = temporary.path().join("failed-usage.log");
    SourceUseLedger::create(&failed_ledger).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_aros"))
        .current_dir(temporary.path())
        .env("AROS_TOOLCHAIN_FETCH_LOCK", &lock)
        .env("AROS_TOOLCHAIN_FETCH_CACHE", &cache)
        .env("AROS_TOOLCHAIN_FETCH_LEDGER", &failed_ledger)
        .env("AROS_TOOLCHAIN_FETCH_UPSTREAM", &upstream)
        .env("AROS_BRIDGE_MARKER", &marker)
        .env("AROS_TEST_CORRUPT", "1")
        .env("AROS_TEST_ORIGINAL", cache.join("llvm-11.0.0.src.tar.xz"))
        .args([
            "toolchain",
            "__metamake-fetch",
            "-a",
            "llvm-11.0.0.src",
            "-s",
            "tar.xz",
            "-l",
            cache.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(
        fs::read(marker.with_extension("txt.payload")).unwrap(),
        payload
    );
    assert_eq!(fs::read_to_string(failed_ledger).unwrap(), "");
}

fn write_executable(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn assert_private_arguments(
    marker: &Path,
    cache: &Path,
    archive: &str,
    filename: &str,
    payload: &[u8],
) {
    let recorded = fs::read_to_string(marker).unwrap();
    let arguments = recorded.split_whitespace().collect::<Vec<_>>();
    assert_eq!(&arguments[..5], ["-a", archive, "-s", "tar.xz", "-l"]);
    let location = Path::new(arguments[5]);
    assert_ne!(location, cache);
    assert!(location
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("metamake-source-"));
    assert!(
        !location.exists(),
        "private source staging must be cleaned after the helper exits"
    );
    assert_eq!(arguments[6], "-cs");
    assert_eq!(
        arguments[7],
        format!("{filename}=sha256:{}", sha256_bytes(payload))
    );
    assert_eq!(arguments.len(), 8);
}

#[test]
fn gnu_bridge_narrows_formats_and_rejects_a_corrupt_selected_payload() {
    let temporary = tempfile::tempdir().unwrap();
    let cache = temporary.path().join("cache");
    fs::create_dir(&cache).unwrap();
    let payload = b"locked GNU payload\n";
    fs::write(cache.join("gcc.tar.xz"), payload).unwrap();
    fs::write(cache.join("gcc.tar.gz"), b"ambient earlier candidate").unwrap();
    let mut document: serde_json::Value = serde_json::from_slice(include_bytes!(
        "../../aros-toolchain/tests/fixtures/gnu-source-lock-v3.json"
    ))
    .unwrap();
    document["sources"][0]["sha256"] = json!(sha256_bytes(payload));
    document["sources"][0]["size"] = json!(payload.len());
    let lock = temporary.path().join("sources.json");
    fs::write(&lock, serde_json::to_vec(&document).unwrap()).unwrap();
    let ledger = temporary.path().join("usage.log");
    SourceUseLedger::create(&ledger).unwrap();
    let marker = temporary.path().join("forwarded.txt");
    let upstream = temporary.path().join("fetch.sh");
    write_executable(
        &upstream,
        "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$*\" > \"$AROS_BRIDGE_MARKER\"\n",
    );
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_aros"))
            .current_dir(temporary.path())
            .env("AROS_TOOLCHAIN_FETCH_LOCK", &lock)
            .env("AROS_TOOLCHAIN_FETCH_CACHE", &cache)
            .env("AROS_TOOLCHAIN_FETCH_LEDGER", &ledger)
            .env("AROS_TOOLCHAIN_FETCH_UPSTREAM", &upstream)
            .env("AROS_BRIDGE_MARKER", &marker)
            .args([
                "toolchain",
                "__metamake-fetch",
                "-a",
                "gcc",
                "-s",
                "tar.gz tar.xz tar.bz2",
                "-l",
                cache.to_str().unwrap(),
            ])
            .output()
            .unwrap()
    };
    let output = run();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_private_arguments(&marker, &cache, "gcc", "gcc.tar.xz", payload);
    assert_eq!(fs::read_to_string(&ledger).unwrap(), "gcc.tar.xz\n");
    fs::write(cache.join("gcc.tar.xz"), b"corrupt selected archive").unwrap();
    fs::write(&marker, b"upstream must not execute again").unwrap();
    assert!(!run().status.success());
    assert_eq!(
        fs::read(&marker).unwrap(),
        b"upstream must not execute again"
    );
    assert_eq!(fs::read_to_string(&ledger).unwrap(), "gcc.tar.xz\n");
    assert_eq!(
        fs::read(cache.join("gcc.tar.gz")).unwrap(),
        b"ambient earlier candidate"
    );
}
