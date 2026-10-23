//! Closed child-environment policies for compatibility phases.

use std::collections::BTreeMap;

use aros_common::{sha256_bytes, Sha256Digest};
use serde::{Deserialize, Serialize};

use super::HostToolClosure;
use crate::ContractError;

const MAX_PROBE_ENVIRONMENT_ENTRIES: usize = 64;
const MAX_PROBE_ENVIRONMENT_BYTES: usize = 64 * 1024;
const POISONED_PATH: &str = "/nonexistent";

/// Closed child-environment policy for one compatibility phase.
#[derive(Debug, Clone)]
pub enum CompatibilityEnvironment {
    /// No child may resolve a command through `PATH`.
    ///
    /// The map must contain exactly `PATH=/nonexistent` plus explicitly
    /// selected variables, so standalone C/C++ probes use absolute tool paths.
    Poisoned {
        /// Explicit child variables, including the required poisoned `PATH`.
        variables: BTreeMap<String, String>,
    },
    /// CMake host tools and upstream configure/Make may resolve only measured
    /// closure entries.
    ///
    /// The supplied map must declare `PATH=/nonexistent`. Resolution replaces
    /// that poisoned marker with the revalidated owned closure; callers cannot
    /// supply or append an ambient path value themselves.
    SealedHostTools {
        /// Explicit child variables, including the required poisoned `PATH`.
        variables: BTreeMap<String, String>,
        /// Exact revalidatable host-command closure for this phase.
        host_tools: HostToolClosure,
    },
}

impl CompatibilityEnvironment {
    /// Apply this exact revalidated policy to a child process, removing ambient
    /// environment entries rather than appending the measured search path.
    ///
    /// # Errors
    /// Rejects invalid variables or changed host closure entries before changing
    /// the command. This transports policy; it does not authenticate execution.
    pub fn apply_to(&self, command: &mut std::process::Command) -> Result<(), ContractError> {
        let resolved = resolve(self)?;
        command.env_clear().envs(resolved.variables);
        Ok(())
    }
}

/// Measured host tool identity written into a report without local paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompatibilityHostToolReport {
    /// Measured executable SHA-256.
    pub sha256: Sha256Digest,
    /// Measured executable byte length.
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ResolvedProbeEnvironment {
    pub(super) variables: BTreeMap<String, String>,
    pub(super) host_tools: BTreeMap<String, CompatibilityHostToolReport>,
}

pub(super) fn resolve(
    environment: &CompatibilityEnvironment,
) -> Result<ResolvedProbeEnvironment, ContractError> {
    let (variables, host_tools) = match environment {
        CompatibilityEnvironment::Poisoned { variables } => {
            if variables.get("PATH").map(String::as_str) != Some(POISONED_PATH) {
                return Err(ContractError::compatibility(
                    "poisoned compatibility environment must use PATH=/nonexistent",
                ));
            }
            (variables.clone(), BTreeMap::new())
        }
        CompatibilityEnvironment::SealedHostTools {
            variables,
            host_tools,
        } => {
            if variables.get("PATH").map(String::as_str) != Some(POISONED_PATH) {
                return Err(ContractError::compatibility(
                    "sealed compatibility host tools require PATH=/nonexistent before installing the measured closure",
                ));
            }
            host_tools.revalidate()?;
            let path = host_tools.root.to_str().ok_or_else(|| {
                ContractError::compatibility(
                    "compatibility host-tool closure path is not valid UTF-8 for PATH",
                )
            })?;
            let mut variables = variables.clone();
            variables.insert("PATH".into(), path.into());
            let host_tools = host_tools
                .tools
                .iter()
                .map(|(name, identity)| {
                    (
                        name.clone(),
                        CompatibilityHostToolReport {
                            sha256: identity.sha256.clone(),
                            size: identity.size,
                        },
                    )
                })
                .collect();
            (variables, host_tools)
        }
    };
    validate_variables(&variables)?;
    Ok(ResolvedProbeEnvironment {
        variables,
        host_tools,
    })
}

pub(super) fn identity(
    environment: &ResolvedProbeEnvironment,
) -> Result<Sha256Digest, ContractError> {
    let mut variables = environment.variables.clone();
    if !environment.host_tools.is_empty() {
        variables.insert("PATH".into(), "/__aros_compatibility_host_tools__".into());
    }
    let encoded = crate::canonical::bytes(&serde_json::json!({
        "environment": variables,
        "host_tools": environment.host_tools,
    }))
    .map_err(|_| {
        ContractError::compatibility("cannot canonically encode the compatibility environment")
    })?;
    Ok(sha256_bytes(&encoded))
}

pub(super) fn validate_variables(
    environment: &BTreeMap<String, String>,
) -> Result<(), ContractError> {
    if environment.len() > MAX_PROBE_ENVIRONMENT_ENTRIES {
        return Err(ContractError::compatibility(
            "compatibility process has more environment entries than the configured limit",
        ));
    }
    let environment_bytes = environment
        .iter()
        .try_fold(0_usize, |total, (name, value)| {
            if !valid_name(name) || value.chars().any(char::is_control) {
                return Err(ContractError::compatibility(
                    "compatibility process environment contains an unsafe name or value",
                ));
            }
            total
                .checked_add(name.len())
                .and_then(|total| total.checked_add(value.len()))
                .ok_or_else(|| {
                    ContractError::compatibility(
                        "compatibility process environment length overflowed",
                    )
                })
        })?;
    if environment_bytes > MAX_PROBE_ENVIRONMENT_BYTES {
        return Err(ContractError::compatibility(
            "compatibility process environment exceeds the configured byte limit",
        ));
    }
    Ok(())
}

fn valid_name(name: &str) -> bool {
    // Autoconf consumes this lower-case cache variable before it derives host
    // compiler helper names. It is the sole non-portable spelling admitted by
    // the closed native upstream-compatibility environment.
    if name == "ac_cv_prog_cc_c23" {
        return true;
    }
    let mut characters = name.bytes();
    matches!(characters.next(), Some(b'A'..=b'Z' | b'_'))
        && characters.all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

#[cfg(test)]
mod handoff_tests {
    use super::*;
    use std::process::Command;
    use std::time::Duration;

    #[test]
    fn policy_handoff_clears_ambient_and_explicit_command_environment() {
        let mut command = Command::new("/bin/sh");
        command.env("INJECTED", "ambient").args([
            "-c",
            "test -z \"${HOME+x}\" && test -z \"${INJECTED+x}\" && printf '%s' \"$PATH\"",
        ]);
        CompatibilityEnvironment::Poisoned {
            variables: BTreeMap::from([("PATH".into(), "/nonexistent".into())]),
        }
        .apply_to(&mut command)
        .unwrap();
        let output =
            aros_common::run_output_with_timeout(&mut command, 4096, Duration::from_secs(5))
                .unwrap();
        assert!(output.status.success() && !output.timed_out);
        assert_eq!(output.stdout.exact_bytes().unwrap(), b"/nonexistent");
    }

    #[test]
    fn invalid_policy_does_not_partially_modify_the_command() {
        let mut command = Command::new("/bin/sh");
        command.env("UNCHANGED", "yes");
        let original: Vec<_> = command
            .get_envs()
            .map(|(key, value)| (key.to_owned(), value.map(std::ffi::OsStr::to_owned)))
            .collect();
        assert!(CompatibilityEnvironment::Poisoned {
            variables: BTreeMap::from([("PATH".into(), "/ambient".into())])
        }
        .apply_to(&mut command)
        .is_err());
        let current: Vec<_> = command
            .get_envs()
            .map(|(key, value)| (key.to_owned(), value.map(std::ffi::OsStr::to_owned)))
            .collect();
        assert_eq!(current, original);
    }

    #[test]
    fn sealed_handoff_uses_only_the_revalidated_closure_path() {
        let temporary = tempfile::tempdir().unwrap();
        let closure = crate::compatibility::prepare_host_tool_closure(
            &crate::compatibility::HostToolClosureRequest {
                output_root: temporary.path().join("closure"),
                tools: vec![crate::compatibility::CompatibilityHostTool {
                    name: "sh".into(),
                    program: std::path::Path::new("/bin/sh").canonicalize().unwrap(),
                }],
            },
        )
        .unwrap();
        let expected = closure.root.to_string_lossy().into_owned();
        let policy = CompatibilityEnvironment::SealedHostTools {
            variables: BTreeMap::from([("PATH".into(), "/nonexistent".into())]),
            host_tools: closure,
        };
        let mut command = Command::new("/bin/sh");
        command
            .env("PATH", "/ambient")
            .args(["-c", "printf '%s' \"$PATH\""]);
        policy.apply_to(&mut command).unwrap();
        let output =
            aros_common::run_output_with_timeout(&mut command, 4096, Duration::from_secs(5))
                .unwrap();
        assert!(output.status.success() && !output.timed_out);
        assert_eq!(output.stdout.exact_bytes().unwrap(), expected.as_bytes());
        std::fs::remove_file(temporary.path().join("closure/sh")).unwrap();
        assert!(policy.apply_to(&mut command).is_err());
    }
}
