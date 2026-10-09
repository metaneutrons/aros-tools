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
use crate::source_lock::CompilerFamily;
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
    let make = probe(make)?;
    let gnu = profile.family() == CompilerFamily::Gnu;
    if gnu {
        require_gnu_make(&make.version)?;
    }
    tools.push(make);
    if gnu {
        let cc_version = tools
            .iter()
            .find(|tool| tool.name == "cc")
            .map(|tool| tool.version.clone())
            .ok_or_else(|| {
                ContractError::prerequisite("native preflight omitted host C compiler")
            })?;
        tools.extend(probe_llvm_gnu_binutils(
            profile.family(),
            &cc_version,
            probe,
        )?);
        for name in ["bison", "flex", "patch", "pkg-config", "ninja"] {
            tools.push(probe(name)?);
        }
    }
    Ok(HostPreflight {
        host,
        profile: profile.name().to_owned(),
        capabilities: profile.capabilities().to_vec(),
        tools,
    })
}

fn uses_llvm_gnu_binutils(family: CompilerFamily, cc_version: &str) -> bool {
    if family != CompilerFamily::Gnu {
        return false;
    }
    // M3 passes the preflighted `cc` invocation path to configure as CC. The
    // source derives HOST_CC_NAME from that invocation name, before symlink
    // resolution; its exact `gcc`-name exception is not selected here.
    let version = cc_version.to_ascii_lowercase();
    version.contains("clang") || version.contains("llvm")
}

fn probe_llvm_gnu_binutils<F>(
    family: CompilerFamily,
    cc_version: &str,
    mut probe_tool: F,
) -> Result<Vec<HostTool>, ContractError>
where
    F: FnMut(&'static str) -> Result<HostTool, ContractError>,
{
    if !uses_llvm_gnu_binutils(family, cc_version) {
        return Ok(Vec::new());
    }
    ["llvm-ar", "llvm-ranlib"]
        .into_iter()
        .map(&mut probe_tool)
        .collect()
}

fn require_gnu_make(version: &str) -> Result<(), ContractError> {
    // The selected source's generated Make rules use GNU Make's $(file ...),
    // introduced in 4.0. Apple's 3.81 can start, but cannot execute that graph.
    let supported = version.strip_prefix("GNU Make ").is_some_and(|version| {
        let mut parts = version.split('.');
        let major = parts.next().and_then(|value| value.parse::<u32>().ok());
        let minor = parts.next().and_then(|value| value.parse::<u32>().ok());
        matches!((major, minor), (Some(major), Some(_)) if major >= 4)
    });
    if !supported {
        return Err(ContractError::prerequisite(
            "native GNU toolchain production requires GNU Make 4.0 or newer; select gmake on macOS",
        ));
    }
    Ok(())
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
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    use std::path::PathBuf;

    use serde_json::json;
    use tempfile::tempdir;

    use super::{
        inspect, probe_llvm_gnu_binutils, probe_selected, require_gnu_make, uses_llvm_gnu_binutils,
    };
    use crate::preflight::HostTool;
    use crate::profiles::Profiles;
    use crate::source_lock::CompilerFamily;
    use crate::ContractError;

    #[test]
    fn gnu_make_gate_accepts_file_function_versions_and_rejects_legacy_or_unknown() {
        for version in ["GNU Make 4.0", "GNU Make 4.4.1", "GNU Make 5.0"] {
            require_gnu_make(version).unwrap();
        }
        for version in [
            "GNU Make 3.81",
            "BSD make 4.4",
            "GNU Make 4",
            "GNU Make 4.bad",
            "GNU Make nope",
        ] {
            assert!(require_gnu_make(version).is_err(), "{version}");
        }
    }

    #[test]
    fn llvm_companion_predicate_is_limited_to_gnu_and_matches_ascii_case_insensitively() {
        assert!(uses_llvm_gnu_binutils(
            CompilerFamily::Gnu,
            "Apple ClAnG version 17.0.0"
        ));
        assert!(uses_llvm_gnu_binutils(
            CompilerFamily::Gnu,
            "Acme LLVM compiler version 19"
        ));
        assert!(!uses_llvm_gnu_binutils(
            CompilerFamily::Gnu,
            "gcc (Homebrew GCC 15.2.0) 15.2.0"
        ));
        assert!(!uses_llvm_gnu_binutils(
            CompilerFamily::Llvm,
            "Apple clang version 17.0.0"
        ));
    }

    #[test]
    fn llvm_gnu_companion_resolver_probes_both_tools_and_propagates_missing_tool() {
        let resolved =
            probe_llvm_gnu_binutils(CompilerFamily::Gnu, "Apple clang version 17.0.0", |name| {
                let path = PathBuf::from(format!("/fake/llvm/bin/{name}"));
                Ok(HostTool {
                    name,
                    path: path.clone(),
                    invocation_path: path,
                    version: format!("{name} fixture"),
                })
            })
            .unwrap();
        assert_eq!(
            resolved.iter().map(|tool| tool.name).collect::<Vec<_>>(),
            ["llvm-ar", "llvm-ranlib"]
        );
        assert!(resolved
            .iter()
            .all(|tool| tool.invocation_path.is_absolute()));

        let mut requested = Vec::new();
        let missing =
            probe_llvm_gnu_binutils(CompilerFamily::Gnu, "Apple clang version 17.0.0", |name| {
                requested.push(name);
                if name == "llvm-ranlib" {
                    return Err(ContractError::prerequisite(
                        "required native producer tool 'llvm-ranlib' is unavailable",
                    ));
                }
                Ok(HostTool {
                    name,
                    path: PathBuf::from(format!("/fake/llvm/bin/{name}")),
                    invocation_path: PathBuf::from(format!("/fake/llvm/bin/{name}")),
                    version: format!("{name} fixture"),
                })
            });

        assert!(missing.is_err());
        assert!(missing.unwrap_err().to_string().contains("llvm-ranlib"));
        assert_eq!(requested, ["llvm-ar", "llvm-ranlib"]);

        let mut llvm_requests = Vec::new();
        let skipped =
            probe_llvm_gnu_binutils(CompilerFamily::Llvm, "Apple clang version 17.0.0", |name| {
                llvm_requests.push(name);
                Err(ContractError::prerequisite("unexpected probe"))
            })
            .unwrap();
        assert!(skipped.is_empty());
        assert!(llvm_requests.is_empty());
    }

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
        // A symlink itself does not need an executable temporary directory:
        // keep the real target in Cargo's proven toolchain location. This
        // exercises the same invocation-name behaviour as rustup's `cargo`
        // proxy without assuming that a runner permits executing new scripts
        // beneath its test-output directory.
        let temporary = tempdir().unwrap();
        let target = which::which("cargo").unwrap();
        let proxy = temporary.path().join("cargo");
        symlink(&target, &proxy).unwrap();

        let tool = probe_selected("cargo", proxy.clone()).unwrap();

        assert_eq!(tool.path, target.canonicalize().unwrap());
        assert_eq!(tool.invocation_path, proxy);
        assert!(tool.version.starts_with("cargo "), "{}", tool.version);
    }
}
