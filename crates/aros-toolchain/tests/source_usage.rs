use std::fs;

use aros_common::{sha256_bytes, DiagnosticCode};
use aros_toolchain::{source_lock::SourceLock, source_usage};
use serde_json::json;

fn lock() -> SourceLock {
    let document = json!({
        "schema": "aros-toolchain-source-lock-v2", "family": "llvm", "version": "11.0.0",
        "sources": [{
            "component": "llvm", "version": "11.0.0", "purpose": "toolchain-component",
            "filename": "llvm.tar.xz", "url": "https://example.invalid/llvm.tar.xz",
            "sha256": sha256_bytes(b"llvm"), "size": 4
        }, {
            "component": "clang", "version": "11.0.0", "purpose": "toolchain-component",
            "filename": "clang.tar.xz", "url": "https://example.invalid/clang.tar.xz",
            "sha256": sha256_bytes(b"clang"), "size": 5
        }],
        "host_python_packages": [{
            "name": "mako", "version": "1.3.10", "filename": "mako.tar.gz",
            "url": "https://example.invalid/mako.tar.gz", "sha256": sha256_bytes(b"mako"), "size": 4,
            "source_root": "mako-1.3.10", "python_path": "."
        }]
    });
    SourceLock::parse(&serde_json::to_vec(&document).unwrap()).unwrap()
}

#[test]
fn source_usage_must_be_an_exact_set_not_a_subset() {
    let temporary = tempfile::tempdir().unwrap();
    let usage = temporary.path().join("usage.log");
    fs::write(&usage, "clang.tar.xz\nllvm.tar.xz\n").unwrap();
    source_usage::verify(&lock(), &usage).unwrap();
    fs::write(&usage, "llvm.tar.xz\n").unwrap();
    let error = source_usage::verify(&lock(), &usage).unwrap_err();
    assert_eq!(
        error.diagnostics().diagnostics[0].code,
        DiagnosticCode::ProducerSourceUse
    );
}

#[cfg(unix)]
#[test]
fn source_usage_never_follows_a_replaced_ledger_symlink() {
    use std::os::unix::fs::symlink;

    let temporary = tempfile::tempdir().unwrap();
    let target = temporary.path().join("target.log");
    let usage = temporary.path().join("usage.log");
    fs::write(&target, "clang.tar.xz\nllvm.tar.xz\n").unwrap();
    symlink(&target, &usage).unwrap();
    let error = source_usage::verify(&lock(), &usage).unwrap_err();
    assert_eq!(
        error.diagnostics().diagnostics[0].code,
        DiagnosticCode::ProducerSourceUse
    );
}
