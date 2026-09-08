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
    pub(crate) fn retained_material(mut self) -> Self {
        for diagnostic in &mut self.diagnostics.diagnostics {
            diagnostic.hint = Some("stop and inspect the owned work/output roots; partial or complete source material is retained, never adopted, removed or authorized for execution by this failure".into());
        }
        self
    }

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

    pub(crate) fn sources(message: impl Into<String>) -> Self {
        Self::new(
            DiagnosticCode::ProducerSources,
            message,
            "restore the exact selected source lock and its verified offline cache entries; do not infer a checksum, alter a cache object or fall back to the network",
        )
    }

    pub(crate) fn source_use(message: impl Into<String>) -> Self {
        Self::new(
            DiagnosticCode::ProducerSourceUse,
            message,
            "use only the exact source closure declared by the selected lock; regenerate and review a new lock when the source build reaches a new input",
        )
    }

    pub(crate) fn environment(message: impl Into<String>) -> Self {
        Self::new(
            DiagnosticCode::ProducerEnvironment,
            message,
            "select a supported host interpreter and the exact lock-owned modules; do not install packages or use a host site-packages fallback during the producer run",
        )
    }

    pub(crate) fn configure(message: impl Into<String>) -> Self {
        Self::new(
            DiagnosticCode::ProducerConfigure,
            message,
            "inspect the retained configure work and phase log; do not reuse the compilation root or bypass the selected source contract",
        )
    }

    pub(crate) fn compiler(message: impl Into<String>) -> Self {
        Self::new(
            DiagnosticCode::ProducerCompiler,
            message,
            "inspect the retained compiler work and phase log; do not adopt a partial compiler tree as a successful candidate",
        )
    }

    pub(crate) fn collector(message: impl Into<String>) -> Self {
        Self::new(
            DiagnosticCode::ProducerCollector,
            message,
            "inspect the retained collector work and phase log; restore the exact tools snapshot and verified vendor closure before retrying in fresh roots",
        )
    }

    pub(crate) fn package(message: impl Into<String>) -> Self {
        Self::new(
            DiagnosticCode::ProducerPackage,
            message,
            "inspect the retained package staging and explicitly selected candidate; do not alter the candidate or overwrite an existing package output",
        )
    }

    pub(crate) fn verification(message: impl Into<String>) -> Self {
        Self::new(
            DiagnosticCode::ProducerVerification,
            message,
            "treat the package as untrusted, retain it for inspection and rebuild it from the exact candidate instead of changing a manifest or checksum to fit",
        )
    }

    pub(crate) fn index(message: impl Into<String>) -> Self {
        Self::new(
            DiagnosticCode::ProducerIndex,
            message,
            "treat the release directory as untrusted; retain it for inspection and regenerate every affected package or support file from the exact closed inputs instead of editing a checksum, manifest, or index to fit",
        )
    }

    pub(crate) fn comparison(message: impl Into<String>) -> Self {
        Self::new(
            DiagnosticCode::ProducerComparison,
            message,
            "treat the independent package sets as non-reproducible; retain both for inspection and rebuild from independently prepared closed inputs rather than copying one output over the other",
        )
    }

    pub(crate) fn recovery(message: impl Into<String>) -> Self {
        Self::new(
            DiagnosticCode::ProducerRecovery,
            message,
            "do not repackage, retarget a tag, or alter a release handoff; restore complete immutable evidence and use a fresh qualification when the failed stage was not packaging alone",
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
