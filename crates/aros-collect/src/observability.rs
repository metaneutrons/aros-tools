//! Stable diagnostics and opt-in local logging for both collector front ends.

use std::env;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fmt::Write as _;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use aros_common::{
    Diagnostic, DiagnosticCode, DiagnosticContext, DiagnosticSet, DiagnosticSeverity,
    DiagnosticStage,
};
use serde::Serialize;

const LOG_SCHEMA: &str = "aros-collect-log-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticFormat {
    Human,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    Human,
    Jsonl,
}

#[derive(Debug, Clone)]
pub struct RuntimeOptions {
    pub diagnostic_format: DiagnosticFormat,
    pub log_level: LogLevel,
    pub log_format: LogFormat,
    pub log_file: Option<PathBuf>,
}

impl Default for RuntimeOptions {
    fn default() -> Self {
        Self {
            diagnostic_format: DiagnosticFormat::Human,
            log_level: LogLevel::Off,
            log_format: LogFormat::Human,
            log_file: None,
        }
    }
}

#[must_use]
pub fn requested_diagnostic_format(arguments: &[OsString]) -> DiagnosticFormat {
    let mut format = match env::var_os("AROS_COLLECT_DIAGNOSTIC_FORMAT").as_deref() {
        Some(value) if value == "json" => DiagnosticFormat::Json,
        _ => DiagnosticFormat::Human,
    };
    let mut index = 1;
    while index < arguments.len() {
        let argument = &arguments[index];
        if argument == "--" {
            break;
        }
        let text = argument.to_string_lossy();
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
            format = if arguments
                .get(index + 1)
                .is_some_and(|value| value == "json")
            {
                DiagnosticFormat::Json
            } else {
                DiagnosticFormat::Human
            };
            index += 1;
        }
        index += 1;
    }
    format
}

impl RuntimeOptions {
    pub fn extract(arguments: Vec<OsString>) -> CollectorResult<(Self, Vec<OsString>)> {
        Self::extract_with(arguments, |name| env::var_os(name))
    }

    #[allow(clippy::needless_pass_by_value)]
    fn extract_with(
        arguments: Vec<OsString>,
        environment: impl Fn(&str) -> Option<OsString>,
    ) -> CollectorResult<(Self, Vec<OsString>)> {
        let mut options = Self::default();
        let mut diagnostic_set = false;
        let mut level_set = false;
        let mut format_set = false;
        let mut file_set = false;
        let mut kept = Vec::with_capacity(arguments.len());
        let mut index = 0;
        while index < arguments.len() {
            let argument = &arguments[index];
            if index == 0 {
                kept.push(argument.clone());
                index += 1;
                continue;
            }
            if argument == "--" {
                kept.extend(arguments[index..].iter().cloned());
                break;
            }
            let text = argument.to_string_lossy();
            let (name, joined_value) = text.split_once('=').map_or_else(
                || (text.as_ref(), None),
                |(name, value)| (name, Some(value)),
            );
            let canonical = match name {
                "--diagnostic-format" | "--aros-diagnostic-format" => Some("diagnostic"),
                "--log-level" | "--aros-log-level" => Some("level"),
                "--log-format" | "--aros-log-format" => Some("format"),
                "--log-file" | "--aros-log-file" => Some("file"),
                _ => None,
            };
            let Some(canonical) = canonical else {
                kept.push(argument.clone());
                index += 1;
                continue;
            };

            let value = if let Some(value) = joined_value {
                OsString::from(value)
            } else {
                index += 1;
                arguments.get(index).cloned().ok_or_else(|| {
                    invocation_failure(format!("{name} requires a value"), Some(index - 1))
                })?
            };
            match canonical {
                "diagnostic" => {
                    options.diagnostic_format = parse_diagnostic_format(&value, index)?;
                    diagnostic_set = true;
                }
                "level" => {
                    options.log_level = parse_log_level(&value, index)?;
                    level_set = true;
                }
                "format" => {
                    options.log_format = parse_log_format(&value, index)?;
                    format_set = true;
                }
                "file" => {
                    if value.is_empty() {
                        return Err(invocation_failure(
                            "--log-file must not be empty",
                            Some(index),
                        ));
                    }
                    options.log_file = Some(PathBuf::from(value));
                    file_set = true;
                }
                _ => unreachable!(),
            }
            index += 1;
        }

        if !diagnostic_set {
            if let Some(value) = environment("AROS_COLLECT_DIAGNOSTIC_FORMAT") {
                options.diagnostic_format = parse_diagnostic_format(&value, 0)?;
            }
        }
        if !level_set {
            if let Some(value) = environment("AROS_COLLECT_LOG_LEVEL") {
                options.log_level = parse_log_level(&value, 0)?;
                level_set = true;
            }
        }
        if !format_set {
            if let Some(value) = environment("AROS_COLLECT_LOG_FORMAT") {
                options.log_format = parse_log_format(&value, 0)?;
            }
        }
        if !file_set {
            options.log_file = environment("AROS_COLLECT_LOG_FILE").map(PathBuf::from);
        }
        if options.log_file.is_some() && !level_set {
            options.log_level = LogLevel::Info;
        }
        if options.log_level != LogLevel::Off && options.log_file.is_none() {
            return Err(CollectorFailure::new(
                Diagnostic::error(
                    DiagnosticCode::CollectorObservability,
                    DiagnosticStage::Observability,
                    "local logging is enabled but no log file was specified",
                )
                .with_hint("pass --log-file PATH or set AROS_COLLECT_LOG_FILE"),
            ));
        }
        Ok((options, kept))
    }
}

fn invocation_failure(
    message: impl Into<String>,
    argument_index: Option<usize>,
) -> CollectorFailure {
    CollectorFailure::new(
        Diagnostic::error(
            DiagnosticCode::CollectorInvocation,
            DiagnosticStage::Invocation,
            message,
        )
        .with_context(DiagnosticContext {
            argument_index,
            ..DiagnosticContext::default()
        }),
    )
}

fn value_text<'a>(value: &'a OsStr, index: usize, option: &str) -> CollectorResult<&'a str> {
    value.to_str().ok_or_else(|| {
        invocation_failure(format!("{option} value is not valid UTF-8"), Some(index))
    })
}

fn parse_diagnostic_format(value: &OsStr, index: usize) -> CollectorResult<DiagnosticFormat> {
    match value_text(value, index, "--diagnostic-format")? {
        "human" => Ok(DiagnosticFormat::Human),
        "json" => Ok(DiagnosticFormat::Json),
        other => Err(invocation_failure(
            format!("unsupported diagnostic format {other:?}; expected human or json"),
            Some(index),
        )),
    }
}

fn parse_log_level(value: &OsStr, index: usize) -> CollectorResult<LogLevel> {
    match value_text(value, index, "--log-level")? {
        "off" => Ok(LogLevel::Off),
        "error" => Ok(LogLevel::Error),
        "warn" => Ok(LogLevel::Warn),
        "info" => Ok(LogLevel::Info),
        "debug" => Ok(LogLevel::Debug),
        "trace" => Ok(LogLevel::Trace),
        other => Err(invocation_failure(
            format!(
                "unsupported log level {other:?}; expected off, error, warn, info, debug, or trace"
            ),
            Some(index),
        )),
    }
}

fn parse_log_format(value: &OsStr, index: usize) -> CollectorResult<LogFormat> {
    match value_text(value, index, "--log-format")? {
        "human" => Ok(LogFormat::Human),
        "jsonl" => Ok(LogFormat::Jsonl),
        other => Err(invocation_failure(
            format!("unsupported log format {other:?}; expected human or jsonl"),
            Some(index),
        )),
    }
}

#[derive(Debug)]
pub struct CollectorFailure {
    diagnostic: Box<Diagnostic>,
}

impl CollectorFailure {
    #[must_use]
    #[allow(clippy::missing_const_for_fn)]
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

impl fmt::Display for CollectorFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.diagnostic.message.fmt(formatter)
    }
}

impl std::error::Error for CollectorFailure {}

pub type CollectorResult<T> = Result<T, CollectorFailure>;

#[must_use]
pub fn failure(
    code: DiagnosticCode,
    stage: DiagnosticStage,
    message: impl Into<String>,
    context: DiagnosticContext,
) -> CollectorFailure {
    CollectorFailure::new(Diagnostic::error(code, stage, message).with_context(context))
}

pub fn render(diagnostics: &DiagnosticSet, format: DiagnosticFormat) {
    match format {
        DiagnosticFormat::Human => eprint!("{diagnostics}"),
        DiagnosticFormat::Json => match serde_json::to_string(&diagnostics) {
            Ok(json) => eprintln!("{json}"),
            Err(error) => eprintln!(
                "error[AC0901] during internal invariant: cannot serialize diagnostics: {error}"
            ),
        },
    }
}

#[derive(Serialize)]
struct LogRecord<'a> {
    schema: &'static str,
    level: LogLevel,
    event: &'a str,
    message: &'a str,
    invocation: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mode: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    diagnostic_code: Option<DiagnosticCode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    diagnostic_stage: Option<DiagnosticStage>,
}

pub struct Logger {
    level: LogLevel,
    format: LogFormat,
    path: Option<PathBuf>,
    file: Option<Mutex<File>>,
    invocation: String,
}

impl Logger {
    pub fn open(options: &RuntimeOptions, invocation: impl Into<String>) -> CollectorResult<Self> {
        let file = if options.log_level == LogLevel::Off {
            None
        } else {
            let path = options.log_file.as_deref().expect("validated log path");
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .map_err(|error| {
                    log_failure(path, format!("cannot open collector log: {error}"))
                })?;
            Some(Mutex::new(file))
        };
        Ok(Self {
            level: options.log_level,
            format: options.log_format,
            path: options.log_file.clone(),
            file,
            invocation: invocation.into(),
        })
    }

    pub fn event(
        &self,
        level: LogLevel,
        event: &str,
        message: &str,
        context: &DiagnosticContext,
    ) -> CollectorResult<()> {
        self.write_event(level, event, message, context, None)
    }

    pub fn diagnostic(&self, diagnostic: &Diagnostic) -> CollectorResult<()> {
        let level = match diagnostic.severity {
            DiagnosticSeverity::Error => LogLevel::Error,
            DiagnosticSeverity::Warning => LogLevel::Warn,
            DiagnosticSeverity::Info => LogLevel::Info,
        };
        let empty_context = DiagnosticContext::default();
        self.write_event(
            level,
            "diagnostic",
            &diagnostic.message,
            diagnostic.context.as_ref().unwrap_or(&empty_context),
            Some((diagnostic.code, diagnostic.stage)),
        )
    }

    fn write_event(
        &self,
        level: LogLevel,
        event: &str,
        message: &str,
        context: &DiagnosticContext,
        diagnostic: Option<(DiagnosticCode, DiagnosticStage)>,
    ) -> CollectorResult<()> {
        if self.level == LogLevel::Off || level > self.level {
            return Ok(());
        }
        let Some(file) = &self.file else {
            return Ok(());
        };
        let mut line = match self.format {
            LogFormat::Human => {
                let mut line = format!(
                    "[{level}] {event}: {message} invocation={}",
                    self.invocation
                );
                if let Some(value) = &context.tool {
                    let _ = write!(line, " tool={value}");
                }
                if let Some(value) = &context.mode {
                    let _ = write!(line, " mode={value}");
                }
                if let Some(value) = &context.output {
                    let _ = write!(line, " output={value}");
                }
                if let Some((code, stage)) = diagnostic {
                    let _ = write!(line, " code={code} stage={stage}");
                }
                line
            }
            LogFormat::Jsonl => serde_json::to_string(&LogRecord {
                schema: LOG_SCHEMA,
                level,
                event,
                message,
                invocation: &self.invocation,
                tool: context.tool.as_deref(),
                mode: context.mode.as_deref(),
                output: context.output.as_deref(),
                diagnostic_code: diagnostic.map(|(code, _)| code),
                diagnostic_stage: diagnostic.map(|(_, stage)| stage),
            })
            .map_err(|error| {
                log_failure(
                    self.path.as_deref().expect("enabled log path"),
                    format!("cannot serialize collector log event: {error}"),
                )
            })?,
        };
        line.push('\n');
        let mut file = file.lock().map_err(|_| {
            log_failure(
                self.path.as_deref().expect("enabled log path"),
                "collector log lock is poisoned",
            )
        })?;
        file.write_all(line.as_bytes()).map_err(|error| {
            log_failure(
                self.path.as_deref().expect("enabled log path"),
                format!("cannot write collector log: {error}"),
            )
        })?;
        file.flush().map_err(|error| {
            log_failure(
                self.path.as_deref().expect("enabled log path"),
                format!("cannot flush collector log: {error}"),
            )
        })
    }
}

fn log_failure(path: &Path, message: impl Into<String>) -> CollectorFailure {
    failure(
        DiagnosticCode::CollectorObservability,
        DiagnosticStage::Observability,
        message,
        DiagnosticContext {
            log_path: Some(path.display().to_string()),
            ..DiagnosticContext::default()
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn runtime_options_are_removed_before_collector_dispatch() {
        let (options, kept) = RuntimeOptions::extract_with(
            strings(&[
                "collect-aros",
                "--diagnostic-format=json",
                "--log-level",
                "debug",
                "--log-format=jsonl",
                "--log-file",
                "collector.log",
                "-o",
                "output.o",
            ]),
            |_| None,
        )
        .unwrap();
        assert_eq!(options.diagnostic_format, DiagnosticFormat::Json);
        assert_eq!(options.log_level, LogLevel::Debug);
        assert_eq!(options.log_format, LogFormat::Jsonl);
        assert_eq!(options.log_file, Some(PathBuf::from("collector.log")));
        assert_eq!(kept, strings(&["collect-aros", "-o", "output.o"]));
    }

    #[test]
    fn explicit_separator_protects_linker_arguments() {
        let (options, kept) = RuntimeOptions::extract_with(
            strings(&[
                "aros-collect",
                "--log-file=collector.log",
                "--",
                "--log-level",
                "trace",
            ]),
            |_| None,
        )
        .unwrap();
        assert_eq!(options.log_level, LogLevel::Info);
        assert_eq!(
            kept,
            strings(&["aros-collect", "--", "--log-level", "trace"])
        );
    }

    #[test]
    fn enabled_logging_requires_an_explicit_local_file() {
        let error =
            RuntimeOptions::extract_with(strings(&["aros-collect", "--log-level=info"]), |_| None)
                .unwrap_err();
        assert_eq!(
            error.diagnostic().code,
            DiagnosticCode::CollectorObservability
        );
    }

    #[test]
    fn jsonl_log_has_a_stable_schema_and_no_ambient_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("collector.jsonl");
        let options = RuntimeOptions {
            diagnostic_format: DiagnosticFormat::Human,
            log_level: LogLevel::Info,
            log_format: LogFormat::Jsonl,
            log_file: Some(path.clone()),
        };
        let logger = Logger::open(&options, "collect-aros").unwrap();
        logger
            .event(
                LogLevel::Info,
                "invocation.start",
                "collector invocation started",
                &DiagnosticContext {
                    mode: Some("driver".into()),
                    output: Some("output.o".into()),
                    ..DiagnosticContext::default()
                },
            )
            .unwrap();

        let line = std::fs::read_to_string(path).unwrap();
        let value: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(value["schema"], LOG_SCHEMA);
        assert_eq!(value["event"], "invocation.start");
        assert_eq!(value["mode"], "driver");
        assert!(value.get("timestamp").is_none());
        assert!(value.get("host").is_none());
    }

    #[test]
    fn human_log_is_local_and_line_oriented() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("collector.log");
        let options = RuntimeOptions {
            diagnostic_format: DiagnosticFormat::Human,
            log_level: LogLevel::Info,
            log_format: LogFormat::Human,
            log_file: Some(path.clone()),
        };
        let logger = Logger::open(&options, "aros-collect").unwrap();
        logger
            .event(
                LogLevel::Info,
                "invocation.complete",
                "collector invocation completed",
                &DiagnosticContext {
                    mode: Some("direct".into()),
                    output: Some("output.o".into()),
                    ..DiagnosticContext::default()
                },
            )
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "[info] invocation.complete: collector invocation completed \
             invocation=aros-collect mode=direct output=output.o\n"
        );
    }
}
