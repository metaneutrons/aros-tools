use thiserror::Error;

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

    #[error("Dependency cycle detected in module: {target}")]
    DependencyCycle { target: String },

    #[error("Command execution failed: {cmd}")]
    CommandFailed { cmd: String },
}

pub type Result<T> = std::result::Result<T, ArosError>;
