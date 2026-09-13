//! Parser model for the resource-oriented cache command family.

use std::path::PathBuf;

use clap::{ArgGroup, Args, Subcommand, ValueEnum};

use crate::toolchain_management::ResultFormat;

/// Cache resources exposed by the public CLI.
#[derive(Subcommand)]
pub enum CacheCommand {
    /// Show the bounded passive status of every cache family.
    Status {
        /// Result representation on stdout, independent of diagnostic format.
        #[arg(long, value_enum, default_value = "human")]
        format: ResultFormat,
    },
    /// Inspect compiler-cache backend selection without querying a backend.
    Compiler {
        /// One compiler-cache subcommand.
        #[command(subcommand)]
        command: CacheCompilerCommand,
    },
    /// Inspect, verify, and populate explicit reviewed source-cache closures.
    Sources {
        /// One source-cache subcommand.
        #[command(subcommand)]
        command: CacheSourcesCommand,
    },
    /// Inspect, verify, and populate host and cross-compiler archive bytes.
    Archives {
        /// One compiler-archive subcommand.
        #[command(subcommand)]
        command: CacheArchivesCommand,
    },
}

/// Compiler-cache resource operations.
#[derive(Subcommand)]
pub enum CacheCompilerCommand {
    /// Show passive compiler-cache backend availability and configuration provenance.
    Status {
        /// Backend projection; auto reports the stable sccache-first choice.
        #[arg(long, value_enum, default_value = "auto")]
        backend: CacheCompilerBackend,

        /// Explicit absolute compiler-cache root to inspect without configuring a backend.
        #[arg(long, value_name = "DIR")]
        dir: Option<PathBuf>,

        /// Result representation on stdout, independent of diagnostic format.
        #[arg(long, value_enum, default_value = "human")]
        format: ResultFormat,
    },
}

/// Source-cache resource operations.
#[derive(Subcommand)]
pub enum CacheSourcesCommand {
    /// Passively observe one explicit source-cache root.
    Status {
        /// Existing or missing absolute source-cache root to observe.
        #[arg(long, value_name = "DIR")]
        dir: PathBuf,

        /// Result representation on stdout, independent of diagnostic format.
        #[arg(long, value_enum, default_value = "human")]
        format: ResultFormat,
    },
    /// List selector-declared source entries without hashing payload content.
    List {
        /// One exact reviewed source-cache selector.
        #[command(flatten)]
        selector: CacheSourceSelector,

        /// Existing real absolute source-cache root.
        #[arg(long, value_name = "DIR")]
        dir: PathBuf,

        /// Result representation on stdout, independent of diagnostic format.
        #[arg(long, value_enum, default_value = "human")]
        format: ResultFormat,
    },
    /// Acquire missing selector-declared source entries and verify the closure.
    Fetch {
        /// One exact reviewed source-cache selector.
        #[command(flatten)]
        selector: CacheSourceSelector,

        /// Existing real absolute source-cache root.
        #[arg(long, value_name = "DIR")]
        dir: PathBuf,

        /// Refuse every network transfer and report exact selected cache misses.
        #[arg(long, env = "AROS_OFFLINE")]
        offline: bool,

        /// Permit a reviewed product plan with explicitly unverified origins.
        #[arg(
            long,
            requires = "source_fetch_plan",
            conflicts_with_all = ["source_lock", "compatibility_ports_lock"]
        )]
        allow_unverified: bool,

        /// Result representation on stdout, independent of diagnostic format.
        #[arg(long, value_enum, default_value = "human")]
        format: ResultFormat,
    },
    /// Hash and verify every strict selector-declared source entry.
    Verify {
        /// One exact reviewed source-cache selector.
        #[command(flatten)]
        selector: CacheSourceSelector,

        /// Existing real absolute source-cache root.
        #[arg(long, value_name = "DIR")]
        dir: PathBuf,

        /// Result representation on stdout, independent of diagnostic format.
        #[arg(long, value_enum, default_value = "human")]
        format: ResultFormat,
    },
}

/// Compiler-archive resource operations.
#[derive(Subcommand)]
pub enum CacheArchivesCommand {
    /// Passively observe the shared compiler-archive cache root.
    Status {
        /// Result representation on stdout, independent of diagnostic format.
        #[arg(long, value_enum, default_value = "human")]
        format: ResultFormat,
    },
    /// List one selected archive's direct cache-entry metadata without hashing bytes.
    List {
        /// One exact configured host or locked cross-toolchain archive.
        #[command(flatten)]
        selector: CacheArchiveSelector,

        /// Result representation on stdout, independent of diagnostic format.
        #[arg(long, value_enum, default_value = "human")]
        format: ResultFormat,
    },
    /// Acquire and verify one selected archive without extracting or installing it.
    Fetch {
        /// One exact configured host or locked cross-toolchain archive.
        #[command(flatten)]
        selector: CacheArchiveSelector,

        /// Refuse every network transfer and require an already verified archive.
        #[arg(long, env = "AROS_OFFLINE", conflicts_with = "refresh")]
        offline: bool,

        /// Reacquire the same declared identity without replacing a cached object.
        #[arg(long, conflicts_with = "offline")]
        refresh: bool,

        /// Result representation on stdout, independent of diagnostic format.
        #[arg(long, value_enum, default_value = "human")]
        format: ResultFormat,
    },
    /// Hash and verify one selected archive's declared byte identity only.
    Verify {
        /// One exact configured host or locked cross-toolchain archive.
        #[command(flatten)]
        selector: CacheArchiveSelector,

        /// Result representation on stdout, independent of diagnostic format.
        #[arg(long, value_enum, default_value = "human")]
        format: ResultFormat,
    },
}

/// One exclusive configured archive declaration.
#[derive(Args)]
#[command(group(
    ArgGroup::new("archive_selector")
        .required(true)
        .multiple(false)
        .args(["host_compiler", "toolchain"])
))]
pub struct CacheArchiveSelector {
    /// AROS checkout carrying the configuration required by the selected archive.
    #[arg(long, value_name = "DIR")]
    pub(crate) project: PathBuf,

    /// Use the configured host LLVM compiler archive.
    #[arg(long)]
    pub(crate) host_compiler: bool,

    /// Use one locked AROS cross-toolchain archive; requires --preset.
    #[arg(long, requires = "preset")]
    pub(crate) toolchain: bool,

    /// Locked AROS target preset selected with --toolchain.
    #[arg(long, value_name = "NAME", requires = "toolchain")]
    pub(crate) preset: Option<String>,

    /// Host release-matrix key; defaults to the running host when omitted.
    #[arg(long, value_name = "HOST")]
    pub(crate) host: Option<String>,
}

/// One exclusive reviewed declaration that selects a source-cache closure.
#[derive(Args)]
#[command(group(
    ArgGroup::new("source_selector")
        .required(true)
        .multiple(false)
        .args(["source_lock", "compatibility_ports_lock", "source_fetch_plan"])
))]
pub struct CacheSourceSelector {
    /// Reviewed native toolchain producer source-lock-v2 document.
    #[arg(long, value_name = "FILE")]
    pub(crate) source_lock: Option<PathBuf>,

    /// Reviewed native compatibility ports-lock-v2 document.
    #[arg(long, value_name = "FILE")]
    pub(crate) compatibility_ports_lock: Option<PathBuf>,

    /// Reviewed product source-fetch-plan-v1 document.
    #[arg(long, value_name = "FILE")]
    pub(crate) source_fetch_plan: Option<PathBuf>,
}

/// Explicit compiler-cache backend selection.
#[derive(Clone, Copy, ValueEnum)]
pub enum CacheCompilerBackend {
    /// Select the first available backend in AROS's stable preference order.
    Auto,
    /// Project the sccache backend only.
    Sccache,
    /// Project the ccache backend only.
    Ccache,
}
