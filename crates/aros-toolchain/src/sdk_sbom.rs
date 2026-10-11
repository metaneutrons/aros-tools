//! Deterministic SPDX metadata for one measured SDK archive.
//!
//! This describes the selected distribution and its source identity. It does
//! not infer third-party components or licenses, authenticate an execution,
//! or grant publication authority. It writes no package or release file.

use aros_common::sdk_manifest::SDK_MAX_METADATA_BYTES;
use serde_json::json;

use crate::sdk_package::{self, SdkVerificationRequest, VerifiedSdkPackage};
use crate::ContractError;

/// Verify one exact local SDK package and render its deterministic SPDX bytes.
///
/// The public package contract remains exactly archive, manifest and sidecar.
/// The returned supplemental document belongs in a separately owned release
/// stage, not in that package directory. Full package verification brackets
/// rendering and both observations must agree. No output is written.
///
/// The SPDX package checksum identifies the actual archive, not the payload
/// tree or source projection. `filesAnalyzed: false` deliberately makes no
/// file-analysis or license/component-completeness claim. The complete SDK
/// payload inventory remains in its independently verified manifest.
///
/// # Errors
/// Returns a verification error for invalid or changing package inputs, SDK
/// identities, artifact measurements or bounded document encoding.
pub fn build_measured_sdk_sbom(request: &SdkVerificationRequest) -> Result<Vec<u8>, ContractError> {
    let initial = sdk_package::verify(request)?;
    let bytes = sdk_spdx_bytes(&initial)?;
    if sdk_package::verify(request)? != initial {
        return Err(error(
            "SDK package changed while rendering its SPDX metadata",
        ));
    }
    Ok(bytes)
}

// Shared release read-back will render the same bytes from its already verified
// selected SDK package. This is not a public shortcut around package acquisition.
pub(crate) fn sdk_spdx_bytes(package: &VerifiedSdkPackage) -> Result<Vec<u8>, ContractError> {
    let manifest = &package.manifest;
    manifest
        .validate()
        .map_err(|_| error("SDK SPDX metadata requires a valid complete SDK manifest"))?;
    if package.archive_size == 0
        || package.archive_size > sdk_package::MAX_ARCHIVE_BYTES
        || package
            .archive_sha256
            .as_str()
            .bytes()
            .all(|byte| byte == b'0')
    {
        return Err(error(
            "SDK SPDX metadata requires a valid measured archive identity",
        ));
    }
    let identity = &manifest.identity;
    let asset = identity
        .asset_name()
        .map_err(|_| error("SDK SPDX metadata has an invalid archive name"))?;
    let created = crate::package::rfc3339(identity.source_date_epoch)
        .map_err(|_| error("SDK SPDX metadata timestamp is outside its supported range"))?;
    let source_info = format!(
        "Selected AROS source commit {}; Git tree {}; normalized source projection SHA256 {}; \
         source-owned SDK contract SHA256 {}; SDK inventory SHA256 {}; target descriptor SHA256 {}. \
         These are selected byte identities, not authenticated execution provenance.",
        identity.source.commit,
        identity.source.git_tree,
        identity.source.content_sha256,
        identity.source.contract_sha256,
        manifest.tree_sha256,
        identity.target.sha256().map_err(|_| error("invalid SDK SPDX target descriptor"))?,
    );
    let document = json!({
        "spdxVersion": "SPDX-2.3",
        "dataLicense": "CC0-1.0",
        "SPDXID": "SPDXRef-DOCUMENT",
        "name": asset,
        "documentNamespace": format!(
            "https://github.com/metaneutrons/aros-toolchains/sdk/{}", package.archive_sha256
        ),
        "creationInfo": {
            "created": created,
            "creators": ["Tool: aros-tools-sdk-sbom-1"],
            "comment": "SDK distribution metadata only; third-party component and license analysis is not asserted."
        },
        "documentDescribes": ["SPDXRef-Package-AROSSDK"],
        "packages": [{
            "SPDXID": "SPDXRef-Package-AROSSDK",
            "name": identity.sdk_id,
            "versionInfo": identity.version,
            "packageFileName": asset,
            "downloadLocation": "NOASSERTION",
            "filesAnalyzed": false,
            "checksums": [{"algorithm": "SHA256", "checksumValue": package.archive_sha256}],
            "sourceInfo": source_info,
            "licenseConcluded": "NOASSERTION",
            "licenseDeclared": "NOASSERTION",
            "copyrightText": "NOASSERTION"
        }],
        "relationships": [{
            "spdxElementId": "SPDXRef-DOCUMENT",
            "relationshipType": "DESCRIBES",
            "relatedSpdxElement": "SPDXRef-Package-AROSSDK"
        }]
    });
    let bytes = crate::package::pretty_json(&document)
        .map_err(|_| error("cannot encode deterministic SDK SPDX metadata"))?;
    if bytes.len() > SDK_MAX_METADATA_BYTES {
        return Err(error(
            "SDK SPDX metadata exceeds its bounded document limit",
        ));
    }
    Ok(bytes)
}

fn error(message: &str) -> ContractError {
    ContractError::verification(message)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use aros_common::sdk_manifest::{
        SdkAbiValue, SdkIdentity, SdkManifest, SdkSourceIdentity, SdkTarget, SDK_MANIFEST_SCHEMA,
    };
    use aros_common::{
        sha256_bytes, toolchain_inventory_sha256, ArosToolchainManifestEntry, Sha256Digest,
    };

    use super::*;

    fn verified_fixture() -> VerifiedSdkPackage {
        let files = vec![ArosToolchainManifestEntry {
            path: "aros.h".into(),
            mode: "0644".into(),
            kind: "file".into(),
            sha256: Some(sha256_bytes(b"header").to_string()),
            size: Some(6),
            target: None,
        }];
        VerifiedSdkPackage {
            manifest: SdkManifest {
                schema: SDK_MANIFEST_SCHEMA.into(),
                identity: SdkIdentity {
                    sdk_id: "core".into(),
                    version: "1.0.0".into(),
                    target: SdkTarget {
                        target_triple: "riscv-aros".into(),
                        abi: BTreeMap::from([(
                            "data_model".into(),
                            SdkAbiValue::Text("ilp32f".into()),
                        )]),
                    },
                    source: SdkSourceIdentity {
                        commit: "1".repeat(40),
                        git_tree: "2".repeat(40),
                        content_sha256: sha256_bytes(b"source"),
                        contract_sha256: sha256_bytes(b"contract"),
                    },
                    source_date_epoch: 946_684_800,
                },
                tree_sha256: Sha256Digest::parse(&toolchain_inventory_sha256(&files).unwrap())
                    .unwrap(),
                files,
            },
            archive_sha256: sha256_bytes(b"measured archive"),
            archive_size: 512,
        }
    }

    #[test]
    fn internal_renderer_revalidates_manifest_identity_and_inventory() {
        let package = verified_fixture();
        assert!(sdk_spdx_bytes(&package).is_ok());
        let mut bad_identity = package.clone();
        bad_identity.manifest.identity.version = "01.0.0".into();
        let mut bad_inventory = package.clone();
        bad_inventory.manifest.files[0].size = Some(7);
        let mut bad_epoch = package;
        bad_epoch.manifest.identity.source_date_epoch = u64::MAX;
        for invalid in [bad_identity, bad_inventory, bad_epoch] {
            let error = sdk_spdx_bytes(&invalid).unwrap_err();
            assert!(error.to_string().contains("valid complete SDK manifest"));
        }
    }

    #[test]
    fn internal_renderer_rejects_unmeasured_or_out_of_bound_archive_claims() {
        let mut package = verified_fixture();
        // Same bound as the actual package reader; these are metadata probes,
        // not a claim that a real maximum-size archive was read.
        package.archive_size = sdk_package::MAX_ARCHIVE_BYTES;
        assert!(sdk_spdx_bytes(&package).is_ok());
        for size in [0, sdk_package::MAX_ARCHIVE_BYTES + 1] {
            package.archive_size = size;
            assert!(sdk_spdx_bytes(&package)
                .unwrap_err()
                .to_string()
                .contains("measured archive identity"));
        }
        package.archive_size = 512;
        package.archive_sha256 = Sha256Digest::parse(&"0".repeat(64)).unwrap();
        assert!(sdk_spdx_bytes(&package)
            .unwrap_err()
            .to_string()
            .contains("measured archive identity"));
    }
}
