#![cfg(unix)]

use std::fs::{self, File};
use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::process::{Command, Output};

use aros_common::{sha256_bytes, sha256_file, Sha256Digest};
use aros_fetch::contract::{Cli, FetchRequest};
use aros_fetch::engine::source_receipt::verify_prepared_source;
use clap::Parser;
use flate2::write::GzEncoder;
use flate2::Compression;

const PATCH_BYTES: &[u8] = b"--- a/hello.txt\n+++ b/hello.txt\n@@ -1 +1 @@\n-old\n+new\n";

struct Fixture {
    root: tempfile::TempDir,
    archive_sha256: String,
    patch_sha256: String,
}

impl Fixture {
    fn new() -> Self {
        Self::with_namespace_payload(false)
    }

    fn with_namespace_payload(include_namespace_payload: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let origin = root.path().join("origin");
        let destination = root.path().join("destination");
        let cache = root.path().join("cache");
        fs::create_dir(&origin).unwrap();
        fs::create_dir(&destination).unwrap();
        fs::create_dir(&cache).unwrap();

        let archive_path = origin.join("fixture.tar.gz");
        let mut archive = tar::Builder::new(GzEncoder::new(
            File::create(&archive_path).unwrap(),
            Compression::default(),
        ));
        let source = b"old\n";
        let mut header = tar::Header::new_gnu();
        header.set_size(source.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        archive
            .append_data(&mut header, "source/sub/hello.txt", &source[..])
            .unwrap();
        if include_namespace_payload {
            let payload = b"extra namespace payload\n";
            header.set_size(payload.len() as u64);
            header.set_cksum();
            archive
                .append_data(&mut header, ".aros-fetch/unbound.txt", &payload[..])
                .unwrap();
        }
        archive.into_inner().unwrap().finish().unwrap();

        fs::write(origin.join("fix.patch"), PATCH_BYTES).unwrap();
        let archive_sha256 = sha256_file(&archive_path).unwrap().digest.to_string();
        let patch_sha256 = sha256_file(&origin.join("fix.patch"))
            .unwrap()
            .digest
            .to_string();
        Self {
            root,
            archive_sha256,
            patch_sha256,
        }
    }

    fn origin(&self) -> PathBuf {
        self.root.path().join("origin")
    }

    fn destination(&self) -> PathBuf {
        self.root.path().join("destination")
    }

    fn cli_args(&self) -> Vec<String> {
        vec![
            "aros-fetch".to_owned(),
            "--archive".to_owned(),
            "fixture".to_owned(),
            "--suffixes".to_owned(),
            "tar.gz".to_owned(),
            "--archive-origins".to_owned(),
            self.origin().to_string_lossy().into_owned(),
            "--patch-origins".to_owned(),
            self.origin().to_string_lossy().into_owned(),
            "--patches".to_owned(),
            "fix.patch:source/sub:-f,-p1".to_owned(),
            "--checksums".to_owned(),
            format!(
                "fixture.tar.gz=sha256:{} fix.patch=sha256:{}",
                self.archive_sha256, self.patch_sha256
            ),
            "--location".to_owned(),
            self.root
                .path()
                .join("cache")
                .to_string_lossy()
                .into_owned(),
            "--destination".to_owned(),
            self.destination().to_string_lossy().into_owned(),
            "--base".to_owned(),
            self.destination().to_string_lossy().into_owned(),
            "--offline".to_owned(),
            "--require-checksums".to_owned(),
        ]
    }

    fn request(&self) -> FetchRequest {
        let cli = Cli::try_parse_from(self.cli_args()).unwrap();
        FetchRequest::from_cli(&cli).unwrap()
    }

    fn run(&self) -> Output {
        Command::new(env!("CARGO_BIN_EXE_aros-fetch"))
            .args(self.cli_args().into_iter().skip(1))
            .output()
            .unwrap()
    }

    fn fetch_and_verify(&self) -> aros_fetch::engine::source_receipt::VerifiedSourceReceipt {
        let output = self.run();
        assert!(
            output.status.success(),
            "stdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let request = self.request();
        verify_prepared_source(&request).unwrap()
    }
}

#[test]
fn subprocess_receipt_binds_archive_direct_patch_options_and_live_tree() {
    let fixture = Fixture::new();
    let verified = fixture.fetch_and_verify();
    assert_eq!(
        fs::read(fixture.destination().join("source/sub/hello.txt")).unwrap(),
        b"new\n"
    );
    assert_eq!(
        verified.destination(),
        fixture.destination().canonicalize().unwrap()
    );
    assert_eq!(verified.request().patches[0].name, "fix.patch");
    assert_eq!(verified.request().patches[0].options, ["-f", "-p1"]);
    assert_eq!(verified.receipt_sha256().as_str().len(), 64);
    assert_eq!(verified.payload_tree_sha256().as_str().len(), 64);
    verified.revalidate().unwrap();
}

#[test]
fn revalidation_rejects_changed_payload_and_replaced_receipt_bytes() {
    let fixture = Fixture::new();
    let verified = fixture.fetch_and_verify();
    fs::write(
        fixture.destination().join("source/sub/hello.txt"),
        b"tampered\n",
    )
    .unwrap();
    assert!(verified.revalidate().is_err());

    let fixture = Fixture::new();
    let verified = fixture.fetch_and_verify();
    let receipt = verified.receipt_path();
    let mut changed_bytes = fs::read(receipt).unwrap();
    changed_bytes.push(b' ');
    let replacement = receipt.with_extension("replacement");
    fs::write(&replacement, changed_bytes).unwrap();
    fs::rename(&replacement, receipt).unwrap();
    assert!(verified.revalidate().is_err());
}

#[test]
fn revalidation_binds_the_complete_receipt_namespace() {
    let fixture = Fixture::with_namespace_payload(true);
    let refused = fixture.run();
    assert!(!refused.status.success());
    assert!(!fixture
        .destination()
        .join(".aros-fetch/unbound.txt")
        .exists());

    let fixture = Fixture::new();
    let verified = fixture.fetch_and_verify();
    fs::write(
        fixture.destination().join(".aros-fetch/added.txt"),
        b"added after verification",
    )
    .unwrap();
    assert!(verified.revalidate().is_err());
}

#[test]
fn verification_rejects_wrong_patch_options_hash_fallback_and_symlink_receipt() {
    let fixture = Fixture::new();
    let verified = fixture.fetch_and_verify();
    let mut wrong_options = fixture.request();
    wrong_options.patches[0].options = vec!["-p1".to_owned(), "-N".to_owned()];
    assert!(verify_prepared_source(&wrong_options).is_err());

    let mut wrong_hash = fixture.request();
    wrong_hash
        .checksums
        .insert("fix.patch".to_owned(), sha256_bytes(b"a different patch"));
    assert!(verify_prepared_source(&wrong_hash).is_err());

    let mut fallback = fixture.request();
    fallback.checksums.insert(
        "fix.patch.tar.gz".to_owned(),
        Sha256Digest::parse(&fixture.patch_sha256).unwrap(),
    );
    assert!(verify_prepared_source(&fallback).is_err());

    let mut archive_fallback = fixture.request();
    archive_fallback
        .archive_candidates
        .push("fixture.zip".to_owned());
    archive_fallback.checksums.insert(
        "fixture.zip".to_owned(),
        Sha256Digest::parse(&fixture.archive_sha256).unwrap(),
    );
    assert!(verify_prepared_source(&archive_fallback).is_err());

    let receipt = verified.receipt_path();
    let saved = receipt.with_extension("saved");
    fs::rename(receipt, &saved).unwrap();
    symlink(&saved, receipt).unwrap();
    assert!(verified.revalidate().is_err());
}

#[test]
#[ignore = "requires explicitly prepared hash-locked vendor archives; read-only, no hardware"]
fn actual_vendor_archive_receipts_bind_their_complete_live_trees() {
    let root = PathBuf::from(std::env::var_os("AROS_TEST_VENDOR_SOURCE_PARENT").unwrap())
        .canonicalize()
        .unwrap();
    let cache = PathBuf::from(std::env::var_os("AROS_TEST_VENDOR_ARCHIVE_CACHE").unwrap())
        .canonicalize()
        .unwrap();
    for (role, archive, suffix, checksum) in [
        (
            "compiler",
            "riscv32-esp-elf-15.2.0_20251204-aarch64-apple-darwin",
            "tar.xz",
            "0869d1083532c631808543dd802885f02dbe1bb3bd640be0dee827e82ded768d",
        ),
        (
            "cmake",
            "cmake-4.0.3-macos-universal",
            "tar.gz",
            "4e85de4daf1c3e82d7dc6b8ba5683972944b466343aeb9c327a742437bb3ce9a",
        ),
        (
            "ninja",
            "ninja-mac",
            "zip",
            "89a287444b5b3e98f88a945afa50ce937b8ffd1dcc59c555ad9b1baf855298c9",
        ),
    ] {
        let destination = root.join(role);
        let arguments = vec![
            "aros-fetch".to_owned(),
            "--archive".into(),
            archive.into(),
            "--suffixes".into(),
            suffix.into(),
            "--archive-origins".into(),
            cache.to_str().unwrap().into(),
            "--destination".into(),
            destination.to_str().unwrap().into(),
            "--base".into(),
            destination.to_str().unwrap().into(),
            "--location".into(),
            cache.to_str().unwrap().into(),
            "--checksums".into(),
            format!("{archive}.{suffix}=sha256:{checksum}"),
            "--offline".into(),
            "--require-checksums".into(),
        ];
        let request = FetchRequest::from_cli(&Cli::try_parse_from(arguments).unwrap()).unwrap();
        let verified = verify_prepared_source(&request).unwrap();
        verified.revalidate().unwrap();
        println!(
            "{role}: receipt {} tree {}",
            verified.receipt_sha256(),
            verified.payload_tree_sha256()
        );
    }
}
