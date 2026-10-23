//! Private prepared invocation data, retained only with a successful stage.
//!
//! This type is not deserializable, portable evidence, or by itself proof that
//! a process ran. The preflight runner owns execution and completed-stage checks.

use super::{digest_json, environment, failure};
use crate::ContractError;
use aros_common::Sha256Digest;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone)]
pub(in crate::compatibility) struct InvocationContext {
    program: PathBuf,
    executable: PathBuf,
    arguments: Vec<String>,
    working_directory: PathBuf,
    environment: environment::ResolvedProbeEnvironment,
    program_sha256: Sha256Digest,
    invocation_sha256: Sha256Digest,
    environment_sha256: Sha256Digest,
}

impl std::fmt::Debug for InvocationContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("InvocationContext")
            .field("argument_count", &self.arguments.len())
            .field("environment_entries", &self.environment.variables.len())
            .finish_non_exhaustive()
    }
}

impl InvocationContext {
    pub(in crate::compatibility) fn new(
        program: &Path,
        arguments: &[String],
        cwd: &Path,
        environment: environment::ResolvedProbeEnvironment,
    ) -> Result<Self, ContractError> {
        environment::validate_variables(&environment.variables)?;
        if arguments.iter().any(|argument| argument.contains('\0')) {
            return Err(failure("Mesa preflight argument contains an OS delimiter"));
        }
        let executable = super::super::checked_executable(program)?;
        let (program_sha256, _) = super::super::measure_executable(&executable)?;
        let working_directory =
            super::super::checked_directory(cwd, "Mesa preflight child working directory")?;
        if working_directory != cwd {
            return Err(failure("Mesa preflight child working directory changed"));
        }
        let invocation_sha256 = digest_json(&serde_json::json!({
            "command": super::super::command_identity(program, arguments)?,
            "working_directory": working_directory,
        }))?;
        let environment_sha256 = environment::identity(&environment)?;
        Ok(Self {
            program: program.into(),
            executable,
            arguments: arguments.to_vec(),
            working_directory,
            environment,
            program_sha256,
            invocation_sha256,
            environment_sha256,
        })
    }

    pub(in crate::compatibility) fn command(&self) -> Command {
        // Preserve the selected spelling/argv[0]; canonical executable paths
        // are for identity checks, not a replacement for invocation semantics.
        let mut command = Command::new(self.program());
        command
            .env_clear()
            .envs(&self.environment.variables)
            .args(self.arguments())
            .current_dir(self.working_directory());
        command
    }

    pub(in crate::compatibility) fn revalidate(&self) -> Result<(), ContractError> {
        let executable = super::super::checked_executable(&self.program)?;
        if executable != self.executable
            || super::super::measure_executable(&executable)?.0 != self.program_sha256
            || super::super::checked_directory(
                &self.working_directory,
                "Mesa preflight child working directory",
            )? != self.working_directory
        {
            return Err(failure(
                "Mesa preflight invocation program or directory changed",
            ));
        }
        Ok(())
    }

    pub(super) const fn environment(&self) -> &environment::ResolvedProbeEnvironment {
        &self.environment
    }

    pub(super) fn program(&self) -> &Path {
        &self.program
    }

    pub(super) fn arguments(&self) -> &[String] {
        &self.arguments
    }

    pub(super) fn working_directory(&self) -> &Path {
        &self.working_directory
    }

    pub(super) const fn program_sha256(&self) -> &Sha256Digest {
        &self.program_sha256
    }

    pub(super) const fn invocation_sha256(&self) -> &Sha256Digest {
        &self.invocation_sha256
    }

    pub(super) const fn environment_sha256(&self) -> &Sha256Digest {
        &self.environment_sha256
    }
}

#[cfg(test)]
#[path = "mesa_preflight_invocation_tests.rs"]
mod tests;
