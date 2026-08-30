//! Release-specific policy over the shared observability layer.

use std::path::PathBuf;

use aros_common::{
    render_diagnostics, requested_diagnostic_format as shared_requested_diagnostic_format,
    Diagnostic, DiagnosticCode, DiagnosticContext, DiagnosticSet, DiagnosticStage,
    Logger as SharedLogger, ObservabilityPolicy,
};
pub use aros_common::{DiagnosticFormat, LogFormat, LogLevel};

use crate::{ReleaseFailure, ReleaseResult};

const POLICY: ObservabilityPolicy = ObservabilityPolicy {
    log_schema: "aros-release-log-v1",
    component: "AROS release producer",
    include_invocation: false,
    observability_code: DiagnosticCode::ReleaseObservability,
    observability_stage: DiagnosticStage::Observability,
    internal_code: DiagnosticCode::ReleaseInternal,
    internal_stage: DiagnosticStage::Internal,
    hint: "select an explicit writable local file or disable release logging",
};

pub struct Logger {
    inner: SharedLogger,
}

impl Logger {
    /// Open the selected local release log.
    ///
    /// # Errors
    ///
    /// Returns `AP0002` if the opt-in sink cannot be opened.
    pub fn open(level: LogLevel, format: LogFormat, path: Option<PathBuf>) -> ReleaseResult<Self> {
        SharedLogger::open(level, format, path, "aros-release", POLICY)
            .map(|inner| Self { inner })
            .map_err(|error| ReleaseFailure::new(error.into_diagnostic()))
    }

    /// Append one structured release event.
    ///
    /// # Errors
    ///
    /// Returns `AP0002` if the log write fails.
    pub fn event(
        &mut self,
        level: LogLevel,
        event: &str,
        message: &str,
        context: &DiagnosticContext,
    ) -> ReleaseResult<()> {
        self.inner
            .event(level, event, message, context)
            .map_err(|error| ReleaseFailure::new(error.into_diagnostic()))
    }

    /// Append one structured diagnostic.
    ///
    /// # Errors
    ///
    /// Returns `AP0002` if the log write fails.
    pub fn diagnostic(&mut self, diagnostic: &Diagnostic) -> ReleaseResult<()> {
        self.inner
            .diagnostic(diagnostic)
            .map_err(|error| ReleaseFailure::new(error.into_diagnostic()))
    }
}

pub fn render(diagnostics: &DiagnosticSet, format: DiagnosticFormat) {
    render_diagnostics(diagnostics, format, POLICY);
}

#[must_use]
pub fn requested_diagnostic_format(arguments: &[std::ffi::OsString]) -> DiagnosticFormat {
    shared_requested_diagnostic_format(arguments, "AROS_RELEASE_DIAGNOSTIC_FORMAT")
}
