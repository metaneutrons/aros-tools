//! Stable diagnostic construction for the fetch engine.

use aros_common::{CommitState, Diagnostic, DiagnosticCode, DiagnosticContext, DiagnosticStage};

use crate::contract::FetchRequest;
use crate::FetchFailure;

pub(super) fn context(request: &FetchRequest, output: Option<String>) -> DiagnosticContext {
    DiagnosticContext {
        mode: Some(if request.offline { "offline" } else { "online" }.into()),
        target: Some(request.destination.display().to_string()),
        output: output.or_else(|| Some(request.archive.clone())),
        ..DiagnosticContext::default()
    }
}

pub(super) fn contract_failure(message: impl Into<String>) -> FetchFailure {
    failure(
        DiagnosticCode::FetchContract,
        DiagnosticStage::FetchContract,
        message,
    )
}

pub(super) fn cache_failure(message: impl Into<String>) -> FetchFailure {
    failure(
        DiagnosticCode::FetchCache,
        DiagnosticStage::CacheOperation,
        message,
    )
}

pub(super) fn network_failure(message: impl Into<String>) -> FetchFailure {
    failure(
        DiagnosticCode::FetchNetwork,
        DiagnosticStage::FetchTransfer,
        message,
    )
}

pub(super) fn integrity_failure(name: &str, message: impl Into<String>) -> FetchFailure {
    failure_with_output(
        DiagnosticCode::FetchIntegrity,
        DiagnosticStage::IntegrityValidation,
        name,
        message,
    )
}

pub(super) fn extraction_failure(name: &str, message: impl Into<String>) -> FetchFailure {
    failure_with_output(
        DiagnosticCode::FetchExtraction,
        DiagnosticStage::ArchiveExtraction,
        name,
        message,
    )
}

pub(super) fn patch_failure(name: &str, message: impl Into<String>) -> FetchFailure {
    failure_with_output(
        DiagnosticCode::FetchPatch,
        DiagnosticStage::PatchApplication,
        name,
        message,
    )
}

pub(super) fn publication_failure(name: &str, message: impl Into<String>) -> FetchFailure {
    failure_with_output(
        DiagnosticCode::FetchPublication,
        DiagnosticStage::Publication,
        name,
        message,
    )
}

pub(super) fn publication_failure_with_state(
    name: &str,
    message: impl Into<String>,
    commit_state: CommitState,
) -> FetchFailure {
    FetchFailure::new(
        Diagnostic::error(
            DiagnosticCode::FetchPublication,
            DiagnosticStage::Publication,
            message,
        )
        .with_context(DiagnosticContext {
            output: Some(name.to_owned()),
            commit_state: Some(commit_state),
            ..DiagnosticContext::default()
        }),
    )
}

fn failure_with_output(
    code: DiagnosticCode,
    stage: DiagnosticStage,
    output: &str,
    message: impl Into<String>,
) -> FetchFailure {
    FetchFailure::new(
        Diagnostic::error(code, stage, message).with_context(DiagnosticContext {
            output: Some(output.to_owned()),
            ..DiagnosticContext::default()
        }),
    )
}

fn failure(
    code: DiagnosticCode,
    stage: DiagnosticStage,
    message: impl Into<String>,
) -> FetchFailure {
    FetchFailure::new(Diagnostic::error(code, stage, message))
}

pub(super) trait FailureHint {
    fn with_hint(self, hint: impl Into<String>) -> Self;
}

impl FailureHint for FetchFailure {
    fn with_hint(self, hint: impl Into<String>) -> Self {
        Self::new(self.into_diagnostic().with_hint(hint))
    }
}
