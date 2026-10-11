use super::{
    SdkArtifactDigest, SdkReleaseLock, SDK_RELEASE_LOCK_MAX_BYTES, SDK_RELEASE_LOCK_SCHEMA,
};
use crate::sdk_manifest::{
    SdkAbiValue, SdkIdentity, SdkSourceIdentity, SdkTarget, SDK_MAX_METADATA_BYTES,
};
use crate::{sha256_bytes, Sha256Digest};
use std::collections::BTreeMap;

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
            content_sha256: sha256_bytes(b"selected SDK source tree"),
            contract_sha256: sha256_bytes(b"selected SDK source contract"),
        },
        source_date_epoch: 1,
    }
}

fn artifact(bytes: &[u8], size: u64) -> SdkArtifactDigest {
    SdkArtifactDigest {
        sha256: sha256_bytes(bytes),
        size,
    }
}

fn release_lock() -> SdkReleaseLock {
    SdkReleaseLock {
        schema: SDK_RELEASE_LOCK_SCHEMA.into(),
        release_tag: "v2.0.0".into(),
        base_url: "https://releases.example.invalid/project/v2.0.0".into(),
        identity: identity(),
        tree_sha256: sha256_bytes(b"SDK tree digest"),
        archive: artifact(b"archive", 1024),
        manifest: artifact(b"manifest", 128),
    }
}

#[test]
fn canonical_lock_round_trips_and_derives_identity_based_urls() {
    let lock = release_lock();
    let bytes = lock.canonical_bytes().unwrap();
    assert_eq!(bytes.last(), Some(&b'\n'));
    assert_eq!(SdkReleaseLock::from_bytes(&bytes).unwrap(), lock);

    let asset = lock.identity.asset_name().unwrap();
    assert_eq!(
        lock.archive_url().unwrap(),
        format!("{}/{asset}", lock.base_url)
    );
    assert_eq!(
        lock.manifest_url().unwrap(),
        format!("{}/{asset}.manifest.json", lock.base_url)
    );
    assert_eq!(
        lock.checksum_url().unwrap(),
        format!("{}/{asset}.sha256", lock.base_url)
    );
    assert_eq!(lock.digest().unwrap(), sha256_bytes(&bytes));
}

#[test]
fn lock_preserves_explicit_identity_metadata_and_allows_mixed_versions() {
    let mut lock = release_lock();
    lock.release_tag = "v9.1.0-rc.2".into();
    lock.base_url = "https://releases.example.invalid/project/v9.1.0-rc.2".into();
    assert_eq!(lock.identity.version, "1.2.3");
    assert!(lock.validate().is_ok());

    let value: serde_json::Value =
        serde_json::from_slice(&lock.canonical_bytes().unwrap()).unwrap();
    assert_eq!(
        value["identity"],
        serde_json::to_value(&lock.identity).unwrap()
    );
    assert_eq!(value["tree_sha256"], lock.tree_sha256.as_str());
    assert_eq!(value["archive"]["sha256"], lock.archive.sha256.as_str());
    assert_eq!(value["manifest"]["size"], lock.manifest.size);
}

#[test]
fn artifact_sizes_and_digests_are_bounded_and_nonzero() {
    let mut lock = release_lock();
    assert!(lock.validate().is_ok());

    lock.archive.size = 0;
    assert!(lock.validate().is_err());
    lock = release_lock();
    lock.archive.size = 4 * 1024 * 1024 * 1024;
    assert!(lock.validate().is_ok());
    lock.archive.size += 1;
    assert!(lock.validate().is_err());

    lock = release_lock();
    lock.manifest.size = 0;
    assert!(lock.validate().is_err());
    lock.manifest.size = SDK_MAX_METADATA_BYTES as u64;
    assert!(lock.validate().is_ok());
    lock.manifest.size += 1;
    assert!(lock.validate().is_err());

    let zero = Sha256Digest::parse(&"0".repeat(64)).unwrap();
    lock = release_lock();
    lock.tree_sha256 = zero.clone();
    assert!(lock.validate().is_err());
    lock = release_lock();
    lock.archive.sha256 = zero.clone();
    assert!(lock.validate().is_err());
    lock = release_lock();
    lock.manifest.sha256 = zero;
    assert!(lock.validate().is_err());
}

#[test]
fn release_tag_requires_canonical_v_prefixed_semver_without_build_metadata() {
    assert!(release_lock().validate().is_ok());
    for tag in ["2.0.0", "v02.0.0", "v2.0", "v2.0.0+build.1", "v2.0.0-"] {
        let mut lock = release_lock();
        lock.release_tag = tag.into();
        assert!(lock.validate().is_err(), "accepted release tag {tag}");
    }
}

#[test]
fn release_url_must_be_canonical_credential_free_and_end_in_literal_tag() {
    for url in [
        "https://user:secret@releases.example.invalid/project/v2.0.0",
        "http://releases.example.invalid/project/v2.0.0",
        "https://releases.example.invalid/project/v2.0.0/",
        "https://releases.example.invalid/project/v2.0.0?download=1",
        "https://releases.example.invalid/project/v2.0.0#archive",
        "https://releases.example.invalid/project/v2.0.1",
        "https://releases.example.invalid/a/../project/v2.0.0",
        "https://releases.example.invalid/pro%6Aect/v2.0.0",
        "https://releases.example.invalid//project/v2.0.0",
    ] {
        let mut lock = release_lock();
        lock.base_url = url.into();
        assert!(lock.validate().is_err(), "accepted base URL {url}");
    }
}

#[test]
fn identity_rejects_host_board_and_compiler_selectors() {
    for selector in ["host", "board", "compiler"] {
        let mut lock = release_lock();
        lock.identity
            .target
            .abi
            .insert(selector.into(), SdkAbiValue::Text("selector".into()));
        assert!(
            lock.validate().is_err(),
            "accepted identity selector {selector}"
        );
    }
}

#[test]
fn oversized_url_is_rejected_before_canonical_encoding() {
    let mut lock = release_lock();
    lock.base_url = format!(
        "https://releases.example.invalid/{}/v2.0.0",
        "a".repeat(4096)
    );
    assert!(lock.validate().is_err());
    assert!(lock.canonical_bytes().is_err());
}

#[test]
fn closed_decoder_rejects_unknown_duplicate_and_oversized_documents() {
    let valid = String::from_utf8(release_lock().canonical_bytes().unwrap()).unwrap();

    let mut unknown = valid.clone();
    let end = unknown.rfind('}').unwrap();
    unknown.insert_str(end, ",\"unexpected\":true");
    assert!(SdkReleaseLock::from_bytes(unknown.as_bytes()).is_err());

    let duplicate = valid.replacen(
        "\"schema\":",
        "\"schema\":\"aros-sdk-lock-v2\",\"schema\":",
        1,
    );
    assert!(SdkReleaseLock::from_bytes(duplicate.as_bytes()).is_err());

    let mut nested = serde_json::to_value(release_lock()).unwrap();
    nested["archive"]["unexpected"] = serde_json::Value::Bool(true);
    assert!(SdkReleaseLock::from_bytes(&serde_json::to_vec(&nested).unwrap()).is_err());

    let oversized = vec![b' '; SDK_RELEASE_LOCK_MAX_BYTES + 1];
    assert!(SdkReleaseLock::from_bytes(&oversized).is_err());
}
