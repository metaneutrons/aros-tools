//! Independent verification of transpiler output against the historic build.
//!
//! The build has two ways of being wrong that a compiler cannot report. A
//! target the transpiler never emits produces no error, because nothing asks
//! for it; and a target it emits with the wrong shape, such as one executable
//! per source file where the reference builds one from all of them, compiles
//! perfectly well and links the wrong binaries.
//!
//! `tools/genmf/genmf.py` expands an mmakefile into the makefile the historic
//! build actually runs, so it answers both questions. This tool runs it over
//! the tree and compares:
//!
//!   * **Coverage** -- every `mmake=` declaration in the tree against the
//!     `MMAKE_ID` entries in `generated_targets.cmake`.
//!   * **Shape** -- for a program target, the reference's `_PROGNAME` against
//!     the name the transpiler gave it, and how many targets each declaration
//!     produced on either side.
//!
//! Both are reported as counts and as files, and the exit code is non-zero
//! when something is missing, so this can gate a build.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::Result;
use aros_common::{
    read_source, render_diagnostics, requested_diagnostic_format, Diagnostic, DiagnosticCode,
    DiagnosticContext, DiagnosticFormat, DiagnosticSet, DiagnosticStage, LogFormat, LogLevel,
    Logger, ObservabilityPolicy, SourceLocation,
};
use clap::{error::ErrorKind, Parser, ValueEnum};
use rayon::prelude::*;
use regex::Regex;

mod coverage_io;
#[cfg(test)]
use coverage_io::find_mmakefiles_with;
use coverage_io::{
    find_mmakefiles, read_optional_coverage_source, CoverageReadError, CoverageReadPhase,
    CoverageReadResult,
};
mod genmf;
use genmf::{expand_all, ExpansionFailure};
#[cfg(test)]
use genmf::{genmf_dependency_files, timestamps_are_fresh};

#[derive(Parser, Debug)]
#[command(
    name = "aros-verify",
    version,
    about = "Compare transpiled CMake targets against the genmf reference expansion",
    after_help = "OBSERVABILITY:\n  --diagnostic-format human|json\n  --log-level off|error|warn|info|debug|trace\n  --log-format human|jsonl\n  --log-file PATH\n\nThe same settings are available through AROS_VERIFY_DIAGNOSTIC_FORMAT,\nAROS_VERIFY_LOG_LEVEL, AROS_VERIFY_LOG_FORMAT, and AROS_VERIFY_LOG_FILE.\nLogging is off by default and is written only to an explicitly selected local file."
)]
struct Args {
    /// Source tree root.
    #[arg(long, default_value = ".")]
    source: PathBuf,

    /// The transpiler's output to check.
    #[arg(long)]
    generated: PathBuf,

    /// Where to cache genmf expansions and write reports.
    #[arg(long)]
    work: PathBuf,

    /// The configured build directory, to check that emitted declarations
    /// actually became CMake targets.
    #[arg(long)]
    build_dir: Option<PathBuf>,

    /// Target CPU for architecture-scoped coverage (for example x86_64).
    #[arg(long, value_parser = parse_arch_component, requires = "platform")]
    cpu: Option<String>,

    /// Target platform for architecture-scoped coverage (for example pc).
    #[arg(long, value_parser = parse_arch_component, requires = "cpu")]
    platform: Option<String>,

    /// Configured target toolchain family (for example llvm or gnu).
    #[arg(long, value_parser = parse_arch_component, requires_all = ["cpu", "platform"])]
    toolchain: Option<String>,

    /// Configured upstream bootloader lane; an empty value means no bootloader.
    #[arg(long, requires_all = ["cpu", "platform"])]
    bootloader: Option<String>,

    /// Coverage profile. Only architecture eligibility is currently
    /// evidence-backed; core/distribution reachability needs verified roots.
    #[arg(long, value_enum, requires_all = ["cpu", "platform"])]
    profile: Option<Profile>,

    /// Re-run genmf even when a cached expansion exists.
    #[arg(long)]
    refresh: bool,

    /// Hard deadline in seconds for each independent genmf expansion.
    #[arg(
        long,
        default_value_t = 30,
        value_parser = clap::value_parser!(u64).range(1..=3600),
        env = "AROS_VERIFY_GENMF_TIMEOUT_SECONDS"
    )]
    genmf_timeout_seconds: u64,

    /// Report only; exit 0 even when targets are missing.
    #[arg(long)]
    no_gate: bool,

    /// Diagnostic renderer used for failures.
    #[arg(long, value_enum, default_value_t = DiagnosticFormat::Human, env = "AROS_VERIFY_DIAGNOSTIC_FORMAT")]
    diagnostic_format: DiagnosticFormat,

    /// Minimum level written to the explicitly selected local log.
    #[arg(long, value_enum, default_value_t = LogLevel::Off, env = "AROS_VERIFY_LOG_LEVEL")]
    log_level: LogLevel,

    /// Local log encoding.
    #[arg(long, value_enum, default_value_t = LogFormat::Human, env = "AROS_VERIFY_LOG_FORMAT")]
    log_format: LogFormat,

    /// Local log destination; required whenever logging is enabled.
    #[arg(long, env = "AROS_VERIFY_LOG_FILE")]
    log_file: Option<PathBuf>,
}

const OBSERVABILITY_POLICY: ObservabilityPolicy = ObservabilityPolicy {
    log_schema: "aros-verify-log-v1",
    component: "AROS verifier",
    include_invocation: false,
    observability_code: DiagnosticCode::VerifyObservability,
    observability_stage: DiagnosticStage::Observability,
    internal_code: DiagnosticCode::VerifyInternal,
    internal_stage: DiagnosticStage::Internal,
    hint: "select an explicit writable local file or disable verifier logging",
};

#[derive(Debug)]
struct VerifyFailure {
    diagnostic: Box<Diagnostic>,
}

impl VerifyFailure {
    fn new(code: DiagnosticCode, stage: DiagnosticStage, message: impl Into<String>) -> Self {
        Self {
            diagnostic: Box::new(Diagnostic::error(code, stage, message)),
        }
    }

    fn at_path(
        code: DiagnosticCode,
        stage: DiagnosticStage,
        message: impl Into<String>,
        path: &Path,
    ) -> Self {
        Self {
            diagnostic: Box::new(
                Diagnostic::error(code, stage, message)
                    .with_location(SourceLocation::new(path.display().to_string())),
            ),
        }
    }

    fn into_diagnostic(self) -> Diagnostic {
        *self.diagnostic
    }
}

fn coverage_read_failure(error: CoverageReadError) -> VerifyFailure {
    let stage = error.phase.diagnostic_stage();
    let message = error.to_string();
    let path = error.path;
    VerifyFailure::at_path(DiagnosticCode::VerifyInput, stage, message, &path)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Profile {
    /// Filter declarations by the configured CMake architecture directories.
    Architecture,
}

impl Profile {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Architecture => "architecture",
        }
    }
}

/// The exact architecture directory sets CMake constructs in AROS.cmake.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ArchitectureScope {
    cpu: String,
    platform: String,
    toolchain: String,
    bootloader: String,
    source_dirs: BTreeSet<String>,
    package_dirs: BTreeSet<String>,
}

impl ArchitectureScope {
    #[cfg(test)]
    fn new(cpu: &str, platform: &str) -> Self {
        let bootloader = if platform == "pc" { "grub2gfx" } else { "" };
        Self::with_configuration(cpu, platform, "llvm", bootloader)
    }

    fn with_configuration(cpu: &str, platform: &str, toolchain: &str, bootloader: &str) -> Self {
        let compatible_cpus: &[&str] = match cpu {
            "x86_64" => &["i386", "x86_64"],
            "aarch64" => &["arm", "aarch64"],
            "riscv64" => &["riscv", "riscv64"],
            _ => &[cpu],
        };

        // cmake/AROS.cmake starts with all-native, then appends these four
        // spellings for every compatible CPU and removes duplicates.
        let mut source_dirs = BTreeSet::from(["all-native".to_owned()]);
        for compatible in compatible_cpus {
            source_dirs.insert(format!("{compatible}-all"));
            source_dirs.insert(format!("{compatible}-native"));
            source_dirs.insert(format!("all-{platform}"));
            source_dirs.insert(format!("{compatible}-{platform}"));
        }

        // Packages are narrower: sources may come from a compatible CPU, but
        // only the configured CPU's package may write an architecture-relative
        // output such as boot/<platform>/aros-bsp.pkg.
        let package_dirs = BTreeSet::from([
            "all-native".to_owned(),
            format!("{cpu}-all"),
            format!("{cpu}-native"),
            format!("all-{platform}"),
            format!("{cpu}-{platform}"),
        ]);

        Self {
            cpu: cpu.to_owned(),
            platform: platform.to_owned(),
            toolchain: toolchain.to_owned(),
            bootloader: bootloader.to_owned(),
            source_dirs,
            package_dirs,
        }
    }

    fn from_args(args: &Args) -> Option<Self> {
        args.cpu
            .as_deref()
            .zip(args.platform.as_deref())
            .map(|(cpu, platform)| {
                Self::with_configuration(
                    cpu,
                    platform,
                    args.toolchain
                        .as_deref()
                        .expect("validated architecture toolchain"),
                    args.bootloader
                        .as_deref()
                        .expect("validated architecture bootloader"),
                )
            })
    }

    fn key(&self) -> String {
        format!("architecture-{}-{}", self.cpu, self.platform)
    }

    /// Concrete values supplied to MetaMake by the equivalent CMake profile.
    ///
    /// Historic MetaMake calls the machine `ARCH`/`AROS_TARGET_ARCH`, while
    /// `AROS_TARGET_PLATFORM` is the compound machine/CPU selector.  CMake's
    /// names are less surprising, so keeping this translation here prevents
    /// the reference denominator from evaluating a condition in a different
    /// context from the transpiler.
    fn make_value(&self, name: &str) -> Option<String> {
        match name {
            "AROS_TARGET_CPU" | "CPU" => Some(self.cpu.clone()),
            "AROS_TARGET_ARCH" | "ARCH" => Some(self.platform.clone()),
            "AROS_TARGET_PLATFORM" => Some(format!("{}-{}", self.platform, self.cpu)),
            "AROS_TOOLCHAIN" => Some(self.toolchain.clone()),
            "AROS_TARGET_BOOTLOADER" => Some(self.bootloader.clone()),
            // CMake currently configures an i386 companion only for x86_64.
            // For every other CPU this is a known empty Make variable, not an
            // unknown value inherited from some unavailable configuration.
            "AROS_TARGET_CPU32" => Some(if self.cpu == "x86_64" {
                "i386".to_owned()
            } else {
                String::new()
            }),
            _ => None,
        }
    }

    fn declaration_is_eligible(&self, declaration: &Declaration) -> bool {
        if matches!(
            declaration.macro_name.as_str(),
            "make_package" | "link_kickstart"
        ) {
            let Some(arch_dir) = declaration_arch_dir(&declaration.file) else {
                return !is_under_arch(&declaration.file);
            };
            self.package_dirs.contains(arch_dir)
        } else {
            self.file_is_eligible(&declaration.file)
        }
    }

    fn file_is_eligible(&self, file: &str) -> bool {
        let Some(arch_dir) = declaration_arch_dir(file) else {
            // CMake gates only paths below arch/<cpu>-<platform>. Everything
            // outside arch/ is shared by all target architectures.
            return !is_under_arch(file);
        };
        self.source_dirs.contains(arch_dir)
    }
}

fn parse_arch_component(value: &str) -> std::result::Result<String, String> {
    if value.is_empty()
        || !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        return Err(format!(
            "'{value}' is not an architecture component (expected ASCII letters, digits, '_' or '-')"
        ));
    }
    Ok(value.to_owned())
}

fn validate_profile_arguments(args: &Args) -> Result<()> {
    if args.cpu.is_some() && (args.toolchain.is_none() || args.bootloader.is_none()) {
        anyhow::bail!(
            "architecture verification requires explicit --toolchain and --bootloader values"
        );
    }
    Ok(())
}

fn is_under_arch(file: &str) -> bool {
    file.split(['/', '\\']).next() == Some("arch")
}

fn declaration_arch_dir(file: &str) -> Option<&str> {
    let mut parts = file.split(['/', '\\']);
    (parts.next()? == "arch").then_some(())?;
    let dir = parts.next()?;
    dir.split_once('-').map(|_| dir)
}

/// One `%build_*` declaration found in an mmakefile.
#[derive(Debug, Clone)]
struct Declaration {
    mmake: String,
    macro_name: String,
    file: String,
    /// Complete continuation-joined macro arguments with insignificant
    /// whitespace collapsed.  Provisioning exclusions use this rather than
    /// only the target name, so changing a compiler, prefix, source, option
    /// owner, or adding an argument fails closed into the normal target gate.
    arguments: String,
}

const LLVM_PROVISIONING_FILE: &str = "tools/crosstools/llvm/mmakefile.src";
const GCC_PROVISIONING_FILE: &str = "tools/crosstools/gnu/mmakefile.src";
const GCC_LIBATOMIC_ARGUMENTS: &str = "mmake=tools-crosstools-gcc-libatomic srcdir=\"$(LIBATOMIC_SRCDIR)\" basedir= gendir=\"$(LIBATOMIC_OBJDIR)\" extraoptions=\"$(LIBATOMIC_OPTS)\" install_env=\"$(LIBATOMIC_ENV)\"";
const LEGACY_GRUB_FILE: &str = "arch/all-pc/boot/grub/mmakefile.src";
const LEGACY_GRUB_ARGUMENTS: &str = "mmake=grub compiler=kernel install_target= srcdir=$(ARCHSRCDIR) extraoptions=\"$(GRUBOPTS)\" extracflags=\"$(GRUBCFLAGS)\"";

/// Exact legacy declarations that provision the compiler installation used as
/// an input by the modern CMake build.  They are not target-tree products.
///
/// The declaration contract is only one layer of the check. The structural
/// boundary below also verifies the few facts that make this classification
/// sound. Unrelated edits must not require an opaque digest update.
const LLVM_PROVISIONING_DECLARATIONS: &[(&str, &str)] = &[
    (
        "crosstools-libunwind",
        "mmake=crosstools-libunwind package=libunwind srcdir=$(LIBUNWIND_BUILDBASE) prefix=\"$(CROSSTOOLSDIR)\" extraoptions=\"$(LLVM_LIBUNWIND_CMAKEOPTIONS)\" compiler=host usecppflags=no",
    ),
    (
        "crosstools-libunwind-release",
        "mmake=crosstools-libunwind-release package=libunwind-release srcdir=$(LIBUNWIND_BUILDBASE) prefix=\"$(CROSSTOOLSDIR)\" extraoptions=\"$(LLVM_LIBUNWIND_CMAKEOPTIONS)\" compiler=host usecppflags=no metadeps=\"setup sdk-includes-1\"",
    ),
    (
        "crosstools-llvm-runtimes",
        "mmake=crosstools-llvm-runtimes package=runtimes srcdir=$(MONOTREE_BUILDBASE)/runtimes prefix=\"$(CROSSTOOLSDIR)\" extraoptions=\"$(LLVM_RUNTIMES_CMAKEOPTIONS)\" compiler=host usecppflags=no",
    ),
    (
        "crosstools-llvm-runtimes-release",
        "mmake=crosstools-llvm-runtimes-release package=runtimes-release srcdir=$(MONOTREE_BUILDBASE)/runtimes prefix=\"$(CROSSTOOLSDIR)\" extraoptions=\"$(LLVM_RUNTIMES_CMAKEOPTIONS)\" compiler=host usecppflags=no metadeps=\"setup sdk-includes-1\"",
    ),
    (
        "crosstools-compiler-rt",
        "mmake=crosstools-compiler-rt package=compiler-rt srcdir=$(COMPILER_RT_BUILDBASE) prefix=\"$(CROSSTOOLSDIR)\" extraoptions=\"$(LLVM_COMPILER_RT_CMAKEOPTIONS)\" compiler=host usecppflags=no",
    ),
    (
        "crosstools-compiler-rt-release",
        "mmake=crosstools-compiler-rt-release package=compiler-rt-release srcdir=$(COMPILER_RT_BUILDBASE) prefix=\"$(CROSSTOOLSDIR)\" extraoptions=\"$(LLVM_COMPILER_RT_CMAKEOPTIONS)\" compiler=host usecppflags=no metadeps=\"setup sdk-includes-1\"",
    ),
    (
        "crosstools-compiler-rt32",
        "mmake=crosstools-compiler-rt32 package=compiler-rt32 srcdir=$(COMPILER_RT_BUILDBASE) prefix=\"$(CROSSTOOLSDIR)\" extraoptions=\"$(LLVM_COMPILER_RT32_CMAKEOPTIONS)\" compiler=host usecppflags=no",
    ),
    (
        "crosstools-compiler-rt32-release",
        "mmake=crosstools-compiler-rt32-release package=compiler-rt32-release srcdir=$(COMPILER_RT_BUILDBASE) prefix=\"$(CROSSTOOLSDIR)\" extraoptions=\"$(LLVM_COMPILER_RT32_CMAKEOPTIONS)\" compiler=host usecppflags=no metadeps=\"setup sdk-includes-1\"",
    ),
    (
        "crosstools-llvm-toolchain",
        "mmake=crosstools-llvm-toolchain package=llvm srcdir=$(LLVM_BUILDBASE) prefix=\"$(CROSSTOOLSDIR)\" extraoptions=\"$(LLVM_CMAKEOPTIONS)\" compiler=host usecppflags=no usecrosstoolsdir=no",
    ),
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ToolchainProvisioningContext {
    llvm: bool,
    gcc_libatomic: bool,
}

fn collapse_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Canonical semantic view of a Make input used for fail-closed fingerprints.
/// `#MM` is MetaMake syntax rather than documentation and must be retained.
fn canonical_make_semantics(content: &str) -> String {
    let continuations = Regex::new(r"\\\r?\n").unwrap();
    let joined = continuations.replace_all(content, "");
    joined
        .lines()
        .filter_map(|raw_line| {
            let trimmed = raw_line.trim();
            let semantic = if trimmed.starts_with("#MM") {
                trimmed
            } else {
                strip_make_comment(raw_line).trim()
            };
            (!semantic.is_empty()).then(|| collapse_whitespace(semantic))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// CMake chooses the compiler before `project()`, making the toolchain an
/// input to every target profile.  Fingerprint precisely that preamble rather
/// than the rest of the independently evolving target build.
fn canonical_cmake_toolchain_input_preamble(content: &str) -> Option<String> {
    let mut semantic_lines = Vec::new();
    let mut project_depth = None;

    for raw_line in content.lines() {
        let semantic = collapse_whitespace(strip_make_comment(raw_line).trim());
        if semantic.is_empty() {
            continue;
        }
        if project_depth.is_none() && semantic.starts_with("project(") {
            project_depth = Some(0isize);
        }
        semantic_lines.push(semantic.clone());

        if let Some(depth) = project_depth.as_mut() {
            for character in semantic.chars() {
                match character {
                    '(' => *depth += 1,
                    ')' => *depth -= 1,
                    _ => {}
                }
            }
            if *depth == 0 {
                return Some(semantic_lines.join("\n"));
            }
        }
    }
    None
}

fn llvm_provisioning_context_matches_sources(
    llvm_mmake: &str,
    make_config: &str,
    cmake_lists: &str,
) -> bool {
    let llvm_semantics = canonical_make_semantics(llvm_mmake);
    let llvm_lines: BTreeSet<&str> = llvm_semantics.lines().collect();
    let make_config_semantics = canonical_make_semantics(make_config);
    let crosstools_placeholders: Vec<_> = make_config_semantics
        .lines()
        .filter(|line| line.starts_with("CROSSTOOLSDIR "))
        .collect();
    let Some(cmake_preamble) = canonical_cmake_toolchain_input_preamble(cmake_lists) else {
        return false;
    };
    let cmake_lines: BTreeSet<&str> = cmake_preamble.lines().collect();

    llvm_lines.contains("LLVM_BUILD_BINDIR:=$(CROSSTOOLSDIR)/bin")
        && llvm_lines.contains("AROS_TOOLCHAIN_DEFAULT_SYSROOT ?= $(AROS_DEVELOPER)")
        && crosstools_placeholders == ["CROSSTOOLSDIR := @AROS_CROSSTOOLSDIR@"]
        && !cmake_lists.contains("CROSSTOOLSDIR")
        && cmake_lines.contains("set(CMAKE_SYSTEM_NAME Generic)")
        && cmake_lines
            .iter()
            .any(|line| line.starts_with("project(AROS-NG") || line.starts_with("project(AROS-NX"))
}

fn detect_toolchain_provisioning_context(
    root: &Path,
) -> CoverageReadResult<ToolchainProvisioningContext> {
    let read = |relative: &str| {
        read_optional_coverage_source(
            &root.join(relative),
            CoverageReadPhase::ProvisioningContextRead,
        )
    };
    let llvm = read(LLVM_PROVISIONING_FILE)?
        .zip(read("config/make.cfg.in")?)
        .zip(read("CMakeLists.txt")?)
        .is_some_and(|((mmake, make_config), cmake_lists)| {
            llvm_provisioning_context_matches_sources(&mmake, &make_config, &cmake_lists)
        });
    let gcc_libatomic = read(GCC_PROVISIONING_FILE)?.is_some_and(|gnu_mmake| {
        let semantics = canonical_make_semantics(&gnu_mmake);
        let lines: BTreeSet<&str> = semantics.lines().collect();
        lines.contains(
            "LIBATOMIC_OBJDIR := $(HOSTGENDIR)/$(CURDIR)/gcc/$(AROS_TARGET_CPU)-aros/libatomic",
        ) && lines.contains(
            "LIBATOMIC_SRCDIR := $(HOSTDIR)/Ports/host/gcc/gcc-$(GCC_VERSION)/libatomic",
        ) && lines.contains(
            "#MM tools-crosstools-gcc-libatomic : crosstools-gcc--fetch tools-crosstools-autolibs linklibs-$(AROS_TARGET_CPU)",
        )
    });
    Ok(ToolchainProvisioningContext {
        llvm,
        gcc_libatomic,
    })
}

fn is_toolchain_provisioning_declaration(
    declaration: &Declaration,
    context: ToolchainProvisioningContext,
) -> bool {
    (context.llvm
        && declaration.file == LLVM_PROVISIONING_FILE
        && declaration.macro_name == "build_with_cmake"
        && LLVM_PROVISIONING_DECLARATIONS
            .iter()
            .any(|(mmake, arguments)| {
                declaration.mmake == *mmake && declaration.arguments == *arguments
            }))
        || (context.gcc_libatomic
            && declaration.file == GCC_PROVISIONING_FILE
            && declaration.macro_name == "build_with_configure"
            && declaration.mmake == "tools-crosstools-gcc-libatomic"
            && declaration.arguments == GCC_LIBATOMIC_ARGUMENTS)
}

fn split_toolchain_provisioning<'a>(
    declarations: &[&'a Declaration],
    context: ToolchainProvisioningContext,
) -> (Vec<&'a Declaration>, Vec<&'a Declaration>) {
    declarations
        .iter()
        .copied()
        .partition(|declaration| is_toolchain_provisioning_declaration(declaration, context))
}

fn is_inactive_profile_declaration(
    declaration: &Declaration,
    scope: Option<&ArchitectureScope>,
) -> bool {
    scope.is_some_and(|scope| {
        scope.bootloader != "grub"
            && declaration.file == LEGACY_GRUB_FILE
            && declaration.macro_name == "build_with_configure"
            && declaration.mmake == "grub"
            && declaration.arguments == LEGACY_GRUB_ARGUMENTS
    })
}

fn split_inactive_profile<'a>(
    declarations: &[&'a Declaration],
    scope: Option<&ArchitectureScope>,
) -> (Vec<&'a Declaration>, Vec<&'a Declaration>) {
    declarations
        .iter()
        .copied()
        .partition(|declaration| is_inactive_profile_declaration(declaration, scope))
}

fn collect_manual_aggregate_declarations(root: &Path) -> CoverageReadResult<Vec<Declaration>> {
    const FILE: &str = "compiler/libhiddstubs/mmakefile.src";
    let path = root.join(FILE);
    let Some(content) =
        read_optional_coverage_source(&path, CoverageReadPhase::ManualAggregateRead)?
    else {
        return Ok(Vec::new());
    };
    let semantics = canonical_make_semantics(&content);
    let lines: BTreeSet<&str> = semantics.lines().collect();
    let required = [
        "#MM- linklibs : linklibs-hiddstubs",
        "#MM- linklibs-hiddstubs: linklibs-hidd-stubs",
        "HIDD_LIB := $(AROS_LIB)/libhiddstubs.a",
        "HIDD_STUBS_OBJ := $(strip $(call WILDCARD, $(GENDIR)/lib/hidd/*.o))",
        "linklibs-hiddstubs: $(HIDD_LIB)",
        "$(HIDD_LIB) : $(HIDD_STUBS_OBJ)",
        "%mklib_q from=$^",
    ];
    if !required.into_iter().all(|line| lines.contains(line))
        || semantics
            .lines()
            .filter(|line| line.starts_with("linklibs-hiddstubs:"))
            .count()
            != 1
    {
        return Ok(Vec::new());
    }
    Ok(vec![Declaration {
        mmake: "linklibs-hiddstubs".to_owned(),
        macro_name: "manual_archive".to_owned(),
        file: FILE.to_owned(),
        arguments: required.join(" | "),
    }])
}

/// What the reference expansion says about one target.
#[derive(Debug, Clone, Default)]
struct RefShape {
    /// `<target>_PROGNAME`, set for `%build_prog`.
    progname: Option<String>,
    /// Whether the expansion carries a module's target list.
    is_module: bool,
}

/// Parse, execute, log, and render one verifier invocation.
#[must_use]
pub fn entry(arguments: Vec<OsString>) -> ExitCode {
    let requested_format = requested_diagnostic_format(&arguments, "AROS_VERIFY_DIAGNOSTIC_FORMAT");
    let args = match Args::try_parse_from(arguments) {
        Ok(args) => args,
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            return match aros_common::write_stdout(&error.to_string()) {
                Ok(()) => ExitCode::SUCCESS,
                Err(output_error) => {
                    render_diagnostics(
                        &DiagnosticSet::single(
                            Diagnostic::error(
                                DiagnosticCode::VerifyObservability,
                                DiagnosticStage::Observability,
                                format!("could not write command help: {output_error}"),
                            )
                            .with_hint("check the stdout destination and retry"),
                        ),
                        requested_format,
                        OBSERVABILITY_POLICY,
                    );
                    ExitCode::FAILURE
                }
            };
        }
        Err(error) => {
            render_diagnostics(
                &DiagnosticSet::single(
                    Diagnostic::error(
                        DiagnosticCode::VerifyInvocation,
                        DiagnosticStage::Invocation,
                        error.to_string().trim().to_owned(),
                    )
                    .with_hint("run 'aros-verify --help' and provide --generated and --work paths"),
                ),
                requested_format,
                OBSERVABILITY_POLICY,
            );
            return ExitCode::FAILURE;
        }
    };
    let logger = match Logger::open(
        args.log_level,
        args.log_format,
        args.log_file.clone(),
        "aros-verify",
        OBSERVABILITY_POLICY,
    ) {
        Ok(logger) => logger,
        Err(error) => {
            render_diagnostics(
                &DiagnosticSet::single(error.into_diagnostic()),
                args.diagnostic_format,
                OBSERVABILITY_POLICY,
            );
            return ExitCode::FAILURE;
        }
    };
    let context = DiagnosticContext {
        target: Some(args.source.display().to_string()),
        output: Some(args.work.display().to_string()),
        ..DiagnosticContext::default()
    };
    if let Err(error) = logger.event(
        LogLevel::Info,
        "invocation.start",
        "independent verifier invocation started",
        &context,
    ) {
        render_diagnostics(
            &DiagnosticSet::single(error.into_diagnostic()),
            args.diagnostic_format,
            OBSERVABILITY_POLICY,
        );
        return ExitCode::FAILURE;
    }
    if let Err(error) = run(&args) {
        let mut diagnostics = vec![error.into_diagnostic()];
        if let Err(log_error) = logger.diagnostic(&diagnostics[0]) {
            diagnostics.push(log_error.into_diagnostic());
        }
        render_diagnostics(
            &DiagnosticSet::new(diagnostics),
            args.diagnostic_format,
            OBSERVABILITY_POLICY,
        );
        return ExitCode::FAILURE;
    }
    if let Some(diagnostic) = aros_common::take_stdout_failure_diagnostic(
        DiagnosticCode::VerifyObservability,
        DiagnosticStage::Observability,
    ) {
        let mut diagnostics = vec![diagnostic];
        if let Err(log_error) = logger.diagnostic(&diagnostics[0]) {
            diagnostics.push(log_error.into_diagnostic());
        }
        render_diagnostics(
            &DiagnosticSet::new(diagnostics),
            args.diagnostic_format,
            OBSERVABILITY_POLICY,
        );
        return ExitCode::FAILURE;
    }
    if let Err(error) = logger.event(
        LogLevel::Info,
        "invocation.complete",
        "independent verifier invocation completed",
        &context,
    ) {
        render_diagnostics(
            &DiagnosticSet::single(error.into_diagnostic()),
            args.diagnostic_format,
            OBSERVABILITY_POLICY,
        );
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn run(args: &Args) -> std::result::Result<(), VerifyFailure> {
    validate_profile_arguments(args).map_err(|error| {
        VerifyFailure::new(
            DiagnosticCode::VerifyInput,
            DiagnosticStage::Configuration,
            format!("invalid verifier profile: {error:#}"),
        )
    })?;
    let architecture = ArchitectureScope::from_args(args);
    let root = args.source.canonicalize().map_err(|error| {
        VerifyFailure::at_path(
            DiagnosticCode::VerifyInput,
            DiagnosticStage::RepositoryDiscovery,
            format!("source tree cannot be opened: {error}"),
            &args.source,
        )
    })?;

    fs::create_dir_all(&args.work).map_err(|error| {
        VerifyFailure::at_path(
            DiagnosticCode::VerifyPublication,
            DiagnosticStage::OutputPublication,
            format!("cannot create verifier work directory: {error}"),
            &args.work,
        )
    })?;
    let cache = args.work.join("genmf");
    fs::create_dir_all(&cache).map_err(|error| {
        VerifyFailure::at_path(
            DiagnosticCode::VerifyPublication,
            DiagnosticStage::OutputPublication,
            format!("cannot create verifier expansion cache: {error}"),
            &cache,
        )
    })?;
    let report_dir = architecture
        .as_ref()
        .map_or_else(|| args.work.clone(), |scope| args.work.join(scope.key()));
    fs::create_dir_all(&report_dir).map_err(|error| {
        VerifyFailure::at_path(
            DiagnosticCode::VerifyPublication,
            DiagnosticStage::OutputPublication,
            format!("cannot create verifier report directory: {error}"),
            &report_dir,
        )
    })?;

    let mmakefiles = find_mmakefiles(&root).map_err(coverage_read_failure)?;
    if mmakefiles.is_empty() {
        return Err(VerifyFailure::at_path(
            DiagnosticCode::VerifyInput,
            DiagnosticStage::SourceWalk,
            "source tree contains no mmakefile or mmakefile.src",
            &root,
        ));
    }

    // 1. What the tree declares. Read straight from the mmakefiles, with line
    //    continuations joined, so this measure does not depend on the
    //    transpiler's own parser being right.
    let mut declarations =
        collect_declarations(&root, &mmakefiles).map_err(coverage_read_failure)?;
    let manual_aggregates =
        collect_manual_aggregate_declarations(&root).map_err(coverage_read_failure)?;
    declarations.extend(manual_aggregates.iter().cloned());
    // The global report deliberately remains a raw tree inventory.  A
    // concrete architecture report additionally evaluates Make conditionals
    // with the target values CMake supplied.  Directory filtering alone is
    // insufficient: several shared mmakefiles declare 32-bit companions only
    // when AROS_TARGET_CPU32 is non-empty.
    let conditional_declarations = architecture
        .as_ref()
        .map(|scope| {
            collect_declarations_for_profile(&root, &mmakefiles, scope).map(|mut declarations| {
                declarations.extend(manual_aggregates.iter().cloned());
                declarations
            })
        })
        .transpose()
        .map_err(coverage_read_failure)?;
    let declaration_candidates = conditional_declarations
        .as_deref()
        .unwrap_or(declarations.as_slice());
    let scoped_inventory: Vec<&Declaration> = declaration_candidates
        .iter()
        .filter(|declaration| {
            architecture
                .as_ref()
                .is_none_or(|scope| scope.declaration_is_eligible(declaration))
        })
        .collect();
    let provisioning_context =
        detect_toolchain_provisioning_context(&root).map_err(coverage_read_failure)?;
    let (toolchain_provisioning, target_candidates) =
        split_toolchain_provisioning(&scoped_inventory, provisioning_context);
    let (inactive_profile, scoped_declarations) =
        split_inactive_profile(&target_candidates, architecture.as_ref());

    // 2. What the historic build makes of it.
    let expansion = expand_all(
        &root,
        &cache,
        &mmakefiles,
        args.refresh,
        Duration::from_secs(args.genmf_timeout_seconds),
    );
    let shapes = collect_shapes(&expansion.expanded).map_err(coverage_read_failure)?;
    let expansion_failures: Vec<String> = expansion
        .failures
        .iter()
        .filter(|failure| {
            architecture
                .as_ref()
                .is_none_or(|scope| scope.file_is_eligible(&failure.file))
        })
        .map(|failure| failure.message.clone())
        .collect();

    // 3. What we produced.
    let generated = fs::read_to_string(&args.generated).map_err(|error| {
        VerifyFailure::at_path(
            DiagnosticCode::VerifyInput,
            DiagnosticStage::Parsing,
            format!("cannot read generated target graph: {error}"),
            &args.generated,
        )
    })?;
    let ours = collect_ours(&generated);

    // ---- Coverage -------------------------------------------------------

    let all_declared: BTreeSet<&str> = declarations.iter().map(|d| d.mmake.as_str()).collect();
    let declared: BTreeSet<&str> = scoped_declarations
        .iter()
        .map(|d| d.mmake.as_str())
        .collect();
    let missing: Vec<&Declaration> = scoped_declarations
        .iter()
        .copied()
        .filter(|d| !ours.contains_key(&d.mmake))
        .collect();

    let mut by_macro: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for d in &scoped_declarations {
        let e = by_macro.entry(d.macro_name.as_str()).or_default();
        e.0 += 1;
        if !ours.contains_key(&d.mmake) {
            e.1 += 1;
        }
    }

    // A target we emit that the tree never declares points at a naming or
    // splitting mistake, which is how the %build_prog / %build_progs mix-up
    // showed up: four executables named after source files instead of one
    // named by progname.
    // Generated output is target-agnostic, and an undeclared id has no source
    // path from which an architecture can be inferred. Keep this global
    // integrity error in every profile rather than guessing it away.
    let undeclared: Vec<&String> = ours
        .keys()
        .filter(|k| !all_declared.contains(k.as_str()))
        .collect();
    let emitted: Vec<&String> = ours
        .keys()
        .filter(|id| {
            architecture.is_none()
                || declared.contains(id.as_str())
                // There is no architecture evidence for an undeclared id.
                // Keep it in every scoped integrity gate rather than silently
                // assigning it to an arbitrary architecture.
                || !all_declared.contains(id.as_str())
        })
        .collect();

    // ---- Realisation ----------------------------------------------------
    //
    // Coverage above measures what the transpiler emitted, which is not the
    // same as what CMake built. A declaration emitted with an empty source
    // list makes every builder return early, so the target never exists, and
    // nothing said so: aros_add_custom_target was an empty stub for 97
    // declarations with 313 source files and this check would have caught it
    // on the first run.
    //
    // CMakeFiles/<id>.dir is the evidence. CMake creates it for any target it
    // configured, and for none it did not.
    let unrealised = args.build_dir.as_ref().map_or_else(Vec::new, |dir| {
        let cmakefiles = dir.join("CMakeFiles");
        let mut present: BTreeSet<String> = BTreeSet::new();
        if let Ok(entries) = fs::read_dir(&cmakefiles) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if let Some(stem) = name.strip_suffix(".dir") {
                    present.insert(stem.to_owned());
                }
            }
        }
        // A package or kickstart declaration becomes an add_custom_target,
        // which gets no CMakeFiles/<id>.dir. Ninja records it as a phony
        // edge instead, so both places have to be read or the four
        // pc-x86_64 packages read as unrealised.
        if let Ok(ninja) = fs::read_to_string(dir.join("build.ninja")) {
            let phony = Regex::new(r"(?m)^build ([^:$ ]+): phony").unwrap();
            for c in phony.captures_iter(&ninja) {
                if let Some(name) = c[1].rsplit('/').next() {
                    present.insert(name.to_owned());
                }
            }
        }
        if present.is_empty() {
            vec![format!(
                "cannot read {} -- configure the build first",
                cmakefiles.display()
            )]
        } else {
            emitted
                .iter()
                .copied()
                .filter(|id| !present.contains(id.as_str()))
                .map(|id| {
                    let where_ = scoped_declarations
                        .iter()
                        .find(|d| d.mmake.as_str() == id.as_str())
                        .map_or_else(|| "?".to_owned(), |d| d.file.clone());
                    format!("{id:44} {where_}")
                })
                .collect()
        }
    });

    // ---- Shape ----------------------------------------------------------

    let mut wrong_name = Vec::new();
    for (mmake, target) in &ours {
        if architecture.is_some() && !declared.contains(mmake.as_str()) {
            continue;
        }
        if let Some(shape) = shapes.get(mmake) {
            if let Some(expected) = &shape.progname {
                if !expected.eq_ignore_ascii_case(target) {
                    wrong_name.push(format!(
                        "{mmake}: reference builds {expected}, we build {target}"
                    ));
                }
            }
        }
    }

    // ---- Report ---------------------------------------------------------

    let pct = if declared.is_empty() {
        100.0
    } else {
        100.0 * (declared.len() - missing.len()) as f64 / declared.len() as f64
    };

    aros_common::outputln!("📐 aros-verify");
    if let Some(scope) = &architecture {
        let profile = args.profile.unwrap_or(Profile::Architecture);
        aros_common::outputln!(
            "   scope         {} {}-{}",
            profile.as_str(),
            scope.cpu,
            scope.platform
        );
        aros_common::outputln!(
            "   reachability  not filtered (no verified core/distribution roots available)"
        );
    }
    aros_common::outputln!(
        "   coverage      {}/{} declared targets ({pct:.1}%)",
        declared.len() - missing.len(),
        declared.len()
    );
    aros_common::outputln!(
        "   provisioning  {} upstream toolchain target(s) tracked outside the target graph",
        toolchain_provisioning.len()
    );
    if !inactive_profile.is_empty() {
        aros_common::outputln!(
            "   inactive      {} declaration(s) excluded by explicit target configuration",
            inactive_profile.len()
        );
    }
    let reference_count = if architecture.is_some() {
        shapes
            .keys()
            .filter(|id| declared.contains(id.as_str()))
            .count()
    } else {
        shapes.len()
    };
    aros_common::outputln!("   reference     {reference_count} targets in the genmf expansion");
    aros_common::outputln!("   emitted       {} MMAKE_IDs", emitted.len());
    if args.build_dir.is_some() {
        let built = emitted.len().saturating_sub(unrealised.len());
        aros_common::outputln!(
            "   realised      {built}/{} emitted became CMake targets",
            emitted.len()
        );
    }

    write_failure_report(
        &report_dir.join("genmf-errors.txt"),
        expansion_failures.clone(),
        &format!(
            "{} mmakefile(s) could not be expanded by genmf",
            expansion_failures.len()
        ),
    )
    .map_err(|error| report_failure(&report_dir, &error))?;

    write_inventory_report(
        &report_dir.join("toolchain-provisioning-targets.txt"),
        toolchain_provisioning
            .iter()
            .map(|declaration| {
                format!(
                    "{:32} %{:22} {}",
                    declaration.mmake, declaration.macro_name, declaration.file
                )
            })
            .collect(),
    )
    .map_err(|error| report_failure(&report_dir, &error))?;

    write_named_inventory_report(
        &report_dir.join("inactive-profile-targets.txt"),
        inactive_profile
            .iter()
            .map(|declaration| {
                format!(
                    "{:32} %{:22} {}  AROS_TARGET_BOOTLOADER={}",
                    declaration.mmake,
                    declaration.macro_name,
                    declaration.file,
                    architecture
                        .as_ref()
                        .map_or("<unset>", |scope| scope.bootloader.as_str())
                )
            })
            .collect(),
        "inactive target-profile inventory",
    )
    .map_err(|error| report_failure(&report_dir, &error))?;

    write_report(
        &report_dir.join("missing-targets.txt"),
        missing
            .iter()
            .map(|d| format!("{:32} %{:22} {}", d.mmake, d.macro_name, d.file))
            .collect(),
        &format!("{} declared target(s) not transpiled", missing.len()),
    )
    .map_err(|error| report_failure(&report_dir, &error))?;

    write_report(
        &report_dir.join("undeclared-targets.txt"),
        undeclared.iter().map(|s| (*s).clone()).collect(),
        &format!(
            "{} emitted target(s) the tree does not declare",
            undeclared.len()
        ),
    )
    .map_err(|error| report_failure(&report_dir, &error))?;

    write_report(
        &report_dir.join("wrong-program-name.txt"),
        wrong_name.clone(),
        &format!("{} target(s) built under the wrong name", wrong_name.len()),
    )
    .map_err(|error| report_failure(&report_dir, &error))?;

    write_report(
        &report_dir.join("unrealised-targets.txt"),
        unrealised.clone(),
        &format!(
            "{} emitted declaration(s) never became a CMake target",
            unrealised.len()
        ),
    )
    .map_err(|error| report_failure(&report_dir, &error))?;

    if !by_macro.is_empty() {
        aros_common::outputln!("\n   {:24} {:>8} {:>8}", "macro", "declared", "missing");
        for (m, (total, miss)) in &by_macro {
            if *miss > 0 {
                aros_common::outputln!("   %{m:23} {total:8} {miss:8}");
            }
        }
    }

    let failed = !missing.is_empty()
        || !undeclared.is_empty()
        || !wrong_name.is_empty()
        || !unrealised.is_empty()
        || !expansion_failures.is_empty();
    if failed && !args.no_gate {
        return Err(verification_gap_failure(&report_dir, &expansion.failures));
    }
    Ok(())
}

fn verification_gap_failure(
    report_dir: &Path,
    expansion_failures: &[ExpansionFailure],
) -> VerifyFailure {
    let timed_out = expansion_failures.iter().find(|failure| failure.timed_out);
    let mut diagnostic = Diagnostic::error(
        DiagnosticCode::VerifyParity,
        DiagnosticStage::GraphValidation,
        timed_out.map_or_else(
            || "verification found gaps; inspect the deterministic reports".to_owned(),
            |_| {
                "verification found gaps after at least one genmf expansion timed out; inspect the deterministic reports"
                    .to_owned()
            },
        ),
    )
    .with_location(SourceLocation::new(report_dir.display().to_string()))
    .with_hint(
        "inspect genmf-errors.txt; fix the hanging expansion or deliberately adjust --genmf-timeout-seconds before retrying",
    );
    if let Some(timeout) = timed_out {
        diagnostic = diagnostic.with_context(DiagnosticContext {
            tool: Some("python3 tools/genmf/genmf.py".to_owned()),
            target: Some(timeout.file.clone()),
            timed_out: Some(true),
            timeout_ms: timeout.timeout_ms,
            ..DiagnosticContext::default()
        });
    }
    VerifyFailure {
        diagnostic: Box::new(diagnostic),
    }
}

fn report_failure(report_dir: &Path, error: &anyhow::Error) -> VerifyFailure {
    VerifyFailure::at_path(
        DiagnosticCode::VerifyPublication,
        DiagnosticStage::OutputPublication,
        format!("cannot publish verifier report: {error:#}"),
        report_dir,
    )
}

fn write_report(path: &Path, mut lines: Vec<String>, headline: &str) -> Result<()> {
    if lines.is_empty() {
        let _ = fs::remove_file(path);
        aros_common::outputln!("   ✅ {headline}");
        return Ok(());
    }
    lines.sort_unstable();
    lines.dedup();
    fs::write(path, lines.join("\n") + "\n")?;
    aros_common::outputln!("   ⚠️  {headline} -> {}", path.display());
    Ok(())
}

/// Writes only actionable reference-expansion failures. A clean run removes a
/// stale report without adding a line to the long-established global output.
fn write_failure_report(path: &Path, mut lines: Vec<String>, headline: &str) -> Result<()> {
    if lines.is_empty() {
        let _ = fs::remove_file(path);
        return Ok(());
    }
    lines.sort_unstable();
    lines.dedup();
    fs::write(path, lines.join("\n") + "\n")?;
    aros_common::outputln!("   ⚠️  {headline} -> {}", path.display());
    Ok(())
}

/// Writes an intentional non-gating inventory.  Unlike a failure report it is
/// expected to remain present: it prevents excluded provisioning work from
/// disappearing behind a denominator adjustment.
fn write_inventory_report(path: &Path, lines: Vec<String>) -> Result<()> {
    write_named_inventory_report(path, lines, "external toolchain provisioning inventory")
}

fn write_named_inventory_report(path: &Path, mut lines: Vec<String>, label: &str) -> Result<()> {
    if lines.is_empty() {
        let _ = fs::remove_file(path);
        return Ok(());
    }
    lines.sort_unstable();
    lines.dedup();
    fs::write(path, lines.join("\n") + "\n")?;
    aros_common::outputln!("   ℹ️  {label} -> {}", path.display());
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MakeTruth {
    False,
    True,
    Unknown,
}

impl MakeTruth {
    const fn not(self) -> Self {
        match self {
            Self::False => Self::True,
            Self::True => Self::False,
            Self::Unknown => Self::Unknown,
        }
    }

    const fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::False, _) | (_, Self::False) => Self::False,
            (Self::True, Self::True) => Self::True,
            _ => Self::Unknown,
        }
    }

    const fn or(self, other: Self) -> Self {
        match (self, other) {
            (Self::True, _) | (_, Self::True) => Self::True,
            (Self::False, Self::False) => Self::False,
            _ => Self::Unknown,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct MakeConditionalFrame {
    parent: MakeTruth,
    matched: MakeTruth,
    current: MakeTruth,
}

impl MakeConditionalFrame {
    const fn new(parent: MakeTruth, condition: MakeTruth) -> Self {
        Self {
            parent,
            matched: condition,
            current: parent.and(condition),
        }
    }

    const fn else_if(&mut self, condition: MakeTruth) {
        self.current = self.parent.and(self.matched.not()).and(condition);
        self.matched = self.matched.or(condition);
    }

    const fn otherwise(&mut self) {
        self.current = self.parent.and(self.matched.not());
        self.matched = MakeTruth::True;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MakeAssignmentKind {
    Simple,
    Recursive,
    SetIfUnset,
    Append,
}

/// File-local values needed by Make conditionals.
///
/// `None` means an assignment occurred under an unresolved guard or its value
/// could not be expanded.  It must remain unknown; converting it to an empty
/// string would incorrectly choose an `ifeq` branch.
#[derive(Default)]
struct MakeConditionScope {
    values: BTreeMap<String, Option<String>>,
    local_names: BTreeSet<String>,
}

fn make_assignment(line: &str) -> Option<(&str, &str, MakeAssignmentKind)> {
    let trimmed = line.trim();
    let (at, width, kind) = [
        (":=", MakeAssignmentKind::Simple),
        ("+=", MakeAssignmentKind::Append),
        ("?=", MakeAssignmentKind::SetIfUnset),
        ("=", MakeAssignmentKind::Recursive),
    ]
    .into_iter()
    .filter_map(|(operator, kind)| trimmed.find(operator).map(|at| (at, operator.len(), kind)))
    .min_by_key(|(at, _, _)| *at)?;
    let name = trimmed[..at].trim();
    if name.is_empty()
        || !name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
    {
        return None;
    }
    Some((name, trimmed[at + width..].trim(), kind))
}

fn strip_make_comment(line: &str) -> &str {
    for (at, character) in line.char_indices() {
        if character != '#' {
            continue;
        }
        let escaped = line[..at]
            .bytes()
            .rev()
            .take_while(|byte| *byte == b'\\')
            .count()
            % 2
            == 1;
        if !escaped {
            return &line[..at];
        }
    }
    line
}

fn make_directive_tail<'a>(line: &'a str, word: &str) -> Option<&'a str> {
    let tail = line.strip_prefix(word)?;
    (tail.is_empty()
        || tail
            .chars()
            .next()
            .is_some_and(|character| character.is_whitespace() || character == '('))
    .then(|| tail.trim())
}

fn split_top_level_comma(raw: &str) -> Option<(&str, &str)> {
    let mut parentheses = 0usize;
    let mut braces = 0usize;
    let mut quote = None;
    for (at, character) in raw.char_indices() {
        if let Some(delimiter) = quote {
            if character == delimiter {
                quote = None;
            }
            continue;
        }
        match character {
            '\'' | '"' => quote = Some(character),
            '(' => parentheses += 1,
            ')' => parentheses = parentheses.saturating_sub(1),
            '{' => braces += 1,
            '}' => braces = braces.saturating_sub(1),
            ',' if parentheses == 0 && braces == 0 => {
                return Some((&raw[..at], &raw[at + 1..]));
            }
            _ => {}
        }
    }
    None
}

fn take_condition_word(raw: &str) -> Option<(&str, &str)> {
    let raw = raw.trim_start();
    let first = raw.chars().next()?;
    if matches!(first, '\'' | '"') {
        let after_quote = &raw[first.len_utf8()..];
        let end = after_quote.find(first)?;
        return Some((&raw[..end + 2], &after_quote[end + 1..]));
    }
    let end = raw.find(char::is_whitespace).unwrap_or(raw.len());
    Some((&raw[..end], &raw[end..]))
}

fn equality_operands(raw: &str) -> Option<(&str, &str)> {
    let raw = raw.trim();
    if raw.starts_with('(') && raw.ends_with(')') {
        return split_top_level_comma(&raw[1..raw.len() - 1]);
    }
    let (left, rest) = take_condition_word(raw)?;
    let (right, trailing) = take_condition_word(rest)?;
    trailing.trim().is_empty().then_some((left, right))
}

fn unquote_condition_value(raw: &str) -> &str {
    let raw = raw.trim();
    if raw.len() >= 2 {
        let bytes = raw.as_bytes();
        if matches!(bytes[0], b'\'' | b'"') && bytes[0] == bytes[raw.len() - 1] {
            return &raw[1..raw.len() - 1];
        }
    }
    raw
}

fn condition_pattern_matches(pattern: &str, word: &str) -> bool {
    let Some(percent) = pattern.find('%') else {
        return pattern == word;
    };
    let prefix = &pattern[..percent];
    let suffix = &pattern[percent + 1..];
    word.len() >= prefix.len() + suffix.len() && word.starts_with(prefix) && word.ends_with(suffix)
}

fn expand_condition_function(
    body: &str,
    variables: &MakeConditionScope,
    target: &ArchitectureScope,
    depth: usize,
) -> Option<String> {
    let split = body.find(char::is_whitespace)?;
    let name = body[..split].trim();
    let arguments = body[split..].trim();
    match name {
        "strip" => Some(
            expand_condition_operand(arguments, variables, target, depth - 1)?
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
        ),
        "findstring" => {
            let (needle, haystack) = split_top_level_comma(arguments)?;
            let needle = expand_condition_operand(needle, variables, target, depth - 1)?;
            let haystack = expand_condition_operand(haystack, variables, target, depth - 1)?;
            Some(if haystack.contains(&needle) {
                needle
            } else {
                String::new()
            })
        }
        "filter" | "filter-out" => {
            let (patterns, words) = split_top_level_comma(arguments)?;
            let patterns = expand_condition_operand(patterns, variables, target, depth - 1)?;
            let words = expand_condition_operand(words, variables, target, depth - 1)?;
            let keep_matches = name == "filter";
            Some(
                words
                    .split_whitespace()
                    .filter(|word| {
                        let matches = patterns
                            .split_whitespace()
                            .any(|pattern| condition_pattern_matches(pattern, word));
                        matches == keep_matches
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
            )
        }
        _ => None,
    }
}

fn expand_condition_reference(
    body: &str,
    variables: &MakeConditionScope,
    target: &ArchitectureScope,
    depth: usize,
) -> Option<String> {
    let body = body.trim();
    if !body.is_empty()
        && body
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
    {
        if let Some(value) = variables.values.get(body) {
            return value
                .as_deref()
                .and_then(|value| expand_condition_operand(value, variables, target, depth - 1));
        }
        if let Some(value) = target.make_value(body) {
            return Some(value);
        }
        return variables.local_names.contains(body).then(String::new);
    }
    expand_condition_function(body, variables, target, depth)
}

fn expand_condition_operand(
    raw: &str,
    variables: &MakeConditionScope,
    target: &ArchitectureScope,
    depth: usize,
) -> Option<String> {
    if depth == 0 {
        return None;
    }
    let mut output = String::with_capacity(raw.len());
    let mut cursor = 0usize;
    while cursor < raw.len() {
        let Some(relative) = raw[cursor..].find('$') else {
            output.push_str(&raw[cursor..]);
            break;
        };
        let dollar = cursor + relative;
        output.push_str(&raw[cursor..dollar]);
        let next = *raw.as_bytes().get(dollar + 1)?;
        if next == b'$' {
            output.push('$');
            cursor = dollar + 2;
            continue;
        }
        let (open, close) = match next {
            b'(' => (b'(', b')'),
            b'{' => (b'{', b'}'),
            _ => return None,
        };
        let mut nesting = 1usize;
        let mut end = dollar + 2;
        while end < raw.len() {
            let byte = raw.as_bytes()[end];
            if byte == b'$' && raw.as_bytes().get(end + 1) == Some(&open) {
                nesting += 1;
                end += 2;
                continue;
            }
            if byte == close {
                nesting -= 1;
                if nesting == 0 {
                    break;
                }
            }
            end += 1;
        }
        if end == raw.len() {
            return None;
        }
        output.push_str(&expand_condition_reference(
            &raw[dollar + 2..end],
            variables,
            target,
            depth - 1,
        )?);
        cursor = end + 1;
    }
    Some(unquote_condition_value(output.trim()).to_owned())
}

fn evaluate_make_conditional(
    directive: &str,
    arguments: &str,
    variables: &MakeConditionScope,
    target: &ArchitectureScope,
) -> MakeTruth {
    const MAX_EXPANSION_DEPTH: usize = 16;
    let value = match directive {
        "ifeq" | "ifneq" => equality_operands(arguments).and_then(|(left, right)| {
            Some(
                expand_condition_operand(left, variables, target, MAX_EXPANSION_DEPTH)?
                    == expand_condition_operand(right, variables, target, MAX_EXPANSION_DEPTH)?,
            )
        }),
        "ifdef" | "ifndef" => {
            let name = arguments.trim();
            let value = variables
                .values
                .get(name)
                .map_or_else(|| target.make_value(name), Clone::clone);
            value.map(|value| !value.is_empty())
        }
        _ => None,
    };
    let Some(mut value) = value else {
        return MakeTruth::Unknown;
    };
    if matches!(directive, "ifneq" | "ifndef") {
        value = !value;
    }
    if value {
        MakeTruth::True
    } else {
        MakeTruth::False
    }
}

/// Selects the branch state for each logical line using only target values we
/// know.  An unresolved condition stays `Unknown`; callers retain declarations
/// from both possible branches so profile mode never hides an unsupported or
/// externally configured declaration merely by guessing its value.
fn make_conditional_line_states(joined: &str, target: &ArchitectureScope) -> Vec<MakeTruth> {
    const MAX_EXPANSION_DEPTH: usize = 16;
    let mut variables = MakeConditionScope::default();
    let mut stack: Vec<MakeConditionalFrame> = Vec::new();
    let mut states = Vec::with_capacity(joined.lines().count());

    for raw_line in joined.lines() {
        let branch_state = stack.last().map_or(MakeTruth::True, |frame| frame.current);
        states.push(branch_state);

        let line = strip_make_comment(raw_line);
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.starts_with('%') {
            continue;
        }
        if let Some((directive, arguments)) = ["ifeq", "ifneq", "ifdef", "ifndef"]
            .into_iter()
            .find_map(|word| make_directive_tail(trimmed, word).map(|tail| (word, tail)))
        {
            let parent = stack.last().map_or(MakeTruth::True, |frame| frame.current);
            let condition = evaluate_make_conditional(directive, arguments, &variables, target);
            stack.push(MakeConditionalFrame::new(parent, condition));
            continue;
        }
        if trimmed == "endif" {
            stack.pop();
            continue;
        }
        if trimmed == "else" || trimmed.starts_with("else ") {
            // Evaluate an `else ifeq` against the scope before borrowing the
            // top frame mutably.
            let condition = trimmed
                .strip_prefix("else")
                .map(str::trim)
                .filter(|tail| !tail.is_empty())
                .map(|tail| {
                    ["ifeq", "ifneq", "ifdef", "ifndef"]
                        .into_iter()
                        .find_map(|word| {
                            make_directive_tail(tail, word).map(|arguments| (word, arguments))
                        })
                        .map_or(MakeTruth::Unknown, |(directive, arguments)| {
                            evaluate_make_conditional(directive, arguments, &variables, target)
                        })
                });
            if let Some(frame) = stack.last_mut() {
                if let Some(condition) = condition {
                    frame.else_if(condition);
                } else {
                    frame.otherwise();
                }
            }
            continue;
        }

        let Some((name, value, kind)) = make_assignment(line) else {
            continue;
        };
        variables.local_names.insert(name.to_owned());
        if branch_state == MakeTruth::False {
            continue;
        }
        if branch_state == MakeTruth::Unknown {
            variables.values.insert(name.to_owned(), None);
            continue;
        }
        if kind == MakeAssignmentKind::SetIfUnset
            && (variables.values.contains_key(name) || target.make_value(name).is_some())
        {
            continue;
        }

        let value = match kind {
            MakeAssignmentKind::Simple => {
                expand_condition_operand(value, &variables, target, MAX_EXPANSION_DEPTH)
            }
            MakeAssignmentKind::Append => match variables.values.get(name) {
                Some(Some(old)) => Some(if old.is_empty() || value.is_empty() {
                    format!("{old}{value}")
                } else {
                    format!("{old} {value}")
                }),
                Some(None) => None,
                None => Some(value.to_owned()),
            },
            MakeAssignmentKind::Recursive | MakeAssignmentKind::SetIfUnset => {
                Some(value.to_owned())
            }
        };
        variables.values.insert(name.to_owned(), value);
    }
    states
}

/// Reads every `%build_* ... mmake=<name>` from the tree.
///
/// Line continuations are joined first: most declarations spread their
/// arguments over several lines, and `mmake=` is often not on the first one.
fn collect_declarations(root: &Path, files: &[PathBuf]) -> CoverageReadResult<Vec<Declaration>> {
    collect_declarations_impl(root, files, None)
}

fn collect_declarations_for_profile(
    root: &Path,
    files: &[PathBuf],
    target: &ArchitectureScope,
) -> CoverageReadResult<Vec<Declaration>> {
    collect_declarations_impl(root, files, Some(target))
}

fn collect_declarations_impl(
    root: &Path,
    files: &[PathBuf],
    target: Option<&ArchitectureScope>,
) -> CoverageReadResult<Vec<Declaration>> {
    // genmf removes the backslash/newline pair and joins exactly the next
    // physical line, retaining its indentation. Do not let whitespace matching
    // cross an intervening blank line and consume the declaration after it.
    let cont = Regex::new(r"\\\r?\n").unwrap();
    let decl = Regex::new(r"^\s*%(build_\w+|make_package|link_kickstart)\b([^\n]*)").unwrap();
    let mmake = Regex::new(r"\bmmake=([\w.-]+)").unwrap();

    let mut out = Vec::new();
    for file in files {
        let text = read_source(file).map_err(|error| {
            CoverageReadError::new(CoverageReadPhase::DeclarationRead, file, error)
        })?;
        let joined = cont.replace_all(&text, "");
        let states = target.map(|target| make_conditional_line_states(&joined, target));
        let relative = file
            .strip_prefix(root)
            .unwrap_or(file)
            .to_string_lossy()
            .to_string();
        for (line_number, line) in joined.lines().enumerate() {
            // A definitely false branch cannot exist in the concrete genmf
            // reference. Unknown guards remain in the denominator so a
            // profile report is conservative instead of silently incomplete.
            if states.as_ref().and_then(|states| states.get(line_number)) == Some(&MakeTruth::False)
            {
                continue;
            }
            let Some(captures) = decl.captures(line) else {
                continue;
            };
            let Some(id) = mmake.captures(&captures[2]) else {
                continue;
            };
            out.push(Declaration {
                mmake: id[1].to_owned(),
                macro_name: captures[1].to_owned(),
                file: relative.clone(),
                arguments: collapse_whitespace(&captures[2]),
            });
        }
    }
    Ok(out)
}

/// Pulls the per-target facts out of the expansions.
fn collect_shapes(
    expanded: &[(String, PathBuf)],
) -> CoverageReadResult<BTreeMap<String, RefShape>> {
    let re_prog = Regex::new(r"(?m)^([A-Za-z0-9_.][\w.-]*)_PROGNAME\s*:?=\s*(\S+)").unwrap();
    let re_mod = Regex::new(r"(?m)^([A-Za-z0-9_.][\w.-]*)_ALLTARGETS\b").unwrap();

    let sources = expanded
        .iter()
        .map(|(_, path)| {
            read_source(path).map(|text| (path, text)).map_err(|error| {
                CoverageReadError::new(CoverageReadPhase::ReferenceShapeRead, path, error)
            })
        })
        .collect::<CoverageReadResult<Vec<_>>>()?;
    let per_file: Vec<BTreeMap<String, RefShape>> = sources
        .par_iter()
        .map(|(_, text)| {
            let mut map: BTreeMap<String, RefShape> = BTreeMap::new();
            for c in re_prog.captures_iter(text) {
                let name = c[1].to_string();
                let value = c[2].to_string();
                // An unresolved Make variable tells us nothing.
                if value.contains('$') {
                    continue;
                }
                map.entry(name).or_default().progname = Some(value);
            }
            for c in re_mod.captures_iter(text) {
                map.entry(c[1].to_string()).or_default().is_module = true;
            }
            map
        })
        .collect();

    let mut all = BTreeMap::new();
    for m in per_file {
        for (k, v) in m {
            let e: &mut RefShape = all.entry(k).or_default();
            if v.progname.is_some() {
                e.progname = v.progname;
            }
            e.is_module |= v.is_module;
        }
    }
    Ok(all)
}

/// Every mmake target the generated file declares, with the name it builds
/// under.
///
/// Build targets carry `TARGET <name>` and `MMAKE_ID <id>`. Package and
/// kickstart declarations carry `NAME <id>` instead and have no separate
/// build name; counting only MMAKE_ID reported all 21 of them as missing.
fn collect_ours(generated: &str) -> BTreeMap<String, String> {
    let block_start = Regex::new(r"^\s*([A-Za-z_][A-Za-z0-9_]*)\s*\(\s*$").unwrap();
    let target_arg = Regex::new(r"^\s*TARGET\s+(\S+)\s*$").unwrap();
    let mmake_arg = Regex::new(r"^\s*MMAKE_ID\s+(\S+)\s*$").unwrap();
    let name_arg = Regex::new(r"^\s*NAME\s+(\S+)\s*$").unwrap();
    let mut out = BTreeMap::new();
    let mut lines = generated.lines();

    while let Some(line) = lines.next() {
        let Some(start) = block_start.captures(line) else {
            continue;
        };
        let function = &start[1];
        let mut target = None;
        let mut mmake = None;
        let mut name = None;

        for line in lines.by_ref() {
            if line.trim() == ")" {
                break;
            }
            if let Some(captures) = target_arg.captures(line) {
                target = Some(captures[1].to_owned());
            } else if let Some(captures) = mmake_arg.captures(line) {
                mmake = Some(captures[1].to_owned());
            } else if let Some(captures) = name_arg.captures(line) {
                name = Some(captures[1].to_owned());
            }
        }

        if let Some(mmake) = mmake {
            out.insert(mmake, target.unwrap_or_default());
        } else if matches!(function, "aros_make_package" | "aros_link_kickstart") {
            if let Some(name) = name {
                // A package has no build name of its own.
                out.entry(name).or_default();
            }
        }
    }
    out
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
