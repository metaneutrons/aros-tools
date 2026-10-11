//! Read-only SDK identity binding to exact committed source inputs.
//!
//! A parsed descriptor or a tree digest alone cannot prove which commit
//! produced a source projection. This boundary joins the committed raw Git
//! projection, selected source profile, sealed native SDK contract and ABI.
//! It does not execute generation, prove reproducibility or admit a release.

use std::collections::BTreeMap;
use std::path::PathBuf;

use aros_common::native_consumer_contract::load_bound_native_consumer_contract;
use aros_common::sdk_manifest::{SdkAbiValue, SdkIdentity};
use aros_common::TargetProfile;

use crate::compatibility_source::{
    verify_engine_free_source, CommittedSourceIdentity, EngineFreeSourceVerificationRequest,
};
use crate::{inspection, recipe::GitObjectId, ContractError};

/// Explicit local source selection for one SDK product identity.
#[derive(Debug, Clone)]
pub struct SdkSourceVerificationRequest {
    /// Clean original Git checkout, not the engine-free projection.
    pub source_root: PathBuf,
    /// Existing normalized engine-free projection of that exact checkout.
    pub materialized_source_root: PathBuf,
    /// Exact source-owned target profile; no board or host inference.
    pub source_preset: String,
    /// Independently selected SDK identity, including its generation epoch.
    pub identity: SdkIdentity,
}

/// Successful local source binding; callers cannot manufacture this record.
///
/// No source-owned command has run. The epoch is explicit identity selection,
/// not an observed generation result or an inferred commit timestamp.
#[derive(Debug, Clone)]
pub struct VerifiedSdkSource {
    request: SdkSourceVerificationRequest,
    profile: TargetProfile,
}

impl VerifiedSdkSource {
    /// Exact independently selected SDK product identity.
    #[must_use]
    pub const fn identity(&self) -> &SdkIdentity {
        &self.request.identity
    }

    /// Measured source profile whose sealed contract agrees with the identity.
    #[must_use]
    pub const fn source_profile(&self) -> &TargetProfile {
        &self.profile
    }

    /// Canonical engine-free source root, not an SDK build output.
    #[must_use]
    pub fn materialized_source_root(&self) -> &std::path::Path {
        &self.request.materialized_source_root
    }

    /// Reaudit the exact original/projected inputs before later local use.
    ///
    /// # Errors
    /// Rejects changed source inputs or profile selection. No files are written.
    pub fn revalidate(&self) -> Result<(), ContractError> {
        let current = verify_sdk_source(&self.request)?;
        if current.profile != self.profile {
            return Err(ContractError::identity(
                "verified SDK source profile changed",
            ));
        }
        Ok(())
    }
}

/// Verify a source-owned SDK descriptor without executing any source programs.
///
/// `identity.source.content_sha256` is the normalized engine-free projection
/// digest, whereas commit/tree bind the complete original checkout. The sealed
/// v2 consumer contract must declare ordinary SDK link probes. Its exact raw
/// digest and all scalar ABI facts must match the selected descriptor.
///
/// # Errors
/// Returns a contract error for malformed identity, unknown/mismatched profile,
/// unsealed/legacy contract, divergent ABI or dirty/changed source material.
/// This operation neither creates build roots nor packages SDK bytes.
pub fn verify_sdk_source(
    request: &SdkSourceVerificationRequest,
) -> Result<VerifiedSdkSource, ContractError> {
    request
        .identity
        .validate()
        .map_err(ContractError::identity)?;
    let original = inspection::directory(&request.source_root)?;
    let projection = inspection::directory(&request.materialized_source_root)?;
    let profile_bytes = inspection::read(&projection.join("aros-targets.toml"))?;
    let profile_text = std::str::from_utf8(&profile_bytes)
        .map_err(|_| ContractError::identity("SDK source profile document is not UTF-8"))?;
    let profiles = TargetProfile::parse_config(profile_text, "aros-targets.toml")
        .map_err(|_| ContractError::identity("SDK source profile document is invalid"))?;
    let mut matching = profiles
        .targets
        .into_iter()
        .filter(|p| p.name == request.source_preset);
    let profile = matching
        .next()
        .ok_or_else(|| ContractError::identity("SDK source preset is absent"))?;
    if matching.next().is_some() {
        return Err(ContractError::identity("SDK source preset is ambiguous"));
    }
    let contract_path = profile.native_consumer_contract.as_deref().ok_or_else(|| {
        ContractError::identity("SDK source preset has no native consumer contract")
    })?;
    let loaded = load_bound_native_consumer_contract(
        &projection,
        std::path::Path::new(contract_path),
        &profile,
    )
    .map_err(|_| {
        ContractError::identity(
            "SDK source consumer contract is not sealed to its profile and inputs",
        )
    })?;
    loaded
        .contract
        .require_native_sdk_link_probes()
        .map_err(|_| {
            ContractError::identity("SDK source contract must declare v2 ordinary link probes")
        })?;
    if loaded.sha256 != request.identity.source.contract_sha256 {
        return Err(ContractError::identity(
            "SDK source contract differs from its selected digest",
        ));
    }
    // Project the closed source ABI representation, not a board-specific list.
    // serde's scalar enum rejects any future non-scalar field until explicitly
    // represented by the SDK contract, rather than silently dropping it.
    let abi: BTreeMap<String, SdkAbiValue> = serde_json::from_value(
        serde_json::to_value(&loaded.contract.abi)
            .map_err(|_| ContractError::identity("cannot encode SDK source ABI"))?,
    )
    .map_err(|_| ContractError::identity("SDK source ABI contains unsupported nonscalar facts"))?;
    if request.identity.target.target_triple != loaded.contract.abi.target_triple
        || request.identity.target.abi != abi
    {
        return Err(ContractError::identity(
            "SDK target or ABI differs from its source contract",
        ));
    }
    // Run the full raw-Git audit after reading the source descriptors. A valid
    // descriptor cannot bypass actual checkout/projection content verification.
    let verified = verify_engine_free_source(&EngineFreeSourceVerificationRequest {
        source_root: original.clone(),
        expected_source: CommittedSourceIdentity {
            commit: GitObjectId::try_from(request.identity.source.commit.clone())
                .map_err(ContractError::identity)?,
            tree: GitObjectId::try_from(request.identity.source.git_tree.clone())
                .map_err(ContractError::identity)?,
        },
        materialized_source_root: projection,
    })?;
    if verified.source_tree_sha256 != request.identity.source.content_sha256 {
        return Err(ContractError::identity(
            "SDK source content differs from its selected engine-free digest",
        ));
    }
    Ok(VerifiedSdkSource {
        request: SdkSourceVerificationRequest {
            source_root: original,
            materialized_source_root: verified.root,
            source_preset: request.source_preset.clone(),
            identity: request.identity.clone(),
        },
        profile,
    })
}
