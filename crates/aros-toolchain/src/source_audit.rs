//! Read-only recursive raw-source checks, not an execution snapshot or attestation.
//!
//! Never consult status/diff filters, index stat caches, ignore rules or source
//! scripts. Git plumbing supplies committed raw blobs; descriptor traversal
//! compares their bytes/modes and rejects every undeclared filesystem entry.

mod inventory;
mod worktree;

use std::time::Instant;

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
}

impl Budget {
    pub(crate) const fn new(deadline: Instant) -> Self {
        Self {
            deadline,
            entries: 0,
            bytes: 0,
        }
    }

    fn check(&self, depth: usize) -> Result<(), ContractError> {
        if Instant::now() >= self.deadline {
            return Err(ContractError::prerequisite(
                "recursive source inspection exceeded its shared 60-second budget",
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
    budget.check(depth)?;
    let mut entries = inventory::read(checkout, budget)?;
    inventory::verify_index(checkout, &entries)?;
    inventory::measure_blobs(checkout, &mut entries, budget)?;
    worktree::verify(checkout, &entries, budget, depth)?;
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
