//! Producer errors use the existing shared diagnostic envelope and codes.

use aros_common::diagnostic::{Diagnostic, DiagnosticCode, DiagnosticSet, DiagnosticStage};

/// Safe, structured contract failure. Input documents are never copied into it.
#[derive(Debug, thiserror::Error)]
#[error("{diagnostics}")]
pub struct ContractError {
    diagnostics: DiagnosticSet,
}

impl ContractError {
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::new(
            DiagnosticCode::ProducerContract,
            message,
            "use the supported producer contract and regenerate the explicit recipe from reviewed inputs; do not edit a claimed digest to bypass validation",
        )
    }

    pub(crate) fn identity(message: impl Into<String>) -> Self {
        Self::new(
            DiagnosticCode::ProducerIdentity,
            message,
            "compare the selected recipe with its trusted source; restore the intended document or deliberately generate a new recipe, then revalidate all source identities",
        )
    }

    fn new(code: DiagnosticCode, message: impl Into<String>, hint: &str) -> Self {
        Self {
            diagnostics: DiagnosticSet::single(
                Diagnostic::error(code, DiagnosticStage::Configuration, message).with_hint(hint),
            ),
        }
    }

    /// Borrow the normal AROS failure envelope for the frontend's renderer.
    #[must_use]
    pub const fn diagnostics(&self) -> &DiagnosticSet {
        &self.diagnostics
    }
}
