//! Managed local compiler-cache commands and rendering.

use std::fs;
use std::os::unix::fs::FileTypeExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use aros_cache::{
    CacheCapability, CacheFamily, CacheFamilyStatus, CacheFamilyStatusKind, CacheSideEffects,
    CompilerBackend, CompilerBackendChoice, CompilerCacheManagedRoot, CompilerCacheMutation,
    CompilerCacheMutationOperation, CompilerCacheMutationPreview, CompilerCacheMutationRequest,
    CompilerCacheMutationResult, CompilerCachePreparation, CompilerCacheStatus,
};
use miette::Result;
use serde::Serialize;

use crate::observability;
use crate::toolchain_management::ResultFormat;

const COMPILER_STATS_SCHEMA: &str = "aros-cache-compiler-stats-v1";
const COMPILER_RESET_PREVIEW_SCHEMA: &str = "aros-cache-compiler-reset-stats-preview-v1";
const COMPILER_RESET_SCHEMA: &str = "aros-cache-compiler-reset-stats-v1";
const COMPILER_CLEAR_PREVIEW_SCHEMA: &str = "aros-cache-compiler-clear-preview-v1";
const COMPILER_CLEAR_SCHEMA: &str = "aros-cache-compiler-clear-v1";
const COMPILER_BACKEND_TIMEOUT: Duration = Duration::from_secs(10);
const MINIMUM_CCACHE_VERSION: (u64, u64, u64) = (4, 14, 0);
const MINIMUM_SCCACHE_VERSION: (u64, u64, u64) = (0, 17, 0);

#[derive(Serialize)]
struct CompilerCacheStatsReport {
    schema: &'static str,
    operation: &'static str,
    side_effects: CacheSideEffects,
    backend: CompilerBackend,
    backend_version: String,
    cache_root: PathBuf,
    data_root: PathBuf,
    backend_report: serde_json::Value,
    boundary: &'static str,
}

#[derive(Serialize)]
struct CompilerCacheMutationPreviewReport {
    schema: &'static str,
    operation: &'static str,
    side_effects: CacheSideEffects,
    preview: CompilerCacheMutationPreview,
    boundary: &'static str,
}

#[derive(Serialize)]
struct CompilerCacheMutationAppliedReport {
    schema: &'static str,
    operation: &'static str,
    side_effects: CacheSideEffects,
    backend_version: String,
    result: CompilerCacheMutationResult,
    boundary: &'static str,
}

/// Render the passive compiler-cache backend projection.
pub fn status(
    backend: CompilerBackendChoice,
    dir: Option<PathBuf>,
    format: ResultFormat,
) -> Result<()> {
    let report =
        aros_cache::compiler_cache_status(backend, dir).map_err(|error| miette::miette!(error))?;
    match format {
        ResultFormat::Human => print_status_human(&report),
        ResultFormat::Json => super::print_json(&report, "compiler cache status")?,
    }
    Ok(())
}

/// Prepare and render one owned local compiler-cache namespace.
pub fn prepare(backend: CompilerBackend, dir: Option<PathBuf>, format: ResultFormat) -> Result<()> {
    let dir = match dir {
        Some(dir) => dir,
        None => {
            aros_cache::resolve_default_compiler_cache_root(
                &aros_cache::CacheEnvironment::current(),
                backend,
            )
            .map_err(|error| miette::miette!(error))?
            .path
        }
    };
    let report = aros_cache::prepare_managed_compiler_cache(backend, dir)
        .map_err(|error| miette::miette!(error))?;
    match format {
        ResultFormat::Human => print_prepare_human(&report),
        ResultFormat::Json => super::print_json(&report, "compiler cache preparation")?,
    }
    Ok(())
}

/// Query bounded statistics from one verified local compiler-cache namespace.
pub fn stats(backend: CompilerBackend, dir: Option<PathBuf>, format: ResultFormat) -> Result<()> {
    let root = managed_root(backend, dir)?;
    let _lease =
        aros_cache::acquire_compiler_cache_build(&root).map_err(|error| miette::miette!(error))?;
    let backend_version = verified_backend_version(&root)?;
    let output = run_backend(&root, statistics_arguments(backend, format))?;
    match format {
        ResultFormat::Human => {
            aros_common::outputln!(
                "Managed {} cache statistics ({})",
                backend.program(),
                root.root().display()
            );
            aros_common::outputln!("  backend version: {backend_version}");
            aros_common::outputln!("{output}");
        }
        ResultFormat::Json => {
            let backend_report = serde_json::from_str(&output).map_err(|error| {
                miette::miette!(
                    "managed {} statistics were not valid JSON: {error}",
                    backend.program()
                )
            })?;
            let report = CompilerCacheStatsReport {
                schema: COMPILER_STATS_SCHEMA,
                operation: "compiler.stats",
                side_effects: stats_side_effects(),
                backend,
                backend_version,
                cache_root: root.root().to_path_buf(),
                data_root: root.data_root().to_path_buf(),
                backend_report,
                boundary: "statistics query is limited to one prepared AROS-owned local namespace and holds a shared lifecycle lease; sccache may start only its private configured server",
            };
            super::print_json(&report, "compiler cache statistics")?;
        }
    }
    Ok(())
}

/// Preview or token-confirm a managed compiler-cache counter reset.
pub fn reset_stats(
    backend: CompilerBackend,
    dir: Option<PathBuf>,
    apply_token: Option<&str>,
    format: ResultFormat,
) -> Result<()> {
    mutation(
        backend,
        dir,
        apply_token,
        format,
        CompilerCacheMutationOperation::ResetStats,
    )
}

/// Preview or token-confirm a managed local compiler-cache clear.
pub fn clear(
    backend: CompilerBackend,
    dir: Option<PathBuf>,
    apply_token: Option<&str>,
    format: ResultFormat,
) -> Result<()> {
    mutation(
        backend,
        dir,
        apply_token,
        format,
        CompilerCacheMutationOperation::Clear,
    )
}

fn mutation(
    backend: CompilerBackend,
    dir: Option<PathBuf>,
    apply_token: Option<&str>,
    format: ResultFormat,
    operation: CompilerCacheMutationOperation,
) -> Result<()> {
    let request = CompilerCacheMutationRequest {
        backend,
        cache_root: managed_root(backend, dir)?.root().to_path_buf(),
    };
    if let Some(apply_token) = apply_token {
        let mut mutation =
            aros_cache::begin_compiler_cache_mutation(&request, operation, apply_token)
                .map_err(|error| miette::miette!(error))?;
        let backend_version = verified_backend_version(mutation.root())?;
        let result = match apply_mutation(&mut mutation, operation) {
            Ok(()) => mutation.finish().map_err(|error| miette::miette!(error)),
            Err(error) => Err(error),
        };
        let result = observability::commit_state(
            result,
            aros_common::CommitState::Indeterminate,
            "compiler-cache backend operation may have changed local state before failing",
        )?;
        observability::record_committed_mutation();
        let report = CompilerCacheMutationAppliedReport {
            schema: match operation {
                CompilerCacheMutationOperation::ResetStats => COMPILER_RESET_SCHEMA,
                CompilerCacheMutationOperation::Clear => COMPILER_CLEAR_SCHEMA,
            },
            operation: mutation_operation_name(operation),
            side_effects: mutation_apply_side_effects(operation),
            backend_version,
            result,
            boundary: mutation_boundary(operation),
        };
        match format {
            ResultFormat::Human => print_mutation_applied_human(&report),
            ResultFormat::Json => super::print_json(&report, "compiler cache mutation")?,
        }
    } else {
        let preview = match operation {
            CompilerCacheMutationOperation::ResetStats => {
                aros_cache::preview_compiler_cache_reset_stats(&request)
            }
            CompilerCacheMutationOperation::Clear => {
                aros_cache::preview_compiler_cache_clear(&request)
            }
        }
        .map_err(|error| miette::miette!(error))?;
        let report = CompilerCacheMutationPreviewReport {
            schema: match operation {
                CompilerCacheMutationOperation::ResetStats => COMPILER_RESET_PREVIEW_SCHEMA,
                CompilerCacheMutationOperation::Clear => COMPILER_CLEAR_PREVIEW_SCHEMA,
            },
            operation: mutation_preview_operation_name(operation),
            side_effects: mutation_preview_side_effects(operation),
            preview,
            boundary: mutation_boundary(operation),
        };
        match format {
            ResultFormat::Human => print_mutation_preview_human(&report),
            ResultFormat::Json => super::print_json(&report, "compiler cache mutation preview")?,
        }
    }
    Ok(())
}

fn managed_root(
    backend: CompilerBackend,
    dir: Option<PathBuf>,
) -> Result<CompilerCacheManagedRoot> {
    let root = match dir {
        Some(dir) => dir,
        None => {
            aros_cache::resolve_default_compiler_cache_root(
                &aros_cache::CacheEnvironment::current(),
                backend,
            )
            .map_err(|error| miette::miette!(error))?
            .path
        }
    };
    aros_cache::load_managed_compiler_cache(backend, root).map_err(|error| miette::miette!(error))
}

fn run_backend(root: &CompilerCacheManagedRoot, arguments: &[&str]) -> Result<String> {
    let executable = root.executable().map_err(|error| miette::miette!(error))?;
    let environment =
        aros_cache::compiler_cache_environment(root).map_err(|error| miette::miette!(error))?;
    let mut command = Command::new(executable);
    environment.apply_to(&mut command);
    command.args(arguments);
    observability::capture_stdout_with_timeout(
        &mut command,
        &format!("managed {} cache operation", root.backend().program()),
        COMPILER_BACKEND_TIMEOUT,
    )
    .map_err(miette::Report::new)
}

fn verified_backend_version(root: &CompilerCacheManagedRoot) -> Result<String> {
    let output = run_backend(root, &["--version"])?;
    let version = parse_backend_version(&output).ok_or_else(|| {
        miette::miette!(
            "managed {} did not report a parseable semantic version",
            root.backend().program()
        )
    })?;
    let minimum = match root.backend() {
        CompilerBackend::Ccache => MINIMUM_CCACHE_VERSION,
        CompilerBackend::Sccache => MINIMUM_SCCACHE_VERSION,
    };
    if version < minimum {
        return Err(miette::miette!(
            "managed {} version {}.{}.{} is below the qualified minimum {}.{}.{}",
            root.backend().program(),
            version.0,
            version.1,
            version.2,
            minimum.0,
            minimum.1,
            minimum.2,
        ));
    }
    Ok(format!("{}.{}.{}", version.0, version.1, version.2))
}

fn parse_backend_version(output: &str) -> Option<(u64, u64, u64)> {
    output
        .split(|character: char| !(character.is_ascii_digit() || character == '.'))
        .find_map(|candidate| {
            let mut components = candidate.split('.');
            let major = components.next()?.parse().ok()?;
            let minor = components.next()?.parse().ok()?;
            let patch = components
                .next()
                .map_or(Some(0), |component| component.parse().ok())?;
            (components.next().is_none()).then_some((major, minor, patch))
        })
}

const fn statistics_arguments(
    backend: CompilerBackend,
    format: ResultFormat,
) -> &'static [&'static str] {
    match (backend, format) {
        (_, ResultFormat::Human) => &["--show-stats"],
        (CompilerBackend::Ccache, ResultFormat::Json) => &["--format", "json", "--print-stats"],
        (CompilerBackend::Sccache, ResultFormat::Json) => {
            &["--show-stats", "--stats-format", "json"]
        }
    }
}

const fn mutation_operation_name(operation: CompilerCacheMutationOperation) -> &'static str {
    match operation {
        CompilerCacheMutationOperation::ResetStats => "compiler.reset_stats.apply",
        CompilerCacheMutationOperation::Clear => "compiler.clear.apply",
    }
}

const fn mutation_preview_operation_name(
    operation: CompilerCacheMutationOperation,
) -> &'static str {
    match operation {
        CompilerCacheMutationOperation::ResetStats => "compiler.reset_stats.preview",
        CompilerCacheMutationOperation::Clear => "compiler.clear.preview",
    }
}

const fn stats_side_effects() -> CacheSideEffects {
    CacheSideEffects {
        creates_state: true,
        mutates_state: true,
        network: false,
        backend_process: true,
        locks: true,
        hashes_payloads: false,
    }
}

const fn mutation_preview_side_effects(
    operation: CompilerCacheMutationOperation,
) -> CacheSideEffects {
    CacheSideEffects {
        creates_state: false,
        mutates_state: false,
        network: false,
        backend_process: false,
        locks: false,
        hashes_payloads: matches!(operation, CompilerCacheMutationOperation::Clear),
    }
}

const fn mutation_apply_side_effects(
    operation: CompilerCacheMutationOperation,
) -> CacheSideEffects {
    CacheSideEffects {
        creates_state: true,
        mutates_state: true,
        network: false,
        backend_process: true,
        locks: true,
        hashes_payloads: matches!(operation, CompilerCacheMutationOperation::Clear),
    }
}

const fn mutation_boundary(operation: CompilerCacheMutationOperation) -> &'static str {
    match operation {
        CompilerCacheMutationOperation::ResetStats => {
            "preview is non-mutating; apply requires its exact unexpired token, holds an exclusive lease, and invokes only the selected backend counter reset in its AROS-owned local namespace"
        }
        CompilerCacheMutationOperation::Clear => {
            "preview measures only the selected AROS-owned local data tree; apply requires its exact unexpired token, holds an exclusive lease, and never falls back to foreign or remote storage"
        }
    }
}

fn apply_mutation(
    mutation: &mut CompilerCacheMutation,
    operation: CompilerCacheMutationOperation,
) -> Result<()> {
    match (mutation.root().backend(), operation) {
        (_, CompilerCacheMutationOperation::ResetStats) => {
            let _ = run_backend(mutation.root(), &["--zero-stats"])?;
        }
        (CompilerBackend::Ccache, CompilerCacheMutationOperation::Clear) => {
            let _ = run_backend(mutation.root(), &["--clear"])?;
        }
        (CompilerBackend::Sccache, CompilerCacheMutationOperation::Clear) => {
            stop_private_sccache_server(mutation)?;
            mutation
                .clear_sccache_data()
                .map_err(|error| miette::miette!(error))?;
        }
    }
    Ok(())
}

fn stop_private_sccache_server(mutation: &mut CompilerCacheMutation) -> Result<()> {
    let socket = mutation.root().root().join("server.sock");
    let metadata = match fs::symlink_metadata(&socket) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(miette::miette!(
                "cannot inspect managed sccache server socket '{}': {error}",
                socket.display()
            ))
        }
    };
    if metadata.file_type().is_symlink() || !metadata.file_type().is_socket() {
        return Err(miette::miette!(
            "managed sccache server path '{}' is not a real Unix-domain socket; refusing clear",
            socket.display()
        ));
    }
    let _ = run_backend(mutation.root(), &["--stop-server"])?;
    mutation
        .remove_stopped_sccache_socket()
        .map_err(|error| miette::miette!(error))
}

fn print_status_human(report: &CompilerCacheStatus) {
    let selected = report
        .selected_backend
        .map_or_else(|| "none".to_owned(), |backend| backend.program().to_owned());
    aros_common::outputln!(
        "Compiler cache status (passive; requested {}, selected {}):",
        match report.requested_backend {
            CompilerBackendChoice::Auto => "auto",
            CompilerBackendChoice::Off => "off",
            CompilerBackendChoice::Sccache => "sccache",
            CompilerBackendChoice::Ccache => "ccache",
        },
        selected
    );
    if let Some(root) = &report.root {
        aros_common::outputln!(
            "  candidate root: {} ({}, {}; status only, not applied)",
            root.root.path.display(),
            root.root.origin.as_str(),
            root.state.as_str(),
        );
    }
    let family = CacheFamilyStatus {
        family: CacheFamily::Compiler,
        status: CacheFamilyStatusKind::Configured,
        capabilities: vec![
            CacheCapability::Status,
            CacheCapability::Prepare,
            CacheCapability::Stats,
            CacheCapability::ResetStats,
            CacheCapability::Clear,
        ],
        root: None,
        backends: report.backends.clone(),
        detail: Some(
            "no backend process or configuration file was queried; storage scope remains uninspected",
        ),
    };
    super::status::print_backend_details(&family);
    aros_common::outputln!("  boundary: {}", family.detail.unwrap());
}

fn print_prepare_human(report: &CompilerCachePreparation) {
    aros_common::outputln!(
        "Prepared local {} cache at {} ({})",
        report.backend.program(),
        report.cache_root.display(),
        report.ownership
    );
    aros_common::outputln!("  data root: {}", report.data_root.display());
    aros_common::outputln!(
        "  next: pass --compiler-cache {} --compiler-cache-dir {} to aros build or aros board build",
        report.backend.program(), report.cache_root.display()
    );
}

fn print_mutation_preview_human(report: &CompilerCacheMutationPreviewReport) {
    let preview = &report.preview;
    aros_common::outputln!(
        "{} preview for managed {} cache:",
        match preview.operation {
            "compiler.reset_stats" => "Reset statistics",
            "compiler.clear" => "Clear",
            _ => "Compiler-cache mutation",
        },
        preview.backend.program(),
    );
    aros_common::outputln!("  root: {}", preview.cache_root.display());
    aros_common::outputln!("  data root: {}", preview.data_root.display());
    if let Some(scope) = &preview.data_scope {
        aros_common::outputln!(
            "  selected data: {} entries, {} bytes (bounded at {} entries / {} bytes)",
            scope.entry_count,
            scope.regular_file_bytes,
            scope.max_entries,
            scope.max_regular_file_bytes,
        );
        aros_common::outputln!("  payload SHA-256: {}", scope.payload_sha256);
    }
    aros_common::outputln!("  recovery: {}", preview.recovery);
    aros_common::outputln!(
        "  apply before {}: aros cache compiler {} --backend {} --dir {} --apply {}",
        preview.expires_unix_seconds,
        match preview.operation {
            "compiler.reset_stats" => "reset-stats",
            "compiler.clear" => "clear",
            _ => "<operation>",
        },
        preview.backend.program(),
        preview.cache_root.display(),
        preview.apply_token,
    );
}

fn print_mutation_applied_human(report: &CompilerCacheMutationAppliedReport) {
    aros_common::outputln!(
        "{} completed for managed {} cache at {}",
        match report.result.operation {
            "compiler.reset_stats" => "Statistics reset",
            "compiler.clear" => "Clear",
            _ => "Compiler-cache mutation",
        },
        report.result.backend.program(),
        report.result.cache_root.display(),
    );
    aros_common::outputln!("  backend version: {}", report.backend_version);
    aros_common::outputln!("  data root: {}", report.result.data_root.display());
    aros_common::outputln!("  outcome: {}", report.result.outcome);
    aros_common::outputln!("  boundary: {}", report.boundary);
}
