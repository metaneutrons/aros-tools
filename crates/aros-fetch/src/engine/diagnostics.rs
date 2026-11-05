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

/// Render an error with every wrapped cause.
///
/// `tar` reports a failed entry as "failed to unpack `<path>`" and keeps the
/// operating-system error ("No space left on device", "Permission denied") as
/// its source, which `Display` alone drops.
pub(super) fn error_chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        let part = cause.to_string();
        if !text.contains(&part) {
            text.push_str(": ");
            text.push_str(&part);
        }
        source = cause.source();
    }
    text
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

#[cfg(test)]
mod tests {
    use super::error_chain;
    use std::fmt;

    /// Mimics `tar`: a short description whose cause is only reachable through
    /// `source()`.
    #[derive(Debug)]
    struct Wrapped(std::io::Error);

    impl fmt::Display for Wrapped {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("failed to unpack `tree/file.ll`")
        }
    }

    impl std::error::Error for Wrapped {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.0)
        }
    }

    #[test]
    fn error_chain_names_the_wrapped_cause() {
        let error = Wrapped(std::io::Error::from_raw_os_error(28));
        let rendered = error_chain(&error);
        assert!(
            rendered.starts_with("failed to unpack `tree/file.ll`: "),
            "{rendered}"
        );
        assert!(rendered.contains("os error 28"), "{rendered}");
    }

    #[test]
    fn error_chain_does_not_repeat_a_cause_already_in_the_text() {
        let error = std::io::Error::from_raw_os_error(13);
        let rendered = error_chain(&error);
        assert_eq!(rendered, error.to_string());
    }
}
