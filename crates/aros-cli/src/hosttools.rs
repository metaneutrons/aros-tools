use console::style;
use miette::Result;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

const REQUIRED_HOST_TOOLS: &[&str] = &["aros-transpiler", "aros-genmodule", "aros-romtool"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostToolsCheck {
    pub bin_dir: PathBuf,
    pub missing: Vec<PathBuf>,
}

impl HostToolsCheck {
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        self.missing.is_empty()
    }
}

#[must_use]
pub fn workspace_dir(repo_root: &Path) -> PathBuf {
    repo_root.join("tools/aros-tools")
}

#[must_use]
pub fn binary_dir(repo_root: &Path) -> PathBuf {
    workspace_dir(repo_root).join("target/release")
}

#[must_use]
pub fn check(repo_root: &Path) -> HostToolsCheck {
    let bin_dir = binary_dir(repo_root);
    let missing = REQUIRED_HOST_TOOLS
        .iter()
        .map(|name| bin_dir.join(executable_name(name)))
        .filter(|path| !is_executable(path))
        .collect();

    HostToolsCheck { bin_dir, missing }
}

/// Builds the Rust programs which CMake consumes at configure time.
///
/// CMake deliberately uses the workspace's default `target/release` location,
/// so this command pins `CARGO_TARGET_DIR` to that same per-checkout path.
/// That keeps independent Git worktrees isolated while avoiding a mismatch
/// between Cargo output and CMake's lookup path.
pub fn build(repo_root: &Path) -> Result<HostToolsCheck> {
    let workspace = workspace_dir(repo_root);
    if !workspace.join("Cargo.toml").is_file() {
        miette::bail!(
            "AROS host-tools workspace is missing at '{}'.",
            workspace.display()
        );
    }

    let target_dir = workspace.join("target");
    let args = cargo_build_args();
    println!(
        "🔧 Building AROS host tools in {}...",
        style(target_dir.display()).cyan()
    );

    let status = Command::new(cargo_program())
        .args(&args)
        .current_dir(&workspace)
        .env("CARGO_TARGET_DIR", &target_dir)
        .status()
        .map_err(|error| {
            miette::miette!(
                "Could not start Cargo while building AROS host tools: {error}. Install Rust/Cargo and retry."
            )
        })?;
    if !status.success() {
        miette::bail!("Cargo failed while building the required AROS host tools.");
    }

    let result = check(repo_root);
    if !result.is_complete() {
        miette::bail!(
            "Cargo completed, but required AROS host tools are still missing: {}",
            format_missing(&result.missing)
        );
    }

    println!(
        "✅ AROS host tools are ready in {}.",
        result.bin_dir.display()
    );
    Ok(result)
}

/// Ensures the CMake configure-time Rust helpers exist, building them only
/// when they are absent or no longer executable.
pub fn ensure(repo_root: &Path) -> Result<HostToolsCheck> {
    let result = check(repo_root);
    if result.is_complete() {
        return Ok(result);
    }

    println!(
        "ℹ️ Required AROS host tools are missing: {}",
        format_missing(&result.missing)
    );
    build(repo_root)
}

pub fn print_check(repo_root: &Path) -> Result<()> {
    let result = check(repo_root);
    if result.is_complete() {
        println!(
            "✅ AROS host tools are ready in {}.",
            result.bin_dir.display()
        );
        return Ok(());
    }

    println!(
        "❌ Missing AROS host tools: {}",
        format_missing(&result.missing)
    );
    println!("   Run `aros hosttools build` to build them.");
    miette::bail!("Required AROS host tools are unavailable.");
}

#[must_use]
pub fn cargo_build_args() -> Vec<OsString> {
    let mut args = vec![
        OsString::from("build"),
        OsString::from("--locked"),
        OsString::from("--release"),
    ];
    for package in REQUIRED_HOST_TOOLS {
        args.push(OsString::from("--package"));
        args.push(OsString::from(package));
    }
    args
}

fn cargo_program() -> OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"))
}

fn executable_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }

    #[cfg(not(unix))]
    {
        true
    }
}

fn format_missing(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|path| {
            path.file_name()
                .unwrap_or(path.as_os_str())
                .to_string_lossy()
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::{binary_dir, cargo_build_args, check, REQUIRED_HOST_TOOLS};

    #[test]
    fn cargo_build_plan_builds_every_required_tool_in_release_mode() {
        let args = cargo_build_args();
        let rendered = args
            .iter()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>();

        assert_eq!(rendered[..3], ["build", "--locked", "--release"]);
        for tool in REQUIRED_HOST_TOOLS {
            assert!(rendered.iter().any(|argument| argument == tool));
        }
    }

    #[test]
    fn check_reports_missing_tools_from_the_checkout_local_release_directory() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let result = check(temp.path());

        assert_eq!(result.bin_dir, binary_dir(temp.path()));
        assert_eq!(result.missing.len(), REQUIRED_HOST_TOOLS.len());
    }
}
