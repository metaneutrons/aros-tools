//! Transpiler policy over the shared AROS diagnostic and local-log contracts.

use aros_common::{
    render_diagnostics, Diagnostic, DiagnosticCode, DiagnosticContext, DiagnosticFormat,
    DiagnosticSet, DiagnosticStage, ObservabilityPolicy,
};
use clap::error::Error as ClapError;

pub const POLICY: ObservabilityPolicy = ObservabilityPolicy {
    log_schema: "aros-transpiler-log-v1",
    component: "transpiler",
    include_invocation: true,
    observability_code: DiagnosticCode::TranspilerObservability,
    observability_stage: DiagnosticStage::Observability,
    internal_code: DiagnosticCode::InternalInvariant,
    internal_stage: DiagnosticStage::Internal,
    hint: "pass --log-file PATH or set AROS_TRANSPILER_LOG_FILE, or disable logging",
};

#[must_use]
pub fn clap_diagnostic(error: &ClapError) -> Diagnostic {
    Diagnostic::error(
        DiagnosticCode::TranspilerInvocation,
        DiagnosticStage::Invocation,
        error.to_string().trim().to_owned(),
    )
    .with_hint("run `aros-transpiler --help` for the complete invocation contract")
    .with_context(DiagnosticContext::default())
}

pub fn render(diagnostics: &DiagnosticSet, format: DiagnosticFormat) {
    render_diagnostics(diagnostics, format, POLICY);
}
