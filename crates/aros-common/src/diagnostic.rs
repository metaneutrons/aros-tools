//! Stable, structured diagnostics shared by AROS-NG build tools.

use serde::Serialize;
use std::fmt;

/// Machine-stable diagnostic identifiers.
///
/// The numeric values are part of the command-line interface. Existing values
/// must not be reused when a diagnostic is retired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum DiagnosticCode {
    #[serde(rename = "AT0001")]
    SourceWalk,
    #[serde(rename = "AT0002")]
    FetchDiscovery,
    #[serde(rename = "AT0003")]
    SourceParse,
    #[serde(rename = "AT0004")]
    CapabilityDrift,
    #[serde(rename = "AT0005")]
    OutputIo,
    #[serde(rename = "AT0006")]
    GraphValidation,
    #[serde(rename = "AT0007")]
    InternalInvariant,
    #[serde(rename = "AC0001")]
    CollectorInvocation,
    #[serde(rename = "AC0002")]
    CollectorObservability,
    #[serde(rename = "AC0101")]
    CollectorToolResolution,
    #[serde(rename = "AC0102")]
    CollectorSysroot,
    #[serde(rename = "AC0201")]
    CollectorResponseFile,
    #[serde(rename = "AC0301")]
    CollectorFirstLink,
    #[serde(rename = "AC0302")]
    CollectorSecondLink,
    #[serde(rename = "AC0401")]
    CollectorObjectInspection,
    #[serde(rename = "AC0501")]
    CollectorSetCollection,
    #[serde(rename = "AC0502")]
    CollectorRequiredInput,
    #[serde(rename = "AC0601")]
    CollectorUndefinedSymbols,
    #[serde(rename = "AC0701")]
    CollectorAbi,
    #[serde(rename = "AC0702")]
    CollectorStrip,
    #[serde(rename = "AC0801")]
    CollectorPublication,
    #[serde(rename = "AC0901")]
    CollectorInternal,
    #[serde(rename = "AH0001")]
    AhiInvocation,
    #[serde(rename = "AH0002")]
    AhiObservability,
    #[serde(rename = "AH0101")]
    AhiContractSyntax,
    #[serde(rename = "AH0102")]
    AhiContractIdentity,
    #[serde(rename = "AH0103")]
    AhiContractPath,
    #[serde(rename = "AH0201")]
    AhiInputIntegrity,
    #[serde(rename = "AH0301")]
    AhiStaging,
    #[serde(rename = "AH0401")]
    AhiConfigure,
    #[serde(rename = "AH0501")]
    AhiBuild,
    #[serde(rename = "AH0601")]
    AhiProductValidation,
    #[serde(rename = "AH0701")]
    AhiSourceAudit,
    #[serde(rename = "AH0901")]
    AhiInternal,
    #[serde(rename = "AR0001")]
    CliInvocation,
    #[serde(rename = "AR0002")]
    CliObservability,
    #[serde(rename = "AR0101")]
    CliRepository,
    #[serde(rename = "AR0201")]
    CliConfiguration,
    #[serde(rename = "AR0301")]
    CliToolResolution,
    #[serde(rename = "AR0401")]
    CliToolchain,
    #[serde(rename = "AR0501")]
    CliNetwork,
    #[serde(rename = "AR0601")]
    CliConfigure,
    #[serde(rename = "AR0602")]
    CliBuild,
    #[serde(rename = "AR0701")]
    CliBoot,
    #[serde(rename = "AR0801")]
    CliPi,
    #[serde(rename = "AR0802")]
    CliMediaSafety,
    #[serde(rename = "AR0901")]
    CliPublication,
    #[serde(rename = "AR0999")]
    CliInternal,
}

impl fmt::Display for DiagnosticCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::SourceWalk => "AT0001",
            Self::FetchDiscovery => "AT0002",
            Self::SourceParse => "AT0003",
            Self::CapabilityDrift => "AT0004",
            Self::OutputIo => "AT0005",
            Self::GraphValidation => "AT0006",
            Self::InternalInvariant => "AT0007",
            Self::CollectorInvocation => "AC0001",
            Self::CollectorObservability => "AC0002",
            Self::CollectorToolResolution => "AC0101",
            Self::CollectorSysroot => "AC0102",
            Self::CollectorResponseFile => "AC0201",
            Self::CollectorFirstLink => "AC0301",
            Self::CollectorSecondLink => "AC0302",
            Self::CollectorObjectInspection => "AC0401",
            Self::CollectorSetCollection => "AC0501",
            Self::CollectorRequiredInput => "AC0502",
            Self::CollectorUndefinedSymbols => "AC0601",
            Self::CollectorAbi => "AC0701",
            Self::CollectorStrip => "AC0702",
            Self::CollectorPublication => "AC0801",
            Self::CollectorInternal => "AC0901",
            Self::AhiInvocation => "AH0001",
            Self::AhiObservability => "AH0002",
            Self::AhiContractSyntax => "AH0101",
            Self::AhiContractIdentity => "AH0102",
            Self::AhiContractPath => "AH0103",
            Self::AhiInputIntegrity => "AH0201",
            Self::AhiStaging => "AH0301",
            Self::AhiConfigure => "AH0401",
            Self::AhiBuild => "AH0501",
            Self::AhiProductValidation => "AH0601",
            Self::AhiSourceAudit => "AH0701",
            Self::AhiInternal => "AH0901",
            Self::CliInvocation => "AR0001",
            Self::CliObservability => "AR0002",
            Self::CliRepository => "AR0101",
            Self::CliConfiguration => "AR0201",
            Self::CliToolResolution => "AR0301",
            Self::CliToolchain => "AR0401",
            Self::CliNetwork => "AR0501",
            Self::CliConfigure => "AR0601",
            Self::CliBuild => "AR0602",
            Self::CliBoot => "AR0701",
            Self::CliPi => "AR0801",
            Self::CliMediaSafety => "AR0802",
            Self::CliPublication => "AR0901",
            Self::CliInternal => "AR0999",
        };
        formatter.write_str(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Info,
}

impl fmt::Display for DiagnosticSeverity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticStage {
    SourceWalk,
    FetchDiscovery,
    Parsing,
    CapabilityValidation,
    GraphValidation,
    OutputPublication,
    Internal,
    Invocation,
    Observability,
    ToolResolution,
    SysrootValidation,
    ResponseExpansion,
    FirstLink,
    ObjectInspection,
    SetCollection,
    RequiredInput,
    SecondLink,
    UndefinedAudit,
    AbiMarking,
    Strip,
    Publication,
    AhiInvocation,
    AhiObservability,
    AhiContractParsing,
    AhiContractValidation,
    AhiInputValidation,
    AhiStaging,
    AhiConfigure,
    AhiBuild,
    AhiProductValidation,
    AhiSourceAudit,
    RepositoryDiscovery,
    Configuration,
    NetworkTransfer,
    BuildConfiguration,
    BuildExecution,
    BootValidation,
    PiOperation,
    MediaSafety,
}

impl fmt::Display for DiagnosticStage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::SourceWalk => "source walk",
            Self::FetchDiscovery => "fetch discovery",
            Self::Parsing => "parsing",
            Self::CapabilityValidation => "capability validation",
            Self::GraphValidation => "graph validation",
            Self::OutputPublication | Self::Publication => "output publication",
            Self::Internal => "internal invariant",
            Self::Invocation => "invocation",
            Self::Observability => "observability",
            Self::ToolResolution => "tool resolution",
            Self::SysrootValidation => "sysroot validation",
            Self::ResponseExpansion => "response-file expansion",
            Self::FirstLink => "first link",
            Self::ObjectInspection => "object inspection",
            Self::SetCollection => "set collection",
            Self::RequiredInput => "required collector input",
            Self::SecondLink => "second link",
            Self::UndefinedAudit => "undefined-symbol audit",
            Self::AbiMarking => "AROS ABI marking",
            Self::Strip => "output stripping",
            Self::AhiInvocation => "AHI invocation",
            Self::AhiObservability => "AHI observability",
            Self::AhiContractParsing => "AHI contract parsing",
            Self::AhiContractValidation => "AHI contract validation",
            Self::AhiInputValidation => "AHI input validation",
            Self::AhiStaging => "AHI staging",
            Self::AhiConfigure => "AHI configure",
            Self::AhiBuild => "AHI build",
            Self::AhiProductValidation => "AHI product validation",
            Self::AhiSourceAudit => "AHI source audit",
            Self::RepositoryDiscovery => "repository discovery",
            Self::Configuration => "configuration",
            Self::NetworkTransfer => "network transfer",
            Self::BuildConfiguration => "build configuration",
            Self::BuildExecution => "build execution",
            Self::BootValidation => "boot validation",
            Self::PiOperation => "Raspberry Pi operation",
            Self::MediaSafety => "removable-media safety",
        })
    }
}

/// Optional machine-readable context attached to a diagnostic.
///
/// These fields deliberately exclude timestamps, host names, and environment
/// snapshots so a diagnostic emitted by a deterministic build remains stable.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct DiagnosticContext {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub argument_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_path: Option<String>,
}

impl DiagnosticContext {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct SourceLocation {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<usize>,
}

impl SourceLocation {
    #[must_use]
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            line: None,
            column: None,
        }
    }

    #[must_use]
    pub const fn at(mut self, line: usize, column: Option<usize>) -> Self {
        self.line = Some(line);
        self.column = column;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Diagnostic {
    pub code: DiagnosticCode,
    pub severity: DiagnosticSeverity,
    pub stage: DiagnosticStage,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<SourceLocation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<DiagnosticContext>,
}

impl Diagnostic {
    #[must_use]
    pub fn error(code: DiagnosticCode, stage: DiagnosticStage, message: impl Into<String>) -> Self {
        Self {
            code,
            severity: DiagnosticSeverity::Error,
            stage,
            message: message.into(),
            location: None,
            hint: None,
            context: None,
        }
    }

    #[must_use]
    pub fn warning(
        code: DiagnosticCode,
        stage: DiagnosticStage,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            severity: DiagnosticSeverity::Warning,
            stage,
            message: message.into(),
            location: None,
            hint: None,
            context: None,
        }
    }

    #[must_use]
    pub fn info(code: DiagnosticCode, stage: DiagnosticStage, message: impl Into<String>) -> Self {
        Self {
            code,
            severity: DiagnosticSeverity::Info,
            stage,
            message: message.into(),
            location: None,
            hint: None,
            context: None,
        }
    }

    #[must_use]
    pub fn with_location(mut self, location: SourceLocation) -> Self {
        self.location = Some(location);
        self
    }

    #[must_use]
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    #[must_use]
    pub fn with_context(mut self, context: DiagnosticContext) -> Self {
        if !context.is_empty() {
            self.context = Some(context);
        }
        self
    }
}

/// Versioned diagnostic document used by both human and JSON renderers.
#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticSet {
    pub schema: &'static str,
    pub diagnostics: Vec<Diagnostic>,
}

impl DiagnosticSet {
    pub const SCHEMA: &'static str = "aros-tool-diagnostics-v1";

    #[must_use]
    pub fn new(mut diagnostics: Vec<Diagnostic>) -> Self {
        diagnostics.sort();
        diagnostics.dedup();
        Self {
            schema: Self::SCHEMA,
            diagnostics,
        }
    }

    #[must_use]
    pub fn single(diagnostic: Diagnostic) -> Self {
        Self::new(vec![diagnostic])
    }

    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error)
    }
}

impl fmt::Display for DiagnosticSet {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, diagnostic) in self.diagnostics.iter().enumerate() {
            if index > 0 {
                writeln!(formatter)?;
            }
            writeln!(
                formatter,
                "{}[{}] during {}: {}",
                diagnostic.severity, diagnostic.code, diagnostic.stage, diagnostic.message
            )?;
            if let Some(location) = &diagnostic.location {
                write!(formatter, "  --> {}", location.path)?;
                if let Some(line) = location.line {
                    write!(formatter, ":{line}")?;
                    if let Some(column) = location.column {
                        write!(formatter, ":{column}")?;
                    }
                }
                writeln!(formatter)?;
            }
            if let Some(hint) = &diagnostic.hint {
                writeln!(formatter, "  help: {hint}")?;
            }
            if let Some(context) = &diagnostic.context {
                write!(formatter, "  context:")?;
                if let Some(value) = &context.tool {
                    write!(formatter, " tool={value}")?;
                }
                if let Some(value) = &context.mode {
                    write!(formatter, " mode={value}")?;
                }
                if let Some(value) = &context.target {
                    write!(formatter, " target={value}")?;
                }
                if let Some(value) = &context.output {
                    write!(formatter, " output={value}")?;
                }
                if let Some(value) = context.argument_index {
                    write!(formatter, " argument_index={value}")?;
                }
                if let Some(value) = context.exit_code {
                    write!(formatter, " exit_code={value}")?;
                }
                if let Some(value) = context.signal {
                    write!(formatter, " signal={value}")?;
                }
                if let Some(value) = &context.log_path {
                    write!(formatter, " log_path={value}")?;
                }
                writeln!(formatter)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Diagnostic {
        Diagnostic::error(
            DiagnosticCode::CapabilityDrift,
            DiagnosticStage::CapabilityValidation,
            "the closed capability no longer matches",
        )
        .with_location(SourceLocation::new("compiler/mmakefile.src").at(12, Some(4)))
        .with_hint("update the transpiler capability")
    }

    #[test]
    fn human_output_keeps_code_location_and_hint() {
        let rendered = DiagnosticSet::single(sample()).to_string();
        assert!(rendered.contains("error[AT0004]"));
        assert!(rendered.contains("compiler/mmakefile.src:12:4"));
        assert!(rendered.contains("help: update the transpiler capability"));
    }

    #[test]
    fn json_output_has_a_versioned_schema_and_stable_fields() {
        let value = serde_json::to_value(DiagnosticSet::single(sample())).unwrap();
        assert_eq!(value["schema"], DiagnosticSet::SCHEMA);
        assert_eq!(value["diagnostics"][0]["code"], "AT0004");
        assert_eq!(value["diagnostics"][0]["severity"], "error");
        assert_eq!(value["diagnostics"][0]["stage"], "capability_validation");
    }

    #[test]
    fn diagnostics_are_sorted_and_deduplicated() {
        let diagnostic = sample();
        let set = DiagnosticSet::new(vec![diagnostic.clone(), diagnostic]);
        assert_eq!(set.diagnostics.len(), 1);
        assert!(set.has_errors());
    }

    #[test]
    fn collector_context_is_stable_in_human_and_json_output() {
        let context = DiagnosticContext {
            tool: Some("ld.lld".into()),
            mode: Some("final".into()),
            output: Some("work/output.o".into()),
            exit_code: Some(1),
            ..DiagnosticContext::default()
        };
        let set = DiagnosticSet::single(
            Diagnostic::error(
                DiagnosticCode::CollectorFirstLink,
                DiagnosticStage::FirstLink,
                "the linker failed",
            )
            .with_context(context),
        );

        let human = set.to_string();
        assert!(human.contains("error[AC0301]"));
        assert!(human.contains("tool=ld.lld mode=final output=work/output.o exit_code=1"));

        let json = serde_json::to_value(set).unwrap();
        assert_eq!(json["diagnostics"][0]["code"], "AC0301");
        assert_eq!(json["diagnostics"][0]["context"]["exit_code"], 1);
    }
}
