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
const LLVM_HOST_ARCHIVERS: [&str; 2] = ["llvm-ar", "llvm-ranlib"];

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
    if profile.family() == crate::source_lock::CompilerFamily::Gnu {
        require_gnu_make(&make.version)?;
    }
    tools.push(make);
    if profile.family() == crate::source_lock::CompilerFamily::Gnu {
        for name in ["bison", "flex", "patch", "pkg-config", "ninja"] {
            tools.push(probe(name)?);
        }
        let host_cc = tools.iter().find(|tool| tool.name == "cc").ok_or_else(|| {
            ContractError::prerequisite(
                "native preflight did not retain the observed host C compiler",
            )
        })?;
        let llvm_archivers = probe_llvm_host_archivers(host_cc, probe)?;
        tools.extend(llvm_archivers);
    }
    Ok(HostPreflight {
        host,
        profile: profile.name().to_owned(),
        capabilities: profile.capabilities().to_vec(),
        tools,
    })
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

/// Probe host archivers selected by AROS's compiler-name/version policy.
///
/// GNU producer profiles can still use an LLVM host compiler. In that case
/// `configure.in` selects `llvm-ar` and `llvm-ranlib`, unless the observed
/// compiler executable is named exactly `gcc`; a later case-insensitive GCC
/// version match also resets the host tool prefix to GNU. The observed
/// invocation path matters here because its basename, not its canonical
/// target, supplies `HOST_CC_NAME`.
fn probe_llvm_host_archivers(
    cc: &HostTool,
    probe: impl FnMut(&'static str) -> Result<HostTool, ContractError>,
) -> Result<Vec<HostTool>, ContractError> {
    if !uses_llvm_host_archiver_prefix(cc) {
        return Ok(Vec::new());
    }
    LLVM_HOST_ARCHIVERS.into_iter().map(probe).collect()
}

fn uses_llvm_host_archiver_prefix(cc: &HostTool) -> bool {
    let host_cc_name_is_gcc = cc.invocation_path.file_name() == Some(std::ffi::OsStr::new("gcc"));
    let version = cc.version.to_ascii_lowercase();
    let is_llvm_or_clang = version.contains("llvm") || version.contains("clang");
    let is_gcc = version.contains("gcc");

    is_llvm_or_clang && !host_cc_name_is_gcc && !is_gcc
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

    use serde_json::json;
    use tempfile::tempdir;

    use super::{
        inspect, probe_llvm_host_archivers, probe_selected, require_gnu_make, HostPreflight,
        HostTool,
    };
    use crate::profiles::Profiles;
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
    fn llvm_host_archiver_admission_matches_configure_name_and_version_rules() {
        let mut requests = Vec::new();
        let llvm_cc = test_host_tool("cc", "/usr/bin/cc", "Apple CLANG version 17.0.0");
        let tools = probe_llvm_host_archivers(&llvm_cc, |name| {
            requests.push(name);
            Ok(test_host_tool(
                name,
                &format!("/opt/homebrew/opt/llvm/bin/{name}"),
                "LLVM test archiver",
            ))
        })
        .unwrap();

        assert_eq!(requests, ["llvm-ar", "llvm-ranlib"]);
        assert_eq!(
            tools.iter().map(|tool| tool.name).collect::<Vec<_>>(),
            ["llvm-ar", "llvm-ranlib"]
        );
        let preflight = HostPreflight {
            host: "test-host",
            profile: "test-profile".to_owned(),
            capabilities: Vec::new(),
            tools,
        };
        assert_eq!(
            preflight.tool_directories(),
            [std::path::PathBuf::from("/opt/homebrew/opt/llvm/bin")]
        );

        for (name, version, required) in [
            ("cc", "Apple clang version 17.0.0", true),
            ("cc", "Apple LLVM version 17.0.0", true),
            ("cc", "GCC version 14.2.0", false),
            ("gcc", "Apple clang version 17.0.0", false),
            ("cc", "Apple clang GCC compatibility version", false),
        ] {
            let cc = test_host_tool(name, &format!("/tool-bin/{name}"), version);
            let mut observed = Vec::new();
            let result = probe_llvm_host_archivers(&cc, |tool_name| {
                observed.push(tool_name);
                Ok(test_host_tool(
                    tool_name,
                    &format!("/llvm-bin/{tool_name}"),
                    "test version",
                ))
            })
            .unwrap();
            assert_eq!(!result.is_empty(), required, "{name}: {version}");
            assert_eq!(observed.is_empty(), !required, "{name}: {version}");
        }
    }

    #[test]
    fn missing_llvm_host_archiver_fails_closed() {
        let cc = test_host_tool("cc", "/usr/bin/cc", "Apple clang version 17.0.0");
        for missing in ["llvm-ar", "llvm-ranlib"] {
            let mut attempted = Vec::new();
            let error = probe_llvm_host_archivers(&cc, |name| {
                attempted.push(name);
                if name == missing {
                    Err(ContractError::prerequisite(format!(
                        "required native producer tool '{name}' is unavailable"
                    )))
                } else {
                    Ok(test_host_tool(
                        name,
                        &format!("/llvm-bin/{name}"),
                        "test version",
                    ))
                }
            })
            .unwrap_err();

            assert!(error.to_string().contains(missing));
            assert_eq!(
                attempted,
                if missing == "llvm-ar" {
                    vec!["llvm-ar"]
                } else {
                    vec!["llvm-ar", "llvm-ranlib"]
                }
            );
        }
    }

    fn test_host_tool(name: &'static str, path: &str, version: &str) -> HostTool {
        let invocation_path = std::path::PathBuf::from(path);
        HostTool {
            name,
            path: invocation_path.clone(),
            invocation_path,
            version: version.to_owned(),
        }
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
