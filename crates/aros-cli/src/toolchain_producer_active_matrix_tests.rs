use std::fs;
use std::path::Path;

use aros_common::sha256_bytes;
use aros_toolchain::{
    canonical,
    release_index::{NativeReleaseArtifact, NativeReleaseIndex, ACTIVE_V1_HOSTS, V1_PROFILES},
};

use super::{qualification_lanes, RecordQualificationArgs, ResultFormat};

#[test]
fn qualification_lanes_follow_the_selected_active_release_matrix() {
    let temporary = tempfile::tempdir().unwrap();
    let lifecycle = temporary.path().join("lifecycle");
    let comparison = temporary.path().join("comparison");
    let compatibility = temporary.path().join("compatibility");
    let artifacts = ACTIVE_V1_HOSTS
        .iter()
        .flat_map(|host| {
            V1_PROFILES
                .iter()
                .map(move |profile| NativeReleaseArtifact {
                    asset: format!("{host}-{profile}.tar.xz"),
                    sha256: "a".repeat(64),
                    size: 1,
                    host: (*host).into(),
                    target_profile: (*profile).into(),
                    target_triple: match *profile {
                        "pc-x86_64" => "x86_64-unknown-aros",
                        "arm-raspi" => "arm-unknown-aros",
                        "rpi-aarch64" => "aarch64-unknown-aros",
                        _ => unreachable!(),
                    }
                    .into(),
                    tree_sha256: "b".repeat(64),
                    llvm_version: "11.0.0".into(),
                    enabled: true,
                    strip_components: 1,
                    required_paths: vec!["bin/clang".into()],
                })
        })
        .collect::<Vec<_>>();
    let index = NativeReleaseIndex {
        schema: 1,
        release_id: "toolchain-v1-test".into(),
        base_url: "https://example.invalid/toolchain-v1-test".into(),
        source_commit: "1".repeat(40),
        producer_commit: "2".repeat(40),
        tools_commit: "3".repeat(40),
        artifacts,
    };
    for artifact in &index.artifacts {
        write_qualification_reports(
            &lifecycle,
            &comparison,
            &compatibility,
            &artifact.host,
            &artifact.target_profile,
        );
    }
    let args = RecordQualificationArgs {
        release_dir: temporary.path().join("release"),
        source_lock_filename: "source-lock.json".into(),
        lifecycle_reports_dir: lifecycle,
        comparison_reports_dir: comparison,
        compatibility_reports_dir: compatibility,
        source_repository: "https://example.invalid/source".into(),
        source_workflow: ".github/workflows/release.yml".into(),
        source_run_id: 1,
        source_tag: "toolchain-v1-test".into(),
        source_tag_object: "4".repeat(40),
        source_tag_commit: "2".repeat(40),
        attestation_repository: "https://example.invalid/source".into(),
        attestation_workflow: ".github/workflows/release.yml".into(),
        attestation_signer: "github-actions".into(),
        created_at: 1,
        expires_at: 2,
        output: temporary.path().join("qualification.json"),
        format: ResultFormat::Human,
    };

    let lanes = qualification_lanes(&args, &index).unwrap();

    assert_eq!(lanes.len(), 9);
    assert_eq!(
        lanes
            .iter()
            .map(|lane| (&lane.host, &lane.target_profile))
            .collect::<Vec<_>>(),
        index
            .artifacts
            .iter()
            .map(|artifact| (&artifact.host, &artifact.target_profile))
            .collect::<Vec<_>>()
    );
    assert!(lanes.iter().all(|lane| lane.host != "macos-x86_64"));
}

fn write_qualification_reports(
    lifecycle: &Path,
    comparison: &Path,
    compatibility: &Path,
    host: &str,
    profile: &str,
) {
    let receipt = native_publish_receipt();
    for copy in ["a", "b"] {
        let path = lifecycle
            .join(format!("native-lifecycle-{host}-{profile}-{copy}"))
            .join("publish.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, serde_json::to_vec(&receipt).unwrap()).unwrap();
    }
    let comparison_path = comparison
        .join(format!("comparison-{host}-{profile}"))
        .join(format!("comparison-{host}-{profile}.json"));
    fs::create_dir_all(comparison_path.parent().unwrap()).unwrap();
    fs::write(
        comparison_path,
        serde_json::to_vec(&serde_json::json!({
            "schema": 1,
            "operation": "compare",
            "byte_identical": true,
            "members": ["archive", "manifest", "checksum", "sbom"],
        }))
        .unwrap(),
    )
    .unwrap();
    let compatibility_path = compatibility
        .join(format!("compatibility-{host}-{profile}"))
        .join("native-compatibility.receipt.json");
    fs::create_dir_all(compatibility_path.parent().unwrap()).unwrap();
    fs::write(
        compatibility_path,
        serde_json::to_vec(&serde_json::json!({
            "schema": "aros-toolchain-native-compatibility-receipt-v2",
            "operation": "native-compatibility",
            "phase_reports": [null, null, null, null, null, null],
            "ports_sources": [{
                "id": "fixture-port",
                "cache_filename": "fixture-source.tar.xz",
                "relative_path": "ports/fixture-source.tar.xz",
                "fetch_marker": "",
                "sha256": "c".repeat(64),
                "size": 1,
            }],
        }))
        .unwrap(),
    )
    .unwrap();
}

fn native_publish_receipt() -> serde_json::Value {
    let mut receipt = serde_json::json!({
        "schema": "aros-toolchain-receipt-v1",
        "backend": "native",
        "phase": "publish",
    });
    let digest = sha256_bytes(&canonical::bytes(&receipt).unwrap());
    receipt["receipt_sha256"] = serde_json::Value::String(digest.as_str().into());
    receipt
}
