//! Pure shell-completion generation from the public Clap command model.

use super::{observability, Cli};
use aros_common::{
    render_diagnostics, Diagnostic, DiagnosticCode, DiagnosticFormat, DiagnosticSet,
    DiagnosticStage,
};
use clap::{CommandFactory, ValueEnum};
use miette::Result;
use std::process::ExitCode;

/// Completion syntaxes intentionally supported by the public frontend.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum CompletionShell {
    /// Bash completion syntax.
    Bash,
    /// Zsh completion syntax.
    Zsh,
    /// Fish completion syntax.
    Fish,
}

impl CompletionShell {
    const fn generator(self) -> clap_complete::Shell {
        match self {
            Self::Bash => clap_complete::Shell::Bash,
            Self::Zsh => clap_complete::Shell::Zsh,
            Self::Fish => clap_complete::Shell::Fish,
        }
    }
}

/// Write a completion script without resolving a checkout or opening a log.
pub fn write(shell: CompletionShell) -> Result<()> {
    let mut command = Cli::command();
    let mut generated = Vec::new();
    clap_complete::generate(shell.generator(), &mut command, "aros", &mut generated);
    let generated =
        String::from_utf8(generated).expect("the supported shell completion generators emit UTF-8");
    aros_common::write_stdout(&generated)
        .map_err(|error| miette::miette!("could not write completion script: {error}"))
}

/// Emit one completion script and render a stable diagnostic only if stdout fails.
pub fn emit(shell: CompletionShell, format: DiagnosticFormat) -> ExitCode {
    match write(shell) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            render_diagnostics(
                &DiagnosticSet::single(
                    Diagnostic::error(
                        DiagnosticCode::CliObservability,
                        DiagnosticStage::Observability,
                        format!("could not write shell completion script: {error}"),
                    )
                    .with_hint("check the stdout destination and retry"),
                ),
                format,
                observability::POLICY,
            );
            ExitCode::FAILURE
        }
    }
}
