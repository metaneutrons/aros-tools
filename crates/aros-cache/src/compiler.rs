//! Compiler-cache backend discovery without starting a backend process.

use std::env;
use std::ffi::OsString;
use std::path::PathBuf;

use serde::Serialize;

/// One supported compiler-cache backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompilerBackend {
    /// Mozilla sccache.
    Sccache,
    /// ccache.
    Ccache,
}

impl CompilerBackend {
    /// Executable name used for both launcher and management processes.
    #[must_use]
    pub const fn program(self) -> &'static str {
        match self {
            Self::Sccache => "sccache",
            Self::Ccache => "ccache",
        }
    }

    /// Command-line argument that asks the backend for statistics.
    #[must_use]
    pub const fn stats_argument() -> &'static str {
        "-s"
    }
}

/// A user or build policy request for compiler caching.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompilerBackendChoice {
    /// Use the first available backend in the stable AROS preference order.
    Auto,
    /// Do not use a compiler-cache launcher.
    Off,
    /// Require sccache; never silently substitute ccache.
    Sccache,
    /// Require ccache; never silently substitute sccache.
    Ccache,
}

/// The resulting compiler-cache choice passed across frontend boundaries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompilerCacheSelection {
    /// Compiler caching is intentionally disabled.
    Off,
    /// A selected backend and its exact executable.
    Backend {
        /// Selected backend kind.
        backend: CompilerBackend,
        /// Absolute executable selected for this operation.
        executable: PathBuf,
    },
}

/// Whether a backend executable was observed without starting it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompilerBackendState {
    /// An executable was found on `PATH`.
    Available,
    /// No executable was found on `PATH`.
    Missing,
}

impl CompilerBackendState {
    /// Stable lowercase label used by human-oriented result rendering.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Missing => "missing",
        }
    }
}

/// Passive scope classification based only on explicitly supplied
/// configuration variables.
///
/// This classification intentionally does not parse configuration files or
/// contact a daemon. Therefore it never claims a configured backend is local
/// or remote until a later explicit query can prove that fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompilerConfigurationScope {
    /// No recognized configuration was set in the observed environment.
    Unconfigured,
    /// One or more recognized variables were set, but effective storage is not
    /// queried by this passive operation.
    ConfigurationUninspected,
}

impl CompilerConfigurationScope {
    /// Stable lowercase label used by human-oriented result rendering.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unconfigured => "unconfigured",
            Self::ConfigurationUninspected => "configuration uninspected",
        }
    }
}

/// One recognized environment variable that influences backend selection or
/// storage. Values are intentionally not serialized because configuration can
/// contain credentials or endpoints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompilerConfigurationSource {
    /// Environment-variable name, not its potentially sensitive value.
    pub variable: &'static str,
}

/// A passive observation of one compiler-cache backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompilerBackendObservation {
    /// Observed backend.
    pub backend: CompilerBackend,
    /// Whether its executable is currently available.
    pub state: CompilerBackendState,
    /// Absolute executable path when one was found.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub executable: Option<PathBuf>,
    /// Whether backend configuration was absent or intentionally uninspected.
    pub configuration_scope: CompilerConfigurationScope,
    /// Environment variables that made the scope non-default.
    pub configuration_sources: Vec<CompilerConfigurationSource>,
    /// This operation's boundary; it never invokes a backend process.
    pub observation: &'static str,
}

/// Environment values relevant to passive compiler-cache discovery.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompilerEnvironment {
    values: Vec<(&'static str, Option<OsString>)>,
}

impl CompilerEnvironment {
    /// Snapshot recognized environment inputs once.
    #[must_use]
    pub fn current() -> Self {
        Self {
            values: vec![
                ("SCCACHE_DIR", env::var_os("SCCACHE_DIR")),
                ("SCCACHE_CONF", env::var_os("SCCACHE_CONF")),
                ("SCCACHE_ENDPOINT", env::var_os("SCCACHE_ENDPOINT")),
                ("SCCACHE_REDIS", env::var_os("SCCACHE_REDIS")),
                ("SCCACHE_MEMCACHED", env::var_os("SCCACHE_MEMCACHED")),
                ("SCCACHE_GCS_BUCKET", env::var_os("SCCACHE_GCS_BUCKET")),
                ("SCCACHE_S3_BUCKET", env::var_os("SCCACHE_S3_BUCKET")),
                (
                    "SCCACHE_AZURE_BLOB_CONTAINER",
                    env::var_os("SCCACHE_AZURE_BLOB_CONTAINER"),
                ),
                ("CCACHE_DIR", env::var_os("CCACHE_DIR")),
                ("CCACHE_CONFIGPATH", env::var_os("CCACHE_CONFIGPATH")),
                (
                    "CCACHE_REMOTE_STORAGE",
                    env::var_os("CCACHE_REMOTE_STORAGE"),
                ),
                (
                    "CCACHE_SECONDARY_STORAGE",
                    env::var_os("CCACHE_SECONDARY_STORAGE"),
                ),
            ],
        }
    }

    #[cfg(test)]
    fn from_set_variables(variables: &[&'static str]) -> Self {
        Self {
            values: variables
                .iter()
                .copied()
                .map(|variable| {
                    (
                        variable,
                        variables
                            .contains(&variable)
                            .then(|| OsString::from("redacted-for-test")),
                    )
                })
                .collect(),
        }
    }

    fn sources_for(&self, backend: CompilerBackend) -> Vec<CompilerConfigurationSource> {
        self.values
            .iter()
            .filter(|(variable, value)| {
                value.is_some() && variable_belongs_to_backend(variable, backend)
            })
            .map(|(variable, _)| CompilerConfigurationSource { variable })
            .collect()
    }
}

/// Observe both supported compiler-cache backends using the process
/// environment and `PATH`. No backend process is invoked.
#[must_use]
pub fn observe_compiler_backends() -> Vec<CompilerBackendObservation> {
    let environment = CompilerEnvironment::current();
    [CompilerBackend::Sccache, CompilerBackend::Ccache]
        .into_iter()
        .map(|backend| observe_compiler_backend_with(backend, &environment, &locate_current))
        .collect()
}

/// Observe one compiler-cache backend using the process environment and
/// `PATH`. No backend process is invoked.
#[must_use]
pub fn observe_compiler_backend(backend: CompilerBackend) -> CompilerBackendObservation {
    observe_compiler_backend_with(backend, &CompilerEnvironment::current(), &locate_current)
}

/// Resolve the exact executable for a compiler-cache request. This function
/// performs no backend query and never invokes a fallback after an explicit
/// backend selection.
///
/// # Errors
///
/// Returns a user-oriented message when an explicitly requested backend is
/// unavailable.
pub fn resolve_compiler_cache(
    choice: CompilerBackendChoice,
) -> Result<CompilerCacheSelection, String> {
    resolve_compiler_cache_with(choice, &locate_current)
}

fn resolve_compiler_cache_with<F>(
    choice: CompilerBackendChoice,
    locate: &F,
) -> Result<CompilerCacheSelection, String>
where
    F: Fn(&'static str) -> Result<PathBuf, which::Error>,
{
    let select = |backend: CompilerBackend| {
        locate(backend.program())
            .map(|executable| CompilerCacheSelection::Backend { backend, executable })
            .map_err(|_| {
                format!(
                    "requested compiler cache '{}' is unavailable on PATH; install it or use --compiler-cache off",
                    backend.program()
                )
            })
    };
    match choice {
        CompilerBackendChoice::Off => Ok(CompilerCacheSelection::Off),
        CompilerBackendChoice::Sccache => select(CompilerBackend::Sccache),
        CompilerBackendChoice::Ccache => select(CompilerBackend::Ccache),
        CompilerBackendChoice::Auto => {
            for backend in [CompilerBackend::Sccache, CompilerBackend::Ccache] {
                if let Ok(executable) = locate(backend.program()) {
                    return Ok(CompilerCacheSelection::Backend {
                        backend,
                        executable,
                    });
                }
            }
            Ok(CompilerCacheSelection::Off)
        }
    }
}

fn observe_compiler_backend_with<F>(
    backend: CompilerBackend,
    environment: &CompilerEnvironment,
    locate: &F,
) -> CompilerBackendObservation
where
    F: Fn(&'static str) -> Result<PathBuf, which::Error>,
{
    let executable = locate(backend.program()).ok();
    let configuration_sources = environment.sources_for(backend);
    CompilerBackendObservation {
        backend,
        state: if executable.is_some() {
            CompilerBackendState::Available
        } else {
            CompilerBackendState::Missing
        },
        executable,
        configuration_scope: if configuration_sources.is_empty() {
            CompilerConfigurationScope::Unconfigured
        } else {
            CompilerConfigurationScope::ConfigurationUninspected
        },
        configuration_sources,
        observation: "passive",
    }
}

fn variable_belongs_to_backend(variable: &str, backend: CompilerBackend) -> bool {
    match backend {
        CompilerBackend::Sccache => variable.starts_with("SCCACHE_"),
        CompilerBackend::Ccache => variable.starts_with("CCACHE_"),
    }
}

fn locate_current(program: &'static str) -> Result<PathBuf, which::Error> {
    which::which(program)
}

#[cfg(test)]
mod tests {
    use super::{
        observe_compiler_backend_with, resolve_compiler_cache_with, CompilerBackend,
        CompilerBackendChoice, CompilerBackendState, CompilerCacheSelection,
        CompilerConfigurationScope, CompilerEnvironment,
    };
    use std::path::PathBuf;

    fn unavailable(_: &str) -> Result<PathBuf, which::Error> {
        Err(which::Error::CannotFindBinaryPath)
    }

    #[test]
    fn explicit_backend_never_falls_back_to_another_backend() {
        let result = resolve_compiler_cache_with(CompilerBackendChoice::Sccache, &unavailable);
        assert!(result
            .unwrap_err()
            .contains("requested compiler cache 'sccache'"));
    }

    #[test]
    fn auto_uses_the_stable_sccache_first_order_then_disables_when_missing() {
        let selected = resolve_compiler_cache_with(CompilerBackendChoice::Auto, &|program| {
            if program == "ccache" {
                Ok(PathBuf::from("/tools/ccache"))
            } else {
                unavailable(program)
            }
        })
        .unwrap();
        assert_eq!(
            selected,
            CompilerCacheSelection::Backend {
                backend: CompilerBackend::Ccache,
                executable: PathBuf::from("/tools/ccache"),
            }
        );
        assert_eq!(
            resolve_compiler_cache_with(CompilerBackendChoice::Auto, &unavailable).unwrap(),
            CompilerCacheSelection::Off
        );
    }

    #[test]
    fn status_does_not_claim_environment_configuration_is_local_or_remote() {
        let environment =
            CompilerEnvironment::from_set_variables(&["SCCACHE_DIR", "SCCACHE_ENDPOINT"]);
        let observation =
            observe_compiler_backend_with(CompilerBackend::Sccache, &environment, &|_| {
                Ok(PathBuf::from("/tools/sccache"))
            });
        assert_eq!(observation.state, CompilerBackendState::Available);
        assert_eq!(
            observation.configuration_scope,
            CompilerConfigurationScope::ConfigurationUninspected
        );
        assert_eq!(
            observation
                .configuration_sources
                .iter()
                .map(|source| source.variable)
                .collect::<Vec<_>>(),
            vec!["SCCACHE_DIR", "SCCACHE_ENDPOINT"]
        );
        assert_eq!(observation.observation, "passive");
    }
}
