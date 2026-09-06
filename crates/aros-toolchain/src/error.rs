//! Producer errors use the existing shared diagnostic envelope and codes.

use aros_common::diagnostic::{
    Diagnostic, DiagnosticCode, DiagnosticContext, DiagnosticSet, DiagnosticStage, SourceLocation,
};

/// Safe, structured contract failure. Input documents are never copied into it.
#[derive(Debug, thiserror::Error)]
#[error("{diagnostics}")]
pub struct ContractError {
    diagnostics: DiagnosticSet,
}

impl ContractError {
    /// Attach only an already validated committed path, never an untracked name
    /// or raw Git output. Nested repository locations remain root-relative.
    pub(crate) fn source_path(mut self, path: &str) -> Self {
        if let Some(diagnostic) = self.diagnostics.diagnostics.first_mut() {
            let location = diagnostic.location.take().map_or_else(
                || path.to_owned(),
                |location| format!("{path}/{}", location.path),
            );
            diagnostic.location = Some(SourceLocation::new(location));
        }
        self
    }

    pub(crate) fn state(message: impl Into<String>) -> Self {
        Self::new(
            DiagnosticCode::ProducerState,
            message,
            "stop the operation and inspect the explicitly selected work/output roots; any partially reserved directories are retained, never reused or automatically deleted",
        )
    }

    pub(crate) fn context(mut self, context: DiagnosticContext) -> Self {
        if let Some(diagnostic) = self.diagnostics.diagnostics.first_mut() {
            diagnostic.context = Some(context);
        }
        self
    }

    pub(crate) fn input(mut self, label: &'static str) -> Self {
        for diagnostic in &mut self.diagnostics.diagnostics {
            diagnostic.message = format!("{label}: {}", diagnostic.message);
        }
        self
    }

    pub(crate) fn preflight(message: impl Into<String>) -> Self {
        Self::new(
            DiagnosticCode::ProducerPreflight,
            message,
            "select readable regular inputs, disjoint absolute roots and positive explicit resource budgets; no directories have been reserved",
        )
    }

    pub(crate) fn prerequisite(message: impl Into<String>) -> Self {
        Self::new(
            DiagnosticCode::ProducerPrerequisite,
            message,
            "install a trusted Git with --no-lazy-fetch support and prepare the selected objects separately; planning never downloads missing objects",
        )
    }

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
