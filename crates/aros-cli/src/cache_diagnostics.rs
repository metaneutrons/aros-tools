//! Cache-lifecycle diagnostic boundaries shared by the CLI dispatcher.
//!
//! Keeping retention/removal recovery guidance outside `main` keeps the
//! parser and top-level error routing within their reviewed size boundary.

use aros_common::{DiagnosticCode, DiagnosticStage};

use crate::cache_command::{
    CacheArchivesCommand, CacheCargoCommand, CacheCommand, CacheCompilerBackend,
    CacheCompilerCommand, CacheGenmfCommand, CacheSourcesCommand, ManagedCompilerBackend,
};

/// Exact structured boundary returned for one cache lifecycle command.
pub type CacheLifecycleBoundary = (
    DiagnosticCode,
    DiagnosticStage,
    &'static str,
    Option<String>,
    &'static str,
);

/// Describe recovery for every cache command without starting a cache backend.
pub fn cache_boundary(command: &CacheCommand) -> CacheLifecycleBoundary {
    match command {
        CacheCommand::Status { .. } => (
            DiagnosticCode::CliConfiguration,
            DiagnosticStage::Configuration,
            "cache.status",
            None,
            "set AROS_HOME or AROS_CACHE_DIR to an absolute accessible path; cache status never creates or clears cache state",
        ),
        CacheCommand::Compiler {
            command: CacheCompilerCommand::Status { backend, dir, .. },
        } => {
            if dir.is_some() {
                (
                    DiagnosticCode::CliConfiguration,
                    DiagnosticStage::Configuration,
                    "cache.compiler.status",
                    None,
                    "pass an absolute --dir path; compiler-cache status observes it only and never configures a backend",
                )
            } else {
                (
                    DiagnosticCode::CliToolResolution,
                    DiagnosticStage::ToolResolution,
                    "cache.compiler.status",
                    Some(
                        match backend {
                            CacheCompilerBackend::Auto => "auto",
                            CacheCompilerBackend::Sccache => "sccache",
                            CacheCompilerBackend::Ccache => "ccache",
                        }
                        .to_owned(),
                    ),
                    "inspect the passive availability report; this command does not start a compiler-cache backend or alter its storage",
                )
            }
        }
        CacheCommand::Compiler {
            command: CacheCompilerCommand::Prepare { dir, .. },
        } => (
            DiagnosticCode::CliConfiguration,
            DiagnosticStage::Configuration,
            "cache.compiler.prepare",
            None,
            if dir.is_some() {
                "pass an absolute empty private --dir; preparation refuses foreign state and creates only a local AROS-managed compiler-cache namespace"
            } else {
                "set AROS_HOME to an absolute private state root, or pass an explicit empty private --dir; preparation refuses foreign state"
            },
        ),
        CacheCommand::Compiler {
            command: CacheCompilerCommand::Stats { backend, dir, .. },
        } => (
            DiagnosticCode::CliToolResolution,
            DiagnosticStage::ToolResolution,
            "cache.compiler.stats",
            Some(compiler_backend_name(*backend).to_owned()),
            if dir.is_some() {
                "pass an absolute prepared AROS-managed --dir and ensure the selected local backend is on PATH; statistics never select a foreign root or a remote backend, but the backend may materialize local metadata"
            } else {
                "prepare the selected backend's AROS_HOME namespace first and ensure its local backend is on PATH; statistics never adopt foreign state, but the backend may materialize metadata in its owned namespace"
            },
        ),
        CacheCommand::Compiler {
            command:
                CacheCompilerCommand::ResetStats {
                    backend,
                    dir,
                    apply,
                    ..
                },
        } => compiler_mutation_boundary(*backend, dir.as_ref(), apply.is_some(), "reset-stats"),
        CacheCommand::Compiler {
            command:
                CacheCompilerCommand::Clear {
                    backend,
                    dir,
                    apply,
                    ..
                },
        } => compiler_mutation_boundary(*backend, dir.as_ref(), apply.is_some(), "clear"),
        CacheCommand::Sources { command } => match command {
            CacheSourcesCommand::Status { .. } => (
                DiagnosticCode::CliConfiguration,
                DiagnosticStage::Configuration,
                "cache.sources.status",
                None,
                "pass an absolute source-cache --dir; status observes root metadata only and never creates cache state",
            ),
            CacheSourcesCommand::List { .. } => (
                DiagnosticCode::CliSourceInput,
                DiagnosticStage::Configuration,
                "cache.sources.list",
                None,
                "select one readable reviewed source lock or product plan and an existing real source-cache root; list never hashes or acquires payloads",
            ),
            CacheSourcesCommand::Fetch { offline, .. } => (
                if *offline {
                    DiagnosticCode::CliSourceLock
                } else {
                    DiagnosticCode::CliNetwork
                },
                DiagnosticStage::Configuration,
                "cache.sources.fetch",
                None,
                "restore the selected reviewed closure and cache entries; offline fetch never accesses the network and online fetch never replaces an existing object",
            ),
            CacheSourcesCommand::Verify { .. } => (
                DiagnosticCode::CliSourceLock,
                DiagnosticStage::Configuration,
                "cache.sources.verify",
                None,
                "restore the exact reviewed selector and cache objects; verify hashes payloads but never changes them",
            ),
            lifecycle @ (CacheSourcesCommand::Keep { .. }
            | CacheSourcesCommand::Release { .. }
            | CacheSourcesCommand::Remove { .. }) => source_lifecycle_boundary(lifecycle),
        },
        CacheCommand::Archives { command } => match command {
            CacheArchivesCommand::Status { .. } => (
                DiagnosticCode::CliConfiguration,
                DiagnosticStage::Configuration,
                "cache.archives.status",
                None,
                "set AROS_HOME or AROS_CACHE_DIR to an absolute accessible path; archive status observes root metadata only",
            ),
            CacheArchivesCommand::List { .. } => (
                DiagnosticCode::CliToolResolution,
                DiagnosticStage::Configuration,
                "cache.archives.list",
                None,
                "select one readable AROS checkout, archive purpose, and optional supported host; list reads cache-entry metadata without hashing or downloading it",
            ),
            CacheArchivesCommand::Fetch { offline, .. } => (
                if *offline {
                    DiagnosticCode::CliToolResolution
                } else {
                    DiagnosticCode::CliNetwork
                },
                DiagnosticStage::Configuration,
                "cache.archives.fetch",
                None,
                "restore the exact configured host/compiler archive; offline fetch requires an already verified cache object and refresh never replaces it",
            ),
            CacheArchivesCommand::Verify { .. } => (
                DiagnosticCode::CliToolResolution,
                DiagnosticStage::Configuration,
                "cache.archives.verify",
                None,
                "restore the exact configured archive bytes; verify checks only declared size and SHA-256, not extraction, installation, or attestation",
            ),
            lifecycle @ (CacheArchivesCommand::Keep { .. }
            | CacheArchivesCommand::Release { .. }
            | CacheArchivesCommand::Remove { .. }) => archive_lifecycle_boundary(lifecycle),
        },
        CacheCommand::Cargo { command } => match command {
            CacheCargoCommand::Status { .. } => (
                DiagnosticCode::CliConfiguration,
                DiagnosticStage::Configuration,
                "cache.cargo.status",
                None,
                "pass an absolute cache --dir; status observes only root metadata and never creates or scans Cargo generations",
            ),
            CacheCargoCommand::List { .. } => (
                DiagnosticCode::CliToolResolution,
                DiagnosticStage::Configuration,
                "cache.cargo.list",
                None,
                "select readable producer and tools checkouts, an absolute cache root and a pinned Cargo executable; list proves the selection with bounded Git and Cargo version probes, then reads only its generation receipt",
            ),
            CacheCargoCommand::Fetch { offline, .. } => (
                if *offline {
                    DiagnosticCode::CliToolResolution
                } else {
                    DiagnosticCode::CliNetwork
                },
                DiagnosticStage::Configuration,
                "cache.cargo.fetch",
                None,
                "use cache cargo fetch before an offline native producer build; offline mode requires a fully verified immutable generation",
            ),
            CacheCargoCommand::Verify { .. } => (
                DiagnosticCode::CliSourceLock,
                DiagnosticStage::Configuration,
                "cache.cargo.verify",
                None,
                "restore the exact producer pin, tools lock and Cargo executable; verify hashes vendor content but never resolves, downloads or rewrites it",
            ),
            lifecycle @ (CacheCargoCommand::Keep { .. }
            | CacheCargoCommand::Release { .. }
            | CacheCargoCommand::Remove { .. }) => cargo_lifecycle_boundary(lifecycle),
        },
        CacheCommand::Genmf { command } => match command {
            CacheGenmfCommand::Status { .. } => (
                DiagnosticCode::CliConfiguration,
                DiagnosticStage::Configuration,
                "cache.genmf.status",
                None,
                "pass an absolute cache --dir; status observes only root metadata and never selects source inputs or creates a generation",
            ),
            CacheGenmfCommand::List { .. } => (
                DiagnosticCode::CliSourceInput,
                DiagnosticStage::Configuration,
                "cache.genmf.list",
                None,
                "pass existing no-follow --source-dir and --dir roots plus an absolute Python interpreter when PATH does not select the intended one; list reads no expansion payload and starts no generator",
            ),
            CacheGenmfCommand::Verify { .. } => (
                DiagnosticCode::CliSourceLock,
                DiagnosticStage::Configuration,
                "cache.genmf.verify",
                None,
                "restore the exact source/template/generator/interpreter selection and its immutable generation; verify makes only its bounded Python version probe and never invokes GenMF or repairs cache state",
            ),
            CacheGenmfCommand::Refresh { .. } => (
                DiagnosticCode::CliBuild,
                DiagnosticStage::BuildExecution,
                "cache.genmf.refresh",
                None,
                "inspect the selected upstream GenMF inputs and Python interpreter; refresh publishes only missing complete generations and rejects any existing byte mismatch without replacement",
            ),
            lifecycle @ (CacheGenmfCommand::Keep { .. }
            | CacheGenmfCommand::Release { .. }
            | CacheGenmfCommand::Remove { .. }) => genmf_lifecycle_boundary(lifecycle),
        },
    }
}

const fn compiler_backend_name(backend: ManagedCompilerBackend) -> &'static str {
    match backend {
        ManagedCompilerBackend::Sccache => "sccache",
        ManagedCompilerBackend::Ccache => "ccache",
    }
}

fn compiler_mutation_boundary(
    backend: ManagedCompilerBackend,
    dir: Option<&std::path::PathBuf>,
    applying: bool,
    operation: &'static str,
) -> CacheLifecycleBoundary {
    (
        if applying {
            DiagnosticCode::CliPublication
        } else {
            DiagnosticCode::CliToolResolution
        },
        if applying {
            DiagnosticStage::Publication
        } else {
            DiagnosticStage::ToolResolution
        },
        match operation {
            "reset-stats" => "cache.compiler.reset_stats",
            "clear" => "cache.compiler.clear",
            _ => "cache.compiler.mutation",
        },
        Some(match backend {
            ManagedCompilerBackend::Sccache => "sccache".to_owned(),
            ManagedCompilerBackend::Ccache => "ccache".to_owned(),
        }),
        if applying {
            "run the matching preview again if its token expired or its owned root/configuration/data binding changed; apply uses one exclusive local lease and never falls back to foreign or remote storage"
        } else if dir.is_some() {
            "pass an absolute prepared AROS-managed --dir; inspect the preview and return its exact unexpired --apply token only after confirming the selected local namespace"
        } else {
            "prepare the selected backend's AROS_HOME namespace first; inspect the preview and return its exact unexpired --apply token only after confirming the selected local namespace"
        },
    )
}

/// Describe recovery for archive retention and preview/apply removal.
pub fn archive_lifecycle_boundary(command: &CacheArchivesCommand) -> CacheLifecycleBoundary {
    match command {
        CacheArchivesCommand::Keep { .. } => (
            DiagnosticCode::CliPublication,
            DiagnosticStage::Publication,
            "cache.archives.keep",
            None,
            "select one declared archive and an unused portable --name; keep creates a named retention receipt but never transfers or removes bytes",
        ),
        CacheArchivesCommand::Release { apply, .. } => (
            if apply.is_some() {
                DiagnosticCode::CliPublication
            } else {
                DiagnosticCode::CliToolResolution
            },
            DiagnosticStage::Publication,
            "cache.archives.release",
            None,
            "run the receipt preview first and pass its exact unexpired --apply token only after confirming that releasing this named reference is intended; release never deletes archive bytes",
        ),
        CacheArchivesCommand::Remove { apply, .. } => (
            if apply.is_some() {
                DiagnosticCode::CliPublication
            } else {
                DiagnosticCode::CliToolResolution
            },
            DiagnosticStage::Publication,
            "cache.archives.remove",
            None,
            "run the preview first and pass its exact unexpired --apply token only after checking blockers; removal never scans or clears a cache root",
        ),
        CacheArchivesCommand::Status { .. }
        | CacheArchivesCommand::List { .. }
        | CacheArchivesCommand::Fetch { .. }
        | CacheArchivesCommand::Verify { .. } => {
            unreachable!("only archive lifecycle commands call archive_lifecycle_boundary")
        }
    }
}

/// Describe recovery for source retention and role-selected removal.
pub fn source_lifecycle_boundary(command: &CacheSourcesCommand) -> CacheLifecycleBoundary {
    match command {
        CacheSourcesCommand::Keep { .. } => (
            DiagnosticCode::CliSourceLock,
            DiagnosticStage::Configuration,
            "cache.sources.keep",
            None,
            "select one verified reviewed source closure, an existing private cache root, and a fresh portable --name; keep never downloads, replaces, or deletes source bytes",
        ),
        CacheSourcesCommand::Release { apply, .. } => (
            if apply.is_some() {
                DiagnosticCode::CliPublication
            } else {
                DiagnosticCode::CliConfiguration
            },
            DiagnosticStage::Configuration,
            "cache.sources.release",
            None,
            "run the receipt preview first and pass its exact unexpired --apply token only after confirming that releasing the named reference is intended; release never deletes source-cache payload bytes",
        ),
        CacheSourcesCommand::Remove { .. } => (
            DiagnosticCode::CliSourceLock,
            DiagnosticStage::Configuration,
            "cache.sources.remove",
            None,
            "select the same reviewed closure and one exact semantic --role; inspect the preview first, then pass its short-lived --apply token only when retention is released and no active consumer holds the object",
        ),
        CacheSourcesCommand::Status { .. }
        | CacheSourcesCommand::List { .. }
        | CacheSourcesCommand::Fetch { .. }
        | CacheSourcesCommand::Verify { .. } => {
            unreachable!("only source lifecycle commands call source_lifecycle_boundary")
        }
    }
}

/// Describe recovery for Cargo retention and preview/apply removal.
pub fn cargo_lifecycle_boundary(command: &CacheCargoCommand) -> CacheLifecycleBoundary {
    match command {
        CacheCargoCommand::Keep { .. } => (
            DiagnosticCode::CliPublication,
            DiagnosticStage::Publication,
            "cache.cargo.keep",
            None,
            "select one fully verified Cargo vendor generation and an unused portable --name; keep retains it but never invokes Cargo, rewrites inputs, or removes data",
        ),
        CacheCargoCommand::Release { apply, .. } => (
            if apply.is_some() {
                DiagnosticCode::CliPublication
            } else {
                DiagnosticCode::CliToolResolution
            },
            DiagnosticStage::Publication,
            "cache.cargo.release",
            None,
            "run the receipt preview first and pass its exact unexpired --apply token only after confirming that releasing the named reference is intended; release never deletes Cargo vendor data",
        ),
        CacheCargoCommand::Remove { apply, .. } => (
            if apply.is_some() {
                DiagnosticCode::CliPublication
            } else {
                DiagnosticCode::CliToolResolution
            },
            DiagnosticStage::Publication,
            "cache.cargo.remove",
            None,
            "run the preview first and pass its exact unexpired --apply token only after checking blockers; removal never clears a Cargo cache root or global CARGO_HOME",
        ),
        CacheCargoCommand::Status { .. }
        | CacheCargoCommand::List { .. }
        | CacheCargoCommand::Fetch { .. }
        | CacheCargoCommand::Verify { .. } => {
            unreachable!("only Cargo lifecycle commands call cargo_lifecycle_boundary")
        }
    }
}

/// Describe recovery for GenMF retention and input-selected removal.
pub fn genmf_lifecycle_boundary(command: &CacheGenmfCommand) -> CacheLifecycleBoundary {
    match command {
        CacheGenmfCommand::Keep { .. } => (
            DiagnosticCode::CliPublication,
            DiagnosticStage::Publication,
            "cache.genmf.keep",
            None,
            "select all current fully verified GenMF generations and an unused portable --name; keep retains the closed selection but never invokes GenMF, replaces bytes, or removes data",
        ),
        CacheGenmfCommand::Release { apply, .. } => (
            if apply.is_some() {
                DiagnosticCode::CliPublication
            } else {
                DiagnosticCode::CliSourceInput
            },
            DiagnosticStage::Publication,
            "cache.genmf.release",
            None,
            "run the receipt preview first and pass its exact unexpired --apply token only after confirming that releasing the named reference is intended; release never deletes GenMF expansion data",
        ),
        CacheGenmfCommand::Remove { apply, .. } => (
            if apply.is_some() {
                DiagnosticCode::CliPublication
            } else {
                DiagnosticCode::CliSourceInput
            },
            DiagnosticStage::Publication,
            "cache.genmf.remove",
            None,
            "select one exact current source-root-relative --source input, run the preview, and pass its exact unexpired --apply token only after checking blockers; removal never clears or scans a GenMF cache root",
        ),
        CacheGenmfCommand::Status { .. }
        | CacheGenmfCommand::List { .. }
        | CacheGenmfCommand::Verify { .. }
        | CacheGenmfCommand::Refresh { .. } => {
            unreachable!("only GenMF lifecycle commands call genmf_lifecycle_boundary")
        }
    }
}
