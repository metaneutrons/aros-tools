pub mod ast;
pub mod generator;
pub mod graph;
pub mod parser;

pub use ast::{ModuleType, TargetDefinition};
pub use generator::generate_cmake;
pub use graph::DependencyGraph;
pub use parser::parse_mmakefile;
