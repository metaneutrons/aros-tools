//! Native, validated implementation of the AROS `%fetch` contract.

pub mod contract;
pub mod engine;
pub mod observability;

use std::fmt;

use aros_common::{CommitState, Diagnostic, DiagnosticContext};

#[derive(Debug)]
pub struct FetchFailure {
    diagnostic: Box<Diagnostic>,
}

impl FetchFailure {
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

    #[must_use]
    pub fn with_commit_state_if_absent(mut self, state: CommitState) -> Self {
        self.diagnostic
            .context
            .get_or_insert_with(DiagnosticContext::default)
            .commit_state
            .get_or_insert(state);
        self
    }
}

impl fmt::Display for FetchFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.diagnostic.message.fmt(formatter)
    }
}

impl std::error::Error for FetchFailure {}

pub type FetchResult<T> = Result<T, FetchFailure>;
