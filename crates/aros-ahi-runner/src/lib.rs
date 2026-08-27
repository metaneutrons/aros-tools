//! Typed execution boundary for the closed AHI capability.

pub mod contract;
pub mod engine;
pub mod observability;
pub mod validation;

use std::fmt;

use aros_common::Diagnostic;

/// One stable diagnostic carried through the runner without text matching.
#[derive(Debug)]
pub struct AhiFailure {
    diagnostic: Box<Diagnostic>,
}

impl AhiFailure {
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

impl fmt::Display for AhiFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.diagnostic.message.fmt(formatter)
    }
}

impl std::error::Error for AhiFailure {}

pub type AhiResult<T> = Result<T, AhiFailure>;
