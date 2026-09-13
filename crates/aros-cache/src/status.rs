//! Bounded, non-creating cache status documents.

use serde::Serialize;

use crate::{
    observe_compiler_backends, observe_root, resolve_archive_cache_root,
    resolve_default_compiler_cache_root, resolve_explicit_root, CacheEnvironment, CompilerBackend,
    CompilerBackendChoice, CompilerBackendObservation, CompilerBackendState, RootObservation,
    RootResolutionError,
};
use std::path::PathBuf;

/// Stable result-schema identifier for passive cache status.
pub const CACHE_STATUS_SCHEMA: &str = "aros-cache-status-v1";

/// Stable result-schema identifier for passive compiler-cache status.
pub const COMPILER_CACHE_STATUS_SCHEMA: &str = "aros-cache-compiler-status-v1";

/// Public cache capability implemented by the current command surface.
///
/// Future lifecycle operations deliberately do not appear here before their
/// individual ownership and safety contracts are implemented.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheCapability {
    /// Bounded passive observation.
    Status,
}

/// Side-effect contract of a versioned cache result.
///
/// A `false` value is a hard boundary of the current status implementation,
/// not an estimate based on a backend's configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "the versioned JSON schema exposes independent, auditable side-effect guarantees"
)]
pub struct CacheSideEffects {
    /// Whether the command creates directories or files.
    pub creates_state: bool,
    /// Whether the command changes files, metadata or backend state.
    pub mutates_state: bool,
    /// Whether the command contacts a network endpoint.
    pub network: bool,
    /// Whether the command starts or queries a compiler-cache daemon.
    pub backend_process: bool,
    /// Whether the command takes a cache lifecycle lock.
    pub locks: bool,
    /// Whether the command hashes cache payload content.
    pub hashes_payloads: bool,
}

const PASSIVE_SIDE_EFFECTS: CacheSideEffects = CacheSideEffects {
    creates_state: false,
    mutates_state: false,
    network: false,
    backend_process: false,
    locks: false,
    hashes_payloads: false,
};

/// One cache family in the public resource-oriented interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheFamily {
    /// Compiler-result acceleration backends.
    Compiler,
    /// Downloaded host and cross-compiler archives.
    Archives,
    /// Producer, product and patch source inputs.
    Sources,
    /// AROS-managed Cargo vendor generations.
    Cargo,
    /// Reusable GenMF reference expansions.
    Genmf,
}

impl CacheFamily {
    /// Stable lowercase label used by human-oriented result rendering.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Compiler => "compiler",
            Self::Archives => "archives",
            Self::Sources => "sources",
            Self::Cargo => "cargo",
            Self::Genmf => "genmf",
        }
    }
}

/// Why a family is or is not observable by the root cache-status command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheFamilyStatusKind {
    /// The family has a configured root or a passive backend observation.
    Configured,
    /// The family has no implicit root; a future operation must require an
    /// explicit caller selection rather than guessing.
    RequiresExplicitSelection,
}

impl CacheFamilyStatusKind {
    /// Stable lowercase label used by human-oriented result rendering.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Configured => "configured",
            Self::RequiresExplicitSelection => "requires explicit selection",
        }
    }
}

/// One bounded observation inside a cache-status document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CacheFamilyStatus {
    /// Cache family being described.
    pub family: CacheFamily,
    /// Whether this command knows a root/backend without guessing.
    pub status: CacheFamilyStatusKind,
    /// Operations the public interface currently supports for this family.
    pub capabilities: Vec<CacheCapability>,
    /// Existing root observation for the archive family.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root: Option<RootObservation>,
    /// Passive observations for supported compiler-cache backends.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub backends: Vec<CompilerBackendObservation>,
    /// Stable, human-readable boundary for an unconfigured family.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<&'static str>,
}

/// Versioned result of `aros cache status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CacheStatus {
    /// Versioned document identifier.
    pub schema: &'static str,
    /// Stable operation identifier for result consumers.
    pub operation: &'static str,
    /// Every observation in this document is passive.
    pub observation: &'static str,
    /// Guaranteed side effects of this operation.
    pub side_effects: CacheSideEffects,
    /// Per-family bounded observations.
    pub families: Vec<CacheFamilyStatus>,
}

/// Versioned result of `aros cache compiler status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompilerCacheStatus {
    /// Versioned document identifier.
    pub schema: &'static str,
    /// Stable operation identifier for result consumers.
    pub operation: &'static str,
    /// Every observation in this document is passive.
    pub observation: &'static str,
    /// Guaranteed side effects of this operation.
    pub side_effects: CacheSideEffects,
    /// Operations the public interface currently supports for compiler caches.
    pub capabilities: Vec<CacheCapability>,
    /// Backend requested for the status projection.
    pub requested_backend: CompilerBackendChoice,
    /// Backend selected from the observed executables, if available.
    pub selected_backend: Option<CompilerBackend>,
    /// Why a backend was selected for this status document.
    pub selection_basis: &'static str,
    /// Whether this passive status command observed a build's actual selection.
    pub effective_build_selection: &'static str,
    /// Candidate root selected only for this status observation.
    ///
    /// This does not configure or claim ownership of a backend's storage.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root: Option<RootObservation>,
    /// Boundary explaining how `root`, when present, relates to a backend.
    pub root_binding: &'static str,
    /// Passive observations of both supported backends.
    pub backends: Vec<CompilerBackendObservation>,
}

/// Build the passive cache-status document from the current environment.
///
/// # Errors
///
/// Returns an error only when the configured archive root cannot be resolved
/// safely, such as a relative `AROS_CACHE_DIR`. This operation does not create
/// directories, scan them, start a compiler-cache daemon, acquire locks or
/// contact the network.
pub fn cache_status() -> Result<CacheStatus, RootResolutionError> {
    cache_status_with(&CacheEnvironment::current())
}

/// Build a passive compiler-cache status document.
///
/// The request affects only which available backend is selected in the result;
/// an unavailable explicit backend is reported as missing, not converted to a
/// fallback or an error. No backend process or configuration file is queried.
///
/// # Errors
///
/// Returns an error only when an explicit root is relative or a selected
/// default AROS state root cannot be derived safely.
pub fn compiler_cache_status(
    requested_backend: CompilerBackendChoice,
    explicit_root: Option<PathBuf>,
) -> Result<CompilerCacheStatus, RootResolutionError> {
    let backends = observe_compiler_backends();
    let selected_backend = match requested_backend {
        CompilerBackendChoice::Auto => backends
            .iter()
            .find(|backend| backend.state == CompilerBackendState::Available)
            .map(|backend| backend.backend),
        CompilerBackendChoice::Sccache => backends
            .iter()
            .find(|backend| {
                backend.backend == CompilerBackend::Sccache
                    && backend.state == CompilerBackendState::Available
            })
            .map(|backend| backend.backend),
        CompilerBackendChoice::Ccache => backends
            .iter()
            .find(|backend| {
                backend.backend == CompilerBackend::Ccache
                    && backend.state == CompilerBackendState::Available
            })
            .map(|backend| backend.backend),
        CompilerBackendChoice::Off => None,
    };
    let root = match explicit_root {
        Some(path) => Some(observe_root(resolve_explicit_root(path)?)),
        None => match selected_backend {
            Some(backend) => Some(observe_root(resolve_default_compiler_cache_root(
                &CacheEnvironment::current(),
                backend,
            )?)),
            None => None,
        },
    };
    Ok(CompilerCacheStatus {
        schema: COMPILER_CACHE_STATUS_SCHEMA,
        operation: "compiler.status",
        observation: "passive",
        side_effects: PASSIVE_SIDE_EFFECTS,
        capabilities: vec![CacheCapability::Status],
        requested_backend,
        selected_backend,
        selection_basis: "executable_availability_only",
        effective_build_selection: "not_observed",
        root,
        root_binding: "status_only_not_applied",
        backends,
    })
}

fn cache_status_with(environment: &CacheEnvironment) -> Result<CacheStatus, RootResolutionError> {
    let archives = observe_root(resolve_archive_cache_root(environment)?);
    Ok(CacheStatus {
        schema: CACHE_STATUS_SCHEMA,
        operation: "status",
        observation: "passive",
        side_effects: PASSIVE_SIDE_EFFECTS,
        families: vec![
            CacheFamilyStatus {
                family: CacheFamily::Compiler,
                status: CacheFamilyStatusKind::Configured,
                capabilities: vec![CacheCapability::Status],
                root: None,
                backends: observe_compiler_backends(),
                detail: Some(
                    "backend executables and environment provenance only; no backend process or configuration file was queried",
                ),
            },
            CacheFamilyStatus {
                family: CacheFamily::Archives,
                status: CacheFamilyStatusKind::Configured,
                capabilities: vec![CacheCapability::Status],
                root: Some(archives),
                backends: Vec::new(),
                detail: Some(
                    "shared host and cross-compiler download cache; installed toolchains are outside this family",
                ),
            },
            unregistered(CacheFamily::Sources, "source and patch caches require an explicit reviewed lock and root; this command does not guess a producer or product cache"),
            unregistered(CacheFamily::Cargo, "Cargo vendor data requires an explicit tools checkout and managed root; global Cargo state is excluded"),
            unregistered(CacheFamily::Genmf, "GenMF expansions require an explicit expansion root and source selection; reports and work trees are excluded"),
        ],
    })
}

fn unregistered(family: CacheFamily, detail: &'static str) -> CacheFamilyStatus {
    CacheFamilyStatus {
        family,
        status: CacheFamilyStatusKind::RequiresExplicitSelection,
        capabilities: vec![CacheCapability::Status],
        root: None,
        backends: Vec::new(),
        detail: Some(detail),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        cache_status_with, compiler_cache_status, CacheFamily, CacheFamilyStatusKind,
        CACHE_STATUS_SCHEMA, COMPILER_CACHE_STATUS_SCHEMA,
    };
    use crate::{CacheEnvironment, CompilerBackendChoice, RootState};
    use std::ffi::OsString;

    #[test]
    fn cache_status_is_bounded_and_marks_unregistered_families_truthfully() {
        let temporary = tempfile::tempdir().unwrap();
        let environment = CacheEnvironment {
            archive_cache_dir: Some(temporary.path().join("missing").into_os_string()),
            ..CacheEnvironment::default()
        };
        let status = cache_status_with(&environment).unwrap();
        assert_eq!(status.schema, CACHE_STATUS_SCHEMA);
        assert_eq!(status.observation, "passive");
        assert_eq!(status.families.len(), 5);
        assert_eq!(status.families[1].family, CacheFamily::Archives);
        assert_eq!(
            status.families[1].root.as_ref().unwrap().state,
            RootState::Missing
        );
        for family in &status.families[2..] {
            assert_eq!(
                family.status,
                CacheFamilyStatusKind::RequiresExplicitSelection
            );
            assert!(family.root.is_none());
            assert!(family.backends.is_empty());
        }
    }

    #[test]
    fn relative_archive_override_fails_without_a_filesystem_observation() {
        let environment = CacheEnvironment {
            archive_cache_dir: Some(OsString::from("relative")),
            ..CacheEnvironment::default()
        };
        assert!(cache_status_with(&environment).is_err());
    }

    #[test]
    fn compiler_status_marks_its_daemon_safe_boundary_in_the_schema() {
        let status = compiler_cache_status(CompilerBackendChoice::Auto, None).unwrap();
        assert_eq!(status.schema, COMPILER_CACHE_STATUS_SCHEMA);
        assert_eq!(status.operation, "compiler.status");
        assert_eq!(status.observation, "passive");
        assert_eq!(status.requested_backend, CompilerBackendChoice::Auto);
        assert_eq!(status.backends.len(), 2);
    }

    #[test]
    fn unavailable_explicit_backend_serializes_a_null_selection() {
        let status = compiler_cache_status(CompilerBackendChoice::Sccache, None).unwrap();
        let document = serde_json::to_value(status).unwrap();
        assert!(document.get("selected_backend").is_some());
    }
}
