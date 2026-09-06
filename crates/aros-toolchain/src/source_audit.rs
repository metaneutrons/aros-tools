//! Read-only recursive raw-source checks, not an execution snapshot or attestation.
//!
//! Never consult status/diff filters, index stat caches, ignore rules or source
//! scripts. Git plumbing supplies committed raw blobs; descriptor traversal
//! compares their bytes/modes and rejects every undeclared filesystem entry.

pub mod inventory;
pub mod worktree;

use std::time::Instant;

use aros_common::CancellationToken;

use crate::{inspection::Checkout, ContractError};

const MAX_ENTRIES: usize = 200_000;
const MAX_DEPTH: usize = 64;
const MAX_SOURCE_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_BLOB_BYTES: usize = 64 * 1024 * 1024;
const MAX_INVENTORY_BYTES: usize = 32 * 1024 * 1024;

pub struct Budget {
    deadline: Instant,
    entries: usize,
    bytes: u64,
    cancellation: CancellationToken,
}

impl Budget {
    pub(crate) fn new(deadline: Instant) -> Self {
        Self::controlled(deadline, &CancellationToken::default())
    }

    pub(crate) fn controlled(deadline: Instant, cancellation: &CancellationToken) -> Self {
        Self {
            deadline,
            entries: 0,
            bytes: 0,
            cancellation: cancellation.clone(),
        }
    }

    pub(crate) fn check(&self, depth: usize) -> Result<(), ContractError> {
        if self.cancellation.is_cancelled() {
            return Err(ContractError::state(
                "source operation cancelled; partial material is retained",
            ));
        }
        if Instant::now() >= self.deadline {
            return Err(ContractError::prerequisite(
                "recursive source inspection exceeded its shared operation budget",
            ));
        }
        if depth > MAX_DEPTH {
            return Err(ContractError::invalid(
                "source nesting exceeds 64 directories/submodules",
            ));
        }
        Ok(())
    }

    fn entry(&mut self, size: usize) -> Result<(), ContractError> {
        self.entries += 1;
        self.bytes += size as u64;
        if self.entries > MAX_ENTRIES || self.bytes > MAX_SOURCE_BYTES {
            return Err(ContractError::invalid("source inspection exceeds 200000 entries or 8 GiB across the selected roots and submodules"));
        }
        Ok(())
    }
}

pub fn verify(
    checkout: &Checkout<'_>,
    budget: &mut Budget,
    depth: usize,
) -> Result<(), ContractError> {
    visit(checkout, budget, depth, "", &mut |_, _, _| Ok(()))
}

/// Raw committed material only; the visitor may retain partial data on error.
pub type Visitor<'a> = dyn FnMut(&str, &inventory::Entry, &[u8]) -> Result<(), ContractError> + 'a;

pub fn visit(
    checkout: &Checkout<'_>,
    budget: &mut Budget,
    depth: usize,
    prefix: &str,
    visitor: &mut Visitor<'_>,
) -> Result<(), ContractError> {
    budget.check(depth)?;
    let binding = worktree::RootBinding::capture(checkout.root)?;
    let mut entries = inventory::read(checkout, budget)?;
    inventory::verify_index(checkout, &entries)?;
    let mut scoped = |path: &str, entry: &inventory::Entry, bytes: &[u8]| {
        let path = if prefix.is_empty() {
            path.to_owned()
        } else {
            format!("{prefix}/{path}")
        };
        visitor(&path, entry, bytes)
    };
    for (path, entry) in &entries {
        if matches!(entry.mode, "040000" | "160000") {
            budget.check(depth)?;
            scoped(path, entry, &[])?;
        }
    }
    inventory::measure_blobs(checkout, &mut entries, budget, &mut scoped)?;
    worktree::verify(checkout, &entries, budget, depth)?;
    for (path, entry) in &entries {
        if entry.mode == "160000" {
            let child_depth = depth + path.split('/').count();
            budget.check(child_depth)?;
            let root = checkout.root.join(path);
            let child =
                Checkout::submodule(&root, &entry.oid, checkout.deadline, &budget.cancellation)
                    .map_err(|error| error.source_path(path))?;
            let path_prefix = if prefix.is_empty() {
                path.clone()
            } else {
                format!("{prefix}/{path}")
            };
            visit(&child, budget, child_depth, &path_prefix, visitor)
                .map_err(|error| error.source_path(path))?;
        }
    }
    if entries.values().any(|entry| entry.mode == "160000") {
        // Parent contents/membership must still match after inspecting children.
        worktree::verify(checkout, &entries, budget, depth)?;
    }
    binding.recheck(checkout.root)?;
    // Catch HEAD/index changes while the raw filesystem was being inspected.
    inventory::verify_index(checkout, &entries)?;
    checkout.recheck()?;
    tracing::debug!(
        entries = entries.len(),
        depth,
        "recursive raw source inspection complete; execution remains blocked"
    );
    Ok(())
}

fn mismatch(message: &str) -> ContractError {
    ContractError::identity(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::Path, process::Command, time::Duration};

    fn git(root: &Path, args: &[&str]) -> String {
        let mut command = Command::new("git");
        command
            .current_dir(root)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .args([
                "-c",
                "user.name=Audit fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .args(args);
        let result =
            aros_common::run_output_with_timeout(&mut command, 64 * 1024, Duration::from_secs(10))
                .unwrap();
        assert!(result.status.success() && !result.timed_out);
        String::from_utf8(result.stdout.exact_bytes().unwrap().to_vec())
            .unwrap()
            .trim()
            .to_owned()
    }

    #[test]
    fn parent_changes_during_child_material_visits_are_not_lost() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        git(&root, &["init", "-q"]);
        let module = root.join("module");
        fs::create_dir(&module).unwrap();
        git(&module, &["init", "-q"]);
        fs::write(module.join("child"), "child material").unwrap();
        git(&module, &["add", "."]);
        git(&module, &["commit", "-qm", "test: child"]);
        fs::write(root.join("parent"), "parent material").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-qm", "test: parent"]);
        let commit =
            crate::recipe::GitObjectId::try_from(git(&root, &["rev-parse", "HEAD"])).unwrap();
        let tree_id =
            crate::recipe::GitObjectId::try_from(git(&root, &["rev-parse", "HEAD^{tree}"]))
                .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let checkout = Checkout::inspect(&root, (&commit, &tree_id), deadline).unwrap();
        let mut changed = false;
        let result = visit(
            &checkout,
            &mut Budget::new(deadline),
            0,
            "",
            &mut |path, _, _| {
                if path == "module/child" {
                    fs::write(root.join("parent"), "changed during recursion").unwrap();
                    changed = true;
                }
                Ok(())
            },
        );
        assert!(changed);
        assert!(result.is_err());
        assert_eq!(
            fs::read(root.join("parent")).unwrap(),
            b"changed during recursion"
        );
    }
}
