use thiserror::Error;

use crate::DiagnosticSet;

#[derive(Error, Debug)]
pub enum ArosError {
    #[error("Toolchain binary '{binary}' not found in PATH or standard toolchain directories")]
    ToolchainNotFound { binary: String },

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Transpiler syntax error in '{file}': {message}")]
    TranspilerSyntax { file: String, message: String },

    #[error("{0}")]
    Diagnostics(DiagnosticSet),

    #[error("Dependency cycle detected in module: {target}")]
    DependencyCycle { target: String },

    #[error("Command execution failed: {cmd}")]
    CommandFailed { cmd: String },
}

impl From<DiagnosticSet> for ArosError {
    fn from(diagnostics: DiagnosticSet) -> Self {
        Self::Diagnostics(diagnostics)
    }
}

pub type Result<T> = std::result::Result<T, ArosError>;
