//! Literal shell recipe syntax, independent of command names and compiler flags.
//!
//! This is not the read-only shell evaluator. It retains every command in every
//! branch without running it, assigning status or selecting an execution path.

use super::lexer::{Lexer, Token};
use super::{Part, Word, MAX_COMMAND_BYTES, MAX_NESTING, MAX_STATEMENTS};

#[cfg(all(test, unix))]
mod actual_tests;
#[cfg(test)]
mod and_or_tests;
#[cfg(all(test, unix))]
mod pipeline_actual_tests;
#[cfg(all(test, unix))]
mod pipeline_shell_tests;
#[cfg(test)]
mod pipeline_tests;
#[cfg(test)]
mod tests;

/// Ordered shell redirection operation; no descriptor or file is opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecipeRedirectionKind {
    Input,
    Output,
    Append,
    DuplicateInput,
    DuplicateOutput,
}

/// A literal target plus its original ordered descriptor operation.
#[derive(Clone, PartialEq, Eq)]
pub struct RecipeRedirection {
    descriptor: u32,
    kind: RecipeRedirectionKind,
    target: String,
}

impl std::fmt::Debug for RecipeRedirection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecipeRedirection")
            .field("descriptor", &self.descriptor)
            .field("kind", &self.kind)
            .field("target_bytes", &self.target.len())
            .finish()
    }
}

impl RecipeRedirection {
    #[must_use]
    pub const fn descriptor(&self) -> u32 {
        self.descriptor
    }
    #[must_use]
    pub const fn kind(&self) -> RecipeRedirectionKind {
        self.kind
    }
    #[must_use]
    pub fn target(&self) -> &str {
        &self.target
    }
}

/// An inert literal command, not an executable identity or permission to run.
///
/// Words include the command word. Redirections retain their own source order;
/// no dynamic expansion is admitted, so removing them does not alter argv.
#[derive(Clone, PartialEq, Eq)]
pub struct LiteralRecipeCommand {
    words: Vec<String>,
    redirections: Vec<RecipeRedirection>,
}

impl std::fmt::Debug for LiteralRecipeCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiteralRecipeCommand")
            .field("word_count", &self.words.len())
            .field("redirection_count", &self.redirections.len())
            .finish()
    }
}

impl LiteralRecipeCommand {
    #[must_use]
    pub fn words(&self) -> &[String] {
        &self.words
    }
    #[must_use]
    pub fn redirections(&self) -> &[RecipeRedirection] {
        &self.redirections
    }
}

/// A status-dependent AND/OR connector, not an observed command outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecipeAndOrOperator {
    And,
    Or,
}

/// A complete retained list/if tree, including diagnostic and failure branches.
/// Conditions contain commands, not inferred booleans. No branch is discarded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LiteralRecipeStatement {
    Command(LiteralRecipeCommand),
    /// Two or more simple commands, in original pipe order.
    ///
    /// Pipe connections precede each stage's explicit redirections in the
    /// shell. Retaining this structure does not model status, pipefail,
    /// concurrent processes, input bytes or tools that reinterpret stdin.
    Pipeline(Vec<LiteralRecipeCommand>),
    /// POSIX AND/OR lists have equal precedence, evaluated left to right.
    /// Retain every operand without choosing one or inferring its status.
    AndOr {
        first: Box<Self>,
        rest: Vec<(RecipeAndOrOperator, Self)>,
    },
    If {
        condition: Vec<Self>,
        then_body: Vec<Self>,
        else_body: Vec<Self>,
    },
}

/// Parse an already Make-expanded shell recipe without execution or lookup.
///
/// The bounded POSIX subset admits literal words, sequential lists, simple-command
/// pipelines, AND/OR lists, nested if commands and ordered file/descriptor redirections. Quotes and escapes decode
/// to exact argv bytes. All branches are checked. Shell parameters, globbing,
/// substitutions, assignments, loops, compound redirections and
/// unknown control syntax refuse, rather than becoming guessed argv. Make's
/// recipe prefixes must already be handled by its owning recipe layer.
/// This does not bind shell dialect/startup/aliases/functions, native executable
/// identities, command roles, cwd changes, outcomes or recipe scheduling.
///
/// # Errors
/// Returns static syntax/budget refusals, never raw recipe text or values.
pub fn parse_literal_recipe(recipe: &str) -> Result<Vec<LiteralRecipeStatement>, String> {
    if recipe.len() > MAX_COMMAND_BYTES
        || recipe
            .chars()
            .any(|ch| ch.is_control() && !matches!(ch, '\t' | '\n'))
    {
        return Err("invalid literal shell recipe bytes or size".into());
    }
    let mut parser = Parser {
        tokens: Lexer::new(recipe).lex_recipe()?,
        cursor: 0,
        statements: 0,
    };
    let result = parser.list(&[], 0)?;
    if parser.cursor != parser.tokens.len() {
        return Err("unexpected literal shell recipe syntax".into());
    }
    Ok(result)
}

struct Parser {
    tokens: Vec<Token>,
    cursor: usize,
    statements: usize,
}

impl Parser {
    fn keyword(&self, value: &str) -> bool {
        matches!(self.tokens.get(self.cursor), Some(Token::Word(word)) if word.keyword(value))
    }

    fn expect(&mut self, value: &str) -> Result<(), String> {
        if !self.keyword(value) {
            return Err("missing literal shell recipe control keyword".into());
        }
        self.cursor += 1;
        Ok(())
    }

    fn list(
        &mut self,
        stops: &[&str],
        depth: usize,
    ) -> Result<Vec<LiteralRecipeStatement>, String> {
        let mut result = Vec::new();
        loop {
            while matches!(self.tokens.get(self.cursor), Some(Token::Newline)) {
                self.cursor += 1;
            }
            if self.cursor == self.tokens.len() || stops.iter().any(|value| self.keyword(value)) {
                return Ok(result);
            }
            self.statements += 1;
            if self.statements > MAX_STATEMENTS {
                return Err("literal shell recipe exceeds statement limit".into());
            }
            result.push(self.and_or(depth)?);
            match self.tokens.get(self.cursor) {
                None => return Ok(result),
                Some(Token::Semicolon | Token::Newline) => {
                    self.cursor += 1;
                }
                _ => return Err("missing literal shell command separator".into()),
            }
        }
    }

    fn and_or(&mut self, depth: usize) -> Result<LiteralRecipeStatement, String> {
        let first = self.compound_or_pipeline(depth)?;
        let mut rest = Vec::new();
        while let Some(operator) = match self.tokens.get(self.cursor) {
            Some(Token::AndIf) => Some(RecipeAndOrOperator::And),
            Some(Token::OrIf) => Some(RecipeAndOrOperator::Or),
            _ => None,
        } {
            self.cursor += 1;
            while matches!(self.tokens.get(self.cursor), Some(Token::Newline)) {
                self.cursor += 1;
            }
            self.statements += 1;
            if self.statements > MAX_STATEMENTS {
                return Err("literal shell recipe exceeds statement limit".into());
            }
            rest.push((operator, self.compound_or_pipeline(depth)?));
        }
        if rest.is_empty() {
            Ok(first)
        } else {
            Ok(LiteralRecipeStatement::AndOr {
                first: Box::new(first),
                rest,
            })
        }
    }

    fn compound_or_pipeline(&mut self, depth: usize) -> Result<LiteralRecipeStatement, String> {
        if !self.keyword("if") {
            return self.pipeline();
        }
        if depth >= MAX_NESTING {
            return Err("literal shell recipe exceeds nesting limit".into());
        }
        self.cursor += 1;
        let condition = self.list(&["then"], depth + 1)?;
        self.expect("then")?;
        let then_body = self.list(&["else", "fi"], depth + 1)?;
        let else_body = if self.keyword("else") {
            self.cursor += 1;
            let body = self.list(&["fi"], depth + 1)?;
            if body.is_empty() {
                return Err("empty literal shell else body".into());
            }
            body
        } else {
            Vec::new()
        };
        self.expect("fi")?;
        if condition.is_empty() || then_body.is_empty() {
            return Err("empty literal shell condition or body".into());
        }
        Ok(LiteralRecipeStatement::If {
            condition,
            then_body,
            else_body,
        })
    }

    fn pipeline(&mut self) -> Result<LiteralRecipeStatement, String> {
        let first = self.command()?;
        if !matches!(self.tokens.get(self.cursor), Some(Token::Pipe)) {
            return Ok(LiteralRecipeStatement::Command(first));
        }
        let mut commands = vec![first];
        while matches!(self.tokens.get(self.cursor), Some(Token::Pipe)) {
            self.cursor += 1;
            // A POSIX pipe permits line breaks/comments before its next stage.
            while matches!(self.tokens.get(self.cursor), Some(Token::Newline)) {
                self.cursor += 1;
            }
            self.statements += 1;
            if self.statements > MAX_STATEMENTS {
                return Err("literal shell recipe exceeds statement limit".into());
            }
            commands.push(self.command()?);
        }
        Ok(LiteralRecipeStatement::Pipeline(commands))
    }

    fn command(&mut self) -> Result<LiteralRecipeCommand, String> {
        let mut result = LiteralRecipeCommand {
            words: Vec::new(),
            redirections: Vec::new(),
        };
        loop {
            match self.tokens.get(self.cursor) {
                Some(Token::Word(word)) => {
                    if result.words.is_empty()
                        && (super::is_control_word(word)
                            || word.keyword("if")
                            || word.keyword("for")
                            || word.keyword("in"))
                    {
                        return Err("unsupported literal shell control command placement".into());
                    }
                    if result.words.is_empty() && assignment(word) {
                        return Err("shell assignment needs effect modeling".into());
                    }
                    result.words.push(literal(word)?);
                    self.cursor += 1;
                }
                Some(Token::Redirection { descriptor, kind }) => {
                    let (descriptor, kind) = (*descriptor, *kind);
                    self.cursor += 1;
                    let Some(Token::Word(word)) = self.tokens.get(self.cursor) else {
                        return Err("missing literal shell redirection target".into());
                    };
                    let target = literal(word)?;
                    if target.is_empty() {
                        return Err("empty literal shell redirection target".into());
                    }
                    if matches!(
                        kind,
                        RecipeRedirectionKind::DuplicateInput
                            | RecipeRedirectionKind::DuplicateOutput
                    ) && target != "-"
                        && (!target.bytes().all(|byte| byte.is_ascii_digit())
                            || target.parse::<u32>().is_err())
                    {
                        return Err("nonliteral shell descriptor duplication".into());
                    }
                    result.redirections.push(RecipeRedirection {
                        descriptor,
                        kind,
                        target,
                    });
                    self.cursor += 1;
                }
                _ => break,
            }
        }
        if result.words.first().is_none_or(String::is_empty) {
            return Err("missing literal shell command word".into());
        }
        Ok(result)
    }
}

fn literal(word: &Word) -> Result<String, String> {
    let mut result = String::new();
    for part in &word.parts {
        match part {
            Part::Literal { text, quoted } => {
                if !quoted && text.contains(['*', '?', '[', ']']) {
                    return Err("shell pathname expansion needs captured context".into());
                }
                result.push_str(text);
            }
            Part::Variable { .. } => {
                return Err("shell parameter needs bound runtime context".into())
            }
        }
    }
    Ok(result)
}

fn assignment(word: &Word) -> bool {
    let mut name = String::new();
    for part in &word.parts {
        match part {
            Part::Literal {
                text,
                quoted: false,
            } => {
                if let Some((prefix, _)) = text.split_once('=') {
                    name.push_str(prefix);
                    return super::valid_identifier(&name);
                }
                name.push_str(text);
            }
            _ => return false,
        }
    }
    false
}
