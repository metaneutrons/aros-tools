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
