//! The CMake build engine, carried inside the tools that generate for it.
//!
//! The engine is the set of CMake modules a transpiled target graph calls into.
//! It describes how any AROS tree is built rather than describing one tree, and
//! the generated graph and these modules are two halves of one contract, so
//! they are versioned together here instead of living in the source tree.
//!
//! Nothing is ever written into a source tree. [`materialize`] places the
//! engine in a build directory, which keeps a pristine upstream checkout
//! pristine and lets the same tool build a tree that has never heard of CMake.

use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

include!(concat!(env!("OUT_DIR"), "/engine_files.rs"));
include!(concat!(env!("OUT_DIR"), "/engine_digest.rs"));

/// Name of the stamp file [`materialize`] writes beside the engine.
///
/// It records the digest of the copy on disk. A run whose digest already matches
/// rewrites nothing, which keeps CMake from re-configuring because a timestamp
/// moved under it.
pub const STAMP_FILE: &str = ".aros-cmake-engine";

/// The contract version the generated target graph must be built for.
///
/// Read from `engine/EngineVersion.cmake` at compile time, so this constant and
/// the engine's own declaration cannot disagree.
#[must_use]
pub const fn api_version() -> u32 {
    API_VERSION
}

/// SHA-256 over every embedded file, path and length included.
///
/// Stable for a given engine and independent of the order a directory walk
/// happens to return.
#[must_use]
pub const fn digest() -> &'static str {
    DIGEST
}

/// How many files the embedded engine holds.
#[must_use]
pub fn file_count() -> usize {
    FILES.len()
}

/// Every embedded path, relative to the engine root, in sorted order.
pub fn paths() -> impl Iterator<Item = &'static str> {
    FILES.iter().map(|(path, _)| *path)
}

/// Reads one embedded file by its relative path.
#[must_use]
pub fn file(path: &str) -> Option<&'static str> {
    FILES
        .iter()
        .find(|(candidate, _)| *candidate == path)
        .map(|(_, contents)| *contents)
}

/// What a call to [`materialize`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    /// Directory the engine now occupies.
    pub root: PathBuf,
    /// Digest of the placed engine, equal to [`digest`].
    pub digest: &'static str,
    /// Files written on this call; zero when the copy on disk already matched.
    pub written: usize,
    /// Files removed because they are not part of this engine.
    pub removed: usize,
    /// Whether the directory already held exactly this engine.
    pub reused: bool,
}

impl fmt::Display for Placement {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.reused {
            write!(
                formatter,
                "engine {} already present at {}",
                &self.digest[..12],
                self.root.display()
            )
        } else {
            write!(
                formatter,
                "engine {} written to {} ({} files, {} removed)",
                &self.digest[..12],
                self.root.display(),
                self.written,
                self.removed
            )
        }
    }
}

/// Places the embedded engine in `root`, making the directory hold exactly it.
///
/// Files that are not part of this engine are removed. That is the point rather
/// than tidiness: a module left behind by an older engine would still be found
/// by `include()`, and would then silently take precedence over nothing at all
/// while the build assumed it was gone.
///
/// A directory whose stamp already records this digest is left untouched and
/// reported as reused.
///
/// # Errors
///
/// Any filesystem failure while reading the stamp, creating directories,
/// writing files or removing foreign entries.
pub fn materialize(root: &Path) -> io::Result<Placement> {
    if let Some(placement) = reuse(root) {
        return Ok(placement);
    }

    fs::create_dir_all(root)?;
    let mut written = 0;
    let mut expected = BTreeSet::new();
    for (relative, contents) in FILES {
        let destination = root.join(relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&destination, contents)?;
        expected.insert(destination);
        written += 1;
    }

    let stamp = root.join(STAMP_FILE);
    expected.insert(stamp.clone());
    let removed = remove_foreign(root, &expected)?;
    fs::write(&stamp, format!("{DIGEST}\n"))?;

    Ok(Placement {
        root: root.to_path_buf(),
        digest: DIGEST,
        written,
        removed,
        reused: false,
    })
}

/// Reports an existing directory that already holds exactly this engine.
///
/// An absent or unreadable stamp is not an error here, it simply means the
/// engine has to be placed, so this answers with an `Option` rather than a
/// `Result`.
fn reuse(root: &Path) -> Option<Placement> {
    let stamp = root.join(STAMP_FILE);
    let recorded = fs::read_to_string(&stamp).ok()?;
    if recorded.trim() != DIGEST {
        return None;
    }
    // The stamp is a claim, not proof. A file deleted by hand after the stamp
    // was written would otherwise go unnoticed, so every path is checked.
    for (relative, _) in FILES {
        if !root.join(relative).is_file() {
            return None;
        }
    }
    Some(Placement {
        root: root.to_path_buf(),
        digest: DIGEST,
        written: 0,
        removed: 0,
        reused: true,
    })
}

/// Removes everything under `directory` that `expected` does not name.
fn remove_foreign(directory: &Path, expected: &BTreeSet<PathBuf>) -> io::Result<usize> {
    let mut removed = 0;
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            removed += remove_foreign(&path, expected)?;
            if fs::read_dir(&path)?.next().is_none() {
                fs::remove_dir(&path)?;
            }
            continue;
        }
        if !expected.contains(&path) {
            fs::remove_file(&path)?;
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests;
