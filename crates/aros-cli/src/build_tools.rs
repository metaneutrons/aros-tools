//! Build and resolve the Rust tools consumed by the generated AROS build.

use console::style;
use miette::Result;
use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

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
const BUILD_TOOL_VERSION_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
struct BuildToolIssue {
    path: PathBuf,
    detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildToolsCheck {
    pub bin_dir: PathBuf,
    pub missing: Vec<PathBuf>,
    incompatible: Vec<BuildToolIssue>,
}

impl BuildToolsCheck {
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        self.missing.is_empty() && self.incompatible.is_empty()
    }

    /// Render every missing or incompatible suite member for diagnostics.
    #[must_use]
    pub fn problem_summary(&self) -> String {
        let mut problems = Vec::new();
        if !self.missing.is_empty() {
            problems.push(format!("missing {}", format_missing(&self.missing)));
        }
        if !self.incompatible.is_empty() {
            problems.push(format!(
                "incompatible {}",
                self.incompatible
                    .iter()
                    .map(|issue| format!("{} ({})", issue.path.display(), issue.detail))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        problems.join("; ")
    }
}

#[must_use]
pub fn check(repo_root: Option<&Path>) -> BuildToolsCheck {
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

    check_directory(candidates.into_iter().next().unwrap_or_else(|| {
        repo_root.map_or_else(
            || PathBuf::from("target/release"),
            |root| root.join("tools/aros-tools/target/release"),
        )
    }))
}

fn check_directory(bin_dir: PathBuf) -> BuildToolsCheck {
    let mut missing = Vec::new();
    let mut incompatible = Vec::new();
    for name in REQUIRED_BUILD_TOOLS {
        let path = bin_dir.join(executable_name(name));
        if !is_executable(&path) {
            missing.push(path);
            continue;
        }
        if let Err(detail) = validate_tool_version(name, &path) {
            incompatible.push(BuildToolIssue { path, detail });
        }
    }

    BuildToolsCheck {
        bin_dir,
        missing,
        incompatible,
    }
}

fn validate_tool_version(name: &str, path: &Path) -> std::result::Result<(), String> {
    validate_tool_version_with_timeout(name, path, BUILD_TOOL_VERSION_TIMEOUT)
}

fn validate_tool_version_with_timeout(
    name: &str,
    path: &Path,
    timeout: Duration,
) -> std::result::Result<(), String> {
    let observed = crate::observability::capture_stdout_with_timeout(
        Command::new(path).arg("--version"),
        &format!("{name} suite-version probe at '{}'", path.display()),
        timeout,
    )
    .map_err(|error| error.to_string())?;
    let expected = format!("{name} {}", env!("CARGO_PKG_VERSION"));
    if observed != expected {
        return Err(format!(
            "reported version {observed:?}; expected {expected:?}"
        ));
    }
    Ok(())
}

fn candidate_directories(repo_root: Option<&Path>) -> Vec<PathBuf> {
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

fn source_workspace(repo_root: Option<&Path>) -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("AROS_TOOLS_SOURCE_DIR") {
        return Some(PathBuf::from(explicit));
    }
    repo_root.and_then(|repo_root| {
        let transitional = repo_root.join("tools/aros-tools");
        transitional
            .join("Cargo.toml")
            .is_file()
            .then_some(transitional)
    })
}

/// Builds the Rust programs which CMake consumes at configure time.
///
/// Standalone installations normally ship all binaries together and do not
/// need this command. Source builds select the workspace explicitly with
/// `AROS_TOOLS_SOURCE_DIR`; an embedded legacy path remains a transitional
/// fallback until the old repository is retired.
pub fn build(repo_root: Option<&Path>) -> Result<BuildToolsCheck> {
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
    aros_common::outputln!(
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
            "Cargo completed, but the required AROS build-tool suite is invalid: {}",
            result.problem_summary()
        );
    }

    aros_common::outputln!(
        "✅ AROS build tools are ready in {}.",
        result.bin_dir.display()
    );
    Ok(result)
}

/// Ensures the CMake configure-time Rust helpers exist, building them only
/// when they are absent or no longer executable.
pub fn ensure(repo_root: &Path) -> Result<BuildToolsCheck> {
    let result = check(Some(repo_root));
    if result.is_complete() {
        return Ok(result);
    }

    if source_workspace(Some(repo_root)).is_some() {
        aros_common::outputln!(
            "ℹ️ Required AROS build tools are unavailable: {}",
            result.problem_summary()
        );
        return build(Some(repo_root));
    }

    miette::bail!(
        "Required AROS build tools are unavailable or incompatible in '{}': {}. Install one complete aros-tools version, add its binary directory to PATH, or set AROS_BUILD_TOOLS_DIR.",
        result.bin_dir.display(),
        result.problem_summary()
    );
}

pub fn print_check(repo_root: Option<&Path>) -> Result<()> {
    let result = check(repo_root);
    if result.is_complete() {
        aros_common::outputln!(
            "✅ AROS build tools are ready in {}.",
            result.bin_dir.display()
        );
        return Ok(());
    }

    aros_common::outputln!(
        "❌ Invalid AROS build-tool suite: {}",
        result.problem_summary()
    );
    aros_common::outputln!(
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
    use super::{
        cargo_build_args, check_directory, validate_tool_version_with_timeout, REQUIRED_BUILD_TOOLS,
    };
    use std::fs;
    use std::path::Path;
    use std::time::Duration;

    #[cfg(unix)]
    fn write_version_tool(path: &Path, name: &str, version: &str, extra: &str) {
        use std::io::Write as _;
        use std::os::unix::fs::PermissionsExt as _;

        // Publish the executable only after its writable handle is closed.
        // Linux otherwise permits a narrow ETXTBSY race on busy CI filesystems.
        let staged = path.with_extension("fixture-staged");
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged)
            .expect("staged version fixture");
        file.write_all(
            format!(
                "#!/bin/sh\nset -eu\nif [ \"${{1:-}}\" = \"--version\" ]; then\n  {extra}\n  printf '%s\\n' '{name} {version}'\n  exit 0\nfi\nexit 64\n"
            )
            .as_bytes(),
        )
        .expect("version fixture contents");
        file.sync_all().expect("version fixture synchronization");
        drop(file);
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o755))
            .expect("version fixture permissions");
        fs::rename(&staged, path).expect("publish version fixture");
    }

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
        assert!(result.incompatible.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn check_accepts_only_one_exact_workspace_version() {
        let temp = tempfile::tempdir().expect("temporary directory");
        for tool in REQUIRED_BUILD_TOOLS {
            write_version_tool(
                temp.path().join(tool).as_path(),
                tool,
                env!("CARGO_PKG_VERSION"),
                "",
            );
        }

        let result = check_directory(temp.path().to_path_buf());
        assert!(result.is_complete(), "{}", result.problem_summary());
    }

    #[cfg(unix)]
    #[test]
    fn check_rejects_a_mixed_version_suite() {
        let temp = tempfile::tempdir().expect("temporary directory");
        for tool in REQUIRED_BUILD_TOOLS {
            let version = if *tool == "aros-fetch" {
                "99.0.0"
            } else {
                env!("CARGO_PKG_VERSION")
            };
            write_version_tool(temp.path().join(tool).as_path(), tool, version, "");
        }

        let result = check_directory(temp.path().to_path_buf());
        assert!(!result.is_complete());
        assert!(result.missing.is_empty());
        assert_eq!(result.incompatible.len(), 1);
        assert!(result.problem_summary().contains("aros-fetch"));
        assert!(result.problem_summary().contains("99.0.0"));
    }

    #[cfg(unix)]
    #[test]
    fn version_probe_has_a_hard_process_group_deadline() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let tool = temp.path().join("aros-fetch");
        write_version_tool(&tool, "aros-fetch", env!("CARGO_PKG_VERSION"), "sleep 10");

        let error =
            validate_tool_version_with_timeout("aros-fetch", &tool, Duration::from_millis(50))
                .expect_err("hanging version probe must fail");
        assert!(error.contains("timed out"), "{error}");
    }
}
