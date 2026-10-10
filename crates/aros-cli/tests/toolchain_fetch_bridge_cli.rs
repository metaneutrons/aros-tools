//! The hidden MetaMake bridge executes the validated Rust fetch engine only.

#![cfg(unix)]

use std::fs;
use std::io::{Cursor, Write};
use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::process::{Command, Output};

use aros_common::sha256_bytes;
use aros_toolchain::metamake_fetch::SourceUseLedger;
use serde_json::json;
use tar::{Builder, Header};
use tempfile::TempDir;
use xz2::write::XzEncoder;

struct Fixture {
    temporary: TempDir,
    source_root: PathBuf,
    work: PathBuf,
    cache: PathBuf,
    lock: PathBuf,
    ledger: PathBuf,
    marker: PathBuf,
    output: PathBuf,
    fetch_script: PathBuf,
}

impl Fixture {
    fn new(payload: &[u8]) -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let source_root = temporary.path().join("source");
        let work = source_root.join("work");
        let patches = source_root.join("patches");
        fs::create_dir_all(&work).unwrap();
        fs::create_dir_all(&patches).unwrap();
        let fetch_script = source_root.join("scripts/fetch.sh");
        fs::create_dir_all(fetch_script.parent().unwrap()).unwrap();
        fs::write(
            &fetch_script,
            "#!/bin/sh\ntouch \"$AROS_SCRIPT_MARKER\"\nexit 91\n",
        )
        .unwrap();

        let cache = temporary.path().join("cache");
        fs::create_dir(&cache).unwrap();
        fs::write(cache.join("gcc.tar.xz"), payload).unwrap();
        fs::write(cache.join("gcc.tar.gz"), b"ambient wrong suffix candidate").unwrap();

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
        let marker = temporary.path().join("source-script-ran");
        let output = source_root.join("extracted");
        Self {
            temporary,
            source_root,
            work,
            cache,
            lock,
            ledger,
            marker,
            output,
            fetch_script,
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_aros"));
        command
            .current_dir(&self.work)
            .env("AROS_TOOLCHAIN_FETCH_LOCK", &self.lock)
            .env("AROS_TOOLCHAIN_FETCH_CACHE", &self.cache)
            .env("AROS_TOOLCHAIN_FETCH_LEDGER", &self.ledger)
            .env("AROS_TOOLCHAIN_FETCH_UPSTREAM", &self.fetch_script)
            .env("AROS_SCRIPT_MARKER", &self.marker)
            .env("AROS_FETCH_OFFLINE", "false")
            .env("AROS_FETCH_REQUIRE_CHECKSUMS", "false")
            .env("AROS_FETCH_LOG_LEVEL", "debug")
            .env(
                "AROS_FETCH_LOG_FILE",
                self.temporary.path().join("ambient.log"),
            )
            .args(["toolchain", "__metamake-fetch"])
            .args(args);
        command
    }

    fn base_arguments(&self) -> Vec<String> {
        vec![
            "-ao".into(),
            "https://archive.invalid/unused".into(),
            "-a".into(),
            "gcc".into(),
            "-s".into(),
            "tar.gz tar.xz tar.bz2".into(),
            "-l".into(),
            self.cache.display().to_string(),
            "-d".into(),
            self.output.display().to_string(),
        ]
    }
}

#[test]
fn bridge_extracts_locked_tar_xz_applies_contained_patch_and_never_runs_fetch_sh() {
    let payload = tar_xz("hello.txt", b"before\n");
    let fixture = Fixture::new(&payload);
    fs::write(
        fixture.source_root.join("patches/change.patch"),
        b"--- hello.txt\n+++ hello.txt\n@@ -1 +1 @@\n-before\n+after\n",
    )
    .unwrap();
    let mut args = fixture.base_arguments();
    args.extend([
        "-po".into(),
        "../patches".into(),
        "-p".into(),
        "change.patch".into(),
    ]);
    let args = args.iter().map(String::as_str).collect::<Vec<_>>();
    let output = fixture.command(&args).output().unwrap();
    assert_success(&output);
    assert_eq!(
        fs::read(fixture.output.join("hello.txt")).unwrap(),
        b"after\n"
    );
    assert_eq!(fs::read_to_string(&fixture.ledger).unwrap(), "gcc.tar.xz\n");
    assert_eq!(
        fs::read(fixture.cache.join("gcc.tar.gz")).unwrap(),
        b"ambient wrong suffix candidate"
    );
    assert!(!fixture.marker.exists(), "upstream fetch.sh was executed");
    assert!(
        !fixture.temporary.path().join("ambient.log").exists(),
        "ambient logging settings must not enable fetch logs"
    );
}

#[test]
fn corrupt_locked_payload_fails_before_source_output_or_ledger_record() {
    let payload = tar_xz("hello.txt", b"valid locked payload\n");
    let fixture = Fixture::new(&payload);
    fs::write(fixture.cache.join("gcc.tar.xz"), b"corrupt bytes").unwrap();
    let args = fixture.base_arguments();
    let args = args.iter().map(String::as_str).collect::<Vec<_>>();
    let output = fixture.command(&args).output().unwrap();
    assert!(!output.status.success());
    assert!(!fixture.output.exists());
    assert_eq!(fs::read_to_string(&fixture.ledger).unwrap(), "");
    assert!(!fixture.marker.exists());
    assert_eq!(
        fs::read(fixture.cache.join("gcc.tar.gz")).unwrap(),
        b"ambient wrong suffix candidate"
    );
}

#[test]
fn bridge_accepts_empty_checksum_placeholders_and_the_exact_locked_checksum() {
    let payload = tar_xz("hello.txt", b"locked payload\n");
    let locked_checksum = format!("gcc.tar.xz=sha256:{}", sha256_bytes(&payload));
    let checksum_arguments = [
        vec!["-cs".into(), String::new()],
        vec!["-cs".into(), " \t\r\n".into()],
        vec!["--checksums".into(), String::new()],
        vec!["--checksums".into(), " \t".into()],
        vec!["--checksums=".into()],
        vec!["--checksums".into(), locked_checksum],
    ];

    for checksum_arguments in checksum_arguments {
        let fixture = Fixture::new(&payload);
        let mut args = fixture.base_arguments();
        args.extend(checksum_arguments);
        let args = args.iter().map(String::as_str).collect::<Vec<_>>();
        let output = fixture.command(&args).output().unwrap();
        assert_success(&output);
        assert_eq!(
            fs::read(fixture.output.join("hello.txt")).unwrap(),
            b"locked payload\n"
        );
        assert_eq!(fs::read_to_string(&fixture.ledger).unwrap(), "gcc.tar.xz\n");
        assert!(!fixture.marker.exists(), "upstream fetch.sh was executed");
    }
}

#[test]
fn corrupt_locked_payload_with_empty_checksum_placeholder_fails_closed() {
    let payload = tar_xz("hello.txt", b"valid locked payload\n");
    let fixture = Fixture::new(&payload);
    fs::write(fixture.cache.join("gcc.tar.xz"), b"corrupt bytes").unwrap();
    let mut args = fixture.base_arguments();
    args.extend(["-cs".into(), " \t\n".into()]);
    let args = args.iter().map(String::as_str).collect::<Vec<_>>();
    let output = fixture.command(&args).output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("selected a missing, unsafe or changed locked source archive"));
    assert!(!fixture.output.exists());
    assert_eq!(fs::read_to_string(&fixture.ledger).unwrap(), "");
    assert!(!fixture.marker.exists());
}

#[test]
fn bridge_rejects_duplicate_empty_or_mixed_checksum_options() {
    let payload = tar_xz("hello.txt", b"locked payload\n");
    let locked_checksum = format!("gcc.tar.xz=sha256:{}", sha256_bytes(&payload));
    let invalid = [
        vec!["-cs".into(), String::new(), "--checksums=".into()],
        vec!["-cs".into(), " \t".into(), "-cs".into(), locked_checksum],
        vec!["--checksums=".into(), "--checksums".into(), String::new()],
    ];

    for invalid in invalid {
        let fixture = Fixture::new(&payload);
        let mut args = fixture.base_arguments();
        args.extend(invalid);
        let args = args.iter().map(String::as_str).collect::<Vec<_>>();
        let output = fixture.command(&args).output().unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr)
            .contains("repeated -cs/--checksums options are not allowed"));
        assert!(!fixture.output.exists());
        assert_eq!(fs::read_to_string(&fixture.ledger).unwrap(), "");
        assert!(!fixture.marker.exists());
    }
}

#[test]
fn bridge_rejects_nonempty_checksums_that_are_not_one_exact_lock_entry() {
    let payload = tar_xz("hello.txt", b"locked payload\n");
    let digest = sha256_bytes(&payload).to_string();
    let locked_checksum = format!("gcc.tar.xz=sha256:{digest}");
    let invalid = [
        format!("other.tar.xz=sha256:{digest}"),
        format!("{locked_checksum} other.tar.xz=sha256:{digest}"),
        "gcc.tar.xz=sha256:not-a-digest".into(),
        format!("\u{2003}{locked_checksum}\u{2003}"),
    ];

    for invalid in invalid {
        let fixture = Fixture::new(&payload);
        let mut args = fixture.base_arguments();
        args.extend(["--checksums".into(), invalid]);
        let args = args.iter().map(String::as_str).collect::<Vec<_>>();
        let output = fixture.command(&args).output().unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr)
            .contains("source checksum differs from the selected source lock"));
        assert!(!fixture.output.exists());
        assert_eq!(fs::read_to_string(&fixture.ledger).unwrap(), "");
        assert!(!fixture.marker.exists());
    }
}

#[test]
fn bridge_rejects_remote_and_escaping_patch_origins_even_offline() {
    let payload = tar_xz("hello.txt", b"before\n");
    for (index, patch_origin) in ["https://patches.invalid", "../../outside-patches"]
        .into_iter()
        .enumerate()
    {
        let fixture = Fixture::new(&payload);
        let outside = fixture.temporary.path().join("outside-patches");
        fs::create_dir(&outside).unwrap();
        let ledger = fixture.temporary.path().join(format!("usage-{index}.log"));
        SourceUseLedger::create(&ledger).unwrap();
        let mut args = fixture.base_arguments();
        args.extend([
            "-po".into(),
            patch_origin.into(),
            "-p".into(),
            "missing.patch".into(),
        ]);
        let args = args.iter().map(String::as_str).collect::<Vec<_>>();
        let output = fixture
            .command(&args)
            .env("AROS_TOOLCHAIN_FETCH_LEDGER", &ledger)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert_eq!(fs::read_to_string(ledger).unwrap(), "");
        assert!(!fixture.output.exists());
        assert!(!fixture.marker.exists());
        assert!(String::from_utf8_lossy(&output.stderr).contains("patch origin"));
    }
}

#[test]
fn bridge_rejects_patch_file_symlink_that_escapes_source_snapshot() {
    let payload = tar_xz("hello.txt", b"before\n");
    let fixture = Fixture::new(&payload);
    let external_patch = fixture.temporary.path().join("external.patch");
    fs::write(
        &external_patch,
        b"--- hello.txt\n+++ hello.txt\n@@ -1 +1 @@\n-before\n+after\n",
    )
    .unwrap();
    symlink(
        &external_patch,
        fixture.source_root.join("patches/change.patch"),
    )
    .unwrap();
    let mut args = fixture.base_arguments();
    args.extend([
        "-po".into(),
        "../patches".into(),
        "-p".into(),
        "change.patch".into(),
    ]);
    let args = args.iter().map(String::as_str).collect::<Vec<_>>();
    let output = fixture.command(&args).output().unwrap();
    assert!(!output.status.success());
    assert_eq!(fs::read_to_string(&fixture.ledger).unwrap(), "");
    assert!(!fixture.output.join("hello.txt").exists());
    assert!(!fixture.marker.exists());
}

#[test]
fn mandatory_policy_bypasses_and_duplicate_or_conflicting_lock_checksums_are_rejected() {
    let payload = tar_xz("hello.txt", b"before\n");
    for invalid in [
        vec!["--offline=false"],
        vec!["--offline"],
        vec!["--require-checksums=false"],
        vec!["--require-checksums"],
        vec![
            "-cs",
            "gcc.tar.xz=sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "-cs",
            "gcc.tar.xz=sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ],
        vec![
            "-cs",
            "gcc.tar.xz=sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ],
    ] {
        let fixture = Fixture::new(&payload);
        let mut args = fixture.base_arguments();
        args.extend(invalid.iter().map(|value| (*value).into()));
        let args = args.iter().map(String::as_str).collect::<Vec<_>>();
        let output = fixture.command(&args).output().unwrap();
        assert!(!output.status.success(), "accepted {invalid:?}");
        assert_eq!(fs::read_to_string(&fixture.ledger).unwrap(), "");
        assert!(!fixture.output.exists());
        assert!(!fixture.marker.exists());
    }
}

#[test]
fn gnu_suffix_list_resolves_only_the_lock_candidate_and_preserves_ambient_decoy() {
    let payload = tar_xz("hello.txt", b"locked\n");
    let fixture = Fixture::new(&payload);
    let mut args = fixture.base_arguments();
    let suffixes = args
        .iter_mut()
        .find(|argument| *argument == "tar.gz tar.xz tar.bz2")
        .unwrap();
    *suffixes = "tar.bz2 tar.gz tar.xz".into();
    let args = args.iter().map(String::as_str).collect::<Vec<_>>();
    let output = fixture.command(&args).output().unwrap();
    assert_success(&output);
    assert_eq!(
        fs::read(fixture.output.join("hello.txt")).unwrap(),
        b"locked\n"
    );
    assert_eq!(fs::read_to_string(&fixture.ledger).unwrap(), "gcc.tar.xz\n");
    assert_eq!(
        fs::read(fixture.cache.join("gcc.tar.gz")).unwrap(),
        b"ambient wrong suffix candidate"
    );
    assert!(!fixture.marker.exists());
}

fn tar_xz(path: &str, contents: &[u8]) -> Vec<u8> {
    let mut archive = Builder::new(Vec::new());
    let mut header = Header::new_gnu();
    header.set_path(path).unwrap();
    header.set_size(contents.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    archive
        .append_data(&mut header, path, Cursor::new(contents))
        .unwrap();
    let tar = archive.into_inner().unwrap();
    let mut encoder = XzEncoder::new(Vec::new(), 6);
    encoder.write_all(&tar).unwrap();
    encoder.finish().unwrap()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
