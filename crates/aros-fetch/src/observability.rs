//! Fetch-specific policy over the shared AROS tool observability layer.

use std::path::PathBuf;

use aros_common::{
    render_diagnostics, requested_diagnostic_format as shared_requested_diagnostic_format,
    Diagnostic, DiagnosticCode, DiagnosticContext, DiagnosticSet, DiagnosticStage,
    Logger as SharedLogger, ObservabilityPolicy,
};
pub use aros_common::{DiagnosticFormat, LogFormat, LogLevel};

use crate::{FetchFailure, FetchResult};

const POLICY: ObservabilityPolicy = ObservabilityPolicy {
    log_schema: "aros-fetch-log-v1",
    component: "AROS fetcher",
    include_invocation: false,
    observability_code: DiagnosticCode::FetchObservability,
    observability_stage: DiagnosticStage::FetchObservability,
    internal_code: DiagnosticCode::FetchInternal,
    internal_stage: DiagnosticStage::Internal,
    hint: "select an explicit writable local file or disable fetch logging",
};

pub struct Logger {
    inner: SharedLogger,
}

impl Logger {
    /// Open the selected local fetch log sink.
    ///
    /// # Errors
    ///
    /// Returns a stable observability diagnostic if logging is enabled without
    /// an explicit path or the selected file cannot be opened.
    pub fn open(level: LogLevel, format: LogFormat, path: Option<PathBuf>) -> FetchResult<Self> {
        SharedLogger::open(level, format, path, "aros-fetch", POLICY)
            .map(|inner| Self { inner })
            .map_err(|error| FetchFailure::new(error.into_diagnostic()))
    }

    /// Append one structured fetch event.
    ///
    /// # Errors
    ///
    /// Returns a stable observability diagnostic when the log write fails.
    pub fn event(
        &mut self,
        level: LogLevel,
        event: &str,
        message: &str,
        context: &DiagnosticContext,
    ) -> FetchResult<()> {
        self.inner
            .event(level, event, message, context)
            .map_err(|error| FetchFailure::new(error.into_diagnostic()))
    }

    /// Append one structured fetch diagnostic.
    ///
    /// # Errors
    ///
    /// Returns a stable observability diagnostic when the log write fails.
    pub fn diagnostic(&mut self, diagnostic: &Diagnostic) -> FetchResult<()> {
        self.inner
            .diagnostic(diagnostic)
            .map_err(|error| FetchFailure::new(error.into_diagnostic()))
    }
}

pub fn render(diagnostics: &DiagnosticSet, format: DiagnosticFormat) {
    render_diagnostics(diagnostics, format, POLICY);
}

#[must_use]
pub fn requested_diagnostic_format(arguments: &[std::ffi::OsString]) -> DiagnosticFormat {
    shared_requested_diagnostic_format(arguments, "AROS_FETCH_DIAGNOSTIC_FORMAT")
}
