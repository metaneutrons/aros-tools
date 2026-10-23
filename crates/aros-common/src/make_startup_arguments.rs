//! Conservative admission of literal Make startup arguments, not execution authority.
//!
//! Preserve the original argument sequence. This deliberately does not evaluate
//! assignments, normalize GNU options, discover input files or emulate GNU's
//! complete option parser. Unsupported forms refuse before a native process is
//! started. Native observations must still establish origin/flavor/value.

use std::{collections::BTreeSet, fmt};

use crate::make_initial_values::validate_command_line_names;

const MAX_ARGUMENTS: usize = 4096;
const MAX_BYTES: usize = 128 * 1024;
const INPUT_CONTROLS: [&str; 4] = ["MAKEFILES", "MAKEFLAGS", "GNUMAKEFLAGS", "MFLAGS"];

/// A closed literal startup grammar with separately retained input identities.
///
/// Neither dry-run nor this type makes arbitrary Make input safe: parsing,
/// remake recipes and recursive recipes can have effects. Bind the selected
/// executable, explicit environment, cwd, input closure, observation phase and
/// receipt independently before launch. Inherited option/preload controls must
/// also pass `make_observer::validate_source_prelude_inputs`.
#[derive(Clone, Eq, PartialEq)]
pub struct LiteralMakeStartupArguments {
    arguments: Vec<String>,
    command_line_names: BTreeSet<String>,
    input_files: Vec<String>,
    old_files: Vec<String>,
    goals: Vec<String>,
}

impl fmt::Debug for LiteralMakeStartupArguments {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LiteralMakeStartupArguments")
            .field("argument_count", &self.arguments.len())
            .field("command_line_count", &self.command_line_names.len())
            .field("input_count", &self.input_files.len())
            .field("old_file_count", &self.old_files.len())
            .field("goal_count", &self.goals.len())
            .finish_non_exhaustive()
    }
}

impl LiteralMakeStartupArguments {
    /// Admit explicitly separated literal options, files, goals and `NAME=value`.
    ///
    /// The admitted grammar is intentionally closed: individual `-r`, `-R`,
    /// `-n`, `-s`, `--no-print-directory`; `-f FILE`, `--file=FILE`, `-o FILE`,
    /// `--old-file=FILE`; literal goals; and ordinary literal `NAME=value`.
    /// Order and duplicates are preserved, including repeated assignments and
    /// input paths. There is no computed final assignment map.
    ///
    /// # Errors
    /// Refuses expressions/controls, shell assignments, option bundles, unknown
    /// options, GNU's `-` stdin shorthand, unsafe names/goals and exhausted bounds. Errors
    /// and Debug never contain caller-supplied argument or value data.
    /// Device/descriptor aliases must be refused by the separate input-identity
    /// binding; this syntax-only type never resolves a supplied path.
    pub fn admit(arguments: &[String]) -> crate::Result<Self> {
        if arguments.is_empty() || arguments.len() > MAX_ARGUMENTS {
            return Err(invalid("argument count exceeds the admitted range"));
        }
        let mut bytes = 0usize;
        for argument in arguments {
            bytes = bytes.saturating_add(argument.len());
            if bytes > MAX_BYTES
                || argument.is_empty()
                || argument.contains('$')
                || argument.chars().any(char::is_control)
            {
                return Err(invalid("argument data is unsupported or exceeds its bound"));
            }
        }
        let mut result = Self {
            arguments: arguments.to_vec(),
            command_line_names: BTreeSet::new(),
            input_files: Vec::new(),
            old_files: Vec::new(),
            goals: Vec::new(),
        };
        let mut cursor = 0;
        while let Some(argument) = arguments.get(cursor) {
            match argument.as_str() {
                "-r" | "-R" | "-n" | "-s" | "--no-print-directory" => {}
                "-f" | "-o" => {
                    let operand = arguments
                        .get(cursor + 1)
                        .ok_or_else(|| invalid("file option operand is missing"))?;
                    result.record_file(operand, argument == "-o")?;
                    cursor += 1;
                }
                _ => {
                    if let Some(file) = argument.strip_prefix("--file=") {
                        result.record_file(file, false)?;
                    } else if let Some(file) = argument.strip_prefix("--old-file=") {
                        result.record_file(file, true)?;
                    } else if argument.starts_with('-') {
                        return Err(invalid("startup option needs explicit effect modeling"));
                    } else if let Some((name, value)) = argument.split_once('=') {
                        // Other assignment operators require a separately
                        // reviewed admission rule; GNU remains the evaluator.
                        validate_command_line_names(&BTreeSet::from([name.to_owned()]))?;
                        if INPUT_CONTROLS.contains(&name) && !value.is_empty() {
                            return Err(invalid(
                                "nonempty startup control needs explicit modeling",
                            ));
                        }
                        result.command_line_names.insert(name.to_owned());
                    } else {
                        if !argument
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || b"/._+-".contains(&byte))
                        {
                            return Err(invalid("goal syntax needs explicit modeling"));
                        }
                        result.goals.push(argument.clone());
                    }
                }
            }
            cursor += 1;
        }
        if result.input_files.is_empty() || result.goals.is_empty() {
            return Err(invalid("explicit input and goal are required"));
        }
        Ok(result)
    }

    fn record_file(&mut self, file: &str, old: bool) -> crate::Result<()> {
        if file.is_empty() || file.starts_with('-') || file.starts_with('~') {
            return Err(invalid("file operand is not a supported literal identity"));
        }
        if old {
            self.old_files.push(file.to_owned());
        } else {
            self.input_files.push(file.to_owned());
        }
        Ok(())
    }

    #[must_use]
    pub fn arguments(&self) -> &[String] {
        &self.arguments
    }

    #[must_use]
    pub const fn command_line_names(&self) -> &BTreeSet<String> {
        &self.command_line_names
    }

    #[must_use]
    pub fn input_files(&self) -> &[String] {
        &self.input_files
    }

    #[must_use]
    pub fn old_files(&self) -> &[String] {
        &self.old_files
    }

    #[must_use]
    pub fn goals(&self) -> &[String] {
        &self.goals
    }
}

fn invalid(message: &str) -> crate::ArosError {
    crate::ArosError::Configuration {
        file: "Make startup arguments".into(),
        message: message.into(),
    }
}

#[cfg(test)]
#[path = "make_startup_arguments_tests.rs"]
mod tests;
