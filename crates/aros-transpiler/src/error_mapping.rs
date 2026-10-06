//! Mapping of command errors onto stable diagnostics.

use aros_common::{
    ArosError, Diagnostic, DiagnosticCode, DiagnosticSet, DiagnosticStage, SourceLocation,
};
use std::path::Path;

pub fn diagnostics_error(diagnostics: Vec<Diagnostic>) -> ArosError {
    ArosError::Diagnostics(DiagnosticSet::new(diagnostics))
}

pub fn source_location(path: &Path, root: &Path) -> SourceLocation {
    SourceLocation::new(
        path.strip_prefix(root)
            .unwrap_or(path)
            .display()
            .to_string(),
    )
}

pub fn error_to_diagnostics(error: ArosError) -> DiagnosticSet {
    match error {
        ArosError::Diagnostics(diagnostics) => diagnostics,
        ArosError::TranspilerSyntax { file, message } => DiagnosticSet::single(
            Diagnostic::error(
                DiagnosticCode::SourceParse,
                DiagnosticStage::Parsing,
                message,
            )
            .with_location(SourceLocation::new(file)),
        ),
        ArosError::Configuration { file, message } => DiagnosticSet::single(
            Diagnostic::error(
                DiagnosticCode::InternalInvariant,
                DiagnosticStage::Internal,
                format!("unexpected configuration error in `{file}`: {message}"),
            )
            .with_location(SourceLocation::new(file)),
        ),
        ArosError::ToolchainManifest { file, message } => DiagnosticSet::single(
            Diagnostic::error(
                DiagnosticCode::InternalInvariant,
                DiagnosticStage::Internal,
                format!("unexpected toolchain manifest error in `{file}`: {message}"),
            )
            .with_location(SourceLocation::new(file)),
        ),
        ArosError::MediaProfile { file, message } => DiagnosticSet::single(
            Diagnostic::error(
                DiagnosticCode::InternalInvariant,
                DiagnosticStage::Internal,
                format!("unexpected media profile error in `{file}`: {message}"),
            )
            .with_location(SourceLocation::new(file)),
        ),
        ArosError::DependencyCycle { target } => DiagnosticSet::single(
            Diagnostic::error(
                DiagnosticCode::GraphValidation,
                DiagnosticStage::GraphValidation,
                format!("dependency cycle detected in module: {target}"),
            )
            .with_hint("break or explicitly model the cycle before publishing the graph"),
        ),
        ArosError::Io(error) => DiagnosticSet::single(Diagnostic::error(
            DiagnosticCode::OutputIo,
            DiagnosticStage::OutputPublication,
            error.to_string(),
        )),
        ArosError::Json(error) => DiagnosticSet::single(Diagnostic::error(
            DiagnosticCode::InternalInvariant,
            DiagnosticStage::Internal,
            format!("diagnostic serialization failed: {error}"),
        )),
        ArosError::ToolchainNotFound { binary } => DiagnosticSet::single(Diagnostic::error(
            DiagnosticCode::InternalInvariant,
            DiagnosticStage::Internal,
            format!("unexpected toolchain lookup for `{binary}`"),
        )),
        ArosError::CommandFailed { cmd } => DiagnosticSet::single(Diagnostic::error(
            DiagnosticCode::InternalInvariant,
            DiagnosticStage::Internal,
            format!("unexpected command failure: {cmd}"),
        )),
    }
}

#[cfg(test)]
mod error_mapping_tests {
    use super::*;

    #[test]
    fn unexpected_toolchain_manifest_error_has_a_stable_diagnostic() {
        let diagnostics = error_to_diagnostics(ArosError::ToolchainManifest {
            file: "toolchain-manifest.json".to_owned(),
            message: "unknown field".to_owned(),
        });

        assert_eq!(diagnostics.diagnostics.len(), 1);
        let diagnostic = &diagnostics.diagnostics[0];
        assert_eq!(diagnostic.code, DiagnosticCode::InternalInvariant);
        assert_eq!(diagnostic.stage, DiagnosticStage::Internal);
        assert_eq!(
            diagnostic
                .location
                .as_ref()
                .map(|location| location.path.as_str()),
            Some("toolchain-manifest.json")
        );
        assert!(diagnostic
            .message
            .contains("unexpected toolchain manifest error"));
    }
}
