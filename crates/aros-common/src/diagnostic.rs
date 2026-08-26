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
}

impl fmt::Display for DiagnosticStage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::SourceWalk => "source walk",
            Self::FetchDiscovery => "fetch discovery",
            Self::Parsing => "parsing",
            Self::CapabilityValidation => "capability validation",
            Self::GraphValidation => "graph validation",
            Self::OutputPublication => "output publication",
            Self::Internal => "internal invariant",
        })
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
}
