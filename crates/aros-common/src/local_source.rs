//! Actual local checkout identity, separate from clean Git release provenance.
//!
//! Git HEAD is a baseline only. The SHA-256 includes the current filesystem
//! inputs, including ignored files, dirty edits and initialized submodules.
//! Only Git metadata and one explicitly selected CLI build subtree are omitted.
//! Capture before a build and remeasure before publishing its outputs. This
//! byte identity is not a producer attestation, build proof or release identity.

use crate::{
    run_output_with_timeout, validate_existing_directory_prefix_nofollow, Sha256Digest,
    TreeTraversalLimits,
};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

const SCHEMA: &str = "aros-local-source-v1";
const MAX_GIT_BYTES: usize = 16 * 1024 * 1024;
const GIT_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_ENTRIES: usize = 300_000;
const MAX_SOURCE_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// Portable measured identity of actual local source inputs. No clean-tree or
/// released-source claim is implied by `head_baseline`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalSourceIdentity {
    pub schema: String,
    pub head_baseline: String,
    pub submodules_sha256: Sha256Digest,
    pub generated_subtree: String,
    pub content_sha256: Sha256Digest,
    pub entry_count: usize,
    pub regular_file_bytes: u64,
}

impl LocalSourceIdentity {
    /// Check the generated namespace before a caller writes or cleans it.
    ///
    /// # Errors
    /// Rejects unsafe/non-root source selection, invalid presets, tracked
    /// output, or invalid/uninitialized Git source selection.
    pub fn validate_build_namespace(root: &Path, preset: &str) -> Result<(), String> {
        validate_preset(preset)?;
        let root = source_root(root)?;
        git_selection(&root, &format!("build/{preset}"))?;
        validate_selected_build_path(&root, preset)?;
        Ok(())
    }

    /// Measure the selected checkout without writing or executing source code.
    ///
    /// The caller must create the CLI build directory before its first capture,
    /// then keep the selected preset unchanged. Sibling builds are not excluded.
    ///
    /// # Errors
    /// Rejects a non-root checkout, unborn/invalid HEAD, uninitialized/conflicted
    /// submodule, tracked generated output, unsafe link/special file, resource
    /// excess, or changing source/Git selection during measurement.
    pub fn capture(root: &Path, preset: &str) -> Result<Self, String> {
        validate_preset(preset)?;
        let root = source_root(root)?;
        let generated_subtree = format!("build/{preset}");
        let before = git_selection(&root, &generated_subtree)?;
        validate_selected_build_path(&root, preset)?;
        let tree = crate::publication::measure_source_tree_content_cas_bounded(
            &root,
            Some(&generated_subtree),
            TreeTraversalLimits::new(MAX_ENTRIES, MAX_SOURCE_BYTES)
                .map_err(|error| error.to_string())?,
        )
        .map_err(|error| format!("cannot measure local source inputs: {error}"))?;
        let after = git_selection(&root, &generated_subtree)?;
        validate_selected_build_path(&root, preset)?;
        if before != after {
            return Err("Git source selection changed during local source measurement".into());
        }
        Ok(Self {
            schema: SCHEMA.into(),
            head_baseline: before.head,
            submodules_sha256: crate::sha256_bytes(&before.submodules),
            generated_subtree,
            content_sha256: tree.payload_digest_excluding(None),
            entry_count: tree.entry_count(),
            regular_file_bytes: tree
                .regular_file_bytes()
                .ok_or("source byte count overflowed")?,
        })
    }

    /// Require the actual current inputs to equal this pre-build snapshot.
    ///
    /// # Errors
    /// Rejects malformed metadata or any source, preset or baseline difference;
    /// capture errors are propagated rather than weakening verification.
    pub fn verify(&self, root: &Path, preset: &str) -> Result<(), String> {
        if self.schema != SCHEMA {
            return Err("unsupported local source identity schema".into());
        }
        let actual = Self::capture(root, preset)?;
        if *self != actual {
            return Err("actual local source inputs differ from the pre-build snapshot".into());
        }
        Ok(())
    }
}

fn validate_selected_build_path(root: &Path, preset: &str) -> Result<(), String> {
    validate_existing_directory_prefix_nofollow(&root.join("build").join(preset))
        .map_err(|error| format!("unsafe selected CLI build path: {error}"))
}

#[derive(PartialEq, Eq)]
struct GitSelection {
    head: String,
    submodules: Vec<u8>,
}

fn source_root(root: &Path) -> Result<PathBuf, String> {
    validate_existing_directory_prefix_nofollow(root).map_err(|error| error.to_string())?;
    let root = root.canonicalize().map_err(|error| error.to_string())?;
    let git_root = git_text(&root, &["rev-parse", "--show-toplevel"])?;
    let git_root = Path::new(&git_root)
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if root != git_root {
        return Err("local source root must be the exact Git checkout root".into());
    }
    Ok(root)
}

fn git_selection(root: &Path, generated_subtree: &str) -> Result<GitSelection, String> {
    let head = git_text(root, &["rev-parse", "--verify", "HEAD^{commit}"])?;
    if !matches!(head.len(), 40 | 64) || !head.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("local source baseline is not a SHA-1 or SHA-256 commit identity".into());
    }
    // Include index and committed-tree checks: a staged deletion is not
    // permission to hide tracked source under the generated namespace.
    if !git_bytes(root, &["ls-files", "-z", "--", generated_subtree])?.is_empty()
        || !git_bytes(
            root,
            &["ls-tree", "-r", "-z", "HEAD", "--", generated_subtree],
        )?
        .is_empty()
    {
        return Err("CLI generated build subtree contains tracked source".into());
    }
    let submodules = git_bytes(root, &["submodule", "status", "--recursive"])?;
    for line in submodules
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        if !matches!(line.first(), Some(b' ' | b'+')) {
            return Err("local source contains an uninitialized or conflicted submodule".into());
        }
    }
    Ok(GitSelection { head, submodules })
}

fn validate_preset(preset: &str) -> Result<(), String> {
    if preset.is_empty()
        || preset.len() > 128
        || !preset
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err("local source preset must be a portable single-component ID".into());
    }
    Ok(())
}

fn git_text(root: &Path, args: &[&str]) -> Result<String, String> {
    let bytes = git_bytes(root, args)?;
    String::from_utf8(bytes)
        .map(|value| value.trim_end_matches(['\r', '\n']).to_owned())
        .map_err(|_| "Git source identity is not UTF-8".into())
}

fn git_bytes(root: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let mut command = Command::new("git");
    // Ambient repository/index overrides must not redirect this measurement.
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            command.env_remove(key);
        }
    }
    command
        .arg("-c")
        .arg("core.fsmonitor=false")
        .arg("-C")
        .arg(root)
        .args(args);
    command.env("GIT_TERMINAL_PROMPT", "0");
    command.env("GIT_NO_REPLACE_OBJECTS", "1");
    let output = run_output_with_timeout(&mut command, MAX_GIT_BYTES, GIT_TIMEOUT)
        .map_err(|error| format!("cannot inspect local source Git identity: {error}"))?;
    if !output.status.success() || output.timed_out {
        return Err(format!(
            "local source Git inspection failed: {}",
            args.join(" ")
        ));
    }
    output
        .stdout
        .exact_bytes()
        .map(<[u8]>::to_vec)
        .ok_or_else(|| "Git source inspection exceeds the capture limit".into())
}

#[cfg(test)]
#[path = "local_source_tests.rs"]
mod tests;
