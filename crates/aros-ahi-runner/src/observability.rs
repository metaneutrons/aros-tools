//! AHI-specific policy over shared AROS tool observability.

use std::path::PathBuf;

use aros_common::{
    render_diagnostics, requested_diagnostic_format as shared_requested_diagnostic_format,
    Diagnostic, DiagnosticCode, DiagnosticContext, DiagnosticSet, DiagnosticStage,
    Logger as SharedLogger, ObservabilityPolicy,
};
pub use aros_common::{DiagnosticFormat, LogFormat, LogLevel};

use crate::{AhiFailure, AhiResult};

const POLICY: ObservabilityPolicy = ObservabilityPolicy {
    log_schema: "aros-ahi-runner-log-v1",
    component: "AHI runner",
    include_invocation: false,
    observability_code: DiagnosticCode::AhiObservability,
    observability_stage: DiagnosticStage::AhiObservability,
    internal_code: DiagnosticCode::AhiInternal,
    internal_stage: DiagnosticStage::Internal,
    hint: "select an explicit writable local file or disable AHI runner logging",
};

pub struct Logger {
    inner: SharedLogger,
}

impl Logger {
    /// Open the AHI runner's local structured log.
    ///
    /// # Errors
    ///
    /// Returns a structured observability failure for an invalid or
    /// inaccessible log destination.
    pub fn open(level: LogLevel, format: LogFormat, path: Option<PathBuf>) -> AhiResult<Self> {
        SharedLogger::open(level, format, path, "aros-ahi-runner", POLICY)
            .map(|inner| Self { inner })
            .map_err(|error| AhiFailure::new(error.into_diagnostic()))
    }

    /// Append one AHI execution event.
    ///
    /// # Errors
    ///
    /// Returns a structured observability failure when writing fails.
    pub fn event(
        &mut self,
        level: LogLevel,
        event: &str,
        message: &str,
        context: &DiagnosticContext,
    ) -> AhiResult<()> {
        self.inner
            .event(level, event, message, context)
            .map_err(|error| AhiFailure::new(error.into_diagnostic()))
    }

    /// Append one structured AHI diagnostic.
    ///
    /// # Errors
    ///
    /// Returns a structured observability failure when writing fails.
    pub fn diagnostic(&mut self, diagnostic: &Diagnostic) -> AhiResult<()> {
        self.inner
            .diagnostic(diagnostic)
            .map_err(|error| AhiFailure::new(error.into_diagnostic()))
    }
}

pub fn render(diagnostics: &DiagnosticSet, format: DiagnosticFormat) {
    render_diagnostics(diagnostics, format, POLICY);
}

#[must_use]
pub fn requested_diagnostic_format(arguments: &[std::ffi::OsString]) -> DiagnosticFormat {
    shared_requested_diagnostic_format(arguments, "AROS_AHI_DIAGNOSTIC_FORMAT")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabled_logging_requires_an_explicit_file() {
        let error = Logger::open(LogLevel::Info, LogFormat::Jsonl, None)
            .err()
            .unwrap();
        assert_eq!(error.diagnostic().code, DiagnosticCode::AhiObservability);
    }

    #[test]
    fn jsonl_log_has_a_stable_schema_without_ambient_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("ahi.jsonl");
        let mut logger =
            Logger::open(LogLevel::Info, LogFormat::Jsonl, Some(path.clone())).unwrap();
        logger
            .event(
                LogLevel::Info,
                "contract.validated",
                "AHI contract validated",
                &DiagnosticContext {
                    mode: Some("arm".into()),
                    target: Some("arm-unknown-aros".into()),
                    ..DiagnosticContext::default()
                },
            )
            .unwrap();
        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(value["schema"], POLICY.log_schema);
        assert_eq!(value["event"], "contract.validated");
        assert!(value.get("invocation").is_none());
        assert!(value.get("timestamp").is_none());
        assert!(value.get("hostname").is_none());
    }
}
