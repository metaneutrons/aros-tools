//! Join literal command words to independently captured program roles.
//!
//! No basename, language, CPU, compiler flag or PATH search chooses a role.
//! Unknown commands and every branch remain explicit obligations. These data
//! matches neither authorize execution nor prove which conditional branch ran.

use super::{LiteralRecipeCommand, LiteralRecipeStatement};
use aros_common::captured_programs::{CapturedProgramIdentity, CapturedProgramSet};

const MAX_STATEMENTS: usize = 4096;
const MAX_BATCH_STATEMENTS: usize = 262_144;
const MAX_RECIPES: usize = 65_536;
const MAX_DEPTH: usize = 32;
const MAX_STORAGE: usize = 64 * 1024 * 1024;

#[cfg(all(test, unix))]
mod tests;

/// Structural location only, independent of command spelling or execution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecipeProgramStep {
    Statement(usize),
    Condition,
    Then,
    Else,
    PipelineStage(usize),
    AndOrOperand(usize),
}

/// An original command plus an exact, optional selected-role match.
pub struct RecipeCommandBinding<'a> {
    position: Vec<RecipeProgramStep>,
    command: &'a LiteralRecipeCommand,
    program: Option<&'a CapturedProgramIdentity>,
    programs: &'a CapturedProgramSet,
}

impl std::fmt::Debug for RecipeCommandBinding<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecipeCommandBinding")
            .field("position", &self.position)
            .field("selected", &self.program.is_some())
            .finish_non_exhaustive()
    }
}

impl<'a> RecipeCommandBinding<'a> {
    #[must_use]
    pub fn position(&self) -> &[RecipeProgramStep] {
        &self.position
    }
    #[must_use]
    pub const fn command(&self) -> &'a LiteralRecipeCommand {
        self.command
    }
    #[must_use]
    pub const fn program(&self) -> Option<&'a CapturedProgramIdentity> {
        self.program
    }

    /// Bind a program-valued argument at an independently selected argv slot.
    ///
    /// This supports generators that receive auxiliary tools as arguments.
    /// No option name, basename or argument position is inferred. Matching an
    /// argument does not change this command's own role or bind a script's
    /// interpreter/runtime dependencies. The owning transaction still controls
    /// when these selected file identities may be used.
    ///
    /// # Errors
    /// Refuses the command-word slot, absent words/roles and mismatched exact
    /// invocation spellings, including different aliases of identical bytes.
    pub fn argument_role(
        &self,
        word_index: usize,
        role: &str,
    ) -> Result<&'a CapturedProgramIdentity, String> {
        if word_index == 0 {
            return Err("recipe argument role cannot select the command word".into());
        }
        let word = self
            .command
            .words()
            .get(word_index)
            .ok_or("recipe program argument is absent")?;
        let selected = self
            .programs
            .identity_for_role(role)
            .ok_or("selected argument program role is absent")?;
        if selected.invocation_path().as_os_str() != std::ffi::OsStr::new(word) {
            return Err("recipe program argument differs from selected role".into());
        }
        Ok(selected)
    }
}

/// Complete structural command enumeration, not complete native execution.
#[derive(Debug)]
pub struct RecipeProgramBindings<'a> {
    commands: Vec<RecipeCommandBinding<'a>>,
}

impl<'a> RecipeProgramBindings<'a> {
    #[must_use]
    pub fn commands(&self) -> &[RecipeCommandBinding<'a>] {
        &self.commands
    }

    /// Unmatched commands, including diagnostics/builtins, are never discarded.
    pub fn unresolved(&self) -> impl Iterator<Item = &RecipeCommandBinding<'a>> {
        self.commands
            .iter()
            .filter(|command| command.program.is_none())
    }

    /// Require one structural occurrence of an explicitly selected role.
    ///
    /// This does not select an executed branch, discharge other commands or
    /// identify compiler input/output arguments. Multiple occurrences refuse
    /// even when their argv is identical or their branches are mutually exclusive.
    ///
    /// # Errors
    /// Refuses missing or ambiguous role occurrences without exposing values.
    pub fn unique_role(&self, role: &str) -> Result<&RecipeCommandBinding<'a>, String> {
        let mut selected = self.commands.iter().filter(|command| {
            command
                .program
                .is_some_and(|program| program.role() == role)
        });
        let result = selected
            .next()
            .ok_or("selected recipe program role is absent")?;
        if selected.next().is_some() {
            return Err("selected recipe program role is ambiguous".into());
        }
        Ok(result)
    }
}

/// Match all literal program words in a verified program-set transaction.
///
/// Results borrow both the original AST and independently selected identities.
/// Exact absolute invocation spelling is required, including compiler aliases:
/// identical resolved executable bytes do not make two argv[0] paths equivalent.
/// This does not resolve bare/relative commands, expand environment prefixes,
/// bind interpreter scripts/tool-valued arguments or authenticate runtime state.
/// The owning caller must revalidate again before later use/publication.
///
/// # Errors
/// Refuses program-set drift, malformed/public AST values and aggregate bounds.
/// No partial bindings are returned and no command is executed.
pub fn bind_literal_programs<'a>(
    statements: &'a [LiteralRecipeStatement],
    programs: &'a CapturedProgramSet,
) -> Result<RecipeProgramBindings<'a>, String> {
    bind_literal_program_batches(&[statements], programs)?
        .pop()
        .ok_or_else(|| "recipe program binding result is absent".into())
}

/// Bind a recipe batch with one complete program-set revalidation boundary.
///
/// Per-recipe syntax bounds and shared batch storage/work bounds apply. This
/// avoids rehashing an immutable compiler prefix once for every object recipe.
/// Unknown commands remain in every result, in original recipe/branch order.
///
/// # Errors
/// Refuses any scalar binding error and aggregate recipe/statement/storage
/// limits; no earlier recipe binding is returned after a later refusal.
pub fn bind_literal_program_batches<'a>(
    recipes: &[&'a [LiteralRecipeStatement]],
    programs: &'a CapturedProgramSet,
) -> Result<Vec<RecipeProgramBindings<'a>>, String> {
    with_literal_program_bindings(recipes, programs, Ok)
}

/// Derive a result while every selected program remains in the same transaction.
///
/// Owning adapters may bind additional captured input files inside `operation`
/// without rehashing the program roots separately for every recipe. The callback
/// must not publish intermediate results. No command or argument is executed.
///
/// # Errors
/// Refuses binding/budget errors, callback failure or program drift before the
/// complete callback result can be returned.
pub fn with_literal_program_bindings<'a, T>(
    recipes: &[&'a [LiteralRecipeStatement]],
    programs: &'a CapturedProgramSet,
    operation: impl FnOnce(Vec<RecipeProgramBindings<'a>>) -> Result<T, String>,
) -> Result<T, String> {
    if recipes.len() > MAX_RECIPES {
        return Err("recipe program binding batch exceeds recipe limit".into());
    }
    programs.transaction(|_| {
        let mut walker = Walker {
            programs,
            commands: Vec::new(),
            count: 0,
            recipe_start: 0,
            bytes: recipes.len() * 64,
        };
        let mut results = Vec::with_capacity(recipes.len());
        for statements in recipes {
            walker.recipe_start = walker.count;
            walker.list(statements, &mut Vec::new(), 0)?;
            results.push(RecipeProgramBindings {
                commands: std::mem::take(&mut walker.commands),
            });
        }
        operation(results)
    })
}

struct Walker<'a> {
    programs: &'a CapturedProgramSet,
    commands: Vec<RecipeCommandBinding<'a>>,
    count: usize,
    recipe_start: usize,
    bytes: usize,
}

impl<'a> Walker<'a> {
    fn charge(&mut self, bytes: usize) -> Result<(), String> {
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .filter(|bytes| *bytes <= MAX_STORAGE)
            .ok_or("recipe program bindings exceed storage limit")?;
        Ok(())
    }

    fn list(
        &mut self,
        statements: &'a [LiteralRecipeStatement],
        position: &mut Vec<RecipeProgramStep>,
        depth: usize,
    ) -> Result<(), String> {
        if depth > MAX_DEPTH {
            return Err("recipe program bindings exceed nesting limit".into());
        }
        for (index, statement) in statements.iter().enumerate() {
            self.count += 1;
            if self.count > MAX_BATCH_STATEMENTS || self.count - self.recipe_start > MAX_STATEMENTS
            {
                return Err("recipe program bindings exceed statement limit".into());
            }
            self.charge(128)?;
            position.push(RecipeProgramStep::Statement(index));
            match statement {
                LiteralRecipeStatement::Command(command) => {
                    self.command(command, position)?;
                }
                LiteralRecipeStatement::Pipeline(commands) => {
                    if commands.len() < 2 {
                        return Err("recipe program binding has an incomplete pipeline".into());
                    }
                    for (index, command) in commands.iter().enumerate() {
                        if index != 0 {
                            self.count += 1;
                            if self.count > MAX_BATCH_STATEMENTS
                                || self.count - self.recipe_start > MAX_STATEMENTS
                            {
                                return Err("recipe program bindings exceed statement limit".into());
                            }
                            self.charge(128)?;
                        }
                        position.push(RecipeProgramStep::PipelineStage(index));
                        self.command(command, position)?;
                        position.pop();
                    }
                }
                LiteralRecipeStatement::AndOr { first, rest } => {
                    if rest.is_empty() {
                        return Err("recipe program binding has an incomplete AND/OR list".into());
                    }
                    for (index, operand) in std::iter::once(first.as_ref())
                        .chain(rest.iter().map(|(_, operand)| operand))
                        .enumerate()
                    {
                        position.push(RecipeProgramStep::AndOrOperand(index));
                        self.list(std::slice::from_ref(operand), position, depth + 1)?;
                        position.pop();
                    }
                }
                LiteralRecipeStatement::If {
                    condition,
                    then_body,
                    else_body,
                } => {
                    if condition.is_empty() || then_body.is_empty() {
                        return Err("recipe program binding has an empty control branch".into());
                    }
                    for (step, list) in [
                        (RecipeProgramStep::Condition, condition),
                        (RecipeProgramStep::Then, then_body),
                        (RecipeProgramStep::Else, else_body),
                    ] {
                        position.push(step);
                        self.list(list, position, depth + 1)?;
                        position.pop();
                    }
                }
            }
            position.pop();
        }
        Ok(())
    }

    fn command(
        &mut self,
        command: &'a LiteralRecipeCommand,
        position: &[RecipeProgramStep],
    ) -> Result<(), String> {
        let word = command
            .words()
            .first()
            .filter(|word| !word.is_empty())
            .ok_or("recipe program word is absent")?;
        for value in command.words().iter().map(String::as_str).chain(
            command
                .redirections()
                .iter()
                .map(super::RecipeRedirection::target),
        ) {
            self.charge(
                value
                    .len()
                    .checked_add(64)
                    .ok_or("recipe program binding size overflow")?,
            )?;
        }
        self.charge(std::mem::size_of_val(position))?;
        self.commands.push(RecipeCommandBinding {
            position: position.to_vec(),
            command,
            program: self.programs.identity_for_word(word),
            programs: self.programs,
        });
        Ok(())
    }
}
