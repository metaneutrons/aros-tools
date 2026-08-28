//! Repository discovery and access to repository-level configuration.

use miette::{IntoDiagnostic, Result};
use std::path::{Path, PathBuf};

/// Repository-relative target and host-compiler configuration.
pub const TARGETS_FILE: &str = "aros-targets.toml";

/// Load every configured target from the repository SSOT.
pub fn load_target_profiles() -> Result<Vec<aros_common::TargetProfile>> {
    aros_common::TargetProfile::load_from_file(Path::new(TARGETS_FILE)).into_diagnostic()
}

/// Finds the repository root from a directory inside an AROS-NG checkout.
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
        "Could not find an AROS-NG checkout above '{}'. Run this command from the repository or one of its subdirectories.",
        start.display()
    );
}

pub fn find_root() -> Result<PathBuf> {
    let current_dir = std::env::current_dir()
        .map_err(|error| miette::miette!("Could not determine the current directory: {error}"))?;
    find_root_from(&current_dir)
}

fn is_repo_root(path: &Path) -> bool {
    path.join("CMakeLists.txt").is_file()
        && path.join(TARGETS_FILE).is_file()
        && path.join("tools/aros-tools/Cargo.toml").is_file()
}

#[cfg(test)]
mod tests {
    use super::find_root_from;

    #[test]
    fn finds_checkout_root_from_a_nested_directory() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let root = temp.path().join("AROS-NG");
        std::fs::create_dir_all(root.join("tools/aros-tools/crates")).expect("checkout layout");
        std::fs::write(root.join("CMakeLists.txt"), "").expect("cmake marker");
        std::fs::write(root.join("aros-targets.toml"), "").expect("target marker");
        std::fs::write(root.join("tools/aros-tools/Cargo.toml"), "").expect("cargo marker");

        let nested = root.join("tools/aros-tools/crates");
        assert_eq!(
            find_root_from(&nested).expect("root"),
            root.canonicalize().unwrap()
        );
    }
}
