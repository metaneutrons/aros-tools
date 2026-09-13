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
    /// Prepare and verify immutable Cargo vendor generations for the native producer.
    Cargo {
        /// One Cargo vendor-cache subcommand.
        #[command(subcommand)]
        command: CacheCargoCommand,
    },
    /// Inspect, regenerate, retain, and safely remove immutable GenMF reference expansions.
    Genmf {
        /// One GenMF reference-cache subcommand.
        #[command(subcommand)]
        command: CacheGenmfCommand,
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
    /// Claim one empty private root as a local-only compiler-cache namespace.
    Prepare {
        /// Backend that will exclusively own this cache root.
        #[arg(long, value_enum)]
        backend: ManagedCompilerBackend,

        /// Empty existing or new absolute root to claim; foreign state is refused.
        #[arg(long, value_name = "DIR")]
        dir: PathBuf,

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
    /// Retain one fully verified reviewed source closure under a named no-clobber reference.
    Keep {
        /// One exact reviewed source-cache selector.
        #[command(flatten)]
        selector: CacheSourceSelector,

        /// Existing real absolute source-cache root.
        #[arg(long, value_name = "DIR")]
        dir: PathBuf,

        /// New portable name for the retention reference.
        #[arg(long, value_name = "NAME")]
        name: String,

        /// Result representation on stdout, independent of diagnostic format.
        #[arg(long, value_enum, default_value = "human")]
        format: ResultFormat,
    },
    /// Preview or token-confirm release of one named source-cache retention reference.
    Release {
        /// Existing real absolute source-cache root.
        #[arg(long, value_name = "DIR")]
        dir: PathBuf,

        /// Existing portable retention-reference name.
        #[arg(long, value_name = "NAME")]
        name: String,

        /// Exact token from a prior release preview; without it, print a new preview.
        #[arg(long, value_name = "TOKEN")]
        apply: Option<String>,

        /// Result representation on stdout, independent of diagnostic format.
        #[arg(long, value_enum, default_value = "human")]
        format: ResultFormat,
    },
    /// Preview or apply removal of one role-selected source object.
    Remove {
        /// One exact reviewed source-cache selector.
        #[command(flatten)]
        selector: CacheSourceSelector,

        /// Existing real absolute source-cache root.
        #[arg(long, value_name = "DIR")]
        dir: PathBuf,

        /// Exact semantic role from the reviewed selector to remove.
        #[arg(long, value_name = "ROLE")]
        role: String,

        /// Exact token from a prior removal preview; without it, print a new preview.
        #[arg(long, value_name = "TOKEN")]
        apply: Option<String>,

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
    /// Retain one selected archive under a named no-clobber reference.
    Keep {
        /// One exact configured host or locked cross-toolchain archive.
        #[command(flatten)]
        selector: CacheArchiveSelector,

        /// New portable name for the retention reference.
        #[arg(long, value_name = "NAME")]
        name: String,

        /// Result representation on stdout, independent of diagnostic format.
        #[arg(long, value_enum, default_value = "human")]
        format: ResultFormat,
    },
    /// Preview or token-confirm release of one named archive retention reference.
    Release {
        /// Existing portable retention-reference name.
        #[arg(long, value_name = "NAME")]
        name: String,

        /// Exact token from a prior release preview; without it, print a new preview.
        #[arg(long, value_name = "TOKEN")]
        apply: Option<String>,

        /// Result representation on stdout, independent of diagnostic format.
        #[arg(long, value_enum, default_value = "human")]
        format: ResultFormat,
    },
    /// Preview or apply removal of one exact selected archive.
    Remove {
        /// One exact configured host or locked cross-toolchain archive.
        #[command(flatten)]
        selector: CacheArchiveSelector,

        /// Exact token from a prior removal preview; without it, print a new preview.
        #[arg(long, value_name = "TOKEN")]
        apply: Option<String>,

        /// Result representation on stdout, independent of diagnostic format.
        #[arg(long, value_enum, default_value = "human")]
        format: ResultFormat,
    },
}

/// Cargo vendor-cache resource operations.
#[derive(Subcommand)]
pub enum CacheCargoCommand {
    /// Passively observe one explicit parent cache root.
    Status {
        /// Existing or missing absolute cache root to observe.
        #[arg(long, value_name = "DIR")]
        dir: PathBuf,

        /// Result representation on stdout, independent of diagnostic format.
        #[arg(long, value_enum, default_value = "human")]
        format: ResultFormat,
    },
    /// Resolve one exact selection, then read its generation receipt without hashing its vendor tree.
    List {
        /// One exact producer/tools/Cargo/cache selection.
        #[command(flatten)]
        selector: CacheCargoSelector,

        /// Result representation on stdout, independent of diagnostic format.
        #[arg(long, value_enum, default_value = "human")]
        format: ResultFormat,
    },
    /// Populate one selected immutable generation, or reuse it after full verification.
    Fetch {
        /// One exact producer/tools/Cargo/cache selection.
        #[command(flatten)]
        selector: CacheCargoSelector,

        /// Refuse Cargo resolution and require an already verified generation.
        #[arg(long, env = "AROS_OFFLINE")]
        offline: bool,

        /// Result representation on stdout, independent of diagnostic format.
        #[arg(long, value_enum, default_value = "human")]
        format: ResultFormat,
    },
    /// Fully validate one selected generation against its lockfile and receipts.
    Verify {
        /// One exact producer/tools/Cargo/cache selection.
        #[command(flatten)]
        selector: CacheCargoSelector,

        /// Result representation on stdout, independent of diagnostic format.
        #[arg(long, value_enum, default_value = "human")]
        format: ResultFormat,
    },
    /// Retain one verified Cargo vendor generation under a named no-clobber reference.
    Keep {
        /// One exact producer/tools/Cargo/cache selection.
        #[command(flatten)]
        selector: CacheCargoSelector,

        /// New portable name for the retention reference.
        #[arg(long, value_name = "NAME")]
        name: String,

        /// Result representation on stdout, independent of diagnostic format.
        #[arg(long, value_enum, default_value = "human")]
        format: ResultFormat,
    },
    /// Preview or token-confirm release of one named Cargo retention reference.
    Release {
        /// Existing AROS-managed parent cache root holding the reference.
        #[arg(long, value_name = "DIR")]
        dir: PathBuf,

        /// Existing portable retention-reference name.
        #[arg(long, value_name = "NAME")]
        name: String,

        /// Exact token from a prior release preview; without it, print a new preview.
        #[arg(long, value_name = "TOKEN")]
        apply: Option<String>,

        /// Result representation on stdout, independent of diagnostic format.
        #[arg(long, value_enum, default_value = "human")]
        format: ResultFormat,
    },
    /// Preview or apply removal of one exact verified Cargo vendor generation.
    Remove {
        /// One exact producer/tools/Cargo/cache selection.
        #[command(flatten)]
        selector: CacheCargoSelector,

        /// Exact token from a prior removal preview; without it, print a new preview.
        #[arg(long, value_name = "TOKEN")]
        apply: Option<String>,

        /// Result representation on stdout, independent of diagnostic format.
        #[arg(long, value_enum, default_value = "human")]
        format: ResultFormat,
    },
}

/// GenMF reference-cache resource operations.
#[derive(Subcommand)]
pub enum CacheGenmfCommand {
    /// Passively observe one explicit GenMF cache root.
    Status {
        /// Existing or missing absolute parent cache root to observe.
        #[arg(long, value_name = "DIR")]
        dir: PathBuf,

        /// Result representation on stdout, independent of diagnostic format.
        #[arg(long, value_enum, default_value = "human")]
        format: ResultFormat,
    },
    /// Select current source inputs and list generation metadata without reading payloads.
    List {
        /// Exact source/cache/interpreter selection.
        #[command(flatten)]
        selector: CacheGenmfSelector,

        /// Result representation on stdout, independent of diagnostic format.
        #[arg(long, value_enum, default_value = "human")]
        format: ResultFormat,
    },
    /// Fully validate all immutable generations selected by current source inputs.
    Verify {
        /// Exact source/cache/interpreter selection.
        #[command(flatten)]
        selector: CacheGenmfSelector,

        /// Result representation on stdout, independent of diagnostic format.
        #[arg(long, value_enum, default_value = "human")]
        format: ResultFormat,
    },
    /// Regenerate current references and prove existing immutable generations match.
    Refresh {
        /// Exact source/cache/interpreter selection.
        #[command(flatten)]
        selector: CacheGenmfSelector,

        /// Result representation on stdout, independent of diagnostic format.
        #[arg(long, value_enum, default_value = "human")]
        format: ResultFormat,
    },
    /// Retain every verified generation in the exact current GenMF selection.
    Keep {
        /// Exact source/cache/interpreter selection.
        #[command(flatten)]
        selector: CacheGenmfSelector,

        /// New portable name for the retention reference.
        #[arg(long, value_name = "NAME")]
        name: String,

        /// Result representation on stdout, independent of diagnostic format.
        #[arg(long, value_enum, default_value = "human")]
        format: ResultFormat,
    },
    /// Preview or token-confirm release of one named GenMF retention reference.
    Release {
        /// Existing no-follow parent cache root holding the reference.
        #[arg(long, value_name = "DIR")]
        dir: PathBuf,

        /// Existing portable retention-reference name.
        #[arg(long, value_name = "NAME")]
        name: String,

        /// Exact token from a prior release preview; without it, print a new preview.
        #[arg(long, value_name = "TOKEN")]
        apply: Option<String>,

        /// Result representation on stdout, independent of diagnostic format.
        #[arg(long, value_enum, default_value = "human")]
        format: ResultFormat,
    },
    /// Preview or apply removal of one current input-selected immutable generation.
    Remove {
        /// Exact source/cache/interpreter selection.
        #[command(flatten)]
        selector: CacheGenmfSelector,

        /// Exact source-root-relative MMake input from the current selection, for example rom/mmakefile.
        #[arg(long, value_name = "PATH")]
        source: String,

        /// Exact token from a prior removal preview; without it, print a new preview.
        #[arg(long, value_name = "TOKEN")]
        apply: Option<String>,

        /// Result representation on stdout, independent of diagnostic format.
        #[arg(long, value_enum, default_value = "human")]
        format: ResultFormat,
    },
}

/// Exact selection of the versioned GenMF reference-cache namespace.
#[derive(Args)]
pub struct CacheGenmfSelector {
    /// Existing no-follow AROS source checkout containing GenMF and MMake files.
    #[arg(long, value_name = "DIR")]
    pub(crate) source_dir: PathBuf,

    /// Existing no-follow parent cache root owning genmf/v1 generations.
    #[arg(long, value_name = "DIR")]
    pub(crate) dir: PathBuf,

    /// Exact absolute Python interpreter; defaults to the Python resolved from PATH.
    #[arg(long, value_name = "FILE")]
    pub(crate) python: Option<PathBuf>,

    /// Bounded lock wait and GenMF invocation deadline per selected expansion.
    #[arg(
        long,
        default_value_t = 30,
        value_parser = clap::value_parser!(u64).range(1..=3600),
        env = "AROS_CACHE_GENMF_TIMEOUT_SECONDS"
    )]
    pub(crate) timeout_seconds: u64,
}

/// Explicit selector for one immutable Cargo vendor generation.
#[derive(Args)]
pub struct CacheCargoSelector {
    /// Producer checkout containing toolchains/rust-toolchain.toml.
    #[arg(long, value_name = "DIR")]
    pub(crate) producer_dir: PathBuf,

    /// aros-tools checkout containing Cargo.toml and Cargo.lock.
    #[arg(long, value_name = "DIR")]
    pub(crate) tools_dir: PathBuf,

    /// Existing AROS-managed source-cache root used for cargo/v1 generations.
    #[arg(long, value_name = "DIR")]
    pub(crate) dir: PathBuf,

    /// Exact Cargo executable; defaults to the absolute Cargo resolved from PATH.
    #[arg(long, value_name = "FILE")]
    pub(crate) cargo: Option<PathBuf>,
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

/// One concrete backend permitted to own a managed local cache namespace.
#[derive(Clone, Copy, ValueEnum)]
pub enum ManagedCompilerBackend {
    /// Mozilla sccache with a private local disk store and private UDS socket.
    Sccache,
    /// ccache with a private local store and generated local-only configuration.
    Ccache,
}
