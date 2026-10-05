//! Bind partition construction to a revalidated checkout-owned source contract.

use std::io::Read;
use std::path::Path;

use serde::Serialize;

use super::{encode_esp_partition_source, EspPartitionArtifact};
use crate::native_build_contract::{load_bound_native_build_contract, NativeBuildInput};
use crate::{open_regular_file_nofollow, sha256_bytes, Sha256Digest, TargetProfile};

/// Local source consistency observations for one generated partition table.
/// This is not Git/release provenance, a native build-set or flash permission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NativePartitionBinding {
    pub schema: &'static str,
    pub qualification: &'static str,
    pub profile: String,
    pub board: String,
    /// A declaration in the selected source contract, not a verified Git OID.
    pub declared_source_baseline: String,
    pub source_contract_sha256: Sha256Digest,
    pub contract_inputs: Vec<NativeBuildInput>,
    /// Hash of the caller's parsed selector, not the raw targets file.
    pub resolved_profile_sha256: Sha256Digest,
    pub partition_source: String,
    pub partition_source_sha256: Sha256Digest,
    /// Geometry remains explicit caller input until the media adapter qualifies it.
    pub geometry_origin: &'static str,
    pub table_offset: u32,
    pub flash_bytes: u64,
    pub artifact_sha256: Sha256Digest,
    pub artifact_size: usize,
}

/// In-memory artifact and its exact local source-consistency binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativePartitionArtifact {
    pub artifact: EspPartitionArtifact,
    pub binding: NativePartitionBinding,
}

/// Generate a table only from the inventoried CSV of the expected source contract.
///
/// The entire contract/input inventory is validated before and after encoding.
/// The CSV is independently read with no-follow traversal and a 64 KiB limit;
/// its raw bytes must still match the declared input. No source script or Make
/// is executed and no artifact is published. Numeric geometry is explicit,
/// unqualified caller input; this does not infer the board's flash layout.
///
/// # Errors
/// Rejects altered contracts/inputs, unsafe files, oversized or invalid CSV,
/// mismatched selectors, and invalid explicit geometry.
pub fn encode_bound_native_partition_table(
    source_root: &Path,
    relative_contract: &Path,
    profile: &TargetProfile,
    expected_contract_sha256: &Sha256Digest,
    table_offset: u32,
    flash_bytes: u64,
) -> Result<NativePartitionArtifact, String> {
    let loaded = load_bound_native_build_contract(source_root, relative_contract, profile)
        .map_err(|error| error.to_string())?;
    if &loaded.sha256 != expected_contract_sha256 {
        return Err("native partition source contract SHA-256 differs".into());
    }
    let input = loaded
        .contract
        .inputs
        .iter()
        .find(|input| input.path == loaded.contract.media.partition_table)
        .ok_or("partition source is not in the validated contract inventory")?;
    // Canonicalize the explicit root, not an input path: no-follow opening must
    // still see every original CSV component and reject in-tree symlinks too.
    let root = source_root
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let mut file = open_regular_file_nofollow(&root.join(&input.path))
        .map_err(|error| format!("cannot open bound partition source: {error}"))?;
    if file.metadata().map_err(|error| error.to_string())?.len() > 64 * 1024 {
        return Err("bound partition source exceeds its byte budget".into());
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(64 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > 64 * 1024 {
        return Err("bound partition source exceeds its byte budget".into());
    }
    let artifact = encode_esp_partition_source(&bytes, &input.sha256, table_offset, flash_bytes)?;
    let revalidated = load_bound_native_build_contract(source_root, relative_contract, profile)
        .map_err(|error| error.to_string())?;
    if revalidated != loaded {
        return Err("native partition source contract changed during generation".into());
    }
    let binding = NativePartitionBinding {
        schema: "aros-native-partition-binding-v1",
        qualification: "experimental-source-consistency-only",
        profile: loaded.contract.profile,
        board: loaded.contract.board,
        declared_source_baseline: loaded.contract.source_baseline,
        source_contract_sha256: loaded.sha256,
        contract_inputs: loaded.contract.inputs.clone(),
        resolved_profile_sha256: sha256_bytes(
            &serde_json::to_vec(profile).map_err(|error| error.to_string())?,
        ),
        partition_source: input.path.clone(),
        partition_source_sha256: input.sha256.clone(),
        geometry_origin: "explicit-caller-input-unqualified",
        table_offset,
        flash_bytes,
        artifact_sha256: artifact.sha256.clone(),
        artifact_size: artifact.bytes.len(),
    };
    Ok(NativePartitionArtifact { artifact, binding })
}
