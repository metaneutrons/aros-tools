use crate::arch_sources::collect_arch_sources;
use crate::copy_includes::collect_copy_includes;
use crate::fetch::collect_fetches;
use crate::flags::collect_flags;
use crate::includes::{collect_arch_decls, collect_includes};
use crate::make_opts::collect_make_opts;
use crate::ast::{MetaTargetRule, ModuleType, ParsedMmakefile, TargetDefinition};
use aros_common::Result;
use regex::Regex;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

fn sanitize_ident(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn expand_file_list(raw: &str, vars: &HashMap<String, Vec<String>>) -> Vec<String> {
    let mut result = Vec::new();
    for token in raw.split_whitespace() {
        let cleaned = token.replace(['"', '\\'], "").trim().to_string();
        if cleaned.is_empty()
            || cleaned.contains('(')
            || cleaned.contains(')')
            || cleaned.contains('$')
            || cleaned.contains(',')
            || cleaned.contains('/')
        {
            if cleaned.starts_with("$(") && cleaned.ends_with(')') {
                let var_name = &cleaned[2..cleaned.len() - 1];
                if let Some(list) = vars.get(var_name) {
                    for item in list {
                        if !item.contains('(')
                            && !item.contains('$')
                            && !item.contains('/')
                            && !item.is_empty()
                        {
                            result.push(sanitize_ident(item));
                        }
                    }
                }
            }
            continue;
        }
        result.push(sanitize_ident(&cleaned));
    }
    result
}

/// Parses a single `mmakefile.src` file into structured target definitions and meta-target rules.
///
/// # Errors
/// Returns an error if the file cannot be read.
#[allow(clippy::missing_panics_doc)]
pub fn parse_mmakefile(path: &Path, root: &Path) -> Result<ParsedMmakefile> {
    let content = fs::read_to_string(path)?;
    let parent_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let rel_dir = parent_dir
        .strip_prefix(root)
        .unwrap_or(parent_dir)
        .to_path_buf();
    let mut targets = Vec::new();
    let mut meta_rules = Vec::new();

    // Include paths are a file-level property in Make: USER_INCLUDES applies to
    // every rule in the mmakefile, so the same set is attached to each target
    // parsed out of this file.
    let include_set = collect_includes(&content, &rel_dir);
    let arch_decls = collect_arch_decls(&content, &rel_dir);
    let (copy_includes, skipped_copy_includes, adhoc_header_rules) =
        collect_copy_includes(&content, &rel_dir);
    // USER_CPPFLAGS / USER_CFLAGS apply to every rule in the mmakefile, so the
    // same set is attached to each target parsed out of it.
    let mut flag_set = collect_flags(&content);
    let (mut arch_sources, skipped_arch_sources) = collect_arch_sources(&content, &rel_dir);
    // A %build_archspecific file contributes to a target defined elsewhere, so
    // its own USER_INCLUDES and flags have to travel with the declaration.
    for d in &mut arch_sources {
        d.include_dirs = include_set.dirs.clone();
        d.defines = flag_set.defines.clone();
        d.compile_options = flag_set.compile_options.clone();
    }
    let (fetches, skipped_fetches) = collect_fetches(&content, &rel_dir);

    // Architecture option files. Their contents are tagged with the
    // architecture they belong to, so CMake can keep the ones that apply; the
    // transpiler itself stays target-agnostic.
    let (opts_files, skipped_make_opts) = collect_make_opts(&content, &rel_dir, root);
    let skipped_conditions = flag_set.skipped_conditions.clone();
    // Flags guarded by an `ifeq` on the CPU or platform are already tagged by
    // the flag collector; the make.opts contents are appended below.
    let mut arch_defines: Vec<(String, String)> = flag_set.arch_defines.clone();
    let mut arch_compile_options: Vec<(String, String)> = flag_set.arch_compile_options.clone();
    let mut opts_include_dirs: Vec<String> = Vec::new();
    let mut opts_arch_includes: Vec<(String, String)> = Vec::new();
    for f in &opts_files {
        let Ok(body) = fs::read_to_string(root.join(&f.path)) else {
            continue;
        };
        let opts_flags = collect_flags(&body);
        // Include paths from an option file are resolved against the including
        // mmakefile's directory, which is what Make does.
        let opts_incs = collect_includes(&body, &rel_dir);
        match &f.tag {
            Some(tag) => {
                for d in opts_flags.defines {
                    arch_defines.push((tag.clone(), d));
                }
                for o in opts_flags.compile_options {
                    arch_compile_options.push((tag.clone(), o));
                }
                for d in opts_incs.dirs {
                    opts_arch_includes.push((tag.clone(), d));
                }
            }
            None => {
                // A local make.opts always applies.
                flag_set.defines.extend(opts_flags.defines);
                flag_set
                    .compile_options
                    .extend(opts_flags.compile_options);
                opts_include_dirs.extend(opts_incs.dirs);
            }
        }
    }


    // Collect Makefile variable assignments
    let mut vars: HashMap<String, Vec<String>> = HashMap::new();
    let mut current_var: Option<String> = None;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.starts_with('%') {
            current_var = None;
            continue;
        }

        if let Some((k, v)) = line.split_once(":=") {
            let var_name = k.trim().to_string();
            let values: Vec<String> = v
                .split_whitespace()
                .filter(|s| *s != "\\")
                .map(|s| s.replace(['"', '\\'], "").trim().to_string())
                .filter(|s| !s.is_empty() && !s.contains('/') && !s.contains('$'))
                .collect();
            vars.insert(var_name.clone(), values);
            current_var = if line.ends_with('\\') {
                Some(var_name)
            } else {
                None
            };
        } else if let Some(ref var_name) = current_var {
            let values: Vec<String> = trimmed
                .split_whitespace()
                .filter(|s| *s != "\\")
                .map(|s| s.replace(['"', '\\'], "").trim().to_string())
                .filter(|s| !s.is_empty() && !s.contains('/') && !s.contains('$'))
                .collect();
            if let Some(existing) = vars.get_mut(var_name) {
                existing.extend(values);
            }
            if !line.ends_with('\\') {
                current_var = None;
            }
        }
    }

    let re_module = Regex::new(r"(?s)%build_module\s+mmake=([^\s\\]+).*?modname=([^\s\\]+).*?modtype=([^\s\\]+)(.*?)(?:%common|$)").unwrap();
    let re_prog = Regex::new(
        r#"(?s)%build_prog(?:s)?\s+mmake=([^\s\\]+).*?\sfiles=(?:"([^"]+)"|([^\s\\]+))"#,
    )
    .unwrap();
    // Anchored on leading whitespace so that `linklibfiles=`, `asmfiles=`,
    // `cxxfiles=`, `objcfiles=` and `excludefiles=` are not mistaken for the
    // module's `files=` argument. The regex crate has no lookbehind.
    let re_files = Regex::new(r#"(?:^|\s)files=(?:"([^"]+)"|([^\s\\]+))"#).unwrap();
    let re_libs = Regex::new(r#"uselibs=(?:"([^"]+)"|([^\s\\]+))"#).unwrap();
    let re_mm = Regex::new(r"(?m)^#MM-?\s+([^\s:]+)\s*:\s*(.+)").unwrap();

    // 1. Extract module definitions
    for cap in re_module.captures_iter(&content) {
        let mmake_name = sanitize_ident(&cap[1]);
        let mod_name = sanitize_ident(&cap[2]);
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
            expand_file_list(files_str, &vars)
        });

        let use_libs: Vec<String> = re_libs.captures(rest).map_or_else(Vec::new, |lcap| {
            let libs_str = lcap
                .get(1)
                .or_else(|| lcap.get(2))
                .map_or("", |m| m.as_str());
            expand_file_list(libs_str, &vars)
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
            include_dirs: {
                let mut d = include_set.dirs.clone();
                d.extend(opts_include_dirs.iter().cloned());
                d
            },
            arch_modules: include_set.arch_modules.clone(),
            arch_includes: opts_arch_includes.clone(),
            defines: flag_set.defines.clone(),
            undefines: flag_set.undefines.clone(),
            compile_options: flag_set.compile_options.clone(),
            arch_sources: Vec::new(),
            arch_defines: arch_defines.clone(),
            arch_compile_options: arch_compile_options.clone(),
        });
    }

    // 2. Extract program definitions
    for cap in re_prog.captures_iter(&content) {
        let mmake_name = sanitize_ident(&cap[1]);
        let files_raw = cap
            .get(2)
            .or_else(|| cap.get(3))
            .map_or("", |m| m.as_str())
            .trim()
            .trim_matches('"');

        // Check if files references $(VAR)
        let resolved_progs: Vec<(String, String)> =
            if files_raw.starts_with("$(") && files_raw.ends_with(')') {
                let var_name = &files_raw[2..files_raw.len() - 1];
                vars.get(var_name).map_or_else(
                    || {
                        vec![(
                            mmake_name.clone(),
                            mmake_name
                                .split('-')
                                .next_back()
                                .unwrap_or("prog")
                                .to_string(),
                        )]
                    },
                    |list| {
                        list.iter()
                            .map(|name| {
                                (
                                    format!("{mmake_name}-{}", name.to_lowercase()),
                                    sanitize_ident(name),
                                )
                            })
                            .collect()
                    },
                )
            } else {
                let prog_name = sanitize_ident(mmake_name.split('-').next_back().unwrap_or("prog"));
                vec![(mmake_name.clone(), prog_name)]
            };

        for (target_mmake, prog_name) in resolved_progs {
            let source_file = format!("{prog_name}.c");
            targets.push(TargetDefinition {
                mmake_name: target_mmake,
                target_name: prog_name,
                module_type: ModuleType::Program,
                source_files: vec![source_file],
                use_libs: Vec::new(),
                dependencies: Vec::new(),
                dir_path: rel_dir.clone(),
                target_dir: None,
                compiler_flags: Vec::new(),
                include_dirs: {
                    let mut d = include_set.dirs.clone();
                    d.extend(opts_include_dirs.iter().cloned());
                    d
                },
                arch_modules: include_set.arch_modules.clone(),
                arch_includes: opts_arch_includes.clone(),
                defines: flag_set.defines.clone(),
                undefines: flag_set.undefines.clone(),
                compile_options: flag_set.compile_options.clone(),
                arch_sources: Vec::new(),
                arch_defines: arch_defines.clone(),
                arch_compile_options: arch_compile_options.clone(),
            });
        }
    }

    // 3. Extract #MM and #MM- meta-target rules
    for cap in re_mm.captures_iter(&content) {
        let meta_name = sanitize_ident(&cap[1]);
        let deps_str = &cap[2];
        let deps: Vec<String> = deps_str
            .split_whitespace()
            .filter(|s| !s.contains('$') && !s.contains('(') && !s.contains(')'))
            .map(sanitize_ident)
            .filter(|s| !s.is_empty())
            .collect();

        if !deps.is_empty() {
            meta_rules.push(MetaTargetRule {
                name: meta_name,
                dependencies: deps,
            });
        }
    }

    Ok(ParsedMmakefile {
        targets,
        meta_rules,
        arch_decls,
        unresolved_includes: include_set.unresolved,
        copy_includes,
        skipped_copy_includes,
        adhoc_header_rules,
        flags: flag_set,
        arch_sources,
        skipped_arch_sources,
        fetches,
        skipped_fetches,
        skipped_make_opts,
        skipped_conditions,
    })
}
