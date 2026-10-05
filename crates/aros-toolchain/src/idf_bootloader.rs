//! Fixed native ESP-IDF bootloader adapter, not a board-script interface.
//!
//! Source owns chip, project, board defaults and patch semantics. An explicit
//! local byte lock binds prepared vendor sources/tools; this adapter owns fresh
//! project/output paths, offline controls, process supervision and receipts.
//! It never builds the dummy application, adopts its flash table, runs vendor
//! installation/activation helpers or writes a device. Local byte consistency
//! is not authenticated archive/patch provenance or release authority.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use aros_common::native_build_contract::{
    load_bound_native_build_contract, LoadedNativeBuildContract,
};
use aros_common::{
    copy_tree_from_snapshot_nofollow, create_unique_directory_nofollow,
    measure_regular_file_bounded, measure_tree_content_cas_bounded, sha256_bytes,
    validate_private_directory_nofollow, CancellationToken, Sha256Digest, TargetProfile,
    TreeTraversalLimits,
};
use aros_fetch::engine::source_receipt::VerifiedSourceReceipt;
use serde::{Deserialize, Serialize};

use crate::wheel_environment::{
    base_environment, run_phase, PreparedWheelEnvironment, WheelPhaseReceipt,
};
use crate::ContractError;

const DOCUMENT_LIMIT: u64 = 1024 * 1024;

/// Prepared external byte identities. Paths come only from the explicit request.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IdfBootloaderLock {
    pub schema_version: u32,
    pub format: String,
    pub qualification: String,
    pub host: String,
    pub idf_version: String,
    pub compiler_prefix: String,
    pub idf_tree_sha256: Sha256Digest,
    pub idf_source_receipt_sha256: Sha256Digest,
    pub compiler_source_receipt_sha256: Sha256Digest,
    pub cmake_source_receipt_sha256: Sha256Digest,
    pub ninja_source_receipt_sha256: Sha256Digest,
    pub compiler_tree_sha256: Sha256Digest,
    pub cmake_tree_sha256: Sha256Digest,
    pub ninja_sha256: Sha256Digest,
    pub git_sha256: Sha256Digest,
    pub requirements_sha256: Sha256Digest,
    pub constraints_sha256: Sha256Digest,
    pub wheel_lock_sha256: Sha256Digest,
}

#[derive(Debug)]
pub struct BoundIdfBootloaderLock {
    lock: IdfBootloaderLock,
    raw: Vec<u8>,
    sha256: Sha256Digest,
}

/// Validate the closed lock without touching files or executing a process.
///
/// # Errors
/// Rejects changed bytes, unsupported schema/host/qualification and unsafe IDs.
pub fn bind_idf_bootloader_lock(
    bytes: &[u8],
    expected: &Sha256Digest,
) -> Result<BoundIdfBootloaderLock, ContractError> {
    if bytes.len() as u64 > DOCUMENT_LIMIT || sha256_bytes(bytes) != *expected {
        return Err(failure("IDF lock raw-byte binding failed"));
    }
    let lock: IdfBootloaderLock =
        serde_json::from_slice(bytes).map_err(|_| failure("invalid closed IDF lock"))?;
    let portable = |value: &str| {
        !value.is_empty()
            && value.len() <= 128
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b))
    };
    if lock.schema_version != 1
        || lock.format != "aros-idf-bootloader-v1"
        || lock.qualification != "local-byte-lock-only"
        || lock.host != format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
        || !portable(&lock.compiler_prefix)
        || lock.compiler_prefix.starts_with('.')
        || !portable(&lock.idf_version)
        || lock.idf_version.starts_with('.')
    {
        return Err(failure("unsupported or unsafe IDF lock identity"));
    }
    Ok(BoundIdfBootloaderLock {
        lock,
        raw: bytes.to_vec(),
        sha256: expected.clone(),
    })
}

/// All inputs are explicit; no PATH discovery or mutable ambient IDF venv.
#[derive(Clone, Copy)]
pub struct IdfBootloaderRequest<'a> {
    pub lock: &'a BoundIdfBootloaderLock,
    pub python: &'a PreparedWheelEnvironment,
    pub source_root: &'a Path,
    pub profile: &'a TargetProfile,
    pub native_contract_sha256: &'a Sha256Digest,
    pub idf_root: &'a Path,
    /// Exact archive/ordered-patch receipt bound to the live prepared source tree.
    pub idf_source: &'a VerifiedSourceReceipt,
    pub compiler_source: &'a VerifiedSourceReceipt,
    pub cmake_source: &'a VerifiedSourceReceipt,
    pub ninja_source: &'a VerifiedSourceReceipt,
    pub compiler_root: &'a Path,
    pub cmake_root: &'a Path,
    pub cmake: &'a Path,
    pub ninja: &'a Path,
    pub git: &'a Path,
    pub constraints: &'a Path,
    pub work_parent: &'a Path,
    pub jobs: u32,
    pub timeout: Duration,
    pub cancellation: &'a CancellationToken,
}

#[derive(Debug, Serialize)]
pub struct IdfBootloaderReceipt {
    pub schema_version: u32,
    pub format: &'static str,
    pub qualification: &'static str,
    pub lock_sha256: Sha256Digest,
    pub source_contract_sha256: Sha256Digest,
    pub source_baseline: String,
    pub profile: String,
    pub board: String,
    pub chip: String,
    pub python_receipt_sha256: Sha256Digest,
    pub idf_root: PathBuf,
    pub idf_source_receipt: PathBuf,
    pub idf_source_receipt_sha256: Sha256Digest,
    pub vendor_sources: Vec<IdfVendorSourceReceipt>,
    pub execution_idf_root: PathBuf,
    pub execution_idf_tree_before: Sha256Digest,
    pub execution_idf_tree_after: Sha256Digest,
    pub execution_git_index_before: Sha256Digest,
    pub execution_git_index_after: Sha256Digest,
    pub compiler_root: PathBuf,
    pub cmake_root: PathBuf,
    pub cmake: PathBuf,
    pub ninja: PathBuf,
    pub git: PathBuf,
    pub work_root: PathBuf,
    pub phases: Vec<WheelPhaseReceipt>,
    pub bootloader: IdfOutput,
    pub elf: IdfOutput,
}

#[derive(Debug, Serialize)]
pub struct IdfOutput {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub sha256: Sha256Digest,
}

/// Checked vendor archive identity, not an authenticated origin statement.
#[derive(Debug, Serialize)]
pub struct IdfVendorSourceReceipt {
    pub tool: String,
    pub receipt_path: PathBuf,
    pub receipt_sha256: Sha256Digest,
    pub archive: String,
    pub archive_sha256: Sha256Digest,
    pub payload_tree_sha256: Sha256Digest,
}

/// Fresh retained output, requiring independent ESP image verification before
/// media composition. There is no implicit device/release publication method.
#[derive(Debug)]
pub struct BuiltIdfBootloader {
    pub work_root: PathBuf,
    pub receipt: IdfBootloaderReceipt,
}

/// Build only the source-selected bootloader under a single process deadline.
///
/// # Errors
/// Refuses missing/changed/unsafe inputs before reservation. Later failures
/// retain their private output/log root, never silently reuse a previous build.
pub fn build_idf_bootloader(
    request: &IdfBootloaderRequest<'_>,
) -> Result<BuiltIdfBootloader, ContractError> {
    if request.jobs == 0 || request.timeout.is_zero() || request.cancellation.is_cancelled() {
        return Err(failure(
            "IDF build requires positive budgets and a live cancellation token",
        ));
    }
    let deadline = Instant::now()
        .checked_add(request.timeout)
        .ok_or_else(|| failure("IDF deadline overflow"))?;
    validate_private_directory_nofollow(request.work_parent).map_err(io_failure)?;
    for path in [
        request.source_root,
        request.idf_root,
        request.compiler_root,
        request.cmake_root,
        request.cmake,
        request.ninja,
        request.git,
        request.constraints,
        request.work_parent,
    ] {
        if !path.is_absolute()
            || path
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            return Err(failure(
                "IDF inputs must be explicit absolute paths without traversal",
            ));
        }
    }
    let parent = request.work_parent.canonicalize().map_err(io_failure)?;
    for input in [
        request.source_root,
        request.idf_root,
        request.compiler_root,
        request.cmake_root,
        &request.python.work_root,
    ] {
        let input = input.canonicalize().map_err(io_failure)?;
        if input.starts_with(&parent) || parent.starts_with(&input) {
            return Err(failure("IDF work parent overlaps an input root"));
        }
    }
    let loaded = source_contract(request)?;
    verify_inputs(request)?;
    let project_files = project_inputs(request, &loaded)?;
    let root =
        create_unique_directory_nofollow(&parent, "aros-idf-bootloader").map_err(io_failure)?;
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).map_err(io_failure)?;
    build_owned(request, loaded, project_files, root, deadline)
        .map_err(ContractError::retained_material)
}

fn build_owned(
    request: &IdfBootloaderRequest<'_>,
    loaded: LoadedNativeBuildContract,
    project_files: Vec<(&'static str, Vec<u8>)>,
    root: PathBuf,
    deadline: Instant,
) -> Result<BuiltIdfBootloader, ContractError> {
    for relative in [
        "logs",
        "tmp",
        "home",
        "idf-tools",
        "host-bin",
        "project",
        "project/main",
    ] {
        fs::create_dir(root.join(relative)).map_err(io_failure)?;
    }
    let host_bin = root.join("host-bin");
    for (name, program) in [
        ("ninja", request.ninja),
        ("git", request.git),
        ("cmake", request.cmake),
    ] {
        alias_program(&host_bin, name, program)?;
    }
    // Resolving a venv interpreter symlink would discard its pyvenv.cfg context.
    // These fixed launchers preserve the selected interpreter's original path.
    for name in ["python", "python3"] {
        alias_interpreter(&host_bin, name, &request.python.interpreter)?;
    }
    for suffix in [
        "gcc",
        "g++",
        "as",
        "ld",
        "objcopy",
        "ar",
        "ranlib",
        "gcc-ar",
        "gcc-ranlib",
        "nm",
        "objdump",
        "readelf",
        "size",
        "addr2line",
        "strip",
    ] {
        let name = format!("{}-{suffix}", request.lock.lock.compiler_prefix);
        alias_program(
            &host_bin,
            &name,
            &request.compiler_root.join("bin").join(&name),
        )?;
    }
    for (relative, bytes) in project_files {
        write_new(&root.join("project").join(relative), &bytes)?;
    }
    write_new(&root.join("selected-inputs.json"), &request.lock.raw)?;
    let constraints = bytes(request.constraints, DOCUMENT_LIMIT)?;
    let minor = request
        .lock
        .lock
        .idf_version
        .split('.')
        .take(2)
        .collect::<Vec<_>>()
        .join(".");
    let constraint_path = root
        .join("idf-tools")
        .join(format!("espidf.constraints.v{minor}.txt"));
    write_new(&constraint_path, &constraints)?;
    // Git describe --dirty writes an index even with optional locks disabled.
    // Run the vendor consumer on an owned copy, never on immutable inputs.
    let execution_idf = root.join("idf-source");
    fs::create_dir(&execution_idf).map_err(io_failure)?;
    let limits = TreeTraversalLimits::new(300_000, 4 * 1024 * 1024 * 1024).map_err(io_failure)?;
    let input_snapshot =
        measure_tree_content_cas_bounded(request.idf_root, limits).map_err(io_failure)?;
    require_locked_idf_snapshot(&input_snapshot, &request.lock.lock.idf_tree_sha256)?;
    let execution_before =
        copy_tree_from_snapshot_nofollow(request.idf_root, &execution_idf, &input_snapshot, limits)
            .map_err(io_failure)?;
    require_locked_idf_snapshot(&execution_before, &request.lock.lock.idf_tree_sha256)?;
    let index_before = digest(&execution_idf.join(".git/index"), 128 * 1024 * 1024)?;
    let execution_request = IdfBootloaderRequest {
        idf_root: &execution_idf,
        ..*request
    };
    let project = root.join("project");
    let build = root.join("build");
    let mut environment = build_environment(&execution_request, &root)?;
    // All inputs were validated before reservation; repeat at execution boundary.
    verify_inputs(request)?;
    let mut phases = Vec::new();
    let probe = vec![
        "-I".into(),
        "-B".into(),
        text(&execution_idf.join("tools/check_python_dependencies.py"))?,
        "-r".into(),
        text(&execution_idf.join("tools/requirements/requirements.core.txt"))?,
        "-c".into(),
        text(&constraint_path)?,
    ];
    run_phase(
        &root,
        "idf-python-dependencies",
        &request.python.interpreter,
        &probe,
        &environment,
        deadline,
        request.cancellation,
        &mut phases,
    )?;
    request.python.revalidate()?;
    let arguments = vec![
        "-G".into(),
        "Ninja".into(),
        "-S".into(),
        text(&project)?,
        "-B".into(),
        text(&build)?,
        format!("-DIDF_TARGET={}", loaded.contract.media.chip),
        format!(
            "-DSDKCONFIG_DEFAULTS={}",
            text(&project.join("sdkconfig.defaults"))?
        ),
        format!("-DSDKCONFIG={}", text(&root.join("sdkconfig"))?),
        format!("-DPYTHON={}", text(&request.python.interpreter)?),
        format!("-DCMAKE_MAKE_PROGRAM={}", text(request.ninja)?),
        format!("-DGIT_EXECUTABLE={}", text(request.git)?),
        "-DIDF_COMPONENT_MANAGER=0".into(),
    ];
    run_phase(
        &root,
        "idf-configure",
        request.cmake,
        &arguments,
        &environment,
        deadline,
        request.cancellation,
        &mut phases,
    )?;
    verify_configured_tools(&build, request, &loaded.contract.media.chip)?;
    request.python.revalidate()?;
    environment.insert(
        "CMAKE_BUILD_PARALLEL_LEVEL".into(),
        request.jobs.to_string(),
    );
    let arguments = vec![
        "--build".into(),
        text(&build)?,
        "--target".into(),
        "bootloader".into(),
        "--parallel".into(),
        request.jobs.to_string(),
    ];
    run_phase(
        &root,
        "idf-bootloader",
        request.cmake,
        &arguments,
        &environment,
        deadline,
        request.cancellation,
        &mut phases,
    )?;
    verify_configured_tools(&build, request, &loaded.contract.media.chip)?;
    verify_configured_tools(
        &build.join("bootloader"),
        request,
        &loaded.contract.media.chip,
    )?;
    verify_inputs(request)?;
    let execution_after =
        measure_tree_content_cas_bounded(&execution_idf, limits).map_err(io_failure)?;
    if !execution_before.matches_content_except_regular_file(&execution_after, ".git/index") {
        return Err(failure(
            "owned IDF source changed outside its explicit Git index metadata",
        ));
    }
    if source_contract(request)? != loaded {
        return Err(failure("native source contract changed during IDF build"));
    }
    if bytes(&root.join("selected-inputs.json"), DOCUMENT_LIMIT)? != request.lock.raw
        || bytes(&constraint_path, DOCUMENT_LIMIT)? != constraints
    {
        return Err(failure("owned IDF control inputs changed during build"));
    }
    if Instant::now() >= deadline || request.cancellation.is_cancelled() {
        return Err(failure(
            "IDF build deadline/cancellation reached during validation",
        ));
    }
    let receipt = IdfBootloaderReceipt {
        schema_version: 1,
        format: "aros-idf-bootloader-receipt-v1",
        qualification: "local-byte-consistency-only",
        lock_sha256: request.lock.sha256.clone(),
        source_contract_sha256: loaded.sha256,
        source_baseline: loaded.contract.source_baseline,
        profile: loaded.contract.profile,
        board: loaded.contract.board,
        chip: loaded.contract.media.chip,
        python_receipt_sha256: digest(
            &request.python.work_root.join("environment.receipt.json"),
            DOCUMENT_LIMIT,
        )?,
        idf_root: request.idf_root.to_owned(),
        idf_source_receipt: request.idf_source.receipt_path().to_owned(),
        idf_source_receipt_sha256: request.idf_source.receipt_sha256().clone(),
        vendor_sources: verify_vendor_sources(request)?,
        execution_idf_root: execution_idf.clone(),
        execution_idf_tree_before: execution_before.payload_digest_excluding(None),
        execution_idf_tree_after: execution_after.payload_digest_excluding(None),
        execution_git_index_before: index_before,
        execution_git_index_after: digest(&execution_idf.join(".git/index"), 128 * 1024 * 1024)?,
        compiler_root: request.compiler_root.to_owned(),
        cmake_root: request.cmake_root.to_owned(),
        cmake: request.cmake.to_owned(),
        ninja: request.ninja.to_owned(),
        git: request.git.to_owned(),
        work_root: root.clone(),
        phases,
        bootloader: output(&build.join("bootloader/bootloader.bin"), 1024 * 1024)?,
        elf: output(&build.join("bootloader/bootloader.elf"), 64 * 1024 * 1024)?,
    };
    write_new(
        &root.join("bootloader.receipt.json"),
        &serde_json::to_vec_pretty(&receipt).map_err(|_| failure("cannot encode IDF receipt"))?,
    )?;
    Ok(BuiltIdfBootloader {
        work_root: root,
        receipt,
    })
}

fn source_contract(
    request: &IdfBootloaderRequest<'_>,
) -> Result<LoadedNativeBuildContract, ContractError> {
    let relative = request
        .profile
        .native_build_contract
        .as_deref()
        .ok_or_else(|| failure("profile has no native build contract"))?;
    let loaded =
        load_bound_native_build_contract(request.source_root, Path::new(relative), request.profile)
            .map_err(|_| failure("native source input binding failed"))?;
    if loaded.sha256 != *request.native_contract_sha256
        || loaded.contract.media.idf_version != request.lock.lock.idf_version
    {
        return Err(failure(
            "IDF/source version or raw source contract binding differs",
        ));
    }
    Ok(loaded)
}

fn project_inputs(
    request: &IdfBootloaderRequest<'_>,
    loaded: &LoadedNativeBuildContract,
) -> Result<Vec<(&'static str, Vec<u8>)>, ContractError> {
    let config = Path::new(&loaded.contract.media.bootloader_configuration);
    let parent = config
        .parent()
        .ok_or_else(|| failure("missing source-owned IDF project"))?;
    let mut result = Vec::new();
    for relative in [
        "CMakeLists.txt",
        "main/CMakeLists.txt",
        "main/main.c",
        "sdkconfig.defaults",
    ] {
        let path = if relative == "sdkconfig.defaults" {
            config.to_owned()
        } else {
            parent.join(relative)
        };
        let input = loaded
            .contract
            .inputs
            .iter()
            .find(|entry| Path::new(&entry.path) == path)
            .ok_or_else(|| failure("IDF project input is not in native source inventory"))?;
        let raw = bytes(&request.source_root.join(&path), DOCUMENT_LIMIT)?;
        if sha256_bytes(&raw) != input.sha256 {
            return Err(failure("source-owned IDF project bytes changed"));
        }
        result.push((relative, raw));
    }
    Ok(result)
}

fn verify_inputs(request: &IdfBootloaderRequest<'_>) -> Result<(), ContractError> {
    request.python.revalidate()?;
    let lock = &request.lock.lock;
    verify_idf_source_binding(request)?;
    verify_vendor_sources(request)?;
    if request.python.receipt.lock_sha256 != lock.wheel_lock_sha256
        || digest(request.ninja, 128 * 1024 * 1024)? != lock.ninja_sha256
        || digest(request.git, 128 * 1024 * 1024)? != lock.git_sha256
        || digest(request.constraints, DOCUMENT_LIMIT)? != lock.constraints_sha256
        || digest(
            &request
                .idf_root
                .join("tools/requirements/requirements.core.txt"),
            DOCUMENT_LIMIT,
        )? != lock.requirements_sha256
    {
        return Err(failure(
            "prepared IDF external input bytes differ from the lock",
        ));
    }
    if tree_digest(request.idf_root)? != lock.idf_tree_sha256
        || tree_digest(request.compiler_root)? != lock.compiler_tree_sha256
        || tree_digest(request.cmake_root)? != lock.cmake_tree_sha256
    {
        return Err(failure(
            "prepared IDF external input tree differs from the lock",
        ));
    }
    let cmake = request.cmake.canonicalize().map_err(io_failure)?;
    let prefix = request.cmake_root.canonicalize().map_err(io_failure)?;
    if !cmake.starts_with(&prefix) {
        return Err(failure("CMake executable escapes its measured prefix"));
    }
    bytes(&cmake, 128 * 1024 * 1024)?;
    for suffix in ["gcc", "g++", "as", "ld", "objcopy", "ar", "ranlib"] {
        bytes(
            &request
                .compiler_root
                .join("bin")
                .join(format!("{}-{suffix}", lock.compiler_prefix)),
            128 * 1024 * 1024,
        )?;
    }
    let chip = source_contract(request)?.contract.media.chip;
    let toolchain = request
        .idf_root
        .join("tools/cmake")
        .join(format!("toolchain-{chip}.cmake"));
    let toolchain = String::from_utf8(bytes(&toolchain, DOCUMENT_LIMIT)?)
        .map_err(|_| failure("IDF toolchain file is not UTF-8"))?;
    require_toolchain_prefix(&toolchain, &lock.compiler_prefix)?;
    Ok(())
}

fn verify_idf_source_binding(request: &IdfBootloaderRequest<'_>) -> Result<(), ContractError> {
    request.idf_source.revalidate().map_err(|error| {
        failure(&format!(
            "IDF archive/patch source receipt no longer matches its tree: {error}"
        ))
    })?;
    if *request.idf_source.receipt_sha256() != request.lock.lock.idf_source_receipt_sha256 {
        return Err(failure(
            "IDF source receipt differs from the selected byte lock",
        ));
    }
    let loaded = source_contract(request)?;
    let patch_path = Path::new(&loaded.contract.media.bootloader_patch);
    let patch_name = patch_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| failure("source patch has no portable filename"))?;
    let inventoried = loaded
        .contract
        .inputs
        .iter()
        .find(|input| Path::new(&input.path) == patch_path)
        .ok_or_else(|| failure("IDF patch is not in the source inventory"))?;
    let selected = request.idf_source.request();
    let [patch] = selected.patches.as_slice() else {
        return Err(failure(
            "IDF source must bind exactly its inventoried source patch",
        ));
    };
    let directory = patch
        .subdirectory
        .as_deref()
        .ok_or_else(|| failure("IDF source patch must select its extracted source directory"))?;
    if patch.name != patch_name
        || selected.checksums.get(&patch.name) != Some(&inventoried.sha256)
        || request.idf_root.canonicalize().map_err(io_failure)?
            != request
                .idf_source
                .destination()
                .join(directory)
                .canonicalize()
                .map_err(io_failure)?
    {
        return Err(failure(
            "IDF prepared source/patch differs from its source-owned binding",
        ));
    }
    Ok(())
}

fn verify_vendor_sources(
    request: &IdfBootloaderRequest<'_>,
) -> Result<Vec<IdfVendorSourceReceipt>, ContractError> {
    let metadata: serde_json::Value = serde_json::from_slice(&bytes(
        &request.idf_root.join("tools/tools.json"),
        DOCUMENT_LIMIT,
    )?)
    .map_err(|_| failure("invalid IDF vendor tool metadata"))?;
    let platform = match request.lock.lock.host.as_str() {
        "macos-aarch64" => "macos-arm64",
        "linux-x86_64" => "linux-amd64",
        "linux-aarch64" => "linux-arm64",
        _ => return Err(failure("unsupported IDF vendor host platform")),
    };
    let mut receipts = Vec::new();
    for (tool, source, selected, expected) in [
        (
            request.lock.lock.compiler_prefix.as_str(),
            request.compiler_source,
            request.compiler_root,
            &request.lock.lock.compiler_source_receipt_sha256,
        ),
        (
            "cmake",
            request.cmake_source,
            request.cmake_root,
            &request.lock.lock.cmake_source_receipt_sha256,
        ),
        (
            "ninja",
            request.ninja_source,
            request.ninja,
            &request.lock.lock.ninja_source_receipt_sha256,
        ),
    ] {
        source
            .revalidate()
            .map_err(|error| failure(&format!("vendor archive receipt/tree changed: {error}")))?;
        if source.receipt_sha256() != expected {
            return Err(failure(
                "vendor source receipt differs from the selected byte lock",
            ));
        }
        let selected = selected.canonicalize().map_err(io_failure)?;
        if !selected.starts_with(source.destination()) {
            return Err(failure(
                "selected vendor tool escapes its archive receipt tree",
            ));
        }
        let declaration = source.request();
        let [archive] = declaration.archive_candidates.as_slice() else {
            return Err(failure("vendor tool must select one archive"));
        };
        let archive_sha256 = declaration
            .checksums
            .get(archive)
            .ok_or_else(|| failure("vendor archive checksum missing"))?;
        if !declaration.patches.is_empty() {
            return Err(failure("IDF vendor tool archives must be unpatched"));
        }
        require_vendor_archive(&metadata, tool, platform, archive, archive_sha256)?;
        receipts.push(IdfVendorSourceReceipt {
            tool: tool.into(),
            receipt_path: source.receipt_path().to_owned(),
            receipt_sha256: source.receipt_sha256().clone(),
            archive: archive.clone(),
            archive_sha256: archive_sha256.clone(),
            payload_tree_sha256: source.payload_tree_sha256().clone(),
        });
    }
    Ok(receipts)
}

// Vendor metadata is owned by the locked IDF tree. Do not hardcode tool
// versions or accept a same-named host executable outside its archive tree.
fn require_vendor_archive(
    metadata: &serde_json::Value,
    tool: &str,
    platform: &str,
    archive: &str,
    checksum: &Sha256Digest,
) -> Result<(), ContractError> {
    let tools = metadata["tools"]
        .as_array()
        .ok_or_else(|| failure("missing vendor tools list"))?;
    let matches = tools
        .iter()
        .filter(|entry| entry["name"].as_str() == Some(tool))
        .collect::<Vec<_>>();
    let [entry] = matches.as_slice() else {
        return Err(failure("vendor tool selection is missing or ambiguous"));
    };
    let versions = entry["versions"]
        .as_array()
        .ok_or_else(|| failure("missing vendor tool versions"))?;
    let matches = versions
        .iter()
        .filter(|version| version["status"].as_str() == Some("recommended"))
        .collect::<Vec<_>>();
    let [version] = matches.as_slice() else {
        return Err(failure(
            "recommended vendor version is missing or ambiguous",
        ));
    };
    let selected = &version[platform];
    let url = selected["url"]
        .as_str()
        .ok_or_else(|| failure("vendor tool has no selected host archive"))?;
    if url.rsplit('/').next() != Some(archive)
        || selected["sha256"].as_str() != Some(checksum.as_str())
        || selected["size"].as_u64().is_none_or(|size| size == 0)
    {
        return Err(failure(
            "vendor archive differs from locked IDF tool metadata",
        ));
    }
    Ok(())
}

fn require_locked_idf_snapshot(
    snapshot: &aros_common::TreeContentCas,
    expected: &Sha256Digest,
) -> Result<(), ContractError> {
    if snapshot.payload_digest_excluding(None) != *expected {
        return Err(failure(
            "IDF execution snapshot differs from the locked input tree",
        ));
    }
    Ok(())
}

fn require_toolchain_prefix(toolchain: &str, prefix: &str) -> Result<(), ContractError> {
    let expected = format!("set(_CMAKE_TOOLCHAIN_PREFIX {prefix}-)");
    if toolchain
        .lines()
        .filter(|line| {
            line.trim_start()
                .starts_with("set(_CMAKE_TOOLCHAIN_PREFIX ")
        })
        .map(str::trim)
        .collect::<Vec<_>>()
        != [expected.as_str()]
    {
        return Err(failure(
            "IDF target compiler prefix differs from the byte lock",
        ));
    }
    Ok(())
}

fn cache_value<'a>(cache: &'a str, key: &str) -> Result<&'a str, ContractError> {
    let values = cache
        .lines()
        .filter_map(|line| {
            let (name, value) = line.split_once('=')?;
            let (name, _) = name.split_once(':')?;
            (name == key).then_some(value)
        })
        .collect::<Vec<_>>();
    match values.as_slice() {
        [value] => Ok(value),
        _ => Err(failure("CMake selection is missing or ambiguous")),
    }
}

fn verify_configured_tools(
    build: &Path,
    request: &IdfBootloaderRequest<'_>,
    chip: &str,
) -> Result<(), ContractError> {
    let cache = String::from_utf8(bytes(&build.join("CMakeCache.txt"), DOCUMENT_LIMIT)?)
        .map_err(|_| failure("CMake cache is not UTF-8"))?;
    for (key, expected) in [
        ("CMAKE_COMMAND", request.cmake),
        ("CMAKE_MAKE_PROGRAM", request.ninja),
        ("GIT_EXECUTABLE", request.git),
        ("PYTHON", request.python.interpreter.as_path()),
    ] {
        require_tool_selection(Path::new(cache_value(&cache, key)?), expected)?;
    }
    if cache_value(&cache, "IDF_TARGET")? != chip {
        return Err(failure(
            "configured IDF target differs from source contract",
        ));
    }
    let version = [
        "CMAKE_CACHE_MAJOR_VERSION",
        "CMAKE_CACHE_MINOR_VERSION",
        "CMAKE_CACHE_PATCH_VERSION",
    ]
    .iter()
    .map(|key| cache_value(&cache, key))
    .collect::<Result<Vec<_>, _>>()?;
    if version
        .iter()
        .any(|part| part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(failure("configured CMake version directory is unsafe"));
    }
    let metadata = build.join("CMakeFiles").join(version.join("."));
    for (language, suffix) in [("C", "gcc"), ("CXX", "g++"), ("ASM", "gcc")] {
        let raw = String::from_utf8(bytes(
            &metadata.join(format!("CMake{language}Compiler.cmake")),
            DOCUMENT_LIMIT,
        )?)
        .map_err(|_| failure("CMake compiler metadata is not UTF-8"))?;
        let prefix = format!("set(CMAKE_{language}_COMPILER \"");
        let selected = raw
            .lines()
            .filter_map(|line| {
                line.strip_prefix(&prefix)
                    .and_then(|value| value.strip_suffix("\")"))
            })
            .collect::<Vec<_>>();
        let [selected] = selected.as_slice() else {
            return Err(failure(
                "configured compiler selection is missing or ambiguous",
            ));
        };
        let expected = request
            .compiler_root
            .join("bin")
            .join(format!("{}-{suffix}", request.lock.lock.compiler_prefix));
        require_tool_selection(Path::new(selected), &expected)?;
    }
    Ok(())
}

fn require_tool_selection(selected: &Path, expected: &Path) -> Result<(), ContractError> {
    if !selected.is_absolute()
        || selected.canonicalize().map_err(io_failure)?
            != expected.canonicalize().map_err(io_failure)?
    {
        return Err(failure(
            "configured IDF tool escapes its explicit selection",
        ));
    }
    Ok(())
}

fn alias_program(bin: &Path, name: &str, program: &Path) -> Result<(), ContractError> {
    let program = program.canonicalize().map_err(io_failure)?;
    bytes(&program, 128 * 1024 * 1024)?;
    if fs::metadata(&program)
        .map_err(io_failure)?
        .permissions()
        .mode()
        & 0o111
        == 0
        || rustix::fs::access(&program, rustix::fs::Access::EXEC_OK).is_err()
    {
        return Err(failure("selected IDF tool is not executable"));
    }
    std::os::unix::fs::symlink(program, bin.join(name)).map_err(io_failure)
}

fn alias_interpreter(bin: &Path, name: &str, program: &Path) -> Result<(), ContractError> {
    let program = text(program)?.replace('\'', "'\"'\"'");
    let path = bin.join(name);
    write_new(
        &path,
        format!("#!/bin/sh\nexec '{program}' \"$@\"\n").as_bytes(),
    )?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(io_failure)
}

fn controlled_path(root: &Path) -> Result<String, ContractError> {
    std::env::join_paths([
        root.join("host-bin"),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/bin"),
    ])
    .map_err(|_| failure("IDF tool path cannot be encoded"))?
    .into_string()
    .map_err(|_| failure("IDF tool path must be UTF-8"))
}

fn build_environment(
    request: &IdfBootloaderRequest<'_>,
    root: &Path,
) -> Result<BTreeMap<String, String>, ContractError> {
    let mut result = base_environment(root);
    for (key, value) in [
        ("PATH", controlled_path(root)?),
        ("HOME", text(&root.join("home"))?),
        ("IDF_PATH", text(request.idf_root)?),
        ("IDF_TOOLS_PATH", text(&root.join("idf-tools"))?),
        (
            "IDF_PYTHON_ENV_PATH",
            text(&request.python.receipt.environment_root)?,
        ),
        ("IDF_PYTHON_CHECK_CONSTRAINTS", "1".into()),
        ("IDF_COMPONENT_MANAGER", "0".into()),
        ("IDF_SKIP_CHECK_SUBMODULES", "1".into()),
        ("IDF_CCACHE_ENABLE", "0".into()),
        ("GIT_CONFIG_NOSYSTEM", "1".into()),
        ("GIT_CONFIG_GLOBAL", "/dev/null".into()),
        ("GIT_CONFIG_COUNT", "0".into()),
        // Suppress optional writes; describe's explicit refresh uses an owned copy.
        ("GIT_OPTIONAL_LOCKS", "0".into()),
    ] {
        result.insert(key.into(), value);
    }
    Ok(result)
}

fn tree_digest(root: &Path) -> Result<Sha256Digest, ContractError> {
    let limits = TreeTraversalLimits::new(300_000, 4 * 1024 * 1024 * 1024).map_err(io_failure)?;
    measure_tree_content_cas_bounded(root, limits)
        .map(|cas| cas.payload_digest_excluding(None))
        .map_err(io_failure)
}
fn bytes(path: &Path, maximum: u64) -> Result<Vec<u8>, ContractError> {
    measure_regular_file_bounded(path, maximum)
        .map_err(io_failure)?
        .map(|(_, raw)| raw)
        .ok_or_else(|| failure("IDF input/output is not a bounded no-follow regular file"))
}
fn digest(path: &Path, maximum: u64) -> Result<Sha256Digest, ContractError> {
    bytes(path, maximum).map(|raw| sha256_bytes(&raw))
}
fn output(path: &Path, maximum: u64) -> Result<IdfOutput, ContractError> {
    let raw = bytes(path, maximum)?;
    if raw.is_empty() {
        return Err(failure("IDF output is empty"));
    }
    Ok(IdfOutput {
        path: path.to_owned(),
        size_bytes: raw.len() as u64,
        sha256: sha256_bytes(&raw),
    })
}
fn text(path: &Path) -> Result<String, ContractError> {
    path.to_str()
        .filter(|value| !value.chars().any(char::is_control))
        .map(str::to_owned)
        .ok_or_else(|| failure("IDF path must be safe UTF-8"))
}
fn write_new(path: &Path, raw: &[u8]) -> Result<(), ContractError> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(io_failure)?;
    file.write_all(raw)
        .and_then(|()| file.sync_all())
        .map_err(io_failure)
}
fn failure(message: &str) -> ContractError {
    ContractError::environment(message)
}
fn io_failure(_: std::io::Error) -> ContractError {
    failure("IDF filesystem boundary failed")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vendor_metadata_selects_one_exact_recommended_host_archive() {
        let checksum = sha256_bytes(b"vendor archive");
        let metadata = serde_json::json!({"tools":[{"name":"compiler","versions":[{
            "name":"any-version","status":"recommended","macos-arm64":{
                "url":"https://vendor.invalid/releases/tool.tar.xz","sha256":checksum,"size":14
            }
        }]}]});
        require_vendor_archive(
            &metadata,
            "compiler",
            "macos-arm64",
            "tool.tar.xz",
            &checksum,
        )
        .unwrap();
        for (tool, host, archive, digest) in [
            ("other", "macos-arm64", "tool.tar.xz", checksum.clone()),
            ("compiler", "linux-amd64", "tool.tar.xz", checksum.clone()),
            ("compiler", "macos-arm64", "other.tar.xz", checksum.clone()),
            (
                "compiler",
                "macos-arm64",
                "tool.tar.xz",
                sha256_bytes(b"other"),
            ),
        ] {
            assert!(require_vendor_archive(&metadata, tool, host, archive, &digest).is_err());
        }
        let mut changed = metadata.clone();
        changed["tools"]
            .as_array_mut()
            .unwrap()
            .push(metadata["tools"][0].clone());
        assert!(require_vendor_archive(
            &changed,
            "compiler",
            "macos-arm64",
            "tool.tar.xz",
            &checksum
        )
        .is_err());
        let mut changed = metadata.clone();
        changed["tools"][0]["versions"]
            .as_array_mut()
            .unwrap()
            .push(metadata["tools"][0]["versions"][0].clone());
        assert!(require_vendor_archive(
            &changed,
            "compiler",
            "macos-arm64",
            "tool.tar.xz",
            &checksum
        )
        .is_err());
        for (key, value) in [
            ("size", serde_json::json!(0)),
            ("size", serde_json::json!(-1)),
            ("sha256", serde_json::json!("unknown")),
            ("url", serde_json::json!(null)),
        ] {
            let mut changed = metadata.clone();
            changed["tools"][0]["versions"][0]["macos-arm64"][key] = value;
            assert!(require_vendor_archive(
                &changed,
                "compiler",
                "macos-arm64",
                "tool.tar.xz",
                &checksum
            )
            .is_err());
        }
    }

    #[test]
    fn execution_snapshot_must_match_the_lock_not_only_its_copy() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        fs::write(root.join("source.c"), b"locked source").unwrap();
        let limits = TreeTraversalLimits::new(100, 1024).unwrap();
        let initial = measure_tree_content_cas_bounded(&root, limits).unwrap();
        let locked = initial.payload_digest_excluding(None);
        require_locked_idf_snapshot(&initial, &locked).unwrap();
        fs::write(root.join("source.c"), b"coherent changed source").unwrap();
        let changed = measure_tree_content_cas_bounded(&root, limits).unwrap();
        assert!(require_locked_idf_snapshot(&changed, &locked).is_err());
        fs::write(root.join("source.c"), b"locked source").unwrap();
        assert!(require_locked_idf_snapshot(&changed, &locked).is_err());
    }

    #[test]
    fn configured_cache_values_must_be_exact_and_unambiguous() {
        let cache =
            "// ignored comment\nOTHER:STRING=value\nPYTHON:UNINITIALIZED=/private/python\n";
        assert_eq!(cache_value(cache, "PYTHON").unwrap(), "/private/python");
        assert!(cache_value(cache, "MISSING").is_err());
        assert!(cache_value("PYTHON:STRING=/one\nPYTHON:STRING=/two\n", "PYTHON").is_err());
        assert!(cache_value("PYTHON_WRAPPER:STRING=/other\n", "PYTHON").is_err());
    }

    #[test]
    fn toolchain_prefix_requires_one_exact_source_selection() {
        let expected = "set(_CMAKE_TOOLCHAIN_PREFIX riscv32-esp-elf-)";
        require_toolchain_prefix(expected, "riscv32-esp-elf").unwrap();
        require_toolchain_prefix(&format!("  {expected}\n"), "riscv32-esp-elf").unwrap();
        for raw in [
            "",
            "set(_CMAKE_TOOLCHAIN_PREFIX other-)",
            &format!("{expected}\n{expected}"),
        ] {
            assert!(require_toolchain_prefix(raw, "riscv32-esp-elf").is_err());
        }
    }

    #[test]
    fn configured_program_must_be_absolute_and_match_selected_bytes() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let selected = root.join("selected");
        let other = root.join("other");
        fs::write(&selected, b"selected").unwrap();
        fs::write(&other, b"other").unwrap();
        fs::set_permissions(&selected, fs::Permissions::from_mode(0o700)).unwrap();
        alias_program(&root, "alias", &selected).unwrap();
        require_tool_selection(&root.join("alias"), &selected).unwrap();
        assert!(require_tool_selection(Path::new("selected"), &selected).is_err());
        assert!(require_tool_selection(&other, &selected).is_err());
    }

    #[test]
    fn non_executable_selected_tool_is_refused_before_alias_publication() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let selected = root.join("selected");
        fs::write(&selected, b"#!/bin/sh\nexit 0\n").unwrap();
        for mode in [0o600, 0o644] {
            fs::set_permissions(&selected, fs::Permissions::from_mode(mode)).unwrap();
            assert!(alias_program(&root, "git", &selected).is_err());
            assert!(!root.join("git").exists());
        }
    }

    #[test]
    fn selected_path_excludes_shadowing_tool_and_wheel_directories() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let bin = root.join("host-bin");
        let vendor = root.join("vendor-bin");
        let wheels = root.join("wheel-bin");
        fs::create_dir(&bin).unwrap();
        fs::create_dir(&vendor).unwrap();
        fs::create_dir(&wheels).unwrap();
        let marker = root.join("shadow-executed");
        for directory in [&vendor, &wheels] {
            for name in ["git", "riscv32-esp-elf-gcc", "python3"] {
                let path = directory.join(name);
                fs::write(
                    &path,
                    format!("#!/bin/sh\ntouch '{}'\nexit 99\n", marker.display()),
                )
                .unwrap();
                fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            }
        }
        for name in ["git", "riscv32-esp-elf-gcc"] {
            alias_program(&bin, name, Path::new("/usr/bin/true")).unwrap();
        }
        // Include a quote in the selected path to exercise literal argument handling.
        let python = root.join("selected'python");
        fs::write(&python, b"#!/bin/sh\n[ \"$1\" = 'literal argument' ]\n").unwrap();
        fs::set_permissions(&python, fs::Permissions::from_mode(0o700)).unwrap();
        alias_interpreter(&bin, "python3", &python).unwrap();
        let path = controlled_path(&root).unwrap();
        assert_eq!(
            std::env::split_paths(&path).collect::<Vec<_>>(),
            [bin, PathBuf::from("/usr/bin"), PathBuf::from("/bin")]
        );
        let status = std::process::Command::new("/bin/sh")
            .args([
                "-c",
                "git && riscv32-esp-elf-gcc && python3 'literal argument'",
            ])
            .env_clear()
            .env("PATH", path)
            .status()
            .unwrap();
        assert!(status.success());
        assert!(!marker.exists());
    }
}
