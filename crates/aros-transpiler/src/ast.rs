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
}
