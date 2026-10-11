#![cfg(unix)]

//! Bounded acquisition and verification of one explicitly locked SDK release.
//!
//! This adapter uses the exact-byte HTTPS cache primitive. The lock establishes
//! selected byte identities only; it does not authenticate a publisher or
//! authorize a release.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use aros_common::sdk_release_lock::SdkReleaseLock;
use aros_common::{ensure_directory_nofollow, validate_existing_directory_prefix_nofollow};
use aros_fetch::engine::cache::{
    acquire_https_cache_payload, snapshot_verified_cache_payload, VerifiedCachePayload,
};
use aros_toolchain::sdk_package::SdkLockedArchiveRequest;
use miette::{bail, IntoDiagnostic, Result, WrapErr};

/// Two lock-bound SDK package members retained with verified private snapshots.
pub struct AcquiredSdk {
    archive: VerifiedCachePayload,
    manifest: VerifiedCachePayload,
    cache_namespace: PathBuf,
    /// Persistent cache path for the locked archive.
    pub(super) cache_archive: PathBuf,
    /// Persistent cache path for the locked external manifest.
    pub(super) cache_manifest: PathBuf,
}

impl AcquiredSdk {
    /// Path to the private verified archive snapshot used by installation.
    pub(super) fn archive_path(&self) -> &Path {
        self.archive.path()
    }

    /// Recheck both cache objects and snapshots before a later consuming step.
    pub(super) fn revalidate(&self) -> Result<()> {
        validate_existing_directory_prefix_nofollow(&self.cache_namespace)
            .into_diagnostic()
            .wrap_err("SDK release cache namespace is no longer a safe directory")?;
        require_real_directory(&self.cache_namespace, "SDK release cache namespace")?;
        self.archive
            .revalidate()
            .map_err(|error| miette::miette!("locked SDK archive cache entry changed: {error}"))?;
        self.manifest
            .revalidate()
            .map_err(|error| miette::miette!("locked SDK manifest cache entry changed: {error}"))?;
        validate_existing_directory_prefix_nofollow(&self.cache_namespace)
            .into_diagnostic()
            .wrap_err("SDK release cache namespace changed during revalidation")?;
        require_real_directory(&self.cache_namespace, "SDK release cache namespace")
    }
}

/// Acquire and fully verify the archive and external manifest selected by `lock`.
///
/// `cache` is an explicit absolute cache parent. The per-lock namespace uses
/// the SHA-256 of the canonical lock bytes, so releases remain host-independent
/// and distinct lock documents cannot alias one another.
pub async fn prepare(lock: &SdkReleaseLock, cache: &Path, offline: bool) -> Result<AcquiredSdk> {
    lock.validate()
        .map_err(|error| miette::miette!("invalid SDK release lock: {error}"))?;
    if !cache.is_absolute() {
        bail!("SDK release cache parent must be an absolute path");
    }
    if cache.file_name().is_none() {
        bail!("SDK release cache parent must name a directory below the filesystem root");
    }
    validate_existing_directory_prefix_nofollow(cache)
        .into_diagnostic()
        .wrap_err("SDK release cache parent contains an unsafe path component")?;

    let lock_digest = lock
        .digest()
        .map_err(|error| miette::miette!("cannot derive canonical SDK lock digest: {error}"))?;
    let cache_namespace = cache.join(lock_digest.as_str());
    prepare_namespace(cache, &cache_namespace, offline)?;

    let archive_name = lock
        .identity
        .asset_name()
        .map_err(|error| miette::miette!("cannot derive locked SDK archive name: {error}"))?;
    let manifest_name = format!("{archive_name}.manifest.json");
    let archive_url = lock
        .archive_url()
        .map_err(|error| miette::miette!("cannot derive locked SDK archive URL: {error}"))?;
    let manifest_url = lock
        .manifest_url()
        .map_err(|error| miette::miette!("cannot derive locked SDK manifest URL: {error}"))?;

    let manifest = acquire_member(
        &cache_namespace,
        &manifest_name,
        &manifest_url,
        lock.manifest.size,
        &lock.manifest.sha256,
        offline,
    )
    .await
    .map_err(|error| {
        miette::miette!("cannot acquire the SDK manifest selected by the release lock: {error}")
    })?;

    let archive = acquire_member(
        &cache_namespace,
        &archive_name,
        &archive_url,
        lock.archive.size,
        &lock.archive.sha256,
        offline,
    )
    .await
    .map_err(|error| {
        miette::miette!("cannot acquire the SDK archive selected by the release lock: {error}")
    })?;

    aros_toolchain::sdk_package::verify_locked_archive(&SdkLockedArchiveRequest {
        archive: archive.path().to_path_buf(),
        lock: lock.clone(),
    })
    .map_err(|error| miette::miette!("locked SDK archive verification failed: {error}"))?;

    let acquired = AcquiredSdk {
        archive,
        manifest,
        cache_archive: cache_namespace.join(archive_name),
        cache_manifest: cache_namespace.join(manifest_name),
        cache_namespace,
    };
    acquired.revalidate()?;
    Ok(acquired)
}

async fn acquire_member(
    namespace: &Path,
    name: &str,
    url: &str,
    size: u64,
    digest: &aros_common::Sha256Digest,
    offline: bool,
) -> aros_fetch::FetchResult<VerifiedCachePayload> {
    // The acquisition primitive creates a persistent candidate lock even on a
    // cache miss. Offline observation must not create that control-plane file.
    if offline {
        snapshot_verified_cache_payload(namespace, name, size, digest)
    } else {
        acquire_https_cache_payload(namespace, name, url, size, digest, false).await
    }
}

fn prepare_namespace(cache: &Path, namespace: &Path, offline: bool) -> Result<()> {
    if offline {
        require_real_directory(cache, "SDK release cache parent")?;
        validate_existing_directory_prefix_nofollow(namespace)
            .into_diagnostic()
            .wrap_err("SDK release cache namespace contains an unsafe path component")?;
        require_real_directory(namespace, "SDK release cache namespace")?;
        return Ok(());
    }

    ensure_directory_nofollow(cache)
        .into_diagnostic()
        .wrap_err("cannot create or validate SDK release cache parent")?;
    ensure_directory_nofollow(namespace)
        .into_diagnostic()
        .wrap_err("cannot create or validate SDK release cache namespace")?;
    validate_existing_directory_prefix_nofollow(namespace)
        .into_diagnostic()
        .wrap_err("SDK release cache namespace contains an unsafe path component")?;
    require_real_directory(namespace, "SDK release cache namespace")
}

fn require_real_directory(path: &Path, label: &str) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(_) => Err(miette::miette!("{label} is not a real directory")),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            Err(miette::miette!("{label} does not exist"))
        }
        Err(error) => Err(error)
            .into_diagnostic()
            .wrap_err(format!("cannot inspect {label}")),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};

    use aros_common::sdk_manifest::{SdkAbiValue, SdkIdentity, SdkSourceIdentity, SdkTarget};
    use aros_common::sdk_release_lock::{
        SdkArtifactDigest, SdkReleaseLock, SDK_RELEASE_LOCK_SCHEMA,
    };
    use aros_common::{
        ensure_directory_nofollow, sha256_bytes, toolchain_tree_inventory_excluding, Sha256Digest,
    };
    use aros_toolchain::sdk_package::SdkPackageRequest;

    use super::prepare;

    fn identity() -> SdkIdentity {
        SdkIdentity {
            sdk_id: "core".into(),
            version: "1.2.3".into(),
            target: SdkTarget {
                target_triple: "i686-aros".into(),
                abi: BTreeMap::from([("data_model".into(), SdkAbiValue::Text("ilp32".into()))]),
            },
            source: SdkSourceIdentity {
                commit: "1".repeat(40),
                git_tree: "2".repeat(40),
                content_sha256: sha256_bytes(b"SDK acquisition test source"),
                contract_sha256: sha256_bytes(b"SDK acquisition test contract"),
            },
            source_date_epoch: 946_684_800,
        }
    }

    fn locked_fixture(root: &Path) -> (SdkReleaseLock, PathBuf) {
        let candidate = root.join("candidate");
        fs::create_dir(&candidate).unwrap();
        fs::create_dir(candidate.join("bin")).unwrap();
        fs::create_dir(candidate.join("include")).unwrap();
        fs::write(candidate.join("bin/aros"), b"#!/bin/sh\nexit 0\n").unwrap();
        fs::write(candidate.join("include/aros.h"), b"#define AROS_SDK 1\n").unwrap();
        fs::write(
            candidate.join("toolchain-manifest.json"),
            b"source metadata\n",
        )
        .unwrap();
        let (tree_digest, _) =
            toolchain_tree_inventory_excluding(&candidate, &["sdk-manifest.json"])
                .expect("measure fixture SDK tree");
        let tree_sha256 = Sha256Digest::parse(&tree_digest).unwrap();
        let identity = identity();
        let output = aros_toolchain::sdk_package::package(&SdkPackageRequest {
            candidate_root: candidate,
            output_dir: root.join("package"),
            identity: identity.clone(),
            expected_tree_sha256: tree_sha256.clone(),
            forbidden_prefixes: vec![PathBuf::from("/tmp/aros-sdk-test-private-root")],
        })
        .expect("package small fixture SDK");
        let manifest_bytes = fs::read(&output.manifest).unwrap();
        (
            SdkReleaseLock {
                schema: SDK_RELEASE_LOCK_SCHEMA.into(),
                release_tag: "v2.0.0".into(),
                base_url: "https://releases.example.invalid/project/v2.0.0".into(),
                identity,
                tree_sha256,
                archive: SdkArtifactDigest {
                    sha256: output.archive_sha256,
                    size: output.archive_size,
                },
                manifest: SdkArtifactDigest {
                    sha256: sha256_bytes(&manifest_bytes),
                    size: manifest_bytes.len() as u64,
                },
            },
            output.archive,
        )
    }

    fn namespace(cache: &Path, lock: &SdkReleaseLock) -> PathBuf {
        cache.join(lock.digest().unwrap().as_str())
    }

    #[tokio::test]
    async fn offline_missing_namespace_is_not_created() {
        let temporary = tempfile::tempdir().unwrap();
        let (lock, _) = locked_fixture(temporary.path());
        let cache = temporary.path().join("sdk-cache");
        ensure_directory_nofollow(&cache).unwrap();
        let namespace = namespace(&cache, &lock);

        assert!(prepare(&lock, &cache, true).await.is_err());
        assert!(!namespace.exists());
    }

    #[tokio::test]
    async fn offline_missing_members_leave_an_existing_namespace_unchanged() {
        let temporary = tempfile::tempdir().unwrap();
        let (lock, _) = locked_fixture(temporary.path());
        let cache = temporary.path().join("sdk-cache");
        let selected = namespace(&cache, &lock);
        ensure_directory_nofollow(&selected).unwrap();
        assert!(prepare(&lock, &cache, true).await.is_err());
        assert_eq!(fs::read_dir(&selected).unwrap().count(), 0);
        let asset = lock.identity.asset_name().unwrap();
        let manifest_name = format!("{asset}.manifest.json");
        fs::copy(
            temporary.path().join("package").join(&manifest_name),
            selected.join(&manifest_name),
        )
        .unwrap();
        assert!(prepare(&lock, &cache, true).await.is_err());
        let names: Vec<_> = fs::read_dir(&selected)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from(manifest_name)]);
    }

    #[tokio::test]
    async fn offline_corrupt_archive_is_rejected_without_repair() {
        let temporary = tempfile::tempdir().unwrap();
        let (lock, _) = locked_fixture(temporary.path());
        let cache = temporary.path().join("sdk-cache");
        let namespace = namespace(&cache, &lock);
        ensure_directory_nofollow(&namespace).unwrap();
        let asset = lock.identity.asset_name().unwrap();
        let manifest = fs::read(
            temporary
                .path()
                .join("package")
                .join(format!("{asset}.manifest.json")),
        )
        .unwrap();
        let archive_path = namespace.join(&asset);
        fs::write(namespace.join(format!("{asset}.manifest.json")), manifest).unwrap();
        fs::write(&archive_path, b"poisoned archive bytes").unwrap();

        assert!(prepare(&lock, &cache, true).await.is_err());
        assert_eq!(fs::read(archive_path).unwrap(), b"poisoned archive bytes");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn offline_archive_symlink_is_rejected_and_left_untouched() {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::tempdir().unwrap();
        let (lock, source_archive) = locked_fixture(temporary.path());
        let cache = temporary.path().join("sdk-cache");
        let namespace = namespace(&cache, &lock);
        ensure_directory_nofollow(&namespace).unwrap();
        let asset = lock.identity.asset_name().unwrap();
        let manifest = fs::read(
            temporary
                .path()
                .join("package")
                .join(format!("{asset}.manifest.json")),
        )
        .unwrap();
        fs::write(namespace.join(format!("{asset}.manifest.json")), manifest).unwrap();
        let archive_path = namespace.join(&asset);
        symlink(&source_archive, &archive_path).unwrap();

        assert!(prepare(&lock, &cache, true).await.is_err());
        assert_eq!(fs::read_link(archive_path).unwrap(), source_archive);
    }
}
