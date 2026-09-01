//! Streaming extraction resource limits shared by TAR and ZIP readers.

use std::path::Path;

use super::{extraction_failure, FetchResult};

pub(super) const MAX_ARCHIVE_ENTRIES: u64 = 500_000;
pub(super) const MAX_ARCHIVE_ENTRY_BYTES: u64 = 8 * 1024 * 1024 * 1024;
pub(super) const MAX_ARCHIVE_EXPANDED_BYTES: u64 = 32 * 1024 * 1024 * 1024;

#[derive(Default)]
pub(super) struct ExtractionBudget {
    pub(super) entries: u64,
    pub(super) expanded_bytes: u64,
}

impl ExtractionBudget {
    pub(super) fn output_probe_limit(&self) -> u64 {
        MAX_ARCHIVE_ENTRY_BYTES
            .min(MAX_ARCHIVE_EXPANDED_BYTES.saturating_sub(self.expanded_bytes))
            .saturating_add(1)
    }

    pub(super) fn account(&mut self, size: u64, archive: &str, path: &Path) -> FetchResult<()> {
        self.entries = self
            .entries
            .checked_add(1)
            .ok_or_else(|| extraction_failure(archive, "archive entry counter overflowed"))?;
        if self.entries > MAX_ARCHIVE_ENTRIES {
            return Err(extraction_failure(
                archive,
                format!("archive exceeds the {MAX_ARCHIVE_ENTRIES}-entry safety limit"),
            ));
        }
        if size > MAX_ARCHIVE_ENTRY_BYTES {
            return Err(extraction_failure(
                archive,
                format!(
                    "archive entry '{}' exceeds the {MAX_ARCHIVE_ENTRY_BYTES}-byte per-entry safety limit",
                    path.display()
                ),
            ));
        }
        self.expanded_bytes = self.expanded_bytes.checked_add(size).ok_or_else(|| {
            extraction_failure(archive, "archive expanded-size counter overflowed")
        })?;
        if self.expanded_bytes > MAX_ARCHIVE_EXPANDED_BYTES {
            return Err(extraction_failure(
                archive,
                format!(
                    "archive exceeds the {MAX_ARCHIVE_EXPANDED_BYTES}-byte total expanded-size safety limit"
                ),
            ));
        }
        Ok(())
    }
}
