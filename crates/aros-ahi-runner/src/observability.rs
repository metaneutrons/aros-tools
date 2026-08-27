//! Stable diagnostics and opt-in local logging for the AHI runner.

use std::fmt::{self, Write as _};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use aros_common::{
    Diagnostic, DiagnosticCode, DiagnosticContext, DiagnosticSet, DiagnosticSeverity,
    DiagnosticStage,
};
use clap::ValueEnum;
use serde::Serialize;

use crate::{AhiFailure, AhiResult};

const LOG_SCHEMA: &str = "aros-ahi-runner-log-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum DiagnosticFormat {
    Human,
    Json,
}

impl fmt::Display for DiagnosticFormat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Human => "human",
            Self::Json => "json",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Off,
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl fmt::Display for LogLevel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Off => "off",
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum LogFormat {
    Human,
    Jsonl,
}

impl fmt::Display for LogFormat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Human => "human",
            Self::Jsonl => "jsonl",
        })
    }
}

#[derive(Debug, Serialize)]
struct LogRecord<'a> {
    schema: &'static str,
    level: LogLevel,
    event: &'a str,
    message: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    diagnostic_code: Option<DiagnosticCode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    diagnostic_stage: Option<DiagnosticStage>,
    #[serde(flatten)]
    context: &'a DiagnosticContext,
}

#[derive(Debug)]
pub struct Logger {
    level: LogLevel,
    format: LogFormat,
    path: Option<PathBuf>,
    file: Option<File>,
}

impl Logger {
    pub fn open(level: LogLevel, format: LogFormat, path: Option<PathBuf>) -> AhiResult<Self> {
        if level != LogLevel::Off && path.is_none() {
            return Err(observability_failure(
                None,
                "local logging is enabled but no log file was specified",
            ));
        }
        let file = if level == LogLevel::Off {
            None
        } else {
            let Some(selected) = path.as_deref() else {
                return Err(observability_failure(
                    None,
                    "enabled AHI logger has no selected file",
                ));
            };
            Some(
                OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(selected)
                    .map_err(|error| {
                        observability_failure(
                            Some(selected),
                            format!("cannot open AHI runner log: {error}"),
                        )
                    })?,
            )
        };
        Ok(Self {
            level,
            format,
            path,
            file,
        })
    }

    pub fn event(
        &mut self,
        level: LogLevel,
        event: &str,
        message: &str,
        context: &DiagnosticContext,
    ) -> AhiResult<()> {
        self.write(level, event, message, context, None)
    }

    pub fn diagnostic(&mut self, diagnostic: &Diagnostic) -> AhiResult<()> {
        let level = match diagnostic.severity {
            DiagnosticSeverity::Error => LogLevel::Error,
            DiagnosticSeverity::Warning => LogLevel::Warn,
            DiagnosticSeverity::Info => LogLevel::Info,
        };
        let empty = DiagnosticContext::default();
        self.write(
            level,
            "diagnostic",
            &diagnostic.message,
            diagnostic.context.as_ref().unwrap_or(&empty),
            Some((diagnostic.code, diagnostic.stage)),
        )
    }

    fn write(
        &mut self,
        level: LogLevel,
        event: &str,
        message: &str,
        context: &DiagnosticContext,
        diagnostic: Option<(DiagnosticCode, DiagnosticStage)>,
    ) -> AhiResult<()> {
        if self.level == LogLevel::Off || level > self.level {
            return Ok(());
        }
        let line = match self.format {
            LogFormat::Human => {
                let mut rendered = format!("{level:?} {event}: {message}");
                if let Some(mode) = &context.mode {
                    let _ = write!(rendered, " mode={mode}");
                }
                if let Some(target) = &context.target {
                    let _ = write!(rendered, " target={target}");
                }
                if let Some(output) = &context.output {
                    let _ = write!(rendered, " output={output}");
                }
                rendered.push('\n');
                rendered
            }
            LogFormat::Jsonl => {
                let record = LogRecord {
                    schema: LOG_SCHEMA,
                    level,
                    event,
                    message,
                    diagnostic_code: diagnostic.map(|value| value.0),
                    diagnostic_stage: diagnostic.map(|value| value.1),
                    context,
                };
                let mut rendered = serde_json::to_string(&record).map_err(|error| {
                    observability_failure(
                        self.path.as_deref(),
                        format!("cannot serialize AHI runner log event: {error}"),
                    )
                })?;
                rendered.push('\n');
                rendered
            }
        };
        let selected_path = self.path.clone();
        let Some(file) = self.file.as_mut() else {
            return Err(observability_failure(
                selected_path.as_deref(),
                "enabled AHI logger lost its output file",
            ));
        };
        file.write_all(line.as_bytes())
            .and_then(|()| file.flush())
            .map_err(|error| {
                observability_failure(
                    selected_path.as_deref(),
                    format!("cannot write AHI runner log: {error}"),
                )
            })
    }
}

pub fn render(diagnostics: &DiagnosticSet, format: DiagnosticFormat) {
    match format {
        DiagnosticFormat::Human => eprint!("{diagnostics}"),
        DiagnosticFormat::Json => match serde_json::to_string(diagnostics) {
            Ok(document) => eprintln!("{document}"),
            Err(error) => eprintln!(
                "error[AH0901] during internal invariant: cannot serialize diagnostics: {error}"
            ),
        },
    }
}

#[must_use]
pub fn requested_diagnostic_format(arguments: &[std::ffi::OsString]) -> DiagnosticFormat {
    let mut format =
        std::env::var_os("AROS_AHI_DIAGNOSTIC_FORMAT").map_or(DiagnosticFormat::Human, |value| {
            if value == "json" {
                DiagnosticFormat::Json
            } else {
                DiagnosticFormat::Human
            }
        });
    let mut index = 1;
    while index < arguments.len() {
        let text = arguments[index].to_string_lossy();
        if let Some(value) = text.strip_prefix("--diagnostic-format=") {
            format = if value == "json" {
                DiagnosticFormat::Json
            } else {
                DiagnosticFormat::Human
            };
        } else if text == "--diagnostic-format" {
            if arguments
                .get(index + 1)
                .is_some_and(|value| value == "json")
            {
                format = DiagnosticFormat::Json;
            }
            index += 1;
        }
        index += 1;
    }
    format
}

fn observability_failure(path: Option<&Path>, message: impl Into<String>) -> AhiFailure {
    let context = DiagnosticContext {
        log_path: path.map(|value| value.display().to_string()),
        ..DiagnosticContext::default()
    };
    AhiFailure::new(
        Diagnostic::error(
            DiagnosticCode::AhiObservability,
            DiagnosticStage::AhiObservability,
            message,
        )
        .with_hint("select an explicit writable local file or disable AHI runner logging")
        .with_context(context),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabled_logging_requires_an_explicit_file() {
        let error = Logger::open(LogLevel::Info, LogFormat::Jsonl, None).unwrap_err();
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
        assert_eq!(value["schema"], LOG_SCHEMA);
        assert_eq!(value["event"], "contract.validated");
        assert!(value.get("timestamp").is_none());
        assert!(value.get("hostname").is_none());
    }
}
