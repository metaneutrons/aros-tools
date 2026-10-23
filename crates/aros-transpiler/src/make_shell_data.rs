//! Bounded read-only POSIX shell data, not a process launcher or general shell.
//!
//! Commands are interpreted against caller-bound filesystem callbacks. No host
//! environment, executable search or write operation is available. The caller
//! must bind the selected shell dialect/startup context independently.

pub mod artifact_binding;
mod parser;
mod printf;
pub mod program_binding;
mod words;

pub use parser::recipe::{
    parse_literal_recipe, LiteralRecipeCommand, LiteralRecipeStatement, RecipeAndOrOperator,
    RecipeRedirection, RecipeRedirectionKind,
};

use parser::{Statement, Word};
use std::{collections::BTreeMap, path::Path};

const MAX_OUTPUT: usize = 1024 * 1024;
const MAX_STEPS: usize = 1024 * 1024;

type DirectoryQuery<'a> = dyn Fn(&str) -> Result<Option<String>, String> + 'a;
type FileQuery<'a> = dyn Fn(&str) -> Result<bool, String> + 'a;
type GlobQuery<'a> = dyn Fn(&str) -> Result<Vec<String>, String> + 'a;
type WorkCharge<'a> = dyn Fn(usize) -> Result<(), String> + 'a;

/// Explicit data capabilities, never implicit ambient filesystem/environment.
pub struct ShellDataContext<'a> {
    pub working_directory: &'a Path,
    pub directory: &'a DirectoryQuery<'a>,
    pub regular_file: &'a FileQuery<'a>,
    pub glob: &'a GlobQuery<'a>,
    /// Independently known default IFS and absent CDPATH/ENV/BASH_ENV controls.
    /// False refuses unquoted splitting and cd rather than guessing behavior.
    /// This alone does not bind the selected shell dialect or builtin identity.
    pub default_shell_controls: bool,
    pub charge: &'a WorkCharge<'a>,
}

/// Captured stdout/status from the data model; Debug never prints raw stdout.
pub struct ShellDataOutput {
    pub stdout: String,
    pub status: u8,
}

impl std::fmt::Debug for ShellDataOutput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ShellDataOutput")
            .field("stdout_bytes", &self.stdout.len())
            .field("status", &self.status)
            .finish()
    }
}

/// Interpret sequential commands, for loops and if branches without execution.
///
/// Admitted intrinsic commands are cd, printf, literal echo, test/[ (-f/-d),
/// true/false and :. Unbound parameters, unsupported commands/operators and
/// exhausted limits refuse, including unsafe syntax in inactive branches.
///
/// # Errors
/// Returns fixed syntax/semantic errors and caller capability/budget refusals.
pub fn evaluate(command: &str, context: &ShellDataContext<'_>) -> Result<ShellDataOutput, String> {
    (context.charge)(command.len())?;
    let statements = parser::parse(command)?;
    validate_commands(&statements)?;
    let cwd = context
        .working_directory
        .to_str()
        .filter(|_| context.working_directory.is_absolute())
        .ok_or("shell data needs an absolute UTF-8 working directory")?
        .to_owned();
    let mut evaluator = Evaluator {
        context,
        cwd,
        variables: BTreeMap::new(),
        stdout: String::new(),
        steps: 0,
    };
    let status = evaluator.list(&statements)?;
    Ok(ShellDataOutput {
        stdout: evaluator.stdout,
        status,
    })
}

/// GNU Make shell-function newline folding, applied only after data evaluation.
#[must_use]
pub fn fold_stdout(stdout: &str) -> String {
    let mut result = String::with_capacity(stdout.len());
    let mut pending = 0;
    let mut characters = stdout.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\r' && characters.peek() == Some(&'\n') {
            characters.next();
            pending += 1;
        } else if character == '\n' {
            pending += 1;
        } else {
            result.extend(std::iter::repeat_n(' ', pending));
            pending = 0;
            result.push(character);
        }
    }
    result
}

fn validate_commands(statements: &[Statement]) -> Result<(), String> {
    for statement in statements {
        match statement {
            Statement::Simple(words) => {
                if !words.first().is_some_and(|word| {
                    ["cd", "printf", "echo", "test", "[", "true", "false", ":"]
                        .iter()
                        .any(|name| word.keyword(name))
                }) {
                    return Err("unsupported shell data command".into());
                }
            }
            Statement::For { name, body, .. } => {
                // These names have shell-owned assignment side effects, not
                // ordinary local-data semantics. Do not silently ignore them.
                if [
                    "IFS", "CDPATH", "PATH", "ENV", "BASH_ENV", "PWD", "OLDPWD", "OPTIND", "PPID",
                ]
                .contains(&name.as_str())
                {
                    return Err("shell-owned loop parameter is unsupported".into());
                }
                validate_commands(body)?;
            }
            Statement::If {
                condition,
                then_body,
                else_body,
            } => {
                validate_commands(condition)?;
                validate_commands(then_body)?;
                validate_commands(else_body)?;
            }
        }
    }
    Ok(())
}

pub(super) struct Evaluator<'a, 'b> {
    context: &'a ShellDataContext<'b>,
    cwd: String,
    variables: BTreeMap<String, String>,
    stdout: String,
    steps: usize,
}

impl Evaluator<'_, '_> {
    fn charge(&mut self, amount: usize) -> Result<(), String> {
        self.steps = self
            .steps
            .checked_add(amount.max(1))
            .filter(|steps| *steps <= MAX_STEPS)
            .ok_or("shell data work exceeds its limit")?;
        (self.context.charge)(amount.max(1))
    }

    fn append(&mut self, value: &str) -> Result<(), String> {
        if self
            .stdout
            .len()
            .checked_add(value.len())
            .is_none_or(|size| size > MAX_OUTPUT)
        {
            return Err("shell data output exceeds its limit".into());
        }
        self.charge(value.len())?;
        self.stdout.push_str(value);
        Ok(())
    }

    fn list(&mut self, statements: &[Statement]) -> Result<u8, String> {
        let mut status = 0;
        for statement in statements {
            self.charge(1)?;
            status = match statement {
                Statement::Simple(words) => self.simple(words)?,
                Statement::For { name, words, body } => {
                    let selected = self.expand_words(words)?;
                    let mut last = 0;
                    for word in selected {
                        self.charge(word.len())?;
                        self.variables.insert(name.clone(), word);
                        last = self.list(body)?;
                    }
                    last
                }
                Statement::If {
                    condition,
                    then_body,
                    else_body,
                } => {
                    if self.list(condition)? == 0 {
                        self.list(then_body)?
                    } else {
                        self.list(else_body)?
                    }
                }
            };
        }
        Ok(status)
    }

    fn simple(&mut self, words: &[Word]) -> Result<u8, String> {
        let arguments = self.expand_words(words)?;
        let (command, args) = arguments.split_first().ok_or("empty shell data command")?;
        match command.as_str() {
            "cd" if args.len() == 1 && !args[0].is_empty() && !args[0].starts_with('-') => {
                if !self.context.default_shell_controls {
                    return Err("shell directory change needs bound shell controls".into());
                }
                let path = self.path(&args[0]);
                if let Some(directory) = (self.context.directory)(&path)? {
                    self.cwd = directory;
                    Ok(0)
                } else {
                    Ok(1)
                }
            }
            "printf" if !args.is_empty() => self.printf(args),
            "echo"
                if !args.first().is_some_and(|arg| arg.starts_with('-'))
                    && !args.iter().any(|arg| arg.contains('\\')) =>
            {
                self.append(&args.join(" "))?;
                self.append("\n")?;
                Ok(0)
            }
            "test" | "[" => self.test(command, args),
            "true" | ":" if args.is_empty() => Ok(0),
            "false" if args.is_empty() => Ok(1),
            _ => Err("unsupported shell data command arguments".into()),
        }
    }

    fn test(&self, command: &str, args: &[String]) -> Result<u8, String> {
        let args = if command == "[" {
            if args.last().map(String::as_str) != Some("]") {
                return Err("shell test lacks its closing bracket".into());
            }
            &args[..args.len() - 1]
        } else {
            args
        };
        let (negative, args) = if args.first().map(String::as_str) == Some("!") {
            (true, &args[1..])
        } else {
            (false, args)
        };
        let value = match args {
            [operator, path] if operator == "-f" => {
                !path.is_empty() && (self.context.regular_file)(&self.path(path))?
            }
            [operator, path] if operator == "-d" => {
                !path.is_empty() && (self.context.directory)(&self.path(path))?.is_some()
            }
            [] => false,
            [value] => !value.is_empty(),
            _ => return Err("unsupported shell test expression".into()),
        };
        Ok(u8::from(!(value ^ negative)))
    }

    fn path(&self, input: &str) -> String {
        if Path::new(input).is_absolute() {
            input.into()
        } else {
            format!("{}/{input}", self.cwd)
        }
    }
}

#[cfg(test)]
mod tests;
