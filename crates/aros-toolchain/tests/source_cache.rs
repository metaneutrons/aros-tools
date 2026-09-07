//! M2 cache tests use only local synthetic bytes; they never perform transport.

use std::fs;

use aros_common::{sha256_bytes, DiagnosticCode};
use aros_toolchain::{source_cache, source_lock::SourceLock};
use serde_json::json;

fn lock(bytes: &[u8]) -> SourceLock {
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
    SourceLock::parse(&serde_json::to_vec(&source).unwrap()).unwrap()
}

#[test]
fn accepts_exact_complete_local_cache_entries() {
    let temporary = tempfile::tempdir().unwrap();
    let bytes = b"verified source fixture\n";
    fs::write(temporary.path().join("llvm.tar.xz"), bytes).unwrap();
    fs::write(temporary.path().join("mako.tar.gz"), bytes).unwrap();
    let verified = source_cache::verify(temporary.path(), &lock(bytes)).unwrap();
    assert_eq!(verified.payloads.len(), 2);
    assert_eq!(verified.payloads[0].filename, "llvm.tar.xz");
    assert_eq!(verified.payloads[1].filename, "mako.tar.gz");
}

#[test]
fn missing_or_tampered_payload_fails_as_a_source_error() {
    let temporary = tempfile::tempdir().unwrap();
    let bytes = b"verified source fixture\n";
    fs::write(temporary.path().join("llvm.tar.xz"), bytes).unwrap();
    let missing = source_cache::verify(temporary.path(), &lock(bytes)).unwrap_err();
    assert_eq!(
        missing.diagnostics().diagnostics[0].code,
        DiagnosticCode::ProducerSources
    );

    fs::write(temporary.path().join("mako.tar.gz"), b"changed\n").unwrap();
    let changed = source_cache::verify(temporary.path(), &lock(bytes)).unwrap_err();
    assert_eq!(
        changed.diagnostics().diagnostics[0].code,
        DiagnosticCode::ProducerSources
    );
}

#[tokio::test]
async fn offline_acquisition_never_substitutes_an_unlocked_or_missing_payload() {
    let temporary = tempfile::tempdir().unwrap();
    let bytes = b"verified source fixture\n";
    fs::write(temporary.path().join("llvm.tar.xz"), bytes).unwrap();
    fs::write(temporary.path().join("mako.tar.gz"), bytes).unwrap();
    let verified = source_cache::acquire(temporary.path(), &lock(bytes), true)
        .await
        .unwrap();
    assert_eq!(verified.payloads.len(), 2);

    fs::remove_file(temporary.path().join("mako.tar.gz")).unwrap();
    let missing = source_cache::acquire(temporary.path(), &lock(bytes), true)
        .await
        .unwrap_err();
    assert_eq!(
        missing.diagnostics().diagnostics[0].code,
        DiagnosticCode::ProducerSources
    );
}

#[tokio::test]
async fn poisoned_existing_cache_is_never_refreshed_from_the_network() {
    let temporary = tempfile::tempdir().unwrap();
    let bytes = b"verified source fixture\n";
    fs::write(temporary.path().join("llvm.tar.xz"), bytes).unwrap();
    fs::write(temporary.path().join("mako.tar.gz"), b"poisoned\n").unwrap();
    let error = source_cache::acquire(temporary.path(), &lock(bytes), false)
        .await
        .unwrap_err();
    assert_eq!(
        error.diagnostics().diagnostics[0].code,
        DiagnosticCode::ProducerSources
    );
}
