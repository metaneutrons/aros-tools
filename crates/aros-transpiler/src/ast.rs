use crate::arch_sources::ArchSourceDecl;
use crate::copy_includes::{AdhocHeaderRule, CopyIncludesDecl};
use crate::fetch::FetchDecl;
use crate::flags::FlagSet;
use crate::includes::ArchIncludeDecl;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Types of buildable units in AROS mmakefiles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModuleType {
    Library,
    Device,
    Resource,
    Hidd,
    Datatype,
    Gadget,
    Mcc,
    Program,
    LinkLib,
    Package,
    Custom,
}

/// A parsed build target definition from an mmakefile.src.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetDefinition {
    pub mmake_name: String,
    pub target_name: String,
    pub module_type: ModuleType,
    pub source_files: Vec<String>,
    pub use_libs: Vec<String>,
    pub dependencies: Vec<String>,
    pub dir_path: PathBuf,
    pub target_dir: Option<String>,
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

/// Result of parsing an mmakefile.src.
#[derive(Debug, Clone, Default)]
pub struct ParsedMmakefile {
    pub targets: Vec<TargetDefinition>,
    pub meta_rules: Vec<MetaTargetRule>,
    /// `%set_archincludes` declarations contributed by this file.
    pub arch_decls: Vec<ArchIncludeDecl>,
    /// Include tokens whose Make variables were not resolved, for reporting.
    pub unresolved_includes: Vec<String>,
    /// `%copy_includes` declarations that stage public headers into the SDK.
    pub copy_includes: Vec<CopyIncludesDecl>,
    /// `%copy_includes` declarations that could not be resolved, for reporting.
    pub skipped_copy_includes: Vec<String>,
    /// Hand-written Make rules that stage headers; these need a static CMake
    /// counterpart and are reported so new ones do not go unnoticed.
    pub adhoc_header_rules: Vec<AdhocHeaderRule>,
    /// Hand-written `$(GENDIR)` rules producing something other than a header,
    /// for reporting.
    pub generated_file_rules: Vec<String>,
    /// Flags collected from this file, including what had to be skipped.
    pub flags: FlagSet,
    /// `%build_archspecific` declarations contributed by this file.
    pub arch_sources: Vec<ArchSourceDecl>,
    /// Declarations whose file list could not be resolved, for reporting.
    pub skipped_arch_sources: Vec<String>,
    /// `%fetch` declarations for third-party sources.
    pub fetches: Vec<FetchDecl>,
    /// `%fetch` declarations that could not be resolved, for reporting.
    pub skipped_fetches: Vec<String>,
    /// `-include .../make.opts` files that could not be used, for reporting.
    pub skipped_make_opts: Vec<String>,
    /// Make conditionals whose flags were dropped, for reporting.
    pub skipped_conditions: Vec<String>,
}
