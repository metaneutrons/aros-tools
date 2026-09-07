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
fn bridge_resolves_one_locked_archive_records_it_and_forwards_original_arguments() {
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
        "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$*\" > \"$AROS_BRIDGE_MARKER\"\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_aros"))
        .current_dir(temporary.path())
        .env("AROS_TOOLCHAIN_FETCH_LOCK", &lock)
        .env("AROS_TOOLCHAIN_FETCH_CACHE", &cache)
        .env("AROS_TOOLCHAIN_FETCH_LEDGER", &ledger_path)
        .env("AROS_TOOLCHAIN_FETCH_UPSTREAM", &upstream)
        .env("AROS_BRIDGE_MARKER", &marker)
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
    assert_eq!(
        fs::read_to_string(marker).unwrap(),
        format!("-a llvm-11.0.0.src -s tar.xz -l {}\n", cache.display())
    );
}

fn write_executable(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}
