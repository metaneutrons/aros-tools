pub mod arch_sources;
pub mod ast;
pub mod catalogs;
pub mod copy_includes;
pub mod dirs;
pub mod fetch;
pub mod flags;
pub mod generator;
pub mod graph;
pub mod icons;
pub mod includes;
pub mod make_expr;
pub mod make_opts;
pub mod packages;
pub mod parser;

pub use arch_sources::ArchSourceDecl;
pub use ast::{ModuleType, TargetDefinition};
pub use catalogs::CatalogDecl;
pub use copy_includes::CopyIncludesDecl;
pub use fetch::FetchDecl;
pub use flags::FlagSet;
pub use generator::generate_cmake;
pub use graph::DependencyGraph;
pub use icons::{IconSet, IconTarget};
pub use includes::{ArchIncludeDecl, IncludeSet};
pub use make_expr::{
    evaluate_make_expr, evaluate_make_list, MakeExprContext, MakeExprError, MakeVariableGuard,
    MakeVariableLookup,
};
pub use make_opts::MakeOptsFile;
pub use parser::{
    parse_mmakefile, parse_mmakefile_with_context, parse_mmakefile_with_dirs,
    parse_mmakefile_with_dirs_and_context, TargetContext,
};
