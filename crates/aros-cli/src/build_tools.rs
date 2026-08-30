//! Build and resolve the Rust tools consumed by the generated AROS build.

use console::style;
use miette::Result;
use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

// aros-collect and aros-ahi-runner belong here even though they run during the
// build rather than at configure time. Generated build rules require these
// exact executables; omitting either one makes a fresh checkout configure
// successfully only to fail later or bypass required AROS semantics.
const REQUIRED_BUILD_TOOLS: &[&str] = &[
    "aros-transpiler",
    "aros-genmodule",
    "aros-romtool",
    "aros-collect",
    "aros-ahi-runner",
    "aros-fetch",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildToolsCheck {
    pub bin_dir: PathBuf,
    pub missing: Vec<PathBuf>,
}

impl BuildToolsCheck {
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        self.missing.is_empty()
    }
}

#[must_use]
pub fn check(repo_root: &Path) -> BuildToolsCheck {
    if let Some(explicit) = std::env::var_os("AROS_BUILD_TOOLS_DIR") {
        return check_directory(PathBuf::from(explicit));
    }

    let candidates = candidate_directories(repo_root);
    for candidate in &candidates {
        let result = check_directory(candidate.clone());
        if result.is_complete() {
            return result;
        }
    }

    check_directory(
        candidates
            .into_iter()
            .next()
            .unwrap_or_else(|| repo_root.join("tools/aros-tools/target/release")),
    )
}

fn check_directory(bin_dir: PathBuf) -> BuildToolsCheck {
    let missing = REQUIRED_BUILD_TOOLS
        .iter()
        .map(|name| bin_dir.join(executable_name(name)))
        .filter(|path| !is_executable(path))
        .collect();

    BuildToolsCheck { bin_dir, missing }
}

fn candidate_directories(repo_root: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(executable) = std::env::current_exe() {
        if let Some(parent) = executable.parent() {
            candidates.push(parent.to_path_buf());
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path));
    }
    if let Some(workspace) = source_workspace(repo_root) {
        candidates.push(workspace.join("target/release"));
    }

    let mut seen = HashSet::new();
    candidates.retain(|candidate| seen.insert(candidate.clone()));
    candidates
}

fn source_workspace(repo_root: &Path) -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("AROS_TOOLS_SOURCE_DIR") {
        return Some(PathBuf::from(explicit));
    }
    let transitional = repo_root.join("tools/aros-tools");
    transitional
        .join("Cargo.toml")
        .is_file()
        .then_some(transitional)
}

/// Builds the Rust programs which CMake consumes at configure time.
///
/// Standalone installations normally ship all binaries together and do not
/// need this command. Source builds select the workspace explicitly with
/// `AROS_TOOLS_SOURCE_DIR`; the embedded AROS-NG path remains a transitional
/// fallback until the old repository is retired.
pub fn build(repo_root: &Path) -> Result<BuildToolsCheck> {
    let workspace = source_workspace(repo_root).ok_or_else(|| {
        miette::miette!(
            "No AROS tools source workspace is configured. Install the complete aros-tools package, or set AROS_TOOLS_SOURCE_DIR to a checkout of the aros-tools repository."
        )
    })?;
    if !workspace.join("Cargo.toml").is_file() {
        miette::bail!(
            "AROS build-tools workspace is missing at '{}'.",
            workspace.display()
        );
    }

    let target_dir = workspace.join("target");
    let args = cargo_build_args();
    println!(
        "🔧 Building AROS build tools in {}...",
        style(target_dir.display()).cyan()
    );

    crate::observability::run_command(
        Command::new(cargo_program())
            .args(&args)
            .current_dir(&workspace)
            .env("CARGO_TARGET_DIR", &target_dir),
        "Cargo while building the required AROS build tools",
    )?;

    let result = check_directory(target_dir.join("release"));
    if !result.is_complete() {
        miette::bail!(
            "Cargo completed, but required AROS build tools are still missing: {}",
            format_missing(&result.missing)
        );
    }

    println!(
        "✅ AROS build tools are ready in {}.",
        result.bin_dir.display()
    );
    Ok(result)
}

/// Ensures the CMake configure-time Rust helpers exist, building them only
/// when they are absent or no longer executable.
pub fn ensure(repo_root: &Path) -> Result<BuildToolsCheck> {
    let result = check(repo_root);
    if result.is_complete() {
        return Ok(result);
    }

    if source_workspace(repo_root).is_some() {
        println!(
            "ℹ️ Required AROS build tools are missing: {}",
            format_missing(&result.missing)
        );
        return build(repo_root);
    }

    miette::bail!(
        "Required AROS build tools are unavailable from '{}': {}. Install the complete aros-tools package, add its binary directory to PATH, or set AROS_BUILD_TOOLS_DIR.",
        result.bin_dir.display(),
        format_missing(&result.missing)
    );
}

pub fn print_check(repo_root: &Path) -> Result<()> {
    let result = check(repo_root);
    if result.is_complete() {
        println!(
            "✅ AROS build tools are ready in {}.",
            result.bin_dir.display()
        );
        return Ok(());
    }

    println!(
        "❌ Missing AROS build tools: {}",
        format_missing(&result.missing)
    );
    println!(
        "   Install the complete aros-tools package, add its binary directory to PATH, or set AROS_BUILD_TOOLS_DIR."
    );
    miette::bail!("Required AROS build tools are unavailable.");
}

#[must_use]
pub fn cargo_build_args() -> Vec<OsString> {
    let mut args = vec![
        OsString::from("build"),
        OsString::from("--locked"),
        OsString::from("--release"),
    ];
    for package in REQUIRED_BUILD_TOOLS {
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
    use super::{cargo_build_args, check_directory, REQUIRED_BUILD_TOOLS};

    #[test]
    fn cargo_build_plan_builds_every_required_tool_in_release_mode() {
        let args = cargo_build_args();
        let rendered = args
            .iter()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>();

        assert_eq!(rendered[..3], ["build", "--locked", "--release"]);
        for tool in REQUIRED_BUILD_TOOLS {
            assert!(rendered.iter().any(|argument| argument == tool));
        }
    }

    #[test]
    fn check_reports_missing_tools_from_the_selected_directory() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let result = check_directory(temp.path().to_path_buf());

        assert_eq!(result.bin_dir, temp.path());
        assert_eq!(result.missing.len(), REQUIRED_BUILD_TOOLS.len());
    }
}
