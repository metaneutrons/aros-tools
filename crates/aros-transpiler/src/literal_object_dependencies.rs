//! File ownership for literal objects and their independent dependency context.

use super::LiteralObjectDecl;

/// A source-declared dependency file naming its associated object output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiteralDependencyDecl {
    /// Public Make dependency sidecar under the generated build root.
    pub output: String,
    /// Source dependency arguments, independent from compile arguments.
    pub arguments: Vec<String>,
}

impl LiteralObjectDecl {
    pub(crate) fn produced_outputs(&self) -> impl Iterator<Item = String> + '_ {
        [self.output.clone(), format!("{}.d", self.output)]
            .into_iter()
            .chain(
                self.dependency
                    .iter()
                    .map(|dependency| dependency.output.clone()),
            )
    }
}
