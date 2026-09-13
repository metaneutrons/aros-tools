//! Immutable Cargo vendor-generation selection, publication and verification.
//!
//! This module owns the public cache-generation contract. The sibling
//! `cargo_vendor` module owns the no-follow validation and private offline
//! runtime copy used by the native lifecycle.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use aros_cache::{
    observe_root, resolve_explicit_root, CacheCapability, CacheSideEffects, RootObservation,
};
use aros_common::{
    create_unique_directory_nofollow, measure_tree_content_cas, publish_prepared_tree_noclobber,
    run_output_with_control, sha256_bytes, sha256_file, AdvisoryFileLock, CancellationToken,
    Sha256Digest,
};
use rustix::fs::{self as rfs, AtFlags, FileType, Mode};
use serde::{Deserialize, Serialize};

use crate::cargo_vendor::{
    normalize_system_parent, open_existing_directory, read_cargo_lock, read_direct_regular,
    render_vendor_configuration, safe_leaf, validate_cargo_lock, validate_vendor_tree,
    MAX_TEMPLATE_BYTES, VENDOR_DIRECTORY, VENDOR_PLACEHOLDER, VENDOR_TEMPLATE,
};
use crate::filesystem::{open_directory, DIRECTORY};
use crate::ContractError;

const CARGO_CACHE_NAMESPACE: &str = "cargo";
const CARGO_CACHE_VERSION: &str = "v1";
const CARGO_RECEIPT: &str = "receipt.json";
const CARGO_VENDOR_CAPTURE_LIMIT: usize = 64 * 1024;
const CARGO_VENDOR_TIMEOUT: Duration = Duration::from_mins(15);

/// Stable schema for a selected Cargo vendor-cache generation.
pub const CARGO_VENDOR_RECEIPT_SCHEMA: &str = "aros-cargo-vendor-generation-v1";

/// Stable schema for a metadata-only Cargo vendor-cache projection.
pub const CARGO_VENDOR_LIST_SCHEMA: &str = "aros-cache-cargo-list-v1";

/// Stable schema for Cargo vendor-cache verification.
pub const CARGO_VENDOR_VERIFY_SCHEMA: &str = "aros-cache-cargo-verify-v1";

/// Stable schema for Cargo vendor-cache population.
pub const CARGO_VENDOR_FETCH_SCHEMA: &str = "aros-cache-cargo-fetch-v1";

/// Stable schema for passive Cargo vendor-cache root observation.
pub const CARGO_VENDOR_STATUS_SCHEMA: &str = "aros-cache-cargo-status-v1";

/// Passive observation of the explicit parent cache root used by Cargo
/// generations. It never scans, creates or hashes a generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CargoVendorStatus {
    /// Versioned result schema.
    pub schema: &'static str,
    /// Stable operation identifier.
    pub operation: &'static str,
    /// This report is root metadata only.
    pub observation: &'static str,
    /// Hard side-effect contract.
    pub side_effects: CacheSideEffects,
    /// Supported explicit-generation operations.
    pub capabilities: [CacheCapability; 4],
    /// Observed caller-selected cache root.
    pub root: RootObservation,
    /// Namespaced immutable object layout below the root.
    pub object_layout: &'static str,
    /// Explicit boundary for global Cargo state.
    pub boundary: &'static str,
}

/// Explicit inputs that select one immutable Cargo vendor generation.
///
/// The producer checkout supplies the pinned Rust channel; the tools checkout
/// supplies the Cargo workspace and lock. The cache root is deliberately
/// separate from `CARGO_HOME`: a generated object is published under a
/// versioned AROS namespace and never reuses ambient Cargo state.
#[derive(Debug, Clone)]
pub struct CargoVendorRequest {
    /// Producer checkout that owns `toolchains/rust-toolchain.toml`.
    pub producer_dir: PathBuf,
    /// Tools checkout containing the selected Cargo workspace and lockfile.
    pub tools_dir: PathBuf,
    /// Verified Git tree selected by the native lifecycle. Public CLI callers
    /// leave this unset and the checkout is required to be clean and queried
    /// directly.
    pub tools_tree: Option<String>,
    /// Exact Cargo invocation path. A rustup proxy path is retained when that
    /// is the selected executable because it controls toolchain dispatch.
    pub cargo: PathBuf,
    /// Existing AROS-managed cache root.
    pub cache_dir: PathBuf,
}

/// Content and tool identity selecting one generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CargoVendorSelection {
    /// Schema of the identity payload.
    pub schema: String,
    /// Deterministic cache object key.
    pub generation: String,
    /// Canonical producer checkout selected by the caller.
    pub producer_dir: PathBuf,
    /// Canonical tools checkout selected by the caller.
    pub tools_dir: PathBuf,
    /// Selected committed Git tree for the tools checkout.
    pub tools_tree: String,
    /// Direct Cargo manifest digest.
    pub cargo_manifest_sha256: Sha256Digest,
    /// Direct Cargo lockfile digest.
    pub cargo_lock_sha256: Sha256Digest,
    /// Exact producer Rust toolchain-file digest.
    pub rust_toolchain_sha256: Sha256Digest,
    /// Exact pinned Rust/Cargo channel.
    pub rust_channel: String,
    /// Cargo invocation path retained for rustup dispatch semantics.
    pub cargo_invocation_path: PathBuf,
    /// Canonical regular executable behind the invocation path.
    pub cargo_resolved_path: PathBuf,
    /// SHA-256 of the resolved Cargo executable bytes.
    pub cargo_sha256: Sha256Digest,
    /// Exact normalized Cargo version observed under the selected Rust channel.
    pub cargo_version: String,
}

/// Portable content and tool identity stored in a published generation.
///
/// Checkout paths are intentionally absent. The native lifecycle snapshots its
/// input checkouts into fresh private directories, so a cache prepared from
/// the original checkout must remain selectable when those bytes are later
/// consumed from the snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CargoVendorIdentity {
    /// Schema of the identity payload.
    pub schema: String,
    /// Deterministic cache object key.
    pub generation: String,
    /// Selected committed Git tree for the tools checkout.
    pub tools_tree: String,
    /// Direct Cargo manifest digest.
    pub cargo_manifest_sha256: Sha256Digest,
    /// Direct Cargo lockfile digest.
    pub cargo_lock_sha256: Sha256Digest,
    /// Exact producer Rust toolchain-file digest.
    pub rust_toolchain_sha256: Sha256Digest,
    /// Exact pinned Rust/Cargo channel.
    pub rust_channel: String,
    /// SHA-256 of the resolved Cargo executable bytes.
    pub cargo_sha256: Sha256Digest,
    /// Exact normalized Cargo version observed under the selected Rust channel.
    pub cargo_version: String,
}

impl CargoVendorSelection {
    /// Return the portable cache identity without machine-local checkout paths.
    #[must_use]
    pub fn identity(&self) -> CargoVendorIdentity {
        CargoVendorIdentity {
            schema: self.schema.clone(),
            generation: self.generation.clone(),
            tools_tree: self.tools_tree.clone(),
            cargo_manifest_sha256: self.cargo_manifest_sha256.clone(),
            cargo_lock_sha256: self.cargo_lock_sha256.clone(),
            rust_toolchain_sha256: self.rust_toolchain_sha256.clone(),
            rust_channel: self.rust_channel.clone(),
            cargo_sha256: self.cargo_sha256.clone(),
            cargo_version: self.cargo_version.clone(),
        }
    }
}

/// Exact observed generation metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CargoVendorGeneration {
    /// Stable result schema.
    pub schema: &'static str,
    /// Stable operation identifier.
    pub operation: &'static str,
    /// Selected immutable identity.
    pub selection: CargoVendorSelection,
    /// Published generation directory.
    pub generation_dir: PathBuf,
    /// Number of checksum-validated external packages.
    pub package_count: usize,
    /// Content identity of the complete vendor tree.
    pub vendor_tree_sha256: Sha256Digest,
    /// SHA-256 of the single-placeholder configuration template.
    pub configuration_template_sha256: Sha256Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CargoVendorReceipt {
    schema: String,
    identity: CargoVendorIdentity,
    package_count: usize,
    vendor_tree_sha256: Sha256Digest,
    configuration_template_sha256: Sha256Digest,
}
const MAX_MANIFEST_BYTES: u64 = 8 * 1024 * 1024;

/// Observe one explicit Cargo cache parent without creating or scanning it.
///
/// # Errors
///
/// Returns AX0101 only when `cache_root` is relative. Missing, unsafe or
/// inaccessible absolute roots remain truthful successful observations.
pub fn cargo_vendor_status(cache_root: &Path) -> Result<CargoVendorStatus, ContractError> {
    let root = resolve_explicit_root(cache_root.to_owned())
        .map_err(|error| ContractError::invalid(error.to_string()))?;
    Ok(CargoVendorStatus {
        schema: CARGO_VENDOR_STATUS_SCHEMA,
        operation: "cargo.status",
        observation: "passive",
        side_effects: CacheSideEffects {
            creates_state: false,
            mutates_state: false,
            network: false,
            backend_process: false,
            locks: false,
            hashes_payloads: false,
        },
        capabilities: [
            CacheCapability::Status,
            CacheCapability::List,
            CacheCapability::Fetch,
            CacheCapability::Verify,
        ],
        root: observe_root(root),
        object_layout: "cargo/v1/<selection-sha256>/{cargo-vendor,cargo-vendor-config.toml,receipt.json}",
        boundary: "global CARGO_HOME, user Cargo configuration and credentials are excluded; all non-status operations require explicit producer, tools, Cargo and cache inputs",
    })
}

/// Select one Cargo vendor generation without reading or mutating cache data.
///
/// The selection binds the selected committed tools tree, direct workspace
/// inputs, producer Rust pin and exact Cargo executable/version. It
/// intentionally refuses an unpinned channel: an ambient default toolchain is
/// not a reproducible producer input.
///
/// # Errors
///
/// Returns AX0401 when an input is relative, unsafe, unreadable, lacks the
/// required pin/workspace files, or the selected Cargo executable cannot prove
/// its exact pinned version. This operation never invokes Cargo subcommands
/// that modify cache state or resolve dependencies.
pub fn select_vendor_generation(
    request: &CargoVendorRequest,
) -> Result<CargoVendorSelection, ContractError> {
    let producer = canonical_directory(&request.producer_dir, "selected producer checkout")?;
    let tools = canonical_directory(&request.tools_dir, "selected tools checkout")?;
    let cache = canonical_directory(&request.cache_dir, "selected Cargo cache root")?;
    let _ = cache;
    let rust_toolchain = producer.join("toolchains/rust-toolchain.toml");
    let rust_toolchain_bytes = read_regular_path(
        &rust_toolchain,
        MAX_TEMPLATE_BYTES,
        "selected producer Rust toolchain file",
    )?;
    let rust_channel = parse_rust_channel(&rust_toolchain_bytes)?;
    let manifest = tools.join("Cargo.toml");
    let lock = tools.join("Cargo.lock");
    let manifest_bytes =
        read_regular_path(&manifest, MAX_MANIFEST_BYTES, "selected tools Cargo.toml")?;
    let lock_bytes = read_cargo_lock(&lock)?;
    let invocation = canonical_executable(&request.cargo, "selected Cargo executable", false)?;
    let resolved = canonical_executable(&request.cargo, "selected Cargo executable", true)?;
    let cargo_version = probe_cargo_version(&invocation, &rust_channel)?;
    let tools_tree = match &request.tools_tree {
        Some(tree) if valid_git_tree(tree) => tree.clone(),
        Some(_) => {
            return Err(ContractError::environment(
                "selected native tools tree is not a lowercase 40-hex Git object identity",
            ))
        }
        None => selected_clean_tools_tree(&tools)?,
    };
    let cargo_sha256 = sha256_file(&resolved)
        .map_err(|_| ContractError::environment("cannot hash selected Cargo executable"))?
        .digest;
    let identity = CargoVendorSelection {
        schema: CARGO_VENDOR_RECEIPT_SCHEMA.to_owned(),
        generation: String::new(),
        producer_dir: producer,
        tools_dir: tools,
        tools_tree,
        cargo_manifest_sha256: sha256_bytes(&manifest_bytes),
        cargo_lock_sha256: sha256_bytes(&lock_bytes),
        rust_toolchain_sha256: sha256_bytes(&rust_toolchain_bytes),
        rust_channel,
        cargo_invocation_path: invocation,
        cargo_resolved_path: resolved,
        cargo_sha256,
        cargo_version,
    };
    let mut portable = identity.identity();
    portable.generation.clear();
    let identity_bytes = serde_json::to_vec(&portable)
        .map_err(|_| ContractError::state("cannot encode Cargo vendor selection identity"))?;
    Ok(CargoVendorSelection {
        generation: sha256_bytes(&identity_bytes).to_string(),
        ..identity
    })
}

/// Passively list the exact generation selected by explicit producer, tools,
/// Cargo and cache inputs.
///
/// It reads direct identity inputs and runs bounded Git/Cargo version probes to
/// select the generation, but never reads a vendor payload, resolves
/// dependencies, creates directories, or takes a cache lock.
///
/// # Errors
///
/// Returns AX0401 when selection inputs or an existing receipt are unsafe or
/// inconsistent. A missing generation is represented as `Ok(None)`.
pub fn list_vendor_generation(
    request: &CargoVendorRequest,
) -> Result<Option<CargoVendorGeneration>, ContractError> {
    let selection = select_vendor_generation(request)?;
    let generation = generation_path(&request.cache_dir, &selection)?;
    match fs::symlink_metadata(&generation) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => {
            return Err(ContractError::environment(
                "selected Cargo vendor generation is not a real directory",
            ))
        }
        Err(_) => {
            return Err(ContractError::environment(
                "cannot inspect selected Cargo vendor generation",
            ))
        }
    }
    list_vendor_generation_at(&generation, &selection).map(Some)
}

/// Verify the exact selected Cargo vendor generation without invoking Cargo.
///
/// # Errors
///
/// Returns AX0401 if the generation is missing, unsafe, does not bind the
/// current explicit selection, or fails Cargo checksum/lock validation.
pub fn verify_vendor_generation(
    request: &CargoVendorRequest,
) -> Result<CargoVendorGeneration, ContractError> {
    let selection = select_vendor_generation(request)?;
    let generation = generation_path(&request.cache_dir, &selection)?;
    verify_vendor_generation_at(&generation, &selection, "cargo.verify")
}

/// Populate one immutable Cargo vendor generation through the selected Cargo
/// executable, or reuse its fully verified existing instance.
///
/// The Cargo process runs with a fresh private `CARGO_HOME`, `HOME`, temporary
/// directory and working directory; it cannot consume a user Cargo config or
/// credentials. Cargo's resolver remains authoritative: this function passes
/// `--locked` and never edits the selected lockfile.
///
/// When `offline` is set, population is forbidden and this is equivalent to
/// strict verification of the selected existing generation.
///
/// # Errors
///
/// Returns AX0401 for invalid selection inputs or a rejected cached object and
/// AX0801 when Cargo cannot complete its bounded, cancellable generation.
pub fn fetch_vendor_generation(
    request: &CargoVendorRequest,
    offline: bool,
    cancellation: &CancellationToken,
) -> Result<CargoVendorGeneration, ContractError> {
    let selection = select_vendor_generation(request)?;
    let generation = generation_path(&request.cache_dir, &selection)?;
    if offline {
        return verify_vendor_generation_at(&generation, &selection, "cargo.fetch.offline");
    }
    let generation_parent = ensure_generation_parent(&request.cache_dir)?;
    let lock_path = generation_parent.join(format!("{}.lock", selection.generation));
    let _lock = acquire_generation_lock(&lock_path, cancellation)?;
    if generation.exists() {
        return verify_vendor_generation_at(&generation, &selection, "cargo.fetch");
    }
    let staging = create_unique_directory_nofollow(&generation_parent, ".cargo-vendor-stage")
        .map_err(|_| ContractError::state("cannot reserve Cargo vendor staging directory"))?;
    let result = populate_vendor_generation(&staging, &selection, cancellation);
    match result {
        Ok(_) => {
            publish_prepared_tree_noclobber(&staging, &generation).map_err(|_| {
                ContractError::state("cannot atomically publish verified Cargo vendor generation")
            })?;
            verify_vendor_generation_at(&generation, &selection, "cargo.fetch")
        }
        Err(error) => Err(error),
    }
}

fn acquire_generation_lock(
    lock_path: &Path,
    cancellation: &CancellationToken,
) -> Result<AdvisoryFileLock, ContractError> {
    let deadline = Instant::now() + CARGO_VENDOR_TIMEOUT;
    loop {
        if cancellation.is_cancelled() {
            return Err(ContractError::state(
                "Cargo vendor generation cancelled while waiting for its cache lock",
            ));
        }
        match AdvisoryFileLock::acquire(lock_path) {
            Ok(lock) => return Ok(lock),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(ContractError::state(
                        "Cargo vendor generation exceeded its 15 minute cache-lock wait limit",
                    ));
                }
                thread::sleep(Duration::from_millis(25));
            }
            Err(_) => {
                return Err(ContractError::state(
                    "cannot acquire Cargo vendor generation lock",
                ))
            }
        }
    }
}

fn populate_vendor_generation(
    staging: &Path,
    selection: &CargoVendorSelection,
    cancellation: &CancellationToken,
) -> Result<CargoVendorGeneration, ContractError> {
    let vendor = staging.join(VENDOR_DIRECTORY);
    let parent = staging.parent().ok_or_else(|| {
        ContractError::state("Cargo vendor staging directory has no managed parent")
    })?;
    let process_root = tempfile::Builder::new()
        .prefix(".cargo-vendor-process")
        .tempdir_in(parent)
        .map_err(|_| ContractError::state("cannot create isolated Cargo vendor process root"))?;
    let private_home = process_root.path().join("cargo-home");
    let temporary = process_root.path().join("tmp");
    fs::create_dir(&private_home)
        .and_then(|()| fs::create_dir(&temporary))
        .map_err(|_| {
            ContractError::state("cannot create isolated Cargo vendor process environment")
        })?;
    let mut command = Command::new(&selection.cargo_invocation_path);
    command
        .env_clear()
        .current_dir(staging)
        .arg("vendor")
        .arg("--locked")
        .arg("--versioned-dirs")
        .arg("--manifest-path")
        .arg(selection.tools_dir.join("Cargo.toml"))
        .arg(&vendor)
        .env("CARGO_HOME", &private_home)
        .env("HOME", &private_home)
        .env("TMPDIR", &temporary)
        .env("PATH", isolated_vendor_path(selection)?)
        .env("CARGO_INCREMENTAL", "0")
        .env("RUSTUP_TOOLCHAIN", &selection.rust_channel);
    preserve_rustup_home(&mut command)?;
    let output = run_output_with_control(
        &mut command,
        CARGO_VENDOR_CAPTURE_LIMIT,
        CARGO_VENDOR_TIMEOUT,
        cancellation,
    )
    .map_err(|_| ContractError::state("cannot execute selected Cargo vendor command"))?;
    if output.cancelled {
        return Err(ContractError::state("Cargo vendor generation cancelled"));
    }
    if output.timed_out {
        return Err(ContractError::state(
            "Cargo vendor generation exceeded its 15 minute limit",
        ));
    }
    if !output.status.success() {
        return Err(ContractError::state(format!(
            "Cargo vendor generation failed: {}",
            bounded_process_summary(&output)
        )));
    }
    let template = canonical_vendor_template(
        output.stdout.exact_bytes().ok_or_else(|| {
            ContractError::state("Cargo vendor configuration exceeded the bounded capture limit")
        })?,
        &vendor,
    )?;
    write_new_regular(&staging.join(VENDOR_TEMPLATE), &template)?;
    let packages = validate_vendor_tree(&vendor)?;
    validate_cargo_lock(&selection.tools_dir.join("Cargo.lock"), &packages)?;
    let vendor_tree = measure_tree_content_cas(&vendor)
        .map_err(|_| {
            ContractError::environment("cannot safely measure generated Cargo vendor tree")
        })?
        .payload_digest_excluding(None);
    let receipt = CargoVendorReceipt {
        schema: CARGO_VENDOR_RECEIPT_SCHEMA.to_owned(),
        identity: selection.identity(),
        package_count: packages.len(),
        vendor_tree_sha256: vendor_tree.clone(),
        configuration_template_sha256: sha256_bytes(&template),
    };
    let receipt = serde_json::to_vec_pretty(&receipt)
        .map_err(|_| ContractError::state("cannot encode Cargo vendor receipt"))?;
    write_new_regular(&staging.join(CARGO_RECEIPT), &receipt)?;
    Ok(CargoVendorGeneration {
        schema: CARGO_VENDOR_FETCH_SCHEMA,
        operation: "cargo.fetch",
        selection: selection.clone(),
        generation_dir: staging.to_owned(),
        package_count: packages.len(),
        vendor_tree_sha256: vendor_tree,
        configuration_template_sha256: sha256_bytes(&template),
    })
}

fn generation_path(
    cache_dir: &Path,
    selection: &CargoVendorSelection,
) -> Result<PathBuf, ContractError> {
    let cache = canonical_directory(cache_dir, "selected Cargo cache root")?;
    Ok(cache
        .join(CARGO_CACHE_NAMESPACE)
        .join(CARGO_CACHE_VERSION)
        .join(&selection.generation))
}

fn ensure_generation_parent(cache_dir: &Path) -> Result<PathBuf, ContractError> {
    let cache = canonical_directory(cache_dir, "selected Cargo cache root")?;
    let mut current_path = cache.clone();
    let mut current = open_existing_directory(&cache, "selected Cargo cache root")?;
    for component in [CARGO_CACHE_NAMESPACE, CARGO_CACHE_VERSION] {
        match rfs::statat(&current, component, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) if FileType::from_raw_mode(stat.st_mode).is_dir() => {}
            Ok(_) => {
                return Err(ContractError::environment(
                    "Cargo cache namespace contains a non-directory object",
                ))
            }
            Err(rustix::io::Errno::NOENT) => {
                rfs::mkdirat(
                    &current,
                    component,
                    Mode::RUSR | Mode::WUSR | Mode::XUSR | Mode::RGRP | Mode::XGRP,
                )
                .map_err(|_| ContractError::state("cannot create Cargo cache namespace"))?;
            }
            Err(_) => {
                return Err(ContractError::environment(
                    "cannot inspect Cargo cache namespace",
                ))
            }
        }
        current = File::from(
            rfs::openat(&current, component, DIRECTORY, Mode::empty()).map_err(|_| {
                ContractError::environment("cannot safely open Cargo cache namespace")
            })?,
        );
        current_path.push(component);
    }
    Ok(current_path)
}

fn verify_vendor_generation_at(
    generation: &Path,
    selection: &CargoVendorSelection,
    operation: &'static str,
) -> Result<CargoVendorGeneration, ContractError> {
    let root = open_existing_directory(generation, "selected Cargo vendor generation")?;
    let receipt_bytes = read_direct_regular(&root, CARGO_RECEIPT, MAX_TEMPLATE_BYTES)?;
    let receipt: CargoVendorReceipt = serde_json::from_slice(&receipt_bytes).map_err(|_| {
        ContractError::environment("Cargo vendor generation receipt is malformed or unsupported")
    })?;
    if receipt.schema != CARGO_VENDOR_RECEIPT_SCHEMA || receipt.identity != selection.identity() {
        return Err(ContractError::environment(
            "Cargo vendor generation does not bind the selected producer, tools, lock and Cargo identity",
        ));
    }
    let template = read_direct_regular(&root, VENDOR_TEMPLATE, MAX_TEMPLATE_BYTES)?;
    let template_text = std::str::from_utf8(&template)
        .map_err(|_| ContractError::environment("Cargo vendor configuration is not UTF-8"))?;
    let vendor = generation.join(VENDOR_DIRECTORY);
    let _ = render_vendor_configuration(template_text, &vendor)?;
    let packages = validate_vendor_tree(&vendor)?;
    validate_cargo_lock(&selection.tools_dir.join("Cargo.lock"), &packages)?;
    let measured_vendor_tree = measure_tree_content_cas(&vendor)
        .map_err(|_| ContractError::environment("cannot safely measure Cargo vendor generation"))?
        .payload_digest_excluding(None);
    let template_digest = sha256_bytes(&template);
    if receipt.package_count != packages.len()
        || receipt.vendor_tree_sha256 != measured_vendor_tree
        || receipt.configuration_template_sha256 != template_digest
    {
        return Err(ContractError::environment(
            "Cargo vendor generation receipt does not match its validated content",
        ));
    }
    Ok(CargoVendorGeneration {
        schema: if operation == "cargo.list" {
            CARGO_VENDOR_LIST_SCHEMA
        } else {
            CARGO_VENDOR_VERIFY_SCHEMA
        },
        operation,
        selection: selection.clone(),
        generation_dir: generation.to_owned(),
        package_count: packages.len(),
        vendor_tree_sha256: measured_vendor_tree,
        configuration_template_sha256: template_digest,
    })
}

fn list_vendor_generation_at(
    generation: &Path,
    selection: &CargoVendorSelection,
) -> Result<CargoVendorGeneration, ContractError> {
    let root = open_existing_directory(generation, "selected Cargo vendor generation")?;
    let receipt_bytes = read_direct_regular(&root, CARGO_RECEIPT, MAX_TEMPLATE_BYTES)?;
    let receipt: CargoVendorReceipt = serde_json::from_slice(&receipt_bytes).map_err(|_| {
        ContractError::environment("Cargo vendor generation receipt is malformed or unsupported")
    })?;
    if receipt.schema != CARGO_VENDOR_RECEIPT_SCHEMA || receipt.identity != selection.identity() {
        return Err(ContractError::environment(
            "Cargo vendor generation receipt does not bind the selected producer, tools, lock and Cargo identity",
        ));
    }
    Ok(CargoVendorGeneration {
        schema: CARGO_VENDOR_LIST_SCHEMA,
        operation: "cargo.list",
        selection: selection.clone(),
        generation_dir: generation.to_owned(),
        package_count: receipt.package_count,
        vendor_tree_sha256: receipt.vendor_tree_sha256,
        configuration_template_sha256: receipt.configuration_template_sha256,
    })
}

fn canonical_directory(path: &Path, label: &str) -> Result<PathBuf, ContractError> {
    if !path.is_absolute() {
        return Err(ContractError::environment(format!(
            "{label} must be an absolute path"
        )));
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| ContractError::environment(format!("{label} cannot be resolved")))?;
    let metadata = fs::symlink_metadata(&canonical)
        .map_err(|_| ContractError::environment(format!("{label} cannot be inspected")))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(ContractError::environment(format!(
            "{label} is not a real directory"
        )));
    }
    let _ = open_directory(&canonical)
        .map_err(|_| ContractError::environment(format!("{label} is unsafe")))?;
    Ok(canonical)
}

fn selected_clean_tools_tree(tools: &Path) -> Result<String, ContractError> {
    let root = PathBuf::from(git_output(tools, &["rev-parse", "--show-toplevel"])?);
    let relative = tools.strip_prefix(&root).map_err(|_| {
        ContractError::environment("selected tools directory is outside its reported Git checkout")
    })?;
    let pathspec = if relative.as_os_str().is_empty() {
        ".".to_owned()
    } else {
        relative
            .to_str()
            .filter(|value| !value.is_empty() && !value.contains('\0'))
            .map(str::to_owned)
            .ok_or_else(|| {
                ContractError::environment("selected tools directory has a non-portable Git path")
            })?
    };
    if !git_output(
        &root,
        &[
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--",
            &pathspec,
        ],
    )?
    .is_empty()
    {
        return Err(ContractError::environment(
            "selected tools directory is not clean; commit or discard its changes before preparing a reproducible Cargo vendor generation",
        ));
    }
    let object = if pathspec == "." {
        let mut object = String::from("HEAD^");
        object.push('{');
        object.push_str("tree");
        object.push('}');
        object
    } else {
        format!("HEAD:{pathspec}")
    };
    let tree = git_output(&root, &["rev-parse", &object])?;
    if !valid_git_tree(&tree) {
        return Err(ContractError::environment(
            "selected tools checkout did not report a lowercase 40-hex Git tree identity",
        ));
    }
    Ok(tree)
}

fn git_output(directory: &Path, arguments: &[&str]) -> Result<String, ContractError> {
    let candidate = which::which("git").map_err(|_| {
        ContractError::prerequisite("required Cargo vendor tool 'git' is unavailable")
    })?;
    let git = canonical_executable(&candidate, "selected Git executable", false)?;
    let mut command = Command::new(&git);
    command
        .env_clear()
        .current_dir(directory)
        .args(arguments)
        .env("PATH", isolated_path([git.as_path()])?)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0");
    let cancellation = CancellationToken::default();
    let output = run_output_with_control(
        &mut command,
        16 * 1024,
        Duration::from_secs(10),
        &cancellation,
    )
    .map_err(|_| ContractError::environment("cannot execute Git for selected tools checkout"))?;
    if output.timed_out || output.cancelled || !output.status.success() {
        return Err(ContractError::environment(format!(
            "selected tools checkout cannot provide a clean committed Git tree: {}",
            bounded_process_summary(&output)
        )));
    }
    let text = output
        .stdout
        .exact_bytes()
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .ok_or_else(|| {
            ContractError::environment("selected tools Git output is not bounded UTF-8")
        })?;
    Ok(text.trim().to_owned())
}

fn valid_git_tree(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn canonical_executable(path: &Path, label: &str, resolve: bool) -> Result<PathBuf, ContractError> {
    if !path.is_absolute() {
        return Err(ContractError::environment(format!(
            "{label} must be an absolute path"
        )));
    }
    let candidate = if resolve {
        path.canonicalize()
            .map_err(|_| ContractError::environment(format!("{label} cannot be resolved")))?
    } else {
        path.to_owned()
    };
    let metadata = fs::metadata(&candidate)
        .map_err(|_| ContractError::environment(format!("{label} cannot be inspected")))?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
        return Err(ContractError::environment(format!(
            "{label} is not an executable regular file"
        )));
    }
    Ok(candidate)
}

fn read_regular_path(path: &Path, limit: u64, label: &str) -> Result<Vec<u8>, ContractError> {
    if !path.is_absolute() {
        return Err(ContractError::environment(format!(
            "{label} must be an absolute path"
        )));
    }
    let parent = path
        .parent()
        .ok_or_else(|| ContractError::environment(format!("{label} has no parent directory")))?;
    let parent = normalize_system_parent(parent)?;
    let parent_file = open_existing_directory(&parent, label)?;
    let leaf = safe_leaf(path)?;
    read_direct_regular(&parent_file, &leaf, limit)
}

fn parse_rust_channel(bytes: &[u8]) -> Result<String, ContractError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct RustToolchainDocument {
        toolchain: RustToolchainSection,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct RustToolchainSection {
        channel: String,
        profile: String,
        #[serde(default)]
        components: Vec<String>,
        #[serde(default)]
        targets: Vec<String>,
    }
    let document: RustToolchainDocument =
        toml::from_str(std::str::from_utf8(bytes).map_err(|_| {
            ContractError::environment("selected producer Rust toolchain file is not UTF-8")
        })?)
        .map_err(|_| {
            ContractError::environment("selected producer Rust toolchain file is malformed")
        })?;
    if document.toolchain.profile != "minimal"
        || semver::Version::parse(&document.toolchain.channel).is_err()
        || document
            .toolchain
            .components
            .iter()
            .chain(document.toolchain.targets.iter())
            .any(String::is_empty)
    {
        return Err(ContractError::environment(
            "selected producer Rust toolchain is not an explicit stable minimal toolchain",
        ));
    }
    Ok(document.toolchain.channel)
}

fn probe_cargo_version(invocation: &Path, rust_channel: &str) -> Result<String, ContractError> {
    let mut command = Command::new(invocation);
    command
        .env_clear()
        .arg("--version")
        .env("PATH", isolated_cargo_path(invocation)?)
        .env("RUSTUP_TOOLCHAIN", rust_channel);
    preserve_rustup_home(&mut command)?;
    let cancellation = CancellationToken::default();
    let output = run_output_with_control(
        &mut command,
        16 * 1024,
        Duration::from_secs(10),
        &cancellation,
    )
    .map_err(|_| ContractError::environment("cannot execute selected Cargo executable"))?;
    if output.timed_out || output.cancelled || !output.status.success() {
        return Err(ContractError::environment(
            "selected Cargo executable cannot report its pinned toolchain version",
        ));
    }
    let version = output
        .stdout
        .exact_bytes()
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .and_then(|text| text.lines().next())
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or_else(|| ContractError::environment("selected Cargo version output is malformed"))?;
    if version != rust_channel {
        return Err(ContractError::environment(format!(
            "selected Cargo version {version} does not match pinned producer channel {rust_channel}"
        )));
    }
    Ok(version.to_owned())
}

fn isolated_vendor_path(selection: &CargoVendorSelection) -> Result<OsString, ContractError> {
    let git = which::which("git").map_err(|_| {
        ContractError::prerequisite("required Cargo vendor tool 'git' is unavailable")
    })?;
    let git = canonical_executable(&git, "selected Git executable", false)?;
    isolated_path([
        selection.cargo_invocation_path.as_path(),
        selection.cargo_resolved_path.as_path(),
        git.as_path(),
    ])
}

fn isolated_cargo_path(cargo: &Path) -> Result<OsString, ContractError> {
    isolated_path([cargo])
}

fn isolated_path<'a>(
    programs: impl IntoIterator<Item = &'a Path>,
) -> Result<OsString, ContractError> {
    let mut directories = Vec::new();
    for program in programs {
        let directory = program.parent().ok_or_else(|| {
            ContractError::environment("selected Cargo process tool has no executable directory")
        })?;
        if !directory.is_absolute() {
            return Err(ContractError::environment(
                "selected Cargo process tool directory must be absolute",
            ));
        }
        if !directories
            .iter()
            .any(|existing: &PathBuf| existing == directory)
        {
            directories.push(directory.to_owned());
        }
    }
    std::env::join_paths(directories)
        .map_err(|_| ContractError::environment("cannot construct isolated Cargo process PATH"))
}

fn preserve_rustup_home(command: &mut Command) -> Result<(), ContractError> {
    let explicit = std::env::var_os("RUSTUP_HOME").map(PathBuf::from);
    let candidate = explicit
        .clone()
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".rustup")));
    let Some(candidate) = candidate else {
        return Ok(());
    };
    if !candidate.is_absolute() {
        return Err(ContractError::environment(
            "RUSTUP_HOME must be absolute when selected",
        ));
    }
    if !candidate.exists() {
        return if explicit.is_some() {
            Err(ContractError::environment(
                "selected Rustup home cannot be resolved",
            ))
        } else {
            Ok(())
        };
    }
    let canonical = candidate
        .canonicalize()
        .map_err(|_| ContractError::environment("selected Rustup home cannot be resolved"))?;
    let metadata = fs::symlink_metadata(&canonical)
        .map_err(|_| ContractError::environment("selected Rustup home cannot be inspected"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(ContractError::environment(
            "selected Rustup home is not a real directory",
        ));
    }
    command.env("RUSTUP_HOME", canonical);
    Ok(())
}

fn canonical_vendor_template(output: &[u8], vendor: &Path) -> Result<Vec<u8>, ContractError> {
    let text = std::str::from_utf8(output)
        .map_err(|_| ContractError::environment("Cargo vendor configuration is not UTF-8"))?;
    let mut document: toml::Value = toml::from_str(text)
        .map_err(|_| ContractError::environment("Cargo vendor emitted malformed configuration"))?;
    let sources = document
        .get_mut("source")
        .and_then(toml::Value::as_table_mut)
        .ok_or_else(|| {
            ContractError::environment("Cargo vendor configuration has no source table")
        })?;
    let vendored = sources
        .get_mut("vendored-sources")
        .and_then(toml::Value::as_table_mut)
        .ok_or_else(|| {
            ContractError::environment("Cargo vendor configuration has no vendored-sources mapping")
        })?;
    if vendored.len() != 1
        || vendored.get("directory").and_then(toml::Value::as_str)
            != Some(vendor.to_string_lossy().as_ref())
    {
        return Err(ContractError::environment(
            "Cargo vendor configuration does not bind exactly the generated vendor directory",
        ));
    }
    vendored.insert(
        "directory".to_owned(),
        toml::Value::String(VENDOR_PLACEHOLDER.to_owned()),
    );
    let template = toml::to_string(&document)
        .map_err(|_| ContractError::state("cannot encode generated Cargo vendor configuration"))?;
    let _ = render_vendor_configuration(&template, vendor)?;
    Ok(template.into_bytes())
}

fn write_new_regular(path: &Path, bytes: &[u8]) -> Result<(), ContractError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| ContractError::state("cannot create generated Cargo vendor metadata"))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| ContractError::state("cannot persist generated Cargo vendor metadata"))
}

fn bounded_process_summary(output: &aros_common::ProcessOutput) -> String {
    let text = output.stderr.rendered_lossy();
    let first = text.lines().next().unwrap_or("no diagnostic output");
    let summary = first.chars().take(240).collect::<String>();
    if summary.is_empty() {
        "no diagnostic output".to_owned()
    } else {
        summary
    }
}
