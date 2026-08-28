use crate::arch_sources::ArchSourceDecl;
use crate::ast::{
    AhiBuildDecl, ConfigureBuildDecl, CopyDirectoryDecl, DefineHeaderDecl, ExternalCMakeDecl,
    GrubBuildDecl, MetaTargetRule, ModuleType, PythonOutputsDecl, TargetDefinition,
};
use crate::catalogs::CatalogDecl;
use crate::copy_includes::{AdhocHeaderRule, CopyIncludesDecl, HeaderTransformDecl};
use crate::default_link_set::DefaultLinkSet;
use crate::fetch::FetchDecl;
use crate::flexcat::{FlexCatHeaderDecl, FlexCatSourceDecl};
use crate::icons::{IconSet, IconTarget};
use crate::ilbm::IlbmSourceDecl;
use crate::includes::ArchIncludeDecl;
use crate::packages::{runtime_name, ResolvedPackageMember};
use aros_common::{ArosError, Result};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

/// A script-generated file bound to the targets that consume it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedScriptOutput {
    /// Name of the generator target.
    pub owner: String,
    pub script: String,
    pub arguments: Vec<String>,
    pub outputs: Vec<String>,
    pub depends: Vec<String>,
    /// Capture the script's standard output into the sole declared output.
    pub stdout: bool,
    /// Directory selected by an exact `cd <dir> && $(PYTHON) ...` recipe.
    pub working_directory: Option<String>,
    /// Fetch targets which materialise the script, its inputs, or its working
    /// directory. These must be custom-command dependencies, not sibling edges.
    pub dependency_targets: Vec<String>,
    /// Targets that name one of the outputs as a source.
    pub consumers: Vec<String>,
}

/// Dependency Graph for parallel target building and cycle detection.
#[derive(Debug, Default)]
pub struct DependencyGraph {
    pub targets: HashMap<String, TargetDefinition>,
    /// The compiler spec's default link set, resolved to concrete archive
    /// targets in spec order. Empty until `resolve_default_link_set` runs.
    pub default_link_set: Vec<ResolvedDefaultLinkItem>,
    /// `%rule_link_binary` declarations, in declaration order.
    pub binary_objects: Vec<crate::binary_objects::BinaryObjectDecl>,
    /// Reachable exact Python recipes, each resolved to the target that
    /// consumes its output. The script may be in-tree or materialised by a
    /// `%fetch`. Empty until `resolve_script_outputs` runs.
    pub script_outputs: Vec<ResolvedScriptOutput>,
    /// The same declarations before they are bound to a consumer.
    pending_script_outputs: Vec<crate::copy_includes::ScriptOutputDecl>,
    /// `%make_hidd_stubs` declarations, in declaration order. They are the
    /// inputs of one archive, so they are resolved together rather than one at
    /// a time.
    pub hidd_stubs: Vec<crate::hidd_stubs::HiddStubsDecl>,
    /// Public headers a host tool writes.
    pub host_generated_headers: Vec<crate::host_generated_headers::HostGeneratedHeader>,
    /// The section-ordering script a kickstart member's partial link needs,
    /// already split into the tokens the link takes (`-T <path>`), from
    /// KERNEL_KOBJ_LDSCRIPT in config/make.cfg.in. Empty when the target names
    /// none.
    pub kickstart_kobj_ldscript: Vec<String>,
    /// Strictly capability-checked third-party CMake builds. Each contributes
    /// both a real mmake workflow endpoint and a distinct link interface.
    pub external_cmake: Vec<ExternalCMakeDecl>,
    /// Strictly capability-checked local configure-style builds.
    pub configure_builds: Vec<ConfigureBuildDecl>,
    /// Strictly capability-checked GRUB 2.12 host-tool lanes.
    pub grub_builds: Vec<GrubBuildDecl>,
    /// Strictly capability-checked AHI subsystem build.
    pub ahi_builds: Vec<AhiBuildDecl>,
    /// Strictly capability-checked fetched Python output groups.
    pub python_outputs: Vec<PythonOutputsDecl>,
    pub meta_targets: HashMap<String, HashSet<String>>,
    /// Every unique `%build_icons` mmake id. This is separate from `targets`:
    /// icons are generated runtime resources, not compiled modules.
    pub icon_targets: HashMap<String, IconTarget>,
    /// Resolved icon declarations in source order. Duplicate mmake ids are
    /// intentional and their output rules must be aggregated by CMake.
    pub icons: Vec<IconSet>,
    /// Fully resolved `%build_catalogs` declarations. These are generated
    /// runtime resources rather than compiled module targets.
    pub catalogs: Vec<CatalogDecl>,
    /// Safe paired hand-written FlexCat source/header/catalog rules.  Their
    /// source product replaces a nominal source-tree `locale.c` with a
    /// build-tree output before concrete targets are created.
    pub flexcat_sources: Vec<FlexCatSourceDecl>,
    /// Safe header-only FlexCat rules. Their #MM owner edge supplies ordering
    /// and propagates the generated include directory to concrete consumers.
    pub flexcat_headers: Vec<FlexCatHeaderDecl>,
    /// Exact ILBM-to-C include generators. Their owner edge publishes the
    /// private generated include directory to the concrete compile target.
    pub ilbm_sources: Vec<IlbmSourceDecl>,
    /// Every `%set_archincludes` declaration in the tree, keyed by `modname`.
    pub arch_decls: HashMap<String, Vec<ArchIncludeDecl>>,
    /// Every resolved `%copy_includes` declaration, deduplicated.
    pub copy_includes: Vec<CopyIncludesDecl>,
    /// Every safe `%copy_dir_recursive` output declaration, deduplicated by
    /// concrete MetaMake owner.
    pub copy_directories: Vec<CopyDirectoryDecl>,
    /// Hand-written header staging rules found anywhere in the tree.
    pub adhoc_header_rules: Vec<AdhocHeaderRule>,
    /// Safe hand-written header transforms, with graph-resolved ordering.
    pub header_transforms: Vec<HeaderTransformDecl>,
    /// Exact host-Bison generated C outputs.
    pub bison_outputs: Vec<crate::copy_includes::BisonOutputDecl>,
    /// Declaration-owned literal define headers, with their compile consumers.
    pub define_headers: Vec<DefineHeaderDecl>,
    /// `%build_archspecific` declarations, keyed by the target they extend.
    pub arch_sources: HashMap<String, Vec<ArchSourceDecl>>,
    /// `%fetch` declarations for third-party sources.
    pub fetches: Vec<FetchDecl>,
    /// Fetch targets whose unpacked trees are required to determine source
    /// lists. They are emitted as configure dependencies only while the
    /// corresponding wildcard inventory is absent.
    pub source_inventory_fetches: Vec<String>,
    /// `%make_package` and `%link_kickstart` declarations.
    pub packages: Vec<crate::packages::PackageDecl>,
}

/// Splits an `arch/<cpu>-<platform>/...` path into its two components.
fn arch_of(dir: &std::path::Path) -> Option<(String, String)> {
    let s = dir.to_string_lossy().replace('\\', "/");
    let rest = s.strip_prefix("arch/")?;
    let first = rest.split('/').next()?;
    let (cpu, platform) = first.split_once('-')?;
    Some((cpu.to_owned(), platform.to_owned()))
}

fn define_header_compile_targets(mmake: &str, target: &TargetDefinition) -> Vec<String> {
    match target.module_type {
        ModuleType::ProgramGroup => {
            let mut members = target
                .source_files
                .iter()
                .chain(&target.cxx_source_files)
                .chain(&target.objc_source_files)
                .chain(&target.asm_source_files)
                .filter_map(|source| std::path::Path::new(source).file_stem())
                .map(|stem| format!("{mmake}-{}", stem.to_string_lossy()))
                .collect::<Vec<_>>();
            members.sort();
            members.dedup();
            members
        }
        // Despite having no declaration sources, this is a real compiler
        // target: aros_add_library(GENMODULE_ONLY) creates the runtime with
        // libentry plus the generated start/end sources under the mmake id.
        ModuleType::Library if target.genmodule_only => vec![mmake.to_owned()],
        // These declarations materialise only utility/package orchestration
        // under their mmake id. An ABI's compiling target is its generated
        // client archive and does not consume the declaration's `uselibs`.
        ModuleType::Abi | ModuleType::Package | ModuleType::Custom => Vec::new(),
        _ => vec![mmake.to_owned()],
    }
}

/// Lexically resolves a source-root-relative path without consulting the host
/// filesystem. Catalog headers are generated into the build tree, so
/// canonicalization would both require an output that does not exist yet and
/// make the graph depend on the host platform's path semantics.
fn normalize_root_relative_path(path: &Path) -> Option<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(value) => normalized.push(value),
            Component::ParentDir => {
                if !normalized.pop() {
                    return None;
                }
            }
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(normalized)
}

/// Resolves the parent directory of a relative `%build_catalogs source=`
/// output. Absolute and deferred outputs deliberately remain outside this
/// structural inference: their generator location is owned by the CMake
/// helper, not by a source-tree directory.
fn relative_catalog_source_parent(catalog: &CatalogDecl) -> Option<PathBuf> {
    let source = catalog.source.as_deref()?;
    if source.is_empty() || source.contains(['$', ';', '\\']) || Path::new(source).is_absolute() {
        return None;
    }
    let output = normalize_root_relative_path(&Path::new(&catalog.declaring_dir).join(source))?;
    output.parent().map(Path::to_path_buf)
}

/// Resolves one declared compilation source into its logical source-root
/// location. This mirrors the in-tree path cases accepted by CMake's source
/// resolver, while intentionally excluding deferred/absolute port inputs.
fn resolve_logical_target_source(directory: &Path, source: &str) -> Option<PathBuf> {
    if source.is_empty() || source.contains(['$', ';', '\\']) {
        return None;
    }
    let source = if let Some(relative) = source.strip_prefix("${CMAKE_SOURCE_DIR}/") {
        PathBuf::from(relative)
    } else {
        let source = Path::new(source);
        if source.is_absolute() {
            return None;
        }
        directory.join(source)
    };
    normalize_root_relative_path(&source)
}

fn source_shares_logical_directory(directory: &Path, source: &str, expected: &Path) -> bool {
    resolve_logical_target_source(directory, source)
        .and_then(|resolved| resolved.parent().map(Path::to_path_buf))
        .is_some_and(|parent| parent == expected)
}

/// Whether a declared source points at the exact logical C product of a
/// paired hand-written FlexCat recipe. MetaMake commonly lists `locale`
/// without its extension, while the rule itself writes `locale.c`.
fn source_is_flexcat_product(directory: &Path, source: &str, expected: &Path) -> bool {
    let Some(resolved) = resolve_logical_target_source(directory, source) else {
        return false;
    };
    let resolved = if resolved.extension().is_none() {
        resolved.with_extension("c")
    } else {
        resolved
    };
    resolved == expected
}

/// Returns the real CMake compile target for one matching source. A program
/// group expands each source into a separate executable, so its aggregate
/// mmake id is not a concrete compilation target and must not be used here.
fn catalog_compile_target_for_source(target: &TargetDefinition, source: &str) -> Option<String> {
    match target.module_type {
        ModuleType::ProgramGroup => Path::new(source)
            .file_stem()
            .map(|stem| format!("{}-{}", target.mmake_name, stem.to_string_lossy())),
        ModuleType::Abi | ModuleType::Package => None,
        _ => Some(target.mmake_name.clone()),
    }
}

/// Whether a candidate's directory can serve a declaration made under `ctx`.
///
/// The rule matches AROS_ARCH_SOURCE_DIRS in cmake/AROS.cmake: "all" is a
/// wildcard in either position and "native" is a platform shared by every
/// non-hosted target. A candidate outside arch/ is architecture-neutral and
/// always eligible.
///
/// This is what separates the six targets named `serial` across four
/// architectures, so a package declared in arch/i386-pc/boot picks
/// kernel-pc-i386-serial and not the Amiga or Unix one.
fn arch_compatible(candidate: Option<&(String, String)>, ctx: Option<&(String, String)>) -> bool {
    let Some((cand_cpu, cand_plat)) = candidate else {
        return true;
    };
    let Some((ctx_cpu, ctx_plat)) = ctx else {
        // A declaration outside arch/ describes the portable base, so an
        // architecture-specific candidate cannot be meant.
        return false;
    };
    let cpu_compatible = cand_cpu == "all"
        || cand_cpu == ctx_cpu
        || matches!(
            (ctx_cpu.as_str(), cand_cpu.as_str()),
            ("x86_64", "i386") | ("aarch64", "arm") | ("riscv64", "riscv")
        );
    cpu_compatible && (cand_plat == "all" || cand_plat == "native" || cand_plat == ctx_plat)
}

/// The runtime basename a target definition produces.
///
/// An explicit/effective `modsuffix` replaces the module type. Otherwise a
/// custom or simple module's declared type wins over the coarse AST kind.
/// This mirrors the output naming used by the CMake builders and lets package
/// resolution match an authoritative filename rather than a potentially
/// same-named module of another kind.
fn target_runtime_name(target: &TargetDefinition) -> Option<String> {
    // An ABI skeleton publishes headers and a static link stub, not a runtime
    // module. Its declared modtype must therefore never make it eligible for a
    // package member such as `library=foo`.
    if matches!(target.module_type, ModuleType::Abi) {
        return None;
    }
    if let Some(suffix) = target.mod_suffix.as_deref() {
        return Some(runtime_name(suffix, &target.target_name));
    }
    if let Some(declared) = target.declared_mod_type.as_deref() {
        return match declared {
            "printer" => Some(target.target_name.clone()),
            "usbclass" | "btclass" => Some(runtime_name("class", &target.target_name)),
            kind => Some(runtime_name(kind, &target.target_name)),
        };
    }
    let kind = match target.module_type {
        ModuleType::Library => "library",
        ModuleType::Device => "device",
        ModuleType::Resource => "resource",
        ModuleType::Hidd => "hidd",
        ModuleType::Datatype => "datatype",
        ModuleType::Gadget => "gadget",
        ModuleType::Mcc => "mcc",
        _ => return None,
    };
    Some(runtime_name(kind, &target.target_name))
}

/// Whether a raw `-l<name>` consumer can find this declaration's archive in
/// the target SDK library directory.
///
/// Full genmodule/ABI client archives are public by construction. An ordinary
/// link library is public only when it already owns, or may safely be promoted
/// to, its canonical SDK archive name. In-tree/private `libdir=` outputs are
/// deliberately excluded: the direct AROS linker rule has no proven search
/// path for them.
fn has_public_link_archive(target: &TargetDefinition) -> bool {
    match target.module_type {
        ModuleType::Abi => true,
        ModuleType::Library => {
            target.genmodule_only
                || target
                    .genmodule_linklibs
                    .as_ref()
                    .is_some_and(|metadata| metadata.enabled)
        }
        ModuleType::LinkLib => target.canonical_linklib_output || target.canonical_linklib_eligible,
        _ => false,
    }
}

/// Whether a raw linker spelling can reach this declaration's private archive.
///
/// `-L` is intentionally compared for exact equality with the parser-proven
/// build-tree output directory. A parent directory, a child directory or a
/// similarly named path does not establish archive visibility.
fn has_matching_private_link_archive(
    provider: &TargetDefinition,
    consumer: &TargetDefinition,
) -> bool {
    provider.linklib_output_dir.as_ref().is_some_and(|output| {
        consumer.link_options.iter().any(|option| {
            option
                .strip_prefix("-L")
                .is_some_and(|directory| directory == output)
        })
    })
}

fn raw_link_archive_visible(provider: &TargetDefinition, consumer: &TargetDefinition) -> bool {
    has_public_link_archive(provider) || has_matching_private_link_archive(provider, consumer)
}

/// A private provider must retain its declared output location even when a raw
/// `-l` consumer binds to it. Only a public-eligible ordinary linklib may be
/// promoted into the canonical SDK archive directory.
fn needs_canonical_link_archive(provider: &TargetDefinition) -> bool {
    provider.module_type == ModuleType::LinkLib
        && provider.linklib_output_dir.is_none()
        && provider.canonical_linklib_eligible
}

/// One default-link-set item bound to the target that builds its archive.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ResolvedDefaultLinkItem {
    /// Archive base name from the spec, without `-l`.
    pub name: String,
    /// CMake target producing `lib<name>.a`.
    pub archive: String,
    /// Driver switches that must be absent for this item to apply.
    pub require_absent: Vec<String>,
    /// Driver switches that must be present for this item to apply.
    pub require_present: Vec<String>,
}

#[path = "graph_generated.rs"]
mod generated;
#[path = "graph_inventory.rs"]
mod inventory;
#[path = "graph_linking.rs"]
mod linking;
#[path = "graph_meta.rs"]
mod meta;

#[cfg(test)]
#[path = "graph_tests.rs"]
mod tests;
