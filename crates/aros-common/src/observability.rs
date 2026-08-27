//! Shared stable diagnostic rendering and opt-in local logging.

use std::ffi::OsString;
use std::fmt::{self, Write as _};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use clap::ValueEnum;
use serde::Serialize;

use crate::{
    Diagnostic, DiagnosticCode, DiagnosticContext, DiagnosticSet, DiagnosticSeverity,
    DiagnosticStage,
};

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

#[derive(Debug, Clone, Copy)]
pub struct ObservabilityPolicy {
    pub log_schema: &'static str,
    pub component: &'static str,
    pub include_invocation: bool,
    pub observability_code: DiagnosticCode,
    pub observability_stage: DiagnosticStage,
    pub internal_code: DiagnosticCode,
    pub internal_stage: DiagnosticStage,
    pub hint: &'static str,
}

#[derive(Debug)]
pub struct DiagnosticFailure {
    diagnostic: Box<Diagnostic>,
}

impl DiagnosticFailure {
    #[must_use]
    pub fn new(diagnostic: Diagnostic) -> Self {
        Self {
            diagnostic: Box::new(diagnostic),
        }
    }

    #[must_use]
    pub const fn diagnostic(&self) -> &Diagnostic {
        &self.diagnostic
    }

    #[must_use]
    pub fn into_diagnostic(self) -> Diagnostic {
        *self.diagnostic
    }
}

impl fmt::Display for DiagnosticFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.diagnostic.message.fmt(formatter)
    }
}

impl std::error::Error for DiagnosticFailure {}

pub fn render_diagnostics(
    diagnostics: &DiagnosticSet,
    format: DiagnosticFormat,
    policy: ObservabilityPolicy,
) {
    match format {
        DiagnosticFormat::Human => eprint!("{diagnostics}"),
        DiagnosticFormat::Json => match serde_json::to_string(diagnostics) {
            Ok(document) => eprintln!("{document}"),
            Err(error) => eprintln!(
                "error[{}] during {}: cannot serialize diagnostics: {error}",
                policy.internal_code, policy.internal_stage
            ),
        },
    }
}

#[must_use]
pub fn requested_diagnostic_format(
    arguments: &[OsString],
    environment_name: &str,
) -> DiagnosticFormat {
    let mut format = std::env::var_os(environment_name).map_or(DiagnosticFormat::Human, |value| {
        if value == "json" {
            DiagnosticFormat::Json
        } else {
            DiagnosticFormat::Human
        }
    });
    let mut index = 1;
    while index < arguments.len() {
        let text = arguments[index].to_string_lossy();
        if text == "--" {
            break;
        }
        if let Some(value) = text
            .strip_prefix("--diagnostic-format=")
            .or_else(|| text.strip_prefix("--aros-diagnostic-format="))
        {
            format = if value == "json" {
                DiagnosticFormat::Json
            } else {
                DiagnosticFormat::Human
            };
        } else if text == "--diagnostic-format" || text == "--aros-diagnostic-format" {
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

#[derive(Debug, Serialize)]
struct LogRecord<'a> {
    schema: &'static str,
    level: LogLevel,
    event: &'a str,
    message: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    invocation: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    diagnostic_code: Option<DiagnosticCode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    diagnostic_stage: Option<DiagnosticStage>,
    #[serde(flatten)]
    context: &'a DiagnosticContext,
}

pub struct Logger {
    level: LogLevel,
    format: LogFormat,
    path: Option<PathBuf>,
    file: Option<Mutex<File>>,
    invocation: String,
    policy: ObservabilityPolicy,
}

impl Logger {
    pub fn open(
        level: LogLevel,
        format: LogFormat,
        path: Option<PathBuf>,
        invocation: impl Into<String>,
        policy: ObservabilityPolicy,
    ) -> Result<Self, DiagnosticFailure> {
        if level != LogLevel::Off && path.is_none() {
            return Err(observability_failure(
                policy,
                None,
                "local logging is enabled but no log file was specified",
            ));
        }
        let file = if level == LogLevel::Off {
            None
        } else {
            let Some(selected) = path.as_deref() else {
                return Err(observability_failure(
                    policy,
                    None,
                    "enabled logger has no selected local file",
                ));
            };
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(selected)
                .map_err(|error| {
                    observability_failure(
                        policy,
                        Some(selected),
                        format!("cannot open {} log: {error}", policy.component),
                    )
                })?;
            Some(Mutex::new(file))
        };
        Ok(Self {
            level,
            format,
            path,
            file,
            invocation: invocation.into(),
            policy,
        })
    }

    pub fn event(
        &self,
        level: LogLevel,
        event: &str,
        message: &str,
        context: &DiagnosticContext,
    ) -> Result<(), DiagnosticFailure> {
        self.write(level, event, message, context, None)
    }

    pub fn diagnostic(&self, diagnostic: &Diagnostic) -> Result<(), DiagnosticFailure> {
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
        &self,
        level: LogLevel,
        event: &str,
        message: &str,
        context: &DiagnosticContext,
        diagnostic: Option<(DiagnosticCode, DiagnosticStage)>,
    ) -> Result<(), DiagnosticFailure> {
        if self.level == LogLevel::Off || level > self.level {
            return Ok(());
        }
        let Some(file) = &self.file else {
            return Ok(());
        };
        let mut line = match self.format {
            LogFormat::Human => self.human_line(level, event, message, context, diagnostic),
            LogFormat::Jsonl => serde_json::to_string(&LogRecord {
                schema: self.policy.log_schema,
                level,
                event,
                message,
                invocation: self
                    .policy
                    .include_invocation
                    .then_some(self.invocation.as_str()),
                diagnostic_code: diagnostic.map(|value| value.0),
                diagnostic_stage: diagnostic.map(|value| value.1),
                context,
            })
            .map_err(|error| {
                observability_failure(
                    self.policy,
                    self.path.as_deref(),
                    format!(
                        "cannot serialize {} log event: {error}",
                        self.policy.component
                    ),
                )
            })?,
        };
        line.push('\n');
        let mut file = file.lock().map_err(|_| {
            observability_failure(
                self.policy,
                self.path.as_deref(),
                format!("{} log lock is poisoned", self.policy.component),
            )
        })?;
        file.write_all(line.as_bytes())
            .and_then(|()| file.flush())
            .map_err(|error| {
                observability_failure(
                    self.policy,
                    self.path.as_deref(),
                    format!("cannot write {} log: {error}", self.policy.component),
                )
            })
    }

    fn human_line(
        &self,
        level: LogLevel,
        event: &str,
        message: &str,
        context: &DiagnosticContext,
        diagnostic: Option<(DiagnosticCode, DiagnosticStage)>,
    ) -> String {
        let mut line = format!("[{level}] {event}: {message}");
        if self.policy.include_invocation {
            let _ = write!(line, " invocation={}", self.invocation);
        }
        append_context(&mut line, context);
        if let Some((code, stage)) = diagnostic {
            let _ = write!(line, " code={code} stage={stage}");
        }
        line
    }
}

fn append_context(line: &mut String, context: &DiagnosticContext) {
    if let Some(value) = &context.tool {
        let _ = write!(line, " tool={value}");
    }
    if let Some(value) = &context.mode {
        let _ = write!(line, " mode={value}");
    }
    if let Some(value) = &context.target {
        let _ = write!(line, " target={value}");
    }
    if let Some(value) = &context.output {
        let _ = write!(line, " output={value}");
    }
    if let Some(value) = context.argument_index {
        let _ = write!(line, " argument_index={value}");
    }
    if let Some(value) = context.exit_code {
        let _ = write!(line, " exit_code={value}");
    }
    if let Some(value) = context.signal {
        let _ = write!(line, " signal={value}");
    }
    if let Some(value) = &context.log_path {
        let _ = write!(line, " log_path={value}");
    }
}

fn observability_failure(
    policy: ObservabilityPolicy,
    path: Option<&Path>,
    message: impl Into<String>,
) -> DiagnosticFailure {
    DiagnosticFailure::new(
        Diagnostic::error(
            policy.observability_code,
            policy.observability_stage,
            message,
        )
        .with_hint(policy.hint)
        .with_context(DiagnosticContext {
            log_path: path.map(|value| value.display().to_string()),
            ..DiagnosticContext::default()
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const POLICY: ObservabilityPolicy = ObservabilityPolicy {
        log_schema: "aros-test-log-v1",
        component: "test tool",
        include_invocation: true,
        observability_code: DiagnosticCode::CliObservability,
        observability_stage: DiagnosticStage::Observability,
        internal_code: DiagnosticCode::CliInternal,
        internal_stage: DiagnosticStage::Internal,
        hint: "select an explicit writable local file or disable logging",
    };

    #[test]
    fn enabled_logging_requires_an_explicit_file() {
        let error = Logger::open(LogLevel::Info, LogFormat::Jsonl, None, "test", POLICY)
            .err()
            .unwrap();
        assert_eq!(error.diagnostic().code, DiagnosticCode::CliObservability);
    }

    #[test]
    fn jsonl_log_is_stable_and_contains_all_selected_context() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("test.jsonl");
        let logger = Logger::open(
            LogLevel::Info,
            LogFormat::Jsonl,
            Some(path.clone()),
            "test",
            POLICY,
        )
        .unwrap();
        logger
            .event(
                LogLevel::Info,
                "invocation.start",
                "test started",
                &DiagnosticContext {
                    mode: Some("build".into()),
                    target: Some("pc-x86_64".into()),
                    ..DiagnosticContext::default()
                },
            )
            .unwrap();
        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(value["schema"], "aros-test-log-v1");
        assert_eq!(value["mode"], "build");
        assert_eq!(value["target"], "pc-x86_64");
        assert!(value.get("timestamp").is_none());
        assert!(value.get("host").is_none());
    }
}
