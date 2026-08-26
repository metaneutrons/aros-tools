pub mod arch_sources;
pub mod ast;
pub mod binary_objects;
pub mod capability;
pub mod catalogs;
pub mod collector;
pub mod copy_directories;
pub mod copy_includes;
pub mod default_link_set;
pub mod dirs;
pub mod fetch;
pub mod fingerprints;
pub mod flags;
pub mod flexcat;
pub mod generator;
pub mod genmodule_linklibs;
pub mod graph;
pub mod hidd_stubs;
pub mod host_generated_headers;
pub mod icons;
pub mod includes;
pub mod local_make_includes;
pub mod make_deps;
pub mod make_expr;
pub mod make_opts;
pub mod make_vars;
pub mod module_paths;
pub mod packages;
pub mod parser;
pub mod sources;

pub use arch_sources::ArchSourceDecl;
pub use ast::{
    AhiBuildDecl, ConfigureBuildDecl, CopyDirectoryDecl, DefineHeaderDecl, ExternalCMakeDecl,
    GrubBuildDecl, ModuleType, PythonGeneratorJob, PythonOutputsDecl, PythonPackageDecl,
    TargetDefinition,
};
pub use catalogs::CatalogDecl;
pub use copy_includes::CopyIncludesDecl;
pub use default_link_set::{
    default_link_set_available, read_default_link_set, DefaultLinkItem, DefaultLinkSet,
};
pub use fetch::FetchDecl;
pub use flags::FlagSet;
pub use flexcat::FlexCatSourceDecl;
pub use generator::{generate_cmake, generated_header};
pub use genmodule_linklibs::{resolve_generated_linklib_sources, GeneratedLinklibSources};
pub use graph::DependencyGraph;
pub use icons::{IconSet, IconTarget};
pub use includes::{ArchIncludeDecl, IncludeSet};
pub use local_make_includes::{
    inline_local_make_includes, IncludedLocalMakeFragment, LocalMakeFragmentPolicy,
    LocalMakeIncludeIssue, LocalMakeIncludeIssueKind, LocalMakeIncludeLimits, LocalMakeIncludeScan,
};
pub use make_expr::{
    evaluate_make_expr, evaluate_make_list, MakeExprContext, MakeExprError, MakeVariableGuard,
    MakeVariableLookup,
};
pub use make_opts::MakeOptsFile;
pub use parser::{
    collect_mmakefile_fetches_with_context, parse_mmakefile, parse_mmakefile_with_context,
    parse_mmakefile_with_dirs, parse_mmakefile_with_dirs_and_context,
    parse_mmakefile_with_dirs_and_context_and_fetches, TargetContext,
};

#[cfg(test)]
pub mod testing;
