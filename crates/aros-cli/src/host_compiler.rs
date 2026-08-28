//! Deterministic host LLVM selection, download, and installation.

use crate::artifact::{
    aros_home, command_exists, commit_staging, extract_to_staging, obtain_archive, require_sha256,
};
use aros_common::target::{HostCompilerConfig, TargetProfile};
use console::{style, Emoji};
use miette::{bail, IntoDiagnostic, Result, WrapErr};
use std::fs;
use std::path::{Path, PathBuf};

static CHECK: Emoji<'_, '_> = Emoji("✅ ", "");
static DOWNLOAD: Emoji<'_, '_> = Emoji("⬇️  ", "");
const VERSION_TOKEN: &str = "{version}";

#[derive(Debug, Clone)]
pub struct HostCompilerPaths {
    pub clang: PathBuf,
    pub clangxx: PathBuf,
    pub lld: PathBuf,
    pub llvm_ar: PathBuf,
}

pub struct HostCompilerSelection {
    pub host_key: String,
    pub platform_label: String,
    pub version: String,
    pub url: String,
    pub sha256: Option<String>,
}

pub fn default_host_compiler_dir() -> PathBuf {
    std::env::var_os("AROS_HOST_COMPILER_DIR")
        .or_else(|| std::env::var_os("AROS_HOST_TOOLS_DIR"))
        .or_else(|| std::env::var_os("AROS_TOOLCHAIN_DIR"))
        .map_or_else(|| aros_home().join("toolchain"), PathBuf::from)
}

pub fn host_platform_key() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Ok("macos-aarch64"),
        ("macos", "x86_64") => Ok("macos-x86_64"),
        ("linux", "x86_64") => Ok("linux-x86_64"),
        ("linux", "aarch64") => Ok("linux-aarch64"),
        (os, arch) => bail!("unsupported host platform: {os} {arch}"),
    }
}

pub fn host_platform_label(host_key: &str) -> &str {
    match host_key {
        "macos-aarch64" => "macOS Apple Silicon (aarch64)",
        "macos-x86_64" => "macOS Intel (x86_64)",
        "linux-x86_64" => "Linux (x86_64)",
        "linux-aarch64" => "Linux (aarch64)",
        _ => host_key,
    }
}

pub fn load_host_compiler_config() -> Result<HostCompilerConfig> {
    let config =
        TargetProfile::load_config(Path::new(crate::repo::TARGETS_FILE)).into_diagnostic()?;
    config.host_compiler.ok_or_else(|| {
        miette::miette!("aros-targets.toml has no required [host_compiler] configuration")
    })
}

pub fn select_host_compiler(cfg: &HostCompilerConfig) -> Result<HostCompilerSelection> {
    let host_key = host_platform_key()?;
    let version = std::env::var("AROS_LLVM_VERSION").unwrap_or_else(|_| cfg.llvm_version.clone());
    let host_asset = cfg.hosts.get(host_key).ok_or_else(|| {
        miette::miette!("host '{host_key}' is not configured in aros-targets.toml")
    })?;
    let asset = host_asset.asset.replace(VERSION_TOKEN, &version);
    let base_url = std::env::var("AROS_HOST_COMPILER_URL")
        .or_else(|_| std::env::var("AROS_HOST_TOOLS_URL"))
        .or_else(|_| std::env::var("AROS_TOOLCHAIN_URL"))
        .unwrap_or_else(|_| cfg.base_url.replace(VERSION_TOKEN, &version));
    Ok(HostCompilerSelection {
        host_key: host_key.into(),
        platform_label: host_platform_label(host_key).into(),
        version,
        url: format!("{}/{asset}", base_url.trim_end_matches('/')),
        sha256: host_asset.sha256.clone(),
    })
}

pub fn host_compiler_paths(root: &Path) -> HostCompilerPaths {
    HostCompilerPaths {
        clang: root.join("bin/clang"),
        clangxx: root.join("bin/clang++"),
        lld: root.join("bin/ld.lld"),
        llvm_ar: root.join("bin/llvm-ar"),
    }
}

pub fn is_host_compiler_installed(paths: &HostCompilerPaths) -> bool {
    command_exists(&paths.clang)
        && command_exists(&paths.clangxx)
        && command_exists(&paths.lld)
        && command_exists(&paths.llvm_ar)
}

pub async fn install(force: bool, offline: bool) -> Result<HostCompilerPaths> {
    let config = load_host_compiler_config()?;
    let selection = select_host_compiler(&config)?;
    let destination = default_host_compiler_dir();
    let paths = host_compiler_paths(&destination);

    println!("Host compiler: {}", style(&selection.platform_label).cyan());
    println!("LLVM version:  {}", style(&selection.version).yellow());
    println!("Location:      {}", destination.display());

    if is_host_compiler_installed(&paths) && !force {
        println!("{CHECK} Host compiler already available and structurally valid");
        return Ok(paths);
    }

    let expected_sha256 = require_sha256(
        selection.sha256.as_deref(),
        &format!("host compiler asset for {}", selection.host_key),
    )?;
    println!("{DOWNLOAD} {}", style(&selection.url).dim());
    let archive = obtain_archive(&selection.url, &expected_sha256, None, offline, force).await?;

    if destination.exists() {
        if is_host_compiler_installed(&paths) {
            println!("{CHECK} Existing host compiler matches the required layout");
            return Ok(paths);
        }
        bail!(
            "refusing to overwrite existing host-compiler directory '{}'; move it aside or set AROS_HOST_COMPILER_DIR",
            destination.display()
        );
    }

    let parent = destination
        .parent()
        .ok_or_else(|| miette::miette!("host-compiler destination has no parent"))?;
    let staging = extract_to_staging(&archive, parent, 1)?;
    let staged_paths = host_compiler_paths(staging.path());
    if !is_host_compiler_installed(&staged_paths) {
        bail!("downloaded host compiler does not contain the required LLVM tools");
    }
    fs::write(
        staging.path().join(".aros-host-compiler-sha256"),
        format!("{expected_sha256}\n"),
    )
    .into_diagnostic()
    .wrap_err("failed to write host-compiler installation marker")?;
    commit_staging(&staging, &destination)?;
    println!(
        "{CHECK} Installed host compiler at {}",
        destination.display()
    );
    Ok(paths)
}
