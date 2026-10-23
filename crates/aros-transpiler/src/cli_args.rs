//! Command-line surface of the transpiler.

use aros_common::{DiagnosticFormat, LogFormat, LogLevel};
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "Fail-closed AROS MetaMake-to-CMake transpiler",
    after_help = "OBSERVABILITY:\n  --diagnostic-format human|json\n  --log-level off|error|warn|info|debug|trace\n  --log-format human|jsonl\n  --log-file PATH\n\nThe same settings are available through AROS_TRANSPILER_DIAGNOSTIC_FORMAT,\nAROS_TRANSPILER_LOG_LEVEL, AROS_TRANSPILER_LOG_FORMAT, and\nAROS_TRANSPILER_LOG_FILE. Logging is off by default. A selected file without a\nselected level uses info; explicit off creates no sink, and a non-off level\nrequires a local file."
)]
#[command(group(clap::ArgGroup::new("native_selection").args(["native_profile", "native_consumer_profile"]).multiple(false)))]
#[command(group(clap::ArgGroup::new("genmodule_selection").args(["native_profile", "native_consumer_profile", "mesa_genmodule_full_inventory_profile"]).multiple(false)))]
pub struct Args {
    /// Root directory of AROS source tree
    #[arg(short, long, default_value = ".")]
    pub source_dir: PathBuf,

    /// Output path for generated CMake targets file
    #[arg(short, long, default_value = "build/generated_targets.cmake")]
    pub output: PathBuf,

    /// Physical configure-time path behind `${AROS_PORTS_DIR}`
    #[arg(long)]
    pub ports_dir: Option<PathBuf>,

    /// Prepare only the source-inventory sidecar; do not publish a build graph
    #[arg(long, requires = "native_selection")]
    pub source_inventory_only: bool,

    /// Export selected Mesa genmodule declarations as data; do not emit CMake
    #[arg(long, requires_all = ["genmodule_selection", "mesa_version", "cpu", "platform", "toolchain", "cpu32", "use_mmu", "float_abi"], conflicts_with_all = ["source_inventory_only", "native_graph_audit", "mesa_configuration_input", "validate_native_consumer_only"])]
    pub mesa_genmodule_inventory_only: bool,

    /// Bind a source SDK profile but inventory the ordinary full target graph,
    /// not that contract's SDK roots. Publishes declaration data only.
    #[arg(long, requires = "mesa_genmodule_inventory_only", conflicts_with_all = ["native_profile", "native_consumer_profile"])]
    pub mesa_genmodule_full_inventory_profile: Option<String>,

    /// Expected SDK configuration-contract digest for the full-target inventory
    #[arg(long, requires = "mesa_genmodule_full_inventory_profile")]
    pub mesa_genmodule_full_contract_sha256: Option<String>,

    /// Write a diagnostic-only selected graph audit; publish no graph or inventory
    #[arg(long, requires = "source_inventory_only")]
    pub native_graph_audit: Option<PathBuf>,

    /// Source-owned native profile whose complete dependency graph is selected
    #[arg(long)]
    pub native_profile: Option<String>,

    /// Expected digest of that profile's validated native build contract
    #[arg(
        long,
        requires = "native_profile",
        conflicts_with = "native_consumer_profile"
    )]
    pub native_contract_sha256: Option<String>,

    /// Source-owned SDK/consumer graph, without core, package or media policy
    #[arg(long)]
    pub native_consumer_profile: Option<String>,

    /// Expected digest of the selected source-owned consumer contract
    #[arg(
        long,
        requires = "native_consumer_profile",
        conflicts_with = "native_profile"
    )]
    pub native_consumer_contract_sha256: Option<String>,

    /// Validate only the consumer binding and emit JSON; no graph or build proof
    #[arg(
        long,
        requires = "native_consumer_profile",
        conflicts_with_all = ["native_profile", "source_inventory_only", "native_graph_audit"]
    )]
    pub validate_native_consumer_only: bool,

    /// Target instruction set (for example x86_64, arm, or aarch64)
    #[arg(long)]
    pub cpu: Option<String>,

    /// Target machine/platform (for example pc or raspi)
    #[arg(long)]
    pub platform: Option<String>,

    /// MetaMake target family
    #[arg(long)]
    pub family: Option<String>,

    /// MetaMake target variant; pass an empty value for the ordinary variant
    #[arg(long)]
    pub variant: Option<String>,

    /// Toolchain family (gnu or llvm)
    #[arg(long)]
    pub toolchain: Option<String>,

    /// Optional 32-bit companion CPU
    #[arg(long)]
    pub cpu32: Option<String>,

    /// Historic USE_MMU value (0 or 1)
    #[arg(long)]
    pub use_mmu: Option<String>,

    /// Historic GCC_CONFIG_FLOAT_ABI value
    #[arg(long)]
    pub float_abi: Option<String>,

    /// Explicit Mesa version selector (MetaMake OPT_MESAGL)
    #[arg(long = "mesa-version")]
    pub mesa_version: Option<String>,

    /// Data-only Mesa configuration input; producer execution is proved by its owner
    #[arg(long, requires_all = ["mesa_configuration_input_sha256", "mesa_version", "cpu", "toolchain"], conflicts_with_all = ["validate_native_consumer_only", "native_graph_audit"])]
    pub mesa_configuration_input: Option<PathBuf>,

    /// Exact digest of the parent-selected data input, not a producer attestation
    #[arg(long, requires = "mesa_configuration_input")]
    pub mesa_configuration_input_sha256: Option<String>,

    /// Explicit target LLVM version selector (MetaMake TARGET_LLVM_VER)
    #[arg(long = "target-llvm-ver")]
    pub target_llvm_ver: Option<String>,

    /// Explicit target LLVM runtimes layout selector
    #[arg(long = "target-llvm-runtimes-style")]
    pub target_llvm_runtimes_style: Option<String>,

    /// Explicit target Rust selector (MetaMake TARGET_RUST)
    #[arg(long = "target-rust")]
    pub target_rust: Option<String>,

    /// Explicit target Rust version selector (MetaMake TARGET_RUST_VER)
    #[arg(long = "target-rust-ver")]
    pub target_rust_ver: Option<String>,

    /// The build directory of the calling engine. A native invocation excludes
    /// it from MetaMake discovery by exact path when it lies inside the source
    /// tree, because its generated files are not source recipes.
    #[arg(long = "build-dir")]
    pub build_dir: Option<PathBuf>,

    /// Diagnostic renderer used for failures
    #[arg(
        long,
        value_enum,
        default_value_t = DiagnosticFormat::Human,
        env = "AROS_TRANSPILER_DIAGNOSTIC_FORMAT"
    )]
    pub diagnostic_format: DiagnosticFormat,

    /// Local logging threshold; logging is disabled by default.
    #[arg(
        long,
        value_enum,
        default_value_t = LogLevel::Off,
        env = "AROS_TRANSPILER_LOG_LEVEL"
    )]
    pub log_level: LogLevel,

    /// Stable local log encoding.
    #[arg(
        long,
        value_enum,
        default_value_t = LogFormat::Human,
        env = "AROS_TRANSPILER_LOG_FORMAT"
    )]
    pub log_format: LogFormat,

    /// Explicit local log destination.
    #[arg(long, env = "AROS_TRANSPILER_LOG_FILE")]
    pub log_file: Option<PathBuf>,
}

#[cfg(test)]
mod target_context_cli_tests {
    use super::*;

    #[test]
    fn genmodule_inventory_requires_explicit_selection_and_is_not_a_configured_export() {
        let base = [
            "aros-transpiler",
            "--mesa-genmodule-inventory-only",
            "--native-consumer-profile",
            "sdk-aarch64",
            "--mesa-version",
            "26.0.0",
            "--cpu",
            "aarch64",
            "--platform",
            "raspi",
            "--toolchain",
            "llvm",
            "--cpu32",
            "",
            "--use-mmu",
            "1",
            "--float-abi",
            "",
        ];
        assert!(
            Args::try_parse_from(base)
                .unwrap()
                .mesa_genmodule_inventory_only
        );
        assert!(
            Args::try_parse_from(["aros-transpiler", "--mesa-genmodule-inventory-only"]).is_err()
        );
        for conflict in ["--source-inventory-only", "--validate-native-consumer-only"] {
            assert!(Args::try_parse_from(base.into_iter().chain([conflict])).is_err());
        }
        assert!(Args::try_parse_from(base.into_iter().chain([
            "--mesa-configuration-input",
            "input.json",
            "--mesa-configuration-input-sha256",
            "00"
        ]))
        .is_err());
        let missing_float = &base[..base.len() - 2];
        assert!(Args::try_parse_from(missing_float.iter().copied()).is_err());
        let full: Vec<_> = base
            .into_iter()
            .map(|argument| {
                if argument == "--native-consumer-profile" {
                    "--mesa-genmodule-full-inventory-profile"
                } else {
                    argument
                }
            })
            .collect();
        assert!(Args::try_parse_from(full.iter().copied())
            .unwrap()
            .mesa_genmodule_full_inventory_profile
            .is_some());
        assert!(Args::try_parse_from(
            full.iter()
                .copied()
                .chain(["--native-consumer-profile", "other"])
        )
        .is_err());
        assert!(Args::try_parse_from([
            "aros-transpiler",
            "--mesa-genmodule-full-inventory-profile",
            "sdk-aarch64"
        ])
        .is_err());
    }

    #[test]
    fn upstream_selector_arguments_are_explicit_and_omittable() {
        let selected = Args::try_parse_from([
            "aros-transpiler",
            "--mesa-version",
            "26.0.0",
            "--target-llvm-ver",
            "23.0.0",
            "--target-llvm-runtimes-style",
            "umbrella",
            "--target-rust",
            "yes",
            "--target-rust-ver",
            "1.98.1",
        ])
        .expect("explicit upstream selectors");
        assert_eq!(selected.mesa_version.as_deref(), Some("26.0.0"));
        assert_eq!(selected.target_llvm_ver.as_deref(), Some("23.0.0"));
        assert_eq!(
            selected.target_llvm_runtimes_style.as_deref(),
            Some("umbrella")
        );
        assert_eq!(selected.target_rust.as_deref(), Some("yes"));
        assert_eq!(selected.target_rust_ver.as_deref(), Some("1.98.1"));

        let altered = Args::try_parse_from(["aros-transpiler", "--target-llvm-ver", "24.0.0"])
            .expect("altered LLVM selector");
        assert_eq!(altered.target_llvm_ver.as_deref(), Some("24.0.0"));

        let omitted = Args::try_parse_from(["aros-transpiler"]).expect("legacy caller");
        assert_eq!(omitted.mesa_version, None);
        assert_eq!(omitted.target_llvm_ver, None);
        assert_eq!(omitted.target_llvm_runtimes_style, None);
        assert_eq!(omitted.target_rust, None);
        assert_eq!(omitted.target_rust_ver, None);
    }
}
