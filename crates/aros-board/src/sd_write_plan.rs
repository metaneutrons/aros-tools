//! Immutable presentation contract for a fail-closed SD write.

use crate::sd_disk::{DiskCandidate, VerifiedImageArtifact};

/// Read-only binding of one verified image to one current disk.
/// The writer derives it again before opening a raw device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WritePlan {
    pub(super) artifact: VerifiedImageArtifact,
    pub(super) candidate: DiskCandidate,
    pub(super) confirmation_token: String,
}

impl WritePlan {
    /// Board-bound image artifact.
    #[must_use]
    pub const fn artifact(&self) -> &VerifiedImageArtifact {
        &self.artifact
    }

    /// Unique current disk candidate.
    #[must_use]
    pub const fn candidate(&self) -> &DiskCandidate {
        &self.candidate
    }

    /// Token for exactly this artifact/candidate pair.
    #[must_use]
    pub fn confirmation_token(&self) -> &str {
        &self.confirmation_token
    }
}
