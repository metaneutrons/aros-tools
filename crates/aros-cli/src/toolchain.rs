use anyhow::{bail, Context, Result};
use aros_common::target::{ArosConfig, TargetProfile, ToolchainConfig};
use console::{style, Emoji};
use futures_util::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use std::fs::{self, File};
use std::io::BufReader;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use xz2::read::XzDecoder;

static DOWNLOAD: Emoji<'_, '_> = Emoji("⬇️  ", "");
static PACKAGE: Emoji<'_, '_> = Emoji("📦 ", "");
static CHECK: Emoji<'_, '_> = Emoji("✅ ", "");
static SPARKLES: Emoji<'_, '_> = Emoji("✨ ", "");

#[allow(dead_code)]
pub struct ToolchainPaths {
    pub root: PathBuf,
    pub clang: PathBuf,
    pub clangxx: PathBuf,
    pub lld: PathBuf,
    pub llvm_ar: PathBuf,
}

pub fn default_toolchain_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("AROS_TOOLCHAIN_DIR") {
        PathBuf::from(dir)
    } else if let Some(home) = dirs_home() {
        home.join(".aros").join("toolchain")
    } else {
        PathBuf::from(".aros-toolchain")
    }
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}

pub fn load_toolchain_config() -> ToolchainConfig {
    let config = TargetProfile::load_config(Path::new("aros-targets.toml"))
        .unwrap_or_else(|_| ArosConfig {
            toolchain: Some(ToolchainConfig::default()),
            targets: TargetProfile::default_profiles(),
        });
    config.toolchain.unwrap_or_default()
}

pub fn detect_host_platform(cfg: &ToolchainConfig) -> Result<(String, String)> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    let host_key = match (os, arch) {
        ("macos", "aarch64") => "macos-aarch64",
        ("macos", "x86_64") => "macos-x86_64",
        ("linux", "x86_64") => "linux-x86_64",
        ("linux", "aarch64") => "linux-aarch64",
        _ => bail!("Unsupported host platform: {} {}", os, arch),
    };

    let platform_label = match host_key {
        "macos-aarch64" => "macOS Apple Silicon (aarch64)",
        "macos-x86_64" => "macOS Intel (x86_64)",
        "linux-x86_64" => "Linux (x86_64)",
        "linux-aarch64" => "Linux (aarch64)",
        _ => host_key,
    };

    let version = std::env::var("AROS_LLVM_VERSION").unwrap_or_else(|_| cfg.llvm_version.clone());
    let host_asset = cfg
        .hosts
        .get(host_key)
        .ok_or_else(|| anyhow::anyhow!("Host '{}' not configured in aros-targets.toml", host_key))?;

    let asset_filename = host_asset.asset.replace("{version}", &version);
    let base_url = std::env::var("AROS_TOOLCHAIN_URL")
        .unwrap_or_else(|_| cfg.base_url.replace("{version}", &version));

    let download_url = format!("{base_url}/{asset_filename}");
    Ok((platform_label.to_string(), download_url))
}

pub fn get_toolchain_paths(root: &Path) -> ToolchainPaths {
    ToolchainPaths {
        root: root.to_path_buf(),
        clang: root.join("bin").join("clang"),
        clangxx: root.join("bin").join("clang++"),
        lld: root.join("bin").join("ld.lld"),
        llvm_ar: root.join("bin").join("llvm-ar"),
    }
}

pub fn is_toolchain_installed(paths: &ToolchainPaths) -> bool {
    paths.clang.exists() && (paths.lld.exists() || paths.root.join("bin").join("lld").exists())
}

pub async fn setup_toolchain(force: bool) -> Result<ToolchainPaths> {
    let dest_dir = default_toolchain_dir();
    let paths = get_toolchain_paths(&dest_dir);
    let cfg = load_toolchain_config();

    println!(
        "{SPARKLES} {}",
        style("AROS-NG Declarative Toolchain Manager").cyan().bold()
    );

    let (platform_name, url) = detect_host_platform(&cfg)?;
    println!("  • Host Platform:     {}", style(platform_name).green().bold());
    println!("  • LLVM Version:      {}", style(&cfg.llvm_version).yellow().bold());
    println!("  • Target Location:   {}", style(dest_dir.display()).dim());
    println!("  • Config Source:     {}", style("aros-targets.toml [toolchain]").cyan());

    if !force && is_toolchain_installed(&paths) {
        println!(
            "{CHECK} Toolchain is already installed and verified at {}",
            style(paths.root.display()).green().bold()
        );
        return Ok(paths);
    }

    fs::create_dir_all(&dest_dir).context("Failed to create toolchain root directory")?;

    let client = reqwest::Client::builder()
        .user_agent("aros-tools/0.1.0 (https://aros.org)")
        .build()?;

    println!("\n{DOWNLOAD} Downloading declarative LLVM/LLD toolchain from GitHub...");
    println!("  {}", style(&url).dim());

    let res = client.get(&url).send().await.context("Failed to connect to download source")?;
    if !res.status().is_success() {
        bail!("Failed to download toolchain from '{}': HTTP {}", url, res.status());
    }

    let total_size = res.content_length().unwrap_or(0);
    let pb = ProgressBar::new(total_size);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta}) {msg}")
            .expect("Failed to set progress bar template")
            .progress_chars("#>-"),
    );

    let temp_archive = tempfile::NamedTempFile::new()
        .context("Failed to create temporary download file")?;
    let temp_path = temp_archive.path().to_path_buf();

    let mut file = tokio::fs::File::create(&temp_path)
        .await
        .context("Failed to open temporary file for writing")?;

    let mut stream = res.bytes_stream();
    while let Some(chunk_res) = stream.next().await {
        let chunk = chunk_res.context("Error downloading chunk")?;
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk)
            .await
            .context("Error writing chunk to disk")?;
        pb.inc(chunk.len() as u64);
    }
    pb.finish_with_message("Download complete!");

    println!("{PACKAGE} Extracting toolchain archive into {}...", style(dest_dir.display()).cyan());

    // Extract .tar.xz
    let tar_file = File::open(&temp_path).context("Failed to open downloaded archive")?;
    let xz_decoder = XzDecoder::new(BufReader::new(tar_file));
    let mut archive = tar::Archive::new(xz_decoder);

    // Extract entries stripping top-level directory
    for entry_res in archive.entries().context("Failed to read tar archive")? {
        let mut entry = entry_res.context("Tar entry error")?;
        let path = entry.path().context("Invalid tar entry path")?.to_path_buf();

        let mut components = path.components();
        components.next(); // Strip root directory (e.g. clang+llvm-18.1.8-arm64-apple-macos11/)
        let sub_path: PathBuf = components.collect();

        if sub_path.as_os_str().is_empty() {
            continue;
        }

        let target_file = dest_dir.join(&sub_path);
        if entry.header().entry_type().is_dir() {
            fs::create_dir_all(&target_file)?;
        } else {
            if let Some(parent) = target_file.parent() {
                fs::create_dir_all(parent)?;
            }
            entry.unpack(&target_file)?;

            // Set executable bit on unix
            if let Ok(metadata) = target_file.metadata() {
                let mut perms = metadata.permissions();
                let mode = perms.mode();
                if target_file.starts_with(dest_dir.join("bin")) || (mode & 0o111 != 0) {
                    perms.set_mode(0o755);
                    let _ = fs::set_permissions(&target_file, perms);
                }
            }
        }
    }

    println!("{CHECK} {} {}", style("Declarative toolchain successfully installed to:").green().bold(), dest_dir.display());

    Ok(paths)
}
