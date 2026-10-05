//! Offline external-media preparation selected by a source-owned native contract.
//!
//! This creates only a fresh bootloader and partition table. It neither adopts
//! native core/BSP/Developer outputs nor constructs a complete flash plan, writes
//! a device, or confers release provenance on editable local byte locks.

use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use aros_common::native_media::{resolve_bound_native_media, NativeMediaBinding};
use aros_common::{
    create_unique_directory_nofollow, open_regular_file_nofollow, sha256_bytes,
    validate_private_directory_nofollow, CancellationToken, Sha256Digest, TargetProfile,
};
use aros_fetch::contract::{Cli, FetchRequest};
use aros_fetch::engine::source_receipt::{verify_prepared_source, VerifiedSourceReceipt};
use aros_verify::esp_image::{verify_esp_image, EspImageFacts, EspImagePolicy};
use aros_verify::esp_partition::{verify_esp_partition_table, EspPartitionFacts};
use serde::{Deserialize, Serialize};

use crate::idf_bootloader::{bind_idf_bootloader_lock, build_idf_bootloader, IdfBootloaderRequest};
use crate::wheel_environment::{
    bind_wheel_environment_lock, prepare_wheel_environment, WheelEnvironmentRequest,
};
use crate::ContractError;

const DOCUMENT_LIMIT: u64 = 1024 * 1024;

/// Explicit regular document and its expected raw-byte identity.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PinnedDocument {
    pub path: PathBuf,
    pub sha256: Sha256Digest,
}

/// One direct, ordered source patch; no shell fragments or archive alternatives.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedSourcePatch {
    pub name: String,
    pub subdirectory: Option<String>,
    pub options: Vec<String>,
    pub sha256: Sha256Digest,
}

/// Read-only receipt declaration. Origins are deliberately not provenance.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedSourceInput {
    pub destination: PathBuf,
    pub archive: String,
    pub suffix: String,
    pub sha256: Sha256Digest,
    pub patches: Vec<PreparedSourcePatch>,
}

impl PreparedSourceInput {
    /// Close the declaration through the actual fetch parser, without fetching.
    ///
    /// # Errors
    /// Rejects ambiguous names, unsupported options and changed local receipts.
    pub fn verify(&self) -> Result<VerifiedSourceReceipt, ContractError> {
        let request = self.request()?;
        verify_prepared_source(&request).map_err(|error| failure(error.to_string()))
    }

    fn request(&self) -> Result<FetchRequest, ContractError> {
        absolute_path(&self.destination)?;
        if self.patches.len() > 32 {
            return Err(failure("too many prepared source patches"));
        }
        let mut checksums = format!("{}.{}=sha256:{}", self.archive, self.suffix, self.sha256);
        let mut patches = Vec::new();
        for patch in &self.patches {
            // Preserve field boundaries; string interpolation must not allow
            // one JSON field to introduce another legacy parser declaration.
            for value in std::iter::once(&patch.name)
                .chain(patch.subdirectory.iter())
                .chain(patch.options.iter())
            {
                if value.is_empty()
                    || value.chars().any(char::is_whitespace)
                    || value.contains([':', ',', '\0'])
                {
                    return Err(failure("prepared patch contains a parser delimiter"));
                }
            }
            patches.push(format!(
                "{}:{}:{}",
                patch.name,
                patch.subdirectory.as_deref().unwrap_or_default(),
                patch.options.join(",")
            ));
            write!(checksums, " {}=sha256:{}", patch.name, patch.sha256)
                .expect("writing to a string cannot fail");
        }
        if self.suffix.is_empty() || self.suffix.chars().any(char::is_whitespace) {
            return Err(failure("one explicit archive suffix is required"));
        }
        FetchRequest::from_cli(&Cli {
            archive_origins: ".".into(),
            archive: self.archive.clone(),
            suffixes: self.suffix.clone(),
            destination: self.destination.clone(),
            patch_origins: ".".into(),
            patches: patches.join(" "),
            base: Some(self.destination.clone()),
            location: self.destination.clone(),
            rename_directory: None,
            checksums,
            force: false,
            offline: true,
            require_checksums: true,
            diagnostic_format: aros_common::DiagnosticFormat::Human,
            log_level: aros_common::LogLevel::Off,
            log_format: aros_common::LogFormat::Jsonl,
            log_file: None,
        })
        .map_err(|error| failure(error.to_string()))
    }
}

/// Closed local selection; provider capabilities are typed, board IDs are not.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeMediaPreparationInputs {
    pub schema_version: u32,
    pub format: String,
    pub qualification: String,
    pub provider: String,
    pub target_profiles_sha256: Sha256Digest,
    pub native_contract_sha256: Sha256Digest,
    pub wheel_lock: PinnedDocument,
    pub idf_lock: PinnedDocument,
    pub interpreter: PathBuf,
    pub runtime_prefix: PathBuf,
    pub wheel_cache: PathBuf,
    pub idf_source: PreparedSourceInput,
    pub compiler_source: PreparedSourceInput,
    pub cmake_source: PreparedSourceInput,
    pub ninja_source: PreparedSourceInput,
    pub idf_root: PathBuf,
    pub compiler_root: PathBuf,
    pub cmake_root: PathBuf,
    pub cmake: PathBuf,
    pub ninja: PathBuf,
    pub git: PathBuf,
    pub constraints: PathBuf,
}

/// Raw-byte-bound caller selection, not an authenticated origin manifest.
pub struct BoundNativeMediaPreparationInputs {
    inputs: NativeMediaPreparationInputs,
    raw: Vec<u8>,
    sha256: Sha256Digest,
}

/// Validate every declared input field without reserving roots or executing tools.
///
/// # Errors
/// Refuses changed/unknown schemas, unsupported providers and unsafe paths.
pub fn bind_native_media_preparation_inputs(
    bytes: &[u8],
    expected: &Sha256Digest,
) -> Result<BoundNativeMediaPreparationInputs, ContractError> {
    if bytes.len() as u64 > DOCUMENT_LIMIT || sha256_bytes(bytes) != *expected {
        return Err(failure("native media inputs raw-byte binding failed"));
    }
    let inputs: NativeMediaPreparationInputs =
        serde_json::from_slice(bytes).map_err(|_| failure("invalid closed native media inputs"))?;
    if inputs.schema_version != 1
        || inputs.format != "aros-native-media-inputs-v1"
        || inputs.qualification != "local-byte-lock-only"
        || inputs.provider != "esp-idf-bootloader-v1"
    {
        return Err(failure(
            "unsupported native media input identity or provider",
        ));
    }
    for path in inputs.paths() {
        absolute_path(path)?;
    }
    for source in inputs.sources() {
        source.request()?;
    }
    Ok(BoundNativeMediaPreparationInputs {
        inputs,
        raw: bytes.to_vec(),
        sha256: expected.clone(),
    })
}

impl NativeMediaPreparationInputs {
    const fn sources(&self) -> [&PreparedSourceInput; 4] {
        [
            &self.idf_source,
            &self.compiler_source,
            &self.cmake_source,
            &self.ninja_source,
        ]
    }

    fn paths(&self) -> [&Path; 16] {
        [
            &self.wheel_lock.path,
            &self.idf_lock.path,
            &self.interpreter,
            &self.runtime_prefix,
            &self.wheel_cache,
            &self.idf_source.destination,
            &self.compiler_source.destination,
            &self.cmake_source.destination,
            &self.ninja_source.destination,
            &self.idf_root,
            &self.compiler_root,
            &self.cmake_root,
            &self.cmake,
            &self.ninja,
            &self.git,
            &self.constraints,
        ]
    }
}

/// Caller-owned budgets and source selection. Work parent must already be private.
pub struct NativeMediaPreparationRequest<'a> {
    pub inputs: &'a BoundNativeMediaPreparationInputs,
    pub source_root: &'a Path,
    pub preset: &'a str,
    pub work_parent: &'a Path,
    pub jobs: u32,
    /// Operation start before input parsing; preflight time consumes the budget.
    pub started_at: Instant,
    pub timeout: Duration,
    pub cancellation: &'a CancellationToken,
}

/// Independently checked external subset. Native output roles are absent by design.
#[derive(Debug, Serialize)]
pub struct NativeMediaPreparationReceipt {
    pub schema: &'static str,
    pub qualification: &'static str,
    pub complete_flash_plan: bool,
    pub inputs_sha256: Sha256Digest,
    pub source_root: PathBuf,
    pub target_profiles_sha256: Sha256Digest,
    pub preset: String,
    pub work_root: PathBuf,
    pub python_receipt: PathBuf,
    pub python_receipt_sha256: Sha256Digest,
    pub bootloader_receipt: PathBuf,
    pub bootloader_receipt_sha256: Sha256Digest,
    pub bootloader_path: PathBuf,
    pub bootloader: EspImageFacts,
    pub partition_table_path: PathBuf,
    pub partition_table: EspPartitionFacts,
    pub media: NativeMediaBinding,
}

/// Prepare fresh external inputs offline; retain failures, never repair/adopt them.
///
/// # Errors
/// Refuses all modified input bindings, unsafe roots, deadline/cancellation,
/// producer failure or an independently invalid image/partition container.
pub fn prepare_native_media(
    request: &NativeMediaPreparationRequest<'_>,
) -> Result<NativeMediaPreparationReceipt, ContractError> {
    if request.jobs == 0 || request.timeout.is_zero() || request.cancellation.is_cancelled() {
        return Err(failure(
            "native media preparation requires live positive budgets",
        ));
    }
    let deadline = request
        .started_at
        .checked_add(request.timeout)
        .ok_or_else(|| failure("native media deadline overflow"))?;
    remaining(deadline, request.cancellation)?;
    absolute_path(request.source_root)?;
    absolute_path(request.work_parent)?;
    validate_private_directory_nofollow(request.work_parent).map_err(io_error)?;
    let parent = request.work_parent.canonicalize().map_err(io_error)?;
    let source = request.source_root.canonicalize().map_err(io_error)?;
    let inputs = &request.inputs.inputs;
    for input in std::iter::once(source.as_path()).chain(inputs.paths()) {
        remaining(deadline, request.cancellation)?;
        let canonical = input.canonicalize().map_err(io_error)?;
        if canonical.starts_with(&parent) || parent.starts_with(&canonical) {
            return Err(failure("native media work parent overlaps an input"));
        }
    }
    for executable in [&inputs.cmake, &inputs.ninja, &inputs.git] {
        validate_executable(executable)?;
    }
    let profiles_path = source.join("aros-targets.toml");
    let profiles_raw = read_document(&profiles_path)?;
    remaining(deadline, request.cancellation)?;
    if sha256_bytes(&profiles_raw) != inputs.target_profiles_sha256 {
        return Err(failure("source target profiles raw-byte binding failed"));
    }
    let profiles = TargetProfile::load_from_file(&profiles_path)
        .map_err(|error| failure(error.to_string()))?;
    if read_document(&profiles_path)? != profiles_raw {
        return Err(failure("source target profiles changed during selection"));
    }
    let profile = profiles
        .iter()
        .find(|profile| profile.name == request.preset)
        .ok_or_else(|| failure("preset is absent from the explicitly selected source"))?;
    let relative = Path::new(
        profile
            .native_build_contract
            .as_deref()
            .ok_or_else(|| failure("preset has no native build contract"))?,
    );
    let media =
        resolve_bound_native_media(&source, relative, profile, &inputs.native_contract_sha256)
            .map_err(failure)?;
    remaining(deadline, request.cancellation)?;
    let wheel_raw = read_document(&inputs.wheel_lock.path)?;
    let wheel_lock = bind_wheel_environment_lock(&wheel_raw, &inputs.wheel_lock.sha256)?;
    let idf_raw = read_document(&inputs.idf_lock.path)?;
    let idf_lock = bind_idf_bootloader_lock(&idf_raw, &inputs.idf_lock.sha256)?;
    let mut sources = Vec::with_capacity(4);
    for source in inputs.sources() {
        remaining(deadline, request.cancellation)?;
        sources.push(source.verify()?);
    }
    remaining(deadline, request.cancellation)?;
    let root = create_unique_directory_nofollow(&parent, "aros-native-media").map_err(io_error)?;
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).map_err(io_error)?;
    let owned = || -> Result<NativeMediaPreparationReceipt, ContractError> {
        write_new(&root.join("selected-inputs.json"), &request.inputs.raw)?;
        let python_parent = private_child(&root, "python")?;
        let idf_parent = private_child(&root, "idf")?;
        let python = prepare_wheel_environment(&WheelEnvironmentRequest {
            lock: &wheel_lock,
            interpreter: &inputs.interpreter,
            runtime_prefix: &inputs.runtime_prefix,
            wheel_cache: &inputs.wheel_cache,
            work_parent: &python_parent,
            timeout: remaining(deadline, request.cancellation)?,
            cancellation: request.cancellation,
        })?;
        let built = build_idf_bootloader(&IdfBootloaderRequest {
            lock: &idf_lock,
            python: &python,
            source_root: &source,
            profile,
            native_contract_sha256: &inputs.native_contract_sha256,
            idf_root: &inputs.idf_root,
            idf_source: &sources[0],
            compiler_source: &sources[1],
            cmake_source: &sources[2],
            ninja_source: &sources[3],
            compiler_root: &inputs.compiler_root,
            cmake_root: &inputs.cmake_root,
            cmake: &inputs.cmake,
            ninja: &inputs.ninja,
            git: &inputs.git,
            constraints: &inputs.constraints,
            work_parent: &idf_parent,
            jobs: request.jobs,
            timeout: remaining(deadline, request.cancellation)?,
            cancellation: request.cancellation,
        })?;
        let boot_slot = media
            .binding
            .layout
            .slots
            .iter()
            .find(|slot| slot.role == "bootloader")
            .ok_or_else(|| failure("source media has no bootloader slot"))?;
        let geometry = &media.binding.geometry;
        let boot_limit = boot_slot.range.end - boot_slot.range.start;
        let boot_bytes = read_bounded(&built.receipt.bootloader.path, boot_limit)?;
        let bootloader = verify_esp_image(
            &boot_bytes,
            EspImagePolicy {
                chip_id: geometry.chip_id,
                revision_min: geometry.revision_min,
                revision_max: geometry.revision_max,
                flash_bytes: geometry.flash_bytes,
                maximum_image_bytes: boot_limit,
            },
        )
        .map_err(failure)?;
        if bootloader.sha256 != built.receipt.bootloader.sha256
            || bootloader.size_bytes as u64 != built.receipt.bootloader.size_bytes
        {
            return Err(failure("bootloader changed after its producer receipt"));
        }
        let partition_table = verify_esp_partition_table(
            &media.partition.artifact.bytes,
            geometry.partition_table_offset,
            geometry.flash_bytes,
            &media.partition.artifact.source.partitions,
        )
        .map_err(failure)?;
        if read_document(&profiles_path)? != profiles_raw
            || read_document(&inputs.wheel_lock.path)? != wheel_raw
            || read_document(&inputs.idf_lock.path)? != idf_raw
        {
            return Err(failure(
                "native media input document changed during preparation",
            ));
        }
        for source in &sources {
            remaining(deadline, request.cancellation)?;
            source
                .revalidate()
                .map_err(|error| failure(error.to_string()))?;
        }
        python.revalidate()?;
        let final_media =
            resolve_bound_native_media(&source, relative, profile, &inputs.native_contract_sha256)
                .map_err(failure)?;
        if final_media != media {
            return Err(failure("source media changed during preparation"));
        }
        remaining(deadline, request.cancellation)?;
        let partition_path = root.join("partition-table.bin");
        write_new(&partition_path, &media.partition.artifact.bytes)?;
        let python_receipt = python.work_root.join("environment.receipt.json");
        let bootloader_receipt = built.work_root.join("bootloader.receipt.json");
        let receipt = NativeMediaPreparationReceipt {
            schema: "aros-native-media-preparation-v1",
            qualification: "local-byte-lock-only",
            complete_flash_plan: false,
            inputs_sha256: request.inputs.sha256.clone(),
            source_root: source.clone(),
            target_profiles_sha256: inputs.target_profiles_sha256.clone(),
            preset: request.preset.into(),
            work_root: root.clone(),
            python_receipt_sha256: sha256_bytes(&read_document(&python_receipt)?),
            python_receipt,
            bootloader_receipt_sha256: sha256_bytes(&read_document(&bootloader_receipt)?),
            bootloader_receipt,
            bootloader_path: built.receipt.bootloader.path,
            bootloader,
            partition_table_path: partition_path,
            partition_table,
            media: media.binding,
        };
        write_new(
            &root.join("preparation.receipt.json"),
            &serde_json::to_vec_pretty(&receipt).map_err(|error| failure(error.to_string()))?,
        )?;
        remaining(deadline, request.cancellation)?;
        Ok(receipt)
    };
    owned().map_err(ContractError::retained_material)
}

/// Bounded, no-follow document read used by explicit CLI selections.
///
/// # Errors
/// Refuses nonregular files, symlink traversal, oversized or growing documents.
pub fn read_document(path: &Path) -> Result<Vec<u8>, ContractError> {
    absolute_path(path)?;
    read_bounded(path, DOCUMENT_LIMIT)
}

fn read_bounded(path: &Path, maximum: u64) -> Result<Vec<u8>, ContractError> {
    let mut file = open_regular_file_nofollow(path).map_err(io_error)?;
    if file.metadata().map_err(io_error)?.len() > maximum {
        return Err(failure("native media input exceeds its byte budget"));
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(maximum.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() as u64 > maximum {
        return Err(failure("native media input grew past its byte budget"));
    }
    Ok(bytes)
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), ContractError> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(io_error)?;
    file.set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(io_error)?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(io_error)
}

fn private_child(root: &Path, name: &str) -> Result<PathBuf, ContractError> {
    let path = root.join(name);
    fs::create_dir(&path).map_err(io_error)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).map_err(io_error)?;
    Ok(path)
}

fn absolute_path(path: &Path) -> Result<(), ContractError> {
    if !path.is_absolute() || path.components().any(|p| matches!(p, Component::ParentDir)) {
        return Err(failure(
            "native media paths must be absolute without parent traversal",
        ));
    }
    Ok(())
}

fn validate_executable(path: &Path) -> Result<(), ContractError> {
    let file = open_regular_file_nofollow(path).map_err(|_| {
        failure(format!(
            "native media executable must be a no-follow regular file: {}",
            path.display()
        ))
    })?;
    if file.metadata().map_err(io_error)?.permissions().mode() & 0o111 == 0 {
        return Err(failure(format!(
            "native media executable has no execute permission: {}",
            path.display()
        )));
    }
    Ok(())
}

fn remaining(
    deadline: Instant,
    cancellation: &CancellationToken,
) -> Result<Duration, ContractError> {
    if cancellation.is_cancelled() {
        return Err(failure("native media preparation cancelled"));
    }
    deadline
        .checked_duration_since(Instant::now())
        .filter(|time| !time.is_zero())
        .ok_or_else(|| failure("native media preparation deadline expired"))
}

fn failure(message: impl Into<String>) -> ContractError {
    ContractError::environment(message)
}
#[allow(clippy::needless_pass_by_value)] // Error adapter is passed directly to map_err.
fn io_error(error: std::io::Error) -> ContractError {
    failure(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executable_preflight_refuses_symlinks_and_nonexecutables() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("tool");
        fs::write(&file, b"not executed").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(validate_executable(&file).is_err());
        fs::set_permissions(&file, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(validate_executable(&file).is_ok());
        let alias = root.path().join("alias");
        std::os::unix::fs::symlink(&file, &alias).unwrap();
        assert!(validate_executable(&alias).is_err());
    }
}
