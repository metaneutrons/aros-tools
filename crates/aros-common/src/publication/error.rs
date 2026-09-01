//! Typed publication failure and rollback classification.

use super::PublicationFailureClass;
use std::fmt;
/// Failure from a durable publication operation.
#[derive(Debug)]
pub struct PublicationError {
    message: String,
    rollback_incomplete: bool,
    pub(super) class: PublicationFailureClass,
}

impl PublicationError {
    pub(super) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            rollback_incomplete: false,
            class: PublicationFailureClass::Io,
        }
    }

    pub(super) fn rollback(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            rollback_incomplete: true,
            class: PublicationFailureClass::RecoveryIncomplete,
        }
    }

    pub(super) fn uncertain(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            rollback_incomplete: false,
            class: PublicationFailureClass::CommitStateUncertain,
        }
    }

    /// The commit state is uncertain and automatic rollback did not restore a
    /// namespace whose identity and contents can be proven.
    pub(super) fn commit_state_uncertain_with_incomplete_rollback(
        message: impl Into<String>,
    ) -> Self {
        Self {
            message: format!(
                "{}; automatic rollback is incomplete or unproven; inspect the retained paths before retrying",
                message.into()
            ),
            rollback_incomplete: true,
            class: PublicationFailureClass::CommitStateUncertain,
        }
    }

    pub(super) fn conflict(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            rollback_incomplete: false,
            class: PublicationFailureClass::Conflict,
        }
    }

    /// Whether recovery could not restore every affected target.
    #[must_use]
    pub const fn rollback_incomplete(&self) -> bool {
        self.rollback_incomplete
    }
}

impl fmt::Display for PublicationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for PublicationError {}

/// Detect the stable rollback-incomplete failure state through `io::Error`.
#[must_use]
pub fn is_rollback_incomplete(error: &std::io::Error) -> bool {
    error
        .get_ref()
        .and_then(|source| source.downcast_ref::<PublicationError>())
        .is_some_and(PublicationError::rollback_incomplete)
}

pub(super) fn io_failure(error: PublicationError) -> std::io::Error {
    std::io::Error::other(error)
}
