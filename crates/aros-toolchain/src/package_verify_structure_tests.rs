#![cfg(unix)]

use super::*;
use crate::package::{package_with_format, PackageRequest};
use serde_json::{json, Map, Value};
use std::ffi::OsString;
use std::fs::File;
use std::io::{Cursor, Write};
use std::os::unix::ffi::OsStringExt as _;
use std::path::{Path, PathBuf};
use tar::{Builder, EntryType};
use tempfile::TempDir;

fn fixture_manifest() -> ArosToolchainManifest {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../scripts/fixtures/toolchain-producer/package-v1.json"
    ))
    .unwrap();
    serde_json::from_value(fixture["manifest"].clone()).unwrap()
}

fn canonical_header(path: &str, entry_type: EntryType, size: u64, mode: u32) -> tar::Header {
    let mut header = tar::Header::new_ustar();
    header.set_path(path).unwrap();
    header.set_entry_type(entry_type);
    header.set_size(size);
    header.set_mode(mode);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(fixture_manifest().source_date_epoch);
    header.set_cksum();
    header
}

#[derive(Clone, Copy)]
enum ParentShape {
    RealDirectory,
    Missing,
    RegularFile,
    Symlink,
}

fn archive_with_parent(shape: ParentShape) -> (TempDir, PathBuf) {
    let temporary = tempfile::tempdir().unwrap();
    let archive_path = temporary.path().join("shape.tar.xz");
    let output = File::create(&archive_path).unwrap();
    let encoder = xz2::write::XzEncoder::new(output, 6);
    let mut builder = Builder::new(encoder);
    builder
        .append(
            &canonical_header("toolchain", EntryType::Directory, 0, 0o755),
            &b""[..],
        )
        .unwrap();

    match shape {
        ParentShape::RealDirectory => {
            builder
                .append(
                    &canonical_header("toolchain/bin", EntryType::Directory, 0, 0o755),
                    &b""[..],
                )
                .unwrap();
        }
        ParentShape::Missing => {}
        ParentShape::RegularFile => {
            builder
                .append(
                    &canonical_header("toolchain/bin", EntryType::Regular, 4, 0o644),
                    &b"data"[..],
                )
                .unwrap();
        }
        ParentShape::Symlink => {
            let mut link = canonical_header("toolchain/bin", EntryType::Symlink, 0, 0o777);
            link.set_link_name(".").unwrap();
            link.set_cksum();
            builder.append(&link, &b""[..]).unwrap();
        }
    }

    builder
        .append(
            &canonical_header("toolchain/bin/clang", EntryType::Regular, 5, 0o755),
            &b"clang"[..],
        )
        .unwrap();
    builder
        .append(
            &canonical_header(
                "toolchain/toolchain-manifest.json",
                EntryType::Regular,
                2,
                0o644,
            ),
            &b"{}"[..],
        )
        .unwrap();
    builder.finish().unwrap();
    builder
        .into_inner()
        .unwrap()
        .finish()
        .unwrap()
        .flush()
        .unwrap();
    (temporary, archive_path)
}

fn assert_parent_rejected(shape: ParentShape) {
    let (_temporary, archive_path) = archive_with_parent(shape);
    let error = verify_archive_tree(File::open(&archive_path).unwrap(), &fixture_manifest(), &[])
        .err()
        .expect("archive with a non-directory or missing parent must be rejected");
    assert!(
        error.to_string().contains("parent"),
        "expected a parent-structure diagnostic, got: {error}"
    );
}

#[test]
fn archive_readback_accepts_a_real_parent_directory() {
    let (_temporary, archive_path) = archive_with_parent(ParentShape::RealDirectory);
    assert!(
        verify_archive_tree(File::open(&archive_path).unwrap(), &fixture_manifest(), &[],).is_ok()
    );
}

#[test]
fn archive_readback_rejects_missing_parent_directories() {
    assert_parent_rejected(ParentShape::Missing);
}

#[test]
fn archive_readback_rejects_regular_file_parents() {
    assert_parent_rejected(ParentShape::RegularFile);
}

#[test]
fn archive_readback_rejects_symlink_parents() {
    assert_parent_rejected(ParentShape::Symlink);
}

fn valid_verification_request(
    directory: &Path,
    forbidden_prefix: PathBuf,
) -> PackageVerificationRequest {
    let source_lock = serde_json::to_vec(&json!({
        "schema": "aros-toolchain-source-lock-v2",
        "family": "llvm",
        "version": "17.0.6",
        "sources": [{
            "component": "llvm",
            "version": "17.0.6",
            "purpose": "toolchain-component",
            "filename": "llvm-17.0.6.tar.xz",
            "url": "https://example.invalid/llvm-17.0.6.tar.xz",
            "sha256": "a".repeat(64),
            "size": 1
        }],
        "host_python_packages": [{
            "name": "wheel",
            "version": "0.43.0",
            "filename": "wheel-0.43.0.tar.gz",
            "url": "https://example.invalid/wheel-0.43.0.tar.gz",
            "sha256": "b".repeat(64),
            "size": 1,
            "source_root": "wheel-0.43.0",
            "python_path": "."
        }]
    }))
    .unwrap();
    let profiles = serde_json::to_vec(&json!({
        "schema": "aros-toolchain-profiles-v1",
        "upstream_commit": "a".repeat(40),
        "profiles": [{
            "name": "pc-x86_64",
            "configure_target": "pc-x86_64",
            "upstream_output_target": "pc-x86_64",
            "target_triple": "x86_64-unknown-aros",
            "cpu": "x86_64",
            "platform": "pc",
            "float_abi": "",
            "capabilities": ["c", "cxx", "standalone-collector"]
        }]
    }))
    .unwrap();
    let mut recipe_record = json!({
        "schema": "aros-toolchain-recipe-v2",
        "source_commit": "1".repeat(40),
        "source_tree": "2".repeat(40),
        "producer_commit": "3".repeat(40),
        "producer_tree": "4".repeat(40),
        "tools_commit": "5".repeat(40),
        "tools_tree": "6".repeat(40),
        "source_date_epoch": 0,
        "source_lock_sha256": aros_common::sha256_bytes(&source_lock),
        "profiles_sha256": aros_common::sha256_bytes(&profiles),
        "patches": []
    });
    recipe_record["recipe_sha256"] = json!(aros_common::sha256_bytes(
        &crate::canonical::bytes(&recipe_record).unwrap()
    ));
    let recipe = Recipe::parse(&serde_json::to_vec(&recipe_record).unwrap()).unwrap();
    let source_lock = crate::source_lock::SourceLock::parse(&source_lock).unwrap();
    let profile = crate::profiles::Profiles::parse(&profiles)
        .unwrap()
        .select("pc-x86_64")
        .unwrap()
        .clone();

    PackageVerificationRequest {
        package_dir: directory.to_path_buf(),
        release_id: "verification-test".into(),
        host: "macos-aarch64".into(),
        recipe,
        source_lock,
        profile,
        build_environment: Map::new(),
        forbidden_prefixes: vec![forbidden_prefix],
    }
}

#[test]
fn utf8_forbidden_prefix_is_scanned_across_buffer_boundaries() {
    let prefix = PathBuf::from("/tmp/valid-build-root");
    let mut bytes = vec![b'x'; SCAN_BUFFER_BYTES - 3];
    bytes.extend_from_slice(prefix.to_str().unwrap().as_bytes());
    let error = hash_entry(
        &mut Cursor::new(bytes.as_slice()),
        bytes.len() as u64,
        &[prefix],
    )
    .expect_err("UTF-8 forbidden prefix must be detected");
    assert!(error.to_string().contains("forbidden build prefix"));
}

#[test]
fn non_utf8_absolute_forbidden_prefix_is_rejected_at_request_validation() {
    let temporary = tempfile::tempdir().unwrap();
    let valid =
        valid_verification_request(temporary.path(), PathBuf::from("/tmp/valid-build-root"));
    assert!(validate_request(&valid, PackageFormat::CompilerFamilyV2).is_ok());

    let invalid_prefix = PathBuf::from(OsString::from_vec(b"/tmp/build-root-\xff".to_vec()));
    let invalid = valid_verification_request(temporary.path(), invalid_prefix);
    let error = validate_request(&invalid, PackageFormat::CompilerFamilyV2)
        .expect_err("absolute non-UTF-8 build root must not be silently ignored");
    assert!(
        error.to_string().to_ascii_lowercase().contains("utf"),
        "expected a UTF-8 prefix diagnostic, got: {error}"
    );
}

#[test]
fn package_producer_rejects_non_utf8_prefix_before_candidate_mutation() {
    let valid_root = tempfile::tempdir().unwrap();
    let valid_verification =
        valid_verification_request(valid_root.path(), PathBuf::from("/tmp/valid-build-root"));
    let valid_candidate = valid_root.path().join("candidate");
    std::fs::create_dir(&valid_candidate).unwrap();
    std::fs::write(valid_candidate.join("clang"), b"synthetic compiler payload").unwrap();
    let valid_output = valid_root.path().join("package-output");
    let valid_request = PackageRequest {
        candidate_root: valid_candidate,
        output_dir: valid_output.clone(),
        release_id: valid_verification.release_id,
        host: valid_verification.host,
        recipe: valid_verification.recipe,
        source_lock: valid_verification.source_lock,
        profile: valid_verification.profile,
        build_environment: valid_verification.build_environment,
        forbidden_prefixes: valid_verification.forbidden_prefixes,
    };
    assert!(package_with_format(&valid_request, PackageFormat::CompilerFamilyV2).is_ok());
    assert!(valid_output.is_dir());

    let invalid_root = tempfile::tempdir().unwrap();
    let invalid_verification = valid_verification_request(
        invalid_root.path(),
        PathBuf::from(OsString::from_vec(b"/tmp/build-root-\xff".to_vec())),
    );
    let invalid_candidate = invalid_root.path().join("candidate");
    std::fs::create_dir(&invalid_candidate).unwrap();
    let candidate_file = invalid_candidate.join("clang");
    let candidate_bytes = b"synthetic compiler payload";
    std::fs::write(&candidate_file, candidate_bytes).unwrap();
    let invalid_output = invalid_root.path().join("package-output");
    let invalid_request = PackageRequest {
        candidate_root: invalid_candidate.clone(),
        output_dir: invalid_output.clone(),
        release_id: invalid_verification.release_id,
        host: invalid_verification.host,
        recipe: invalid_verification.recipe,
        source_lock: invalid_verification.source_lock,
        profile: invalid_verification.profile,
        build_environment: invalid_verification.build_environment,
        forbidden_prefixes: invalid_verification.forbidden_prefixes,
    };
    let error = package_with_format(&invalid_request, PackageFormat::CompilerFamilyV2)
        .expect_err("non-UTF-8 package prefix must fail before output creation");
    assert!(error.to_string().to_ascii_lowercase().contains("utf"));
    assert!(!invalid_output.exists());
    assert_eq!(std::fs::read(&candidate_file).unwrap(), candidate_bytes);
    assert_eq!(
        std::fs::read_dir(&invalid_candidate).unwrap().count(),
        1,
        "candidate tree must remain unchanged"
    );
}
