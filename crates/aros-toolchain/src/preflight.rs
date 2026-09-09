//! Bounded native-host prerequisite discovery for the producer input phase.
//!
//! The observations here are not executable-origin evidence and do not start
//! a compiler build. They identify the exact host tools whose directories a
//! later sanitized environment may admit.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use aros_common::run_output_with_timeout;
use serde::Serialize;

use crate::profiles::Profile;
use crate::ContractError;

const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const CAPTURE_LIMIT: usize = 16 * 1024;

/// One trusted-host tool observation, taken before any build phase starts.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct HostTool {
    /// Stable prerequisite role, not an arbitrary user-supplied command name.
    pub name: &'static str,
    /// Canonical resolved target selected from the current host `PATH`.
    pub path: PathBuf,
    /// Absolute invocation path selected from the current host `PATH`.
    ///
    /// This deliberately retains a checked proxy/symlink path. Replacing it
    /// with its canonical target can change invocation-name-selected tool behaviour;
    /// Rustup's `cargo` proxy is one such supported host prerequisite.
    pub invocation_path: PathBuf,
    /// Sanitized first version line from that exact executable.
    pub version: String,
}

/// Read-only input-stage host observation for one selected profile.
#[derive(Debug, Clone, Serialize)]
pub struct HostPreflight {
    /// Current supported native host identifier.
    pub host: &'static str,
    /// Selected producer profile; this is not a target-host execution claim.
    pub profile: String,
    /// Closed source-contract capability set required by the profile.
    pub capabilities: Vec<String>,
    /// Exact host executables used to form the later sanitized PATH.
    pub tools: Vec<HostTool>,
}

impl HostPreflight {
    /// Directories, in selected-tool precedence order, for `ProducerEnvironment`.
    #[must_use]
    pub fn tool_directories(&self) -> Vec<PathBuf> {
        let mut seen = BTreeSet::new();
        self.tools
            .iter()
            .filter_map(|tool| tool.invocation_path.parent().map(PathBuf::from))
            .filter(|directory| seen.insert(directory.clone()))
            .collect()
    }
}

/// Probe the exact native host tools required before M3 may start a build.
///
/// This checks that the selected profile has a closed recognized capability
/// set, discovers absolute tool paths, and obtains bounded version output. It
/// does not run `configure`, `make`, Cargo compilation, package installation,
/// network acquisition or any source-provided code.
///
/// # Errors
///
/// Returns AX0201 for a missing/failed host prerequisite and AX0101 for a
/// profile unsupported by the compiled native source contract.
pub fn inspect(profile: &Profile) -> Result<HostPreflight, ContractError> {
    let host = aros_common::target::native_host_key().ok_or_else(|| {
        ContractError::preflight("native producer preflight is unsupported on this host")
    })?;
    if profile.capabilities().is_empty() {
        return Err(ContractError::invalid(
            "selected profile has no source-contract capabilities",
        ));
    }
    let mut tools = Vec::new();
    for name in ["git", "cmake", "cc", "c++", "python3", "rustc", "cargo"] {
        tools.push(probe(name)?);
    }
    let make = if which::which("gmake").is_ok() {
        "gmake"
    } else {
        "make"
    };
    tools.push(probe(make)?);
    Ok(HostPreflight {
        host,
        profile: profile.name().to_owned(),
        capabilities: profile.capabilities().to_vec(),
        tools,
    })
}

fn probe(name: &'static str) -> Result<HostTool, ContractError> {
    let selected = which::which(name).map_err(|_| {
        ContractError::prerequisite(format!(
            "required native producer tool '{name}' is unavailable"
        ))
    })?;
    probe_selected(name, selected)
}

fn probe_selected(name: &'static str, selected: PathBuf) -> Result<HostTool, ContractError> {
    if !selected.is_absolute() {
        return Err(ContractError::prerequisite(format!(
            "required native producer tool '{name}' did not resolve to an absolute invocation path"
        )));
    }
    let resolved = fs::canonicalize(&selected).map_err(|_| {
        ContractError::prerequisite(format!(
            "required native producer tool '{name}' cannot be canonicalized"
        ))
    })?;
    if !resolved.is_file() {
        return Err(ContractError::prerequisite(format!(
            "required native producer tool '{name}' is not a regular executable file"
        )));
    }
    let parent = selected.parent().ok_or_else(|| {
        ContractError::prerequisite(format!(
            "required native producer tool '{name}' has no executable directory"
        ))
    })?;
    let mut command = Command::new(&selected);
    command
        .env_clear()
        .env("PATH", parent)
        .env("LC_ALL", "C")
        .arg("--version");
    for variable in ["HOME", "RUSTUP_HOME", "CARGO_HOME"] {
        if let Some(value) =
            std::env::var_os(variable).filter(|value| Path::new(value).is_absolute())
        {
            command.env(variable, value);
        }
    }
    let output =
        run_output_with_timeout(&mut command, CAPTURE_LIMIT, PROBE_TIMEOUT).map_err(|_| {
            ContractError::prerequisite(format!(
                "required native producer tool '{name}' could not be started"
            ))
        })?;
    if output.timed_out || !output.status.success() {
        return Err(ContractError::prerequisite(format!(
            "required native producer tool '{name}' did not provide a usable version"
        )));
    }
    let version = output
        .stdout
        .exact_bytes()
        .or_else(|| output.stderr.exact_bytes())
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .and_then(|text| text.lines().next())
        .filter(|line| version_line(line))
        .ok_or_else(|| {
            ContractError::prerequisite(format!(
                "required native producer tool '{name}' returned an unsafe version line"
            ))
        })?
        .to_owned();
    Ok(HostTool {
        name,
        path: resolved,
        invocation_path: selected,
        version,
    })
}

fn version_line(line: &str) -> bool {
    !line.is_empty()
        && line.len() <= 512
        && line
            .bytes()
            .all(|byte| !byte.is_ascii_control() && byte.is_ascii())
}

#[cfg(test)]
mod tests {
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::{symlink, PermissionsExt};

    use serde_json::json;
    use tempfile::tempdir;

    use super::{inspect, probe_selected};
    use crate::profiles::Profiles;

    #[test]
    fn observes_the_supported_host_tools_without_running_a_build() {
        let profiles = Profiles::parse(
            &serde_json::to_vec(&json!({
                "schema": "aros-toolchain-profiles-v1", "upstream_commit": "a".repeat(40),
                "profiles": [{
                    "name": "pc-x86_64", "configure_target": "pc-x86_64", "upstream_output_target": "pc-x86_64",
                    "target_triple": "x86_64-unknown-aros", "cpu": "x86_64", "platform": "pc", "float_abi": "",
                    "capabilities": ["c", "cxx", "standalone-collector"]
                }]
            }))
            .unwrap(),
        )
        .unwrap();
        let preflight = inspect(profiles.select("pc-x86_64").unwrap()).unwrap();
        assert!(preflight.tools.iter().all(|tool| tool.path.is_absolute()));
        assert!(preflight
            .tools
            .iter()
            .all(|tool| tool.invocation_path.is_absolute()));
        assert!(preflight.tools.iter().all(|tool| !tool.version.is_empty()));
        assert!(!preflight.tool_directories().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn preserves_proxy_invocation_path_after_validating_its_resolved_target() {
        let temporary = tempdir().unwrap();
        let target = temporary.path().join("cargo-proxy-target");
        fs::write(
            &target,
            "#!/bin/sh\ntest \"${0##*/}\" = cargo || exit 97\nprintf '%s\\n' 'cargo fixture 1.0'\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&target).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&target, permissions).unwrap();
        let proxy = temporary.path().join("cargo");
        symlink(&target, &proxy).unwrap();

        let tool = probe_selected("cargo", proxy.clone()).unwrap();

        assert_eq!(tool.path, target.canonicalize().unwrap());
        assert_eq!(tool.invocation_path, proxy);
        assert_eq!(tool.version, "cargo fixture 1.0");
    }
}
