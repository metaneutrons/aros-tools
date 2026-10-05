//! Locked AROS cross-toolchain installation, resolution, and verification.

use crate::artifact::{
    aros_home, command_exists, commit_staging, extract_to_staging, obtain_archive,
    require_absolute_state_path, INSTALL_COMPLETE_FILE,
};
use crate::host_compiler::host_platform_key;
use crate::toolchain_management::ResultFormat;
use aros_common::local_toolchain::{LocalToolchainDescriptor, LOCAL_TOOLCHAIN_DESCRIPTOR_FILE};
use aros_common::target::TargetProfile;
use aros_common::toolchain_layout::{ToolchainToolLayout, TOOLCHAIN_TOOLS_FILE};
use aros_common::toolchain_manifest::{
    ArosCompilerIdentity, ArosToolchainArtifact, ArosToolchainLock, ArosToolchainManifest,
    AROS_TOOLCHAIN_MANIFEST_FILE,
};
use aros_common::toolchain_tree_inventory;
use console::{style, Emoji};
use miette::{bail, IntoDiagnostic, Result, WrapErr};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

static CHECK: Emoji<'_, '_> = Emoji("✅ ", "");
static DOWNLOAD: Emoji<'_, '_> = Emoji("⬇️  ", "");
const REQUIRED_CXX_HEADERS: &[&str] = &[
    "algorithm",
    "cerrno",
    "cinttypes",
    "cstddef",
    "cstdint",
    "deque",
    "memory",
    "string",
    "system_error",
    "vector",
];
// A newly downloaded macOS executable can spend several seconds in the host's
// first-launch security assessment before it reaches `main`. Keep the probe
// bounded, but allow that legitimate cold-start path to finish.
const TOOLCHAIN_PROBE_TIMEOUT: Duration = Duration::from_secs(30);
const LIST_SCHEMA: &str = "aros-toolchain-list-v1";

/// Required executable layout of an installed AROS cross-toolchain.
#[derive(Debug, Clone)]
pub struct ToolchainPaths {
    /// Installation payload root.
    pub root: PathBuf,
    /// Exact executable roles, resolved only beneath this payload root.
    /// Legacy LLVM names remain visible as roles; GNU roles come from its
    /// inventory-bound layout document rather than guessed filenames.
    pub executable_roles: Vec<(&'static str, PathBuf)>,
}

/// Provenance class of a resolved cross-toolchain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolchainSource {
    /// Content-addressed, lock-file-backed release asset.
    LockedRelease,
    /// Explicit local tree carrying a complete manifest.
    LocalManifest,
    /// Explicit legacy AROS-built prefix accepted by its markers.
    LegacyLocal,
    /// Explicit local GNU compiler-only prefix with byte-verified metadata.
    LocalCompilerOnly,
}

/// Verified toolchain selection used by one build.
#[derive(Debug, Clone)]
pub struct ResolvedToolchain {
    /// Required tool paths.
    pub paths: ToolchainPaths,
    /// Exact target triple supplied to the compiler.
    pub target_triple: String,
    /// Release identity, absent only for explicit local toolchains.
    pub release_id: Option<String>,
    /// Provenance class of the selection.
    pub source: ToolchainSource,
}

/// The durable effect observed while resolving a requested toolchain.
///
/// This is intentionally internal to the frontend. Command reporting uses it
/// to preserve actual mutation state if a later stdout or logger operation
/// fails; it never infers state from the command name or `--force` spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolchainInstallDisposition {
    /// An explicit local tree was verified without copying it.
    LocalOverride,
    /// An existing managed installation was verified and reused.
    Reused,
    /// Only the verified archive cache was refreshed.
    ArchiveRefreshed,
    /// A new verified managed installation was durably published.
    Published,
}

/// Resolved toolchain plus the exact installation effect observed by its
/// owner.
#[derive(Debug, Clone)]
pub struct ToolchainInstallOutcome {
    /// Verified toolchain selected for the caller.
    resolved: ResolvedToolchain,
    disposition: ToolchainInstallDisposition,
}

impl ToolchainInstallOutcome {
    const fn new(resolved: ResolvedToolchain, disposition: ToolchainInstallDisposition) -> Self {
        Self {
            resolved,
            disposition,
        }
    }

    /// Whether this invocation itself durably published a new installation.
    pub const fn publication_committed(&self) -> bool {
        matches!(self.disposition, ToolchainInstallDisposition::Published)
    }

    /// Consume the outcome after any frontend reporting observation is made.
    pub fn into_resolved(self) -> ResolvedToolchain {
        self.resolved
    }
}

/// Resolve the versioned toolchain lock-file path inside the selected checkout.
pub fn lock_file_path(repo_root: &Path) -> PathBuf {
    repo_root.join("aros-toolchains.lock.toml")
}

/// Load and validate the deterministic toolchain release lock.
///
/// # Errors
///
/// Returns an error when the lock cannot be read or violates its schema.
pub fn load_lock(repo_root: &Path) -> Result<ArosToolchainLock> {
    let path = lock_file_path(repo_root);
    ArosToolchainLock::load(&path)
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to load AROS toolchain lock '{}'", path.display()))
}

/// Return the root of the content-addressed cross-toolchain store.
pub fn default_store_root() -> Result<PathBuf> {
    match std::env::var_os("AROS_CROSS_TOOLCHAINS_DIR") {
        Some(path) => require_absolute_state_path("AROS_CROSS_TOOLCHAINS_DIR", PathBuf::from(path)),
        None => Ok(aros_home()?.join("cross-toolchains")),
    }
}

/// Resolve an explicit command-line local-toolchain override.
pub fn explicit_local_override(argument: Option<&Path>) -> Option<PathBuf> {
    argument.map(Path::to_path_buf)
}

/// Derive the retained legacy LLVM tool paths from one payload root.
/// GNU payloads must instead bind their explicit layout to their manifest.
pub fn get_toolchain_paths(root: &Path) -> ToolchainPaths {
    let llvm = crate::host_compiler::host_compiler_paths(root);
    ToolchainPaths {
        root: root.into(),
        executable_roles: vec![
            ("clang", llvm.clang),
            ("clang++", llvm.clangxx),
            ("ld.lld", llvm.lld),
            ("llvm-ar", llvm.llvm_ar),
            ("aros-collect", root.join("bin/aros-collect")),
            ("collect-aros", root.join("bin/collect-aros")),
            ("collect-aros32", root.join("bin/collect-aros32")),
        ],
    }
}

/// Load one target profile by its canonical name.
///
/// # Errors
///
/// Returns an error for invalid target configuration or an unknown profile.
pub fn target_profile(repo_root: &Path, name: &str) -> Result<TargetProfile> {
    crate::repo::load_target_profiles(repo_root)?
        .into_iter()
        .find(|profile| profile.name == name)
        .ok_or_else(|| miette::miette!("unknown target preset '{name}' in aros-targets.toml"))
}

/// Derive the canonical AROS compiler triple for a target profile.
pub fn target_triple_for_profile(profile: &TargetProfile) -> String {
    if profile
        .transpiler
        .as_ref()
        .is_some_and(|selectors| selectors.toolchain == "gnu")
    {
        let cpu = profile.arch.source_cpu();
        format!("{cpu}-aros")
    } else {
        format!("{}-unknown-aros", profile.arch)
    }
}

/// Bind an artifact to the checkout's explicit compiler-family/ABI selectors.
/// This checks metadata, not compiler execution or SDK completeness.
pub fn validate_artifact_profile(
    profile: &TargetProfile,
    artifact: &ArosToolchainArtifact,
) -> Result<()> {
    // Schema 1 historically permits an omitted llvm_version in a lock
    // entry. It still selects LLVM, but does not fabricate a version claim.
    if artifact.compiler.is_none() && artifact.llvm_version.is_none() {
        return validate_profile_family(profile, "llvm", &artifact.target_triple);
    }
    validate_profile_compiler(
        profile,
        &artifact
            .compiler_identity()
            .map_err(|error| miette::miette!("{error}"))?,
        &artifact.target_triple,
    )
}

pub fn validate_profile_compiler(
    profile: &TargetProfile,
    compiler: &ArosCompilerIdentity,
    triple: &str,
) -> Result<()> {
    validate_profile_family(profile, compiler.family(), triple)?;
    if let ArosCompilerIdentity::Gnu { target, .. } = compiler {
        compiler
            .validate_for_target(triple)
            .map_err(|error| miette::miette!("{error}"))?;
        if profile.float_abi.as_deref() != Some(target.abi()) {
            bail!(
                "GNU target ABI '{}' does not match the explicit float_abi of preset '{}'",
                target.abi(),
                profile.name
            );
        }
    }
    Ok(())
}

fn validate_profile_family(
    profile: &TargetProfile,
    compiler_family: &str,
    triple: &str,
) -> Result<()> {
    let family = profile
        .transpiler
        .as_ref()
        .map_or("llvm", |selectors| selectors.toolchain.as_str());
    if !matches!(family, "llvm" | "gnu") || family != compiler_family {
        bail!(
            "toolchain compiler family does not match preset '{}' ({family})",
            profile.name
        );
    }
    let expected = target_triple_for_profile(profile);
    if triple != expected {
        bail!(
            "locked target triple '{triple}' does not match preset '{}' ({expected})",
            profile.name
        );
    }
    Ok(())
}

/// Resolve one enabled locked archive against the selected checkout's target
/// contract without inspecting, downloading, extracting, or executing it.
///
/// # Errors
///
/// Returns an error for an unknown profile, absent or disabled matrix entry,
/// or a lock/profile triple mismatch.
pub fn select_locked_artifact<'a>(
    repo_root: &Path,
    lock: &'a ArosToolchainLock,
    host: &str,
    preset: &str,
) -> Result<&'a ArosToolchainArtifact> {
    let profile = target_profile(repo_root, preset)?;
    let compiler_profile = profile.toolchain_profile();
    let artifact = lock.resolve(host, compiler_profile).ok_or_else(|| {
        miette::miette!("no locked AROS toolchain for host '{host}' and compiler profile '{compiler_profile}' selected by preset '{preset}'")
    })?;
    validate_artifact_profile(&profile, artifact)?;
    if !artifact.enabled {
        bail!(
            "AROS toolchain {host}/{preset} is locked but disabled: {}",
            artifact
                .disabled_reason
                .as_deref()
                .unwrap_or("no release asset is available")
        );
    }
    Ok(artifact)
}

/// Return an artifact's content-addressed payload path.
pub fn locked_store_path(
    lock: &ArosToolchainLock,
    artifact: &ArosToolchainArtifact,
) -> Result<PathBuf> {
    Ok(locked_store_envelope(lock, artifact)?.join("toolchain"))
}

fn locked_store_envelope(
    lock: &ArosToolchainLock,
    artifact: &ArosToolchainArtifact,
) -> Result<PathBuf> {
    Ok(default_store_root()?
        .join(&lock.release_id)
        .join(&artifact.host)
        .join(&artifact.target_profile)
        .join(artifact.sha256.to_ascii_lowercase()))
}

/// Install, verify, or explicitly resolve one target cross-toolchain.
///
/// # Errors
///
/// Returns an error for unsupported matrix entries, disabled artifacts,
/// download or identity failures, invalid layouts, or unsafe destinations.
pub async fn install(
    repo_root: &Path,
    preset: &str,
    offline: bool,
    force: bool,
    local: Option<&Path>,
) -> Result<ToolchainInstallOutcome> {
    if let Some(local) = explicit_local_override(local) {
        let resolved = resolve_local(repo_root, &local, preset)?;
        aros_common::outputln!(
            "{CHECK} Using local AROS toolchain without copying it: {}",
            local.display()
        );
        return Ok(ToolchainInstallOutcome::new(
            resolved,
            ToolchainInstallDisposition::LocalOverride,
        ));
    }

    let host = host_platform_key()?;
    let lock = load_lock(repo_root)?;
    let artifact = select_locked_artifact(repo_root, &lock, host, preset)?;

    let envelope = locked_store_envelope(&lock, artifact)?;
    let payload = envelope.join("toolchain");
    match fs::symlink_metadata(&envelope) {
        Ok(_) => {
            let paths = verify_locked_install(&payload, &lock, artifact, true).wrap_err_with(|| {
                format!(
                    "content-addressed destination '{}' already exists but is invalid; it was not overwritten",
                    envelope.display()
                )
            })?;
            if !force {
                return Ok(ToolchainInstallOutcome::new(
                    resolved_locked(paths, &lock, artifact),
                    ToolchainInstallDisposition::Reused,
                ));
            }
            aros_common::outputln!(
                "{DOWNLOAD} Refreshing cached AROS toolchain archive {} for {} / {}",
                style(&lock.release_id).cyan(),
                style(host).yellow(),
                style(preset).yellow()
            );
            obtain_archive(
                &lock
                    .asset_url(artifact)
                    .map_err(|error| miette::miette!("{error}"))?,
                &artifact.sha256,
                artifact.size,
                offline,
                true,
            )
            .await?;
            aros_common::outputln!(
                "{CHECK} Refreshed the verified archive cache; installed toolchain was unchanged"
            );
            return Ok(ToolchainInstallOutcome::new(
                resolved_locked(paths, &lock, artifact),
                ToolchainInstallDisposition::ArchiveRefreshed,
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error)
                .into_diagnostic()
                .wrap_err_with(|| format!("failed to inspect '{}'", envelope.display()));
        }
    }

    aros_common::outputln!(
        "{DOWNLOAD} AROS toolchain {} for {} / {}",
        style(&lock.release_id).cyan(),
        style(host).yellow(),
        style(preset).yellow()
    );
    let archive = obtain_archive(
        &lock
            .asset_url(artifact)
            .map_err(|error| miette::miette!("{error}"))?,
        &artifact.sha256,
        artifact.size,
        offline,
        force,
    )
    .await?;

    match fs::symlink_metadata(&envelope) {
        Ok(_) => {
            let paths = verify_locked_install(&payload, &lock, artifact, true).wrap_err_with(|| {
                format!(
                    "content-addressed destination '{}' appeared during installation but is invalid; it was not overwritten",
                    envelope.display()
                )
            })?;
            return Ok(ToolchainInstallOutcome::new(
                resolved_locked(paths, &lock, artifact),
                ToolchainInstallDisposition::Reused,
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error)
                .into_diagnostic()
                .wrap_err_with(|| format!("failed to inspect '{}'", envelope.display()));
        }
    }

    let parent = envelope
        .parent()
        .ok_or_else(|| miette::miette!("toolchain destination has no parent"))?;
    let payload_staging = extract_to_staging(archive.path(), parent, artifact.strip_components)?;
    verify_locked_install(payload_staging.path(), &lock, artifact, false)?;
    let envelope_staging = tempfile::Builder::new()
        .prefix(".envelope-")
        .tempdir_in(parent)
        .into_diagnostic()
        .wrap_err("failed to create toolchain envelope staging directory")?;
    fs::rename(
        payload_staging.path(),
        envelope_staging.path().join("toolchain"),
    )
    .into_diagnostic()
    .wrap_err("failed to place verified payload in installation envelope")?;
    fs::write(
        envelope_staging.path().join(INSTALL_COMPLETE_FILE),
        b"complete\n",
    )
    .into_diagnostic()
    .wrap_err("failed to write toolchain completion marker")?;
    commit_staging(&envelope_staging, &envelope)?;
    let paths = verify_locked_install(&payload, &lock, artifact, true)?;
    aros_common::outputln!("{CHECK} Installed at {}", payload.display());
    Ok(ToolchainInstallOutcome::new(
        resolved_locked(paths, &lock, artifact),
        ToolchainInstallDisposition::Published,
    ))
}

/// Resolve the verified toolchain required for a build, installing if needed.
///
/// # Errors
///
/// Returns an error when an explicitly selected local tree is invalid or the
/// checkout's locked release cannot be installed and verified. A legacy local
/// prefix is considered only when the caller supplies it explicitly; the
/// managed host-compiler directory is never inferred as a cross-toolchain.
pub async fn resolve_for_build(
    repo_root: &Path,
    preset: &str,
    local: Option<&Path>,
    offline: bool,
) -> Result<ResolvedToolchain> {
    if let Some(local) = explicit_local_override(local) {
        return resolve_local(repo_root, &local, preset);
    }
    install(repo_root, preset, offline, false, None)
        .await
        .map(ToolchainInstallOutcome::into_resolved)
}

/// Resolve an already installed target toolchain without downloading.
///
/// # Errors
///
/// Returns an error when the selected local or locked installation is absent
/// or fails verification.
pub fn path(repo_root: &Path, preset: &str, local: Option<&Path>) -> Result<ResolvedToolchain> {
    if let Some(local) = explicit_local_override(local) {
        return resolve_local(repo_root, &local, preset);
    }
    let host = host_platform_key()?;
    let lock = load_lock(repo_root)?;
    let artifact = select_locked_artifact(repo_root, &lock, host, preset)?;
    let destination = locked_store_path(&lock, artifact)?;
    let paths = verify_locked_install(&destination, &lock, artifact, true)?;
    Ok(resolved_locked(paths, &lock, artifact))
}

/// Fully verify an installed toolchain and smoke-test its executables.
///
/// # Errors
///
/// Returns an error for identity, inventory, layout, or executable failures.
pub fn verify(repo_root: &Path, preset: &str, local: Option<&Path>) -> Result<ResolvedToolchain> {
    let resolved = path(repo_root, preset, local)?;
    aros_common::outputln!(
        "{CHECK} Verified {} for {} ({})",
        resolved.paths.root.display(),
        preset,
        resolved.target_triple
    );
    Ok(resolved)
}

/// Print availability and installation state for the current host matrix.
///
/// # Errors
///
/// Returns an error when host detection or lock-file validation fails.
pub fn list(repo_root: &Path, format: ResultFormat) -> Result<()> {
    let lock = load_lock(repo_root)?;
    let current_host = host_platform_key()?;
    let artifacts = lock
        .artifacts
        .iter()
        .filter(|artifact| artifact.host == current_host)
        .map(|artifact| {
            let destination = locked_store_path(&lock, artifact)?;
            let (status, verification) = if !artifact.enabled {
                (ListArtifactStatus::Disabled, ListVerification::Unavailable)
            } else if inspect_locked_install(&destination, &lock, artifact, true, false).is_ok() {
                (ListArtifactStatus::Installed, ListVerification::Verified)
            } else {
                (
                    ListArtifactStatus::Available,
                    ListVerification::MetadataOnly,
                )
            };
            Ok(ToolchainListEntry {
                target_profile: artifact.target_profile.clone(),
                target_triple: artifact.target_triple.clone(),
                enabled: artifact.enabled,
                status,
                verification,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let result = ToolchainListResult {
        schema: LIST_SCHEMA,
        observation: "lock-and-local-installation",
        host: current_host.to_string(),
        release_id: lock.release_id,
        artifacts,
    };
    match format {
        ResultFormat::Human => print_list_human(&result),
        ResultFormat::Json => {
            let document = serde_json::to_string_pretty(&result)
                .map_err(|error| miette::miette!("could not serialize toolchain list: {error}"))?;
            aros_common::outputln!("{document}");
        }
    }
    Ok(())
}

fn print_list_human(result: &ToolchainListResult) {
    aros_common::outputln!("Release: {}", style(&result.release_id).cyan());
    for artifact in &result.artifacts {
        aros_common::outputln!(
            "  {:<16} {:<22} {}",
            artifact.target_profile,
            artifact.target_triple,
            artifact.status.as_str()
        );
    }
}

#[derive(Serialize)]
struct ToolchainListResult {
    schema: &'static str,
    observation: &'static str,
    host: String,
    release_id: String,
    artifacts: Vec<ToolchainListEntry>,
}

#[derive(Serialize)]
struct ToolchainListEntry {
    target_profile: String,
    target_triple: String,
    enabled: bool,
    status: ListArtifactStatus,
    verification: ListVerification,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
enum ListArtifactStatus {
    Disabled,
    Available,
    Installed,
}

impl ListArtifactStatus {
    const fn as_str(&self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Available => "available",
            Self::Installed => "installed",
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
enum ListVerification {
    Unavailable,
    MetadataOnly,
    Verified,
}

fn resolve_local(repo_root: &Path, root: &Path, preset: &str) -> Result<ResolvedToolchain> {
    // Reject a linked root before canonicalization. In the compiler-only case,
    // `LocalToolchainDescriptor::load` additionally validates every inherited
    // ancestor before it reads the descriptor.
    let original_metadata = fs::symlink_metadata(root)
        .into_diagnostic()
        .wrap_err_with(|| format!("local toolchain '{}' does not exist", root.display()))?;
    if !original_metadata.is_dir() || original_metadata.file_type().is_symlink() {
        bail!(
            "local toolchain root '{}' is not a real directory",
            root.display()
        );
    }
    let local_descriptor_path = root.join(LOCAL_TOOLCHAIN_DESCRIPTOR_FILE);
    let has_local_descriptor = path_entry_present(&local_descriptor_path)?;
    if has_local_descriptor {
        let descriptor_metadata = fs::symlink_metadata(&local_descriptor_path)
            .into_diagnostic()
            .wrap_err("cannot inspect local toolchain descriptor")?;
        if !descriptor_metadata.is_file() || descriptor_metadata.file_type().is_symlink() {
            bail!("local toolchain descriptor is not a regular file");
        }
        let descriptor = LocalToolchainDescriptor::load(root)
            .map_err(|error| miette::miette!("invalid local compiler-only descriptor: {error}"))?;
        let layout = descriptor.verify(root).map_err(|error| {
            miette::miette!("local compiler-only toolchain verification failed: {error}")
        })?;
        let profile = target_profile(repo_root, preset)?;
        let host = host_platform_key()?;
        let expected_profile = profile.toolchain_profile();
        let expected_triple = target_triple_for_profile(&profile);
        if descriptor.host != host
            || descriptor.target_profile != expected_profile
            || descriptor.target_triple != expected_triple
        {
            bail!(
                "local compiler descriptor is for {}/{}/{}; expected {}/{}/{}",
                descriptor.host,
                descriptor.target_profile,
                descriptor.target_triple,
                host,
                expected_profile,
                expected_triple
            );
        }
        validate_profile_compiler(&profile, &descriptor.compiler, &descriptor.target_triple)?;
        let canonical_root = root
            .canonicalize()
            .into_diagnostic()
            .wrap_err("cannot canonicalize verified local compiler-only root")?;
        let paths = ToolchainPaths {
            root: canonical_root.clone(),
            executable_roles: layout
                .resolve_tools(&canonical_root)
                .map_err(|error| miette::miette!("cannot resolve local GNU tool roles: {error}"))?,
        };
        verify_gnu_compiler_drivers(&paths, &descriptor.compiler, &descriptor.target_triple)?;
        smoke_toolchain_tools_with_timeout(
            &paths,
            collector_contract_for_profile(&descriptor.target_profile),
            TOOLCHAIN_PROBE_TIMEOUT,
        )?;
        return Ok(ResolvedToolchain {
            paths,
            target_triple: descriptor.target_triple,
            release_id: None,
            source: ToolchainSource::LocalCompilerOnly,
        });
    }

    let root = root
        .canonicalize()
        .into_diagnostic()
        .wrap_err_with(|| format!("local toolchain '{}' does not exist", root.display()))?;
    let profile = target_profile(repo_root, preset)?;
    let expected_triple = target_triple_for_profile(&profile);
    let manifest_path = root.join(AROS_TOOLCHAIN_MANIFEST_FILE);
    if path_entry_present(&manifest_path)? {
        let manifest = ArosToolchainManifest::load(&root).into_diagnostic()?;
        let host = host_platform_key()?;
        if manifest.host != host
            || manifest.target_profile != profile.toolchain_profile()
            || manifest.target_triple != expected_triple
        {
            bail!(
                "local manifest is for {}/{}/{}; expected {}/{}/{}",
                manifest.host,
                manifest.target_profile,
                manifest.target_triple,
                host,
                profile.toolchain_profile(),
                expected_triple
            );
        }
        let (actual_tree, actual_files) = toolchain_tree_inventory(&root).into_diagnostic()?;
        if actual_tree != manifest.tree_sha256 {
            bail!(
                "local toolchain tree SHA256 mismatch: expected {}, got {}",
                manifest.tree_sha256,
                actual_tree
            );
        }
        if actual_files != manifest.files {
            bail!("local toolchain file inventory does not match its manifest");
        }
        validate_profile_compiler(
            &profile,
            &manifest
                .compiler_identity()
                .map_err(|error| miette::miette!("{error}"))?,
            &manifest.target_triple,
        )?;
        let paths = verified_manifest_tools(&root, &manifest, None)?;
        return Ok(ResolvedToolchain {
            paths,
            target_triple: manifest.target_triple,
            release_id: Some(manifest.release_id),
            source: ToolchainSource::LocalManifest,
        });
    }

    if !is_legacy_aros_prefix(repo_root, &root, preset) {
        bail!(
            "local prefix '{}' has no manifest and does not look like an AROS-built {} cross-toolchain",
            root.display(),
            preset
        );
    }
    verify_tool_paths(&root, collector_contract_for_profile(preset))?;
    Ok(ResolvedToolchain {
        paths: get_toolchain_paths(&root),
        target_triple: expected_triple,
        release_id: None,
        source: ToolchainSource::LegacyLocal,
    })
}

fn path_entry_present(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(miette::miette!(
            "cannot inspect local toolchain path '{}': {error}",
            path.display()
        )),
    }
}

fn verify_locked_install(
    root: &Path,
    lock: &ArosToolchainLock,
    artifact: &ArosToolchainArtifact,
    require_complete: bool,
) -> Result<ToolchainPaths> {
    inspect_locked_install(root, lock, artifact, require_complete, true)
}

fn inspect_locked_install(
    root: &Path,
    lock: &ArosToolchainLock,
    artifact: &ArosToolchainArtifact,
    require_complete: bool,
    execute_probes: bool,
) -> Result<ToolchainPaths> {
    let root_metadata = fs::symlink_metadata(root)
        .into_diagnostic()
        .wrap_err_with(|| format!("toolchain directory '{}' is missing", root.display()))?;
    if !root_metadata.is_dir() || root_metadata.file_type().is_symlink() {
        bail!(
            "toolchain directory '{}' is not a real directory",
            root.display()
        );
    }
    if require_complete {
        let envelope = root
            .parent()
            .ok_or_else(|| miette::miette!("toolchain installation has no envelope"))?;
        let envelope_metadata = fs::symlink_metadata(envelope)
            .into_diagnostic()
            .wrap_err("toolchain installation envelope is missing")?;
        if !envelope_metadata.is_dir() || envelope_metadata.file_type().is_symlink() {
            bail!("toolchain installation envelope is not a real directory");
        }
        let marker = envelope.join(INSTALL_COMPLETE_FILE);
        let marker_metadata = fs::symlink_metadata(&marker)
            .into_diagnostic()
            .wrap_err("toolchain installation is incomplete")?;
        if !marker_metadata.is_file() || marker_metadata.file_type().is_symlink() {
            bail!("toolchain completion marker is not a regular file");
        }
        if fs::read(&marker)
            .into_diagnostic()
            .wrap_err("failed to read toolchain completion marker")?
            != b"complete\n"
        {
            bail!("toolchain completion marker has invalid contents");
        }
    }
    let manifest = ArosToolchainManifest::load(root).into_diagnostic()?;
    if manifest.schema != lock.schema
        || manifest.release_id != lock.release_id
        || manifest.host != artifact.host
        || manifest.target_profile != artifact.target_profile
        || manifest.target_triple != artifact.target_triple
        || manifest.tree_sha256 != artifact.tree_sha256
        || manifest.llvm_version != artifact.llvm_version
        || manifest.compiler != artifact.compiler
    {
        bail!("embedded toolchain manifest does not match the lock entry");
    }
    let (actual_tree, actual_files) = toolchain_tree_inventory(root).into_diagnostic()?;
    if actual_tree != artifact.tree_sha256 {
        bail!(
            "toolchain tree SHA256 mismatch: expected {}, got {}",
            artifact.tree_sha256,
            actual_tree
        );
    }
    if actual_files != manifest.files {
        bail!("toolchain file inventory does not match the embedded manifest");
    }
    for required in &artifact.required_paths {
        if fs::symlink_metadata(root.join(required)).is_err() {
            bail!("required toolchain path '{required}' is missing");
        }
    }
    if execute_probes {
        verified_manifest_tools(root, &manifest, Some(artifact))
    } else {
        manifest_tool_paths(root, &manifest, Some(artifact))
    }
}

fn manifest_tool_paths(
    root: &Path,
    manifest: &ArosToolchainManifest,
    artifact: Option<&ArosToolchainArtifact>,
) -> Result<ToolchainPaths> {
    if manifest
        .compiler_identity()
        .map_err(|error| miette::miette!("{error}"))?
        .family()
        != "gnu"
    {
        let collectors = validate_manifest_collector_contract(manifest, artifact)?;
        require_tool_paths(root, collectors)?;
        let mut paths = get_toolchain_paths(root);
        paths
            .executable_roles
            .retain(|(role, _)| *role != "collect-aros32" || collectors.collect_aros32);
        return Ok(paths);
    }
    let layout = ToolchainToolLayout::load(root).map_err(|error| miette::miette!("{error}"))?;
    if !manifest.files.iter().any(|entry| {
        entry.path == TOOLCHAIN_TOOLS_FILE
            && entry.kind == "file"
            && entry.sha256.as_deref() == Some(layout.sha256().as_str())
    }) {
        bail!("GNU executable layout bytes do not match the manifest inventory");
    }
    layout
        .validate_binding(
            &manifest
                .compiler_identity()
                .map_err(|error| miette::miette!("{error}"))?,
            &manifest.target_triple,
        )
        .map_err(|error| miette::miette!("{error}"))?;
    for required in
        std::iter::once(TOOLCHAIN_TOOLS_FILE).chain(layout.tools().entries().map(|(_, path)| path))
    {
        if !manifest.files.iter().any(|entry| entry.path == required) {
            bail!("GNU manifest omits declared tool path '{required}'");
        }
        if artifact
            .is_some_and(|artifact| !artifact.required_paths.iter().any(|path| path == required))
        {
            bail!("GNU lock omits declared tool path '{required}'");
        }
    }
    Ok(ToolchainPaths {
        root: root.to_path_buf(),
        executable_roles: layout
            .resolve_tools(root)
            .map_err(|error| miette::miette!("{error}"))?,
    })
}

fn verified_manifest_tools(
    root: &Path,
    manifest: &ArosToolchainManifest,
    artifact: Option<&ArosToolchainArtifact>,
) -> Result<ToolchainPaths> {
    let paths = manifest_tool_paths(root, manifest, artifact)?;
    let compiler = manifest
        .compiler_identity()
        .map_err(|error| miette::miette!("{error}"))?;
    verify_gnu_compiler_drivers(&paths, &compiler, &manifest.target_triple)?;
    smoke_toolchain_tools_with_timeout(
        &paths,
        CollectorContract {
            collect_aros32: true,
        },
        TOOLCHAIN_PROBE_TIMEOUT,
    )?;
    Ok(paths)
}

fn verify_gnu_compiler_drivers(
    paths: &ToolchainPaths,
    compiler: &ArosCompilerIdentity,
    target_triple: &str,
) -> Result<()> {
    let ArosCompilerIdentity::Gnu { gcc_version, .. } = compiler else {
        return Ok(());
    };
    for role in ["c", "cxx"] {
        let (_, path) = paths
            .executable_roles
            .iter()
            .find(|(name, _)| *name == role)
            .ok_or_else(|| miette::miette!("GNU layout omits required driver role '{role}'"))?;
        for (argument, expected) in [
            ("-dumpmachine", target_triple),
            ("-dumpfullversion", gcc_version.as_str()),
        ] {
            let observed = crate::observability::capture_stdout_with_timeout(
                Command::new(path).arg(argument),
                &format!("GNU {role} {argument} at '{}'", path.display()),
                TOOLCHAIN_PROBE_TIMEOUT,
            )?;
            if observed != expected {
                bail!("GNU {role} {argument} returned '{observed}', expected '{expected}'");
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CollectorContract {
    collect_aros32: bool,
}

fn collector_contract_for_profile(target_profile: &str) -> CollectorContract {
    CollectorContract {
        collect_aros32: target_profile == "pc-x86_64",
    }
}

fn validate_manifest_collector_contract(
    manifest: &ArosToolchainManifest,
    artifact: Option<&ArosToolchainArtifact>,
) -> Result<CollectorContract> {
    let contract = collector_contract_for_profile(&manifest.target_profile);
    for required in ["bin/aros-collect", "bin/collect-aros"] {
        if !manifest.files.iter().any(|entry| entry.path == required) {
            bail!(
                "toolchain manifest for '{}' omits required collector '{required}'",
                manifest.target_profile
            );
        }
        if artifact
            .is_some_and(|artifact| !artifact.required_paths.iter().any(|path| path == required))
        {
            bail!("toolchain lock entry omits required collector '{required}'");
        }
    }
    let manifest_has_32 = manifest
        .files
        .iter()
        .any(|entry| entry.path == "bin/collect-aros32");
    if manifest_has_32 != contract.collect_aros32 {
        bail!(
            "toolchain manifest collector layout does not match profile '{}': collect-aros32 must {}be present",
            manifest.target_profile,
            if contract.collect_aros32 { "" } else { "not " }
        );
    }
    if let Some(artifact) = artifact {
        let lock_has_32 = artifact
            .required_paths
            .iter()
            .any(|path| path == "bin/collect-aros32");
        if lock_has_32 != contract.collect_aros32 {
            bail!(
                "toolchain lock collector layout does not match profile '{}': collect-aros32 must {}be required",
                artifact.target_profile,
                if contract.collect_aros32 { "" } else { "not " }
            );
        }
    }
    Ok(contract)
}

fn require_tool_paths(root: &Path, collectors: CollectorContract) -> Result<()> {
    let paths = get_toolchain_paths(root);
    for (name, path) in &paths.executable_roles {
        if *name == "collect-aros32" && !collectors.collect_aros32 {
            continue;
        }
        if !command_exists(path) {
            bail!("required tool '{name}' is missing at '{}'", path.display());
        }
    }
    Ok(())
}

fn resolved_locked(
    paths: ToolchainPaths,
    lock: &ArosToolchainLock,
    artifact: &ArosToolchainArtifact,
) -> ResolvedToolchain {
    ResolvedToolchain {
        paths,
        target_triple: artifact.target_triple.clone(),
        release_id: Some(lock.release_id.clone()),
        source: ToolchainSource::LockedRelease,
    }
}

fn is_legacy_aros_prefix(repo_root: &Path, root: &Path, preset: &str) -> bool {
    let Ok(profile) = target_profile(repo_root, preset) else {
        return false;
    };
    if profile
        .transpiler
        .as_ref()
        .is_some_and(|selectors| selectors.toolchain != "llvm")
    {
        return false;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return false;
    };
    let cpu = profile.arch.to_string();
    let mut llvm_marker = false;
    let mut runtime_marker = false;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        llvm_marker |= name.starts_with(".installflag-llvm-") && name.ends_with(&cpu);
        runtime_marker |= name.starts_with(".installflag-compiler_rt-") && name.ends_with(&cpu);
    }
    llvm_marker
        && runtime_marker
        && require_tool_paths(root, collector_contract_for_profile(preset)).is_ok()
        && REQUIRED_CXX_HEADERS
            .iter()
            .all(|header| root.join("include/c++/v1").join(header).is_file())
        && root.join("lib/libc++.a").is_file()
        && root.join("lib/libc++abi.a").is_file()
        && root.join("lib/libunwind.a").is_file()
}

fn verify_tool_paths(root: &Path, collectors: CollectorContract) -> Result<()> {
    require_tool_paths(root, collectors)?;
    smoke_toolchain_tools_with_timeout(
        &get_toolchain_paths(root),
        collectors,
        TOOLCHAIN_PROBE_TIMEOUT,
    )
}

fn smoke_toolchain_tools_with_timeout(
    paths: &ToolchainPaths,
    collectors: CollectorContract,
    timeout: Duration,
) -> Result<()> {
    for (name, path) in &paths.executable_roles {
        if *name == "collect-aros32" && !collectors.collect_aros32 {
            continue;
        }
        crate::observability::capture_stdout_with_timeout(
            Command::new(path).arg("--version"),
            &format!("{name} --version at '{}'", path.display()),
            timeout,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aros_common::toolchain_manifest::{
        ArosToolchainManifestEntry, AROS_TOOLCHAIN_MANIFEST_SCHEMA,
    };
    use serde_json::json;
    use std::ffi::{OsStr, OsString};

    static ENVIRONMENT_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[cfg(unix)]
    #[test]
    fn gnu_layout_reloads_only_the_manifest_bound_raw_bytes() {
        let root = tempfile::tempdir().unwrap();
        let bytes = serde_json::to_vec(&serde_json::json!({
            "schema": "aros-toolchain-tools-v1",
            "compiler": {
                "family": "gnu", "gcc_version": "16.2.0", "binutils_version": "2.47",
                "target": {
                    "schema": "aros-riscv-target-v1", "isa": "rva22u64", "abi": "lp64d",
                    "code_model": "medany", "architecture": "rv64i2p1_m2p0_a2p1_f2p2_d2p2_c2p0",
                    "unaligned_access": false, "atomic_abi": 0, "x3_reg_usage": 0
                }
            },
            "target_triple": "riscv64-aros",
            "tools": {
                "c": "bin/driver", "cxx": "bin/driver", "assembler": "bin/driver",
                "linker": "bin/driver", "archive": "bin/driver", "ranlib": "bin/driver",
                "strip": "bin/driver", "collector": "bin/driver"
            }
        }))
        .unwrap();
        write_tool(root.path(), "driver", "#!/bin/sh\nexit 0\n");
        fs::write(root.path().join(TOOLCHAIN_TOOLS_FILE), &bytes).unwrap();
        let mut manifest = collector_manifest("fixture-target", false);
        manifest.schema = 2;
        manifest.target_triple = "riscv64-aros".into();
        manifest.llvm_version = None;
        let document: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        manifest.compiler = Some(serde_json::from_value(document["compiler"].clone()).unwrap());
        let (tree, files) = toolchain_tree_inventory(root.path()).unwrap();
        manifest.tree_sha256 = tree;
        manifest.files = files;
        assert!(manifest_tool_paths(root.path(), &manifest, None).is_ok());
        let mut changed = bytes;
        changed.push(b'\n');
        fs::write(root.path().join(TOOLCHAIN_TOOLS_FILE), changed).unwrap();
        let error = manifest_tool_paths(root.path(), &manifest, None).unwrap_err();
        assert!(error.to_string().contains("layout bytes do not match"));
    }

    struct ScopedEnvironment {
        name: &'static str,
        original: Option<OsString>,
    }

    impl ScopedEnvironment {
        fn set(name: &'static str, value: &OsStr) -> Self {
            let original = std::env::var_os(name);
            std::env::set_var(name, value);
            Self { name, original }
        }
    }

    impl Drop for ScopedEnvironment {
        fn drop(&mut self) {
            if let Some(original) = &self.original {
                std::env::set_var(self.name, original);
            } else {
                std::env::remove_var(self.name);
            }
        }
    }

    #[cfg(unix)]
    fn write_tool(root: &Path, name: &str, script: &str) {
        use std::os::unix::fs::PermissionsExt as _;

        let path = root.join("bin").join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, script).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    fn working_tools(root: &Path) -> ToolchainPaths {
        for tool in [
            "clang",
            "clang++",
            "ld.lld",
            "llvm-ar",
            "aros-collect",
            "collect-aros",
            "collect-aros32",
        ] {
            write_tool(root, tool, "#!/bin/sh\nprintf '%s\\n' 'fixture 1.0'\n");
        }
        get_toolchain_paths(root)
    }

    #[cfg(unix)]
    fn write_local_compiler_prefix(root: &Path, target_profile: &str) {
        use std::os::unix::fs::PermissionsExt as _;

        let triple = "riscv-aros";
        let compiler: ArosCompilerIdentity = serde_json::from_value(json!({
            "family": "gnu", "gcc_version": "16.2.0", "binutils_version": "2.47",
            "target": {
                "schema": "aros-riscv-target-v1",
                "isa": "rv32imafc_zicsr_zifencei_zaamo_zalrsc",
                "abi": "ilp32f", "code_model": "medany",
                "architecture": "rv32i2p1_m2p0_a2p1_f2p2_c2p0_zicsr2p0_zifencei2p0_zaamo1p0_zalrsc1p0",
                "unaligned_access": false, "atomic_abi": 0, "x3_reg_usage": 0
            }
        }))
        .unwrap();
        let tools = json!({
            "c": "bin/gcc", "cxx": "bin/g++", "assembler": "bin/as",
            "linker": "bin/ld", "archive": "bin/ar", "ranlib": "bin/ranlib",
            "strip": "bin/strip", "collector": "bin/collect-aros", "nm": "bin/nm",
            "objcopy": "bin/objcopy", "objdump": "bin/objdump"
        });
        let layout = json!({
            "schema": "aros-toolchain-tools-v3", "compiler": compiler,
            "target_triple": triple, "tools": tools
        });
        fs::create_dir_all(root.join("bin")).unwrap();
        let script = format!(
            "#!/bin/sh\ncase \"$1\" in\n--version) printf '%s\\n' fixture;;\n-dumpmachine) printf '%s\\n' '{triple}';;\n-dumpfullversion) printf '%s\\n' '16.2.0';;\n*) exit 0;;\nesac\n"
        );
        for path in [
            "bin/gcc",
            "bin/g++",
            "bin/as",
            "bin/ld",
            "bin/ar",
            "bin/ranlib",
            "bin/strip",
            "bin/collect-aros",
            "bin/nm",
            "bin/objcopy",
            "bin/objdump",
        ] {
            fs::write(root.join(path), &script).unwrap();
            fs::set_permissions(root.join(path), fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::write(
            root.join(TOOLCHAIN_TOOLS_FILE),
            serde_json::to_vec(&layout).unwrap(),
        )
        .unwrap();
        let descriptor = LocalToolchainDescriptor::capture(
            root,
            host_platform_key().unwrap(),
            target_profile,
            triple,
            serde_json::from_value(layout["compiler"].clone()).unwrap(),
        )
        .unwrap();
        fs::write(
            root.join(LOCAL_TOOLCHAIN_DESCRIPTOR_FILE),
            serde_json::to_vec(&descriptor).unwrap(),
        )
        .unwrap();
    }

    #[cfg(unix)]
    fn local_gnu_checkout() -> tempfile::TempDir {
        let checkout = tempfile::tempdir().unwrap();
        fs::write(
            checkout.path().join("aros-targets.toml"),
            "[[targets]]\nname='fixture-target'\narch='riscv32'\nplatform='fixture'\nbsp='fixture'\nfloat_abi='ilp32f'\n[targets.transpiler]\nfamily=''\nvariant=''\ntoolchain='gnu'\ncpu32=''\nuse_mmu=false\n",
        )
        .unwrap();
        checkout
    }

    #[test]
    fn store_path_is_content_addressed() {
        let lock = ArosToolchainLock {
            schema: 1,
            release_id: "release-v1".into(),
            base_url: Some("https://example.invalid".into()),
            artifacts: Vec::new(),
        };
        let artifact = ArosToolchainArtifact {
            host: "linux-x86_64".into(),
            target_profile: "pc-x86_64".into(),
            target_triple: "x86_64-unknown-aros".into(),
            asset: "asset.tar.xz".into(),
            sha256: "a".repeat(64),
            tree_sha256: "b".repeat(64),
            llvm_version: Some("11.0.0".into()),
            compiler: None,
            size: None,
            enabled: true,
            disabled_reason: None,
            strip_components: 1,
            required_paths: Vec::new(),
        };
        let path = locked_store_path(&lock, &artifact).unwrap();
        assert!(path.ends_with(format!(
            "release-v1/linux-x86_64/pc-x86_64/{}/toolchain",
            "a".repeat(64)
        )));
    }

    #[test]
    fn selection_requires_checkout_lock_and_explicit_local_argument() {
        let checkout = Path::new("/reviewed/checkout");
        assert_eq!(
            lock_file_path(checkout),
            checkout.join("aros-toolchains.lock.toml")
        );
        assert_eq!(explicit_local_override(None), None);
        assert_eq!(
            explicit_local_override(Some(Path::new("/opt/aros-local"))),
            Some(PathBuf::from("/opt/aros-local"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn explicit_local_compiler_only_resolution_smokes_tools_without_release_identity() {
        let checkout = local_gnu_checkout();
        let prefix = tempfile::tempdir().unwrap();
        write_local_compiler_prefix(prefix.path(), "fixture-target");

        let resolved = resolve_local(checkout.path(), prefix.path(), "fixture-target").unwrap();
        assert_eq!(resolved.source, ToolchainSource::LocalCompilerOnly);
        assert_eq!(resolved.target_triple, "riscv-aros");
        assert_eq!(resolved.release_id, None);
        assert!(resolved
            .paths
            .executable_roles
            .iter()
            .any(|(role, _)| *role == "objdump"));
    }

    #[cfg(unix)]
    #[test]
    fn local_compiler_only_resolution_rejects_profile_and_payload_digest_mismatches() {
        let checkout = local_gnu_checkout();
        let mismatched_profile = tempfile::tempdir().unwrap();
        write_local_compiler_prefix(mismatched_profile.path(), "other-profile");
        assert!(
            resolve_local(checkout.path(), mismatched_profile.path(), "fixture-target").is_err()
        );

        let changed_payload = tempfile::tempdir().unwrap();
        write_local_compiler_prefix(changed_payload.path(), "fixture-target");
        fs::write(
            changed_payload.path().join("bin/gcc"),
            b"changed compiler\n",
        )
        .unwrap();
        let error =
            resolve_local(checkout.path(), changed_payload.path(), "fixture-target").unwrap_err();
        assert!(error.to_string().contains("payload differs"));
    }

    #[cfg(unix)]
    #[test]
    fn local_compiler_only_resolution_rejects_release_coexistence_and_links() {
        use std::os::unix::fs::symlink;

        let checkout = local_gnu_checkout();
        let coexisting = tempfile::tempdir().unwrap();
        write_local_compiler_prefix(coexisting.path(), "fixture-target");
        fs::write(
            coexisting.path().join(AROS_TOOLCHAIN_MANIFEST_FILE),
            b"invalid release manifest",
        )
        .unwrap();
        assert!(resolve_local(checkout.path(), coexisting.path(), "fixture-target").is_err());

        let linked_descriptor = tempfile::tempdir().unwrap();
        write_local_compiler_prefix(linked_descriptor.path(), "fixture-target");
        let descriptor_path = linked_descriptor
            .path()
            .join(LOCAL_TOOLCHAIN_DESCRIPTOR_FILE);
        let outside = tempfile::NamedTempFile::new().unwrap();
        fs::write(outside.path(), fs::read(&descriptor_path).unwrap()).unwrap();
        fs::remove_file(&descriptor_path).unwrap();
        symlink(outside.path(), &descriptor_path).unwrap();
        assert!(
            resolve_local(checkout.path(), linked_descriptor.path(), "fixture-target").is_err()
        );

        let linked_root = linked_descriptor.path().with_extension("root-link");
        symlink(linked_descriptor.path(), &linked_root).unwrap();
        assert!(resolve_local(checkout.path(), &linked_root, "fixture-target").is_err());
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn build_resolution_never_infers_a_legacy_host_compiler_default() {
        let _environment = ENVIRONMENT_LOCK.lock().await;
        let checkout = tempfile::tempdir().unwrap();
        fs::write(
            checkout.path().join("aros-targets.toml"),
            "[[targets]]\nname='pc-x86_64'\narch='x86_64'\nplatform='pc'\nbsp='pc'\n",
        )
        .unwrap();
        let legacy = tempfile::tempdir().unwrap();
        working_tools(legacy.path());
        for marker in [
            ".installflag-llvm-x86_64",
            ".installflag-compiler_rt-x86_64",
        ] {
            fs::write(legacy.path().join(marker), b"complete\n").unwrap();
        }
        let cxx = legacy.path().join("include/c++/v1");
        fs::create_dir_all(&cxx).unwrap();
        for header in REQUIRED_CXX_HEADERS {
            fs::write(cxx.join(header), b"fixture\n").unwrap();
        }
        fs::create_dir_all(legacy.path().join("lib")).unwrap();
        for library in ["libc++.a", "libc++abi.a", "libunwind.a"] {
            fs::write(legacy.path().join("lib").join(library), b"fixture\n").unwrap();
        }

        let _default = ScopedEnvironment::set("AROS_HOST_COMPILER_DIR", legacy.path().as_os_str());
        let explicit = resolve_for_build(checkout.path(), "pc-x86_64", Some(legacy.path()), true)
            .await
            .unwrap();
        assert_eq!(explicit.source, ToolchainSource::LegacyLocal);

        let error = resolve_for_build(checkout.path(), "pc-x86_64", None, true)
            .await
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("failed to load AROS toolchain lock"));
    }

    #[test]
    fn target_triples_are_profile_exact() {
        for (name, arch, triple) in [
            (
                "pc-x86_64",
                aros_common::Architecture::X86_64,
                "x86_64-unknown-aros",
            ),
            (
                "arm-raspi",
                aros_common::Architecture::Arm,
                "arm-unknown-aros",
            ),
            (
                "rpi-aarch64",
                aros_common::Architecture::AArch64,
                "aarch64-unknown-aros",
            ),
            (
                "opensbi-riscv64",
                aros_common::Architecture::Riscv64,
                "riscv64-unknown-aros",
            ),
        ] {
            let profile = TargetProfile {
                name: name.into(),
                arch,
                platform: String::new(),
                bsp: String::new(),
                features: Vec::new(),
                float_abi: None,
                transpiler: None,
                bootloader: None,
                bootstrap_abi: None,
                native_build_contract: None,
                toolchain_profile: None,
            };
            assert_eq!(target_triple_for_profile(&profile), triple);
        }
    }

    fn manifest_entry(path: &str) -> ArosToolchainManifestEntry {
        ArosToolchainManifestEntry {
            path: path.into(),
            mode: "0755".into(),
            kind: "file".into(),
            sha256: Some("a".repeat(64)),
            size: Some(1),
            target: None,
        }
    }

    fn collector_manifest(profile: &str, include_32: bool) -> ArosToolchainManifest {
        let mut files = vec![
            manifest_entry("bin/aros-collect"),
            manifest_entry("bin/collect-aros"),
        ];
        if include_32 {
            files.push(manifest_entry("bin/collect-aros32"));
        }
        ArosToolchainManifest {
            schema: AROS_TOOLCHAIN_MANIFEST_SCHEMA,
            release_id: "fixture".into(),
            host: "linux-x86_64".into(),
            target_profile: profile.into(),
            target_triple: format!("{profile}-unknown-aros"),
            tree_sha256: "b".repeat(64),
            llvm_version: Some("11.0.0".into()),
            compiler: None,
            recipe_sha256: "c".repeat(64),
            source_lock_sha256: "d".repeat(64),
            profiles_sha256: "e".repeat(64),
            source_commit: "1".repeat(40),
            producer_commit: "2".repeat(40),
            tools_commit: "3".repeat(40),
            source_date_epoch: 1,
            capabilities: vec!["collector".into()],
            build_environment: serde_json::Map::new(),
            files,
        }
    }

    #[test]
    fn manifest_collector_contract_is_profile_exact() {
        assert!(
            validate_manifest_collector_contract(&collector_manifest("pc-x86_64", true), None)
                .unwrap()
                .collect_aros32
        );
        assert!(validate_manifest_collector_contract(
            &collector_manifest("pc-x86_64", false),
            None
        )
        .is_err());
        assert!(
            validate_manifest_collector_contract(&collector_manifest("arm-raspi", true), None)
                .is_err()
        );
        assert!(
            !validate_manifest_collector_contract(&collector_manifest("arm-raspi", false), None)
                .unwrap()
                .collect_aros32
        );
    }

    #[cfg(unix)]
    #[test]
    fn smoke_verifies_every_required_build_tool() {
        let root = tempfile::tempdir().unwrap();
        let paths = working_tools(root.path());
        let collectors = collector_contract_for_profile("pc-x86_64");
        smoke_toolchain_tools_with_timeout(&paths, collectors, Duration::from_secs(5)).unwrap();

        write_tool(root.path(), "clang++", "#!/bin/sh\nexit 23\n");
        let error = smoke_toolchain_tools_with_timeout(&paths, collectors, Duration::from_secs(5))
            .unwrap_err();
        assert!(error.to_string().contains("clang++ --version"));
    }

    #[cfg(unix)]
    #[test]
    fn smoke_probe_has_a_hard_deadline() {
        let root = tempfile::tempdir().unwrap();
        let paths = working_tools(root.path());
        write_tool(root.path(), "ld.lld", "#!/bin/sh\nsleep 30\n");
        let error = smoke_toolchain_tools_with_timeout(
            &paths,
            collector_contract_for_profile("pc-x86_64"),
            Duration::from_millis(100),
        )
        .unwrap_err();
        assert!(error.to_string().contains("timed out"));
    }

    #[cfg(unix)]
    #[test]
    fn required_tool_layout_rejects_one_missing_member() {
        let root = tempfile::tempdir().unwrap();
        let paths = working_tools(root.path());
        fs::remove_file(paths.root.join("bin/ld.lld")).unwrap();
        assert!(
            require_tool_paths(root.path(), collector_contract_for_profile("pc-x86_64"))
                .unwrap_err()
                .to_string()
                .contains("ld.lld")
        );
    }

    #[cfg(unix)]
    #[test]
    fn required_collectors_are_never_optional() {
        let root = tempfile::tempdir().unwrap();
        let paths = working_tools(root.path());
        fs::remove_file(paths.root.join("bin/aros-collect")).unwrap();
        let error = verify_tool_paths(root.path(), collector_contract_for_profile("arm-raspi"))
            .unwrap_err();
        assert!(error.to_string().contains("aros-collect"));

        write_tool(
            root.path(),
            "aros-collect",
            "#!/bin/sh\nprintf '%s\\n' 'fixture 1.0'\n",
        );
        fs::remove_file(paths.root.join("bin/collect-aros")).unwrap();
        let error = verify_tool_paths(root.path(), collector_contract_for_profile("arm-raspi"))
            .unwrap_err();
        assert!(error.to_string().contains("collect-aros"));
    }

    #[cfg(unix)]
    #[test]
    fn collect_aros32_is_required_only_by_its_exact_profile() {
        let root = tempfile::tempdir().unwrap();
        let paths = working_tools(root.path());
        fs::remove_file(paths.root.join("bin/collect-aros32")).unwrap();

        verify_tool_paths(root.path(), collector_contract_for_profile("arm-raspi")).unwrap();
        let error = verify_tool_paths(root.path(), collector_contract_for_profile("pc-x86_64"))
            .unwrap_err();
        assert!(error.to_string().contains("collect-aros32"));
    }

    #[test]
    fn completion_marker_lives_outside_immutable_payload() {
        let store = tempfile::tempdir().unwrap();
        let envelope = store.path().join("digest");
        let payload = envelope.join("toolchain");
        fs::create_dir_all(payload.join("bin")).unwrap();
        for tool in [
            "clang",
            "clang++",
            "ld.lld",
            "llvm-ar",
            "aros-collect",
            "collect-aros",
            "collect-aros32",
        ] {
            let path = payload.join("bin").join(tool);
            #[cfg(unix)]
            fs::write(&path, b"#!/bin/sh\nprintf '%s\\n' 'fixture 11.0.0'\n").unwrap();
            #[cfg(not(unix))]
            fs::write(&path, b"tool").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;

                fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        let (tree_sha256, files) = toolchain_tree_inventory(&payload).unwrap();
        let manifest = ArosToolchainManifest {
            schema: AROS_TOOLCHAIN_MANIFEST_SCHEMA,
            release_id: "release-v1".into(),
            host: "linux-x86_64".into(),
            target_profile: "pc-x86_64".into(),
            target_triple: "x86_64-unknown-aros".into(),
            tree_sha256: tree_sha256.clone(),
            llvm_version: Some("11.0.0".into()),
            compiler: None,
            recipe_sha256: "b".repeat(64),
            source_lock_sha256: "c".repeat(64),
            profiles_sha256: "d".repeat(64),
            source_commit: "1".repeat(40),
            producer_commit: "2".repeat(40),
            tools_commit: "3".repeat(40),
            source_date_epoch: 1,
            capabilities: vec!["collector".into()],
            build_environment: serde_json::Map::new(),
            files,
        };
        fs::write(
            payload.join(AROS_TOOLCHAIN_MANIFEST_FILE),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        let artifact = ArosToolchainArtifact {
            host: manifest.host.clone(),
            target_profile: manifest.target_profile.clone(),
            target_triple: manifest.target_triple.clone(),
            asset: "asset.tar.xz".into(),
            sha256: "a".repeat(64),
            tree_sha256,
            llvm_version: manifest.llvm_version.clone(),
            compiler: manifest.compiler.clone(),
            size: None,
            enabled: true,
            disabled_reason: None,
            strip_components: 1,
            required_paths: vec![
                "bin/aros-collect".into(),
                "bin/collect-aros".into(),
                "bin/collect-aros32".into(),
            ],
        };
        let lock = ArosToolchainLock {
            schema: 1,
            release_id: manifest.release_id,
            base_url: Some("https://example.invalid".into()),
            artifacts: vec![artifact.clone()],
        };

        assert!(verify_locked_install(&payload, &lock, &artifact, true).is_err());
        fs::write(envelope.join(INSTALL_COMPLETE_FILE), b"complete\n").unwrap();
        verify_locked_install(&payload, &lock, &artifact, true).unwrap();
        assert!(!payload.join(INSTALL_COMPLETE_FILE).exists());
        assert_schema2_locked_compiler_identity(&payload, &lock, &artifact);

        fs::write(envelope.join(INSTALL_COMPLETE_FILE), b"incomplete\n").unwrap();
        assert!(verify_locked_install(&payload, &lock, &artifact, true).is_err());

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let marker = envelope.join(INSTALL_COMPLETE_FILE);
            let external = store.path().join("external-marker");
            fs::write(&external, b"complete\n").unwrap();
            fs::remove_file(&marker).unwrap();
            symlink(&external, &marker).unwrap();
            assert!(verify_locked_install(&payload, &lock, &artifact, true).is_err());
        }
    }

    fn assert_schema2_locked_compiler_identity(
        payload: &Path,
        legacy_lock: &ArosToolchainLock,
        legacy_artifact: &ArosToolchainArtifact,
    ) {
        let path = payload.join(AROS_TOOLCHAIN_MANIFEST_FILE);
        let original = fs::read(&path).unwrap();
        let mut manifest = ArosToolchainManifest::load(payload).unwrap();
        manifest.schema = aros_common::AROS_TOOLCHAIN_MANIFEST_SCHEMA_V2;
        manifest.llvm_version = None;
        manifest.compiler = Some(aros_common::ArosCompilerIdentity::Llvm {
            version: "11.0.0".into(),
        });
        manifest.validate().unwrap();
        let candidate_bytes = serde_json::to_vec(&manifest).unwrap();
        fs::write(&path, &candidate_bytes).unwrap();
        let mut artifact = legacy_artifact.clone();
        artifact.llvm_version = None;
        artifact.compiler = manifest.compiler;
        let mut lock = legacy_lock.clone();
        lock.schema = aros_common::AROS_TOOLCHAIN_LOCK_SCHEMA_V2;
        lock.artifacts = vec![artifact.clone()];
        lock.validate().unwrap();
        verify_locked_install(payload, &lock, &artifact, true).unwrap();

        // The payload-tree digest excludes the manifest. Compiler metadata
        // therefore needs its own exact lock binding, not just a tree check.
        artifact.compiler = Some(aros_common::ArosCompilerIdentity::Llvm {
            version: "12.0.0".into(),
        });
        lock.artifacts = vec![artifact.clone()];
        lock.validate().unwrap();
        let error = verify_locked_install(payload, &lock, &artifact, true).unwrap_err();
        assert!(error.to_string().contains("does not match the lock entry"));
        assert_eq!(fs::read(&path).unwrap(), candidate_bytes);
        let error = verify_locked_install(payload, legacy_lock, legacy_artifact, true).unwrap_err();
        assert!(error.to_string().contains("does not match the lock entry"));
        assert_eq!(fs::read(&path).unwrap(), candidate_bytes);
        fs::write(path, original).unwrap();
    }
}
