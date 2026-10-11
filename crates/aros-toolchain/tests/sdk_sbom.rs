#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{symlink, MetadataExt};
use std::path::{Path, PathBuf};

use aros_common::sdk_manifest::{SdkAbiValue, SdkIdentity, SdkSourceIdentity, SdkTarget};
use aros_common::{sha256_bytes, toolchain_tree_inventory_excluding, Sha256Digest};
use aros_toolchain::sdk_package::{self, SdkPackageRequest, SdkVerificationRequest};
use aros_toolchain::sdk_sbom::build_measured_sdk_sbom;
use serde_json::Value;

struct Fixture {
    temporary: tempfile::TempDir,
    request: SdkVerificationRequest,
}

fn identity() -> SdkIdentity {
    SdkIdentity {
        sdk_id: "core".into(),
        version: "0.0.0-local".into(),
        target: SdkTarget {
            target_triple: "riscv-aros".into(),
            abi: BTreeMap::from([("data_model".into(), SdkAbiValue::Text("ilp32f".into()))]),
        },
        source: SdkSourceIdentity {
            commit: "1".repeat(40),
            git_tree: "2".repeat(40),
            content_sha256: sha256_bytes(b"synthetic SDK source"),
            contract_sha256: sha256_bytes(b"synthetic SDK source contract"),
        },
        source_date_epoch: 946_684_800,
    }
}

fn fixture(selected: SdkIdentity, header: &[u8]) -> Fixture {
    let temporary = tempfile::tempdir().unwrap();
    let candidate = temporary.path().join("candidate");
    fs::create_dir(&candidate).unwrap();
    fs::create_dir(candidate.join("include")).unwrap();
    fs::create_dir(candidate.join("lib")).unwrap();
    fs::write(candidate.join("include/aros.h"), header).unwrap();
    fs::write(candidate.join("lib/libaros.a"), b"synthetic archive\n").unwrap();
    symlink("include/aros.h", candidate.join("aros.h")).unwrap();
    let (tree, _) = toolchain_tree_inventory_excluding(&candidate, &["sdk-manifest.json"])
        .expect("measure tiny SDK candidate");
    let tree = Sha256Digest::parse(&tree).unwrap();
    let package_dir = temporary.path().join("package");
    let output = sdk_package::package(&SdkPackageRequest {
        candidate_root: candidate,
        output_dir: package_dir.clone(),
        identity: selected.clone(),
        expected_tree_sha256: tree.clone(),
        forbidden_prefixes: vec![],
    })
    .expect("package tiny SDK candidate");
    Fixture {
        temporary,
        request: SdkVerificationRequest {
            package_dir,
            identity: selected,
            expected_tree_sha256: tree,
            expected_archive_sha256: output.archive_sha256,
            expected_archive_size: output.archive_size,
            forbidden_prefixes: vec![],
        },
    }
}

// Ignore access times: read-only verification may legitimately update them.
// Preserve the directory and member identity, permissions, contents and both
// modification/change timestamps so a rewrite with equal bytes is not hidden.
type Stat = (u64, u64, u32, u64, u64, i64, i64, i64, i64);
type Snapshot = BTreeMap<PathBuf, (Stat, Option<Vec<u8>>)>;

fn snapshot(root: &Path) -> Snapshot {
    fn visit(path: &Path, root: &Path, entries: &mut Snapshot) {
        let metadata = fs::symlink_metadata(path).unwrap();
        let stat = (
            metadata.dev(),
            metadata.ino(),
            metadata.mode(),
            metadata.nlink(),
            metadata.size(),
            metadata.mtime(),
            metadata.mtime_nsec(),
            metadata.ctime(),
            metadata.ctime_nsec(),
        );
        let content = if metadata.is_file() {
            Some(fs::read(path).unwrap())
        } else if metadata.file_type().is_symlink() {
            Some(
                fs::read_link(path)
                    .unwrap()
                    .as_os_str()
                    .as_encoded_bytes()
                    .to_vec(),
            )
        } else {
            None
        };
        entries.insert(
            path.strip_prefix(root).unwrap().to_path_buf(),
            (stat, content),
        );
        if metadata.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                visit(&entry.unwrap().path(), root, entries);
            }
        }
    }
    let mut entries = Snapshot::new();
    visit(root, root, &mut entries);
    entries
}

fn reject_without_writes(request: &SdkVerificationRequest, fixture: &Fixture, reason: &str) {
    let before = snapshot(fixture.temporary.path());
    let error = build_measured_sdk_sbom(request).expect_err("invalid SDK must not emit SPDX bytes");
    assert!(
        error.to_string().contains(reason),
        "wrong rejection: {error}"
    );
    assert_eq!(snapshot(fixture.temporary.path()), before);
}

#[test]
fn verified_sdk_emits_repeatable_read_only_archive_bound_spdx() {
    let fixture = fixture(identity(), b"#define AROS_SDK 1\n");
    let before = snapshot(fixture.temporary.path());
    let first = build_measured_sdk_sbom(&fixture.request).unwrap();
    assert_eq!(first, build_measured_sdk_sbom(&fixture.request).unwrap());
    assert_eq!(snapshot(fixture.temporary.path()), before);
    assert_eq!(
        fs::read_dir(&fixture.request.package_dir).unwrap().count(),
        3
    );
    assert!(first.is_ascii());
    assert!(first.ends_with(b"\n"));
    let document: Value = serde_json::from_slice(&first).unwrap();
    let asset = fixture.request.identity.asset_name().unwrap();
    assert_eq!(document["spdxVersion"], "SPDX-2.3");
    assert_eq!(document["dataLicense"], "CC0-1.0");
    assert_eq!(document["name"], asset);
    assert_eq!(document["creationInfo"]["created"], "2000-01-01T00:00:00Z");
    assert_eq!(
        document["creationInfo"]["creators"][0],
        "Tool: aros-tools-sdk-sbom-1"
    );
    assert_eq!(
        document["documentNamespace"],
        format!(
            "https://github.com/metaneutrons/aros-toolchains/sdk/{}",
            fixture.request.expected_archive_sha256
        )
    );
    let packages = document["packages"].as_array().unwrap();
    assert_eq!(packages.len(), 1);
    let package = &packages[0];
    assert_eq!(package["name"], "core");
    assert_eq!(package["versionInfo"], "0.0.0-local");
    assert_eq!(package["packageFileName"], asset);
    assert_eq!(package["checksums"][0]["algorithm"], "SHA256");
    assert_eq!(
        package["checksums"][0]["checksumValue"],
        fixture.request.expected_archive_sha256.as_str()
    );
    assert_ne!(
        fixture.request.expected_archive_sha256,
        fixture.request.expected_tree_sha256
    );
    for field in [
        "downloadLocation",
        "licenseConcluded",
        "licenseDeclared",
        "copyrightText",
    ] {
        assert_eq!(package[field], "NOASSERTION");
    }
    assert_eq!(package["filesAnalyzed"], false);
    assert!(document.get("files").is_none());
    assert!(package.get("packageVerificationCode").is_none());
    let info = package["sourceInfo"].as_str().unwrap();
    for value in [
        fixture.request.identity.source.commit.as_str(),
        fixture.request.identity.source.git_tree.as_str(),
        fixture.request.identity.source.content_sha256.as_str(),
        fixture.request.identity.source.contract_sha256.as_str(),
        fixture.request.expected_tree_sha256.as_str(),
        fixture.request.identity.target.sha256().unwrap().as_str(),
    ] {
        assert!(info.contains(value), "missing SDK identity {value}");
    }
    assert!(info.contains("not authenticated execution provenance"));
    assert_eq!(
        document["documentDescribes"],
        serde_json::json!(["SPDXRef-Package-AROSSDK"])
    );
    assert_eq!(
        document["relationships"],
        serde_json::json!([{
            "spdxElementId": "SPDXRef-DOCUMENT", "relationshipType": "DESCRIBES",
            "relatedSpdxElement": "SPDXRef-Package-AROSSDK"
        }])
    );
}

#[test]
fn archive_payload_and_source_identity_changes_change_the_document() {
    let original = fixture(identity(), b"#define AROS_SDK 1\n");
    let original_bytes = build_measured_sdk_sbom(&original.request).unwrap();
    let changed_payload = fixture(identity(), b"#define AROS_SDK 2\n");
    assert_ne!(
        original_bytes,
        build_measured_sdk_sbom(&changed_payload.request).unwrap()
    );
    let mut changed = identity();
    changed.source.commit = "3".repeat(40);
    changed.source_date_epoch += 1;
    let changed_source = fixture(changed, b"#define AROS_SDK 1\n");
    let changed_bytes = build_measured_sdk_sbom(&changed_source.request).unwrap();
    assert_ne!(original_bytes, changed_bytes);
    let document: Value = serde_json::from_slice(&changed_bytes).unwrap();
    assert_eq!(document["creationInfo"]["created"], "2000-01-01T00:00:01Z");
    assert!(document["packages"][0]["sourceInfo"]
        .as_str()
        .unwrap()
        .contains(&"3".repeat(40)));
}

#[test]
fn sbom_requires_independently_selected_archive_tree_and_target() {
    let fixture = fixture(identity(), b"SDK header\n");
    let mut wrong_hash = fixture.request.clone();
    wrong_hash.expected_archive_sha256 = sha256_bytes(b"not the selected archive");
    reject_without_writes(&wrong_hash, &fixture, "independently selected hash or size");
    let mut wrong_size = fixture.request.clone();
    wrong_size.expected_archive_size += 1;
    reject_without_writes(&wrong_size, &fixture, "independently selected hash or size");
    let mut wrong_tree = fixture.request.clone();
    wrong_tree.expected_tree_sha256 = sha256_bytes(b"not the selected tree");
    reject_without_writes(&wrong_tree, &fixture, "selected tree digest");
    let mut wrong_target = fixture.request.clone();
    wrong_target.identity.target.target_triple = "i686-aros".into();
    reject_without_writes(&wrong_target, &fixture, "selected identity");
}

#[test]
fn sbom_rejects_corrupt_archive_manifest_and_sidecar_without_writes() {
    for suffix in ["", ".manifest.json", ".sha256"] {
        let fixture = fixture(identity(), b"SDK header\n");
        let path = fixture.request.package_dir.join(format!(
            "{}{suffix}",
            fixture.request.identity.asset_name().unwrap()
        ));
        let mut bytes = fs::read(&path).unwrap();
        bytes[0] ^= 1;
        fs::write(path, bytes).unwrap();
        let reason = match suffix {
            "" => "independently selected hash or size",
            ".manifest.json" => "external SDK manifest is invalid",
            _ => "checksum sidecar does not match",
        };
        reject_without_writes(&fixture.request, &fixture, reason);
    }
}

#[test]
fn sbom_rejects_symlink_and_hardlink_package_members_without_writes() {
    for suffix in ["", ".manifest.json", ".sha256"] {
        for hardlink in [false, true] {
            let fixture = fixture(identity(), b"SDK header\n");
            let path = fixture.request.package_dir.join(format!(
                "{}{suffix}",
                fixture.request.identity.asset_name().unwrap()
            ));
            let saved = fixture.temporary.path().join("original-member");
            let reason = if hardlink {
                fs::hard_link(&path, saved).unwrap();
                "single-link regular file"
            } else {
                fs::rename(&path, &saved).unwrap();
                symlink(saved, path).unwrap();
                "cannot safely open SDK package member"
            };
            reject_without_writes(&fixture.request, &fixture, reason);
        }
    }
}

#[test]
fn supplemental_spdx_cannot_be_silently_adopted_as_a_local_package_member() {
    let fixture = fixture(identity(), b"SDK header\n");
    let document = build_measured_sdk_sbom(&fixture.request).unwrap();
    fs::write(fixture.request.package_dir.join("sdk.spdx.json"), document).unwrap();
    reject_without_writes(
        &fixture.request,
        &fixture,
        "exactly the three required files",
    );
}
