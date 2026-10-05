use crate::arch_sources::ArchSourceDecl;
use crate::copy_includes::{AdhocHeaderRule, CopyIncludesDecl, HeaderTransformDecl};
use crate::fetch::FetchDecl;
use crate::flags::FlagSet;
use crate::flexcat::{FlexCatHeaderDecl, FlexCatSourceDecl};
use crate::includes::ArchIncludeDecl;
use aros_common::Diagnostic;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The exact module-producing MetaMake macro that declared a target.
///
/// These spellings share substantial implementation, but differ in the KOBJ
/// prerequisites emitted by `config/make.tmpl`; retain the source declaration
/// rather than attempting to reconstruct it from config or generated ABI data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModuleMacroForm {
    Full,
    RuntimeOnly,
    AbiOnly,
    Simple,
}

impl ModuleMacroForm {
    /// The stable form token accepted by the generated CMake metadata helper.
    #[must_use]
    pub const fn cmake_form(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::RuntimeOnly => "runtime-only",
            Self::AbiOnly => "abi-only",
            Self::Simple => "simple",
        }
    }
}

/// Types of buildable units in AROS mmakefiles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModuleType {
    Library,
    /// `%build_module_abi`: generated headers and a link stub library, but no
    /// runtime module. Keeping this distinct from [`Library`](Self::Library)
    /// prevents package resolution from treating an ABI skeleton as a file.
    Abi,
    /// Source-bound genmodule headers from a full declaration whose runtime
    /// capability was rejected. This creates neither a runtime nor an archive.
    ModuleHeaders,
    Device,
    Resource,
    Hidd,
    Datatype,
    Gadget,
    Mcc,
    Program,
    /// `%build_progs`: one executable per source file, under one mmake name.
    ProgramGroup,
    LinkLib,
    /// `%build_module_simple`: a module linked without the genmodule chain,
    /// so it has no .conf and no generated libdefs header.
    SimpleModule,
    Package,
    Custom,
}

/// Exact client-link metadata carried by a full genmodule declaration.
///
/// `linklibfiles=` are compiled specifically for both normal and relative
/// client archives. `linklibobjs=` names implementation objects reused by
/// those archives; the parser maps them back to declaration-owned sources so
/// CMake can reproduce them without depending on opaque legacy object paths.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GenmoduleLinklibs {
    /// Whether this declaration must materialise its client archives. Explicit
    /// `linklibname=` sets this immediately; the dependency graph may also set
    /// it for a module required by another config's `rellib` directive.
    pub enabled: bool,
    pub has_relative: bool,
    pub relative_libraries: Vec<String>,
    pub source_files: Vec<String>,
    pub object_sources: Vec<String>,
    /// False if any explicit archive input could not be represented exactly.
    pub inputs_exact: bool,
}

/// A parsed build target definition from an mmakefile.src.
#[derive(Debug, Clone, Serialize, Deserialize)]
// These booleans are independent facts from the legacy declarations and build
// graph. Collapsing them into a mode enum would admit invalid combinations or
// hide the distinction needed while canonical link-library ownership resolves.
#[allow(clippy::struct_excessive_bools)]
pub struct TargetDefinition {
    pub mmake_name: String,
    pub target_name: String,
    pub module_type: ModuleType,
    /// Exact `%build_module*` spelling, when this declaration came from one.
    /// `None` for every other concrete target macro. This metadata is source
    /// provenance; config-derived genmodule ABI facts remain separate.
    #[serde(default)]
    pub module_macro: Option<ModuleMacroForm>,
    /// Declaration-scoped native partial-link inputs. Absence means that no
    /// target-conditioned source scope was captured, never an empty input set.
    #[serde(default)]
    pub kobj_scoped_inputs: Option<crate::kobj_scoped_inputs::KobjScopedInputs>,
    /// The module has no hand-written sources because genmodule supplies its
    /// complete runtime implementation. This is deliberately set only for an
    /// explicit `files=""`, never for a source expression that resolved empty.
    #[serde(default)]
    pub genmodule_only: bool,
    /// The source invokes an ABI-producing full module macro with a readable
    /// declaration-owned config. Runtime-only and simple macros never set it.
    /// Header/FD membership is still decided by that config, not by board ID.
    #[serde(default)]
    pub genmodule_abi: bool,
    /// The legacy `%build_linklib` deliberately invokes the archiver with no
    /// objects for this profile. This is accepted only by a target-specific
    /// audited capability; an unresolved source expression must never set it.
    #[serde(default)]
    pub empty_archive: bool,
    /// C source stems or paths from the macro's `files=` lane.
    pub source_files: Vec<String>,
    /// C++ source stems or paths from `cxxfiles=`. Keeping this lane separate
    /// is required for fetched sources which do not exist when CMake first
    /// configures and therefore cannot be classified by probing extensions.
    #[serde(default)]
    pub cxx_source_files: Vec<String>,
    /// `alwayscxxlink=yes` selects the C++ linker even when every declared
    /// translation unit is C.  Mesa HIDDs use this to retain the C++ runtime
    /// link contract of the legacy module macro.
    #[serde(default)]
    pub always_cxx_link: bool,
    /// `%build_prog(s) usestartup=no` opts out of the default startup.o.
    #[serde(default)]
    pub no_startup: bool,
    /// `%build_prog(s) detach=yes` adds the native detached startup object.
    #[serde(default)]
    pub detach: bool,
    /// Objective-C source stems or paths from `objcfiles=`.
    #[serde(default)]
    pub objc_source_files: Vec<String>,
    /// Assembler source stems or paths from `asmfiles=`.
    #[serde(default)]
    pub asm_source_files: Vec<String>,
    pub use_libs: Vec<String>,
    pub dependencies: Vec<String>,
    pub dir_path: PathBuf,
    /// Explicit module output directory after Make-variable expansion.
    /// Relative values are rooted below SYS by CMake; rendered build-tree
    /// paths such as `${AROS_BUILD_DIR}/Libs` remain absolute overrides.
    pub target_dir: Option<String>,
    /// True when a `%build_linklib` declares the extra 32-bit flavour of a
    /// library, which a 64-bit target has beside its own: compiler/crt/stdc
    /// builds stdc.static twice, the second into $(GENDIR)/lib32 for the
    /// bootstrap. Both carry the same libname, so uselibs cannot tell them
    /// apart without this.
    #[serde(default)]
    pub variant_32bit: bool,
    /// mmake ids of the link libraries this target links against, resolved
    /// from `uselibs` once every mmakefile has been parsed.
    #[serde(default)]
    pub link_libs: Vec<String>,
    /// The original `modtype` when CMake cannot infer it from [`ModuleType`],
    /// notably custom and `%build_module_simple` declarations.
    #[serde(default)]
    pub declared_mod_type: Option<String>,
    /// Effective output suffix override, without the leading dot. Full module
    /// declarations may set `modsuffix=` independently of `modtype`; USB and
    /// Bluetooth classes use the type-default suffix `class`.
    #[serde(default)]
    pub mod_suffix: Option<String>,
    /// Public client-link library name requested with `linklibname=`.
    ///
    /// A full library module always exposes its module name as a client-link
    /// library too. This optional alias is kept separately from `target_name`
    /// so `uselibs` can resolve both spellings to the same generated archive.
    #[serde(default)]
    pub linklib_name: Option<String>,
    /// The genmodule config a declaration names with `conffile=`, as a CMake
    /// path. Absent means `<modname>.conf` beside the declaration, which is what
    /// a declaration without `conffile=` means.
    #[serde(default)]
    pub config_file: Option<String>,
    /// The declaration's `confoverride=` file, applied after `config_file`.
    #[serde(default)]
    pub config_override_file: Option<String>,
    /// Full-module normal/relative client archive composition.
    #[serde(default)]
    pub genmodule_linklibs: Option<GenmoduleLinklibs>,
    /// Consumer-side `rellib` requirements from the effective genmodule config.
    /// These do not imply that this module produces a client archive.
    #[serde(default)]
    pub config_relative_libraries: Vec<String>,
    /// Explicit private archive directory from a proven `%build_linklib`
    /// `libdir=` expression. The parser records this only after resolving the
    /// path below the build tree. A raw `-l<name>` consumer may use this
    /// provider only when its own declaration carries the exact matching
    /// `-L<directory>` option.
    #[serde(default)]
    pub linklib_output_dir: Option<String>,
    /// Whether an ordinary `%build_linklib` is proven to own the canonical
    /// target-SDK archive name. This is intentionally false for host, 32-bit,
    /// custom-libdir and in-tree declarations; the CMake layer may migrate
    /// output naming only when this proof is present.
    #[serde(default)]
    pub canonical_linklib_output: bool,
    /// The declaration uses the default target compiler, SDK libdir and native
    /// word size, so a proven `-l<name>` consumer may safely promote it to the
    /// canonical archive path. This remains separate from the actual decision
    /// to avoid moving unrelated in-tree or host archives.
    #[serde(default)]
    pub canonical_linklib_eligible: bool,
    pub compiler_flags: Vec<String>,
    /// Include directories from the mmakefile's `USER_INCLUDES`, already
    /// rendered as CMake paths.
    pub include_dirs: Vec<String>,
    /// `modname` keys whose `%set_archincludes` declarations this target needs,
    /// requested via `%get_archincludes`.
    pub arch_modules: Vec<String>,
    /// Architecture-conditional include directories, resolved from the tree's
    /// `%set_archincludes` declarations. Each entry is `(arch_tag, path)`.
    pub arch_includes: Vec<(String, String)>,
    /// Preprocessor definitions from `USER_CPPFLAGS` / `USER_CFLAGS`.
    pub defines: Vec<String>,
    /// Names to undefine.
    pub undefines: Vec<String>,
    /// Allowlisted codegen options.
    pub compile_options: Vec<String>,
    /// Direct-linker library options from the declaration-local
    /// `USER_LDFLAGS` snapshot. The dependency graph keeps an option only when
    /// it can bind the library name to a public archive producer; `-lpthread`,
    /// for example, is retained together with its `linklibs-pthread` edge.
    #[serde(default)]
    pub link_options: Vec<String>,
    /// Compiler-spec switches which suppress part of the default link set.
    #[serde(default)]
    pub spec_switches: Vec<String>,
    /// Driver-level link options, for a declaration that links a standalone
    /// executable through the compiler driver rather than as an AROS module.
    #[serde(default)]
    pub driver_link_options: Vec<String>,
    /// `TARGET_ISA_LDFLAGS` as this declaration sets it. The PC bootstrap uses
    /// it to link for a different architecture than the rest of the tree
    /// (`--target=i386-pc-linux-gnu -march=i486`), and it is an assignment to a
    /// global rather than to a `USER_*` variable, so the flag collector cannot
    /// see it.
    #[serde(default)]
    pub isa_link_options: Vec<String>,
    /// Architecture-specific source overrides, as `(arch_tag, dir, files)`.
    /// A file listed here replaces the same-named generic source.
    pub arch_sources: Vec<(String, String, Vec<String>)>,
    /// Preprocessor definitions from an architecture `make.opts`, as
    /// `(arch_tag, define)`.
    pub arch_defines: Vec<(String, String)>,
    /// Codegen options from an architecture `make.opts`, as `(arch_tag, opt)`.
    pub arch_compile_options: Vec<(String, String)>,
    /// Codegen options that belong to one architecture lane's own sources:
    /// `(tag, directory, file, option)`. Kept per file rather than per tag
    /// because a lane's flags are the lane's: arch/i386-all/hidd/gfx compiles
    /// rgbconv_sse.c with -msse2 and rgbconv_avx.c with -mavx2 while the
    /// baseline dispatcher beside them must stay baseline ISA, and after a lane
    /// is attached to another lane (see resolve_arch_lane_attachments) the tag
    /// no longer tells them apart.
    #[serde(default)]
    pub arch_source_options: Vec<(String, String, String, String)>,
}

/// A source wildcard waiting for its fetched owner.
///
/// Unlike a compilation target, this record is only a hint for the
/// cold-source preflight and must never be emitted as a build producer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceInventoryNeed {
    pub pattern: String,
    /// Sanitized MetaMake owner from the declaration that contained the
    /// deferred pattern.
    pub owner_mmake: String,
    /// Source-root-relative recipe path that declared the pattern.
    pub recipe: PathBuf,
    /// One-based source line of the declaring macro invocation.
    pub line: usize,
}

/// Selection metadata for a declaration with a deferred source inventory.
///
/// This deliberately excludes
/// compiler inputs and is stored outside `ParsedMmakefile::targets`, so it can
/// inform cold-root selection without becoming a compile producer.
#[derive(Debug, Clone, Serialize, Deserialize)]
// Independent source declaration facts, not lifecycle or board-selection states.
#[allow(clippy::struct_excessive_bools)]
pub struct InventoryTargetIdentity {
    pub mmake_name: String,
    pub target_name: String,
    pub module_type: ModuleType,
    pub dir_path: PathBuf,
    pub variant_32bit: bool,
    pub declared_mod_type: Option<String>,
    pub mod_suffix: Option<String>,
    pub linklib_output_dir: Option<String>,
    pub genmodule_abi: bool,
    pub genmodule_only: bool,
    pub module_macro: Option<ModuleMacroForm>,
    pub target_dir: Option<String>,
    pub linklib_name: Option<String>,
    pub genmodule_linklibs: Option<GenmoduleLinklibs>,
    pub config_relative_libraries: Vec<String>,
    pub canonical_linklib_output: bool,
    pub canonical_linklib_eligible: bool,
    pub empty_archive: bool,
    pub dependencies: Vec<String>,
    pub link_libs: Vec<String>,
    pub use_libs: Vec<String>,
    pub link_options: Vec<String>,
    pub spec_switches: Vec<String>,
}

impl From<&TargetDefinition> for InventoryTargetIdentity {
    fn from(target: &TargetDefinition) -> Self {
        Self {
            mmake_name: target.mmake_name.clone(),
            target_name: target.target_name.clone(),
            module_type: target.module_type.clone(),
            dir_path: target.dir_path.clone(),
            variant_32bit: target.variant_32bit,
            declared_mod_type: target.declared_mod_type.clone(),
            mod_suffix: target.mod_suffix.clone(),
            linklib_output_dir: target.linklib_output_dir.clone(),
            genmodule_abi: target.genmodule_abi,
            genmodule_only: target.genmodule_only,
            module_macro: target.module_macro,
            target_dir: target.target_dir.clone(),
            linklib_name: target.linklib_name.clone(),
            genmodule_linklibs: target.genmodule_linklibs.clone(),
            config_relative_libraries: target.config_relative_libraries.clone(),
            canonical_linklib_output: target.canonical_linklib_output,
            canonical_linklib_eligible: target.canonical_linklib_eligible,
            empty_archive: target.empty_archive,
            dependencies: target.dependencies.clone(),
            link_libs: target.link_libs.clone(),
            use_libs: target.use_libs.clone(),
            link_options: target.link_options.clone(),
            spec_switches: target.spec_switches.clone(),
        }
    }
}

/// A parsed meta-target rule (#MM or #MM-).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetaTargetRule {
    pub name: String,
    pub dependencies: Vec<String>,
}

/// One safely resolved `%copy_dir_recursive` declaration.
///
/// The historic macro is an output-producing phony target rather than a
/// source declaration.  Keeping its owning `mmake` name lets generated CMake
/// replace the fallback phony with a real copy target, while `dependencies`
/// carries the exact `%fetch` endpoint when the source lives in a port tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyDirectoryDecl {
    /// `mmake=`: the MetaMake target which owns this copy operation.
    pub name: String,
    /// Source directory, rendered as a concrete CMake path.
    pub source: String,
    /// Destination directory, rendered as a concrete CMake path.
    pub destination: String,
    /// Declaring mmakefile, relative to the source root.
    pub file: String,
    /// One-based declaration line in `file`.
    pub line: usize,
    /// Exact `%fetch` endpoints that must complete before the copy runs.
    #[serde(default)]
    pub dependencies: Vec<String>,
}

/// A generated header whose complete contents are proven literal `#define`
/// lines from one declaration-owned local Make fragment.
///
/// This deliberately does not represent arbitrary Make recipes. The local
/// fragment validator accepts only one header rule made from a literal
/// overwrite followed by literal appends, and the parser selects its concrete
/// conditional branches for the active target profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefineHeaderDecl {
    /// Real MetaMake target which owns the generated output.
    pub owner: String,
    /// Declaring fragment, relative to the source root.
    pub file: String,
    /// One-based line of the output rule in `file`.
    pub line: usize,
    /// Concrete CMake build-tree output.
    pub output: String,
    /// Text following `#define `, in exact output order.
    pub definitions: Vec<String>,
    /// Source files which must trigger reconfiguration and regeneration.
    pub dependencies: Vec<String>,
    /// Concrete build target whose source declaration owns the fragment.
    pub provider: String,
    /// Compile targets requiring the output and its parent include directory.
    /// The graph fills this after resolving link-library consumers.
    #[serde(default)]
    pub consumers: Vec<String>,
}

/// One strictly capability-checked third-party CMake build.
///
/// `%build_with_cmake` is intentionally not represented as an open-ended bag
/// of legacy macro arguments. Cross-building and installing an upstream CMake
/// project is safe only after its source provenance, products and public
/// interface are known. Each admitted declaration therefore carries the
/// complete contract consumed by `aros_build_external_cmake`; declarations
/// outside the supported capability profiles remain reported as skipped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalCMakeDecl {
    /// MetaMake workflow identity used by #MM dependencies.
    pub mmake_name: String,
    /// Configured upstream source tree.
    pub source_dir: String,
    /// Private out-of-source build directory.
    pub binary_dir: String,
    /// Prefix passed to the upstream install step.
    pub install_prefix: String,
    /// Proven `%fetch` target which materialises `source_dir`.
    pub fetch_target: String,
    /// Local patches admitted by the capability. The fetch helper tracks the
    /// files directly and invalidates the private unpacked tree when one
    /// changes; their contents are repository state, not hidden transpiler
    /// pins.
    #[serde(default)]
    pub local_patch_files: Vec<String>,
    /// Legacy `uselibs=` spelling published by this build.
    pub provided_library: String,
    /// Linkable interface target created by the external-build helper. This is
    /// deliberately distinct from `mmake_name`, which remains the utility/meta
    /// endpoint used to request the configure/build/install workflow.
    pub provider_target: String,
    /// Installed static/shared library products used to make the build
    /// incremental and to define the imported CMake target.
    pub library_products: Vec<String>,
    /// Installed public headers. Listing them explicitly lets Ninja detect an
    /// incomplete install rather than accepting the library alone as success.
    pub header_products: Vec<String>,
    /// Other deterministic install products, such as package metadata. They
    /// participate in collision, existence and incremental-repair checks but
    /// are not exposed as include roots or link items.
    pub auxiliary_products: Vec<String>,
    /// Installed include roots propagated to consumers.
    pub public_include_dirs: Vec<String>,
    /// Fully selected, allowlisted upstream CMake options.
    pub options: Vec<String>,
    /// Optional closed component build/install rather than upstream's default all.
    #[serde(default)]
    pub build_targets: Vec<String>,
    /// Upstream install components needed by the selected static build.
    #[serde(default)]
    pub install_components: Vec<String>,
    /// Static components form a single linker rescan group, not one archive.
    #[serde(default)]
    pub library_group: bool,
    /// Host executables supplied by the verified cross-toolchain, never target ELF.
    #[serde(default)]
    pub host_tools: Vec<String>,
    /// Validated declaration-specific target compile definitions.
    #[serde(default)]
    pub compile_defines: Vec<String>,
    /// Source-root-relative directory of the declaring mmakefile.
    pub dir_path: PathBuf,
}

/// One strictly capability-checked legacy `%build_with_configure` build.
///
/// The original macro can execute arbitrary configure scripts with an open
/// ended environment.  The standalone build deliberately models only audited
/// local-source projects.  Each declaration pins its complete input manifest,
/// private build root and every installed product; the CMake runner accepts no
/// command text from the mmakefile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigureBuildDecl {
    /// MetaMake workflow identity used by `#MM` dependencies.
    pub mmake_name: String,
    /// Closed runner capability (`adflib-host`, `adflib-target`, or
    /// `wirelessmanager`).
    pub mode: String,
    /// Read-only local source root.
    pub source_dir: String,
    /// Private stage/build root below `${AROS_BUILD_DIR}/gen/configure`.
    pub binary_dir: String,
    /// Build-tree prefix receiving the public products.
    pub install_prefix: String,
    /// Path-only source allowlist. CMake snapshots live content hashes into
    /// the runner contract without fixing them in the repository.
    pub input_manifest: String,
    /// Outputs retained below the private build root.
    pub private_products: Vec<String>,
    /// Complete installed product contract.
    pub install_products: Vec<String>,
    /// Link-library targets whose archives the private build command links.
    /// CMake asks each target where it writes.
    #[serde(default)]
    pub dependency_targets: Vec<String>,
    /// Optional `uselibs=` spelling published by an installed archive.
    #[serde(default)]
    pub provided_library: Option<String>,
    /// Distinct linkable interface target for `provided_library`.
    #[serde(default)]
    pub provider_target: Option<String>,
    /// Source-root-relative directory of the declaring mmakefile.
    pub dir_path: PathBuf,
}

/// One strictly capability-checked, versioned GRUB host-tool lane.
///
/// The legacy `%build_with_configure` declarations are intentionally not
/// generalised: the CMake helper owns the fixed upstream archive, local patch,
/// toolchain and complete product manifest.  The parser only selects one of
/// the three audited lanes and provides its private build/install roots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrubBuildDecl {
    /// MetaMake workflow identity used by `#MM` dependencies.
    pub mmake_name: String,
    /// Exact audited upstream GRUB source version.
    pub version: String,
    /// Closed runner capability (`pc`, `efi64`, or `efi32`).
    pub mode: String,
    /// Private build root below `${AROS_BUILD_DIR}/gen/configure`.
    pub binary_dir: String,
    /// Lane-specific host-tool install root below `${AROS_BUILD_DIR}`.
    pub install_prefix: String,
    /// Source-root-relative directory of the declaring mmakefile.
    pub dir_path: PathBuf,
}

/// One strictly capability-checked AHI subsystem build.
///
/// AHI's legacy `%build_with_configure` invocation carries an open-ended
/// Autoconf environment.  This deliberately admits only the one audited
/// subsystem declaration and forwards no source paths, command text or
/// compiler flags.  The CMake helper owns the complete source/product
/// manifest; these fields only select a current target profile and bind the
/// already-materialised host tools by their explicit paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AhiBuildDecl {
    /// MetaMake workflow identity used by `#MM` dependencies.
    pub mmake_name: String,
    /// Closed runner profile (`x86_64`, `arm`, or `aarch64`).
    pub mode: String,
    /// Private build root below `${AROS_BUILD_DIR}/gen/configure`.
    pub binary_dir: String,
    /// Target system prefix receiving the installed AHI subsystem.
    pub install_prefix: String,
    /// Explicit output of the closed host `sfdc` target.
    pub host_sfdc: String,
    /// Explicit, already-validated absolute Perl interpreter chosen by CMake.
    pub host_perl: String,
    /// Source-root-relative directory of the declaring mmakefile.
    pub dir_path: PathBuf,
}

/// One output-producing invocation inside a strictly admitted Python
/// generator group.
///
/// Fetched scripts are relative to the group's fetched source root. A
/// capability may instead name an exact repository-owned adapter. Outputs
/// remain relative to the private build root, and both source roots are
/// checked before execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PythonGeneratorJob {
    pub script: String,
    /// Admit a repository-owned script only through an audited capability.
    #[serde(default)]
    pub local_script: bool,
    pub output: String,
    pub arguments: Vec<String>,
    /// Earlier outputs of this owner needed before this job can run.
    #[serde(default)]
    pub depends_on_outputs: Vec<String>,
}

/// One fetched pure-Python package made available to a generator group.
///
/// Packages are fetched like any other port, but are never installed into the
/// host interpreter.  Their audited import roots are passed through a private
/// `PYTHONPATH`, keeping generator results independent from whatever happens
/// to be installed globally on the build host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PythonPackageDecl {
    pub fetch_target: String,
    pub source_root: String,
    pub python_path: String,
}

/// A capability-checked group of fetched Python generators.
///
/// This is deliberately not a representation of arbitrary Make recipes.
/// Each instance is constructed by a target-specific parser capability which
/// validates the scripts, arguments, products, fetch owner and local patch. The
/// generated CMake then gives all products one real MetaMake owner target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PythonOutputsDecl {
    /// MetaMake target which owns every generated output.
    pub owner: String,
    /// Fetched source root containing scripts and read-only inputs.
    pub source_root: String,
    /// Private build root below which every output must live.
    pub build_root: String,
    /// Fetch target whose completion stamp orders and invalidates the jobs.
    pub fetch_target: String,
    /// Fetched, source-root-relative inputs shared by the jobs.
    pub source_inputs: Vec<String>,
    /// Repository-owned regular files read by the jobs, tracked as direct
    /// build dependencies in addition to the fetched source stamp.
    #[serde(default)]
    pub local_inputs: Vec<String>,
    pub jobs: Vec<PythonGeneratorJob>,
    /// Optional repository-owned adapter for generators which
    /// write named files or need host Flex/Bison rather than stdout-only
    /// Python.  Absence retains the original direct-Python contract.
    #[serde(default)]
    pub driver_script: Option<String>,
    /// Host parser tools required by selected repository-owned adapters.
    #[serde(default)]
    pub requires_flex_bison: bool,
    /// Pure-Python packages exposed only to this owner.
    #[serde(default)]
    pub python_packages: Vec<PythonPackageDecl>,
    /// Exact unpacked source directory refreshed when the local patch changes.
    pub audited_source_dir: String,
    /// Source-tree patches tracked directly as build inputs.
    pub local_patch_files: Vec<String>,
    /// Concrete compile targets which consume the generated products.
    pub consumers: Vec<String>,
    /// Source-root-relative directory of the declaring mmakefile.
    pub dir_path: PathBuf,
}

/// Result of parsing an mmakefile.src.
#[derive(Debug, Clone, Default)]
pub struct ParsedMmakefile {
    /// Digest of the original byte snapshot used to parse this recipe. It is
    /// not a later filesystem measurement or the re-encoded legacy text.
    pub source_sha256: Option<String>,
    /// Literal disabled `##MM` owners from the same original byte snapshot.
    /// These are evidence, not graph providers or implicit optional edges.
    pub disabled_meta_owners: Vec<String>,
    /// Closed ordinary Make aggregates with all host-header leaves represented.
    pub host_header_aggregates: Vec<crate::host_header_aggregates::HostHeaderAggregateDecl>,
    /// Closed directory-only setup recipes with real build endpoints.
    pub directory_setups: Vec<crate::directory_setup::DirectorySetupDecl>,
    /// Named handwritten genmodule header-stamp producers.
    pub genmodule_header_rules: Vec<crate::genmodule_header_rules::GenmoduleHeaderRuleDecl>,
    /// Source-owned handwritten client-source generators, distinct from headers.
    pub genmodule_writefiles_rules:
        Vec<crate::genmodule_writefiles_rules::GenmoduleWritefilesRuleDecl>,
    pub host_file_generators: Vec<aros_common::native_host_generator::NativeHostFileGenerator>,
    /// Closed source-owned host-C header producers with exact prerequisites.
    pub host_header_rules: Vec<crate::host_header_rules::HostHeaderRuleDecl>,
    /// Source-derived SDK text products with ordered literal operations.
    pub sdk_text_rules: Vec<crate::sdk_text_rules::SdkTextRuleDecl>,
    /// Source-owned SDK headers generated from local SFD descriptions.
    pub sfd_header_rules: Vec<crate::sfd_header_rules::SfdHeaderRuleDecl>,
    /// Complete multi-output source text producers, including SDK/host tools.
    pub source_text_rules: Vec<crate::source_text_rules::SourceTextRuleDecl>,
    /// Local source values extracted by closed literal recipe semantics.
    pub source_value_rules: Vec<crate::source_value_rules::SourceValueRuleDecl>,
    pub sdk_file_copies: Vec<crate::sdk_file_copies::SdkFileCopyDecl>,
    pub sdk_asset_rules: Vec<crate::sdk_asset_rules::SdkAssetRuleDecl>,
    pub sdk_program_outputs: Vec<crate::graph::SdkProgramOutput>,
    /// Finite source-owned compilation and Developer-library object staging.
    pub sdk_object_groups: Vec<crate::sdk_objects::SdkObjectGroupDecl>,
    /// Literal compile-only objects, without Developer-library staging.
    pub literal_object_groups: Vec<crate::literal_objects::LiteralObjectGroupDecl>,
    /// Partial source projections, not executable providers. Archive members
    /// still need exact compiler ownership; layered headers still need their
    /// setup and generated-header prerequisites to be closed.
    pub source_archive_projections: Vec<crate::source_archive_rules::SourceArchiveDecl>,
    /// Command proofs are distinct from complete member ownership. A proven
    /// archiver alone does not make a partial archive a native provider.
    pub source_archive_commands: std::collections::BTreeMap<
        (String, String),
        crate::source_archive_command::SourceArchiveCommand,
    >,
    pub source_compile_projections: Vec<crate::source_compile_rules::SourceCompileGroupDecl>,
    pub layered_header_projections: Vec<crate::layered_header_copies::LayeredHeaderCopyDecl>,
    pub source_header_pipelines: Vec<crate::source_header_pipeline::SourceHeaderPipelineDecl>,
    pub source_directory_groups: std::collections::BTreeMap<
        (String, String),
        crate::source_directory_rules::SourceDirectoryGroupDecl,
    >,
    /// Drift in a recognised closed capability. Unlike general coverage gaps,
    /// these are fatal: continuing would execute stale target-specific
    /// assumptions.
    pub capability_errors: Vec<Diagnostic>,
    /// Handwritten recipes outside the new native graph's closed models.
    /// These become fatal when selected by an explicit source-bound native
    /// contract. Full-tree translation retains its existing coverage reports
    /// and SDK bootstrap path rather than treating every older recipe as drift.
    pub native_graph_errors: Vec<Diagnostic>,
    pub targets: Vec<TargetDefinition>,
    /// Strictly modelled `%build_with_cmake` declarations.
    pub external_cmake: Vec<ExternalCMakeDecl>,
    /// Strictly modelled local `%build_with_configure` declarations.
    pub configure_builds: Vec<ConfigureBuildDecl>,
    /// Strictly modelled GRUB host-tool lanes for explicitly audited versions.
    pub grub_builds: Vec<GrubBuildDecl>,
    /// Strictly modelled AHI subsystem configure-style build.
    pub ahi_builds: Vec<AhiBuildDecl>,
    /// Strictly modelled fetched Python output groups.
    pub python_outputs: Vec<PythonOutputsDecl>,
    /// Paired hand-written FlexCat source/header/catalog rules.
    pub flexcat_sources: Vec<FlexCatSourceDecl>,
    /// Hand-written FlexCat rules which generate only a compile-time header.
    pub flexcat_headers: Vec<FlexCatHeaderDecl>,
    /// Exact in-tree ILBM-to-C include generators.
    pub ilbm_sources: Vec<crate::ilbm::IlbmSourceDecl>,
    /// ILBM-to-C recipes that no longer satisfy the safe closed contract.
    pub skipped_ilbm_sources: Vec<String>,
    /// Hand-written FlexCat source rules that did not meet the narrow safe
    /// contract and therefore remain deliberately unmodelled.
    pub skipped_flexcat_sources: Vec<String>,
    pub meta_rules: Vec<MetaTargetRule>,
    /// Handwritten source rules retained separately from macro-generated
    /// aliases. Their prerequisites must not be rebound as implicit spellings.
    pub explicit_meta_rules: Vec<MetaTargetRule>,
    /// Explicit nonvirtual #MM providers. An empty virtual declaration must
    /// not hide a same-named provider whose Make recipe remains unmodelled.
    pub make_meta_providers: Vec<String>,
    /// `%build_icons` target identities, including declarations whose inputs
    /// could not be resolved. Keeping the identity makes the gap visible and
    /// preserves meta-target edges even when no command can be emitted.
    pub icon_targets: Vec<crate::icons::IconTarget>,
    /// Resolved `%build_icons` declarations. Repeated mmake ids deliberately
    /// remain separate: Make merges their prerequisites.
    pub icons: Vec<crate::icons::IconSet>,
    /// `%build_icons` declarations or variants that could not be resolved.
    pub skipped_icons: Vec<String>,
    /// Fully resolved `%build_catalogs` declarations. Unlike compiled modules,
    /// these produce installed locale resources and an optional generated
    /// source/header.
    pub catalogs: Vec<crate::catalogs::CatalogDecl>,
    /// Catalog declarations omitted because an input/default was unresolved.
    pub skipped_catalogs: Vec<String>,
    /// Dynamic #MM names/dependencies that reference Make variables for which
    /// this CMake build has no counterpart.
    pub skipped_meta_rules: Vec<String>,
    /// `%set_archincludes` declarations contributed by this file.
    pub arch_decls: Vec<ArchIncludeDecl>,
    /// Include tokens whose Make variables were not resolved, for reporting.
    pub unresolved_includes: Vec<String>,
    /// `%copy_includes` declarations that stage public headers into the SDK.
    pub copy_includes: Vec<CopyIncludesDecl>,
    /// `%copy_includes` declarations that could not be resolved, for reporting.
    pub skipped_copy_includes: Vec<String>,
    /// Safely resolved `%copy_dir_recursive` declarations.
    pub copy_directories: Vec<CopyDirectoryDecl>,
    /// `%copy_dir_recursive` declarations that were not safe to model.
    pub skipped_copy_directories: Vec<String>,
    /// Hand-written Make rules that stage headers; these need a static CMake
    /// counterpart and are reported so new ones do not go unnoticed.
    pub adhoc_header_rules: Vec<AdhocHeaderRule>,
    /// Safe, literal hand-written recipes promoted to real build outputs.
    pub header_transforms: Vec<HeaderTransformDecl>,
    /// Exact host-Bison generated C outputs.
    pub bison_outputs: Vec<crate::copy_includes::BisonOutputDecl>,
    /// Safe declaration-owned literal `#define` headers.
    pub define_headers: Vec<DefineHeaderDecl>,
    /// Hand-written `$(GENDIR)` rules producing something other than a header,
    /// for reporting.
    pub generated_file_rules: Vec<String>,
    pub script_outputs: Vec<crate::copy_includes::ScriptOutputDecl>,
    pub skipped_script_outputs: Vec<String>,
    /// Build declarations whose kind the target model does not express yet.
    pub skipped_programs: Vec<String>,
    /// Source lanes omitted from an otherwise retained legacy target because
    /// their Make expression cannot yet be evaluated faithfully.
    pub partial_source_lists: Vec<String>,
    /// Fetched-tree wildcard patterns that need their owning fetch to finish
    /// before a second configure can obtain the complete source inventory.
    pub source_inventory_patterns: Vec<String>,
    /// Deferred source wildcards with their declaration owner and provenance.
    pub source_inventory_needs: Vec<SourceInventoryNeed>,
    /// Target identities for deferred declarations, separate from buildable
    /// compilation targets.
    pub source_inventory_targets: Vec<InventoryTargetIdentity>,
    /// Modules whose genmodule config demands a client archive that the target
    /// model does not build yet, because the archive's generated sources are
    /// only derived for `modtype=library`.
    pub skipped_client_archives: Vec<String>,
    /// Explicit program output directories which could not be resolved.
    pub unresolved_output_paths: Vec<String>,
    /// `%make_package` and `%link_kickstart` declarations.
    pub packages: Vec<crate::packages::PackageDecl>,
    /// Package declarations that could not be resolved, for reporting.
    pub skipped_packages: Vec<String>,
    /// Flags collected from this file, including what had to be skipped.
    pub flags: FlagSet,
    /// `%build_archspecific` declarations contributed by this file.
    pub arch_sources: Vec<ArchSourceDecl>,
    /// `%rule_link_binary`: flat binaries wrapped as relocatable objects.
    pub binary_objects: Vec<crate::binary_objects::BinaryObjectDecl>,
    /// Public headers a host tool writes.
    pub hidd_stubs: Vec<crate::hidd_stubs::HiddStubsDecl>,
    pub skipped_hidd_stubs: Vec<String>,
    pub host_generated_headers: Vec<crate::host_generated_headers::HostGeneratedHeader>,
    /// Rules of that shape this could not represent, for reporting.
    pub skipped_host_generated_headers: Vec<String>,
    /// The ones that could not be resolved, for reporting.
    pub skipped_binary_objects: Vec<String>,
    /// Declarations whose file list could not be resolved, for reporting.
    pub skipped_arch_sources: Vec<String>,
    /// `%fetch` declarations for third-party sources.
    pub fetches: Vec<FetchDecl>,
    /// `%fetch` declarations that could not be resolved, for reporting.
    pub skipped_fetches: Vec<String>,
    /// `-include .../make.opts` files that could not be used, for reporting.
    pub skipped_make_opts: Vec<String>,
    /// Local source-tree Make fragments which were unresolved, unsafe, or
    /// broader than the declaration-aware source-list subset.
    pub skipped_local_make_includes: Vec<String>,
    /// Make conditionals whose flags were dropped, for reporting.
    pub skipped_conditions: Vec<String>,
}
