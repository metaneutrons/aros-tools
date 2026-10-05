pub mod arch_endpoint_effects;
pub mod arch_sources;
pub mod assembly_headers;
pub mod ast;
pub mod binary_objects;
pub mod capability;
pub mod catalogs;
pub mod collector;
pub mod copy_directories;
pub mod copy_includes;
pub mod default_link_set;
pub mod directory_setup;
pub mod dirs;
pub mod fetch;
pub mod fingerprints;
pub mod flags;
pub mod flexcat;
pub mod generator;
pub mod genmf_projection;
pub mod genmodule_header_rules;
pub mod genmodule_linklibs;
pub mod genmodule_writefiles_rules;
pub mod graph;
pub mod hidd_stubs;
pub mod host_c_file_rules;
pub mod host_generated_headers;
pub mod host_header_aggregates;
pub mod host_header_rules;
pub mod icons;
pub mod ilbm;
pub mod includes;
pub mod kobj_scoped_inputs;
pub mod layered_header_copies;
mod literal_header_copies;
pub mod literal_objects;
pub mod local_make_includes;
pub mod make_deps;
pub mod make_expr;
pub mod make_opts;
pub mod make_vars;
pub mod metamake_owner_graph;
pub mod metamake_project;
pub mod module_paths;
mod native_meta_providers;
pub mod native_owner_projection;
pub mod native_parser_origins;
pub mod packages;
pub mod parser;
pub mod sdk_asset_rules;
pub mod sdk_file_copies;
pub mod sdk_objects;
pub mod sdk_text_rules;
pub mod sfd_header_rules;
pub mod source_archive_binding;
pub mod source_archive_command;
pub mod source_archive_rules;
pub mod source_compile_rules;
pub mod source_dependency_overlays;
mod source_directory_rules;
pub mod source_header_pipeline;
mod source_rule_ownership;
pub mod source_text_rules;
pub mod source_value_rules;
pub mod sources;
mod static_header_copies;

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
