//! Shared derivation of the exact native compatibility environments.
//!
//! Execution and retained-evidence readers use the same policy, but readers
//! derive their expectation from prepared runtime inputs, never from reports.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use aros_common::Sha256Digest;

use super::{
    environment, native_compatibility_host_tools, CompatibilityEnvironment,
    CompatibilityHostToolReport, HostToolClosure,
};
use crate::python_environment::PythonEnvironment;
use crate::ContractError;

const POISONED_PATH: &str = "/nonexistent";

/// Independently derived environment claims for one native compatibility lane.
///
/// These are measured policy identities, not signatures or release admission.
/// The caller must separately bind the prepared Python runtime and host-tool
/// closure to its verified release inputs. No retained report is consulted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeCompatibilityEnvironmentIdentity {
    /// Revalidated command identities for the four CMake/upstream phases.
    pub host_tools: BTreeMap<String, CompatibilityHostToolReport>,
    /// Closed Python plus measured host-command environment identity.
    pub sdk_environment_sha256: Sha256Digest,
    /// Fixed PATH-poisoned environment identity for standalone C and C++.
    pub standalone_environment_sha256: Sha256Digest,
}

/// Derive the expected environment identities without consuming any reports.
///
/// The prepared interpreter and private import roots are revalidated, as are
/// the exact platform-specific command roles and their executable bytes. The
/// interpreter must be the same executable exposed as the `python3` role.
/// Derivation shares the execution policy, including Autoconf's explicit C23
/// cache input and the standalone `PATH=/nonexistent` policy.
///
/// The closure's local PATH is normalized in its identity. Python executable
/// and import paths retain the existing report-v5 identity policy; this is an
/// exact-runtime expectation, not a portable reconstruction across new roots.
/// Callers must retain that runtime layout or qualify the new environment.
/// Private Python root directories are checked, but their module contents are
/// not remeasured here. Callers must verify those bytes separately against the
/// locked runtime inputs; this digest alone does not detect module changes at
/// an unchanged path.
///
/// # Errors
///
/// Returns AX0401 for unsafe or changed Python runtime inputs and AX0703 for
/// unsupported hosts, mixed interpreters, changed tools, incomplete or excess
/// command roles, or an invalid closed environment. This operation reads the
/// prepared runtime and runs its existing bounded Python version probe; it
/// creates no output roots, executes no compatibility phase, accesses no
/// network, and grants no publication authority.
pub fn derive_native_compatibility_environment_identity(
    host_python: &PythonEnvironment,
    host_tools: &HostToolClosure,
    host: &str,
) -> Result<NativeCompatibilityEnvironmentIdentity, ContractError> {
    let environments = prepare_native_environments(host_python, host_tools, host)?;
    let sdk = environment::resolve(&environments.sdk)?;
    let standalone = environment::resolve(&environments.standalone)?;
    Ok(NativeCompatibilityEnvironmentIdentity {
        sdk_environment_sha256: environment::identity(&sdk)?,
        standalone_environment_sha256: environment::identity(&standalone)?,
        host_tools: sdk.host_tools,
    })
}

pub(super) struct NativeCompatibilityEnvironments {
    pub(super) sdk: CompatibilityEnvironment,
    pub(super) standalone: CompatibilityEnvironment,
}

pub(super) fn prepare_native_environments(
    host_python: &PythonEnvironment,
    host_tools: &HostToolClosure,
    host: &str,
) -> Result<NativeCompatibilityEnvironments, ContractError> {
    // Validate the selector before probing the prepared interpreter.
    let required_host_tools = native_compatibility_host_tools(host)?;
    let mut variables = host_python.compatibility_environment()?;
    // Autoconf 2.73 can append a C23 dialect marker before upstream captures
    // its compiler base name, producing impossible LLVM helper names. This
    // explicit recorded input is identical for execution and read-back.
    variables.insert("ac_cv_prog_cc_c23".into(), String::new());
    validate_host_environment(&variables, host_tools, &required_host_tools)?;
    Ok(NativeCompatibilityEnvironments {
        sdk: CompatibilityEnvironment::SealedHostTools {
            variables,
            host_tools: host_tools.clone(),
        },
        standalone: CompatibilityEnvironment::Poisoned {
            variables: BTreeMap::from([("PATH".into(), POISONED_PATH.into())]),
        },
    })
}

fn validate_host_environment(
    environment: &BTreeMap<String, String>,
    host_tools: &HostToolClosure,
    required_host_tools: &[&str],
) -> Result<(), ContractError> {
    let expected = BTreeSet::from([
        "ac_cv_prog_cc_c23",
        "PATH",
        "PYTHON",
        "PYTHONDONTWRITEBYTECODE",
        "PYTHONHASHSEED",
        "PYTHONNOUSERSITE",
        "PYTHONPATH",
    ]);
    if environment
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>()
        != expected
        || environment.get("PATH").map(String::as_str) != Some(POISONED_PATH)
        || environment
            .get("PYTHONDONTWRITEBYTECODE")
            .map(String::as_str)
            != Some("1")
        || environment.get("PYTHONHASHSEED").map(String::as_str) != Some("0")
        || environment.get("PYTHONNOUSERSITE").map(String::as_str) != Some("1")
        || environment.get("ac_cv_prog_cc_c23").map(String::as_str) != Some("")
        || environment.get("PYTHONPATH").is_none_or(String::is_empty)
    {
        return Err(ContractError::compatibility(
            "upstream compatibility phase does not have the exact closed Python environment",
        ));
    }
    host_tools.revalidate()?;
    let python = PathBuf::from(environment.get("PYTHON").ok_or_else(|| {
        ContractError::compatibility(
            "upstream compatibility Python environment lost its interpreter",
        )
    })?);
    let Some(host_python) = host_tools.tools.get("python3") else {
        return Err(ContractError::compatibility(
            "upstream compatibility host-tool closure does not expose the checked python3 interpreter",
        ));
    };
    let expected_roles = required_host_tools.iter().copied().collect::<BTreeSet<_>>();
    let actual_roles = host_tools
        .tools
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let missing_roles = expected_roles
        .difference(&actual_roles)
        .copied()
        .collect::<Vec<_>>();
    let unexpected_roles = actual_roles
        .difference(&expected_roles)
        .copied()
        .collect::<Vec<_>>();
    if python != host_python.program || !missing_roles.is_empty() || !unexpected_roles.is_empty() {
        return Err(ContractError::compatibility(
            format!(
                "native compatibility host-tool closure does not bind the exact measured command set{}{}",
                if missing_roles.is_empty() { String::new() } else { format!(": missing {}", missing_roles.join(", ")) },
                if unexpected_roles.is_empty() { String::new() } else { format!("; unexpected {}", unexpected_roles.join(", ")) },
            ),
        ));
    }
    Ok(())
}
