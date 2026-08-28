//! Native, validated implementation of the AROS `%fetch` contract.

pub mod contract;
pub mod engine;
pub mod observability;

use std::fmt;

use aros_common::Diagnostic;

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
}

impl fmt::Display for FetchFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.diagnostic.message.fmt(formatter)
    }
}

impl std::error::Error for FetchFailure {}

pub type FetchResult<T> = Result<T, FetchFailure>;
