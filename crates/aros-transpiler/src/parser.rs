use crate::ast::{ModuleType, TargetDefinition};
use aros_common::Result;
use regex::Regex;
use std::fs;
use std::path::Path;

/// Parses a single `mmakefile.src` file into structured target definitions.
///
/// # Errors
/// Returns an error if the file cannot be read.
#[allow(clippy::missing_panics_doc)]
pub fn parse_mmakefile(path: &Path, root: &Path) -> Result<Vec<TargetDefinition>> {
    let content = fs::read_to_string(path)?;
    let parent_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let rel_dir = parent_dir
        .strip_prefix(root)
        .unwrap_or(parent_dir)
        .to_path_buf();
    let mut targets = Vec::new();

    let re_module = Regex::new(r"(?s)%build_module\s+mmake=([^\s\\]+).*?modname=([^\s\\]+).*?modtype=([^\s\\]+)(.*?)(?:%common|$)").unwrap();
    let re_prog = Regex::new(r"%build_prog(?:s)?\s+mmake=([^\s\\]+).*?files=([^\n]+)").unwrap();
    let re_deps = Regex::new(r"#MM\s+([^\s:]+)\s*:\s*(.+)").unwrap();
    let re_files = Regex::new(r#"files=(?:"([^"]+)"|([^\s\\]+))"#).unwrap();
    let re_libs = Regex::new(r#"uselibs=(?:"([^"]+)"|([^\s\\]+))"#).unwrap();

    // Extract module definitions
    for cap in re_module.captures_iter(&content) {
        let mmake_name = cap[1].to_string();
        let mod_name = cap[2].to_string();
        let mod_type_str = &cap[3];
        let rest = &cap[4];

        let module_type = match mod_type_str {
            "library" => ModuleType::Library,
            "device" => ModuleType::Device,
            "resource" => ModuleType::Resource,
            "hidd" => ModuleType::Hidd,
            "datatype" => ModuleType::Datatype,
            "gadget" => ModuleType::Gadget,
            "mcc" => ModuleType::Mcc,
            _ => ModuleType::Custom,
        };

        let source_files: Vec<String> = re_files.captures(rest).map_or_else(Vec::new, |fcap| {
            let files_str = fcap
                .get(1)
                .or_else(|| fcap.get(2))
                .map_or("", |m| m.as_str());
            files_str
                .split_whitespace()
                .map(|s| s.replace(['"', '\\'], "").trim().to_string())
                .filter(|s| !s.is_empty() && !s.starts_with("$("))
                .collect()
        });

        let use_libs: Vec<String> = re_libs.captures(rest).map_or_else(Vec::new, |lcap| {
            let libs_str = lcap
                .get(1)
                .or_else(|| lcap.get(2))
                .map_or("", |m| m.as_str());
            libs_str
                .split_whitespace()
                .map(|s| s.replace(['"', '\\'], "").trim().to_string())
                .filter(|s| !s.is_empty() && !s.starts_with("$("))
                .collect()
        });

        targets.push(TargetDefinition {
            mmake_name,
            target_name: mod_name,
            module_type,
            source_files,
            use_libs,
            dependencies: Vec::new(),
            dir_path: rel_dir.clone(),
            target_dir: None,
            compiler_flags: Vec::new(),
        });
    }

    // Extract program definitions
    for cap in re_prog.captures_iter(&content) {
        let mmake_name = cap[1].to_string();
        let files_str = cap[2].trim_matches('"');
        let source_files: Vec<String> = files_str
            .split_whitespace()
            .map(|s| s.replace(['"', '\\'], "").trim().to_string())
            .filter(|s| !s.is_empty() && !s.starts_with("$("))
            .collect();
        let prog_name = mmake_name
            .split('-')
            .next_back()
            .unwrap_or("prog")
            .to_string();

        targets.push(TargetDefinition {
            mmake_name,
            target_name: prog_name,
            module_type: ModuleType::Program,
            source_files,
            use_libs: Vec::new(),
            dependencies: Vec::new(),
            dir_path: rel_dir.clone(),
            target_dir: None,
            compiler_flags: Vec::new(),
        });
    }

    // Extract dependencies
    for cap in re_deps.captures_iter(&content) {
        let target_id = &cap[1];
        let deps_str = &cap[2];
        let deps: Vec<String> = deps_str
            .split_whitespace()
            .map(ToString::to_string)
            .collect();

        for t in &mut targets {
            if t.mmake_name == target_id {
                t.dependencies.extend(deps.clone());
            }
        }
    }

    Ok(targets)
}
