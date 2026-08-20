use crate::arch_sources::collect_arch_sources;
use crate::ast::{MetaTargetRule, ModuleType, ParsedMmakefile, TargetDefinition};
use crate::copy_includes::collect_copy_includes;
use crate::fetch::collect_fetches;
use crate::flags::collect_flags;
use crate::includes::{collect_arch_decls, collect_includes};
use crate::make_opts::collect_make_opts;
use aros_common::Result;
use regex::Regex;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Makes a name safe to use as a CMake target.
///
/// A dot survives: CMake admits it, and dropping it renamed the binary. The
/// reference builds `atheros5000.device` and `wasapiaudio.dll`, which came out
/// as `atheros5000_device` and `wasapiaudio_dll`.
fn sanitize_ident(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' || c == '-' || c == '.' {
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
/// Resolves a name argument that may reference a Make variable.
///
/// Ten declarations name their output through a variable, for instance
/// `progname=$(EXE)` in external/openurl and `progname=$(EXENAME)` in
/// arch/all-pc/bootstrap. Sanitising those verbatim produced target names like
/// `__EXE_`, and two of them then collided on the same output file. A variable
/// that resolves to exactly one value is substituted; anything else returns
/// None so the caller can report it.
fn resolve_name(raw: &str, vars: &HashMap<String, Vec<String>>) -> Option<String> {
    if !raw.contains("$(") {
        return Some(sanitize_ident(raw));
    }
    let mut out = String::new();
    let mut rest = raw;
    while let Some(start) = rest.find("$(") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after.find(')')?;
        let name = &after[..end];
        let values = vars.get(name)?;
        if values.len() != 1 {
            return None;
        }
        out.push_str(&values[0]);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    if out.is_empty() {
        return None;
    }
    Some(sanitize_ident(&out))
}

/// Collects the source lists a build macro declares.
///
/// The reference treats files, cxxfiles, objcfiles and asmfiles as one set and
/// falls back to a default when all four are empty (make.tmpl:1643 for
/// programs, 2857ff for modules). Returns `(sources, any_declared)`; the flag
/// separates "nothing was declared" from "a list was declared but its Make
/// variables are unresolved", which must not silently fall back.
fn macro_sources(args: &str, vars: &HashMap<String, Vec<String>>) -> (Vec<String>, bool) {
    let mut sources = Vec::new();
    let mut declared = false;
    for key in ["files", "cxxfiles", "objcfiles", "asmfiles"] {
        let Some(raw) = macro_arg(args, key) else {
            continue;
        };
        if raw.trim().is_empty() {
            continue;
        }
        declared = true;
        sources.extend(expand_file_list(&raw, vars));
    }
    (sources, declared)
}

/// Lists the C sources in a directory, for the macros whose `files` default is
/// `$(basename $(call WILDCARD, *.c))`.
fn wildcard_c_sources(dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("c") {
                return None;
            }
            p.file_stem().map(|s| s.to_string_lossy().to_string())
        })
        .collect();
    out.sort_unstable();
    out
}

/// Joins `#MM` lines that continue over several source lines.
///
/// A continued dependency list repeats the `#MM` prefix on every line:
///
/// ```text
/// #MM kernel-bsp-pc-x86_64 :   \
/// #MM         kernel-log       \
/// #MM         kernel-ata
/// ```
///
/// so a per-line regex sees the first line with nothing after the colon but a
/// backslash, and the rest as separate rules with no colon at all. 2223 of the
/// tree's 5089 `#MM` lines are continuations, which is 44% of all metatarget
/// dependencies.
fn join_mm_continuations(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut pending = false;

    for line in content.lines() {
        let trimmed = line.trim_end();
        let is_mm = trimmed.trim_start().starts_with("#MM");
        let continues = trimmed.ends_with('\\');
        let body = trimmed.trim_end_matches('\\').trim_end();

        if pending {
            // Strip the repeated marker so the text reads as one rule.
            let stripped = body
                .trim_start()
                .strip_prefix("#MM-")
                .or_else(|| body.trim_start().strip_prefix("#MM"))
                .unwrap_or(body.trim_start());
            out.push(' ');
            out.push_str(stripped.trim());
        } else {
            out.push_str(body);
        }

        if is_mm && continues {
            pending = true;
        } else {
            pending = false;
            out.push('\n');
        }
    }
    out
}

/// One macro invocation from an mmakefile: its name and its argument text.
struct Invocation {
    name: String,
    args: String,
}

/// Splits an mmakefile into its macro invocations.
///
/// This replaces matching the whole file with one regex. With `(?s)` and a
/// non-greedy tail such as `(.*?)(?:%common|$)`, the first `%build_module` in a
/// file swallowed every later one, because most files carry a single `%common`
/// at the end. 14 files contributed one target each instead of all of theirs,
/// costing 60 targets, among them every Wanderer and Zune class.
fn macro_invocations(content: &str) -> Vec<Invocation> {
    // Continuations are joined first: nearly every declaration spreads its
    // arguments over several lines, and `mmake=` is often not on the first.
    let cont = Regex::new(r"\\\s*\n\s*").unwrap();
    let joined = cont.replace_all(content, " ");

    let mut out = Vec::new();
    for line in joined.lines() {
        let t = line.trim_start();
        let Some(after) = t.strip_prefix('%') else {
            continue;
        };
        let (name, args) = match after.find(char::is_whitespace) {
            Some(i) => (&after[..i], after[i..].trim()),
            None => (after, ""),
        };
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        out.push(Invocation {
            name: name.to_owned(),
            args: args.to_owned(),
        });
    }
    out
}

/// Reads `key=value` or `key="value with spaces"` from an argument text.
///
/// The key must sit at a word boundary, or `files=` also matches the tail of
/// `linklibfiles=` and returns the wrong argument.
fn macro_arg(args: &str, key: &str) -> Option<String> {
    let mut from = 0usize;
    loop {
        let hit = args[from..].find(key)? + from;
        let before_ok = hit == 0
            || args[..hit]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace);
        let rest = &args[hit + key.len()..];
        if before_ok {
            if let Some(v) = rest.strip_prefix("=\"") {
                let end = v.find('"')?;
                return Some(v[..end].to_owned());
            }
            if let Some(v) = rest.strip_prefix('=') {
                let end = v.find(char::is_whitespace).unwrap_or(v.len());
                let value = v[..end].trim();
                if !value.is_empty() {
                    return Some(value.to_owned());
                }
            }
        }
        from = hit + 1;
    }
}

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
    let copy_scan = collect_copy_includes(&content, &rel_dir);
    // USER_CPPFLAGS / USER_CFLAGS apply to every rule in the mmakefile, so the
    // same set is attached to each target parsed out of it.
    let mut flag_set = collect_flags(&content);
    let (packages, skipped_packages) = crate::packages::collect_packages(&content, &rel_dir);
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
                flag_set.compile_options.extend(opts_flags.compile_options);
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

    let invocations = macro_invocations(&content);
    let mut skipped_programs: Vec<String> = Vec::new();
    // Anchored on leading whitespace so that `linklibfiles=`, `asmfiles=`,
    // `cxxfiles=`, `objcfiles=` and `excludefiles=` are not mistaken for the
    // module's `files=` argument. The regex crate has no lookbehind.
    let re_files = Regex::new(r#"(?:^|\s)files=(?:"([^"]+)"|([^\s\\]+))"#).unwrap();
    let re_libs = Regex::new(r#"uselibs=(?:"([^"]+)"|([^\s\\]+))"#).unwrap();
    let re_mm = Regex::new(r"(?m)^#MM-?\s+([^\s:]+)\s*:\s*(.+)").unwrap();

    // 1. Extract module definitions
    for inv in invocations.iter().filter(|i| {
        matches!(
            i.name.as_str(),
            "build_module" | "build_module_abi" | "build_module_library"
        )
    }) {
        // The three spellings are 16-to-21-line wrappers around the same
        // %build_module_core; they differ only in flags that steer meta-target
        // wiring, not compilation (make.tmpl:2212).
        let Some(mmake_raw) = macro_arg(&inv.args, "mmake") else {
            continue;
        };
        let Some(mod_raw) = macro_arg(&inv.args, "modname") else {
            continue;
        };
        let mmake_name = sanitize_ident(&mmake_raw);
        let mod_name = sanitize_ident(&mod_raw);
        let mod_type_owned = macro_arg(&inv.args, "modtype").unwrap_or_default();
        let mod_type_str = mod_type_owned.as_str();
        let rest = inv.args.as_str();

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
            mod_suffix: None,
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
    //
    // %build_prog takes progname=/A and builds one executable from all its
    // files (make.tmpl:1810). %build_progs takes files=/A and builds one per
    // file (make.tmpl:1850). Both used to match the same regex, progname was
    // never read, and every file became its own program: the four sources of
    // `%build_prog progname=SysLog` came out as colorlist, hooks, main and str
    // instead of one SysLog. Only %build_prog is handled here; %build_progs
    // needs one mmake target to carry several executables, which the target
    // model does not express yet, so it is reported instead of guessed at.
    for inv in invocations.iter().filter(|i| i.name == "build_prog") {
        let Some(mmake_raw) = macro_arg(&inv.args, "mmake") else {
            continue;
        };
        let mmake_name = sanitize_ident(&mmake_raw);

        // progname is declared /A, so a declaration without one is malformed
        // rather than something to guess a name for.
        let Some(prog_raw) = macro_arg(&inv.args, "progname") else {
            skipped_programs.push(format!(
                "{}: %build_prog mmake={mmake_raw} has no progname",
                rel_dir.display()
            ));
            continue;
        };
        let Some(prog_name) = resolve_name(&prog_raw, &vars) else {
            skipped_programs.push(format!(
                "{}: %build_prog mmake={mmake_raw} progname={prog_raw} is unresolved",
                rel_dir.display()
            ));
            continue;
        };

        let (mut source_files, declared_any) = macro_sources(&inv.args, &vars);
        if source_files.is_empty() {
            if declared_any {
                // A list was given but its Make variables are unresolved.
                // Falling back to the program name here would compile the
                // wrong file, so report instead.
                skipped_programs.push(format!(
                    "{}: %build_prog mmake={mmake_raw} progname={prog_raw} has an unresolved file list",
                    rel_dir.display()
                ));
                continue;
            }
            source_files.push(prog_name.clone());
        }

        let use_libs =
            macro_arg(&inv.args, "uselibs").map_or_else(Vec::new, |l| expand_file_list(&l, &vars));

        targets.push(TargetDefinition {
            mmake_name,
            target_name: prog_name,
            module_type: ModuleType::Program,
            source_files,
            use_libs,
            dependencies: Vec::new(),
            dir_path: rel_dir.clone(),
            target_dir: None,
            mod_suffix: None,
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

    // 2b. The remaining build macros.
    //
    // All four share the compile model and differ only in what they link:
    // %build_prog one executable, %build_progs one per file, %build_linklib a
    // static library, %build_module_simple a module without the genmodule
    // chain. Only the link kind and the name argument change here.
    for inv in &invocations {
        let (module_type, name_arg) = match inv.name.as_str() {
            "build_progs" => (ModuleType::ProgramGroup, None),
            "build_linklib" => (ModuleType::LinkLib, Some("libname")),
            "build_module_simple" => (ModuleType::SimpleModule, Some("modname")),
            _ => continue,
        };

        let Some(mmake_raw) = macro_arg(&inv.args, "mmake") else {
            continue;
        };
        let mmake_name = sanitize_ident(&mmake_raw);

        // %build_progs has no name of its own: each source file names its own
        // executable, so the mmake id carries the group.
        let target_name = match name_arg {
            None => mmake_name.clone(),
            Some(key) => match macro_arg(&inv.args, key).and_then(|v| {
                resolve_name(&v, &vars).or_else(|| {
                    skipped_programs.push(format!(
                        "{}: %{} mmake={mmake_raw} {key}={v} is unresolved",
                        rel_dir.display(),
                        inv.name
                    ));
                    None
                })
            }) {
                Some(v) => v,
                None => {
                    if macro_arg(&inv.args, key).is_none() {
                        skipped_programs.push(format!(
                            "{}: %{} mmake={mmake_raw} has no {key}",
                            rel_dir.display(),
                            inv.name
                        ));
                    }
                    continue;
                }
            },
        };

        let (mut source_files, declared_any) = macro_sources(&inv.args, &vars);
        if source_files.is_empty() {
            if declared_any {
                skipped_programs.push(format!(
                    "{}: %{} mmake={mmake_raw} has an unresolved file list",
                    rel_dir.display(),
                    inv.name
                ));
                continue;
            }
            // %build_module_simple defaults files to every *.c in the
            // directory. The others have no default, and %build_progs even
            // declares files=/A, so a declaration without sources is
            // malformed.
            if matches!(module_type, ModuleType::SimpleModule) {
                source_files = wildcard_c_sources(parent_dir);
            }
            if source_files.is_empty() {
                skipped_programs.push(format!(
                    "{}: %{} mmake={mmake_raw} declares no sources",
                    rel_dir.display(),
                    inv.name
                ));
                continue;
            }
        }

        let use_libs =
            macro_arg(&inv.args, "uselibs").map_or_else(Vec::new, |l| expand_file_list(&l, &vars));
        let mod_suffix = if matches!(module_type, ModuleType::SimpleModule) {
            macro_arg(&inv.args, "modtype")
        } else {
            None
        };

        targets.push(TargetDefinition {
            mmake_name,
            target_name,
            module_type,
            source_files,
            use_libs,
            dependencies: Vec::new(),
            dir_path: rel_dir.clone(),
            target_dir: None,
            mod_suffix,
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

    // %build_module_macro is invoked five times but defined nowhere in the
    // tree. Four of the five sit under arch/.unmaintained or an architecture
    // we do not build, and one carries a "converted without testing" note, so
    // the historic build cannot expand it either.
    for inv in invocations
        .iter()
        .filter(|i| i.name == "build_module_macro")
    {
        if let Some(m) = macro_arg(&inv.args, "mmake") {
            skipped_programs.push(format!(
                "{}: %build_module_macro mmake={m} (macro is not defined anywhere in the tree)",
                rel_dir.display()
            ));
        }
    }

    // 3. Extract #MM and #MM- meta-target rules
    let mm_content = join_mm_continuations(&content);
    for cap in re_mm.captures_iter(&mm_content) {
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
        copy_includes: copy_scan.decls,
        skipped_copy_includes: copy_scan.skipped,
        adhoc_header_rules: copy_scan.adhoc,
        generated_file_rules: copy_scan.generated_files,
        flags: flag_set,
        arch_sources,
        skipped_arch_sources,
        fetches,
        skipped_fetches,
        skipped_make_opts,
        skipped_conditions,
        skipped_programs,
        packages,
        skipped_packages,
    })
}

#[cfg(test)]
mod tests {
    use super::{macro_arg, macro_invocations, sanitize_ident};

    #[test]
    fn every_declaration_in_a_file_is_seen() {
        // workbench/system/Wanderer/Classes and 13 other files declare several
        // modules with one %common at the end. The previous whole-file regex
        // ended on `(.*?)(?:%common|$)`, so the first match swallowed the rest
        // and 60 targets went missing.
        let src = "\
%build_module  mmake=wanderer-classes-icon modname=Icon modtype=mui files=icon
%build_module  mmake=wanderer-classes-iconlist modname=IconList modtype=mui files=iconlist
%build_module  mmake=wanderer-classes-iconlistview modname=IconListview modtype=mui files=iconlistview

%common
";
        let names: Vec<String> = macro_invocations(src)
            .iter()
            .filter(|i| i.name == "build_module")
            .filter_map(|i| macro_arg(&i.args, "mmake"))
            .collect();
        assert_eq!(
            names,
            vec![
                "wanderer-classes-icon",
                "wanderer-classes-iconlist",
                "wanderer-classes-iconlistview"
            ]
        );
    }

    #[test]
    fn arguments_spread_over_lines_belong_to_their_declaration() {
        let src = "\
%build_prog mmake=aros-tcpip-apps-syslog \\
    progname=SysLog targetdir=$(EXEDIR) \\
    files=$(FILES)

%build_prog mmake=other progname=Other files=other
";
        let invs = macro_invocations(src);
        let progs: Vec<&super::Invocation> =
            invs.iter().filter(|i| i.name == "build_prog").collect();
        assert_eq!(progs.len(), 2);
        assert_eq!(macro_arg(&progs[0].args, "progname").unwrap(), "SysLog");
        assert_eq!(macro_arg(&progs[0].args, "files").unwrap(), "$(FILES)");
        assert_eq!(macro_arg(&progs[1].args, "progname").unwrap(), "Other");
    }

    #[test]
    fn an_argument_name_must_match_at_a_word_boundary() {
        // Searching for `files=` as a substring also hits `linklibfiles=` and
        // `cxxfiles=`, and would return the wrong list.
        let args = "mmake=x linklibfiles=\"a b\" cxxfiles=c files=\"d e\"";
        assert_eq!(macro_arg(args, "files").unwrap(), "d e");
        assert_eq!(macro_arg(args, "linklibfiles").unwrap(), "a b");
        assert_eq!(macro_arg(args, "cxxfiles").unwrap(), "c");
    }

    #[test]
    fn a_missing_argument_is_none() {
        assert!(macro_arg("mmake=x files=y", "progname").is_none());
        // An empty value is not a value.
        assert!(macro_arg("mmake=x progname= files=y", "progname").is_none());
    }

    #[test]
    fn a_dot_survives_sanitising() {
        assert_eq!(sanitize_ident("atheros5000.device"), "atheros5000.device");
        assert_eq!(sanitize_ident("wasapiaudio.dll"), "wasapiaudio.dll");
        assert_eq!(sanitize_ident("odd/name"), "odd_name");
    }

    #[test]
    fn non_macro_lines_are_ignored() {
        let src = "FILES := a b c\n# %build_module in a comment\n%common\n";
        let invs = macro_invocations(src);
        let names: Vec<&str> = invs.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, vec!["common"]);
    }

    #[test]
    fn a_name_argument_resolves_through_a_variable() {
        // external/openurl declares progname=$(EXE) with EXE := OpenURL.
        // Sanitising it verbatim produced the target name __EXE_, and two such
        // targets then collided on the same output file.
        let mut vars = std::collections::HashMap::new();
        vars.insert("EXE".to_owned(), vec!["OpenURL".to_owned()]);
        assert_eq!(super::resolve_name("$(EXE)", &vars).unwrap(), "OpenURL");
        assert_eq!(
            super::resolve_name("mesa3dgl$(EXE)", &vars).unwrap(),
            "mesa3dglOpenURL"
        );
    }

    #[test]
    fn an_unresolvable_name_is_refused() {
        let vars = std::collections::HashMap::new();
        assert!(super::resolve_name("$(EXENAME)", &vars).is_none());
        // A variable holding a list cannot name one target.
        let mut many = std::collections::HashMap::new();
        many.insert("L".to_owned(), vec!["a".to_owned(), "b".to_owned()]);
        assert!(super::resolve_name("$(L)", &many).is_none());
    }

    #[test]
    fn all_four_source_lists_are_read() {
        // developer/debug/test/cplusplus declares files="" cxxfiles="exception".
        let vars = std::collections::HashMap::new();
        let (srcs, declared) = super::macro_sources(
            r#"mmake=x progname=exception files="" cxxfiles="exception""#,
            &vars,
        );
        assert!(declared);
        assert_eq!(srcs, vec!["exception"]);
    }

    #[test]
    fn nothing_declared_is_distinct_from_nothing_resolved() {
        let vars = std::collections::HashMap::new();
        let (srcs, declared) = super::macro_sources("mmake=x progname=p", &vars);
        assert!(srcs.is_empty());
        assert!(!declared, "no list was given at all");

        let (srcs, declared) = super::macro_sources("mmake=x files=$(UNKNOWN)", &vars);
        assert!(srcs.is_empty());
        assert!(declared, "a list was given but did not resolve");
    }
}
