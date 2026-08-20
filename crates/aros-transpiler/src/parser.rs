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
        let prog_name = sanitize_ident(&prog_raw);

        // The reference takes files, cxxfiles, objcfiles and asmfiles
        // together, and falls back to the program name when all four are empty
        // (make.tmpl:1643). 15 of the 264 declarations rely on one of those.
        let mut declared_any = false;
        let mut source_files = Vec::new();
        for key in ["files", "cxxfiles", "objcfiles", "asmfiles"] {
            let Some(raw) = macro_arg(&inv.args, key) else {
                continue;
            };
            if raw.trim().is_empty() {
                continue;
            }
            declared_any = true;
            source_files.extend(expand_file_list(&raw, &vars));
        }
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

    // Declarations of the kinds not modelled yet, so they surface as a count
    // rather than as targets that quietly never exist.
    for inv in &invocations {
        let kind = match inv.name.as_str() {
            "build_progs" => "one executable per file",
            "build_linklib" => "link library",
            "build_module_simple" => "module without genmodule",
            _ => continue,
        };
        if let Some(m) = macro_arg(&inv.args, "mmake") {
            skipped_programs.push(format!(
                "{}: %{} mmake={m} ({kind})",
                rel_dir.display(),
                inv.name
            ));
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
}
