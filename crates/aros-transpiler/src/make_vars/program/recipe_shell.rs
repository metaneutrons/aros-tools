//! Make recipe control prefixes remain distinct from literal shell syntax.

use super::MakeRecipeProjection;
use crate::make_shell_data::{parse_literal_recipe, LiteralRecipeStatement};

/// Explicit prefix characters only, not GNU Make's inferred recursive-command
/// behavior or permission to execute in a dry-run/touch/question mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MakeRecipePrefixes {
    pub silent: bool,
    pub ignore_errors: bool,
    pub explicit_recursive: bool,
}

/// One logical recipe's inert shell tree and explicit Make control characters.
/// Source exports remain on the original `MakeRecipeProjection`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MakeRecipeShellProjection {
    pub prefixes: MakeRecipePrefixes,
    pub statements: Vec<LiteralRecipeStatement>,
}

impl MakeRecipeProjection {
    /// Separate Make's explicit prefixes from the original expanded shell text.
    ///
    /// Leading spaces/tabs and explicit prefixes are stripped by the same
    /// existing Make-prefix scan as the literal reader. After that layer, the
    /// returned body is borrowed without shell parsing or rewriting. This
    /// is data, not permission to execute it: an owning caller must separately
    /// admit its source, recipe, shell, inputs and effects. In particular, this
    /// method does not extend the literal-shell grammar or recorder policy.
    ///
    /// # Errors
    /// Refuses a recipe exceeding the shared shell-text size bound.
    pub fn original_shell_text(&self) -> Result<(MakeRecipePrefixes, &str), String> {
        if self.expression().len() > 1024 * 1024 {
            return Err("Make recipe shell projection exceeds size limit".into());
        }
        let mut text = self.expression();
        let mut prefixes = MakeRecipePrefixes::default();
        loop {
            text = text.trim_start_matches([' ', '\t']);
            match text.as_bytes().first() {
                Some(b'@') => prefixes.silent = true,
                Some(b'-') => prefixes.ignore_errors = true,
                Some(b'+') => prefixes.explicit_recursive = true,
                _ => break,
            }
            text = &text[1..];
        }
        Ok((prefixes, text))
    }

    /// Parse this one expanded logical recipe without command execution.
    ///
    /// Leading spaces/tabs and explicit @/-/+ prefixes follow Make's command
    /// prefix layer, not shell tokenization. Quoted/escaped characters are not
    /// prefixes. This is not `.ONESHELL` projection or inferred recursive Make
    /// execution; the caller must bind those policies and the actual shell.
    /// All commands and branches remain inert, including state-changing shell
    /// builtins whose effects this data API does not interpret.
    ///
    /// # Errors
    /// Refuses size limits and every unsupported literal-shell construct.
    pub fn parse_literal_shell(&self) -> Result<MakeRecipeShellProjection, String> {
        let (prefixes, text) = self.original_shell_text()?;
        Ok(MakeRecipeShellProjection {
            prefixes,
            statements: parse_literal_recipe(text)?,
        })
    }
}
