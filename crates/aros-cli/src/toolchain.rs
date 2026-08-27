use crate::artifact::{
    aros_home, command_exists, commit_staging, extract_to_staging, obtain_archive, tree_inventory,
    INSTALL_COMPLETE_FILE,
};
use crate::host_tools::host_platform_key;
use anyhow::{bail, Context, Result};
use aros_common::target::TargetProfile;
use aros_common::toolchain_manifest::{
    ArosToolchainArtifact, ArosToolchainLock, ArosToolchainManifest, AROS_TOOLCHAIN_MANIFEST_FILE,
};
use console::{style, Emoji};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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

#[derive(Debug, Clone)]
pub struct ToolchainPaths {
    pub root: PathBuf,
    pub clang: PathBuf,
    pub clangxx: PathBuf,
    pub lld: PathBuf,
    pub llvm_ar: PathBuf,
    pub aros_collect: PathBuf,
    pub collect_aros: PathBuf,
    pub collect_aros32: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolchainSource {
    LockedRelease,
    LocalManifest,
    LegacyLocal,
}

#[derive(Debug, Clone)]
pub struct ResolvedToolchain {
    pub paths: ToolchainPaths,
    pub target_triple: String,
    pub release_id: Option<String>,
    pub source: ToolchainSource,
}

pub fn lock_file_path() -> PathBuf {
    std::env::var_os("AROS_TOOLCHAIN_LOCK")
        .map_or_else(|| PathBuf::from("aros-toolchains.lock.toml"), PathBuf::from)
}

pub fn load_lock() -> Result<ArosToolchainLock> {
    let path = lock_file_path();
    ArosToolchainLock::load(&path)
        .with_context(|| format!("failed to load AROS toolchain lock '{}'", path.display()))
}

pub fn default_store_root() -> PathBuf {
    std::env::var_os("AROS_CROSS_TOOLCHAINS_DIR")
        .map_or_else(|| aros_home().join("cross-toolchains"), PathBuf::from)
}

pub fn explicit_local_override(argument: Option<&Path>) -> Option<PathBuf> {
    argument
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("AROS_CROSS_TOOLCHAIN_DIR").map(PathBuf::from))
}

pub fn get_toolchain_paths(root: &Path) -> ToolchainPaths {
    ToolchainPaths {
        root: root.into(),
        clang: root.join("bin/clang"),
        clangxx: root.join("bin/clang++"),
        lld: root.join("bin/ld.lld"),
        llvm_ar: root.join("bin/llvm-ar"),
        aros_collect: root.join("bin/aros-collect"),
        collect_aros: root.join("bin/collect-aros"),
        collect_aros32: root.join("bin/collect-aros32"),
    }
}

pub fn target_profile(name: &str) -> Result<TargetProfile> {
    TargetProfile::load_from_file(Path::new("aros-targets.toml"))?
        .into_iter()
        .find(|profile| profile.name == name)
        .ok_or_else(|| anyhow::anyhow!("unknown target preset '{name}' in aros-targets.toml"))
}

pub fn target_triple_for_profile(profile: &TargetProfile) -> String {
    format!("{}-unknown-aros", profile.arch)
}

pub fn locked_store_path(lock: &ArosToolchainLock, artifact: &ArosToolchainArtifact) -> PathBuf {
    locked_store_envelope(lock, artifact).join("toolchain")
}

fn locked_store_envelope(lock: &ArosToolchainLock, artifact: &ArosToolchainArtifact) -> PathBuf {
    default_store_root()
        .join(&lock.release_id)
        .join(&artifact.host)
        .join(&artifact.target_profile)
        .join(artifact.sha256.to_ascii_lowercase())
}

pub async fn install(
    preset: &str,
    offline: bool,
    force: bool,
    local: Option<&Path>,
) -> Result<ResolvedToolchain> {
    if let Some(local) = explicit_local_override(local) {
        let resolved = resolve_local(&local, preset)?;
        println!(
            "{CHECK} Using local AROS toolchain without copying it: {}",
            local.display()
        );
        return Ok(resolved);
    }

    let host = host_platform_key()?;
    let profile = target_profile(preset)?;
    let expected_triple = target_triple_for_profile(&profile);
    let lock = load_lock()?;
    let artifact = lock.resolve(host, preset).ok_or_else(|| {
        anyhow::anyhow!("no locked AROS toolchain for host '{host}' and preset '{preset}'")
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

    let envelope = locked_store_envelope(&lock, artifact);
    let payload = envelope.join("toolchain");
    if verify_locked_install(&payload, &lock, artifact, true).is_ok() && !force {
        return Ok(resolved_locked(&payload, &lock, artifact));
    }

    println!(
        "{DOWNLOAD} AROS toolchain {} for {} / {}",
        style(&lock.release_id).cyan(),
        style(host).yellow(),
        style(preset).yellow()
    );
    let archive = obtain_archive(
        &lock.asset_url(artifact).map_err(anyhow::Error::msg)?,
        &artifact.sha256,
        artifact.size,
        offline,
        force,
    )
    .await?;

    if envelope.exists() {
        verify_locked_install(&payload, &lock, artifact, true).with_context(|| {
            format!(
                "content-addressed destination '{}' already exists but is invalid; it was not overwritten",
                envelope.display()
            )
        })?;
        return Ok(resolved_locked(&payload, &lock, artifact));
    }

    let parent = envelope
        .parent()
        .ok_or_else(|| anyhow::anyhow!("toolchain destination has no parent"))?;
    let payload_staging = extract_to_staging(&archive, parent, artifact.strip_components)?;
    verify_locked_install(payload_staging.path(), &lock, artifact, false)?;
    let envelope_staging = tempfile::Builder::new()
        .prefix(".envelope-")
        .tempdir_in(parent)
        .context("failed to create toolchain envelope staging directory")?;
    fs::rename(
        payload_staging.path(),
        envelope_staging.path().join("toolchain"),
    )
    .context("failed to place verified payload in installation envelope")?;
    fs::write(
        envelope_staging.path().join(INSTALL_COMPLETE_FILE),
        b"complete\n",
    )
    .context("failed to write toolchain completion marker")?;
    commit_staging(&envelope_staging, &envelope)?;
    verify_locked_install(&payload, &lock, artifact, true)?;
    println!("{CHECK} Installed at {}", payload.display());
    Ok(resolved_locked(&payload, &lock, artifact))
}

pub async fn resolve_for_build(
    preset: &str,
    local: Option<&Path>,
    offline: bool,
) -> Result<ResolvedToolchain> {
    if let Some(local) = explicit_local_override(local) {
        return resolve_local(&local, preset);
    }

    // An explicitly AROS-built legacy prefix remains usable without copying.
    // A plain host LLVM bundle cannot pass this marker-based check.
    let legacy = crate::host_tools::default_host_tools_dir();
    if is_legacy_aros_prefix(&legacy, preset) {
        return resolve_local(&legacy, preset);
    }

    install(preset, offline, false, None).await
}

pub fn path(preset: &str, local: Option<&Path>) -> Result<ResolvedToolchain> {
    if let Some(local) = explicit_local_override(local) {
        return resolve_local(&local, preset);
    }
    let host = host_platform_key()?;
    let lock = load_lock()?;
    let artifact = lock.resolve(host, preset).ok_or_else(|| {
        anyhow::anyhow!("no locked AROS toolchain for host '{host}' and preset '{preset}'")
    })?;
    let destination = locked_store_path(&lock, artifact);
    verify_locked_install(&destination, &lock, artifact, true)?;
    Ok(resolved_locked(&destination, &lock, artifact))
}

pub fn verify(preset: &str, local: Option<&Path>) -> Result<ResolvedToolchain> {
    let resolved = path(preset, local)?;
    smoke_host_tools(&resolved.paths)?;
    println!(
        "{CHECK} Verified {} for {} ({})",
        resolved.paths.root.display(),
        preset,
        resolved.target_triple
    );
    Ok(resolved)
}

pub fn list() -> Result<()> {
    let lock = load_lock()?;
    let current_host = host_platform_key()?;
    println!("Release: {}", style(&lock.release_id).cyan());
    for artifact in lock
        .artifacts
        .iter()
        .filter(|artifact| artifact.host == current_host)
    {
        let destination = locked_store_path(&lock, artifact);
        let status = if !artifact.enabled {
            "disabled"
        } else if verify_locked_install(&destination, &lock, artifact, true).is_ok() {
            "installed"
        } else {
            "available"
        };
        println!(
            "  {:<16} {:<22} {}",
            artifact.target_profile, artifact.target_triple, status
        );
    }
    Ok(())
}

fn resolve_local(root: &Path, preset: &str) -> Result<ResolvedToolchain> {
    let root = root
        .canonicalize()
        .with_context(|| format!("local toolchain '{}' does not exist", root.display()))?;
    let profile = target_profile(preset)?;
    let expected_triple = target_triple_for_profile(&profile);
    let manifest_path = root.join(AROS_TOOLCHAIN_MANIFEST_FILE);
    if manifest_path.exists() {
        let manifest = ArosToolchainManifest::load(&root)?;
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
        require_tool_paths(&root)?;
        return Ok(ResolvedToolchain {
            paths: get_toolchain_paths(&root),
            target_triple: manifest.target_triple,
            release_id: Some(manifest.release_id),
            source: ToolchainSource::LocalManifest,
        });
    }

    if !is_legacy_aros_prefix(&root, preset) {
        bail!(
            "local prefix '{}' has no manifest and does not look like an AROS-built {} cross-toolchain",
            root.display(),
            preset
        );
    }
    require_tool_paths(&root)?;
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
    if !root.is_dir() {
        bail!("toolchain directory '{}' is missing", root.display());
    }
    if require_complete
        && !root
            .parent()
            .is_some_and(|parent| parent.join(INSTALL_COMPLETE_FILE).is_file())
    {
        bail!("toolchain installation is incomplete");
    }
    let manifest = ArosToolchainManifest::load(root)?;
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
    require_tool_paths(root)
}

fn require_tool_paths(root: &Path) -> Result<()> {
    let paths = get_toolchain_paths(root);
    for (name, path) in [
        ("clang", &paths.clang),
        ("clang++", &paths.clangxx),
        ("ld.lld", &paths.lld),
        ("llvm-ar", &paths.llvm_ar),
    ] {
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

fn is_legacy_aros_prefix(root: &Path, preset: &str) -> bool {
    let Ok(profile) = target_profile(preset) else {
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
        && require_tool_paths(root).is_ok()
        && REQUIRED_CXX_HEADERS
            .iter()
            .all(|header| root.join("include/c++/v1").join(header).is_file())
        && root.join("lib/libc++.a").is_file()
        && root.join("lib/libc++abi.a").is_file()
        && root.join("lib/libunwind.a").is_file()
}

fn smoke_host_tools(paths: &ToolchainPaths) -> Result<()> {
    let mut tools = vec![("clang", &paths.clang), ("llvm-ar", &paths.llvm_ar)];
    if paths.aros_collect.is_file() {
        tools.push(("aros-collect", &paths.aros_collect));
    }
    if paths.collect_aros.is_file() {
        tools.push(("collect-aros", &paths.collect_aros));
    }
    if paths.collect_aros32.is_file() {
        tools.push(("collect-aros32", &paths.collect_aros32));
    }
    for (name, path) in tools {
        crate::observability::run_command(
            Command::new(path).arg("--version"),
            &format!("{name} --version at '{}'", path.display()),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aros_common::toolchain_manifest::AROS_TOOLCHAIN_MANIFEST_SCHEMA;

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
        let path = locked_store_path(&lock, &artifact);
        assert!(path.ends_with(format!(
            "release-v1/linux-x86_64/pc-x86_64/{}/toolchain",
            "a".repeat(64)
        )));
    }

    #[test]
    fn target_triples_are_profile_exact() {
        for (name, triple) in [
            ("pc-x86_64", "x86_64-unknown-aros"),
            ("arm-raspi", "arm-unknown-aros"),
            ("rpi-aarch64", "aarch64-unknown-aros"),
        ] {
            let profile = TargetProfile::default_profiles()
                .into_iter()
                .find(|profile| profile.name == name)
                .unwrap();
            assert_eq!(target_triple_for_profile(&profile), triple);
        }
    }

    #[test]
    fn completion_marker_lives_outside_immutable_payload() {
        let store = tempfile::tempdir().unwrap();
        let envelope = store.path().join("digest");
        let payload = envelope.join("toolchain");
        fs::create_dir_all(payload.join("bin")).unwrap();
        for tool in ["clang", "clang++", "ld.lld", "llvm-ar"] {
            fs::write(payload.join("bin").join(tool), b"tool").unwrap();
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
            required_paths: Vec::new(),
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
    }
}
