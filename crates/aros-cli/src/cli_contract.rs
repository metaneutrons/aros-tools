//! Shared parser predicates and repository-routing policy for the frontend.

use super::{repo, BoardCommand, Commands, SourceCommand, ToolchainCommands};
use clap::{Args, Subcommand};
use miette::Result;
use std::path::{Path, PathBuf};

/// Parse an opaque removable-media scan identity without accepting a path.
pub fn parse_opaque_scan_id(value: &str) -> std::result::Result<String, String> {
    if value.is_empty() || value.trim() != value || value.contains('/') || value.contains('\\') {
        return Err(
            "expected an opaque scan ID printed by the corresponding `aros board sd` scan command, not a device path"
                .to_string(),
        );
    }
    Ok(value.to_string())
}

/// Parse a strictly positive parallelism value.
pub fn parse_positive_usize(value: &str) -> std::result::Result<usize, String> {
    let parsed = value
        .parse::<usize>()
        .map_err(|_| format!("'{value}' is not a valid positive integer"))?;
    if parsed == 0 {
        return Err("parallel job count must be greater than zero".to_owned());
    }
    Ok(parsed)
}

/// Identifies the local source-checkout context required by a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepositoryRequirement {
    /// The command is independent of an AROS source checkout.
    Global,
    /// Use a checkout when one is discoverable, but remain useful without one.
    Optional,
    /// Refuse to run until an AROS source checkout has been discovered.
    Required,
}

/// Shared board-profile selection accepted by board operations.
#[derive(Args, Clone)]
pub struct BoardProfileSelection {
    /// Local board profile name from ~/.config/aros/boards.toml.
    #[arg(long)]
    pub profile: String,

    /// Board configuration file; overrides AROS_BOARDS_FILE and the default path.
    #[arg(long, value_name = "PATH", env = "AROS_BOARDS_FILE")]
    pub config: Option<PathBuf>,
}

/// Operations over a recorded transpiler-output baseline.
#[derive(Subcommand)]
pub enum GoldenAction {
    /// Run the transpiler twice and store its output as the baseline.
    Capture {
        /// Preset to capture; repeatable. Default: every configured preset.
        #[arg(long = "preset")]
        presets: Vec<String>,
    },

    /// Run the transpiler and compare its output against the baseline.
    Verify {
        /// Preset to check; repeatable. Default: every configured preset.
        #[arg(long = "preset")]
        presets: Vec<String>,

        /// Replace the baseline with this run instead of reporting differences.
        #[arg(long)]
        update: bool,
    },
}

impl Commands {
    /// Return the explicit source-checkout discovery policy for this command.
    pub const fn repository_requirement(&self) -> RepositoryRequirement {
        match self {
            Self::Source { command } => match command {
                SourceCommand::Init { .. } => RepositoryRequirement::Global,
                SourceCommand::Sync { .. } => RepositoryRequirement::Required,
            },
            Self::Ccache { .. }
            | Self::Install { .. }
            | Self::Completions { .. }
            | Self::Toolchain {
                command:
                    ToolchainCommands::Plan(_)
                    | ToolchainCommands::Build(_)
                    | ToolchainCommands::Producer(_)
                    | ToolchainCommands::MetaMakeFetch(_)
                    | ToolchainCommands::Inventory(_)
                    | ToolchainCommands::Import(_)
                    | ToolchainCommands::Register(_)
                    | ToolchainCommands::Remove(_)
                    | ToolchainCommands::Gc(_),
            } => RepositoryRequirement::Global,
            Self::Info { .. } | Self::BuildTools { .. } => RepositoryRequirement::Optional,
            Self::Board { command } => match command {
                BoardCommand::Init { .. }
                | BoardCommand::Scan
                | BoardCommand::Serve { .. }
                | BoardCommand::Sd { .. }
                | BoardCommand::Console { .. } => RepositoryRequirement::Global,
                BoardCommand::Doctor(_)
                | BoardCommand::Build { .. }
                | BoardCommand::Deploy { .. } => RepositoryRequirement::Required,
            },
            Self::Setup { .. }
            | Self::HostCompiler { .. }
            | Self::Toolchain { .. }
            | Self::Build { .. }
            | Self::Clean { .. }
            | Self::Test { .. }
            | Self::Golden { .. } => RepositoryRequirement::Required,
        }
    }
}

/// Resolve an optional or required source checkout according to command policy.
pub fn resolve_repository(
    invocation_directory: &Path,
    requirement: RepositoryRequirement,
) -> Result<Option<PathBuf>> {
    match requirement {
        RepositoryRequirement::Global => Ok(None),
        RepositoryRequirement::Optional => repo::find_root_optional_from(invocation_directory),
        RepositoryRequirement::Required => repo::find_root_from(invocation_directory).map(Some),
    }
}
