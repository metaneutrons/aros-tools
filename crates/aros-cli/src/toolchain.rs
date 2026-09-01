//! Locked AROS cross-toolchain installation, resolution, and verification.

use crate::artifact::{
    aros_home, command_exists, commit_staging, extract_to_staging, obtain_archive,
    require_absolute_state_path, tree_inventory, INSTALL_COMPLETE_FILE,
};
use crate::host_compiler::host_platform_key;
use aros_common::target::TargetProfile;
use aros_common::toolchain_manifest::{
    ArosToolchainArtifact, ArosToolchainLock, ArosToolchainManifest, AROS_TOOLCHAIN_MANIFEST_FILE,
};
use console::{style, Emoji};
use miette::{bail, IntoDiagnostic, Result, WrapErr};
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

/// Required executable layout of an installed AROS cross-toolchain.
#[derive(Debug, Clone)]
pub struct ToolchainPaths {
    /// Installation payload root.
    pub root: PathBuf,
    /// Clang C frontend.
    pub clang: PathBuf,
    /// Clang C++ frontend.
    pub clangxx: PathBuf,
    /// LLVM linker.
    pub lld: PathBuf,
    /// LLVM archive tool.
    pub llvm_ar: PathBuf,
    /// Native collector frontend.
    pub aros_collect: PathBuf,
    /// Upstream-compatible collector driver.
    pub collect_aros: PathBuf,
    /// Upstream-compatible 32-bit collector driver.
    pub collect_aros32: PathBuf,
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

/// Derive every required tool path from one payload root.
pub fn get_toolchain_paths(root: &Path) -> ToolchainPaths {
    let llvm = crate::host_compiler::host_compiler_paths(root);
    ToolchainPaths {
        root: root.into(),
        clang: llvm.clang,
        clangxx: llvm.clangxx,
        lld: llvm.lld,
        llvm_ar: llvm.llvm_ar,
        aros_collect: root.join("bin/aros-collect"),
        collect_aros: root.join("bin/collect-aros"),
        collect_aros32: root.join("bin/collect-aros32"),
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
    format!("{}-unknown-aros", profile.arch)
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
) -> Result<ResolvedToolchain> {
    if let Some(local) = explicit_local_override(local) {
        let resolved = resolve_local(repo_root, &local, preset)?;
        aros_common::outputln!(
            "{CHECK} Using local AROS toolchain without copying it: {}",
            local.display()
        );
        return Ok(resolved);
    }

    let host = host_platform_key()?;
    let profile = target_profile(repo_root, preset)?;
    let expected_triple = target_triple_for_profile(&profile);
    let lock = load_lock(repo_root)?;
    let artifact = lock.resolve(host, preset).ok_or_else(|| {
        miette::miette!("no locked AROS toolchain for host '{host}' and preset '{preset}'")
    })?;
    if artifact.target_triple != expected_triple {
        bail!(
            "locked target triple '{}' does not match preset '{}' ({})",
            artifact.target_triple,
            preset,
            expected_triple
        );
    }
    if !artifact.enabled {
        bail!(
            "AROS toolchain {host}/{preset} is locked but disabled: {}",
            artifact
                .disabled_reason
                .as_deref()
                .unwrap_or("no release asset is available")
        );
    }

    let envelope = locked_store_envelope(&lock, artifact)?;
    let payload = envelope.join("toolchain");
    match fs::symlink_metadata(&envelope) {
        Ok(_) => {
            verify_locked_install(&payload, &lock, artifact, true).wrap_err_with(|| {
                format!(
                    "content-addressed destination '{}' already exists but is invalid; it was not overwritten",
                    envelope.display()
                )
            })?;
            if !force {
                return Ok(resolved_locked(&payload, &lock, artifact));
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
            return Ok(resolved_locked(&payload, &lock, artifact));
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
            verify_locked_install(&payload, &lock, artifact, true).wrap_err_with(|| {
                format!(
                    "content-addressed destination '{}' appeared during installation but is invalid; it was not overwritten",
                    envelope.display()
                )
            })?;
            return Ok(resolved_locked(&payload, &lock, artifact));
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
    let payload_staging = extract_to_staging(&archive, parent, artifact.strip_components)?;
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
    verify_locked_install(&payload, &lock, artifact, true)?;
    aros_common::outputln!("{CHECK} Installed at {}", payload.display());
    Ok(resolved_locked(&payload, &lock, artifact))
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
    install(repo_root, preset, offline, false, None).await
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
    let artifact = lock.resolve(host, preset).ok_or_else(|| {
        miette::miette!("no locked AROS toolchain for host '{host}' and preset '{preset}'")
    })?;
    let destination = locked_store_path(&lock, artifact)?;
    verify_locked_install(&destination, &lock, artifact, true)?;
    Ok(resolved_locked(&destination, &lock, artifact))
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
pub fn list(repo_root: &Path) -> Result<()> {
    let lock = load_lock(repo_root)?;
    let current_host = host_platform_key()?;
    aros_common::outputln!("Release: {}", style(&lock.release_id).cyan());
    for artifact in lock
        .artifacts
        .iter()
        .filter(|artifact| artifact.host == current_host)
    {
        let destination = locked_store_path(&lock, artifact)?;
        let status = if !artifact.enabled {
            "disabled"
        } else if verify_locked_install(&destination, &lock, artifact, true).is_ok() {
            "installed"
        } else {
            "available"
        };
        aros_common::outputln!(
            "  {:<16} {:<22} {}",
            artifact.target_profile,
            artifact.target_triple,
            status
        );
    }
    Ok(())
}

fn resolve_local(repo_root: &Path, root: &Path, preset: &str) -> Result<ResolvedToolchain> {
    let root = root
        .canonicalize()
        .into_diagnostic()
        .wrap_err_with(|| format!("local toolchain '{}' does not exist", root.display()))?;
    let profile = target_profile(repo_root, preset)?;
    let expected_triple = target_triple_for_profile(&profile);
    let manifest_path = root.join(AROS_TOOLCHAIN_MANIFEST_FILE);
    if manifest_path.exists() {
        let manifest = ArosToolchainManifest::load(&root).into_diagnostic()?;
        let host = host_platform_key()?;
        if manifest.host != host
            || manifest.target_profile != preset
            || manifest.target_triple != expected_triple
        {
            bail!(
                "local manifest is for {}/{}/{}; expected {}/{}/{}",
                manifest.host,
                manifest.target_profile,
                manifest.target_triple,
                host,
                preset,
                expected_triple
            );
        }
        let (actual_tree, actual_files) = tree_inventory(&root)?;
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
        let collectors = validate_manifest_collector_contract(&manifest, None)?;
        verify_tool_paths(&root, collectors)?;
        return Ok(ResolvedToolchain {
            paths: get_toolchain_paths(&root),
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

fn verify_locked_install(
    root: &Path,
    lock: &ArosToolchainLock,
    artifact: &ArosToolchainArtifact,
    require_complete: bool,
) -> Result<()> {
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
    if manifest.release_id != lock.release_id
        || manifest.host != artifact.host
        || manifest.target_profile != artifact.target_profile
        || manifest.target_triple != artifact.target_triple
        || manifest.tree_sha256 != artifact.tree_sha256
        || manifest.llvm_version != artifact.llvm_version
    {
        bail!("embedded toolchain manifest does not match the lock entry");
    }
    let (actual_tree, actual_files) = tree_inventory(root)?;
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
    let collectors = validate_manifest_collector_contract(&manifest, Some(artifact))?;
    verify_tool_paths(root, collectors)
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
    let mut required = vec![
        ("clang", &paths.clang),
        ("clang++", &paths.clangxx),
        ("ld.lld", &paths.lld),
        ("llvm-ar", &paths.llvm_ar),
        ("aros-collect", &paths.aros_collect),
        ("collect-aros", &paths.collect_aros),
    ];
    if collectors.collect_aros32 {
        required.push(("collect-aros32", &paths.collect_aros32));
    }
    for (name, path) in required {
        if !command_exists(path) {
            bail!("required tool '{name}' is missing at '{}'", path.display());
        }
    }
    Ok(())
}

fn resolved_locked(
    root: &Path,
    lock: &ArosToolchainLock,
    artifact: &ArosToolchainArtifact,
) -> ResolvedToolchain {
    ResolvedToolchain {
        paths: get_toolchain_paths(root),
        target_triple: artifact.target_triple.clone(),
        release_id: Some(lock.release_id.clone()),
        source: ToolchainSource::LockedRelease,
    }
}

fn is_legacy_aros_prefix(repo_root: &Path, root: &Path, preset: &str) -> bool {
    let Ok(profile) = target_profile(repo_root, preset) else {
        return false;
    };
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
    let mut tools = vec![
        ("clang", &paths.clang),
        ("clang++", &paths.clangxx),
        ("ld.lld", &paths.lld),
        ("llvm-ar", &paths.llvm_ar),
        ("aros-collect", &paths.aros_collect),
        ("collect-aros", &paths.collect_aros),
    ];
    if collectors.collect_aros32 {
        tools.push(("collect-aros32", &paths.collect_aros32));
    }
    for (name, path) in tools {
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
    use std::ffi::{OsStr, OsString};

    static ENVIRONMENT_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
        fs::remove_file(paths.lld).unwrap();
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
        fs::remove_file(&paths.aros_collect).unwrap();
        let error = verify_tool_paths(root.path(), collector_contract_for_profile("arm-raspi"))
            .unwrap_err();
        assert!(error.to_string().contains("aros-collect"));

        write_tool(
            root.path(),
            "aros-collect",
            "#!/bin/sh\nprintf '%s\\n' 'fixture 1.0'\n",
        );
        fs::remove_file(&paths.collect_aros).unwrap();
        let error = verify_tool_paths(root.path(), collector_contract_for_profile("arm-raspi"))
            .unwrap_err();
        assert!(error.to_string().contains("collect-aros"));
    }

    #[cfg(unix)]
    #[test]
    fn collect_aros32_is_required_only_by_its_exact_profile() {
        let root = tempfile::tempdir().unwrap();
        let paths = working_tools(root.path());
        fs::remove_file(&paths.collect_aros32).unwrap();

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
        let (tree_sha256, files) = tree_inventory(&payload).unwrap();
        let manifest = ArosToolchainManifest {
            schema: AROS_TOOLCHAIN_MANIFEST_SCHEMA,
            release_id: "release-v1".into(),
            host: "linux-x86_64".into(),
            target_profile: "pc-x86_64".into(),
            target_triple: "x86_64-unknown-aros".into(),
            tree_sha256: tree_sha256.clone(),
            llvm_version: Some("11.0.0".into()),
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
}
