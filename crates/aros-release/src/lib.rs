//! Deterministic native release archives for the `aros-tools` workspace.

pub mod archive;
pub mod contract;
pub mod ecosystem;
pub mod observability;

use std::fmt;

use aros_common::Diagnostic;

/// One stable, structured release-production failure.
#[derive(Debug)]
pub struct ReleaseFailure {
    diagnostic: Box<Diagnostic>,
}

impl ReleaseFailure {
    #[must_use]
    pub fn new(diagnostic: Diagnostic) -> Self {
        Self {
            diagnostic: Box::new(diagnostic),
        }
    }

    #[must_use]
    pub const fn diagnostic(&self) -> &Diagnostic {
        &self.diagnostic
    }

    #[must_use]
    pub fn into_diagnostic(self) -> Diagnostic {
        *self.diagnostic
    }
}

impl fmt::Display for ReleaseFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.diagnostic.message.fmt(formatter)
    }
}

impl std::error::Error for ReleaseFailure {}

pub type ReleaseResult<T> = Result<T, ReleaseFailure>;
