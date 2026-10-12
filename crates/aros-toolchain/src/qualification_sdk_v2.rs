//! Pure, bounded claims for input-selected SDK generation measurements.
//!
//! These rows identify measured tuples and byte digests only. They do not
//! assert origin, signatures, execution, or release authority. Readback joins
//! remain opaque and separately acquired.

use aros_common::Sha256Digest;
use serde::{de, Deserialize, Deserializer, Serialize};

use crate::qualification_evidence::safe_segment;
use crate::release_inputs::{ReleaseInputs, ACTIVE_HOSTS};
use crate::ContractError;

#[cfg(unix)]
use crate::sdk_generation_matrix::IndexedSdkGenerationReadback;

const MAX_CLAIMS: usize = 32 * 64 * 3;

/// One input-bound tuple and its SDK-generation measurement claims.
///
/// The four digests name observed bytes. This closed record is not an origin,
/// signature, execution, or qualification proof.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QualificationSdkGenerationV2 {
    /// SDK product selected by the release inputs.
    pub sdk_id: String,
    /// Compiler input group selected by the release inputs.
    pub group_id: String,
    /// Target profile selected by the release inputs.
    pub target_profile: String,
    /// Active native host selected by the release inputs.
    pub host: String,
    /// Raw digest of the SDK generation record.
    #[serde(deserialize_with = "deserialize_canonical_sha256")]
    pub generation_record_sha256: Sha256Digest,
    /// Raw digest of the portable compatibility report.
    #[serde(deserialize_with = "deserialize_canonical_sha256")]
    pub compatibility_report_sha256: Sha256Digest,
    /// Digest claimed by the generation record for its compatibility receipt.
    #[serde(deserialize_with = "deserialize_canonical_sha256")]
    pub compatibility_receipt_sha256: Sha256Digest,
    /// Digest claimed by the generation record for its SDK-link receipt.
    #[serde(deserialize_with = "deserialize_canonical_sha256")]
    pub sdk_link_receipt_sha256: Sha256Digest,
}

/// Validate safe, canonical claim rows without asserting input coverage.
///
/// Empty claims are valid here because compiler-only releases have no SDK
/// generation rows. Complete coverage is checked against [`ReleaseInputs`].
pub(crate) fn validate_structure(
    claims: &[QualificationSdkGenerationV2],
) -> Result<(), ContractError> {
    if claims.len() > MAX_CLAIMS {
        return Err(error("SDK qualification claims exceed the bounded matrix"));
    }
    let mut previous: Option<(&str, &str, &str, &str)> = None;
    for claim in claims {
        if !safe_sdk_id(&claim.sdk_id)
            || !safe_segment(&claim.group_id)
            || !safe_segment(&claim.target_profile)
            || !safe_segment(&claim.host)
            || !ACTIVE_HOSTS.contains(&claim.host.as_str())
        {
            return Err(error("SDK qualification claim has an unsafe tuple"));
        }
        let key = (
            claim.sdk_id.as_str(),
            claim.group_id.as_str(),
            claim.target_profile.as_str(),
            claim.host.as_str(),
        );
        if previous.is_some_and(|prior| prior >= key) {
            return Err(error(
                "SDK qualification claims must be sorted and unique by input tuple",
            ));
        }
        previous = Some(key);
    }
    Ok(())
}

/// Require exact SDK tuple coverage from validated release inputs.
pub(crate) fn validate_against_inputs(
    claims: &[QualificationSdkGenerationV2],
    inputs: &ReleaseInputs,
) -> Result<(), ContractError> {
    validate_structure(claims)?;
    let expected = inputs.expected_sdk_generation_lanes();
    if claims.len() != expected.len()
        || claims.iter().zip(&expected).any(|(claim, lane)| {
            claim.sdk_id != lane.sdk_id()
                || claim.group_id != lane.group_id()
                || claim.target_profile != lane.profile()
                || claim.host != lane.host()
        })
    {
        return Err(error(
            "SDK qualification claims lack exact input-derived tuple coverage",
        ));
    }
    Ok(())
}

/// Project claims solely from an opaque measured SDK generation readback.
#[cfg(unix)]
pub(crate) fn from_readback(
    readback: &IndexedSdkGenerationReadback,
) -> QualificationSdkGenerationV2 {
    let lane = readback.lane();
    let generation = readback.generation();
    QualificationSdkGenerationV2 {
        sdk_id: lane.sdk_id().to_owned(),
        group_id: lane.group_id().to_owned(),
        target_profile: lane.profile().to_owned(),
        host: lane.host().to_owned(),
        generation_record_sha256: generation.record_sha256().clone(),
        compatibility_report_sha256: generation.compatibility_manifest_sha256().clone(),
        compatibility_receipt_sha256: generation.compatibility_receipt_sha256().clone(),
        sdk_link_receipt_sha256: generation.sdk_link_receipt_sha256().clone(),
    }
}

/// Join exact claims to every independently measured SDK generation row.
#[cfg(unix)]
pub(crate) fn match_readback(
    claims: &[QualificationSdkGenerationV2],
    readbacks: &[IndexedSdkGenerationReadback],
) -> Result<(), ContractError> {
    validate_structure(claims)?;
    if claims.len() != readbacks.len() {
        return Err(error(
            "SDK qualification claims differ from measured generation coverage",
        ));
    }
    let mut measured = std::collections::BTreeMap::new();
    for readback in readbacks {
        let lane = readback.lane();
        let key = (
            lane.sdk_id().to_owned(),
            lane.group_id().to_owned(),
            lane.profile().to_owned(),
            lane.host().to_owned(),
        );
        if measured.insert(key, readback).is_some() {
            return Err(error("SDK generation readback contains duplicate tuples"));
        }
    }
    for claim in claims {
        let key = (
            claim.sdk_id.clone(),
            claim.group_id.clone(),
            claim.target_profile.clone(),
            claim.host.clone(),
        );
        let readback = measured
            .get(&key)
            .ok_or_else(|| error("SDK qualification claim has no measured generation row"))?;
        let measured_claim = from_readback(readback);
        if claim != &measured_claim {
            return Err(error(
                "SDK qualification claim differs from measured generation bytes",
            ));
        }
    }
    Ok(())
}

fn safe_sdk_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 96
        && value != "."
        && value != ".."
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"-_.+".contains(&byte)
        })
}

fn deserialize_canonical_sha256<'de, D>(deserializer: D) -> Result<Sha256Digest, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    let digest = Sha256Digest::parse(&value).map_err(de::Error::custom)?;
    if digest.as_str() != value {
        return Err(de::Error::custom(
            "SHA-256 digest must use lowercase hexadecimal",
        ));
    }
    Ok(digest)
}

fn error(message: &str) -> ContractError {
    ContractError::recovery(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim(sdk: &str, group: &str, profile: &str, host: &str) -> QualificationSdkGenerationV2 {
        let digest = Sha256Digest::parse(&"a".repeat(64)).unwrap();
        QualificationSdkGenerationV2 {
            sdk_id: sdk.to_owned(),
            group_id: group.to_owned(),
            target_profile: profile.to_owned(),
            host: host.to_owned(),
            generation_record_sha256: digest.clone(),
            compatibility_report_sha256: digest.clone(),
            compatibility_receipt_sha256: digest.clone(),
            sdk_link_receipt_sha256: digest,
        }
    }

    #[test]
    fn structure_requires_sorted_unique_tuples_and_allows_repeated_digests() {
        let first = claim("sdk-a", "compiler", "rv64", "linux-aarch64");
        let second = claim("sdk-b", "compiler", "rv64", "linux-aarch64");
        assert!(validate_structure(&[]).is_ok());
        assert!(validate_structure(&[first.clone(), second.clone()]).is_ok());
        assert!(validate_structure(&[second.clone(), first]).is_err());
        assert!(validate_structure(&[second.clone(), second]).is_err());
    }

    #[test]
    fn structure_enforces_sdk_grammar_and_current_native_hosts() {
        let long_sdk = "a".repeat(97);
        for invalid in ["", ".", "..", "SDK", "sdk*", long_sdk.as_str()] {
            assert!(
                validate_structure(&[claim(invalid, "compiler", "rv64", "linux-aarch64")]).is_err()
            );
        }
        assert!(validate_structure(&[claim("sdk+1", "compiler", "rv64", "linux-aarch64")]).is_ok());
        assert!(validate_structure(&[claim("sdk", "compiler", "rv64", "Linux-aarch64")]).is_err());
        assert!(validate_structure(&[claim("sdk", "compiler", "rv64", "linux-mips")]).is_err());
    }

    #[test]
    fn serde_is_closed_typed_bounded_and_requires_canonical_digests() {
        let value =
            serde_json::to_value(claim("sdk", "compiler", "rv64", "linux-aarch64")).unwrap();

        assert!(validate_structure(&vec![
            claim("sdk", "compiler", "rv64", "linux-aarch64");
            MAX_CLAIMS + 1
        ])
        .is_err());

        let mut unknown = value.clone();
        unknown["unexpected"] = serde_json::json!(true);
        assert!(serde_json::from_value::<QualificationSdkGenerationV2>(unknown).is_err());

        let mut wrong_type = value.clone();
        wrong_type["host"] = serde_json::json!(7);
        assert!(serde_json::from_value::<QualificationSdkGenerationV2>(wrong_type).is_err());

        let mut oversized = value.clone();
        oversized["sdk_id"] = serde_json::json!("s".repeat(97));
        let oversized: QualificationSdkGenerationV2 = serde_json::from_value(oversized).unwrap();
        assert!(validate_structure(&[oversized]).is_err());

        let mut noncanonical = value;
        noncanonical["generation_record_sha256"] = serde_json::json!("A".repeat(64));
        assert!(serde_json::from_value::<QualificationSdkGenerationV2>(noncanonical).is_err());
    }
}
