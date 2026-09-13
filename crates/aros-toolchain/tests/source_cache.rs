//! M2 cache tests use only local synthetic bytes; they never perform transport.

use std::fs;

use aros_common::{sha256_bytes, DiagnosticCode};
use aros_toolchain::{
    source_cache,
    source_cache_request::{SourceCacheIntegrity, SourceCacheRequest},
};
use serde_json::json;

fn lock_bytes(bytes: &[u8]) -> Vec<u8> {
    let source = json!({
        "schema": "aros-toolchain-source-lock-v2", "family": "llvm", "version": "11.0.0",
        "sources": [{
            "component": "llvm", "version": "11.0.0", "purpose": "toolchain-component",
            "filename": "llvm.tar.xz", "url": "https://example.invalid/llvm.tar.xz",
            "sha256": sha256_bytes(bytes), "size": bytes.len()
        }],
        "host_python_packages": [{
            "name": "mako", "version": "1.3.10", "filename": "mako.tar.gz",
            "url": "https://example.invalid/mako.tar.gz", "sha256": sha256_bytes(bytes), "size": bytes.len(),
            "source_root": "mako-1.3.10", "python_path": "."
        }]
    });
    serde_json::to_vec(&source).unwrap()
}

fn request(bytes: &[u8]) -> SourceCacheRequest {
    SourceCacheRequest::from_source_lock(&lock_bytes(bytes)).unwrap()
}

fn real_tempdir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("aros-source-cache-integration-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .unwrap()
}

#[test]
fn accepts_exact_complete_local_cache_entries() {
    let temporary = real_tempdir();
    let bytes = b"verified source fixture\n";
    fs::write(temporary.path().join("llvm.tar.xz"), bytes).unwrap();
    fs::write(temporary.path().join("mako.tar.gz"), bytes).unwrap();
    let verified = source_cache::verify_request(temporary.path(), &request(bytes)).unwrap();
    assert_eq!(verified.entries.len(), 2);
    assert_eq!(verified.entries[0].filename, "mako.tar.gz");
    assert_eq!(verified.entries[1].filename, "llvm.tar.xz");
}

#[test]
fn missing_or_tampered_payload_fails_as_a_source_error() {
    let temporary = real_tempdir();
    let bytes = b"verified source fixture\n";
    fs::write(temporary.path().join("llvm.tar.xz"), bytes).unwrap();
    let missing = source_cache::verify_request(temporary.path(), &request(bytes)).unwrap_err();
    assert_eq!(
        missing.diagnostics().diagnostics[0].code,
        DiagnosticCode::ProducerSources
    );

    fs::write(temporary.path().join("mako.tar.gz"), b"changed\n").unwrap();
    let changed = source_cache::verify_request(temporary.path(), &request(bytes)).unwrap_err();
    assert_eq!(
        changed.diagnostics().diagnostics[0].code,
        DiagnosticCode::ProducerSources
    );
}

#[test]
fn a_locked_size_mismatch_is_rejected_without_treating_metadata_as_integrity() {
    let temporary = real_tempdir();
    let bytes = b"verified source fixture\n";
    fs::write(temporary.path().join("llvm.tar.xz"), bytes).unwrap();
    fs::write(temporary.path().join("mako.tar.gz"), bytes).unwrap();
    let mut changed_size = request(bytes);
    for entry in &mut changed_size.entries {
        let SourceCacheIntegrity::Locked { size, .. } = &mut entry.integrity else {
            panic!("native source-lock entries must be strict");
        };
        *size += 1;
    }

    let error = source_cache::verify_request(temporary.path(), &changed_size).unwrap_err();
    assert_eq!(
        error.diagnostics().diagnostics[0].code,
        DiagnosticCode::ProducerSources
    );
    assert!(error.to_string().contains("locked size or SHA-256"));
}

#[tokio::test]
async fn offline_acquisition_never_substitutes_an_unlocked_or_missing_payload() {
    let temporary = real_tempdir();
    let bytes = b"verified source fixture\n";
    fs::write(temporary.path().join("llvm.tar.xz"), bytes).unwrap();
    fs::write(temporary.path().join("mako.tar.gz"), bytes).unwrap();
    let verified = source_cache::fetch_request(temporary.path(), &request(bytes), true, false)
        .await
        .unwrap();
    assert_eq!(verified.entries.len(), 2);

    fs::remove_file(temporary.path().join("mako.tar.gz")).unwrap();
    let missing = source_cache::fetch_request(temporary.path(), &request(bytes), true, false)
        .await
        .unwrap_err();
    assert_eq!(
        missing.diagnostics().diagnostics[0].code,
        DiagnosticCode::ProducerSources
    );
}

#[tokio::test]
async fn poisoned_existing_cache_is_never_refreshed_from_the_network() {
    let temporary = real_tempdir();
    let bytes = b"verified source fixture\n";
    fs::write(temporary.path().join("llvm.tar.xz"), bytes).unwrap();
    fs::write(temporary.path().join("mako.tar.gz"), b"poisoned\n").unwrap();
    let error = source_cache::fetch_request(temporary.path(), &request(bytes), false, false)
        .await
        .unwrap_err();
    assert_eq!(
        error.diagnostics().diagnostics[0].code,
        DiagnosticCode::ProducerSources
    );
}
