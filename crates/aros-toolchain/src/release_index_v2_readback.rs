//! Bounded local read-back for a complete compiler-family release inventory.
//!
//! This boundary measures and verifies every indexed native package against
//! validated release inputs and a validated v2 index. It does not qualify the
//! complete release for publication.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use aros_common::{
    open_regular_file_nofollow, sha256_bytes, AROS_TOOLCHAIN_MANIFEST_FILE,
    AROS_TOOLCHAIN_MANIFEST_SCHEMA_V2,
};
use serde_json::{Map, Value};

use crate::package::PackageFormat;
use crate::package_verify::{PackageAssetPaths, PackageVerificationRequest, VerifiedPackage};
use crate::release_index_v2::{NativeReleaseIndexV2, MANIFEST_SCHEMA_NAME, TREE_FIXTURE_NAME};
use crate::release_inputs::ReleaseInputs;
use crate::ContractError;

pub(crate) const MAX_METADATA_BYTES: u64 = 16 * 1024 * 1024;
const MANIFEST_SCHEMA_BYTES: &[u8] =
    include_bytes!("../../aros-common/tests/fixtures/toolchain-manifest-v2.schema.json");
const TREE_FIXTURE_BYTES: &[u8] =
    include_bytes!("../../aros-common/tests/fixtures/tree-digest-v1.fixture.json");

/// Inputs for bounded read-back of one complete indexed release directory.
///
/// Build-environment receipts are supplied independently of archive contents
/// and must have exactly one entry for each canonical archive basename.
#[derive(Debug, Clone)]
pub struct IndexedPackageReadbackRequestV2 {
    /// Absolute directory containing the exact index-derived flat inventory.
    pub directory: PathBuf,
    /// Validated release-input collection and all bound source documents.
    pub inputs: ReleaseInputs,
    /// Validated release index, rebound to `inputs` during read-back.
    pub index: NativeReleaseIndexV2,
    /// Independently supplied build-environment receipt for each archive.
    pub build_environments: BTreeMap<String, Map<String, Value>>,
    /// Absolute build roots forbidden in archive regular-file contents.
    pub forbidden_prefixes: Vec<PathBuf>,
}

/// Fully completed read-back of all indexed native packages.
#[derive(Debug, Clone)]
pub struct IndexedPackageReadbackV2 {
    packages: Vec<IndexedPackageV2>,
}

impl IndexedPackageReadbackV2 {
    /// Verified packages in canonical archive-name order.
    #[must_use]
    pub fn packages(&self) -> &[IndexedPackageV2] {
        &self.packages
    }
}

/// One measured and verified package from an indexed release.
#[derive(Debug, Clone)]
pub struct IndexedPackageV2 {
    asset: String,
    package: VerifiedPackage,
}

impl IndexedPackageV2 {
    /// Canonical archive basename declared by the index.
    #[must_use]
    pub fn asset(&self) -> &str {
        &self.asset
    }

    /// Measured archive identity and validated external manifest.
    #[must_use]
    pub const fn package(&self) -> &VerifiedPackage {
        &self.package
    }
}

/// Measure and verify every native archive in a complete indexed inventory.
///
/// This operation reads in place and never replaces or writes a release file.
/// It rebinds the supplied index to the supplied inputs, checks the exact flat
/// inventory before and after package verification, and applies the strict
/// compiler-family-v2 package verifier to every artifact.
///
/// Success verifies package archives and their package metadata against the
/// index and exact input documents. It does not establish release publication
/// readiness or qualify every other support file in the inventory.
///
/// # Errors
///
/// Returns a sanitized index-contract error for an unsafe directory, an
/// incomplete or changing inventory, mismatched input/index bytes, or any
/// package that fails bounded read-back verification.
pub fn readback_indexed_packages(
    request: &IndexedPackageReadbackRequestV2,
) -> Result<IndexedPackageReadbackV2, ContractError> {
    readback_indexed_packages_with_inventory(request, request.index.expected_inventory())
}

// Only internal stage writers may select an intermediate inventory. The public
// read-back contract continues to require the complete final inventory.
pub(crate) fn readback_indexed_packages_with_inventory(
    request: &IndexedPackageReadbackRequestV2,
    inventory: &BTreeSet<String>,
) -> Result<IndexedPackageReadbackV2, ContractError> {
    validate_directory_path(&request.directory)?;

    let rebound = NativeReleaseIndexV2::parse(
        &request
            .index
            .to_json_bytes()
            .map_err(|_| index_error("validated release index cannot be rebound"))?,
        &request.inputs,
    )
    .map_err(|_| index_error("release index does not bind to the supplied release inputs"))?;
    if rebound != request.index {
        return Err(index_error(
            "release index does not bind to the supplied release inputs",
        ));
    }

    require_exact_inventory(&request.directory, inventory)?;
    verify_input_collection(&request.directory, &request.inputs)?;
    verify_bound_documents(&request.directory, &request.inputs)?;
    verify_on_disk_index(request)?;
    verify_static_support(&request.directory)?;
    verify_build_environment_keys(request)?;
    crate::release_index_v2_builder::validate_request_material(
        &request.build_environments,
        &request.forbidden_prefixes,
    )?;

    let mut packages = Vec::with_capacity(request.index.artifacts().len());
    for artifact in request.index.artifacts() {
        let group = request
            .inputs
            .groups()
            .iter()
            .find(|group| group.id() == artifact.group_id())
            .ok_or_else(|| index_error("release index references an unbound input group"))?;
        let profile = group
            .profiles()
            .select(artifact.target_profile())
            .map_err(|_| index_error("release index references an unbound input profile"))?;
        let build_environment = request
            .build_environments
            .get(artifact.asset())
            .ok_or_else(|| index_error("build-environment map does not match indexed archives"))?;
        let verification_request = PackageVerificationRequest {
            package_dir: request.directory.clone(),
            release_id: request.index.release_id().to_owned(),
            host: artifact.host().to_owned(),
            recipe: group.recipe().clone(),
            source_lock: group.source_lock().clone(),
            profile: profile.clone(),
            build_environment: build_environment.clone(),
            forbidden_prefixes: request.forbidden_prefixes.clone(),
        };
        let paths = PackageAssetPaths::for_asset(&request.directory, artifact.asset());
        let package = crate::package_verify::verify_members_with_format(
            &verification_request,
            &paths,
            PackageFormat::CompilerFamilyV2,
        )
        .map_err(|_| index_error("native compiler-family package failed read-back verification"))?;

        compare_package_with_index(&package, artifact, request.index.release_id())?;
        verify_required_paths(&package, artifact.required_paths())?;
        packages.push(IndexedPackageV2 {
            asset: artifact.asset().to_owned(),
            package,
        });
    }

    validate_directory_path(&request.directory)?;
    require_exact_inventory(&request.directory, inventory)?;
    verify_static_support(&request.directory)?;
    Ok(IndexedPackageReadbackV2 { packages })
}

pub(crate) fn verify_static_support(directory: &Path) -> Result<(), ContractError> {
    for (name, expected) in [
        (MANIFEST_SCHEMA_NAME, MANIFEST_SCHEMA_BYTES),
        (TREE_FIXTURE_NAME, TREE_FIXTURE_BYTES),
    ] {
        let actual =
            read_bounded_regular_file(&directory.join(name), "static release support fixture")?;
        if actual != expected {
            return Err(index_error(
                "on-disk static release support differs from the tools runtime fixture bytes",
            ));
        }
    }
    Ok(())
}

pub(crate) fn verify_input_collection(
    directory: &Path,
    inputs: &ReleaseInputs,
) -> Result<(), ContractError> {
    let bytes = read_bounded_regular_file(
        &directory.join("toolchain-release-inputs-v2.json"),
        "release-input collection",
    )?;
    if &sha256_bytes(&bytes) != inputs.collection_sha256() {
        return Err(index_error(
            "on-disk release-input collection differs from the bound collection bytes",
        ));
    }
    Ok(())
}

pub(crate) fn verify_bound_documents(
    directory: &Path,
    inputs: &ReleaseInputs,
) -> Result<(), ContractError> {
    for group in inputs.groups() {
        for (reference, expected) in [
            (group.recipe_reference(), group.recipe_bytes()),
            (group.source_lock_reference(), group.source_lock_bytes()),
            (group.profiles_reference(), group.profiles_bytes()),
        ] {
            let actual = read_bounded_regular_file(
                &directory.join(reference.file()),
                "bound release-input document",
            )?;
            if actual != expected {
                return Err(index_error(
                    "on-disk release-input document differs from its bound exact bytes",
                ));
            }
        }
    }
    Ok(())
}

fn verify_on_disk_index(request: &IndexedPackageReadbackRequestV2) -> Result<(), ContractError> {
    let bytes = read_bounded_regular_file(
        &request.directory.join("toolchain-index-v2.json"),
        "release index",
    )?;
    let parsed = NativeReleaseIndexV2::parse(&bytes, &request.inputs)
        .map_err(|_| index_error("on-disk release index is invalid for the supplied inputs"))?;
    if parsed != request.index {
        return Err(index_error(
            "on-disk release index differs from the supplied validated index",
        ));
    }
    Ok(())
}

fn verify_build_environment_keys(
    request: &IndexedPackageReadbackRequestV2,
) -> Result<(), ContractError> {
    // Both sides are already in canonical asset order. Reject the cardinality
    // before inspecting untrusted keys, then compare borrowed spellings without
    // duplicating an unbounded caller-supplied map or long rejected key.
    if request.build_environments.len() != request.index.artifacts().len()
        || !request
            .build_environments
            .keys()
            .map(String::as_str)
            .eq(request
                .index
                .artifacts()
                .iter()
                .map(crate::release_index_v2::NativeReleaseArtifactV2::asset))
    {
        return Err(index_error(
            "build-environment map must contain exactly the canonical indexed archives",
        ));
    }
    Ok(())
}

fn compare_package_with_index(
    package: &VerifiedPackage,
    artifact: &crate::release_index_v2::NativeReleaseArtifactV2,
    release_id: &str,
) -> Result<(), ContractError> {
    let manifest = &package.manifest;
    if &package.archive_sha256 != artifact.sha256()
        || package.archive_size != artifact.size()
        || manifest.schema != AROS_TOOLCHAIN_MANIFEST_SCHEMA_V2
        || manifest.llvm_version.is_some()
        || manifest.release_id != release_id
        || manifest.host != artifact.host()
        || manifest.target_profile != artifact.target_profile()
        || manifest.target_triple != artifact.target_triple()
        || manifest.source_commit != artifact.source_commit().as_str()
        || manifest.compiler.as_ref() != Some(artifact.compiler())
        || manifest.tree_sha256 != artifact.tree_sha256().as_str()
    {
        return Err(index_error(
            "measured package identity differs from its indexed artifact",
        ));
    }
    Ok(())
}

pub(crate) fn verify_required_paths(
    package: &VerifiedPackage,
    required_paths: &[String],
) -> Result<(), ContractError> {
    let inventory = package
        .manifest
        .files
        .iter()
        .map(|entry| (entry.path.as_str(), entry.kind.as_str()))
        .collect::<BTreeMap<_, _>>();
    for path in required_paths {
        if path == AROS_TOOLCHAIN_MANIFEST_FILE {
            continue;
        }
        if !matches!(inventory.get(path.as_str()), Some(&"file" | &"symlink")) {
            return Err(index_error(
                "indexed required path is absent or is not a file or symlink",
            ));
        }
    }
    Ok(())
}

pub(crate) fn validate_directory_path(directory: &Path) -> Result<(), ContractError> {
    if !directory.is_absolute() || has_dot_component(directory) {
        return Err(index_error(
            "release directory must be absolute and contain no dot components",
        ));
    }

    let mut current = PathBuf::new();
    for component in directory.components() {
        match component {
            Component::RootDir => current.push(component.as_os_str()),
            Component::Normal(name) => current.push(name),
            Component::CurDir | Component::ParentDir | Component::Prefix(_) => {
                return Err(index_error(
                    "release directory must be absolute and contain no dot components",
                ));
            }
        }
        let metadata = fs::symlink_metadata(&current).map_err(|_| {
            index_error("release directory or one of its ancestors is inaccessible")
        })?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(index_error(
                "release directory and every ancestor must be real directories",
            ));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn has_dot_component(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt as _;

    path.as_os_str()
        .as_bytes()
        .split(|byte| *byte == b'/')
        .any(|component| component == b"." || component == b"..")
}

#[cfg(not(unix))]
fn has_dot_component(path: &Path) -> bool {
    path.components()
        .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
}

pub(crate) fn require_exact_inventory(
    directory: &Path,
    expected: &BTreeSet<String>,
) -> Result<(), ContractError> {
    let mut actual = BTreeSet::new();
    for entry in
        fs::read_dir(directory).map_err(|_| index_error("cannot enumerate release inventory"))?
    {
        let entry = entry.map_err(|_| index_error("cannot read release inventory member"))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| index_error("release inventory member is not UTF-8"))?;
        if !expected.contains(&name) {
            return Err(index_error(
                "release directory does not contain the exact indexed inventory",
            ));
        }
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|_| index_error("cannot inspect release inventory member"))?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(index_error(
                "release inventory contains a non-regular member",
            ));
        }
        actual.insert(name);
    }
    if &actual != expected {
        return Err(index_error(
            "release directory does not contain the exact indexed inventory",
        ));
    }
    Ok(())
}

pub(crate) fn read_bounded_regular_file(path: &Path, kind: &str) -> Result<Vec<u8>, ContractError> {
    let mut file = open_regular_file_nofollow(path)
        .map_err(|_| index_error(format!("cannot safely open {kind}")))?;
    let metadata = file
        .metadata()
        .map_err(|_| index_error(format!("cannot inspect {kind}")))?;
    if !metadata.is_file() || metadata.len() > MAX_METADATA_BYTES {
        return Err(index_error(format!(
            "{kind} exceeds its regular-file metadata bound"
        )));
    }
    let capacity = usize::try_from(metadata.len())
        .map_err(|_| index_error(format!("{kind} exceeds addressable memory")))?;
    let mut bytes = Vec::with_capacity(capacity);
    file.by_ref()
        .take(MAX_METADATA_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| index_error(format!("cannot read {kind}")))?;
    let size = u64::try_from(bytes.len())
        .map_err(|_| index_error(format!("{kind} exceeds its metadata bound")))?;
    if size > MAX_METADATA_BYTES || size != metadata.len() {
        return Err(index_error(format!("{kind} changed while it was read")));
    }
    Ok(bytes)
}

fn index_error(message: impl Into<String>) -> ContractError {
    ContractError::index(message)
}
