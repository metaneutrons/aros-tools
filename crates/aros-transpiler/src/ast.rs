use crate::arch_sources::ArchSourceDecl;
use crate::copy_includes::{AdhocHeaderRule, CopyIncludesDecl, HeaderTransformDecl};
use crate::fetch::FetchDecl;
use crate::flags::FlagSet;
use crate::flexcat::FlexCatSourceDecl;
use crate::includes::ArchIncludeDecl;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Types of buildable units in AROS mmakefiles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModuleType {
    Library,
    /// `%build_module_abi`: generated headers and a link stub library, but no
    /// runtime module. Keeping this distinct from [`Library`](Self::Library)
    /// prevents package resolution from treating an ABI skeleton as a file.
    Abi,
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
    /// The module has no hand-written sources because genmodule supplies its
    /// complete runtime implementation. This is deliberately set only for an
    /// explicit `files=""`, never for a source expression that resolved empty.
    #[serde(default)]
    pub genmodule_only: bool,
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
    /// Full-module normal/relative client archive composition.
    #[serde(default)]
    pub genmodule_linklibs: Option<GenmoduleLinklibs>,
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
    /// Exact archive whose digest is checked before configuration.
    pub source_archive: String,
    pub source_sha256: String,
    /// Local patches admitted by the capability, paired positionally with
    /// their exact digests. The fetch helper uses both lists to invalidate a
    /// previously materialised source tree when a patch input changes.
    #[serde(default)]
    pub local_patch_files: Vec<String>,
    #[serde(default)]
    pub local_patch_sha256: Vec<String>,
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
    /// SHA-256/path manifest which is both the source allowlist and the exact
    /// content fingerprint used by the runner.
    pub input_manifest: String,
    pub input_manifest_sha256: String,
    /// Outputs retained below the private build root.
    pub private_products: Vec<String>,
    /// Complete installed product contract.
    pub install_products: Vec<String>,
    /// Existing build products needed by the private build command.
    #[serde(default)]
    pub dependency_products: Vec<String>,
    /// Optional `uselibs=` spelling published by an installed archive.
    #[serde(default)]
    pub provided_library: Option<String>,
    /// Distinct linkable interface target for `provided_library`.
    #[serde(default)]
    pub provider_target: Option<String>,
    /// Source-root-relative directory of the declaring mmakefile.
    pub dir_path: PathBuf,
}

/// One strictly capability-checked GRUB 2.12 host-tool lane.
///
/// The legacy `%build_with_configure` declarations are intentionally not
/// generalised: the CMake helper owns the fixed upstream archive, local patch,
/// toolchain and complete product manifest.  The parser only selects one of
/// the three audited lanes and provides its private build/install roots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrubBuildDecl {
    /// MetaMake workflow identity used by `#MM` dependencies.
    pub mmake_name: String,
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
/// The script and source inputs are relative to the group's fetched source
/// root, while the output is relative to its private build root.  Keeping
/// these roots separate lets the CMake helper reject source-tree writes and
/// build-tree escapes before it ever starts the interpreter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PythonGeneratorJob {
    pub script: String,
    pub output: String,
    pub arguments: Vec<String>,
}

/// One pinned pure-Python package made available to a generator group.
///
/// Packages are fetched like any other port, but are never installed into the
/// host interpreter.  Their audited import roots are passed through a private
/// `PYTHONPATH`, keeping generator results independent from whatever happens
/// to be installed globally on the build host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PythonPackageDecl {
    pub fetch_target: String,
    pub source_root: String,
    pub source_archive: String,
    pub source_sha256: String,
    pub python_path: String,
}

/// A capability-checked group of fetched Python generators.
///
/// This is deliberately not a representation of arbitrary Make recipes.
/// Each instance is constructed by a target-specific parser capability which
/// pins the scripts, arguments, products, fetch owner and local patch.  The
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
    /// Exact downloaded archive whose digest is verified before Python runs.
    pub source_archive: String,
    pub source_sha256: String,
    /// Fetched, source-root-relative inputs shared by the jobs.
    pub source_inputs: Vec<String>,
    pub jobs: Vec<PythonGeneratorJob>,
    /// Optional repository-owned, content-pinned adapter for generators which
    /// write named files or need host Flex/Bison rather than stdout-only
    /// Python.  Absence retains the original direct-Python contract.
    #[serde(default)]
    pub driver_script: Option<String>,
    #[serde(default)]
    pub driver_sha256: Option<String>,
    /// Pure-Python packages exposed only to this owner.
    #[serde(default)]
    pub python_packages: Vec<PythonPackageDecl>,
    /// Exact unpacked source directory refreshed when the local patch changes.
    pub audited_source_dir: String,
    /// Source-tree patches paired positionally with their pinned SHA-256.
    pub local_patch_files: Vec<String>,
    pub local_patch_sha256: Vec<String>,
    /// Concrete compile targets which consume the generated products.
    pub consumers: Vec<String>,
    /// Source-root-relative directory of the declaring mmakefile.
    pub dir_path: PathBuf,
}

/// Result of parsing an mmakefile.src.
#[derive(Debug, Clone, Default)]
pub struct ParsedMmakefile {
    pub targets: Vec<TargetDefinition>,
    /// Strictly modelled `%build_with_cmake` declarations.
    pub external_cmake: Vec<ExternalCMakeDecl>,
    /// Strictly modelled local `%build_with_configure` declarations.
    pub configure_builds: Vec<ConfigureBuildDecl>,
    /// Strictly modelled GRUB 2.12 host-tool lanes.
    pub grub_builds: Vec<GrubBuildDecl>,
    /// Strictly modelled AHI subsystem configure-style build.
    pub ahi_builds: Vec<AhiBuildDecl>,
    /// Strictly modelled fetched Python output groups.
    pub python_outputs: Vec<PythonOutputsDecl>,
    /// Paired hand-written FlexCat source/header/catalog rules.
    pub flexcat_sources: Vec<FlexCatSourceDecl>,
    /// Hand-written FlexCat source rules that did not meet the narrow safe
    /// contract and therefore remain deliberately unmodelled.
    pub skipped_flexcat_sources: Vec<String>,
    pub meta_rules: Vec<MetaTargetRule>,
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
    /// Safe declaration-owned literal `#define` headers.
    pub define_headers: Vec<DefineHeaderDecl>,
    /// Hand-written `$(GENDIR)` rules producing something other than a header,
    /// for reporting.
    pub generated_file_rules: Vec<String>,
    /// Build declarations whose kind the target model does not express yet.
    pub skipped_programs: Vec<String>,
    /// Source lanes omitted from an otherwise retained legacy target because
    /// their Make expression cannot yet be evaluated faithfully.
    pub partial_source_lists: Vec<String>,
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
