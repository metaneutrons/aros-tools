//! Repository discovery and access to repository-level configuration.

use miette::{IntoDiagnostic, Result};
use std::path::{Path, PathBuf};

/// Repository-relative target and host-compiler configuration.
pub const TARGETS_FILE: &str = "aros-targets.toml";

/// Return the target configuration inside the selected AROS checkout.
#[must_use]
pub fn targets_file(repo_root: &Path) -> PathBuf {
    repo_root.join(TARGETS_FILE)
}

/// Load every target from a checkout override or the built-in tools SSOT.
pub fn load_target_profiles(repo_root: &Path) -> Result<Vec<aros_common::TargetProfile>> {
    Ok(load_target_config(repo_root)?.targets)
}

/// Load the full target and host-compiler contract.
///
/// An existing checkout file is authoritative and fail-closed. A pristine
/// upstream checkout without that tools-owned file uses the contract embedded
/// in aros-tools.
pub fn load_target_config(repo_root: &Path) -> Result<aros_common::target::ArosConfig> {
    aros_common::TargetProfile::load_config_or_builtin(&targets_file(repo_root)).into_diagnostic()
}

/// Finds the repository root from a directory inside an AROS checkout.
///
/// The CLI is normally run from the checkout root, but resolving it explicitly
/// keeps `aros` useful when invoked from a subdirectory or by a wrapper.
pub fn find_root_from(start: &Path) -> Result<PathBuf> {
    let start = start.canonicalize().map_err(|error| {
        miette::miette!(
            "Could not resolve working directory '{}': {error}",
            start.display()
        )
    })?;

    for candidate in start.ancestors() {
        if is_repo_root(candidate) {
            return Ok(candidate.to_path_buf());
        }
    }

    miette::bail!(
        "Could not find an AROS source checkout above '{}'. Run this command from the repository or one of its subdirectories.",
        start.display()
    );
}

pub fn find_root() -> Result<PathBuf> {
    let current_dir = std::env::current_dir()
        .map_err(|error| miette::miette!("Could not determine the current directory: {error}"))?;
    find_root_from(&current_dir)
}

/// Return a discovered checkout when the current directory is inside one.
///
/// Unlike [`find_root`], absence is not an error. Filesystem failures while
/// resolving the current directory remain explicit.
pub fn find_root_optional() -> Result<Option<PathBuf>> {
    let current_dir = std::env::current_dir()
        .map_err(|error| miette::miette!("Could not determine the current directory: {error}"))?;
    let start = current_dir.canonicalize().map_err(|error| {
        miette::miette!(
            "Could not resolve working directory '{}': {error}",
            current_dir.display()
        )
    })?;
    Ok(start
        .ancestors()
        .find(|candidate| is_repo_root(candidate))
        .map(Path::to_path_buf))
}

pub fn is_repo_root(path: &Path) -> bool {
    path.join("configure").is_file()
        && path.join("Makefile.in").is_file()
        && path.join("arch").is_dir()
        && path.join("compiler").is_dir()
        && path.join("rom").is_dir()
}

#[cfg(test)]
mod tests {
    use super::{find_root_from, load_target_profiles, targets_file};

    #[test]
    fn finds_checkout_root_from_a_nested_directory() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let root = temp.path().join("AROS");
        for directory in ["arch", "compiler", "rom", "developer"] {
            std::fs::create_dir_all(root.join(directory)).expect("checkout layout");
        }
        std::fs::write(root.join("configure"), "").expect("configure marker");
        std::fs::write(root.join("Makefile.in"), "").expect("make marker");
        std::fs::write(
            targets_file(&root),
            "[[targets]]\nname='pc-x86_64'\narch='x86_64'\nplatform='pc'\nbsp='pc'\n",
        )
        .expect("target configuration");

        let nested = root.join("developer");
        let discovered = find_root_from(&nested).expect("root");
        assert_eq!(discovered, root.canonicalize().unwrap());
        assert_eq!(
            load_target_profiles(&discovered)
                .expect("profiles from discovered root")
                .into_iter()
                .map(|profile| profile.name)
                .collect::<Vec<_>>(),
            ["pc-x86_64"]
        );
    }

    #[test]
    fn pristine_upstream_layout_uses_built_in_profiles() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let root = temp.path().join("AROS");
        for directory in ["arch", "compiler", "rom"] {
            std::fs::create_dir_all(root.join(directory)).expect("checkout layout");
        }
        std::fs::write(root.join("configure"), "").expect("configure marker");
        std::fs::write(root.join("Makefile.in"), "").expect("make marker");

        assert_eq!(
            load_target_profiles(&root)
                .expect("built-in profiles")
                .into_iter()
                .map(|profile| profile.name)
                .collect::<Vec<_>>(),
            ["pc-x86_64", "rpi-aarch64", "arm-raspi", "opensbi-riscv64"]
        );
        assert!(!targets_file(&root).exists());
    }
}
