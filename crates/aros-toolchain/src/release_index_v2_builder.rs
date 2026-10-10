//! Measured, read-only index construction from the complete pre-index stage.
//!
//! This operation verifies packages rather than trusting caller-supplied hash
//! declarations. It does not write outputs or establish publication readiness.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde_json::{json, Map, Value};

use crate::package::PackageFormat;
use crate::package_verify::{PackageAssetPaths, PackageVerificationRequest};
use crate::release_index_v2::{
    expected_artifacts, expected_inventory_for_assets, safe_release_id, validate_base_url,
    validate_required_paths, NativeReleaseIndexV2, CHECKSUMS_NAME, INDEX_NAME, PROVENANCE_NAME,
};
use crate::release_index_v2_readback::{
    require_exact_inventory, validate_directory_path, verify_bound_documents,
    verify_input_collection, verify_required_paths, verify_static_support, MAX_METADATA_BYTES,
};
use crate::release_inputs::ReleaseInputs;
use crate::ContractError;

const MAX_ENVIRONMENT_DEPTH: usize = 64;
const MAX_FORBIDDEN_PREFIXES: usize = 64;
const MAX_FORBIDDEN_PREFIX_BYTES: usize = 16 * 1024;

/// Explicit inputs for read-only measurement of a complete pre-index stage.
#[derive(Debug, Clone)]
pub struct MeasuredReleaseIndexRequestV2 {
    /// Absolute real directory with input/support documents and package sets.
    /// Index, final checksums and provenance must not exist at this stage.
    pub directory: PathBuf,
    /// Validated collection and exact source documents to bind again on disk.
    pub inputs: ReleaseInputs,
    /// Safe immutable release identifier expected in every package manifest.
    pub release_id: String,
    /// Canonical credential-free HTTPS release download root.
    pub base_url: String,
    /// Independent expected environment material, keyed by canonical archive.
    pub build_environments: BTreeMap<String, Map<String, Value>>,
    /// Sorted safe required payload paths, keyed by canonical archive.
    pub required_paths: BTreeMap<String, Vec<String>>,
    /// Absolute build roots forbidden in regular archive payload contents.
    pub forbidden_prefixes: Vec<PathBuf>,
}

/// Build a complete v2 index from independently verified package bytes.
///
/// The exact pre-index inventory is the input-derived final inventory minus
/// the index, checksum document and provenance bundle. Every lane is verified
/// with the compiler-family-v2 package reader and its independent expected
/// environment. Archive hashes/sizes and payload tree digests come only from
/// that verification. Required paths must exist as payload files or symlinks.
/// Exact input documents and embedded tools support bytes are checked before
/// and after package measurement. Construction writes nothing and returns no
/// partial index. No ownership lock or concurrent-writer snapshot is acquired.
///
/// This is not compiler execution, A/B, relocation, provenance authentication,
/// evidence qualification or release admission. Later stages must write the
/// canonical index, bind evidence/provenance, generate checksums and perform
/// full independent final read-back.
///
/// # Errors
///
/// Returns a sanitized index-contract error for unsafe identity, incomplete or
/// unexpected files, inconsistent independent lane maps, unbound documents,
/// invalid packages or resource-limit violations.
pub fn build_measured_index_v2(
    request: &MeasuredReleaseIndexRequestV2,
) -> Result<NativeReleaseIndexV2, ContractError> {
    if !safe_release_id(&request.release_id) {
        return Err(ContractError::index(
            "measured release has an unsafe release identifier",
        ));
    }
    if request.base_url.len() as u64 > MAX_METADATA_BYTES {
        return Err(ContractError::index(
            "measured release base URL exceeds the metadata bound",
        ));
    }
    validate_base_url(&request.base_url)?;
    validate_directory_path(&request.directory)?;
    let lanes = expected_artifacts(&request.inputs)?;
    let expected_keys = lanes
        .iter()
        .map(|lane| lane.asset.as_str())
        .collect::<BTreeSet<_>>();
    if request.build_environments.len() != expected_keys.len()
        || request.required_paths.len() != expected_keys.len()
        || request
            .build_environments
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
            != expected_keys
        || request
            .required_paths
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
            != expected_keys
    {
        return Err(ContractError::index(
            "measured release lane maps must exactly cover input-derived canonical archives",
        ));
    }
    validate_request_material(&request.build_environments, &request.forbidden_prefixes)?;
    // Bound path material before cloning it into the output record. The final
    // serialized document is independently checked by the bounded parser.
    let mut path_bytes = 0_u64;
    for paths in request.required_paths.values() {
        validate_required_paths(paths)?;
        for path in paths {
            path_bytes = path_bytes.saturating_add(path.len() as u64 + 3);
            if path_bytes > MAX_METADATA_BYTES {
                return Err(ContractError::index(
                    "measured index paths exceed the metadata bound",
                ));
            }
        }
    }

    let mut inventory = expected_inventory_for_assets(
        &request.inputs,
        lanes.iter().map(|lane| lane.asset.as_str()),
    )?;
    for name in [INDEX_NAME, CHECKSUMS_NAME, PROVENANCE_NAME] {
        inventory.remove(name);
    }
    require_exact_inventory(&request.directory, &inventory)?;
    verify_input_collection(&request.directory, &request.inputs)?;
    verify_bound_documents(&request.directory, &request.inputs)?;
    verify_static_support(&request.directory)?;

    let mut artifacts = Vec::with_capacity(lanes.len());
    for lane in lanes {
        let group = request
            .inputs
            .groups()
            .iter()
            .find(|group| group.id() == lane.group_id)
            .ok_or_else(|| ContractError::index("measured lane has no bound group"))?;
        let profile = group
            .profiles()
            .select(&lane.target_profile)
            .map_err(|_| ContractError::index("measured lane has no bound profile"))?;
        let build_environment = request.build_environments.get(&lane.asset).ok_or_else(|| {
            ContractError::index("measured lane lacks an independent environment")
        })?;
        let required_paths = request
            .required_paths
            .get(&lane.asset)
            .ok_or_else(|| ContractError::index("measured lane lacks required paths"))?;
        let package_request = PackageVerificationRequest {
            package_dir: request.directory.clone(),
            release_id: request.release_id.clone(),
            host: lane.host.clone(),
            recipe: group.recipe().clone(),
            source_lock: group.source_lock().clone(),
            profile: profile.clone(),
            build_environment: build_environment.clone(),
            forbidden_prefixes: request.forbidden_prefixes.clone(),
        };
        let package = crate::package_verify::verify_members_with_format(
            &package_request,
            &PackageAssetPaths::for_asset(&request.directory, &lane.asset),
            PackageFormat::CompilerFamilyV2,
        )
        .map_err(|_| {
            ContractError::index("measured native package failed read-back verification")
        })?;
        verify_required_paths(&package, required_paths)?;
        artifacts.push(json!({
            "group_id": lane.group_id,
            "asset": lane.asset,
            "sha256": package.archive_sha256,
            "size": package.archive_size,
            "host": package.manifest.host,
            "target_profile": package.manifest.target_profile,
            "target_triple": package.manifest.target_triple,
            "source_commit": package.manifest.source_commit,
            "compiler": package.manifest.compiler,
            "tree_sha256": package.manifest.tree_sha256,
            "enabled": true,
            "strip_components": 1,
            "required_paths": required_paths,
        }));
    }
    let bytes = serde_json::to_vec(&json!({
        "schema": 2,
        "release_id": request.release_id,
        "base_url": request.base_url,
        "inputs_sha256": request.inputs.collection_sha256(),
        "producer_commit": request.inputs.producer_commit(),
        "tools_commit": request.inputs.tools_commit(),
        "artifacts": artifacts,
    }))
    .map_err(|_| ContractError::index("cannot encode the measured release index"))?;
    let index = NativeReleaseIndexV2::parse(&bytes, &request.inputs)?;

    validate_directory_path(&request.directory)?;
    require_exact_inventory(&request.directory, &inventory)?;
    verify_input_collection(&request.directory, &request.inputs)?;
    verify_bound_documents(&request.directory, &request.inputs)?;
    verify_static_support(&request.directory)?;
    Ok(index)
}

pub(crate) fn validate_request_material(
    build_environments: &BTreeMap<String, Map<String, Value>>,
    forbidden_prefixes: &[PathBuf],
) -> Result<(), ContractError> {
    let mut remaining = MAX_METADATA_BYTES;
    for environment in build_environments.values() {
        for (key, value) in environment {
            consume_environment_budget(&mut remaining, key.len())?;
            validate_environment_value(value, 0, &mut remaining)?;
        }
    }
    if forbidden_prefixes.len() > MAX_FORBIDDEN_PREFIXES {
        return Err(ContractError::index(
            "measured release forbidden-prefix count exceeds its bound",
        ));
    }
    let mut prefix_bytes = 0_usize;
    for prefix in forbidden_prefixes {
        prefix_bytes = prefix_bytes.saturating_add(prefix.as_os_str().len());
        if prefix_bytes > MAX_FORBIDDEN_PREFIX_BYTES {
            return Err(ContractError::index(
                "measured release forbidden-prefix bytes exceed their bound",
            ));
        }
    }
    Ok(())
}

fn consume_environment_budget(remaining: &mut u64, bytes: usize) -> Result<(), ContractError> {
    *remaining = remaining.checked_sub(bytes as u64).ok_or_else(|| {
        ContractError::index("measured release environments exceed their aggregate metadata bound")
    })?;
    Ok(())
}

fn validate_environment_value(
    value: &Value,
    depth: usize,
    remaining: &mut u64,
) -> Result<(), ContractError> {
    if depth > MAX_ENVIRONMENT_DEPTH {
        return Err(ContractError::index(
            "measured release environment nesting exceeds 64 levels",
        ));
    }
    // Account for nodes as well as string contents before any deep clone.
    consume_environment_budget(remaining, std::mem::size_of::<Value>())?;
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                consume_environment_budget(remaining, key.len())?;
                validate_environment_value(child, depth + 1, remaining)?;
            }
        }
        Value::Array(array) => {
            for child in array {
                validate_environment_value(child, depth + 1, remaining)?;
            }
        }
        Value::String(string) => consume_environment_budget(remaining, string.len())?,
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
    Ok(())
}
