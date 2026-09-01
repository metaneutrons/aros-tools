//! Deterministic host LLVM selection, download, and installation.

use crate::artifact::{
    aros_home, command_exists, commit_staging, extract_to_staging, obtain_archive,
    require_absolute_state_path, require_sha256, tree_inventory_excluding,
};
use aros_common::target::{HostCompilerConfig, TargetProfile};
use aros_common::toolchain_manifest::ArosToolchainManifestEntry;
use console::{style, Emoji};
use miette::{bail, IntoDiagnostic, Result, WrapErr};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

static CHECK: Emoji<'_, '_> = Emoji("✅ ", "");
static DOWNLOAD: Emoji<'_, '_> = Emoji("⬇️  ", "");
const VERSION_TOKEN: &str = "{version}";
const INSTALL_METADATA_DIR: &str = ".aros-host-compiler";
const INSTALL_RECEIPT_FILE: &str = "receipt.json";
const INSTALL_RECEIPT_SCHEMA: u32 = 1;
const MAX_INSTALL_RECEIPT_BYTES: usize = 32 * 1024 * 1024;
const HOST_COMPILER_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HostCompilerReceipt {
    schema: u32,
    archive_sha256: String,
    llvm_version: String,
    payload_sha256: String,
    inventory_sha256: String,
    files: Vec<ArosToolchainManifestEntry>,
}

#[derive(Debug)]
struct HostCompilerMeasurement {
    payload_sha256: String,
    inventory_sha256: String,
    files: Vec<ArosToolchainManifestEntry>,
}

/// Required executable layout of a host LLVM installation.
#[derive(Debug, Clone)]
pub struct HostCompilerPaths {
    /// C compiler frontend.
    pub clang: PathBuf,
    /// C++ compiler frontend.
    pub clangxx: PathBuf,
    /// LLVM linker.
    pub lld: PathBuf,
    /// LLVM archive tool.
    pub llvm_ar: PathBuf,
}

/// Host-specific release asset selected from `aros-targets.toml`.
pub struct HostCompilerSelection {
    /// Stable host matrix key.
    pub host_key: String,
    /// Human-readable host description.
    pub platform_label: String,
    /// Selected LLVM release version.
    pub version: String,
    /// Fully resolved release-asset URL.
    pub url: String,
    /// Required archive digest from configuration.
    pub sha256: Option<String>,
}

/// Resolve the configured host-compiler installation directory.
pub fn default_host_compiler_dir() -> Result<PathBuf> {
    match std::env::var_os("AROS_HOST_COMPILER_DIR") {
        Some(path) => require_absolute_state_path("AROS_HOST_COMPILER_DIR", PathBuf::from(path)),
        None => Ok(aros_home()?.join("toolchain")),
    }
}

/// Map the running OS and CPU to the release-matrix host key.
///
/// # Errors
///
/// Returns an error when the host has no supported deterministic release.
pub fn host_platform_key() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Ok("macos-aarch64"),
        ("macos", "x86_64") => Ok("macos-x86_64"),
        ("linux", "x86_64") => Ok("linux-x86_64"),
        ("linux", "aarch64") => Ok("linux-aarch64"),
        (os, arch) => bail!("unsupported host platform: {os} {arch}"),
    }
}

/// Return a human-readable label for a host matrix key.
pub fn host_platform_label(host_key: &str) -> &str {
    match host_key {
        "macos-aarch64" => "macOS Apple Silicon (aarch64)",
        "macos-x86_64" => "macOS Intel (x86_64)",
        "linux-x86_64" => "Linux (x86_64)",
        "linux-aarch64" => "Linux (aarch64)",
        _ => host_key,
    }
}

/// Load the host-compiler release contract from the target SSOT.
///
/// # Errors
///
/// Returns an error for missing, malformed, or incomplete configuration.
pub fn load_host_compiler_config(repo_root: &Path) -> Result<HostCompilerConfig> {
    let config =
        TargetProfile::load_config(&crate::repo::targets_file(repo_root)).into_diagnostic()?;
    config.host_compiler.ok_or_else(|| {
        miette::miette!("aros-targets.toml has no required [host_compiler] configuration")
    })
}

/// Resolve the current host's exact configured asset.
///
/// # Errors
///
/// Returns an error for unsupported hosts or absent matrix entries.
pub fn select_host_compiler(cfg: &HostCompilerConfig) -> Result<HostCompilerSelection> {
    let host_key = host_platform_key()?;
    let version = cfg.llvm_version.clone();
    let host_asset = cfg.hosts.get(host_key).ok_or_else(|| {
        miette::miette!("host '{host_key}' is not configured in aros-targets.toml")
    })?;
    let asset = host_asset.asset.replace(VERSION_TOKEN, &version);
    let base_url = std::env::var("AROS_HOST_COMPILER_URL")
        .unwrap_or_else(|_| cfg.base_url.replace(VERSION_TOKEN, &version));
    Ok(HostCompilerSelection {
        host_key: host_key.into(),
        platform_label: host_platform_label(host_key).into(),
        version,
        url: format!("{}/{asset}", base_url.trim_end_matches('/')),
        sha256: host_asset.sha256.clone(),
    })
}

/// Derive required LLVM executable paths from an installation root.
pub fn host_compiler_paths(root: &Path) -> HostCompilerPaths {
    HostCompilerPaths {
        clang: root.join("bin/clang"),
        clangxx: root.join("bin/clang++"),
        lld: root.join("bin/ld.lld"),
        llvm_ar: root.join("bin/llvm-ar"),
    }
}

fn receipt_path(root: &Path) -> PathBuf {
    root.join(INSTALL_METADATA_DIR).join(INSTALL_RECEIPT_FILE)
}

fn measure_host_compiler_payload(root: &Path) -> Result<HostCompilerMeasurement> {
    let before = aros_common::measure_tree_content_cas(root)
        .into_diagnostic()
        .wrap_err("failed to measure host-compiler payload through no-follow descriptors")?;
    let (inventory_sha256, files) = tree_inventory_excluding(root, &[INSTALL_METADATA_DIR])?;
    let after = aros_common::measure_tree_content_cas(root)
        .into_diagnostic()
        .wrap_err("failed to re-measure host-compiler payload after inventory")?;
    if before != after {
        bail!("host-compiler payload changed while its complete inventory was measured");
    }
    Ok(HostCompilerMeasurement {
        payload_sha256: after
            .payload_digest_excluding(Some(INSTALL_METADATA_DIR))
            .to_string(),
        inventory_sha256,
        files,
    })
}

fn require_contained_executable(root: &Path, name: &str, path: &Path) -> Result<()> {
    let entry = fs::symlink_metadata(path)
        .into_diagnostic()
        .wrap_err_with(|| format!("required host LLVM tool '{name}' is missing"))?;
    if !entry.is_file() && !entry.file_type().is_symlink() {
        bail!("required host LLVM tool '{name}' is not a regular file or declared link");
    }
    let canonical_root = root
        .canonicalize()
        .into_diagnostic()
        .wrap_err("failed to resolve host-compiler root")?;
    let canonical = path
        .canonicalize()
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to resolve host LLVM tool '{name}'"))?;
    if !canonical.starts_with(&canonical_root) {
        bail!("required host LLVM tool '{name}' resolves outside the installed payload");
    }
    if !command_exists(&canonical) {
        bail!("required host LLVM tool '{name}' is not an executable regular file");
    }
    Ok(())
}

fn output_mentions_exact_version(output: &str, expected: &str) -> bool {
    output.match_indices(expected).any(|(offset, value)| {
        let left = output[..offset].chars().next_back();
        let right = output[offset + value.len()..].chars().next();
        left.is_none_or(|character| !character.is_ascii_digit() && character != '.')
            && right.is_none_or(|character| !character.is_ascii_digit() && character != '.')
    })
}

fn probe_host_compiler_tools(root: &Path, expected_version: &str, timeout: Duration) -> Result<()> {
    let paths = host_compiler_paths(root);
    for (name, path) in [
        ("clang", &paths.clang),
        ("clang++", &paths.clangxx),
        ("ld.lld", &paths.lld),
        ("llvm-ar", &paths.llvm_ar),
    ] {
        require_contained_executable(root, name, path)?;
        let output = crate::observability::capture_stdout_with_timeout(
            Command::new(path).arg("--version"),
            &format!("host LLVM {name} --version at '{}'", path.display()),
            timeout,
        )?;
        if !output_mentions_exact_version(&output, expected_version) {
            bail!(
                "host LLVM tool '{name}' did not report the expected LLVM version {expected_version}"
            );
        }
    }
    Ok(())
}

fn verify_payload_and_tools(
    root: &Path,
    expected_version: &str,
) -> Result<HostCompilerMeasurement> {
    let before = aros_common::measure_tree_content_cas(root)
        .into_diagnostic()
        .wrap_err("failed to establish host-compiler pre-probe snapshot")?;
    probe_host_compiler_tools(root, expected_version, HOST_COMPILER_PROBE_TIMEOUT)?;
    let measurement = measure_host_compiler_payload(root)?;
    let after = aros_common::measure_tree_content_cas(root)
        .into_diagnostic()
        .wrap_err("failed to establish host-compiler post-probe snapshot")?;
    if before != after {
        bail!("host-compiler payload changed while required tools were probed");
    }
    Ok(measurement)
}

fn write_install_receipt(root: &Path, expected_sha256: &str, expected_version: &str) -> Result<()> {
    match fs::symlink_metadata(root.join(INSTALL_METADATA_DIR)) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) => bail!(
            "downloaded host compiler contains reserved metadata namespace '{INSTALL_METADATA_DIR}'"
        ),
        Err(error) => return Err(error).into_diagnostic(),
    }
    let measurement = verify_payload_and_tools(root, expected_version)?;
    let receipt = HostCompilerReceipt {
        schema: INSTALL_RECEIPT_SCHEMA,
        archive_sha256: expected_sha256.to_owned(),
        llvm_version: expected_version.to_owned(),
        payload_sha256: measurement.payload_sha256,
        inventory_sha256: measurement.inventory_sha256,
        files: measurement.files,
    };
    let metadata = root.join(INSTALL_METADATA_DIR);
    fs::create_dir(&metadata)
        .into_diagnostic()
        .wrap_err("failed to create host-compiler metadata directory")?;
    let mut bytes = serde_json::to_vec_pretty(&receipt)
        .into_diagnostic()
        .wrap_err("failed to encode host-compiler installation receipt")?;
    bytes.push(b'\n');
    fs::write(metadata.join(INSTALL_RECEIPT_FILE), bytes)
        .into_diagnostic()
        .wrap_err("failed to write host-compiler installation receipt")
}

/// Verify the archive identity, exact installed inventory, and bounded LLVM
/// identity probes for a managed host compiler.
pub fn verify_host_compiler_install(
    root: &Path,
    expected_sha256: &str,
    expected_version: &str,
) -> Result<()> {
    let metadata = fs::symlink_metadata(root)
        .into_diagnostic()
        .wrap_err_with(|| format!("host-compiler installation '{}' is missing", root.display()))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        bail!(
            "host-compiler installation '{}' is not a real directory",
            root.display()
        );
    }
    let receipt_path = receipt_path(root);
    let Some((receipt_identity, receipt_bytes)) = aros_common::measure_regular_file(&receipt_path)
        .into_diagnostic()
        .wrap_err("host-compiler installation receipt is not a no-follow regular file")?
    else {
        bail!("host-compiler installation has no identity receipt");
    };
    if receipt_bytes.len() > MAX_INSTALL_RECEIPT_BYTES {
        bail!("host-compiler installation receipt exceeds its bounded size limit");
    }
    let receipt: HostCompilerReceipt = serde_json::from_slice(&receipt_bytes)
        .into_diagnostic()
        .wrap_err("host-compiler installation receipt is malformed")?;
    if receipt.schema != INSTALL_RECEIPT_SCHEMA {
        bail!("host-compiler installation receipt has an unsupported schema");
    }
    if receipt.archive_sha256 != expected_sha256 {
        bail!(
            "host-compiler installation was built from a different archive; expected SHA256 {expected_sha256}"
        );
    }
    if receipt.llvm_version != expected_version {
        bail!(
            "host-compiler installation reports LLVM {}, expected {expected_version}",
            receipt.llvm_version
        );
    }
    let measurement = verify_payload_and_tools(root, expected_version)?;
    if measurement.payload_sha256 != receipt.payload_sha256
        || measurement.inventory_sha256 != receipt.inventory_sha256
        || measurement.files != receipt.files
    {
        bail!("host-compiler installed payload does not match its persistent full inventory");
    }
    let remeasured_receipt = aros_common::measure_regular_file(&receipt_path)
        .into_diagnostic()
        .wrap_err("failed to re-read host-compiler installation receipt")?;
    if !remeasured_receipt
        .as_ref()
        .is_some_and(|(identity, bytes)| *identity == receipt_identity && bytes == &receipt_bytes)
    {
        bail!("host-compiler installation receipt changed during verification");
    }
    Ok(())
}

/// Install or reuse the deterministic host compiler for the current host.
///
/// # Errors
///
/// Returns an error for invalid configuration, cache/download/verification
/// failures, unsafe existing destinations, or incomplete extracted layouts.
pub async fn install(repo_root: &Path, force: bool, offline: bool) -> Result<HostCompilerPaths> {
    let config = load_host_compiler_config(repo_root)?;
    let selection = select_host_compiler(&config)?;
    let destination = default_host_compiler_dir()?;
    let paths = host_compiler_paths(&destination);
    let expected_sha256 = require_sha256(
        selection.sha256.as_deref(),
        &format!("host compiler asset for {}", selection.host_key),
    )?;

    aros_common::outputln!("Host compiler: {}", style(&selection.platform_label).cyan());
    aros_common::outputln!("LLVM version:  {}", style(&selection.version).yellow());
    aros_common::outputln!("Location:      {}", destination.display());

    match fs::symlink_metadata(&destination) {
        Ok(_) => {
            verify_host_compiler_install(
                &destination,
                &expected_sha256,
                &selection.version,
            )
            .wrap_err_with(|| {
                format!(
                    "existing host-compiler destination '{}' is invalid and was not overwritten; move it aside before reinstalling",
                    destination.display()
                )
            })?;
            if !force {
                aros_common::outputln!(
                    "{CHECK} Host compiler already matches the declared archive"
                );
                return Ok(paths);
            }
            aros_common::outputln!("{DOWNLOAD} {}", style(&selection.url).dim());
            obtain_archive(&selection.url, &expected_sha256, None, offline, true).await?;
            aros_common::outputln!(
                "{CHECK} Refreshed the verified archive cache; installed tools were unchanged"
            );
            return Ok(paths);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error)
                .into_diagnostic()
                .wrap_err_with(|| format!("failed to inspect '{}'", destination.display()));
        }
    }

    aros_common::outputln!("{DOWNLOAD} {}", style(&selection.url).dim());
    let archive = obtain_archive(&selection.url, &expected_sha256, None, offline, force).await?;

    let parent = destination
        .parent()
        .ok_or_else(|| miette::miette!("host-compiler destination has no parent"))?;
    let staging = extract_to_staging(&archive, parent, 1)?;
    write_install_receipt(staging.path(), &expected_sha256, &selection.version)?;
    commit_staging(&staging, &destination)?;
    verify_host_compiler_install(&destination, &expected_sha256, &selection.version)?;
    aros_common::outputln!(
        "{CHECK} Installed host compiler at {}",
        destination.display()
    );
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aros_common::target::HostCompilerAssetConfig;
    use std::collections::HashMap;

    const TEST_LLVM_VERSION: &str = "18.1.8";

    fn create_tool(path: &Path, body: &str) {
        fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;

            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    fn valid_install(digest: &str) -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir(directory.path().join("bin")).unwrap();
        for tool in ["clang", "clang++", "ld.lld", "llvm-ar"] {
            create_tool(
                &directory.path().join("bin").join(tool),
                "printf '%s\\n' 'LLVM version 18.1.8'",
            );
        }
        write_install_receipt(directory.path(), digest, TEST_LLVM_VERSION).unwrap();
        directory
    }

    #[test]
    fn installed_identity_requires_an_exact_receipt_and_all_tools() {
        let digest = "a".repeat(64);
        let directory = valid_install(&digest);
        verify_host_compiler_install(directory.path(), &digest, TEST_LLVM_VERSION).unwrap();

        assert!(
            verify_host_compiler_install(directory.path(), &"b".repeat(64), TEST_LLVM_VERSION)
                .is_err()
        );

        fs::remove_file(directory.path().join("bin/llvm-ar")).unwrap();
        assert!(
            verify_host_compiler_install(directory.path(), &digest, TEST_LLVM_VERSION).is_err()
        );
    }

    #[test]
    fn installed_inventory_rejects_changed_payload_bytes() {
        let digest = "a".repeat(64);
        let directory = valid_install(&digest);
        create_tool(
            &directory.path().join("bin/clang"),
            "printf '%s\\n' 'LLVM version 18.1.8 changed'",
        );

        let error =
            verify_host_compiler_install(directory.path(), &digest, TEST_LLVM_VERSION).unwrap_err();
        assert!(error.to_string().contains("persistent full inventory"));
    }

    #[cfg(unix)]
    #[test]
    fn staging_probe_checks_every_tool_and_rejects_nonzero_exit_and_wrong_version_bytes() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir(directory.path().join("bin")).unwrap();
        for tool in ["clang", "clang++", "ld.lld", "llvm-ar"] {
            create_tool(
                &directory.path().join("bin").join(tool),
                "printf '%s\\n' 'LLVM version 18.1.8'",
            );
        }
        create_tool(&directory.path().join("bin/ld.lld"), "exit 23");
        let error = verify_payload_and_tools(directory.path(), TEST_LLVM_VERSION).unwrap_err();
        assert!(error.to_string().contains("ld.lld"));

        for failing_tool in ["clang", "clang++", "ld.lld", "llvm-ar"] {
            for tool in ["clang", "clang++", "ld.lld", "llvm-ar"] {
                create_tool(
                    &directory.path().join("bin").join(tool),
                    if tool == failing_tool {
                        "printf '%s\\n' 'LLVM version 118.1.80'"
                    } else {
                        "printf '%s\\n' 'LLVM version 18.1.8'"
                    },
                );
            }
            let error = verify_payload_and_tools(directory.path(), TEST_LLVM_VERSION).unwrap_err();
            assert!(error.to_string().contains(failing_tool));
            assert!(error.to_string().contains("expected LLVM version"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn staging_probe_has_a_bounded_process_group_deadline() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir(directory.path().join("bin")).unwrap();
        for tool in ["clang", "clang++", "ld.lld", "llvm-ar"] {
            create_tool(
                &directory.path().join("bin").join(tool),
                if tool == "clang++" {
                    "sleep 30"
                } else {
                    "printf '%s\\n' 'LLVM version 18.1.8'"
                },
            );
        }

        let error =
            probe_host_compiler_tools(directory.path(), TEST_LLVM_VERSION, Duration::from_secs(2))
                .unwrap_err();
        let diagnostic = crate::observability::report_diagnostic(
            &error,
            crate::observability::ErrorBoundary::REPOSITORY,
            aros_common::DiagnosticContext::default(),
        );
        assert!(diagnostic.message.contains("clang++"));
        assert!(diagnostic.message.contains("timed out"));
    }

    #[test]
    fn selection_uses_only_the_declared_host_compiler_version() {
        let host_key = host_platform_key().unwrap();
        let cfg = HostCompilerConfig {
            llvm_version: "18.1.8".into(),
            base_url: "https://example.invalid/llvm/{version}".into(),
            hosts: HashMap::from([(
                host_key.into(),
                HostCompilerAssetConfig {
                    asset: "clang-{version}.tar.xz".into(),
                    sha256: Some("1".repeat(64)),
                },
            )]),
        };

        let selected = select_host_compiler(&cfg).unwrap();
        assert_eq!(selected.version, "18.1.8");
        assert_eq!(
            selected.url,
            "https://example.invalid/llvm/18.1.8/clang-18.1.8.tar.xz"
        );
    }

    #[cfg(unix)]
    #[test]
    fn installed_identity_rejects_a_symlink_receipt() {
        use std::os::unix::fs::symlink;

        let digest = "a".repeat(64);
        let directory = valid_install(&digest);
        let receipt = receipt_path(directory.path());
        let external = directory.path().join("external-receipt");
        fs::write(&external, b"{}\n").unwrap();
        fs::remove_file(&receipt).unwrap();
        symlink(&external, &receipt).unwrap();
        assert!(
            verify_host_compiler_install(directory.path(), &digest, TEST_LLVM_VERSION).is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn installed_identity_rejects_a_tool_symlink_escape() {
        use std::os::unix::fs::symlink;

        let digest = "a".repeat(64);
        let directory = valid_install(&digest);
        let external = tempfile::tempdir().unwrap();
        let external_tool = external.path().join("clang");
        create_tool(&external_tool, "printf '%s\\n' 'LLVM version 18.1.8'");
        let clang = directory.path().join("bin/clang");
        fs::remove_file(&clang).unwrap();
        symlink(&external_tool, &clang).unwrap();

        let error =
            verify_host_compiler_install(directory.path(), &digest, TEST_LLVM_VERSION).unwrap_err();
        assert!(error.to_string().contains("outside the installed payload"));
    }
}
