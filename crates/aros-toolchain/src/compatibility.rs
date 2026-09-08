//! Tools-owned compatibility-probe preparation.
//!
//! This M5 building block prepares the two identities a later compatibility
//! runner is allowed to use: a fresh materialization of the embedded CMake
//! engine and one exact directory of helpers built from the selected tools
//! snapshot. It deliberately does not execute CMake, `configure`, `make`, a
//! compiler, a Python generator, or a release operation. Those later probe
//! phases consume this checked preparation rather than selecting their own
//! source-tree engine or shared Cargo target directory.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use aros_common::{open_regular_file_nofollow, sha256_reader, Sha256Digest};

use crate::filesystem::open_directory;
use crate::ContractError;

/// Helpers a compatibility probe must resolve from one exact fresh target root.
pub const REQUIRED_HELPERS: &[&str] = &[
    "aros-transpiler",
    "aros-genmodule",
    "aros-collect",
    "aros-ahi-runner",
    "aros-fetch",
];

const ENGINE_DIRECTORY: &str = "aros-cmake-engine";
const MAX_ENGINE_ENTRIES: usize = 4_096;
const MAX_ENGINE_DEPTH: usize = 32;

/// Explicit roots for preparing a tools-owned compatibility probe.
#[derive(Debug, Clone)]
pub struct CompatibilityPreparationRequest {
    /// Isolated source tree used by a later probe. It must not carry `cmake/`.
    pub source_root: PathBuf,
    /// Existing owned work root; this operation creates one fresh engine leaf.
    pub work_root: PathBuf,
    /// Fresh Cargo target's `release/` directory for the exact tools snapshot.
    pub helpers_root: PathBuf,
}

/// One measured helper selected from the exact preparation root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelperIdentity {
    /// Absolute helper path below the selected helper root.
    pub path: PathBuf,
    /// Measured helper SHA-256.
    pub sha256: Sha256Digest,
    /// Measured helper byte length.
    pub size: u64,
}

/// Verified tools-owned engine and helper identities for a later probe runner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompatibilityPreparation {
    /// Fresh materialized engine root, distinct from the source tree.
    pub engine_root: PathBuf,
    /// Embedded engine API version selected by this tools build.
    pub engine_api_version: u32,
    /// Digest of the embedded engine selected by this tools build.
    pub engine_sha256: Sha256Digest,
    /// Every required helper, keyed by its fixed executable name.
    pub helpers: BTreeMap<String, HelperIdentity>,
}

/// Prepare a fresh tools-owned engine and resolve every required helper.
///
/// The selected source tree must be an engine-free probe input: a top-level
/// `cmake/` file, directory or link is rejected. `work_root` and
/// `helpers_root` must be distinct absolute real directories. The fixed engine
/// destination `work_root/aros-cmake-engine` must be absent; it is never
/// adopted, repaired, or overwritten. Every embedded file is read back and
/// compared with the compiled-in resource after materialization, and every
/// helper must be a regular executable immediate child of `helpers_root`.
///
/// # Errors
///
/// Returns AX0703 for unsafe, mixed, stale or incomplete engine/helper inputs.
/// It performs no process, network, cache, source-tree, tag or publication
/// operation. Failures retain `work_root` and any newly materialized engine for
/// inspection; this function never removes caller data.
pub fn prepare(
    request: &CompatibilityPreparationRequest,
) -> Result<CompatibilityPreparation, ContractError> {
    let source_root = checked_directory(&request.source_root, "compatibility source root")?;
    let work_root = checked_directory(&request.work_root, "compatibility work root")?;
    let helpers_root = checked_directory(&request.helpers_root, "compatibility helpers root")?;
    if source_root == work_root || source_root == helpers_root || work_root == helpers_root {
        return Err(ContractError::compatibility(
            "compatibility source, work, and helper roots must be distinct",
        ));
    }
    reject_source_tree_engine(&source_root)?;

    let engine_root = work_root.join(ENGINE_DIRECTORY);
    if engine_root.exists() || fs::symlink_metadata(&engine_root).is_ok() {
        return Err(ContractError::compatibility(
            "compatibility engine destination already exists and cannot be adopted",
        ));
    }
    fs::create_dir(&engine_root).map_err(|_| {
        ContractError::compatibility("cannot create the fresh compatibility engine destination")
    })?;
    let placement = aros_cmake_engine::materialize(&engine_root).map_err(|_| {
        ContractError::compatibility("cannot materialize the embedded tools-owned CMake engine")
    })?;
    if placement.root != engine_root || placement.reused || placement.removed != 0 {
        return Err(ContractError::compatibility(
            "fresh compatibility engine materialization reported unexpected prior state",
        ));
    }
    let engine_sha256 = Sha256Digest::parse(aros_cmake_engine::digest()).map_err(|_| {
        ContractError::compatibility("embedded CMake engine exposes an invalid compiled digest")
    })?;
    verify_materialized_engine(&engine_root, &engine_sha256)?;

    let helpers = resolve_helpers(&helpers_root)?;
    Ok(CompatibilityPreparation {
        engine_root,
        engine_api_version: aros_cmake_engine::api_version(),
        engine_sha256,
        helpers,
    })
}

fn checked_directory(path: &Path, label: &str) -> Result<PathBuf, ContractError> {
    if !path.is_absolute() {
        return Err(ContractError::compatibility(format!(
            "{label} must be an absolute directory"
        )));
    }
    let canonical = path.canonicalize().map_err(|_| {
        ContractError::compatibility(format!("{label} cannot be canonicalized after validation"))
    })?;
    if open_directory(&canonical).is_err() {
        return Err(ContractError::compatibility(format!(
            "{label} does not resolve to a real directory without symlink ancestors"
        )));
    }
    Ok(canonical)
}

fn reject_source_tree_engine(source_root: &Path) -> Result<(), ContractError> {
    let source_engine = source_root.join("cmake");
    if fs::symlink_metadata(&source_engine).is_ok() {
        return Err(ContractError::compatibility(
            "compatibility source tree must not contain a copied CMake engine",
        ));
    }
    Ok(())
}

fn verify_materialized_engine(
    engine_root: &Path,
    expected_digest: &Sha256Digest,
) -> Result<(), ContractError> {
    let actual_digest = Sha256Digest::parse(aros_cmake_engine::digest()).map_err(|_| {
        ContractError::compatibility("embedded CMake engine exposes an invalid compiled digest")
    })?;
    if &actual_digest != expected_digest {
        return Err(ContractError::compatibility(
            "embedded CMake engine digest changed during compatibility preparation",
        ));
    }
    let mut expected = BTreeSet::new();
    for relative in aros_cmake_engine::paths() {
        let path = engine_root.join(relative);
        let metadata = fs::symlink_metadata(&path).map_err(|_| {
            ContractError::compatibility("materialized CMake engine is missing an embedded file")
        })?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(ContractError::compatibility(
                "materialized CMake engine contains a non-regular embedded file",
            ));
        }
        let expected_contents = aros_cmake_engine::file(relative).ok_or_else(|| {
            ContractError::compatibility("embedded CMake engine file table changed unexpectedly")
        })?;
        let expected_len = u64::try_from(expected_contents.len()).map_err(|_| {
            ContractError::compatibility("embedded CMake engine file length is not representable")
        })?;
        if metadata.len() != expected_len {
            return Err(ContractError::compatibility(
                "materialized CMake engine file length differs from the embedded tools resource",
            ));
        }
        let contents = fs::read(&path).map_err(|_| {
            ContractError::compatibility("cannot read back a materialized CMake engine file")
        })?;
        if contents != expected_contents.as_bytes() {
            return Err(ContractError::compatibility(
                "materialized CMake engine file differs from the embedded tools resource",
            ));
        }
        expected.insert(relative.to_owned());
    }
    expected.insert(aros_cmake_engine::STAMP_FILE.to_owned());
    let actual = collect_regular_relative_paths(
        engine_root,
        Path::new(""),
        &mut EngineInventoryBudget::default(),
    )?;
    if actual != expected {
        return Err(ContractError::compatibility(
            "materialized CMake engine contains missing, foreign, or unsafe files",
        ));
    }
    let stamp =
        fs::read_to_string(engine_root.join(aros_cmake_engine::STAMP_FILE)).map_err(|_| {
            ContractError::compatibility("cannot read the materialized CMake engine stamp")
        })?;
    if stamp != format!("{}\n", expected_digest.as_str()) {
        return Err(ContractError::compatibility(
            "materialized CMake engine stamp does not bind the embedded digest",
        ));
    }
    Ok(())
}

fn collect_regular_relative_paths(
    root: &Path,
    relative: &Path,
    budget: &mut EngineInventoryBudget,
) -> Result<BTreeSet<String>, ContractError> {
    if relative.components().count() > MAX_ENGINE_DEPTH {
        return Err(ContractError::compatibility(
            "materialized CMake engine exceeds the configured directory-depth limit",
        ));
    }
    let directory = root.join(relative);
    let mut paths = BTreeSet::new();
    for entry in fs::read_dir(&directory).map_err(|_| {
        ContractError::compatibility("cannot enumerate the materialized CMake engine")
    })? {
        let entry = entry.map_err(|_| {
            ContractError::compatibility("cannot read a materialized CMake engine directory entry")
        })?;
        budget.account()?;
        let name = entry.file_name().into_string().map_err(|_| {
            ContractError::compatibility("materialized CMake engine contains a non-UTF-8 path")
        })?;
        let next_relative = relative.join(&name);
        let metadata = fs::symlink_metadata(entry.path()).map_err(|_| {
            ContractError::compatibility("cannot inspect a materialized CMake engine entry")
        })?;
        if metadata.file_type().is_symlink() {
            return Err(ContractError::compatibility(
                "materialized CMake engine contains a symbolic link",
            ));
        }
        if metadata.is_dir() {
            paths.extend(collect_regular_relative_paths(
                root,
                &next_relative,
                budget,
            )?);
        } else if metadata.is_file() {
            let relative = next_relative.to_str().ok_or_else(|| {
                ContractError::compatibility("materialized CMake engine path is not UTF-8")
            })?;
            paths.insert(relative.to_owned());
        } else {
            return Err(ContractError::compatibility(
                "materialized CMake engine contains a non-regular entry",
            ));
        }
    }
    Ok(paths)
}

#[derive(Default)]
struct EngineInventoryBudget {
    entries: usize,
}

impl EngineInventoryBudget {
    fn account(&mut self) -> Result<(), ContractError> {
        self.entries = self.entries.checked_add(1).ok_or_else(|| {
            ContractError::compatibility("materialized CMake engine entry count overflowed")
        })?;
        if self.entries > MAX_ENGINE_ENTRIES {
            return Err(ContractError::compatibility(
                "materialized CMake engine exceeds the configured entry limit",
            ));
        }
        Ok(())
    }
}

fn resolve_helpers(root: &Path) -> Result<BTreeMap<String, HelperIdentity>, ContractError> {
    let mut helpers = BTreeMap::new();
    for name in REQUIRED_HELPERS {
        let path = root.join(name);
        let metadata = fs::symlink_metadata(&path).map_err(|_| {
            ContractError::compatibility(
                "compatibility helper is missing from the exact target root",
            )
        })?;
        let executable = metadata.permissions().mode() & 0o111 != 0;
        if !metadata.is_file() || metadata.file_type().is_symlink() || !executable {
            return Err(ContractError::compatibility(
                "compatibility helper is not a regular executable from the exact target root",
            ));
        }
        let mut file = open_regular_file_nofollow(&path).map_err(|_| {
            ContractError::compatibility(
                "cannot safely open a compatibility helper from the target root",
            )
        })?;
        let opened_metadata = file.metadata().map_err(|_| {
            ContractError::compatibility("cannot inspect an opened compatibility helper")
        })?;
        if !opened_metadata.is_file() || opened_metadata.len() != metadata.len() {
            return Err(ContractError::compatibility(
                "compatibility helper changed while it was opened for measurement",
            ));
        }
        let measured = sha256_reader(&mut file.by_ref()).map_err(|_| {
            ContractError::compatibility(
                "cannot measure a compatibility helper from the target root",
            )
        })?;
        if measured.size != opened_metadata.len() {
            return Err(ContractError::compatibility(
                "compatibility helper changed while it was measured",
            ));
        }
        if measured.size == 0 {
            return Err(ContractError::compatibility(
                "compatibility helper from the exact target root is empty",
            ));
        }
        helpers.insert(
            (*name).to_owned(),
            HelperIdentity {
                path,
                sha256: measured.digest,
                size: measured.size,
            },
        );
    }
    Ok(helpers)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt as _;

    use aros_common::DiagnosticCode;

    use super::{
        prepare, verify_materialized_engine, CompatibilityPreparationRequest, REQUIRED_HELPERS,
    };

    fn request(root: &std::path::Path) -> CompatibilityPreparationRequest {
        let source_root = root.join("source");
        let work_root = root.join("work");
        let helpers_root = root.join("helpers");
        for directory in [&source_root, &work_root, &helpers_root] {
            fs::create_dir(directory).unwrap();
        }
        for helper in REQUIRED_HELPERS {
            let path = helpers_root.join(helper);
            fs::write(&path, b"fixture helper\n").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        CompatibilityPreparationRequest {
            source_root,
            work_root,
            helpers_root,
        }
    }

    #[test]
    fn preparation_materializes_only_the_embedded_engine_and_exact_helpers() {
        let temporary = tempfile::tempdir().unwrap();
        let request = request(temporary.path());
        let prepared = prepare(&request).unwrap();

        assert!(prepared.engine_root.join("CMakeLists.txt").is_file());
        assert!(prepared.engine_root.join("AROS.cmake").is_file());
        assert_eq!(prepared.helpers.len(), REQUIRED_HELPERS.len());
        let helpers_root = request.helpers_root.canonicalize().unwrap();
        assert!(prepared
            .helpers
            .values()
            .all(|helper| helper.path.starts_with(&helpers_root)));
        assert!(!request.source_root.join("cmake").exists());
    }

    #[test]
    fn preparation_rejects_source_engine_reused_destination_and_bad_helper() {
        let temporary = tempfile::tempdir().unwrap();
        let request = request(temporary.path());
        fs::create_dir(request.source_root.join("cmake")).unwrap();
        let source_engine_error = prepare(&request).unwrap_err();
        assert_compatibility(&source_engine_error);
        fs::remove_dir(request.source_root.join("cmake")).unwrap();

        fs::create_dir(request.work_root.join("aros-cmake-engine")).unwrap();
        let reused_engine_error = prepare(&request).unwrap_err();
        assert_compatibility(&reused_engine_error);
        fs::remove_dir(request.work_root.join("aros-cmake-engine")).unwrap();

        let helper = request.helpers_root.join("aros-fetch");
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o600)).unwrap();
        let error = prepare(&request).unwrap_err();
        assert_compatibility(&error);
    }

    #[test]
    fn materialized_engine_rejects_foreign_or_linked_entries() {
        let temporary = tempfile::tempdir().unwrap();
        let engine = temporary.path().join("engine");
        fs::create_dir(&engine).unwrap();
        aros_cmake_engine::materialize(&engine).unwrap();
        let digest = aros_common::Sha256Digest::parse(aros_cmake_engine::digest()).unwrap();
        fs::write(engine.join("foreign.cmake"), b"unexpected\n").unwrap();
        let error = verify_materialized_engine(&engine, &digest).unwrap_err();
        assert_compatibility(&error);
    }

    fn assert_compatibility(error: &crate::ContractError) {
        assert_eq!(
            error.diagnostics().diagnostics[0].code,
            DiagnosticCode::ProducerCompatibility
        );
    }
}
