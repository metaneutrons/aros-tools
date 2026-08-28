use crate::arch_sources::collect_arch_sources;
use crate::ast::{
    GenmoduleLinklibs, MetaTargetRule, ModuleType, ParsedMmakefile, TargetDefinition,
};
use crate::capability::literal_defines::safe_build_tree_output_directory;
use crate::capability::mesa::{
    compile_contract, current_profile, remaining_linklib_sources, PRIVATE_LIBDIR,
};
use crate::capability::mesa::{generators, mesa20, sse41};
use crate::capability::{external_cmake, literal_defines};
use crate::collector::{
    all_sources_are_fetch_owned, collector_forward_local_prelude, declaration_owned_port_scope,
    inline_collector_make_includes,
};
use crate::copy_directories;
use crate::copy_includes::collect_copy_includes_with_scope;
use crate::fetch::{collect_fetches_with_scope, FetchDecl};
use crate::flags::{collect_flags, collect_flags_at, collect_named_link_flags_at, FlagSet};
use crate::flexcat::collect_flexcat_source_rules;
use crate::genmodule_linklibs::resolve_generated_linklib_sources;
use crate::ilbm::collect_ilbm_sources;
use crate::includes::{collect_arch_decls, collect_includes, collect_includes_at};
use crate::local_make_includes::{
    inline_local_make_includes, LocalMakeFragmentPolicy, LocalMakeIncludeLimits,
};
use crate::make_expr::{evaluate_make_expr, MakeExprContext};
use crate::make_opts::collect_make_opts;
use crate::make_vars::{
    collect_vars, collect_vars_impl, collect_vars_impl_with_forward_locals, ConditionalTruth,
    VarScope,
};
use crate::module_paths::{
    implicit_module_meta_rules, is_explicit_genmodule_only, resolve_module_suffix,
    resolve_module_target_dir, resolve_yes_argument,
};
use crate::sources::{
    evaluate_linklib_list, evaluate_macro_sources, evaluate_macro_sources_with_files,
    expand_file_list, map_linklib_object_sources, EvaluatedSources,
};
use aros_common::{read_source, Result};
use aros_common::{Diagnostic, DiagnosticCode, DiagnosticStage, SourceLocation};
use regex::Regex;
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::sync::LazyLock;

/// One non-empty #MM rule. Horizontal whitespace is intentional: `\s*` also
/// consumes a newline, so an empty `#MM setup-ppc :` used to steal the next
/// ordinary Make rule and manufacture `setup-ppc -> setup-ppc` self-cycles.
pub(crate) static META_RULE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^#MM-?[ \t]+([^ \t\r\n:]+)[ \t]*:[ \t]*([^\r\n]+)").unwrap());
static CONTINUATION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\[ \t]*\r?\n[ \t]*").unwrap());

/// Makes a name safe to use as a CMake target.
///
/// A dot survives: CMake admits it, and dropping it renamed the binary. The
/// reference builds `atheros5000.device` and `wasapiaudio.dll`, which came out
/// as `atheros5000_device` and `wasapiaudio_dll`.
pub(crate) fn sanitize_ident(s: &str) -> String {
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

/// Renders the target-configuration variables used in #MM rules as CMake
/// variable references.
///
/// Dropping every dependency containing `$(` removed the root edge from
/// `workbench` to the selected icon set, so 181 correctly generated icon rules
/// would still be unreachable. Only variables with an unambiguous counterpart
/// are translated; callers report every other dynamic token.
fn render_meta_token(raw: &str) -> Option<String> {
    let mut out = String::new();
    let mut rest = raw.trim();
    while let Some(start) = rest.find("$(") {
        out.push_str(&sanitize_ident(&rest[..start]));
        let after = &rest[start + 2..];
        let end = after.find(')')?;
        let name = &after[..end];
        let cmake_name = match name {
            // The historic ARCH/AROS_TARGET_ARCH is the machine (pc, raspi),
            // which this build calls AROS_TARGET_PLATFORM.  Historic
            // AROS_TARGET_PLATFORM is instead the compound MetaMake selector
            // (pc-x86_64, raspi-arm, raspi-aarch64).
            "AROS_TARGET_ARCH" | "ARCH" => "AROS_TARGET_PLATFORM",
            "AROS_TARGET_PLATFORM" => "AROS_TARGET_LEGACY_PLATFORM",
            "AROS_TARGET_CPU" | "CPU" => "AROS_TARGET_CPU",
            "AROS_TARGET_FAMILY" | "FAMILY" => "AROS_TARGET_FAMILY",
            "AROS_TARGET_VARIANT" => "AROS_TARGET_VARIANT",
            "AROS_TARGET_ICONSET" => "AROS_TARGET_ICONSET",
            "AROS_TARGET_CPU32" => "AROS_TARGET_CPU32",
            _ => return None,
        };
        out.push_str("${");
        out.push_str(cmake_name);
        out.push('}');
        rest = &after[end + 1..];
    }
    out.push_str(&sanitize_ident(rest));
    (!out.is_empty()).then_some(out)
}

/// The subset of a genmodule config that decides the client archive.
struct GenmoduleConfigFacts {
    has_relative: bool,
    relative_libraries: Vec<String>,
    /// `options stubs` or `options autoinit` stated in the config. Either one
    /// puts a generated source into `<mod>_LINKLIBFILES` regardless of the
    /// module type, so either one makes the archive exist.
    forces_client_archive: bool,
}

/// The flag state one declaration sees.
///
/// With a target selected the flags are re-collected positionally from the
/// mmakefile's own scope, because Make evaluates a declaration's arguments
/// where the declaration stands. Link options contributed by an architecture
/// `make.opts` are not in that scope, so they are merged in here, once per
/// declaration, exactly as the make.opts defines are.
/// The driver-level link options one declaration states in a global variable.
///
/// A declaration that links for a different architecture than the rest of the
/// tree states it by assigning this global, not a `USER_*` variable, so the
/// flag collector never sees it. `arch/all-pc/bootstrap/mmakefile.src:32` is
/// the case: `--target=i386-pc-linux-gnu -march=i486`, without which the
/// 32-bit multiboot bootstrap is compiled and linked 64-bit.
fn declaration_global_link_options(
    name: &str,
    scope: &VarScope,
    dirs: &crate::dirs::DirVars,
    root: &Path,
    rel_dir: &Path,
    line: usize,
) -> Vec<String> {
    let Some(raw) = scope.raw_at(name, line) else {
        return Vec::new();
    };
    // Evaluated here rather than in the flag collector, which has no directory
    // and so cannot render the path a `-Wl,-T,` or `-Wl,-Map,` carries.
    let context = MakeExprContext::new(scope, dirs, line, root, rel_dir);
    let Ok(value) = evaluate_make_expr(&raw, &context) else {
        return Vec::new();
    };
    value
        .split_whitespace()
        .filter(|token| crate::flags::is_driver_link_option(token))
        .map(str::to_owned)
        .collect()
}

fn declaration_flags_at(
    scope: &VarScope,
    line: usize,
    target: Option<&TargetContext>,
    file_flags: &FlagSet,
    opts_link_options: &[String],
    opts_spec_switches: &[String],
) -> FlagSet {
    let mut flags = target.map_or_else(|| file_flags.clone(), |_| collect_flags_at(scope, line));
    for option in opts_link_options {
        if !flags.link_options.contains(option) {
            flags.link_options.push(option.clone());
        }
    }
    for switch in opts_spec_switches {
        if !flags.spec_switches.contains(switch) {
            flags.spec_switches.push(switch.clone());
        }
    }
    flags
}

fn apply_mesa_compile_contract(
    rel_dir: &Path,
    mmake: &str,
    target: Option<&TargetContext>,
    flags: &mut FlagSet,
    includes: &mut crate::includes::IncludeSet,
) -> std::result::Result<bool, String> {
    let Some(contract) = compile_contract(rel_dir, mmake, target)? else {
        return Ok(false);
    };
    flags.defines = contract.defines;
    flags.undefines = contract.undefines;
    flags.compile_options = contract.options;
    flags.link_options.clear();
    includes.dirs = contract.includes;
    includes.arch_modules.clear();
    Ok(true)
}

fn merge_named_link_flags(flags: &mut FlagSet, scope: &VarScope, line: usize, variable: &str) {
    let local = collect_named_link_flags_at(scope, line, variable);
    for option in local.link_options {
        if !flags.link_options.contains(&option) {
            flags.link_options.push(option);
        }
    }
    for switch in local.spec_switches {
        if !flags.spec_switches.contains(&switch) {
            flags.spec_switches.push(switch);
        }
    }
    for skipped in local.skipped {
        if !flags.skipped.contains(&skipped) {
            flags.skipped.push(skipped);
        }
    }
}

fn read_genmodule_linklib_config(directory: &Path, module: &str) -> Option<GenmoduleConfigFacts> {
    let content = fs::read_to_string(directory.join(format!("{module}.conf"))).ok()?;
    let mut in_config = false;
    let mut has_relative = false;
    let mut forces_client_archive = false;
    let mut relative_libraries = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        match trimmed {
            "##begin config" => {
                in_config = true;
                continue;
            }
            "##end config" => {
                in_config = false;
                continue;
            }
            _ => {}
        }
        if !in_config || trimmed.starts_with('#') {
            continue;
        }
        if let Some(options) = trimmed.strip_prefix("options ") {
            let mut stated = options.split([',', ' ', '\t']);
            // `options` may appear on several lines; each one contributes.
            for option in stated.by_ref() {
                match option {
                    "rellinklib" => has_relative = true,
                    "stubs" | "autoinit" => forces_client_archive = true,
                    _ => {}
                }
            }
        } else if let Some(library) = trimmed.strip_prefix("rellib ") {
            let library = library.split_whitespace().next().unwrap_or_default();
            if !library.is_empty() && !relative_libraries.iter().any(|value| value == library) {
                relative_libraries.push(library.to_owned());
            }
        }
    }
    Some(GenmoduleConfigFacts {
        has_relative,
        relative_libraries,
        forces_client_archive,
    })
}

/// Resolves a single output name through the bounded Make evaluator.
pub(crate) fn evaluate_name(
    raw: &str,
    context: &MakeExprContext<'_>,
) -> std::result::Result<String, String> {
    let expanded = evaluate_make_expr(raw, context).map_err(|error| error.to_string())?;
    let mut words = expanded.split_whitespace();
    let Some(name) = words.next() else {
        return Err("expression expanded to an empty name".to_owned());
    };
    if words.next().is_some() {
        return Err(format!(
            "expression expanded to more than one name: `{expanded}`"
        ));
    }
    let name = sanitize_ident(name);
    if name.is_empty() {
        return Err("expression expanded to an empty name".to_owned());
    }
    Ok(name)
}

fn evaluate_output_directory(
    args: &str,
    context: &MakeExprContext<'_>,
) -> std::result::Result<Option<String>, String> {
    let Some(raw) = macro_arg(args, "targetdir") else {
        return Ok(None);
    };
    let expanded = evaluate_make_expr(&raw, context)
        .map_err(|error| format!("targetdir={raw} cannot be evaluated: {error}"))?;
    let expanded = expanded.trim();
    if expanded.is_empty() {
        return Err(format!("targetdir={raw} expanded to an empty path"));
    }
    Ok(Some(expanded.to_owned()))
}

fn record_partial_source_lists(
    output: &mut Vec<String>,
    source_inventory_patterns: &mut Vec<String>,
    sources: &EvaluatedSources,
    relative_dir: &Path,
    invocation: &Invocation,
    mmake: &str,
) {
    for pattern in &sources.deferred_wildcards {
        if !source_inventory_patterns.contains(pattern) {
            source_inventory_patterns.push(pattern.clone());
        }
    }
    output.extend(sources.diagnostics.iter().map(|diagnostic| {
        format!(
            "{}:{}: %{} mmake={mmake} {diagnostic}",
            relative_dir.display(),
            invocation.line + 1,
            invocation.name
        )
    }));
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
pub(crate) fn join_mm_continuations(content: &str) -> String {
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
                .unwrap_or_else(|| body.trim_start());
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

/// One macro invocation from an mmakefile: its name, argument text, and the
/// line of the continuation-joined file it stands on.
///
/// The line is what makes positional variable lookup possible; see `VarScope`.
#[derive(Clone, Debug)]
pub(crate) struct Invocation {
    pub(crate) name: String,
    pub(crate) args: String,
    pub(crate) line: usize,
}

/// Joins Make continuation lines, so an assignment or a declaration occupies
/// exactly one line.
///
/// Nearly every declaration spreads its arguments over several lines and
/// `mmake=` is often not on the first, and a file list is nearly always written
/// one name per continued line. Joining first means one pass can both read the
/// assignments and see where each declaration stands.
#[must_use]
pub fn join_continuations(content: &str) -> String {
    CONTINUATION_RE.replace_all(content, " ").into_owned()
}

/// Concrete target values available while scanning Make conditionals.
///
/// Every field is optional on purpose.  An omitted value is not the same as an
/// empty Make variable: the former means that the CMake configuration did not
/// provide enough information to select a branch, while the latter can make an
/// `ifeq ($(VAR),)` condition decidable.  Library callers that do not supply a
/// context retain the conservative, target-agnostic parser behaviour.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TargetContext {
    pub cpu: Option<String>,
    pub platform: Option<String>,
    pub family: Option<String>,
    pub variant: Option<String>,
    pub toolchain: Option<String>,
    pub cpu32: Option<String>,
    pub use_mmu: Option<String>,
    pub float_abi: Option<String>,
}

impl TargetContext {
    /// The value of one target parameter, for callers outside this module that
    /// have to decide a Make conditional on it.
    #[must_use]
    pub fn value_of(&self, name: &str) -> Option<String> {
        self.value(name)
    }

    pub(crate) fn value(&self, name: &str) -> Option<String> {
        match name {
            "AROS_TARGET_CPU" | "CPU" => self.cpu.clone(),
            // Historic MetaMake calls the machine ARCH.  Its
            // AROS_TARGET_PLATFORM is instead the compound machine/CPU name.
            "AROS_TARGET_ARCH" | "ARCH" => self.platform.clone(),
            "AROS_TARGET_PLATFORM" => Some(format!(
                "{}-{}",
                self.platform.as_deref()?,
                self.cpu.as_deref()?
            )),
            "AROS_TARGET_FAMILY" | "FAMILY" => self.family.clone(),
            "AROS_TARGET_VARIANT" => self.variant.clone(),
            "AROS_TOOLCHAIN" => self.toolchain.clone(),
            "AROS_TARGET_CPU32" => self.cpu32.clone(),
            "USE_MMU" => self.use_mmu.clone(),
            "GCC_CONFIG_FLOAT_ABI" => self.float_abi.clone(),
            _ => None,
        }
    }
}

/// Splits continuation-joined mmakefile text into its macro invocations.
///
/// Takes text already run through `join_continuations`, and records each
/// invocation's line in that text, so a declaration's arguments can be resolved
/// against the variable state as of that point rather than the file's last word.
///
/// This replaces matching the whole file with one regex. With `(?s)` and a
/// non-greedy tail such as `(.*?)(?:%common|$)`, the first `%build_module` in a
/// file swallowed every later one, because most files carry a single `%common`
/// at the end. 14 files contributed one target each instead of all of theirs,
/// costing 60 targets, among them every Wanderer and Zune class.
pub(crate) fn macro_invocations(joined: &str) -> Vec<Invocation> {
    let mut out = Vec::new();
    for (line_no, line) in joined.lines().enumerate() {
        let t = line.trim_start();
        let Some(after) = t.strip_prefix('%') else {
            continue;
        };
        let (name, args) = after
            .find(char::is_whitespace)
            .map_or((after, ""), |i| (&after[..i], after[i..].trim()));
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        out.push(Invocation {
            name: name.to_owned(),
            args: args.to_owned(),
            line: line_no,
        });
    }
    out
}

pub(crate) fn is_concrete_build_invocation(name: &str) -> bool {
    matches!(
        name,
        "build_module"
            | "build_module_abi"
            | "build_module_library"
            | "build_prog"
            | "build_progs"
            | "build_linklib"
            | "build_module_simple"
            | "build_with_cmake"
            | "build_with_configure"
    )
}

pub(crate) fn select_target_invocations(
    joined: &str,
    line_states: Option<&[ConditionalTruth]>,
    relative_dir: &Path,
    skipped: &mut Vec<String>,
) -> Vec<Invocation> {
    macro_invocations(joined)
        .into_iter()
        .filter_map(|invocation| {
            if !is_concrete_build_invocation(&invocation.name) {
                return Some(invocation);
            }
            let Some(states) = line_states else {
                return Some(invocation);
            };
            match states
                .get(invocation.line)
                .copied()
                .unwrap_or(ConditionalTruth::Unknown)
            {
                ConditionalTruth::True => Some(invocation),
                ConditionalTruth::False => None,
                ConditionalTruth::Unknown => {
                    let mmake = macro_arg(&invocation.args, "mmake")
                        .map_or_else(String::new, |name| format!(" mmake={name}"));
                    skipped.push(format!(
                        "{}:{}: %{}{} is guarded by an unresolved Make conditional",
                        relative_dir.display(),
                        invocation.line + 1,
                        invocation.name,
                        mmake
                    ));
                    None
                }
            }
        })
        .collect()
}

/// Reads `key=value` or `key="value with spaces"` from an argument text.
///
/// The key must sit at a word boundary, or `files=` also matches the tail of
/// `linklibfiles=` and returns the wrong argument.
pub(crate) fn macro_arg(args: &str, key: &str) -> Option<String> {
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

/// Returns the top-level keyword names in one macro invocation.
///
/// Values may contain quoted whitespace or nested Make functions. Neither may
/// manufacture another macro argument: only an identifier beginning at a
/// top-level word boundary and followed immediately by `=` is retained.
pub(crate) fn macro_argument_names(args: &str) -> Vec<String> {
    let bytes = args.as_bytes();
    let mut names = Vec::new();
    let mut cursor = 0usize;
    let mut quote = None;
    let mut make_depth = 0usize;
    let mut word_boundary = true;

    while cursor < bytes.len() {
        let byte = bytes[cursor];
        if let Some(delimiter) = quote {
            if byte == delimiter {
                quote = None;
            }
            cursor += 1;
            continue;
        }
        if matches!(byte, b'\'' | b'"') {
            quote = Some(byte);
            word_boundary = false;
            cursor += 1;
            continue;
        }
        if byte == b'$' && bytes.get(cursor + 1) == Some(&b'(') {
            make_depth += 1;
            word_boundary = false;
            cursor += 2;
            continue;
        }
        if byte == b')' && make_depth > 0 {
            make_depth -= 1;
            cursor += 1;
            continue;
        }
        if make_depth == 0 && byte.is_ascii_whitespace() {
            word_boundary = true;
            cursor += 1;
            continue;
        }
        if make_depth == 0 && word_boundary && (byte.is_ascii_alphabetic() || byte == b'_') {
            let start = cursor;
            cursor += 1;
            while bytes
                .get(cursor)
                .is_some_and(|candidate| candidate.is_ascii_alphanumeric() || *candidate == b'_')
            {
                cursor += 1;
            }
            if bytes.get(cursor) == Some(&b'=') {
                names.push(args[start..cursor].to_owned());
            }
            word_boundary = false;
            continue;
        }
        word_boundary = false;
        cursor += 1;
    }
    names
}

/// Parses a single `mmakefile.src` into target definitions and meta rules.
///
/// # Errors
///
/// Returns an error if the file cannot be read.
#[allow(clippy::missing_panics_doc)]
pub fn parse_mmakefile(path: &Path, root: &Path) -> Result<ParsedMmakefile> {
    let dirs = crate::dirs::DirVars::load(root);
    parse_mmakefile_with_dirs(path, root, &dirs)
}

/// Parses one mmakefile for a concrete target configuration.
///
/// Unlike [`parse_mmakefile`], this form may select `ifeq`/`ifneq` branches
/// whose operands are completely known from `target`. Unknown target settings
/// remain unsafe and are reported rather than inferred.
///
/// # Errors
///
/// Returns an error if the file cannot be read.
#[allow(clippy::missing_panics_doc)]
pub fn parse_mmakefile_with_context(
    path: &Path,
    root: &Path,
    target: &TargetContext,
) -> Result<ParsedMmakefile> {
    let dirs = crate::dirs::DirVars::load(root);
    parse_mmakefile_with_dirs_and_context(path, root, &dirs, target)
}

/// Parses one mmakefile with the shared directory-variable table.
///
/// The command-line scanner calls this form so `config/make.cfg.in` is read
/// once for the whole tree rather than once per mmakefile. The two-argument
/// wrapper remains convenient for focused tests and library callers.
///
/// # Errors
///
/// Returns an error if the file cannot be read.
#[allow(clippy::missing_panics_doc)]
pub fn parse_mmakefile_with_dirs(
    path: &Path,
    root: &Path,
    dirs: &crate::dirs::DirVars,
) -> Result<ParsedMmakefile> {
    parse_mmakefile_impl(path, root, dirs, None, &[])
}

/// Parses one mmakefile for a concrete target using a shared directory table.
///
/// # Errors
///
/// Returns an error if the file cannot be read.
#[allow(clippy::missing_panics_doc)]
pub fn parse_mmakefile_with_dirs_and_context(
    path: &Path,
    root: &Path,
    dirs: &crate::dirs::DirVars,
    target: &TargetContext,
) -> Result<ParsedMmakefile> {
    parse_mmakefile_impl(path, root, dirs, Some(target), &[])
}

/// Parses one mmakefile with a tree-wide inventory of proven `%fetch`
/// declarations available for declaration-local input ownership checks.
///
/// The inventory does not add fetches to this file's result. It only lets a
/// safe local variable fragment prove that a source or include path belongs to
/// a fetch declared centrally elsewhere in the tree.
///
/// # Errors
///
/// Returns an error if the file cannot be read.
#[allow(clippy::missing_panics_doc)]
pub fn parse_mmakefile_with_dirs_and_context_and_fetches(
    path: &Path,
    root: &Path,
    dirs: &crate::dirs::DirVars,
    target: &TargetContext,
    known_fetches: &[FetchDecl],
) -> Result<ParsedMmakefile> {
    parse_mmakefile_impl(path, root, dirs, Some(target), known_fetches)
}

/// Collects the target-selected `%fetch` declarations of one mmakefile.
///
/// This cheap first pass supplies the tree-wide ownership inventory required
/// by centrally declared ports without parsing the file's build targets.
///
/// # Errors
///
/// Returns an error if the file cannot be read.
#[allow(clippy::missing_panics_doc)]
pub fn collect_mmakefile_fetches_with_context(
    path: &Path,
    root: &Path,
    target: &TargetContext,
) -> Result<Vec<FetchDecl>> {
    let content = read_source(path)?;
    let parent_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let rel_dir = parent_dir
        .strip_prefix(root)
        .unwrap_or(parent_dir)
        .to_path_buf();
    let mut visited = HashSet::new();
    let collector_content =
        inline_collector_make_includes(&content, root, &rel_dir, &mut visited, 8);
    let collector_joined = join_continuations(&collector_content);
    let collector_input = format!(
        "{}{}",
        collector_forward_local_prelude(&collector_joined),
        collector_joined
    );
    let scope = collect_vars_impl(&collector_input, Some(target)).0;
    Ok(collect_fetches_with_scope(&content, &rel_dir, &scope).0)
}

fn capability_diagnostic(
    relative_path: &Path,
    line: Option<usize>,
    message: impl Into<String>,
) -> Diagnostic {
    let location = line.map_or_else(
        || SourceLocation::new(relative_path.display().to_string()),
        |line| SourceLocation::new(relative_path.display().to_string()).at(line, None),
    );
    Diagnostic::error(
        DiagnosticCode::CapabilityDrift,
        DiagnosticStage::CapabilityValidation,
        message,
    )
    .with_location(location)
    .with_hint(
        "refusing a partial translation; review the upstream change and update the transpiler capability",
    )
}

fn expected_grub_profile_exclusion(target: Option<&TargetContext>) -> bool {
    target.is_some_and(|target| {
        matches!(
            (
                target.cpu.as_deref(),
                target.platform.as_deref(),
                target.toolchain.as_deref(),
                target.cpu32.as_deref(),
                target.use_mmu.as_deref(),
                target.float_abi.as_deref(),
            ),
            (
                Some("arm"),
                Some("raspi"),
                Some("llvm"),
                Some(""),
                Some("1"),
                Some("hard")
            ) | (
                Some("aarch64"),
                Some("raspi"),
                Some("llvm"),
                Some(""),
                Some("1"),
                Some("")
            )
        )
    })
}

fn parse_mmakefile_impl(
    path: &Path,
    root: &Path,
    dirs: &crate::dirs::DirVars,
    target: Option<&TargetContext>,
    known_fetches: &[FetchDecl],
) -> Result<ParsedMmakefile> {
    let content = read_source(path)?;
    let parent_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let rel_dir = parent_dir
        .strip_prefix(root)
        .unwrap_or(parent_dir)
        .to_path_buf();

    let relative_path = path.strip_prefix(root).unwrap_or(path).to_path_buf();

    // Fetch recipes expand after the complete file has been read. Their
    // collector owns the existing bounded include traversal and supplies the
    // ownership proof used by the declaration-specific port scope below.
    let mut collector_visited = HashSet::new();
    let collector_content =
        inline_collector_make_includes(&content, root, &rel_dir, &mut collector_visited, 8);
    let collector_joined = join_continuations(&collector_content);
    let collector_input = format!(
        "{}{}",
        collector_forward_local_prelude(&collector_joined),
        collector_joined
    );
    let collector_scope = target.map_or_else(
        || collect_vars(&collector_input),
        |target| collect_vars_impl(&collector_input, Some(target)).0,
    );
    let (fetches, skipped_fetches) =
        collect_fetches_with_scope(&content, &rel_dir, &collector_scope);
    let mut ownership_fetches = known_fetches.to_vec();
    ownership_fetches.extend(fetches.iter().cloned());

    // A small number of declarations keep a plain source inventory in a
    // sibling Make fragment. This remains the global default. A broader safe
    // variable scope is considered separately and adopted only when every
    // declaration is proven to compile sources owned by one of the fetches
    // above; there is deliberately no broad fallback.
    let plain_local_make_scan = inline_local_make_includes(
        &content,
        root,
        &relative_path,
        LocalMakeIncludeLimits::default(),
        LocalMakeFragmentPolicy::PlainSourceLists,
    );
    let port_scope_candidate = inline_local_make_includes(
        &content,
        root,
        &relative_path,
        LocalMakeIncludeLimits::default(),
        LocalMakeFragmentPolicy::SafeVariableScopes,
    );
    let port_scope_adopted = declaration_owned_port_scope(
        &plain_local_make_scan,
        &port_scope_candidate,
        target,
        dirs,
        root,
        &rel_dir,
        &ownership_fetches,
    );
    let define_scope_candidate = inline_local_make_includes(
        &content,
        root,
        &relative_path,
        LocalMakeIncludeLimits::default(),
        LocalMakeFragmentPolicy::LiteralDefineHeader,
    );
    let define_headers = (!port_scope_adopted)
        .then(|| {
            literal_defines::owned_scope(
                &plain_local_make_scan,
                &define_scope_candidate,
                target,
                dirs,
                root,
                &relative_path,
                &rel_dir,
                &content,
            )
        })
        .flatten()
        .unwrap_or_default();
    let define_scope_adopted = !define_headers.is_empty();
    let local_make_scan = if port_scope_adopted {
        port_scope_candidate
    } else if define_scope_adopted {
        define_scope_candidate
    } else {
        plain_local_make_scan
    };
    let skipped_local_make_includes = local_make_scan
        .issues
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();

    // Make evaluates ordinary build-macro arguments at their declaration
    // line, while `%fetch` recipes retain references until recipe execution
    // after the complete file has been read. Both use the same selected
    // conditional scope but deliberately query it at different positions.
    let joined = join_continuations(&local_make_scan.expanded);
    let (scope, conditional_line_states) = target.map_or_else(
        || (collect_vars(&joined), None),
        |target| {
            let (scope, states) = if port_scope_adopted {
                collect_vars_impl_with_forward_locals(&joined, Some(target), true)
            } else {
                collect_vars_impl(&joined, Some(target))
            };
            (scope, Some(states))
        },
    );
    let mut targets = Vec::new();
    let mut meta_rules = Vec::new();
    let mut skipped_meta_rules = Vec::new();

    // Include paths are a file-level property in Make: USER_INCLUDES applies to
    // every rule in the mmakefile, so the same set is attached to each target
    // parsed out of this file.
    let include_set = collect_includes(&content, &rel_dir);
    let arch_decls = collect_arch_decls(&content, &rel_dir);
    let mut copy_scan = collect_copy_includes_with_scope(&content, &rel_dir, &collector_scope);
    // USER_CPPFLAGS / USER_CFLAGS apply to every rule in the mmakefile, so the
    // same set is attached to each target parsed out of it.
    let mut flag_set = collect_flags(&content);
    let (packages, skipped_packages) = crate::packages::collect_packages(&content, &rel_dir);
    // Collected from `joined`, not from `content`: the declaration line has to
    // be in the same coordinate system as `scope`, which is built from the
    // joined and locally-included text. Read against the raw file the line
    // numbers drift with every continuation and every inlined fragment, so the
    // positional flag lookup below would read some other declaration's flags.
    let (mut arch_sources, skipped_arch_sources) = collect_arch_sources(&joined, &rel_dir, target);
    // A %build_archspecific file contributes to a target defined elsewhere, so
    // its own USER_INCLUDES and flags have to travel with the declaration.
    //
    // Read at the declaration's own line, not file-wide. One mmakefile can hold
    // several declarations with different flags:
    // arch/i386-all/hidd/gfx sets `USER_CFLAGS :=` before the baseline lane,
    // `$(HIDDGFX_SSE_CFLAGS)` before the SSE lane and `$(HIDDGFX_AVX_CFLAGS)`
    // before the AVX one. The file-wide value is whichever assignment happens to
    // win, and with it rgbconv_avx.c cannot compile at all.
    for d in &mut arch_sources {
        let at = collect_includes_at(&joined, &scope, d.line, &rel_dir);
        let flags = collect_flags_at(&scope, d.line);
        d.include_dirs = at.dirs;
        d.defines = flags.defines;
        d.compile_options = flags.compile_options;
    }
    // Architecture option files. Their contents are tagged with the
    // architecture they belong to, so CMake can keep the ones that apply; the
    // transpiler itself stays target-agnostic.
    let (opts_files, mut skipped_make_opts) = collect_make_opts(&content, &rel_dir, root);
    let active_tags = crate::make_opts::active_arch_tags(
        target.and_then(|target| target.platform.as_deref()),
        target.and_then(|target| target.cpu.as_deref()),
    );
    let mut undecidable_arch_link_options: Vec<String> = Vec::new();
    // Merged into every declaration's flags below rather than into `flag_set`:
    // with a target selected, declaration flags are re-collected positionally
    // from the mmakefile's own scope (collect_flags_at), so anything added to
    // `flag_set` here would be discarded. This is the same arrangement the
    // make.opts defines already use.
    let mut opts_link_options: Vec<String> = Vec::new();
    let mut opts_spec_switches: Vec<String> = Vec::new();
    let skipped_conditions = flag_set.skipped_conditions.clone();
    // Flags guarded by an `ifeq` on the CPU or platform are already tagged by
    // the flag collector; the make.opts contents are appended below.
    let mut arch_defines: Vec<(String, String)> = flag_set.arch_defines.clone();
    let mut arch_compile_options: Vec<(String, String)> = flag_set.arch_compile_options.clone();
    let mut opts_include_dirs: Vec<String> = Vec::new();
    let mut opts_arch_includes: Vec<(String, String)> = Vec::new();
    for f in &opts_files {
        let Ok(body) = read_source(&root.join(&f.path)) else {
            continue;
        };
        let opts_flags = collect_flags(&body);
        // Include paths from an option file are resolved against the including
        // mmakefile's directory, which is what Make does.
        let opts_incs = collect_includes(&body, &rel_dir);
        if let Some(tag) = &f.tag {
            for d in opts_flags.defines {
                arch_defines.push((tag.clone(), d));
            }
            for o in opts_flags.compile_options {
                arch_compile_options.push((tag.clone(), o));
            }
            for d in opts_incs.dirs {
                opts_arch_includes.push((tag.clone(), d));
            }
            // USER_LDFLAGS in a make.opts was read and then dropped, with
            // no report. arch/all-pc/kernel/make.opts:1 is
            //
            //   USER_LDFLAGS := -L$(GENDIR)/lib -lbootconsole -lacpica
            //
            // and without it kernel.resource leaves con_Putc, scr_Width,
            // the whole boot console and every Acpi* undefined. The
            // bootstrap loader forgives exactly one undefined symbol,
            // SysBase (bootstrap/elfloader.c:157), so that image cannot
            // load.
            //
            // Folded in rather than carried as a tagged lane, because the
            // graph has to see the `-L` to authorise a private archive:
            // libbootconsole.a lives in $(GENDIR)/lib, and
            // has_matching_private_link_archive compares that directory
            // against the consumer's own link options.
            if opts_flags.link_options.is_empty() {
            } else if active_tags.iter().any(|active| active == tag) {
                opts_link_options.extend(opts_flags.link_options);
                opts_spec_switches.extend(opts_flags.spec_switches);
            } else if target.is_none() {
                undecidable_arch_link_options.push(format!(
                    "{}: link options tagged {tag} cannot be decided without a target: {}",
                    f.path,
                    opts_flags.link_options.join(" ")
                ));
            }
        } else {
            // A local make.opts always applies.
            flag_set.defines.extend(opts_flags.defines);
            flag_set.compile_options.extend(opts_flags.compile_options);
            opts_link_options.extend(opts_flags.link_options);
            opts_spec_switches.extend(opts_flags.spec_switches);
            opts_include_dirs.extend(opts_incs.dirs);
        }
    }

    // Make evaluates a declaration's arguments where the declaration stands, so
    // the variable state is positional. Both scans read the same
    // continuation-joined text, which is what makes their line numbers
    // comparable.
    skipped_make_opts.extend(undecidable_arch_link_options);

    let icon_scan = crate::icons::collect_icons_all(&joined, dirs, &rel_dir);
    let catalog_scan = crate::catalogs::collect_catalogs_with_line_states(
        &joined,
        &scope,
        dirs,
        root,
        &rel_dir,
        conditional_line_states.as_deref(),
    );
    let mut skipped_programs: Vec<String> = Vec::new();
    let mut capability_errors: Vec<Diagnostic> = Vec::new();
    let invocations = select_target_invocations(
        &joined,
        conditional_line_states.as_deref(),
        &rel_dir,
        &mut skipped_programs,
    );
    // `%copy_dir_recursive` owns filesystem output, so unlike a generic
    // auxiliary macro it must not survive an inactive or unknown conditional.
    // Non-profiled parser callers still get a line-state scan: only their
    // unconditional declarations are safe to materialise.
    let fallback_copy_directory_states = conditional_line_states
        .is_none()
        .then(|| collect_vars_impl(&joined, None).1);
    let copy_directory_line_states = conditional_line_states
        .as_deref()
        .or(fallback_copy_directory_states.as_deref());
    let (copy_directories, skipped_copy_directories) = copy_directories::collect(
        &invocations,
        &scope,
        dirs,
        root,
        &rel_dir,
        copy_directory_line_states,
    );
    let mut external_cmake = Vec::new();
    for invocation in invocations
        .iter()
        .filter(|invocation| invocation.name == "build_with_cmake")
    {
        let expression_context =
            MakeExprContext::new(&scope, dirs, invocation.line, root, &rel_dir);
        match external_cmake::parse(
            invocation,
            &expression_context,
            &rel_dir,
            &fetches,
            target,
            &content,
        ) {
            Ok(declaration) => external_cmake.push(declaration),
            Err(reason) => {
                let mmake = macro_arg(&invocation.args, "mmake")
                    .map_or_else(String::new, |name| format!(" mmake={name}"));
                if matches!(
                    rel_dir.to_str(),
                    Some("compiler/cunit" | "workbench/classes/datatypes/heic")
                ) {
                    capability_errors.push(capability_diagnostic(
                        &relative_path,
                        Some(invocation.line + 1),
                        format!("%build_with_cmake{mmake} no longer matches its closed capability: {reason}"),
                    ));
                }
                skipped_programs.push(format!(
                    "{}:{}: %build_with_cmake{mmake} skipped: {reason}",
                    rel_dir.display(),
                    invocation.line + 1
                ));
            }
        }
    }
    let mut configure_builds = Vec::new();
    let mut grub_builds = Vec::new();
    let mut ahi_builds = Vec::new();
    for invocation in invocations
        .iter()
        .filter(|invocation| invocation.name == "build_with_configure")
    {
        match crate::capability::ahi::parse(root, invocation, &rel_dir, target) {
            Ok(Some(declaration)) => ahi_builds.push(declaration),
            Ok(None) => match crate::capability::grub2::parse(root, invocation, &rel_dir, target) {
                Ok(Some(declaration)) => grub_builds.push(declaration),
                Ok(None) => {
                    match crate::capability::configure::parse(root, invocation, &rel_dir, target) {
                        Ok(declaration) => configure_builds.push(declaration),
                        Err(reason) => {
                            let mmake = macro_arg(&invocation.args, "mmake")
                                .map_or_else(String::new, |name| format!(" mmake={name}"));
                            if matches!(
                                rel_dir.to_str(),
                                Some(
                                    "tools/ADFlib"
                                        | "workbench/network/WirelessManager/wpa_supplicant"
                                )
                            ) {
                                capability_errors.push(capability_diagnostic(
                                    &relative_path,
                                    Some(invocation.line + 1),
                                    format!("%build_with_configure{mmake} no longer matches its closed capability: {reason}"),
                                ));
                            }
                            skipped_programs.push(format!(
                                "{}:{}: %build_with_configure{mmake} skipped: {reason}",
                                rel_dir.display(),
                                invocation.line + 1
                            ));
                        }
                    }
                }
                Err(reason) => {
                    let mmake = macro_arg(&invocation.args, "mmake")
                        .map_or_else(String::new, |name| format!(" mmake={name}"));
                    if !expected_grub_profile_exclusion(target) {
                        capability_errors.push(capability_diagnostic(
                            &relative_path,
                            Some(invocation.line + 1),
                            format!("%build_with_configure{mmake} no longer matches the closed GRUB2 capability: {reason}"),
                        ));
                    }
                    skipped_programs.push(format!(
                        "{}:{}: %build_with_configure{mmake} skipped: {reason}",
                        rel_dir.display(),
                        invocation.line + 1
                    ));
                }
            },
            Err(reason) => {
                let mmake = macro_arg(&invocation.args, "mmake")
                    .map_or_else(String::new, |name| format!(" mmake={name}"));
                capability_errors.push(capability_diagnostic(
                    &relative_path,
                    Some(invocation.line + 1),
                    format!("%build_with_configure{mmake} no longer matches the closed AHI capability: {reason}"),
                ));
                skipped_programs.push(format!(
                    "{}:{}: %build_with_configure{mmake} skipped: {reason}",
                    rel_dir.display(),
                    invocation.line + 1
                ));
            }
        }
    }
    let mut partial_source_lists: Vec<String> = Vec::new();
    let mut source_inventory_patterns: Vec<String> = Vec::new();
    let mut skipped_client_archives: Vec<String> = Vec::new();
    let mut unresolved_output_paths: Vec<String> = Vec::new();
    let re_libs = Regex::new(r#"uselibs=(?:"([^"]+)"|([^\s\\]+))"#).unwrap();

    // 1. Extract module definitions
    for inv in invocations.iter().filter(|i| {
        matches!(
            i.name.as_str(),
            "build_module" | "build_module_abi" | "build_module_library"
        )
    }) {
        // The three spellings wrap the same %build_module_core, but the ABI
        // form deliberately has no runtime compilation (make.tmpl:2828).
        let Some(mmake_raw) = macro_arg(&inv.args, "mmake") else {
            continue;
        };
        let Some(mod_raw) = macro_arg(&inv.args, "modname") else {
            continue;
        };
        let vars = scope.snapshot(inv.line);
        let expression_context = MakeExprContext::new(&scope, dirs, inv.line, root, &rel_dir);
        let isa_link_options = declaration_global_link_options(
            "TARGET_ISA_LDFLAGS",
            &scope,
            dirs,
            root,
            &rel_dir,
            inv.line,
        );
        let driver_link_options =
            declaration_global_link_options("USER_LDFLAGS", &scope, dirs, root, &rel_dir, inv.line);
        let mut declaration_flags = declaration_flags_at(
            &scope,
            inv.line,
            target,
            &flag_set,
            &opts_link_options,
            &opts_spec_switches,
        );
        let mut declaration_includes = target.map_or_else(
            || include_set.clone(),
            |_| collect_includes_at(&joined, &scope, inv.line, &rel_dir),
        );
        let mmake_name = sanitize_ident(&mmake_raw);
        if let Err(reason) = apply_mesa_compile_contract(
            &rel_dir,
            &mmake_name,
            target,
            &mut declaration_flags,
            &mut declaration_includes,
        ) {
            capability_errors.push(capability_diagnostic(
                &relative_path,
                Some(inv.line + 1),
                format!(
                    "%{} mmake={mmake_raw} no longer matches the Mesa compile capability: {reason}",
                    inv.name
                ),
            ));
            skipped_programs.push(format!(
                "{}:{}: %{} mmake={mmake_raw} Mesa 20.0.8 compile contract skipped: {reason}",
                rel_dir.display(),
                inv.line + 1,
                inv.name
            ));
            continue;
        }
        let mod_name = sanitize_ident(&mod_raw);
        let mod_type_owned = macro_arg(&inv.args, "modtype").unwrap_or_default();
        let mod_type_str = mod_type_owned.as_str();
        let rest = inv.args.as_str();
        let is_abi = inv.name == "build_module_abi";

        let module_type = if is_abi {
            ModuleType::Abi
        } else {
            match mod_type_str {
                "library" => ModuleType::Library,
                "device" => ModuleType::Device,
                "resource" => ModuleType::Resource,
                "hidd" => ModuleType::Hidd,
                "datatype" => ModuleType::Datatype,
                "gadget" => ModuleType::Gadget,
                "mcc" => ModuleType::Mcc,
                _ => ModuleType::Custom,
            }
        };
        let genmodule_only = is_explicit_genmodule_only(&inv.name, rest, mod_type_str);
        let linklib_name = match macro_arg(rest, "linklibname") {
            Some(raw) if !raw.is_empty() => match evaluate_name(&raw, &expression_context) {
                Ok(name) => Some(name),
                Err(reason) => {
                    skipped_programs.push(format!(
                        "{}:{}: %{} mmake={mmake_raw} linklibname={raw} is unresolved: {reason}",
                        rel_dir.display(),
                        inv.line + 1,
                        inv.name
                    ));
                    continue;
                }
            },
            _ => None,
        };

        let arch_specific = match resolve_yes_argument(rest, "archspecific", &scope, dirs, inv.line)
        {
            Ok(value) => value,
            Err(reason) => {
                skipped_programs.push(format!(
                    "{}:{}: %{} mmake={mmake_raw} {reason}",
                    rel_dir.display(),
                    inv.line + 1,
                    inv.name
                ));
                continue;
            }
        };
        let always_cxx_link =
            match resolve_yes_argument(rest, "alwayscxxlink", &scope, dirs, inv.line) {
                Ok(value) => value,
                Err(reason) => {
                    skipped_programs.push(format!(
                        "{}:{}: %{} mmake={mmake_raw} {reason}",
                        rel_dir.display(),
                        inv.line + 1,
                        inv.name
                    ));
                    continue;
                }
            };
        let target_dir = match resolve_module_target_dir(
            rest,
            &scope,
            dirs,
            inv.line,
            mod_type_str,
            true,
            arch_specific,
        ) {
            Ok(value) => value,
            Err(reason) => {
                skipped_programs.push(format!(
                    "{}:{}: %{} mmake={mmake_raw} {reason}",
                    rel_dir.display(),
                    inv.line + 1,
                    inv.name
                ));
                continue;
            }
        };
        let mod_suffix = match resolve_module_suffix(rest, &scope, dirs, inv.line, mod_type_str) {
            Ok(value) => value,
            Err(reason) => {
                skipped_programs.push(format!(
                    "{}:{}: %{} mmake={mmake_raw} {reason}",
                    rel_dir.display(),
                    inv.line + 1,
                    inv.name
                ));
                continue;
            }
        };

        // An ABI skeleton has no implementation sources, and the one explicit
        // genmodule-only library is implemented entirely by generated start/end
        // files. Every other empty result keeps the existing strict source-list
        // handling: unresolved lists may never turn into generated-only modules.
        let sources = if is_abi || genmodule_only {
            EvaluatedSources::default()
        } else {
            // The same source-list rules as every other build macro: the union
            // of all four lanes, with the reference's *.c default only when no
            // lane was declared (make.tmpl:2802).
            let mut sources = match evaluate_macro_sources(rest, &vars, &expression_context) {
                Ok(sources) => sources,
                Err(reason) => {
                    skipped_programs.push(format!(
                        "{}:{}: %{} mmake={mmake_raw} modname={mod_raw} {reason}",
                        rel_dir.display(),
                        inv.line + 1,
                        inv.name
                    ));
                    continue;
                }
            };
            record_partial_source_lists(
                &mut partial_source_lists,
                &mut source_inventory_patterns,
                &sources,
                &rel_dir,
                inv,
                &mmake_raw,
            );
            if sources.is_empty() {
                if sources.declared {
                    skipped_programs.push(format!(
                        "{}: %{} mmake={mmake_raw} modname={mod_raw} has an unresolved file list",
                        rel_dir.display(),
                        inv.name
                    ));
                    continue;
                }
                sources.c = wildcard_c_sources(parent_dir);
                if sources.is_empty() {
                    skipped_programs.push(format!(
                        "{}: %{} mmake={mmake_raw} modname={mod_raw} declares no sources",
                        rel_dir.display(),
                        inv.name
                    ));
                    continue;
                }
            }
            sources
        };

        let use_libs: Vec<String> = re_libs.captures(rest).map_or_else(Vec::new, |lcap| {
            let libs_str = lcap
                .get(1)
                .or_else(|| lcap.get(2))
                .map_or("", |m| m.as_str());
            expand_file_list(libs_str, &vars)
        });
        let declared_mod_type = matches!(module_type, ModuleType::Abi | ModuleType::Custom)
            .then(|| mod_type_owned.clone());

        // `conffile=` names the genmodule config, and 81 of the 83 declarations
        // that state one give a file whose stem is not modname:
        // con_handler.conf for modname=con, VMM_Handler.conf for modname=VMM.
        // Without carrying it, CMake derives `<modname>.conf`, finds nothing and
        // generates no scaffolding at all -- silently, because a module with no
        // config is a legitimate hand-written one.
        let config_file = macro_arg(&inv.args, "conffile").and_then(|raw| {
            let raw = raw.trim().trim_matches('"');
            match evaluate_make_expr(raw, &expression_context) {
                Ok(value) => {
                    let value = value.trim().trim_matches('"').to_owned();
                    if value.is_empty() || value.contains(char::is_whitespace) {
                        skipped_programs.push(format!(
                            "{}: %{} mmake={mmake_raw} conffile={raw} is not one path",
                            rel_dir.display(),
                            inv.name
                        ));
                        None
                    } else if value.starts_with("${") || value.starts_with('/') {
                        Some(value)
                    } else {
                        // Relative to the declaring directory, as Make reads it.
                        Some(format!(
                            "${{CMAKE_SOURCE_DIR}}/{}/{value}",
                            rel_dir.display()
                        ))
                    }
                }
                Err(error) => {
                    skipped_programs.push(format!(
                        "{}: %{} mmake={mmake_raw} conffile={raw} cannot be \
                         evaluated: {error}",
                        rel_dir.display(),
                        inv.name
                    ));
                    None
                }
            }
        });

        // Upstream creates the client archive when `<mod>_LINKLIB` is
        // non-empty, and make.tmpl derives that from the file set, not from
        // `linklibname=`:
        //
        //   config/make.tmpl:2270  _LINKLIB is empty exactly when
        //                          _LINKLIBFILES, _LINKLIBAFILES,
        //                          linklibfiles= and _ARCHNLIBFILES are all
        //                          empty; linklibname= only renames it
        //   tools/genmodule/writemakefile.c:78
        //                          _LINKLIBFILES gets <mod>_getlibbase for
        //                          every LIBRARY, <mod>_autoinit under
        //                          OPTION_AUTOINIT and the stubs under
        //                          OPTION_STUBS
        //   tools/genmodule/config.c:797
        //                          a LIBRARY defaults to OPTION_AUTOINIT,
        //                          every other module type to NOAUTOINIT
        //
        // So every modtype=library module has a client archive, and so does
        // any other module whose config states `options stubs` or
        // `options autoinit` (rom/timer is the one such case in the tree).
        // Keying it on linklibname= left 100 library archives unbuilt, which
        // is what the symbol audit sees as undefined DOSBase, UtilityBase and
        // the rest: the base is defined by AROS_LIBSET in <mod>_autoinit.c
        // (compiler/include/aros/symbolsets.h:118), and that object lives in
        // exactly this archive.
        if module_type != ModuleType::Library {
            if let Some(facts) = read_genmodule_linklib_config(parent_dir, &mod_name) {
                if facts.forces_client_archive {
                    skipped_client_archives.push(format!(
                        "{}:{}: %{} mmake={mmake_raw} modname={mod_raw} modtype={mod_type_owned}: \
                         config states `options stubs` or `options autoinit`, so upstream builds \
                         lib{mod_name}.a; the generated client sources are only derived for \
                         modtype=library",
                        rel_dir.display(),
                        inv.line + 1,
                        inv.name
                    ));
                }
            }
        }
        let genmodule_linklibs = if module_type == ModuleType::Library {
            read_genmodule_linklib_config(parent_dir, &mod_name).map(
                |GenmoduleConfigFacts {
                     has_relative,
                     relative_libraries,
                     forces_client_archive,
                 }| {
                    let mut inputs_exact = true;
                    let source_files = match evaluate_linklib_list(
                        rest,
                        "linklibfiles",
                        &vars,
                        &expression_context,
                    ) {
                        Ok(files) => files,
                        Err(error) => {
                            partial_source_lists.push(format!(
                                "{}:{}: %{} mmake={mmake_raw} {error}",
                                rel_dir.display(),
                                inv.line + 1,
                                inv.name
                            ));
                            inputs_exact = false;
                            Vec::new()
                        }
                    };
                    let object_sources = match evaluate_linklib_list(
                        rest,
                        "linklibobjs",
                        &vars,
                        &expression_context,
                    ) {
                        Ok(objects) => match map_linklib_object_sources(&objects, &sources.c) {
                            Ok(mapped) => mapped,
                            Err(error) => {
                                partial_source_lists.push(format!(
                                    "{}:{}: %{} mmake={mmake_raw} {error}",
                                    rel_dir.display(),
                                    inv.line + 1,
                                    inv.name
                                ));
                                inputs_exact = false;
                                Vec::new()
                            }
                        },
                        Err(error) => {
                            partial_source_lists.push(format!(
                                "{}:{}: %{} mmake={mmake_raw} {error}",
                                rel_dir.display(),
                                inv.line + 1,
                                inv.name
                            ));
                            inputs_exact = false;
                            Vec::new()
                        }
                    };
                    GenmoduleLinklibs {
                        enabled: linklib_name.is_some()
                            || forces_client_archive
                            || module_type == ModuleType::Library
                            || !source_files.is_empty()
                            || !object_sources.is_empty(),
                        has_relative,
                        relative_libraries,
                        source_files,
                        object_sources,
                        inputs_exact,
                    }
                },
            )
        } else {
            None
        };

        // All three %build_module* forms expand the implicit MetaMake
        // aliases and architecture endpoints.  `genmodule_only` describes
        // only how sources are materialised; using it as a guard here made
        // ordinary sourceful modules lose their upstream prerequisite graph.
        let include_set = match macro_arg(rest, "include_set") {
            Some(raw) => {
                let Some(rendered) = render_meta_token(&raw) else {
                    skipped_programs.push(format!(
                        "{}:{}: %{} mmake={mmake_raw} include_set={raw} contains an unmapped Make variable",
                        rel_dir.display(),
                        inv.line + 1,
                        inv.name
                    ));
                    continue;
                };
                rendered
            }
            None => "includes-all".to_owned(),
        };
        meta_rules.extend(implicit_module_meta_rules(
            &mmake_name,
            &mod_name,
            &include_set,
            &use_libs,
            inv.name != "build_module_library",
            inv.name != "build_module_abi",
            is_abi || genmodule_only,
        ));

        targets.push(TargetDefinition {
            mmake_name,
            target_name: mod_name,
            module_type,
            genmodule_only,
            empty_archive: false,
            source_files: sources.c,
            cxx_source_files: sources.cxx,
            always_cxx_link,
            objc_source_files: sources.objc,
            asm_source_files: sources.asm,
            use_libs,
            dependencies: Vec::new(),
            dir_path: rel_dir.clone(),
            target_dir,
            link_libs: Vec::new(),
            variant_32bit: false,
            declared_mod_type,
            mod_suffix,
            linklib_name,
            config_file,
            genmodule_linklibs,
            canonical_linklib_output: false,
            canonical_linklib_eligible: false,
            linklib_output_dir: None,
            compiler_flags: Vec::new(),
            include_dirs: {
                let mut d = declaration_includes.dirs.clone();
                d.extend(opts_include_dirs.iter().cloned());
                d
            },
            arch_modules: declaration_includes.arch_modules.clone(),
            arch_includes: opts_arch_includes.clone(),
            defines: declaration_flags.defines,
            undefines: declaration_flags.undefines,
            compile_options: declaration_flags.compile_options,
            link_options: declaration_flags.link_options,
            spec_switches: declaration_flags.spec_switches.clone(),
            driver_link_options: driver_link_options.clone(),
            isa_link_options: isa_link_options.clone(),
            arch_sources: Vec::new(),
            arch_defines: arch_defines.clone(),
            arch_compile_options: arch_compile_options.clone(),
            arch_source_options: Vec::new(),
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
        let vars = scope.snapshot(inv.line);
        let expression_context = MakeExprContext::new(&scope, dirs, inv.line, root, &rel_dir);
        let isa_link_options = declaration_global_link_options(
            "TARGET_ISA_LDFLAGS",
            &scope,
            dirs,
            root,
            &rel_dir,
            inv.line,
        );
        let driver_link_options =
            declaration_global_link_options("USER_LDFLAGS", &scope, dirs, root, &rel_dir, inv.line);
        let declaration_flags = declaration_flags_at(
            &scope,
            inv.line,
            target,
            &flag_set,
            &opts_link_options,
            &opts_spec_switches,
        );
        let declaration_includes = target.map_or_else(
            || include_set.clone(),
            |_| collect_includes_at(&joined, &scope, inv.line, &rel_dir),
        );
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
        let prog_name = match evaluate_name(&prog_raw, &expression_context) {
            Ok(name) => name,
            Err(reason) => {
                skipped_programs.push(format!(
                    "{}:{}: %build_prog mmake={mmake_raw} progname={prog_raw} is unresolved: {reason}",
                    rel_dir.display(),
                    inv.line + 1
                ));
                continue;
            }
        };

        let mut sources = match evaluate_macro_sources(&inv.args, &vars, &expression_context) {
            Ok(sources) => sources,
            Err(reason) => {
                skipped_programs.push(format!(
                    "{}:{}: %build_prog mmake={mmake_raw} progname={prog_raw} {reason}",
                    rel_dir.display(),
                    inv.line + 1
                ));
                continue;
            }
        };
        record_partial_source_lists(
            &mut partial_source_lists,
            &mut source_inventory_patterns,
            &sources,
            &rel_dir,
            inv,
            &mmake_raw,
        );
        if sources.is_empty() {
            if sources.declared {
                // A list was given but its Make variables are unresolved.
                // Falling back to the program name here would compile the
                // wrong file, so report instead.
                skipped_programs.push(format!(
                    "{}: %build_prog mmake={mmake_raw} progname={prog_raw} has an unresolved file list",
                    rel_dir.display()
                ));
                continue;
            }
            sources.c.push(prog_name.clone());
        }

        let use_libs =
            macro_arg(&inv.args, "uselibs").map_or_else(Vec::new, |l| expand_file_list(&l, &vars));
        let always_cxx_link =
            match resolve_yes_argument(&inv.args, "alwayscxxlink", &scope, dirs, inv.line) {
                Ok(value) => value,
                Err(reason) => {
                    skipped_programs.push(format!(
                        "{}:{}: %build_prog mmake={mmake_raw} {reason}",
                        rel_dir.display(),
                        inv.line + 1
                    ));
                    continue;
                }
            };
        let target_dir = match evaluate_output_directory(&inv.args, &expression_context) {
            Ok(directory) => directory,
            Err(reason) => {
                unresolved_output_paths.push(format!(
                    "{}:{}: %build_prog mmake={mmake_raw} {reason}",
                    rel_dir.display(),
                    inv.line + 1
                ));
                None
            }
        };

        targets.push(TargetDefinition {
            mmake_name,
            target_name: prog_name,
            module_type: ModuleType::Program,
            genmodule_only: false,
            empty_archive: false,
            source_files: sources.c,
            cxx_source_files: sources.cxx,
            always_cxx_link,
            objc_source_files: sources.objc,
            asm_source_files: sources.asm,
            use_libs,
            dependencies: Vec::new(),
            dir_path: rel_dir.clone(),
            target_dir,
            link_libs: Vec::new(),
            variant_32bit: false,
            declared_mod_type: None,
            mod_suffix: None,
            linklib_name: None,
            config_file: None,
            genmodule_linklibs: None,
            canonical_linklib_output: false,
            canonical_linklib_eligible: false,
            linklib_output_dir: None,
            compiler_flags: Vec::new(),
            include_dirs: {
                let mut d = declaration_includes.dirs.clone();
                d.extend(opts_include_dirs.iter().cloned());
                d
            },
            arch_modules: declaration_includes.arch_modules.clone(),
            arch_includes: opts_arch_includes.clone(),
            defines: declaration_flags.defines,
            undefines: declaration_flags.undefines,
            compile_options: declaration_flags.compile_options,
            link_options: declaration_flags.link_options,
            spec_switches: declaration_flags.spec_switches.clone(),
            driver_link_options: driver_link_options.clone(),
            isa_link_options: isa_link_options.clone(),
            arch_sources: Vec::new(),
            arch_defines: arch_defines.clone(),
            arch_compile_options: arch_compile_options.clone(),
            arch_source_options: Vec::new(),
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
        let vars = scope.snapshot(inv.line);
        let expression_context = MakeExprContext::new(&scope, dirs, inv.line, root, &rel_dir);
        let isa_link_options = declaration_global_link_options(
            "TARGET_ISA_LDFLAGS",
            &scope,
            dirs,
            root,
            &rel_dir,
            inv.line,
        );
        let driver_link_options =
            declaration_global_link_options("USER_LDFLAGS", &scope, dirs, root, &rel_dir, inv.line);
        let mut declaration_flags = declaration_flags_at(
            &scope,
            inv.line,
            target,
            &flag_set,
            &opts_link_options,
            &opts_spec_switches,
        );
        let mut declaration_includes = target.map_or_else(
            || include_set.clone(),
            |_| collect_includes_at(&joined, &scope, inv.line, &rel_dir),
        );
        let mmake_name = sanitize_ident(&mmake_raw);
        let mesa20_capability_sources = match remaining_linklib_sources(
            root,
            &rel_dir,
            &mmake_name,
            target,
        ) {
            Ok(sources) => sources,
            Err(reason) => {
                capability_errors.push(capability_diagnostic(
                        &relative_path,
                        Some(inv.line + 1),
                        format!(
                            "%{} mmake={mmake_raw} no longer matches the Mesa archive capability: {reason}",
                            inv.name
                        ),
                    ));
                skipped_programs.push(format!(
                    "{}:{}: %{} mmake={mmake_raw} Mesa 20.0.8 archive capability skipped: {reason}",
                    rel_dir.display(),
                    inv.line + 1,
                    inv.name
                ));
                continue;
            }
        };
        let mesa20_capability_active = mesa20_capability_sources.is_some();
        let nouveau_drm_capability_sources = match crate::capability::nouveau::drm_sources(
            root,
            &rel_dir,
            &mmake_name,
            target,
        ) {
            Ok(sources) => sources,
            Err(reason) => {
                capability_errors.push(capability_diagnostic(
                        &relative_path,
                        Some(inv.line + 1),
                        format!(
                            "%{} mmake={mmake_raw} no longer matches the Nouveau DRM archive capability: {reason}",
                            inv.name
                        ),
                    ));
                skipped_programs.push(format!(
                    "{}:{}: %{} mmake={mmake_raw} Nouveau DRM archive capability skipped: {reason}",
                    rel_dir.display(),
                    inv.line + 1,
                    inv.name
                ));
                continue;
            }
        };
        let nouveau_drm_capability_active = nouveau_drm_capability_sources.is_some();
        let nouveau_gallium_capability_sources = match crate::capability::nouveau::gallium_sources(
            root,
            &rel_dir,
            &mmake_name,
            target,
        ) {
            Ok(sources) => sources,
            Err(reason) => {
                capability_errors.push(capability_diagnostic(
                    &relative_path,
                    Some(inv.line + 1),
                    format!(
                        "%{} mmake={mmake_raw} no longer matches the Nouveau Gallium archive capability: {reason}",
                        inv.name
                    ),
                ));
                skipped_programs.push(format!(
                        "{}:{}: %{} mmake={mmake_raw} Nouveau Gallium archive capability skipped: {reason}",
                        rel_dir.display(),
                        inv.line + 1,
                        inv.name
                    ));
                continue;
            }
        };
        let nouveau_gallium_capability_active = nouveau_gallium_capability_sources.is_some();
        if let Err(reason) = apply_mesa_compile_contract(
            &rel_dir,
            &mmake_name,
            target,
            &mut declaration_flags,
            &mut declaration_includes,
        ) {
            capability_errors.push(capability_diagnostic(
                &relative_path,
                Some(inv.line + 1),
                format!(
                    "%{} mmake={mmake_raw} no longer matches the Mesa compile capability: {reason}",
                    inv.name
                ),
            ));
            skipped_programs.push(format!(
                "{}:{}: %{} mmake={mmake_raw} Mesa 20.0.8 compile contract skipped: {reason}",
                rel_dir.display(),
                inv.line + 1,
                inv.name
            ));
            continue;
        }
        match crate::capability::nouveau::drm_compile_contract(&rel_dir, &mmake_name, target) {
            Ok(Some(contract)) => {
                declaration_flags.defines = contract.defines;
                declaration_flags.undefines.clear();
                declaration_flags.compile_options = contract.options;
                declaration_flags.link_options.clear();
                declaration_includes.dirs = contract.includes;
                declaration_includes.arch_modules.clear();
            }
            Ok(None) => {}
            Err(reason) => {
                capability_errors.push(capability_diagnostic(
                    &relative_path,
                    Some(inv.line + 1),
                    format!(
                        "%{} mmake={mmake_raw} no longer matches the Nouveau DRM compile capability: {reason}",
                        inv.name
                    ),
                ));
                skipped_programs.push(format!(
                    "{}:{}: %{} mmake={mmake_raw} Nouveau DRM compile contract skipped: {reason}",
                    rel_dir.display(),
                    inv.line + 1,
                    inv.name
                ));
                continue;
            }
        }
        match crate::capability::nouveau::gallium_compile_contract(&rel_dir, &mmake_name, target) {
            Ok(Some(contract)) => {
                declaration_flags.defines = contract.defines;
                declaration_flags.undefines.clear();
                declaration_flags.compile_options = contract.options;
                declaration_flags.link_options.clear();
                declaration_includes.dirs = contract.includes;
                declaration_includes.arch_modules.clear();
            }
            Ok(None) => {}
            Err(reason) => {
                capability_errors.push(capability_diagnostic(
                    &relative_path,
                    Some(inv.line + 1),
                    format!(
                        "%{} mmake={mmake_raw} no longer matches the Nouveau Gallium compile capability: {reason}",
                        inv.name
                    ),
                ));
                skipped_programs.push(format!(
                    "{}:{}: %{} mmake={mmake_raw} Nouveau Gallium compile contract skipped: {reason}",
                    rel_dir.display(),
                    inv.line + 1,
                    inv.name
                ));
                continue;
            }
        }
        let mesa_sse41_profile = (mmake_name == sse41::MMAKE
            && sse41::validate_static_contract(root, &content).is_ok())
        .then(|| sse41::profile(&rel_dir, target).ok().flatten())
        .flatten();
        let empty_archive = mesa_sse41_profile == Some(false);
        if let Some(x86_64) = mesa_sse41_profile {
            // The ordinary local-include scanner cannot adopt mesa.cfg for
            // this file on a cold tree: the neighbouring full libmesa target
            // still depends on the not-yet-fetched upstream inventory. Admit
            // the exact declaration-local view only together with the
            // capability and profile contract validated below.
            declaration_flags.defines = sse41::defines(x86_64);
            declaration_flags.undefines.clear();
            declaration_flags.compile_options = sse41::compile_options(x86_64);
            declaration_flags.link_options.clear();
            declaration_includes.dirs = sse41::INCLUDES
                .iter()
                .map(|include| (*include).to_owned())
                .collect();
            declaration_includes.arch_modules.clear();
        }

        // %build_progs has no name of its own: each source file names its own
        // executable, so the mmake id carries the group.
        let target_name = match name_arg {
            None => mmake_name.clone(),
            Some(key) => {
                let Some(raw) = macro_arg(&inv.args, key) else {
                    skipped_programs.push(format!(
                        "{}: %{} mmake={mmake_raw} has no {key}",
                        rel_dir.display(),
                        inv.name
                    ));
                    continue;
                };
                match evaluate_name(&raw, &expression_context) {
                    Ok(name) => name,
                    Err(reason) => {
                        skipped_programs.push(format!(
                            "{}:{}: %{} mmake={mmake_raw} {key}={raw} is unresolved: {reason}",
                            rel_dir.display(),
                            inv.line + 1,
                            inv.name
                        ));
                        continue;
                    }
                }
            }
        };
        if matches!(module_type, ModuleType::SimpleModule) {
            // config/make.tmpl appends `<modname>_LDFLAGS` only to this bare
            // module's link. Preserve that scope instead of forcing a
            // file-global USER_LDFLAGS change onto neighbouring modules.
            merge_named_link_flags(
                &mut declaration_flags,
                &scope,
                inv.line,
                &format!("{target_name}_LDFLAGS"),
            );
        }

        let resolved_generated_files = if module_type == ModuleType::LinkLib {
            match macro_arg(&inv.args, "files") {
                Some(files) => {
                    match resolve_generated_linklib_sources(&files, &joined, &rel_dir, |name| {
                        expression_context.safe_local_raw(name)
                    }) {
                        Ok(Some(generated)) => Some(generated.sources),
                        Ok(None) => None,
                        Err(reason) => {
                            skipped_programs.push(format!(
                                "{}:{}: %{} mmake={mmake_raw} {reason}",
                                rel_dir.display(),
                                inv.line + 1,
                                inv.name
                            ));
                            continue;
                        }
                    }
                }
                None => None,
            }
        } else {
            None
        };
        let capability_files = mesa_sse41_profile.map(sse41::sources);
        let mut sources = if let Some(sources) = mesa20_capability_sources {
            sources
        } else if let Some(sources) = nouveau_drm_capability_sources {
            sources
        } else if let Some(sources) = nouveau_gallium_capability_sources {
            sources
        } else {
            match evaluate_macro_sources_with_files(
                &inv.args,
                &vars,
                &expression_context,
                capability_files
                    .as_deref()
                    .or(resolved_generated_files.as_deref()),
            ) {
                Ok(sources) => sources,
                Err(reason) => {
                    skipped_programs.push(format!(
                        "{}:{}: %{} mmake={mmake_raw} {reason}",
                        rel_dir.display(),
                        inv.line + 1,
                        inv.name
                    ));
                    continue;
                }
            }
        };
        record_partial_source_lists(
            &mut partial_source_lists,
            &mut source_inventory_patterns,
            &sources,
            &rel_dir,
            inv,
            &mmake_raw,
        );
        if sources.is_empty() && !empty_archive {
            if sources.declared {
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
                sources.c = wildcard_c_sources(parent_dir);
            }
            if sources.is_empty() {
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
        let is_simple_module = matches!(module_type, ModuleType::SimpleModule);
        let always_cxx_link = if is_simple_module {
            match resolve_yes_argument(&inv.args, "alwayscxxlink", &scope, dirs, inv.line) {
                Ok(value) => value,
                Err(reason) => {
                    skipped_programs.push(format!(
                        "{}:{}: %{} mmake={mmake_raw} {reason}",
                        rel_dir.display(),
                        inv.line + 1,
                        inv.name
                    ));
                    continue;
                }
            }
        } else {
            false
        };
        let declared_mod_type = if is_simple_module {
            macro_arg(&inv.args, "modtype")
        } else {
            None
        };
        let is_program_group = matches!(module_type, ModuleType::ProgramGroup);
        let target_dir = if is_simple_module {
            match resolve_module_target_dir(
                &inv.args,
                &scope,
                dirs,
                inv.line,
                declared_mod_type.as_deref().unwrap_or_default(),
                false,
                false,
            ) {
                Ok(value) => value,
                Err(reason) => {
                    skipped_programs.push(format!(
                        "{}:{}: %{} mmake={mmake_raw} {reason}",
                        rel_dir.display(),
                        inv.line + 1,
                        inv.name
                    ));
                    continue;
                }
            }
        } else if is_program_group {
            match evaluate_output_directory(&inv.args, &expression_context) {
                Ok(directory) => directory,
                Err(reason) => {
                    unresolved_output_paths.push(format!(
                        "{}:{}: %{} mmake={mmake_raw} {reason}",
                        rel_dir.display(),
                        inv.line + 1,
                        inv.name
                    ));
                    None
                }
            }
        } else {
            None
        };
        let mod_suffix = if is_simple_module {
            match resolve_module_suffix(
                &inv.args,
                &scope,
                dirs,
                inv.line,
                declared_mod_type.as_deref().unwrap_or_default(),
            ) {
                Ok(value) => value,
                Err(reason) => {
                    skipped_programs.push(format!(
                        "{}:{}: %{} mmake={mmake_raw} {reason}",
                        rel_dir.display(),
                        inv.line + 1,
                        inv.name
                    ));
                    continue;
                }
            }
        } else {
            None
        };
        // The 32-bit flavour is told apart by where it writes, not by its
        // name: libdir=$(GENDIR)/lib32 and objdir=.../32bit.
        let variant_32bit = ["libdir", "objdir"].iter().any(|k| {
            macro_arg(&inv.args, k).is_some_and(|v| v.contains("lib32") || v.contains("32bit"))
        });
        let canonical_linklib_eligible = matches!(module_type, ModuleType::LinkLib)
            && macro_arg(&inv.args, "libdir").is_none()
            && macro_arg(&inv.args, "compiler").is_none_or(|value| value == "target")
            && !variant_32bit;
        let canonical_linklib_output = canonical_linklib_eligible
            && (all_sources_are_fetch_owned(&sources, &fetches)
                || nouveau_drm_capability_active
                || nouveau_gallium_capability_active);
        let linklib_output_dir = if mesa_sse41_profile.is_some() || mesa20_capability_active {
            Some(PRIVATE_LIBDIR.to_owned())
        } else if matches!(module_type, ModuleType::LinkLib) {
            macro_arg(&inv.args, "libdir").and_then(|raw| {
                match evaluate_make_expr(&raw, &expression_context) {
                    Ok(directory) if safe_build_tree_output_directory(&directory) => {
                        Some(directory)
                    }
                    Ok(directory) => {
                        unresolved_output_paths.push(format!(
                            "{}:{}: %{} mmake={mmake_raw} libdir={raw} resolves outside the build tree ({directory})",
                            rel_dir.display(),
                            inv.line + 1,
                            inv.name
                        ));
                        None
                    }
                    Err(reason) => {
                        unresolved_output_paths.push(format!(
                            "{}:{}: %{} mmake={mmake_raw} libdir={raw} is unresolved: {reason}",
                            rel_dir.display(),
                            inv.line + 1,
                            inv.name
                        ));
                        None
                    }
                }
            })
        } else {
            None
        };

        targets.push(TargetDefinition {
            mmake_name,
            target_name,
            module_type,
            genmodule_only: false,
            empty_archive,
            source_files: sources.c,
            cxx_source_files: sources.cxx,
            always_cxx_link,
            objc_source_files: sources.objc,
            asm_source_files: sources.asm,
            use_libs,
            dependencies: Vec::new(),
            dir_path: rel_dir.clone(),
            target_dir,
            link_libs: Vec::new(),
            variant_32bit,
            declared_mod_type,
            mod_suffix,
            linklib_name: None,
            config_file: None,
            genmodule_linklibs: None,
            canonical_linklib_output,
            canonical_linklib_eligible,
            linklib_output_dir,
            compiler_flags: Vec::new(),
            include_dirs: {
                let mut d = declaration_includes.dirs.clone();
                d.extend(opts_include_dirs.iter().cloned());
                d
            },
            arch_modules: declaration_includes.arch_modules.clone(),
            arch_includes: opts_arch_includes.clone(),
            defines: declaration_flags.defines,
            undefines: declaration_flags.undefines,
            compile_options: declaration_flags.compile_options,
            link_options: declaration_flags.link_options,
            spec_switches: declaration_flags.spec_switches.clone(),
            driver_link_options: driver_link_options.clone(),
            isa_link_options: isa_link_options.clone(),
            arch_sources: Vec::new(),
            arch_defines: arch_defines.clone(),
            arch_compile_options: arch_compile_options.clone(),
            arch_source_options: Vec::new(),
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

    if let Err(reason) = sse41::validate(
        root,
        &rel_dir,
        target,
        &content,
        &targets,
        &ownership_fetches,
    ) {
        // The ordinary parser may have resolved part of this declaration, but
        // executable empty-archive support and the target-only ISA flag are
        // admitted as one atomic capability. Any drift removes the target.
        targets.retain(|candidate| candidate.mmake_name != sse41::MMAKE);
        capability_errors.push(capability_diagnostic(
            &relative_path,
            None,
            format!("Mesa SSE4.1 link library no longer matches its closed capability: {reason}"),
        ));
        skipped_programs.push(format!(
            "{}: Mesa SSE4.1 link library skipped: {reason}",
            rel_dir.display()
        ));
    }

    if targets
        .iter()
        .any(|candidate| candidate.mmake_name == crate::capability::nouveau::DRM_MMAKE)
    {
        if let Err(reason) =
            crate::capability::nouveau::validate_drm(root, &rel_dir, target, &targets)
        {
            // The DRM source fragment is intentionally admitted only as one
            // closed capability.  Do not leave a partially inferred target in
            // the graph when its recipe, inventory or canonical archive proof
            // has drifted.
            targets
                .retain(|candidate| candidate.mmake_name != crate::capability::nouveau::DRM_MMAKE);
            capability_errors.push(capability_diagnostic(
                &relative_path,
                None,
                format!(
                    "Nouveau DRM link library no longer matches its closed capability: {reason}"
                ),
            ));
            skipped_programs.push(format!(
                "{}: Nouveau DRM link library skipped: {reason}",
                rel_dir.display()
            ));
        }
    }

    if targets
        .iter()
        .any(|candidate| candidate.mmake_name == crate::capability::nouveau::GALLIUM_MMAKE)
    {
        if let Err(reason) =
            crate::capability::nouveau::validate_gallium(root, &rel_dir, target, &targets)
        {
            // The fetched Mesa lane contains a C++ source inventory. Keep it
            // atomic with its checked source and flag contract rather than
            // leaving an inferred C-only or private-output approximation in
            // the graph.
            targets.retain(|candidate| {
                candidate.mmake_name != crate::capability::nouveau::GALLIUM_MMAKE
            });
            capability_errors.push(capability_diagnostic(
                &relative_path,
                None,
                format!(
                    "Nouveau Gallium link library no longer matches its closed capability: {reason}"
                ),
            ));
            skipped_programs.push(format!(
                "{}: Nouveau Gallium link library skipped: {reason}",
                rel_dir.display()
            ));
        }
    }

    let mut python_outputs = Vec::new();
    match generators::parse_glapi(&rel_dir, target, &content, &targets, &ownership_fetches) {
        Ok(Some(declaration)) => python_outputs.push(declaration),
        Ok(None) => {}
        Err(reason) => {
            capability_errors.push(capability_diagnostic(
                &relative_path,
                None,
                format!("Mesa glapi generator no longer matches its closed capability: {reason}"),
            ));
            skipped_programs.push(format!(
                "{}: Mesa glapi Python generator skipped: {reason}",
                rel_dir.display()
            ));
        }
    }
    match generators::parse_mesautil(&rel_dir, target, &content, &targets, &ownership_fetches) {
        Ok(Some(declaration)) => python_outputs.push(declaration),
        Ok(None) => {}
        Err(reason) => {
            capability_errors.push(capability_diagnostic(
                &relative_path,
                None,
                format!("Mesa utility generator no longer matches its closed capability: {reason}"),
            ));
            skipped_programs.push(format!(
                "{}: Mesa utility Python generator skipped: {reason}",
                rel_dir.display()
            ));
        }
    }
    let mesa20_required_target = match rel_dir.to_str() {
        Some("workbench/libs/mesa/libcompiler") => Some("mesa3d-linklib-compiler"),
        Some("workbench/libs/mesa/libgalliumaux") => Some("mesa3d-linklib-galliumauxiliary"),
        Some("workbench/libs/mesa/libmesa") => Some("mesa3d-linklib-mesa"),
        Some("arch/arm-native/soc/broadcom/2708/hidd/vc4gallium")
            if current_profile(target).ok() != Some("x86_64") =>
        {
            Some("linklibs-gallium_vc4")
        }
        _ => None,
    };
    match mesa20::parse_remaining(
        root,
        &rel_dir,
        target,
        &content,
        &targets,
        &ownership_fetches,
    ) {
        Ok(Some(declaration)) => python_outputs.push(declaration),
        Ok(None) => {}
        Err(reason) => {
            if let Some(mmake) = mesa20_required_target {
                // Source admission and every generator product form one
                // capability. A partial archive with missing generated
                // translation units is never an executable fallback.
                targets.retain(|candidate| candidate.mmake_name != mmake);
            }
            capability_errors.push(capability_diagnostic(
                &relative_path,
                None,
                format!(
                    "Mesa 20.0.8 archive/generator no longer matches its closed capability: {reason}"
                ),
            ));
            skipped_programs.push(format!(
                "{}: Mesa 20.0.8 archive/generator capability skipped: {reason}",
                rel_dir.display()
            ));
        }
    }
    match mesa20::parse_v3d(
        root,
        &rel_dir,
        target,
        &content,
        &targets,
        &ownership_fetches,
    ) {
        Ok(declarations) => python_outputs.extend(declarations),
        Err(reason) => {
            targets.retain(|candidate| candidate.mmake_name != "linklibs-gallium_v3d");
            capability_errors.push(capability_diagnostic(
                &relative_path,
                None,
                format!(
                    "Mesa 20.0.8 V3D archive/generators no longer match their closed capability: {reason}"
                ),
            ));
            skipped_programs.push(format!(
                "{}: Mesa 20.0.8 V3D archive/generator capability skipped: {reason}",
                rel_dir.display()
            ));
        }
    }

    // Paired FlexCat recipes are normal Make rules rather than a MetaMake
    // macro.  Parse them after all concrete source lists are known, so the
    // graph can bind their generated `locale.c` only to real consumers.
    let flexcat_scan = collect_flexcat_source_rules(&content, root, &rel_dir, &scope, dirs);
    let ilbm_scan = collect_ilbm_sources(&content, root, &rel_dir, &scope, dirs);

    // 3. Extract #MM and #MM- meta-target rules
    let mm_content = join_mm_continuations(&content);
    for cap in META_RULE_RE.captures_iter(&mm_content) {
        let raw_meta = &cap[1];
        let Some(meta_name) = render_meta_token(raw_meta) else {
            skipped_meta_rules.push(format!(
                "{}: #MM target {raw_meta} contains an unmapped Make variable",
                rel_dir.display()
            ));
            continue;
        };
        let deps_str = &cap[2];
        let mut deps = Vec::new();
        for raw_dep in deps_str.split_whitespace() {
            match render_meta_token(raw_dep) {
                Some(dep) => deps.push(dep),
                None => skipped_meta_rules.push(format!(
                    "{}: #MM {raw_meta} dependency {raw_dep} contains an unmapped Make variable",
                    rel_dir.display()
                )),
            }
        }

        if !deps.is_empty() {
            meta_rules.push(MetaTargetRule {
                name: meta_name,
                dependencies: deps,
            });
        }
    }

    // %rule_link_binary needs the file's targets, to check an explicit mmake=,
    // and the %build_archspecific object roots, which is how the reference
    // attaches an unnamed one.
    let known_target_names: Vec<String> = targets.iter().map(|t| t.mmake_name.clone()).collect();
    let arch_object_roots: Vec<(String, String, String)> = arch_sources
        .iter()
        .filter_map(|decl| {
            let maindir = decl.maindir.as_ref()?.trim_matches('/');
            let modname = decl.modname.as_ref()?;
            Some((
                format!("${{AROS_BUILD_DIR}}/gen/{maindir}/{modname}/arch"),
                decl.mainmmake.clone(),
                decl.tag.clone(),
            ))
        })
        .collect();
    let (host_generated_headers, skipped_host_generated_headers) =
        crate::host_generated_headers::collect_host_generated_headers(&content, &rel_dir);
    let (hidd_stubs, skipped_hidd_stubs) =
        crate::hidd_stubs::collect_hidd_stubs(&content, &scope, dirs, root, &rel_dir);
    let (binary_objects, skipped_binary_objects) = crate::binary_objects::collect_binary_objects(
        &content,
        &scope,
        dirs,
        root,
        &rel_dir,
        &known_target_names,
        &arch_object_roots,
    );

    // A pattern recipe is a template, not a literal missing output. When a
    // closed Python-output capability instantiates concrete products matching
    // that template (V3D's version wrappers are the current case), keep the
    // template out of the residual generated-file report.
    let capability_outputs = python_outputs
        .iter()
        .flat_map(|declaration| {
            declaration.jobs.iter().map(|job| {
                format!(
                    "{}/{}",
                    declaration.build_root.trim_end_matches('/'),
                    job.output.trim_start_matches('/')
                )
                .replace("${AROS_BUILD_DIR}", "${CMAKE_BINARY_DIR}")
            })
        })
        .collect::<Vec<_>>();
    copy_scan.generated_files.retain(|report| {
        let Some((target, _)) = report.split_once(" <- ") else {
            return true;
        };
        let target = target.replace("${AROS_BUILD_DIR}", "${CMAKE_BINARY_DIR}");
        let Some((prefix, suffix)) = target.split_once('%') else {
            return true;
        };
        !capability_outputs.iter().any(|output| {
            output
                .strip_prefix(prefix)
                .and_then(|rest| rest.strip_suffix(suffix))
                .is_some()
        })
    });

    Ok(ParsedMmakefile {
        capability_errors,
        targets,
        external_cmake,
        configure_builds,
        grub_builds,
        ahi_builds,
        python_outputs,
        flexcat_sources: flexcat_scan.declarations,
        flexcat_headers: flexcat_scan.headers,
        skipped_flexcat_sources: flexcat_scan.skipped,
        ilbm_sources: ilbm_scan.declarations,
        skipped_ilbm_sources: ilbm_scan.skipped,
        meta_rules,
        icon_targets: icon_scan.targets,
        icons: icon_scan.sets,
        skipped_icons: icon_scan.skipped,
        catalogs: catalog_scan.declarations,
        skipped_catalogs: catalog_scan.skipped,
        skipped_meta_rules,
        arch_decls,
        unresolved_includes: include_set.unresolved,
        copy_includes: copy_scan.decls,
        skipped_copy_includes: copy_scan.skipped,
        copy_directories,
        skipped_copy_directories,
        adhoc_header_rules: copy_scan.adhoc,
        header_transforms: copy_scan.transforms,
        bison_outputs: copy_scan.bison_outputs,
        define_headers,
        generated_file_rules: copy_scan.generated_files,
        script_outputs: copy_scan.script_outputs,
        skipped_script_outputs: copy_scan.skipped_script_outputs,
        flags: flag_set,
        arch_sources,
        skipped_arch_sources,
        binary_objects,
        skipped_binary_objects,
        hidd_stubs,
        skipped_hidd_stubs,
        host_generated_headers,
        skipped_host_generated_headers,
        fetches,
        skipped_fetches,
        skipped_make_opts,
        skipped_local_make_includes,
        skipped_conditions,
        skipped_programs,
        partial_source_lists,
        source_inventory_patterns,
        skipped_client_archives,
        unresolved_output_paths,
        packages,
        skipped_packages,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        capability_diagnostic, collect_vars, collect_vars_impl, evaluate_macro_sources,
        implicit_module_meta_rules, is_explicit_genmodule_only, join_continuations,
        join_mm_continuations, macro_arg, macro_argument_names, macro_invocations,
        render_meta_token, resolve_module_suffix, resolve_module_target_dir, sanitize_ident,
        select_target_invocations, MakeExprContext, TargetContext, META_RULE_RE,
    };
    use crate::ast::ModuleType;
    use crate::capability::external_cmake::{self, AOM_COMMON_OPTIONS};
    use crate::dirs::DirVars;
    use crate::make_vars::collect_vars_with_context;
    use crate::testing::{dirs, root, target_context, TempTree};
    use aros_common::{read_source, DiagnosticCode, DiagnosticStage};
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::path::Path;
    use walkdir::WalkDir;

    #[test]
    fn capability_drift_is_typed_without_inspecting_failure_text() {
        let diagnostic = capability_diagnostic(
            Path::new("workbench/example/mmakefile.src"),
            Some(17),
            "opaque upstream failure",
        );
        assert_eq!(diagnostic.code, DiagnosticCode::CapabilityDrift);
        assert_eq!(diagnostic.stage, DiagnosticStage::CapabilityValidation);
        let location = diagnostic.location.unwrap();
        assert_eq!(location.path, "workbench/example/mmakefile.src");
        assert_eq!(location.line, Some(17));
    }

    #[test]
    fn recursive_collector_includes_keep_original_curdir_and_make_root() {
        let tree = TempTree::new();
        fs::create_dir_all(tree.0.join("shared")).unwrap();
        fs::create_dir_all(tree.0.join("module/path")).unwrap();
        fs::write(
            tree.0.join("shared/vars.mk"),
            "include nested.mk\ninclude $(SRCDIR)/$(CURDIR)/local.mk\n",
        )
        .unwrap();
        fs::write(tree.0.join("nested.mk"), "ROOT_RELATIVE_INCLUDE := yes\n").unwrap();
        fs::write(
            tree.0.join("shared/nested.mk"),
            "WRONG_INCLUDE_FILE_DIRECTORY := yes\n",
        )
        .unwrap();
        fs::write(
            tree.0.join("module/path/local.mk"),
            "ORIGINAL_MMAKE_CURDIR := yes\n",
        )
        .unwrap();
        fs::write(
            tree.0.join("shared/local.mk"),
            "WRONG_RECURSIVE_CURDIR := yes\n",
        )
        .unwrap();

        let mut visited = std::collections::HashSet::new();
        let inlined = super::inline_collector_make_includes(
            "include $(SRCDIR)/shared/vars.mk\n",
            &tree.0,
            Path::new("module/path"),
            &mut visited,
            8,
        );
        assert!(
            inlined.contains("ROOT_RELATIVE_INCLUDE := yes"),
            "{inlined}"
        );
        assert!(
            inlined.contains("ORIGINAL_MMAKE_CURDIR := yes"),
            "{inlined}"
        );
        assert!(
            !inlined.contains("WRONG_INCLUDE_FILE_DIRECTORY"),
            "{inlined}"
        );
        assert!(!inlined.contains("WRONG_RECURSIVE_CURDIR"), "{inlined}");
    }

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
        let joined = join_continuations(src);
        let invs = macro_invocations(&joined);
        let progs: Vec<&super::Invocation> =
            invs.iter().filter(|i| i.name == "build_prog").collect();
        assert_eq!(progs.len(), 2);
        assert_eq!(macro_arg(&progs[0].args, "progname").unwrap(), "SysLog");
        assert_eq!(macro_arg(&progs[0].args, "files").unwrap(), "$(FILES)");
        assert_eq!(macro_arg(&progs[1].args, "progname").unwrap(), "Other");
    }

    #[test]
    fn only_a_literal_empty_library_file_list_is_genmodule_only() {
        assert!(is_explicit_genmodule_only(
            "build_module",
            r#"mmake=x modname=x modtype=library files="""#,
            "library"
        ));
        for (invocation, args, mod_type) in [
            (
                "build_module",
                "mmake=x modname=x modtype=library files=$(EMPTY)",
                "library",
            ),
            (
                "build_module",
                r#"mmake=x modname=x modtype=library files="" cxxfiles=x"#,
                "library",
            ),
            (
                "build_module",
                r#"mmake=x modname=x modtype=device files="""#,
                "device",
            ),
            (
                "build_module_abi",
                r#"mmake=x modname=x modtype=library files="""#,
                "library",
            ),
            (
                "build_module",
                r#"mmake=x modname=x modtype=library files=""junk"#,
                "library",
            ),
            (
                "build_module",
                r#"mmake=x modname=x modtype=library notfiles="""#,
                "library",
            ),
        ] {
            assert!(
                !is_explicit_genmodule_only(invocation, args, mod_type),
                "unexpected generated-only acceptance: %{invocation} {args}"
            );
        }
    }

    #[test]
    fn generated_module_meta_rules_keep_aliases_and_every_arch_endpoint() {
        let rules = implicit_module_meta_rules(
            "module-id",
            "module",
            "includes-set",
            &["dependency_rel".to_owned()],
            true,
            true,
            true,
        );
        let mut metas: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for rule in rules {
            metas
                .entry(rule.name)
                .or_default()
                .extend(rule.dependencies);
        }

        for (name, dependency) in [
            ("includes-set", "module-id-includes"),
            ("includes-module", "module-id-includes"),
            ("includes-module_rel", "module-id-includes"),
            ("linklibs-module", "module-id-linklib"),
            ("linklibs-module_rel", "module-id-linklib"),
            ("module-id-genmodfiles", "module-id-genmakefile"),
        ] {
            assert!(metas[name].contains(dependency), "{name} -> {dependency}");
        }
        for dependency in [
            "module-id-includes",
            "core-linklibs",
            "linklibs-dependency_rel",
            "module-id-${AROS_TARGET_CPU}",
        ] {
            assert!(metas["module-id"].contains(dependency), "{dependency}");
        }
        assert!(metas["module-id-quick"].contains("module-id"));
        for dependency in [
            "module-id-includes",
            "includes-dependency_rel",
            "module-id-${AROS_TARGET_CPU}-linklib",
        ] {
            assert!(
                metas["module-id-linklib"].contains(dependency),
                "{dependency}"
            );
        }
        for dependency in [
            "module-id-includes",
            "core-linklibs",
            "linklibs-dependency_rel",
            "module-id-${AROS_TARGET_CPU}-kobj",
            "module-id-${AROS_TARGET_CPU}",
        ] {
            assert!(metas["module-id-kobj"].contains(dependency), "{dependency}");
        }

        for suffix in [
            "",
            "-set-archincludes",
            "-linklib",
            "-kobj",
            "-kobj-quick",
            "-quick",
        ] {
            let leaf = format!(
                "module-id-${{AROS_TARGET_PLATFORM}}-${{AROS_TARGET_CPU}}-${{AROS_TARGET_VARIANT}}{suffix}"
            );
            assert!(metas.contains_key(&leaf), "missing {leaf}");
        }
        assert!(metas
            .values()
            .flatten()
            .all(|dependency| { dependency != "linklibs-" && dependency != "includes-" }));
    }

    #[test]
    fn target_context_selects_build_invocations_and_reports_unknown_guards() {
        let joined = join_continuations(
            "ifneq ($(AROS_TARGET_CPU32),)\n\
             %build_linklib mmake=linklibs-only32 libname=only32 files=only32\n\
             else\n\
             %build_linklib mmake=linklibs-native libname=native files=native\n\
             endif\n\
             ifeq ($(EXTERNAL_SWITCH),yes)\n\
             %build_prog mmake=unknown progname=unknown files=unknown\n\
             endif\n",
        );

        for (context, expected) in [
            (target_context("x86_64", "pc", ""), "linklibs-only32"),
            (target_context("arm", "raspi", "hard"), "linklibs-native"),
        ] {
            let (_, states) = collect_vars_impl(&joined, Some(&context));
            let mut skipped = Vec::new();
            let invocations = select_target_invocations(
                &joined,
                Some(&states),
                Path::new("fixture"),
                &mut skipped,
            );
            let names: Vec<String> = invocations
                .iter()
                .filter_map(|invocation| macro_arg(&invocation.args, "mmake"))
                .collect();
            assert_eq!(names, [expected]);
            assert_eq!(skipped.len(), 1, "{skipped:#?}");
            assert!(skipped[0].contains("mmake=unknown"), "{skipped:#?}");
        }
    }

    #[test]
    fn target_context_selects_external_cmake_invocations() {
        let joined = join_continuations(
            "ifeq ($(AROS_TARGET_CPU),x86_64)\n\
             %build_with_cmake mmake=cmake-x86 srcdir=x prefix=x extraoptions=x\n\
             endif\n\
             ifeq ($(AROS_TARGET_CPU),arm)\n\
             %build_with_cmake mmake=cmake-arm srcdir=x prefix=x extraoptions=x\n\
             endif\n\
             ifeq ($(UNKNOWN_EXTERNAL_SWITCH),yes)\n\
             %build_with_cmake mmake=cmake-unknown srcdir=x prefix=x extraoptions=x\n\
             endif\n",
        );

        for (context, expected) in [
            (target_context("x86_64", "pc", ""), "cmake-x86"),
            (target_context("arm", "raspi", "hard"), "cmake-arm"),
        ] {
            let (_, states) = collect_vars_impl(&joined, Some(&context));
            let mut skipped = Vec::new();
            let invocations = select_target_invocations(
                &joined,
                Some(&states),
                Path::new("fixture"),
                &mut skipped,
            );
            let selected: Vec<_> = invocations
                .iter()
                .filter(|invocation| invocation.name == "build_with_cmake")
                .filter_map(|invocation| macro_arg(&invocation.args, "mmake"))
                .collect();
            assert_eq!(selected, [expected]);
            assert_eq!(skipped.len(), 1, "{skipped:#?}");
            assert!(
                skipped[0].contains("%build_with_cmake mmake=cmake-unknown"),
                "{skipped:#?}"
            );
        }
    }

    #[test]
    fn macro_argument_scanner_ignores_nested_and_quoted_assignments() {
        assert_eq!(
            macro_argument_names(
                "mmake=x extraoptions=\"-DFOO=yes INNER=not-an-argument\" \
                 srcdir=$(if $(COND),A=B,C=D) prefix=x"
            ),
            ["mmake", "extraoptions", "srcdir", "prefix"]
        );
    }

    fn parsed_aom_capability(
        profile: &TargetContext,
    ) -> (
        super::Invocation,
        super::VarScope,
        Vec<crate::fetch::FetchDecl>,
        String,
    ) {
        let root = root();
        let relative_dir = Path::new("workbench/classes/datatypes/heic");
        let content = read_source(&root.join(relative_dir).join("mmakefile.src")).unwrap();
        let joined = join_continuations(&content);
        let (scope, states) = collect_vars_impl(&joined, Some(profile));
        let mut skipped = Vec::new();
        let invocation =
            select_target_invocations(&joined, Some(&states), relative_dir, &mut skipped)
                .into_iter()
                .find(|invocation| {
                    invocation.name == "build_with_cmake"
                        && macro_arg(&invocation.args, "mmake").as_deref()
                            == Some("datatypes-heic-linklibs-aom")
                })
                .unwrap();
        assert!(
            skipped
                .iter()
                .all(|diagnostic| !diagnostic.contains("datatypes-heic-linklibs-aom")),
            "{skipped:#?}"
        );
        let (fetches, skipped_fetches) =
            crate::fetch::collect_fetches_with_scope(&content, relative_dir, &scope);
        assert!(
            skipped_fetches
                .iter()
                .all(|diagnostic| !diagnostic.contains("linklibs-aom-fetch")),
            "{skipped_fetches:#?}"
        );
        (invocation, scope, fetches, content)
    }

    #[test]
    fn aom_external_cmake_capability_is_profile_exact() {
        let root = root();
        let relative_dir = Path::new("workbench/classes/datatypes/heic");
        let directory_vars = dirs();
        for (profile, specific) in [
            (
                target_context("x86_64", "pc", ""),
                vec!["-DAOM_TARGET_CPU=generic"],
            ),
            (
                target_context("arm", "raspi", "hard"),
                vec![
                    "-DAOM_TARGET_CPU=arm",
                    "-DENABLE_NEON=0",
                    "-DCONFIG_RUNTIME_CPU_DETECT=0",
                ],
            ),
            (
                target_context("aarch64", "raspi", ""),
                vec!["-DAOM_TARGET_CPU=generic"],
            ),
        ] {
            let (invocation, scope, fetches, content) = parsed_aom_capability(&profile);
            let expression_context = MakeExprContext::new(
                &scope,
                &directory_vars,
                invocation.line,
                &root,
                relative_dir,
            );
            let declaration = external_cmake::parse(
                &invocation,
                &expression_context,
                relative_dir,
                &fetches,
                Some(&profile),
                &content,
            )
            .unwrap();
            let mut expected: Vec<_> = AOM_COMMON_OPTIONS
                .iter()
                .map(|option| (*option).to_owned())
                .collect();
            expected.extend(specific.into_iter().map(str::to_owned));

            assert_eq!(declaration.mmake_name, "datatypes-heic-linklibs-aom");
            assert_eq!(
                declaration.provider_target,
                "datatypes-heic-linklibs-aom-external-aom"
            );
            assert_eq!(
                declaration.source_dir,
                "${AROS_PORTS_DIR}/libaom/libaom-3.12.1"
            );
            assert_eq!(
                declaration.binary_dir,
                "${AROS_BUILD_DIR}/gen/external-cmake/workbench/classes/datatypes/heic/aom"
            );
            assert_eq!(
                declaration.install_prefix,
                "${AROS_BUILD_DIR}/SYS/Developer"
            );
            assert_eq!(declaration.fetch_target, "linklibs-aom-fetch");
            assert_eq!(declaration.provided_library, "aom");
            assert_eq!(declaration.header_products.len(), 7);
            assert!(declaration
                .options
                .contains(&"-DCMAKE_BUILD_TYPE=Release".to_owned()));
            assert_eq!(
                declaration.auxiliary_products,
                ["${AROS_BUILD_DIR}/SYS/Developer/lib/pkgconfig/aom.pc"]
            );
            assert_eq!(declaration.options, expected);
        }
    }

    #[test]
    fn aom_external_cmake_capability_rejects_declaration_fetch_and_profile_drift() {
        let root = root();
        let relative_dir = Path::new("workbench/classes/datatypes/heic");
        let profile = target_context("x86_64", "pc", "");
        let (invocation, scope, fetches, content) = parsed_aom_capability(&profile);
        let directory_vars = dirs();
        let expression_context = MakeExprContext::new(
            &scope,
            &directory_vars,
            invocation.line,
            &root,
            relative_dir,
        );
        let parse = |invocation: &super::Invocation,
                     fetches: &[crate::fetch::FetchDecl],
                     profile: &TargetContext,
                     content: &str| {
            external_cmake::parse(
                invocation,
                &expression_context,
                relative_dir,
                fetches,
                Some(profile),
                content,
            )
            .unwrap_err()
        };

        let mut changed = invocation.clone();
        changed.args.push_str(" compiler=host");
        assert!(parse(&changed, &fetches, &profile, &content).contains("argument set"));

        let mut changed = invocation.clone();
        changed.args = changed.args.replace("package=aom", "package=other");
        assert!(parse(&changed, &fetches, &profile, &content).contains("package uses"));

        let mut changed = invocation.clone();
        changed.args = changed.args.replace(
            "extraldflags=\"$(LIBAOM_LDFLAGS)\"",
            "extraldflags=\"-lstdc++\"",
        );
        assert!(parse(&changed, &fetches, &profile, &content).contains("extraldflags uses"));

        let changed_content = content.replace("-DENABLE_TESTS=OFF", "-DENABLE_TESTS=ON");
        assert!(parse(&invocation, &fetches, &profile, &changed_content)
            .contains("declaration block differs"));

        let changed_content = content.replace(
            "LIBAOM_LDFLAGS+=$(TARGET_CXX_LDFLAGS)",
            "LIBAOM_LDFLAGS+=-Wl,--unreviewed",
        );
        assert!(parse(&invocation, &fetches, &profile, &changed_content)
            .contains("declaration block differs"));

        let mut changed_fetches = fetches.clone();
        changed_fetches[0].origins = "https://unreviewed.invalid".to_owned();
        assert!(
            parse(&invocation, &changed_fetches, &profile, &content).contains("archive_origins")
        );

        let mut changed_fetches = fetches.clone();
        changed_fetches[0].patches = "unreviewed.diff".to_owned();
        assert!(parse(&invocation, &changed_fetches, &profile, &content).contains("patches_specs"));

        let mut unsupported = profile;
        unsupported.toolchain = Some("gnu".to_owned());
        assert!(parse(&invocation, &fetches, &unsupported, &content)
            .contains("does not support target profile"));
    }

    #[test]
    fn target_context_selects_catalog_branches_and_reports_unknown_guards() {
        let tree = TempTree::new();
        let catalogs = tree.0.join("catalogs");
        fs::create_dir_all(&catalogs).unwrap();
        fs::write(catalogs.join("messages.cd"), "").unwrap();
        fs::write(catalogs.join("german.ct"), "").unwrap();
        let declaration = |mmake: &str| {
            format!(
                "%build_catalogs mmake={mmake} name=Sample subdir=Tools \
                 catalogs=german description=messages source=\"\" \
                 dir=$(TARGETDIR)/SYS/Locale/Catalogs\n"
            )
        };
        let source = format!(
            "ifeq ($(AROS_TARGET_CPU),x86_64)\n{}endif\n\
             ifeq ($(AROS_TARGET_CPU),arm)\n{}endif\n\
             ifeq ($(EXTERNAL_CATALOG_SWITCH),yes)\n{}endif\n",
            declaration("catalogs-x86"),
            declaration("catalogs-arm"),
            declaration("catalogs-unknown")
        );
        let file = catalogs.join("mmakefile.src");
        fs::write(&file, source).unwrap();
        let dirs = DirVars::load(&tree.0);

        for (context, expected) in [
            (target_context("x86_64", "pc", ""), "catalogs-x86"),
            (target_context("arm", "raspi", "hard"), "catalogs-arm"),
        ] {
            let parsed =
                super::parse_mmakefile_with_dirs_and_context(&file, &tree.0, &dirs, &context)
                    .unwrap();
            let names: Vec<_> = parsed
                .catalogs
                .iter()
                .map(|catalog| catalog.mmake.as_str())
                .collect();
            assert_eq!(names, [expected]);
            assert_eq!(parsed.skipped_catalogs.len(), 1);
            assert!(
                parsed.skipped_catalogs[0].contains("mmake=catalogs-unknown"),
                "{:#?}",
                parsed.skipped_catalogs
            );
        }
    }

    #[test]
    fn boost_recursive_copies_render_sdk_roots_and_port_source() {
        let root = root();
        let dirs = dirs();
        let parsed = super::parse_mmakefile_with_dirs_and_context(
            &root.join("compiler/boost/mmakefile.src"),
            &root,
            &dirs,
            &target_context("x86_64", "pc", ""),
        )
        .unwrap();

        assert!(parsed.skipped_copy_directories.is_empty(), "{parsed:#?}");
        assert_eq!(parsed.copy_directories.len(), 4);
        let geninc = parsed
            .copy_directories
            .iter()
            .find(|declaration| declaration.name == "compiler-boost-geninc-copy")
            .expect("GENINCDIR staging declaration");
        assert_eq!(geninc.source, "${AROS_PORTS_DIR}/boost/boost_1_89_0/boost");
        assert_eq!(geninc.destination, "${AROS_GENINC_DIR}/boost");
        let sdk = parsed
            .copy_directories
            .iter()
            .find(|declaration| declaration.name == "compiler-boost-includes-copy")
            .expect("SDK staging declaration");
        assert_eq!(sdk.source, geninc.source);
        assert_eq!(sdk.destination, "${AROS_SDK_INCLUDE_DIR}/boost");

        // The in-tree subset stages the same two destinations from
        // compiler/boost/include, for the release closure that must not fetch.
        let subset_geninc = parsed
            .copy_directories
            .iter()
            .find(|declaration| declaration.name == "compiler-boost-subset-geninc-copy")
            .expect("subset GENINCDIR staging declaration");
        assert_eq!(
            subset_geninc.source,
            "${CMAKE_SOURCE_DIR}/compiler/boost/include/boost"
        );
        assert_eq!(subset_geninc.destination, geninc.destination);
        let subset_sdk = parsed
            .copy_directories
            .iter()
            .find(|declaration| declaration.name == "compiler-boost-subset-includes-copy")
            .expect("subset SDK staging declaration");
        assert_eq!(subset_sdk.source, subset_geninc.source);
        assert_eq!(subset_sdk.destination, sdk.destination);
    }

    #[test]
    fn recursive_copy_collector_rejects_host_paths_and_unaudited_excludes() {
        let tree = TempTree::new();
        let module = tree.0.join("module");
        fs::create_dir_all(&module).unwrap();
        fs::create_dir_all(module.join("assets")).unwrap();
        let file = module.join("mmakefile.src");
        fs::write(
            &file,
            "%copy_dir_recursive mmake=safe-copy src=assets/. dst=$(TARGETDIR)/staged\n\
             %copy_dir_recursive mmake=host-copy src=/tmp/host dst=$(TARGETDIR)/staged\n\
             %copy_dir_recursive mmake=filtered-copy src=assets dst=$(TARGETDIR)/staged excludefiles=\"*.py\"\n",
        )
        .unwrap();
        let dirs = DirVars::load(&tree.0);
        let parsed = super::parse_mmakefile_with_dirs_and_context(
            &file,
            &tree.0,
            &dirs,
            &target_context("x86_64", "pc", ""),
        )
        .unwrap();

        assert_eq!(parsed.copy_directories.len(), 1, "{parsed:#?}");
        assert_eq!(parsed.copy_directories[0].name, "safe-copy");
        assert_eq!(
            parsed.copy_directories[0].source,
            "${CMAKE_SOURCE_DIR}/module/assets"
        );
        assert_eq!(
            parsed.copy_directories[0].destination,
            "${AROS_BUILD_DIR}/staged"
        );
        assert_eq!(parsed.skipped_copy_directories.len(), 2, "{parsed:#?}");
        assert!(parsed
            .skipped_copy_directories
            .iter()
            .any(|message| message.contains("host-copy")));
        assert!(parsed
            .skipped_copy_directories
            .iter()
            .any(|message| message.contains("filtered-copy")));
    }

    #[test]
    fn a_reassigned_list_is_read_as_of_each_declaration() {
        // arch/m68k-amiga/c/mmakefile.src, reduced. Reading the file-global
        // value gave both declarations `gdbstop`, so two targets claimed the
        // same output path and Ninja refused to generate the build.
        let src = "\
FILES := gdbstub

%build_progs mmake=workbench-c-m68k-gdbstub files=$(FILES) targetdir=$(AROS_C)

FILES := gdbstop

%build_progs mmake=workbench-c-m68k-misc files=$(FILES) targetdir=$(AROS_C)
";
        let joined = join_continuations(src);
        let scope = collect_vars(&joined);
        let invs = macro_invocations(&joined);
        assert_eq!(invs.len(), 2);

        let first = scope.snapshot(invs[0].line);
        assert_eq!(first.get("FILES").unwrap(), &vec!["gdbstub".to_owned()]);
        let second = scope.snapshot(invs[1].line);
        assert_eq!(second.get("FILES").unwrap(), &vec!["gdbstop".to_owned()]);
    }

    #[test]
    fn a_declaration_does_not_see_a_later_assignment() {
        let src = "%build_prog mmake=a progname=A files=$(F)\nF := late\n";
        let joined = join_continuations(src);
        let scope = collect_vars(&joined);
        let invs = macro_invocations(&joined);
        assert!(
            !scope.snapshot(invs[0].line).contains_key("F"),
            "a declaration must not read an assignment made after it"
        );
    }

    #[test]
    fn a_self_referential_assignment_keeps_the_earlier_value() {
        let src =
            "FILES := a b\nFILES := $(FILES) c\n%build_prog mmake=m progname=M files=$(FILES)\n";
        let joined = join_continuations(src);
        let scope = collect_vars(&joined);
        let invs = macro_invocations(&joined);
        assert_eq!(
            scope.snapshot(invs[0].line).get("FILES").unwrap(),
            &vec!["a".to_owned(), "b".to_owned(), "c".to_owned()]
        );
    }

    #[test]
    fn appended_values_accumulate_in_the_positional_snapshot_and_raw_value() {
        let src = "ICONS := A B\nICONS += C D\n%build_icons mmake=x icons=$(ICONS) dir=x\nICONS += late\n";
        let joined = join_continuations(src);
        let scope = collect_vars(&joined);
        let inv = &macro_invocations(&joined)[0];
        assert_eq!(
            scope.snapshot(inv.line).get("ICONS").unwrap(),
            &vec![
                "A".to_owned(),
                "B".to_owned(),
                "C".to_owned(),
                "D".to_owned()
            ]
        );
        assert_eq!(scope.raw_at("ICONS", inv.line).as_deref(), Some("A B C D"));
    }

    #[test]
    fn poseidon_static_runtime_applies_to_the_simple_module() {
        let root = root();
        let parsed = super::parse_mmakefile_with_dirs_and_context(
            &root.join("rom/usb/poseidon/mmakefile.src"),
            &root,
            &dirs(),
            &target_context("x86_64", "pc", ""),
        )
        .unwrap();
        let targets: BTreeMap<_, _> = parsed
            .targets
            .iter()
            .map(|target| (target.mmake_name.as_str(), target))
            .collect();
        let poseidon = targets.get("kernel-usb-poseidon").unwrap();
        let usbromstartup = targets.get("kernel-usb-usbromstartup").unwrap();

        assert_eq!(poseidon.spec_switches, ["static"]);
        assert_eq!(usbromstartup.spec_switches, ["static"]);
    }

    #[test]
    fn conditional_assignments_are_visible_to_strict_expression_callers() {
        let joined = join_continuations(
            "FILES := common\n\
             ifeq ($(ARCH),pc)\n\
             FILES += pc-only\n\
             else\n\
             FILES += other-only\n\
             endif\n\
             %build_prog mmake=x progname=x files=$(FILES)\n",
        );
        let invocation = macro_invocations(&joined).remove(0);
        let scope = collect_vars(&joined);

        assert!(scope.conditionally_assigned_before("FILES", invocation.line));
        // Preserve the existing raw view for collectors that partition and
        // evaluate Make branches themselves.
        assert_eq!(
            scope.raw_at("FILES", invocation.line).as_deref(),
            Some("common pc-only other-only")
        );
        assert!(!scope.conditionally_assigned_before("UNRELATED", invocation.line));
    }

    #[test]
    fn target_context_selects_one_conditional_branch_without_merging() {
        let joined = join_continuations(
            "FILES := common\n\
             ifeq ($(AROS_TARGET_CPU),x86_64)\n\
             FILES += x86-only\n\
             else ifeq ($(AROS_TARGET_CPU),aarch64)\n\
             FILES += arm64-only\n\
             else\n\
             FILES += other-only\n\
             endif\n\
             %build_prog mmake=x progname=x files=$(FILES)\n",
        );
        let invocation = macro_invocations(&joined).remove(0);

        let x86 = collect_vars_with_context(&joined, &target_context("x86_64", "pc", ""));
        assert_eq!(
            x86.raw_at("FILES", invocation.line).as_deref(),
            Some("common x86-only")
        );
        assert!(!x86.conditionally_assigned_before("FILES", invocation.line));

        let aarch64 = collect_vars_with_context(&joined, &target_context("aarch64", "raspi", ""));
        assert_eq!(
            aarch64.raw_at("FILES", invocation.line).as_deref(),
            Some("common arm64-only")
        );
        assert!(!aarch64.conditionally_assigned_before("FILES", invocation.line));
    }

    #[test]
    fn unknown_target_condition_is_unsafe_and_never_merged() {
        let joined = join_continuations(
            "FILES := common\n\
             ifeq ($(UNCONFIGURED_SWITCH),yes)\n\
             FILES += enabled\n\
             else\n\
             FILES += disabled\n\
             endif\n\
             %build_prog mmake=x progname=x files=$(FILES)\n",
        );
        let invocation = macro_invocations(&joined).remove(0);
        let scope = collect_vars_with_context(&joined, &target_context("x86_64", "pc", ""));
        assert_eq!(
            scope.raw_at("FILES", invocation.line).as_deref(),
            Some("common")
        );
        assert!(scope.conditionally_assigned_before("FILES", invocation.line));
    }

    #[test]
    fn a_seen_local_switch_has_make_empty_value_but_an_external_name_stays_unknown() {
        let joined = join_continuations(
            "FILES := common\n\
             #LOCAL_DISABLED=yes\n\
             ifeq ($(AROS_TARGET_CPU),x86_64)\n\
             LOCAL_CPU_FEATURE=yes\n\
             endif\n\
             ifeq ($(LOCAL_DISABLED),yes)\n\
             FILES += disabled-comment-option\n\
             endif\n\
             ifeq ($(LOCAL_CPU_FEATURE),yes)\n\
             FILES += cpu-feature\n\
             endif\n\
             %build_prog mmake=x progname=x files=$(FILES)\n",
        );
        let invocation = macro_invocations(&joined).remove(0);
        let arm = collect_vars_with_context(&joined, &target_context("arm", "raspi", "hard"));
        assert_eq!(
            arm.raw_at("FILES", invocation.line).as_deref(),
            Some("common")
        );
        assert!(!arm.conditionally_assigned_before("FILES", invocation.line));

        let external = join_continuations(
            "FILES := common\n\
             ifeq ($(EXTERNAL_CONFIG),yes)\n\
             FILES += configured\n\
             endif\n\
             %build_prog mmake=x progname=x files=$(FILES)\n",
        );
        let invocation = macro_invocations(&external).remove(0);
        let arm = collect_vars_with_context(&external, &target_context("arm", "raspi", "hard"));
        assert!(arm.conditionally_assigned_before("FILES", invocation.line));
    }

    #[test]
    fn target_context_evaluates_local_constants_and_make_filters() {
        let joined = join_continuations(
            "DEBUG_ACPI := no\n\
             FILES := common\n\
             ifeq ($(DEBUG_ACPI),yes)\n\
             FILES += debug\n\
             else\n\
             FILES += release\n\
             endif\n\
             ifneq (,$(filter arm aarch64,$(AROS_TARGET_CPU)))\n\
             FILES += arm-family\n\
             endif\n\
             %build_prog mmake=x progname=x files=$(FILES)\n",
        );
        let invocation = macro_invocations(&joined).remove(0);
        let scope = collect_vars_with_context(&joined, &target_context("aarch64", "raspi", ""));
        assert_eq!(
            scope.raw_at("FILES", invocation.line).as_deref(),
            Some("common release arm-family")
        );
        assert!(!scope.conditionally_assigned_before("FILES", invocation.line));
    }

    #[test]
    fn a_conditional_assignment_does_not_overwrite_an_existing_value() {
        let scope = collect_vars("A := first\nA ?= second\n%build_prog mmake=x progname=X\n");
        assert_eq!(scope.raw_at("A", usize::MAX).as_deref(), Some("first"));
    }

    #[test]
    fn a_posix_simple_assignment_is_not_mistaken_for_colon_equals() {
        let scope = collect_vars("A ::= immediate\n%build_prog mmake=x progname=X\n");
        assert_eq!(scope.raw_at("A", usize::MAX).as_deref(), Some("immediate"));
    }

    #[test]
    fn an_assignment_comment_is_not_a_list_item() {
        let scope = collect_vars(
            "FILES := SerialClass SerialUnitClass #unix_funcs\n\
             %build_module mmake=x modname=x files=$(FILES)\n",
        );
        assert_eq!(
            scope.raw_at("FILES", usize::MAX).as_deref(),
            Some("SerialClass SerialUnitClass")
        );
    }

    #[test]
    fn a_continued_list_is_one_assignment() {
        let src = "QPARTFILES  := \\\n    QP_Main \\\n    QP_Gui\n%build_prog mmake=m progname=M files=$(QPARTFILES)\n";
        let joined = join_continuations(src);
        let scope = collect_vars(&joined);
        let invs = macro_invocations(&joined);
        assert_eq!(
            scope.snapshot(invs[0].line).get("QPARTFILES").unwrap(),
            &vec!["QP_Main".to_owned(), "QP_Gui".to_owned()]
        );
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
    fn known_dynamic_meta_target_variables_become_cmake_references() {
        assert_eq!(
            render_meta_token("iconset-$(AROS_TARGET_ICONSET)-wbench-icons").unwrap(),
            "iconset-${AROS_TARGET_ICONSET}-wbench-icons"
        );
        assert_eq!(
            render_meta_token("includes-$(ARCH)-$(CPU)").unwrap(),
            "includes-${AROS_TARGET_PLATFORM}-${AROS_TARGET_CPU}"
        );
        assert_eq!(
            render_meta_token("distfiles-$(AROS_TARGET_PLATFORM)").unwrap(),
            "distfiles-${AROS_TARGET_LEGACY_PLATFORM}"
        );
        assert_eq!(
            render_meta_token("grub2-efi32-$(AROS_TARGET_CPU32)-quick").unwrap(),
            "grub2-efi32-${AROS_TARGET_CPU32}-quick"
        );
        assert!(render_meta_token("target-$(SOMETHING_UNKNOWN)").is_none());
    }

    #[test]
    fn an_empty_meta_rule_does_not_consume_the_next_make_rule() {
        let source = "#MM setup-ppc :\nsetup-ppc : preplink\n";
        let joined = join_mm_continuations(source);
        assert!(META_RULE_RE.captures_iter(&joined).next().is_none());
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
        assert_eq!(
            crate::sources::resolve_name("$(EXE)", &vars).unwrap(),
            "OpenURL"
        );
        assert_eq!(
            crate::sources::resolve_name("mesa3dgl$(EXE)", &vars).unwrap(),
            "mesa3dglOpenURL"
        );
    }

    #[test]
    fn an_unresolvable_name_is_refused() {
        let vars = std::collections::HashMap::new();
        assert!(crate::sources::resolve_name("$(EXENAME)", &vars).is_none());
        // A variable holding a list cannot name one target.
        let mut many = std::collections::HashMap::new();
        many.insert("L".to_owned(), vec!["a".to_owned(), "b".to_owned()]);
        assert!(crate::sources::resolve_name("$(L)", &many).is_none());
    }

    #[test]
    fn all_four_source_lists_are_read() {
        // developer/debug/test/cplusplus declares files="" cxxfiles="exception".
        let vars = std::collections::HashMap::new();
        let (srcs, declared) = crate::sources::macro_sources(
            r#"mmake=x progname=exception files="" cxxfiles="exception""#,
            &vars,
        );
        assert!(declared);
        assert_eq!(srcs, vec!["exception"]);
    }

    #[test]
    fn nothing_declared_is_distinct_from_nothing_resolved() {
        let vars = std::collections::HashMap::new();
        let (srcs, declared) = crate::sources::macro_sources("mmake=x progname=p", &vars);
        assert!(srcs.is_empty());
        assert!(!declared, "no list was given at all");

        let (srcs, declared) = crate::sources::macro_sources("mmake=x files=$(UNKNOWN)", &vars);
        assert!(srcs.is_empty());
        assert!(declared, "a list was given but did not resolve");
    }

    #[test]
    fn strict_expression_fallback_keeps_language_lanes_and_rejects_conditions() {
        let root = root();
        let dirs = dirs();
        let joined = join_continuations(
            "PORTROOT := $(PORTSDIR)/fixture\n\
             CFILES := one two\n\
             CXXFILES := three four\n\
             %build_linklib mmake=ok libname=ok \\\n+                 files=\"$(addprefix $(PORTROOT)/,$(CFILES))\" \\\n+                 cxxfiles=\"$(addprefix $(PORTROOT)/,$(CXXFILES))\"\n",
        );
        let scope = collect_vars(&joined);
        let invocation = macro_invocations(&joined).remove(0);
        let legacy = scope.snapshot(invocation.line);
        let context =
            MakeExprContext::new(&scope, &dirs, invocation.line, &root, Path::new("fixture"));
        let sources = evaluate_macro_sources(&invocation.args, &legacy, &context).unwrap();
        assert_eq!(
            sources.c,
            [
                "${AROS_PORTS_DIR}/fixture/one",
                "${AROS_PORTS_DIR}/fixture/two"
            ]
        );
        assert_eq!(
            sources.cxx,
            [
                "${AROS_PORTS_DIR}/fixture/three",
                "${AROS_PORTS_DIR}/fixture/four"
            ]
        );

        let conditional = join_continuations(
            "FILES := common\n\
             ifeq ($(ARCH),pc)\n\
             FILES += pc-only\n\
             endif\n\
             %build_linklib mmake=unsafe libname=unsafe \\\n+                 files=\"$(addprefix source/,$(FILES))\"\n",
        );
        let scope = collect_vars(&conditional);
        let invocation = macro_invocations(&conditional).remove(0);
        let legacy = scope.snapshot(invocation.line);
        let context =
            MakeExprContext::new(&scope, &dirs, invocation.line, &root, Path::new("fixture"));
        let error = evaluate_macro_sources(&invocation.args, &legacy, &context).unwrap_err();
        assert!(error.contains("unevaluated Make conditional"), "{error}");

        let partial = join_continuations(
            "FILES := common\n\
             ifeq ($(ARCH),pc)\n\
             FILES += pc-only\n\
             else\n\
             FILES += arm-only\n\
             endif\n\
             %build_linklib mmake=legacy libname=legacy \\\n+                 files=$(FILES) cxxfiles=$(UNKNOWN_CXX)\n",
        );
        let scope = collect_vars(&partial);
        let invocation = macro_invocations(&partial).remove(0);
        let legacy = scope.snapshot(invocation.line);
        let context =
            MakeExprContext::new(&scope, &dirs, invocation.line, &root, Path::new("fixture"));
        let error = evaluate_macro_sources(&invocation.args, &legacy, &context).unwrap_err();
        assert!(error.contains("unevaluated Make conditional"), "{error}");

        let mixed = join_continuations(
            "FILES := common\n\
             %build_linklib mmake=legacy libname=legacy \\\n+                 files=$(FILES) cxxfiles=$(UNKNOWN_CXX)\n",
        );
        let scope = collect_vars(&mixed);
        let invocation = macro_invocations(&mixed).remove(0);
        let legacy = scope.snapshot(invocation.line);
        let context =
            MakeExprContext::new(&scope, &dirs, invocation.line, &root, Path::new("fixture"));
        let sources = evaluate_macro_sources(&invocation.args, &legacy, &context).unwrap();
        assert_eq!(sources.c, ["common"]);
        assert!(sources.cxx.is_empty());
        assert_eq!(sources.diagnostics.len(), 1, "{:#?}", sources.diagnostics);
        assert!(sources.diagnostics[0].contains("UNKNOWN_CXX"));
    }

    #[test]
    fn module_declarations_receive_the_checked_mesa_contract() {
        let root = root();
        for (mmakefile, mmake) in [
            ("workbench/hidds/gallium/mmakefile.src", "hidd-gallium"),
            (
                "workbench/libs/gallium/mmakefile.src",
                "workbench-libs-gallium",
            ),
        ] {
            let parsed = super::parse_mmakefile_with_dirs_and_context(
                &root.join(mmakefile),
                &root,
                &dirs(),
                &target_context("arm", "raspi", "hard"),
            )
            .unwrap();
            let target = parsed
                .targets
                .iter()
                .find(|target| target.mmake_name == mmake)
                .expect("Gallium module target");
            assert!(target
                .include_dirs
                .contains(&"${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/include".to_owned()));
            assert!(target
                .defines
                .contains(&"USE_GCC_ATOMIC_BUILTINS".to_owned()));
            assert!(target
                .compile_options
                .contains(&"-fno-strict-aliasing".to_owned()));
        }
    }

    #[test]
    fn freetype_keeps_independent_prefixed_source_fragments() {
        let root = root();
        let parsed = super::parse_mmakefile_with_dirs_and_context(
            &root.join("workbench/libs/freetype2/mmakefile.src"),
            &root,
            &dirs(),
            &target_context("x86_64", "pc", ""),
        )
        .unwrap();
        let target = parsed
            .targets
            .iter()
            .find(|target| target.mmake_name == "workbench-libs-freetype-linklib")
            .expect("the independently resolvable FT2 source block must retain the target");
        assert!(!target.source_files.is_empty());
        assert!(target
            .source_files
            .iter()
            .all(|source| source.starts_with("${AROS_PORTS_DIR}/freetype2/freetype-2.14.3/src/")));
        assert!(target
            .source_files
            .iter()
            .any(|source| source.ends_with("/gzip/ftgzip")));
        assert!(!target
            .source_files
            .iter()
            .any(|source| source == "gzip/ftgzip"));
        assert!(parsed.partial_source_lists.iter().any(|diagnostic| {
            diagnostic.contains("workbench-libs-freetype-linklib")
                && diagnostic.contains("omitted unresolved source fragment")
        }));
        assert!(parsed.source_inventory_patterns.iter().any(|pattern| {
            pattern == "${AROS_PORTS_DIR}/freetype2/freetype-2.14.3/builds/aros/src/base/*.c"
        }));
    }

    #[test]
    fn mesa_included_config_resolves_fetch_and_public_headers_for_all_profiles() {
        let root = root();
        let dirs = dirs();
        let file = root.join("workbench/libs/mesa/mmakefile.src");

        for (cpu, platform, float_abi) in [
            ("x86_64", "pc", ""),
            ("arm", "raspi", "hard"),
            ("aarch64", "raspi", ""),
        ] {
            let parsed = super::parse_mmakefile_with_dirs_and_context(
                &file,
                &root,
                &dirs,
                &target_context(cpu, platform, float_abi),
            )
            .unwrap();

            assert!(
                parsed.skipped_fetches.is_empty(),
                "{cpu}: {:#?}",
                parsed.skipped_fetches
            );
            assert!(
                parsed.skipped_copy_includes.is_empty(),
                "{cpu}: {:#?}",
                parsed.skipped_copy_includes
            );
            assert_eq!(parsed.fetches.len(), 3, "{cpu}");
            let fetch = parsed
                .fetches
                .iter()
                .find(|fetch| fetch.name == "mesa3d-fetch")
                .unwrap();
            assert_eq!(fetch.name, "mesa3d-fetch");
            assert_eq!(fetch.archive, "mesa-20.0.8");
            assert_eq!(fetch.suffixes, "tar.xz tar.gz");
            assert_eq!(fetch.destination, "${AROS_PORTS_DIR}/mesa");
            assert_eq!(fetch.location, "${AROS_PORTS_SOURCE_DIR}");
            assert!(fetch.origins.ends_with("older-versions/20.x"));
            assert_eq!(fetch.patches, "mesa-20.0.8-aros.diff:mesa-20.0.8:-p1");
            for (name, archive, origin) in [
                (
                    "mesa3d-mako-fetch",
                    "mako-1.3.10",
                    "https://files.pythonhosted.org/packages/9e/38/bd5b78a920a64d708fe6bc8e0a2c075e1389d53bef8413725c63ba041535",
                ),
                (
                    "mesa3d-markupsafe-fetch",
                    "markupsafe-3.0.2",
                    "https://files.pythonhosted.org/packages/b2/97/5d42485e71dfc078108a86d6de8fa46db44a1a9295e89c5d6d4a06e23a62",
                ),
            ] {
                let package = parsed
                    .fetches
                    .iter()
                    .find(|fetch| fetch.name == name)
                    .unwrap();
                assert_eq!(package.archive, archive);
                assert_eq!(package.suffixes, "tar.gz");
                assert_eq!(package.origins, origin);
                assert_eq!(package.destination, "${AROS_PORTS_DIR}/mesa-python");
                assert_eq!(package.location, "${AROS_PORTS_SOURCE_DIR}");
                assert_eq!(package.patches, "::");
            }

            assert_eq!(parsed.copy_includes.len(), 4, "{cpu}");
            assert!(parsed
                .copy_includes
                .iter()
                .all(|copy| copy.name == "mesa3d-includes-copy" && copy.flatten));
            let headers: BTreeMap<_, _> = parsed
                .copy_includes
                .iter()
                .map(|copy| (copy.dest.as_str(), copy.patterns.as_slice()))
                .collect();
            assert_eq!(headers["GL"], ["gl.h", "glext.h"]);
            assert_eq!(headers["KHR"], ["khrplatform.h"]);
            assert_eq!(
                headers["EGL"],
                [
                    "egl.h",
                    "eglext.h",
                    "eglplatform.h",
                    "eglmesaext.h",
                    "eglextchromium.h"
                ]
            );
            assert_eq!(
                headers["vulkan"],
                ["vulkan.h", "vulkan_core.h", "vk_icd.h", "vk_platform.h"]
            );
            assert_eq!(
                parsed
                    .copy_includes
                    .iter()
                    .map(|copy| copy.patterns.len())
                    .sum::<usize>(),
                12
            );
            assert!(parsed.copy_includes.iter().all(|copy| copy
                .source_dir
                .starts_with("${AROS_PORTS_DIR}/mesa/mesa-20.0.8/include/")));
        }
    }

    #[test]
    fn real_cpu32_build_invocations_are_absent_on_arm_and_present_on_x86() {
        let root = root();
        let dirs = dirs();
        for (path, mmake) in [
            ("compiler/alib/mmakefile.src", "linklibs-amiga32"),
            (
                "compiler/arossupport/mmakefile.src",
                "linklibs-arossupport32",
            ),
            ("compiler/autoinit/mmakefile.src", "linklibs-autoinit32"),
        ] {
            let arm = super::parse_mmakefile_with_dirs_and_context(
                &root.join(path),
                &root,
                &dirs,
                &target_context("arm", "raspi", "hard"),
            )
            .unwrap();
            assert!(
                arm.targets.iter().all(|target| target.mmake_name != mmake),
                "{mmake} leaked into ARM"
            );

            let x86 = super::parse_mmakefile_with_dirs_and_context(
                &root.join(path),
                &root,
                &dirs,
                &target_context("x86_64", "pc", ""),
            )
            .unwrap();
            assert!(
                x86.targets.iter().any(|target| target.mmake_name == mmake),
                "{mmake} was lost on x86_64"
            );
        }
    }

    #[test]
    fn real_tree_e1_resolves_exactly_48_targets_without_merging_cxx_sources() {
        let root = root();
        let dirs = dirs();
        let files = [
            "developer/debug/test/freetype/mmakefile.src",
            "external/bz2/mmakefile.src",
            "tools/mkamikeymap/mmakefile.src",
            "workbench/classes/datatypes/heic/mmakefile.src",
            "workbench/classes/datatypes/jpegxl/mmakefile.src",
            "workbench/classes/datatypes/webp/mmakefile.src",
            "workbench/libs/codesets/mmakefile.src",
            "workbench/libs/expat/mmakefile.src",
            "workbench/libs/jpeg/mmakefile.src",
            "workbench/libs/lzma/mmakefile.src",
            "workbench/libs/utf8proc/mmakefile.src",
        ];
        let expected: BTreeSet<&str> = "
            test-freetype-lib-graph test-freetype-lib-common test-freetype-lib-ftcommon
            test-freetype-ftstring test-freetype-ftstring-static test-freetype-ftview
            test-freetype-ftview-static external-bz2-lib linklibs-bz2-nostdio
            external-bz2-bzip2-bin external-bz2-bzip2recover-bin tools-mkkeymap
            tools-mkamikeymap datatypes-heic-linklibs-de265 datatypes-heic-linklibs-heif
            datatypes-jpegxl-linklibs-brotli datatypes-jpegxl-linklibs-hwy
            datatypes-jpegxl-linklibs-jxl datatypes-webp-linklibs-webpdecode
            datatypes-webp-linklibs-webpencode datatypes-webp-linklibs-webputils
            workbench-libs-codesets-library linklibs-codesets libcodesets-test-b64d
            libcodesets-test-b64e libcodesets-test-detectcodeset
            libcodesets-test-utf8tostrhook libcodesets-test-demo1 libcodesets-test-convert
            libcodesets-test-autoopen workbench-libs-expat-lib workbench-libs-expat-examples
            workbench-libs-jpeg workbench-libs-lzma-library linklibs-lzma
            workbench-libs-utf8proc-library linklibs-utf8proc
            workbench-libs-utf8proc-tests-case workbench-libs-utf8proc-tests-charwidth
            workbench-libs-utf8proc-tests-custom workbench-libs-utf8proc-tests-grapheme
            workbench-libs-utf8proc-tests-iscase workbench-libs-utf8proc-tests-iterate
            workbench-libs-utf8proc-tests-maxdecomposition workbench-libs-utf8proc-tests-misc
            workbench-libs-utf8proc-tests-norm workbench-libs-utf8proc-tests-printproperty
            workbench-libs-utf8proc-tests-valid
        "
        .split_whitespace()
        .collect();
        assert_eq!(expected.len(), 48);

        let mut targets = BTreeMap::new();
        for file in files {
            let parsed = super::parse_mmakefile_with_dirs(&root.join(file), &root, &dirs).unwrap();
            for target in parsed.targets {
                if expected.contains(target.mmake_name.as_str()) {
                    targets.insert(target.mmake_name.clone(), target);
                }
            }
        }
        assert_eq!(
            targets.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            expected
        );

        let cxx_targets: BTreeSet<&str> = targets
            .iter()
            .filter(|(_, target)| !target.cxx_source_files.is_empty())
            .map(|(name, _)| name.as_str())
            .collect();
        assert_eq!(
            cxx_targets,
            BTreeSet::from([
                "datatypes-heic-linklibs-de265",
                "datatypes-heic-linklibs-heif",
                "datatypes-jpegxl-linklibs-hwy",
                "datatypes-jpegxl-linklibs-jxl",
            ])
        );
        assert_eq!(
            targets["datatypes-heic-linklibs-de265"]
                .cxx_source_files
                .len(),
            34
        );
        assert_eq!(
            targets["datatypes-heic-linklibs-heif"]
                .cxx_source_files
                .len(),
            119
        );
        assert_eq!(
            targets["datatypes-jpegxl-linklibs-hwy"]
                .cxx_source_files
                .len(),
            7
        );
        assert_eq!(
            targets["datatypes-jpegxl-linklibs-jxl"]
                .cxx_source_files
                .len(),
            76
        );

        let port_targets = targets
            .values()
            .filter(|target| {
                target
                    .source_files
                    .iter()
                    .chain(&target.cxx_source_files)
                    .any(|source| source.starts_with("${AROS_PORTS_DIR}/"))
            })
            .count();
        assert_eq!(port_targets, 46);
        assert!(targets.values().all(|target| target
            .source_files
            .iter()
            .chain(&target.cxx_source_files)
            .all(|source| !source.contains("/Volumes/Dev/"))));
    }

    #[test]
    fn concrete_profiles_keep_core_conditional_targets_and_select_png_sources() {
        let root = root();
        let dirs = dirs();
        let files = [
            "arch/all-hosted/filesys/emul_handler/mmakefile.src",
            "arch/all-native/acpica/mmakefile.src",
            "arch/all-unix/hidd/unixio/mmakefile.src",
            "arch/arm-all/arm-aeabi/mmakefile.src",
            "rom/kernel/mmakefile.src",
            "workbench/libs/png/mmakefile.src",
        ];
        let expected: BTreeSet<&str> = BTreeSet::from([
            "kernel-fs-emul",
            "kernel-acpica-sharedlib",
            "kernel-unixio",
            "linklibs-aeabi",
            "kernel-kernel",
            "workbench-libs-png",
            "linklibs-png-nostdio",
        ]);

        for (cpu, platform, float_abi) in [
            ("x86_64", "pc", ""),
            ("arm", "raspi", "hard"),
            ("aarch64", "raspi", ""),
        ] {
            let target = target_context(cpu, platform, float_abi);
            let mut parsed_targets = BTreeMap::new();
            let mut skipped = Vec::new();
            for file in files {
                let parsed = super::parse_mmakefile_with_dirs_and_context(
                    &root.join(file),
                    &root,
                    &dirs,
                    &target,
                )
                .unwrap();
                skipped.extend(parsed.skipped_programs);
                for parsed_target in parsed.targets {
                    if expected.contains(parsed_target.mmake_name.as_str()) {
                        parsed_targets.insert(parsed_target.mmake_name.clone(), parsed_target);
                    }
                }
            }
            assert_eq!(
                parsed_targets
                    .keys()
                    .map(String::as_str)
                    .collect::<BTreeSet<_>>(),
                expected,
                "{cpu}-{platform}: {skipped:#?}"
            );

            let png = &parsed_targets["workbench-libs-png"].source_files;
            assert_eq!(
                png.iter().any(|source| source.contains("intel/")),
                cpu == "x86_64",
                "{cpu}-{platform} selected the wrong Intel PNG branch"
            );
            assert_eq!(
                png.iter().any(|source| source.contains("arm/")),
                cpu == "aarch64",
                "{cpu}-{platform} selected the wrong Arm PNG branch"
            );
            assert!(parsed_targets["kernel-kernel"]
                .source_files
                .iter()
                .any(|source| source == "kernel_mm"));
        }

        let arm = target_context("arm", "raspi", "hard");
        let aeabi = super::parse_mmakefile_with_dirs_and_context(
            &root.join("arch/arm-all/arm-aeabi/mmakefile.src"),
            &root,
            &dirs,
            &arm,
        )
        .unwrap();
        let aeabi = aeabi
            .targets
            .iter()
            .find(|target| target.mmake_name == "linklibs-aeabi")
            .unwrap();
        assert!(aeabi.source_files.iter().any(|source| source == "i2d"));
        assert!(!aeabi
            .source_files
            .iter()
            .any(|source| source == "softfloat"));

        let kernel_file = root.join("rom/kernel/mmakefile.src");
        let mut no_mmu = target_context("x86_64", "pc", "");
        no_mmu.use_mmu = Some("0".to_owned());
        let kernel =
            super::parse_mmakefile_with_dirs_and_context(&kernel_file, &root, &dirs, &no_mmu)
                .unwrap();
        let kernel = kernel
            .targets
            .iter()
            .find(|target| target.mmake_name == "kernel-kernel")
            .unwrap();
        assert!(kernel
            .source_files
            .iter()
            .all(|source| source != "kernel_mm"));

        let mut unknown_mmu = target_context("x86_64", "pc", "");
        unknown_mmu.use_mmu = None;
        let kernel =
            super::parse_mmakefile_with_dirs_and_context(&kernel_file, &root, &dirs, &unknown_mmu)
                .unwrap();
        assert!(kernel
            .targets
            .iter()
            .all(|target| target.mmake_name != "kernel-kernel"));
        assert!(kernel
            .skipped_programs
            .iter()
            .any(|diagnostic| diagnostic.contains("unevaluated Make conditional")));
    }

    #[test]
    fn btcore_plain_local_source_inventory_is_real_in_all_current_profiles() {
        let root = root();
        let dirs = dirs();
        let file = root.join("rom/bluetooth/stack/mmakefile.src");
        for (cpu, platform, float_abi) in [
            ("x86_64", "pc", ""),
            ("arm", "raspi", "hard"),
            ("aarch64", "raspi", ""),
        ] {
            let parsed = super::parse_mmakefile_with_dirs_and_context(
                &file,
                &root,
                &dirs,
                &target_context(cpu, platform, float_abi),
            )
            .unwrap();
            let btcore = parsed
                .targets
                .iter()
                .find(|target| target.mmake_name == "linklibs-btcore")
                .unwrap_or_else(|| panic!("{cpu}-{platform}: {:#?}", parsed.skipped_programs));
            assert_eq!(btcore.module_type, ModuleType::LinkLib);
            assert_eq!(btcore.target_name, "btcore");
            assert_eq!(btcore.source_files.len(), 28, "{cpu}-{platform}");
            assert!(btcore
                .source_files
                .iter()
                .all(|source| source.starts_with("${CMAKE_SOURCE_DIR}/rom/bluetooth/stack/")));
            assert!(btcore
                .source_files
                .iter()
                .any(|source| source.ends_with("/core/security/smp_manager")));
            assert!(btcore
                .source_files
                .iter()
                .any(|source| source.ends_with("/aros/input_bridge")));
            assert!(parsed.skipped_local_make_includes.is_empty());
            assert!(parsed
                .skipped_programs
                .iter()
                .all(|message| !message.contains("linklibs-btcore")));
        }
    }

    #[test]
    fn zstd_plain_source_inventory_is_cold_fetch_exact_in_all_current_profiles() {
        let root = root();
        let dirs = dirs();
        let file = root.join("workbench/libs/zstd/mmakefile.src");
        let expected: Vec<String> = [
            "lib/common/debug",
            "lib/common/entropy_common",
            "lib/common/error_private",
            "lib/common/fse_decompress",
            "lib/common/pool",
            "lib/common/threading",
            "lib/common/xxhash",
            "lib/common/zstd_common",
            "lib/compress/fse_compress",
            "lib/compress/hist",
            "lib/compress/huf_compress",
            "lib/compress/zstd_compress",
            "lib/compress/zstd_compress_literals",
            "lib/compress/zstd_compress_sequences",
            "lib/compress/zstd_compress_superblock",
            "lib/compress/zstd_double_fast",
            "lib/compress/zstd_fast",
            "lib/compress/zstd_lazy",
            "lib/compress/zstd_ldm",
            "lib/compress/zstd_opt",
            "lib/compress/zstd_preSplit",
            "lib/compress/zstdmt_compress",
            "lib/decompress/huf_decompress",
            "lib/decompress/zstd_ddict",
            "lib/decompress/zstd_decompress",
            "lib/decompress/zstd_decompress_block",
            "lib/dictBuilder/cover",
            "lib/dictBuilder/divsufsort",
            "lib/dictBuilder/fastcover",
            "lib/dictBuilder/zdict",
        ]
        .into_iter()
        .map(|stem| format!("${{AROS_PORTS_DIR}}/zstd/zstd-1.5.7/{stem}"))
        .collect();

        for (cpu, platform, float_abi) in [
            ("x86_64", "pc", ""),
            ("arm", "raspi", "hard"),
            ("aarch64", "raspi", ""),
        ] {
            let parsed = super::parse_mmakefile_with_dirs_and_context(
                &file,
                &root,
                &dirs,
                &target_context(cpu, platform, float_abi),
            )
            .unwrap();
            let targets: BTreeMap<_, _> = parsed
                .targets
                .iter()
                .map(|target| (target.mmake_name.as_str(), target))
                .collect();

            let module = targets
                .get("workbench-libs-zstd-library")
                .unwrap_or_else(|| panic!("{cpu}-{platform}: {:#?}", parsed.skipped_programs));
            let static_lib = targets
                .get("linklibs-zstd")
                .unwrap_or_else(|| panic!("{cpu}-{platform}: {:#?}", parsed.skipped_programs));
            for target in [module, static_lib] {
                assert_eq!(
                    target.source_files, expected,
                    "{cpu}: {}",
                    target.mmake_name
                );
                assert_eq!(
                    target.include_dirs,
                    ["${CMAKE_SOURCE_DIR}/workbench/libs/zstd"],
                    "{cpu}: {}",
                    target.mmake_name
                );
                assert_eq!(
                    target.defines,
                    ["ZSTD_NO_TRACE"],
                    "{cpu}: {}",
                    target.mmake_name
                );
                assert!(
                    target.link_options.is_empty(),
                    "{cpu}: {}",
                    target.mmake_name
                );
            }

            assert_eq!(module.module_type, ModuleType::Library);
            assert_eq!(module.target_name, "zstd");
            assert_eq!(module.linklib_name.as_deref(), Some("zstd"));
            let genmodule = module.genmodule_linklibs.as_ref().unwrap();
            assert!(genmodule.enabled && genmodule.has_relative && genmodule.inputs_exact);
            assert_eq!(genmodule.relative_libraries, ["posixc", "stdc"]);
            assert!(genmodule.source_files.is_empty());
            assert!(genmodule.object_sources.is_empty());

            assert_eq!(static_lib.module_type, ModuleType::LinkLib);
            assert_eq!(static_lib.target_name, "zstd-static");
            assert!(static_lib.canonical_linklib_output);
            assert!(parsed
                .flags
                .spec_switches
                .iter()
                .any(|flag| flag == "static"));

            let copy = parsed
                .copy_includes
                .iter()
                .find(|copy| copy.name == "workbench-libs-zstd-includes-copy")
                .unwrap();
            assert_eq!(copy.dest, ".");
            assert_eq!(copy.source_dir, "${AROS_PORTS_DIR}/zstd/zstd-1.5.7/lib");
            assert_eq!(copy.patterns, ["zstd.h", "zstd_errors.h", "zdict.h"]);
            assert!(copy.flatten);

            let fetch = parsed
                .fetches
                .iter()
                .find(|fetch| fetch.name == "workbench-libs-zstd-fetch")
                .unwrap();
            assert_eq!(fetch.archive, "zstd-1.5.7");
            assert_eq!(fetch.destination, "${AROS_PORTS_DIR}/zstd");
            assert!(fetch.origins.contains("/v1.5.7"));
            assert!(parsed.skipped_local_make_includes.is_empty(), "{cpu}");
            assert!(parsed
                .skipped_programs
                .iter()
                .all(|message| !message.contains("workbench-libs-zstd-library")
                    && !message.contains("linklibs-zstd")));
        }
    }

    #[test]
    fn atheros_hal_literal_fragment_is_exact_in_all_current_profiles() {
        let root = root();
        let dirs = dirs();
        let file = root.join("workbench/devs/networks/atheros5000/hal/mmakefile.src");
        let expected_sources = [
            "ah",
            "ah_regdomain",
            "ah_eeprom_v3",
            "ah_eeprom_v14",
            "ah_eeprom_v4k",
            "ar5211/ar5211_attach",
            "ar5211/ar5211_beacon",
            "ar5211/ar5211_interrupts",
            "ar5211/ar5211_keycache",
            "ar5211/ar5211_misc",
            "ar5211/ar5211_power",
            "ar5211/ar5211_phy",
            "ar5211/ar5211_recv",
            "ar5211/ar5211_reset",
            "ar5211/ar5211_xmit",
            "ar5212/ar5212_attach",
            "ar5212/ar5212_beacon",
            "ar5212/ar5212_eeprom",
            "ar5212/ar5212_gpio",
            "ar5212/ar5212_interrupts",
            "ar5212/ar5212_keycache",
            "ar5212/ar5212_misc",
            "ar5212/ar5212_power",
            "ar5212/ar5212_phy",
            "ar5212/ar5212_recv",
            "ar5212/ar5212_reset",
            "ar5212/ar5212_xmit",
            "ar5212/ar5212_ani",
            "ar5212/ar5212_rfgain",
            "ar5416/ar5416_ani",
            "ar5416/ar5416_attach",
            "ar5416/ar5416_beacon",
            "ar5416/ar5416_cal",
            "ar5416/ar5416_cal_adcdc",
            "ar5416/ar5416_cal_adcgain",
            "ar5416/ar5416_cal_iq",
            "ar5416/ar5416_eeprom",
            "ar5416/ar5416_gpio",
            "ar5416/ar5416_interrupts",
            "ar5416/ar5416_keycache",
            "ar5416/ar5416_misc",
            "ar5416/ar5416_power",
            "ar5416/ar5416_phy",
            "ar5416/ar5416_recv",
            "ar5416/ar5416_reset",
            "ar5416/ar5416_xmit",
            "ar5416/ar9160_attach",
            "ar5416/ar9280_attach",
            "ar5416/ar9280",
            "ar5416/ar9285_attach",
            "ar5416/ar9285",
            "ar5416/ar9285_reset",
            "ar5212/ar2316",
            "ar5212/ar2317",
            "ar5416/ar2133",
            "ar5212/ar2413",
            "ar5212/ar2425",
            "ar5212/ar5111",
            "ar5212/ar5112",
            "ar5212/ar5413",
        ];
        let expected_definitions = [
            "AH_HAS_RF 1",
            "AH_SUPPORT_AR5211 1",
            "AH_SUPPORT_AR5212 1",
            "AH_SUPPORT_AR5416 1",
            "AH_SUPPORT_2316 1",
            "AH_SUPPORT_2317 1",
            "AH_SUPPORT_2133 1",
            "AH_SUPPORT_2413 1",
            "AH_SUPPORT_2417 1",
            "AH_SUPPORT_2425 1",
            "AH_SUPPORT_5111 1",
            "AH_SUPPORT_5112 1",
            "AH_SUPPORT_5413 1",
            "AH_ENABLE_FORCEBIAS 1",
        ];

        for (cpu, platform, float_abi) in [
            ("x86_64", "pc", ""),
            ("arm", "raspi", "hard"),
            ("aarch64", "raspi", ""),
        ] {
            let parsed = super::parse_mmakefile_with_dirs_and_context(
                &file,
                &root,
                &dirs,
                &target_context(cpu, platform, float_abi),
            )
            .unwrap();
            let hal = parsed
                .targets
                .iter()
                .find(|target| target.mmake_name == "workbench-devs-networks-atheros5000-hal")
                .unwrap_or_else(|| panic!("{cpu}: {:#?}", parsed.skipped_programs));
            assert_eq!(hal.module_type, ModuleType::LinkLib, "{cpu}");
            assert_eq!(hal.target_name, "athhal", "{cpu}");
            assert_eq!(hal.source_files, expected_sources, "{cpu}");
            assert!(hal.cxx_source_files.is_empty(), "{cpu}");
            assert!(hal.objc_source_files.is_empty(), "{cpu}");
            assert!(hal.asm_source_files.is_empty(), "{cpu}");

            assert_eq!(parsed.define_headers.len(), 1, "{cpu}");
            let header = &parsed.define_headers[0];
            assert_eq!(
                header.owner, "workbench-devs-networks-atheros5000-hal-opts",
                "{cpu}"
            );
            assert_eq!(header.provider, hal.mmake_name, "{cpu}");
            assert_eq!(
                header.output, "${AROS_BUILD_DIR}/workbench/devs/networks/atheros5000/hal/opt_ah.h",
                "{cpu}"
            );
            assert_eq!(header.definitions, expected_definitions, "{cpu}");
            assert_eq!(
                header.file, "workbench/devs/networks/atheros5000/hal/Makefile.inc",
                "{cpu}"
            );
            assert_eq!(header.line, 265, "{cpu}");
            assert_eq!(
                header.dependencies,
                [
                    "${CMAKE_SOURCE_DIR}/workbench/devs/networks/atheros5000/hal/Makefile.inc",
                    "${CMAKE_SOURCE_DIR}/workbench/devs/networks/atheros5000/hal/mmakefile.src",
                ],
                "{cpu}"
            );
            assert!(header.consumers.is_empty(), "{cpu}");
            assert!(parsed.skipped_local_make_includes.is_empty(), "{cpu}");
            assert!(parsed.partial_source_lists.is_empty(), "{cpu}");
            assert!(parsed.generated_file_rules.is_empty(), "{cpu}");
            assert!(parsed.adhoc_header_rules.is_empty(), "{cpu}");
            assert!(
                parsed
                    .skipped_programs
                    .iter()
                    .all(|message| !message.contains(&hal.mmake_name)),
                "{cpu}"
            );
        }
    }

    #[test]
    fn literal_define_header_adoption_rejects_output_traversal() {
        let tree = TempTree::new();
        fs::create_dir_all(tree.0.join("module")).unwrap();
        fs::write(tree.0.join("module/one.c"), "int one;\n").unwrap();
        fs::write(
            tree.0.join("module/options.mk"),
            "FILES := one\n$(OUT):\n\techo \"#define SAFE 1\" >escape.h\n",
        )
        .unwrap();
        fs::write(
            tree.0.join("module/mmakefile.src"),
            "OUT := $(TOP)/$(CURDIR)/../escape.h\ninclude $(SRCDIR)/$(CURDIR)/options.mk\n%build_linklib mmake=provider libname=provider files=\"$(FILES)\"\n#MM\nprovider-opts: $(OUT)\n",
        )
        .unwrap();

        let parsed = super::parse_mmakefile_with_dirs_and_context(
            &tree.0.join("module/mmakefile.src"),
            &tree.0,
            &DirVars::load(&tree.0),
            &target_context("x86_64", "pc", ""),
        )
        .unwrap();
        assert!(parsed.define_headers.is_empty());
        assert!(parsed
            .targets
            .iter()
            .all(|target| target.mmake_name != "provider"));
        assert!(!parsed.skipped_local_make_includes.is_empty());
    }

    #[test]
    fn literal_define_fragment_cannot_change_non_source_build_properties() {
        for escaped_use in [
            "USER_CFLAGS += -DFRAGMENT_MODE=$(MODE)\n",
            "%build_linklib mmake=provider libname=provider files=\"$(FILES)\" libdir=$(MODE)\n",
        ] {
            let tree = TempTree::new();
            fs::create_dir_all(tree.0.join("module")).unwrap();
            fs::write(tree.0.join("module/one.c"), "int one;\n").unwrap();
            fs::write(
                tree.0.join("module/options.mk"),
                "FILES := one\nMODE := private\n$(OUT):\n\techo \"#define SAFE 1\" >options.h\n",
            )
            .unwrap();
            let declaration = if escaped_use.starts_with("%build_linklib") {
                escaped_use.to_owned()
            } else {
                format!(
                    "{escaped_use}%build_linklib mmake=provider libname=provider files=\"$(FILES)\"\n"
                )
            };
            fs::write(
                tree.0.join("module/mmakefile.src"),
                format!(
                    "OUT := $(TOP)/$(CURDIR)/options.h\ninclude $(SRCDIR)/$(CURDIR)/options.mk\n{declaration}#MM\nprovider-opts: $(OUT)\n"
                ),
            )
            .unwrap();

            let parsed = super::parse_mmakefile_with_dirs_and_context(
                &tree.0.join("module/mmakefile.src"),
                &tree.0,
                &DirVars::load(&tree.0),
                &target_context("x86_64", "pc", ""),
            )
            .unwrap();
            assert!(parsed.define_headers.is_empty(), "{escaped_use}");
            assert!(
                parsed
                    .targets
                    .iter()
                    .all(|target| target.mmake_name != "provider"),
                "{escaped_use}"
            );
            assert!(
                !parsed.skipped_local_make_includes.is_empty(),
                "{escaped_use}"
            );
        }

        // Global Make controls are consumed by the reference templates even
        // without a textual reference in this mmakefile. They therefore must
        // not enter the otherwise closed source/header fragment scope.
        let tree = TempTree::new();
        fs::create_dir_all(tree.0.join("module")).unwrap();
        fs::write(tree.0.join("module/one.c"), "int one;\n").unwrap();
        fs::write(
            tree.0.join("module/options.mk"),
            "FILES := one\nCFLAGS := -O0\n$(OUT):\n\techo \"#define SAFE 1\" >options.h\n",
        )
        .unwrap();
        fs::write(
            tree.0.join("module/mmakefile.src"),
            "OUT := $(TOP)/$(CURDIR)/options.h\ninclude $(SRCDIR)/$(CURDIR)/options.mk\n%build_linklib mmake=provider libname=provider files=\"$(FILES)\"\n#MM\nprovider-opts: $(OUT)\n",
        )
        .unwrap();
        let parsed = super::parse_mmakefile_with_dirs_and_context(
            &tree.0.join("module/mmakefile.src"),
            &tree.0,
            &DirVars::load(&tree.0),
            &target_context("x86_64", "pc", ""),
        )
        .unwrap();
        assert!(parsed.define_headers.is_empty());
        assert!(parsed
            .targets
            .iter()
            .all(|target| target.mmake_name != "provider"));

        // Even a variable which genuinely controls a header branch cannot be
        // an ambient template property or one of the provider's implicit
        // private variables. Backward product closure alone is deliberately
        // insufficient for these names.
        for control in [
            "TARGET_CC",
            "TARGET_SYSROOT",
            "TARGET_LTO",
            "SAFETY_CFLAGS",
            "CFLAGS_IQUOTE_END",
            "AR",
            "RANLIB",
            "provider_FILES",
            "provider_OBJDIR",
            "provider_C_FILES",
        ] {
            let tree = TempTree::new();
            fs::create_dir_all(tree.0.join("module")).unwrap();
            fs::write(tree.0.join("module/one.c"), "int one;\n").unwrap();
            fs::write(
                tree.0.join("module/options.mk"),
                format!(
                    "FILES := one\n{control} := 1\n$(OUT):\n\techo \"#define SAFE 1\" >options.h\nifeq ($({control}),1)\n\techo \"#define SELECTED 1\" >>options.h\nendif\n"
                ),
            )
            .unwrap();
            fs::write(
                tree.0.join("module/mmakefile.src"),
                "OUT := $(TOP)/$(CURDIR)/options.h\ninclude $(SRCDIR)/$(CURDIR)/options.mk\n%build_linklib mmake=provider libname=provider files=\"$(FILES)\"\n#MM\nprovider-opts: $(OUT)\n",
            )
            .unwrap();
            let parsed = super::parse_mmakefile_with_dirs_and_context(
                &tree.0.join("module/mmakefile.src"),
                &tree.0,
                &DirVars::load(&tree.0),
                &target_context("x86_64", "pc", ""),
            )
            .unwrap();
            assert!(parsed.define_headers.is_empty(), "{control}");
            assert!(
                parsed
                    .targets
                    .iter()
                    .all(|target| target.mmake_name != "provider"),
                "{control}"
            );
        }

        // An innocuous unused assignment is outside both permitted product
        // closures and is rejected without trying to enumerate every way a
        // future Make template might consume it.
        let tree = TempTree::new();
        fs::create_dir_all(tree.0.join("module")).unwrap();
        fs::write(tree.0.join("module/one.c"), "int one;\n").unwrap();
        fs::write(
            tree.0.join("module/options.mk"),
            "FILES := one\nUNUSED_FEATURE := 1\n$(OUT):\n\techo \"#define SAFE 1\" >options.h\n",
        )
        .unwrap();
        fs::write(
            tree.0.join("module/mmakefile.src"),
            "OUT := $(TOP)/$(CURDIR)/options.h\ninclude $(SRCDIR)/$(CURDIR)/options.mk\n%build_linklib mmake=provider libname=provider files=\"$(FILES)\"\n#MM\nprovider-opts: $(OUT)\n",
        )
        .unwrap();
        let parsed = super::parse_mmakefile_with_dirs_and_context(
            &tree.0.join("module/mmakefile.src"),
            &tree.0,
            &DirVars::load(&tree.0),
            &target_context("x86_64", "pc", ""),
        )
        .unwrap();
        assert!(parsed.define_headers.is_empty());
        assert!(parsed
            .targets
            .iter()
            .all(|target| target.mmake_name != "provider"));
    }

    #[test]
    fn literal_define_header_rejects_duplicate_active_macro_names() {
        let tree = TempTree::new();
        fs::create_dir_all(tree.0.join("module")).unwrap();
        fs::write(tree.0.join("module/one.c"), "int one;\n").unwrap();
        fs::write(
            tree.0.join("module/options.mk"),
            "FILES := one\n$(OUT):\n\techo \"#define SAFE 1\" >options.h\n\techo \"#define SAFE 2\" >>options.h\n",
        )
        .unwrap();
        fs::write(
            tree.0.join("module/mmakefile.src"),
            "OUT := $(TOP)/$(CURDIR)/options.h\ninclude $(SRCDIR)/$(CURDIR)/options.mk\n%build_linklib mmake=provider libname=provider files=\"$(FILES)\"\n#MM\nprovider-opts: $(OUT)\n",
        )
        .unwrap();
        let parsed = super::parse_mmakefile_with_dirs_and_context(
            &tree.0.join("module/mmakefile.src"),
            &tree.0,
            &DirVars::load(&tree.0),
            &target_context("x86_64", "pc", ""),
        )
        .unwrap();
        assert!(parsed.define_headers.is_empty());
        assert!(parsed
            .targets
            .iter()
            .all(|target| target.mmake_name != "provider"));
    }

    #[test]
    fn zlib_port_scope_is_declaration_owned_and_profile_exact() {
        let root = root();
        let dirs = dirs();
        let file = root.join("workbench/libs/z/mmakefile.src");
        for (cpu, platform, float_abi, source_count) in [
            ("x86_64", "pc", "", 21),
            ("arm", "raspi", "hard", 15),
            ("aarch64", "raspi", "", 20),
        ] {
            let parsed = super::parse_mmakefile_with_dirs_and_context(
                &file,
                &root,
                &dirs,
                &target_context(cpu, platform, float_abi),
            )
            .unwrap();
            let targets: BTreeMap<_, _> = parsed
                .targets
                .iter()
                .map(|target| (target.mmake_name.as_str(), target))
                .collect();

            for mmake in [
                "workbench-libs-z",
                "linklibs-z-static",
                "linklibs-z-nogzip-static",
            ] {
                let target = targets.get(mmake).unwrap_or_else(|| {
                    panic!(
                        "{cpu}-{platform}: missing {mmake}: {:#?}",
                        parsed.skipped_programs
                    )
                });
                assert_eq!(target.source_files.len(), source_count, "{cpu}: {mmake}");
                assert!(target.source_files.iter().all(|source| source.starts_with(
                    "${AROS_PORTS_DIR}/zlib/chromium-da752eb2a3660cf1bf8dac620f6380b89dd953a7/"
                )));
                assert_eq!(
                    target.include_dirs,
                    ["${AROS_PORTS_DIR}/zlib/chromium-da752eb2a3660cf1bf8dac620f6380b89dd953a7"],
                    "{cpu}: {mmake}"
                );
                assert_eq!(target.link_options, ["-lpthread"], "{cpu}: {mmake}");
            }

            let module = targets["workbench-libs-z"];
            assert_eq!(module.linklib_name.as_deref(), Some("z"));
            let genmodule = module.genmodule_linklibs.as_ref().unwrap();
            assert!(genmodule.enabled && genmodule.has_relative);
            assert!(genmodule.inputs_exact);
            assert_eq!(genmodule.relative_libraries, ["posixc", "stdc"]);
            assert!(genmodule.source_files.is_empty());
            assert!(genmodule.object_sources.is_empty());
            for define in ["_XOPEN_SOURCE=600", "STDC", "AMIGA"] {
                assert!(
                    module.defines.iter().any(|value| value == define),
                    "{cpu}: {define}"
                );
            }
            assert!(!module
                .defines
                .iter()
                .any(|value| { matches!(value.as_str(), "NO_STRERROR" | "NDEBUG" | "NO_GZIP") }));

            let static_lib = targets["linklibs-z-static"];
            assert!(static_lib
                .defines
                .iter()
                .any(|value| value == "NO_STRERROR"));
            assert!(static_lib.defines.iter().any(|value| value == "NDEBUG"));
            assert!(!static_lib.defines.iter().any(|value| value == "NO_GZIP"));

            let no_gzip = targets["linklibs-z-nogzip-static"];
            assert!(no_gzip.defines.iter().any(|value| value == "NO_GZIP"));
            assert!(static_lib.canonical_linklib_output, "{cpu}: z.static");
            assert!(no_gzip.canonical_linklib_output, "{cpu}: z-nogzip.static");
            assert!(!module.canonical_linklib_output);

            let minigzip = targets["workbench-libs-z-minigzip"];
            assert_eq!(
                minigzip.source_files,
                ["${AROS_PORTS_DIR}/zlib/chromium-da752eb2a3660cf1bf8dac620f6380b89dd953a7/test/minigzip"]
            );
            assert!(minigzip.defines.iter().any(|value| value == "NO_GZIP"));
            assert_eq!(minigzip.link_options, ["-lpthread"]);
            assert!(!minigzip.canonical_linklib_output);

            assert_eq!(parsed.header_transforms.len(), 1, "{cpu}: transforms");
            let fetch = parsed
                .fetches
                .iter()
                .find(|fetch| fetch.name == "zlib-fetch")
                .expect("production zlib fetch");
            assert_eq!(
                fetch.base,
                "${AROS_PORTS_DIR}/zlib/chromium-da752eb2a3660cf1bf8dac620f6380b89dd953a7"
            );
            assert_eq!(fetch.destination, fetch.base);
            assert!(fetch.location.contains("chromium-da752eb2a3660cf1"));
            assert!(!fetch.origins.contains("cache://"));
            assert!(fetch
                .origins
                .contains("da752eb2a3660cf1bf8dac620f6380b89dd953a7"));
            let transform = &parsed.header_transforms[0];
            assert_eq!(transform.name, "workbench-libs-z-geninc");
            assert_eq!(
                transform.input,
                "${AROS_PORTS_DIR}/zlib/chromium-da752eb2a3660cf1bf8dac620f6380b89dd953a7/zconf.h.chr"
            );
            assert_eq!(transform.output, "${AROS_SDK_INCLUDE_DIR}/zconf.h");
            assert!(parsed
                .adhoc_header_rules
                .iter()
                .all(|rule| rule.dest != "zconf.h"));

            let x86_define = module
                .defines
                .iter()
                .any(|value| value == "INFLATE_CHUNK_SIMD_SSE2");
            let arm64_define = module
                .defines
                .iter()
                .any(|value| value == "INFLATE_CHUNK_SIMD_NEON");
            assert_eq!(x86_define, cpu == "x86_64", "{cpu}: x86 flags");
            assert_eq!(arm64_define, cpu == "aarch64", "{cpu}: arm64 flags");
            assert_eq!(
                module.compile_options,
                if cpu == "aarch64" {
                    vec!["-march=armv8-a+crc+crypto".to_owned()]
                } else {
                    Vec::new()
                },
                "{cpu}: compile options"
            );

            assert!(parsed.skipped_local_make_includes.is_empty(), "{cpu}");
            assert!(parsed
                .skipped_programs
                .iter()
                .all(|message| !message.contains("workbench-libs-z")
                    && !message.contains("linklibs-z")));
        }
    }

    #[test]
    fn relative_zlib_dependencies_have_exact_full_module_archive_inputs() {
        let root = root();
        let dirs = dirs();
        for (relative, mmake, source_count, object_count) in [
            ("compiler/crt/posixc/mmakefile.src", "compiler-posixc", 8, 1),
            ("compiler/crt/stdc/mmakefile.src", "compiler-stdc", 9, 13),
        ] {
            let parsed = super::parse_mmakefile_with_dirs_and_context(
                &root.join(relative),
                &root,
                &dirs,
                &target_context("x86_64", "pc", ""),
            )
            .unwrap();
            let target = parsed
                .targets
                .iter()
                .find(|target| target.mmake_name == mmake)
                .unwrap();
            let genmodule = target.genmodule_linklibs.as_ref().unwrap();
            assert!(genmodule.has_relative, "{mmake}");
            assert!(
                genmodule.inputs_exact,
                "{mmake}: {:#?}",
                parsed.partial_source_lists
            );
            assert_eq!(genmodule.source_files.len(), source_count, "{mmake}");
            assert_eq!(genmodule.object_sources.len(), object_count, "{mmake}");
        }
    }

    #[test]
    fn broad_safe_fragment_without_a_fetch_owner_remains_deferred() {
        let tree = TempTree::new();
        fs::create_dir_all(tree.0.join("module")).unwrap();
        fs::write(
            tree.0.join("module/make.opt"),
            "ARCHSRCDIR := $(PORTSDIR)/unowned/src\nUSER_INCLUDES += -I$(ARCHSRCDIR)\n",
        )
        .unwrap();
        fs::write(
            tree.0.join("module/mmakefile.src"),
            "include $(SRCDIR)/$(CURDIR)/make.opt\nFILES := one two\n%build_linklib mmake=unowned libname=unowned files=\"$(addprefix $(ARCHSRCDIR)/,$(FILES))\"\n",
        )
        .unwrap();

        let parsed = super::parse_mmakefile_with_dirs_and_context(
            &tree.0.join("module/mmakefile.src"),
            &tree.0,
            &DirVars::load(&tree.0),
            &target_context("x86_64", "pc", ""),
        )
        .unwrap();
        assert!(parsed
            .targets
            .iter()
            .all(|target| target.mmake_name != "unowned"));
        assert!(parsed
            .skipped_local_make_includes
            .iter()
            .any(|message| message.contains("broader than one plain source-list")));
    }

    #[test]
    fn canonical_linklib_output_requires_target_owned_port_sources() {
        let tree = TempTree::new();
        fs::create_dir_all(tree.0.join("module")).unwrap();
        fs::write(
            tree.0.join("module/mmakefile.src"),
            "\
%fetch mmake=owned-fetch archive=owned destination=$(PORTSDIR)/owned
%build_linklib mmake=owned libname=owned files=$(PORTSDIR)/owned/x
%build_linklib mmake=owned-target libname=owned-target files=$(PORTSDIR)/owned/x compiler=target
%build_linklib mmake=owned-host libname=owned-host files=$(PORTSDIR)/owned/x compiler=host
%build_linklib mmake=owned-libdir libname=owned-libdir files=$(PORTSDIR)/owned/x libdir=$(GENDIR)/lib
%build_linklib mmake=owned-32 libname=owned-32 files=$(PORTSDIR)/owned/x objdir=$(GENDIR)/module/32bit
%build_linklib mmake=foreign libname=foreign files=$(PORTSDIR)/foreign/x
",
        )
        .unwrap();
        let parsed = super::parse_mmakefile_with_dirs_and_context(
            &tree.0.join("module/mmakefile.src"),
            &tree.0,
            &DirVars::load(&tree.0),
            &target_context("x86_64", "pc", ""),
        )
        .unwrap();
        let targets: BTreeMap<_, _> = parsed
            .targets
            .iter()
            .map(|target| (target.mmake_name.as_str(), target))
            .collect();
        assert!(targets["owned"].canonical_linklib_output);
        assert!(targets["owned-target"].canonical_linklib_output);
        for mmake in ["owned-host", "owned-libdir", "owned-32", "foreign"] {
            assert!(!targets[mmake].canonical_linklib_output, "{mmake}");
        }

        let zopfli = super::parse_mmakefile_with_dirs_and_context(
            &root().join("tools/zopfli/mmakefile.src"),
            &root(),
            &dirs(),
            &target_context("x86_64", "pc", ""),
        )
        .unwrap();
        for target in zopfli.targets.iter().filter(|target| {
            matches!(
                target.mmake_name.as_str(),
                "linklibs-zopfli" | "host-linklibs-zopfli"
            )
        }) {
            assert!(!target.canonical_linklib_output, "{}", target.mmake_name);
        }
    }

    #[test]
    fn generated_linklib_wildcards_are_exact_manifests_in_all_current_profiles() {
        let root = root();
        let dirs = dirs();
        let expected = BTreeMap::from([
            (
                "compiler-posixc-lfa-linklib",
                vec!["@AROS_GENMODULE|normal|stackstubs,regcallstubs|posixc|library|posixc_lfa.conf"],
            ),
            (
                "compiler-posixc-lfa-linklib-rel",
                vec!["@AROS_GENMODULE|rel|stackstubs,regcallstubs|posixc|library|posixc_lfa.conf"],
            ),
            (
                "workbench-libs-gl-linklib",
                vec![
                    "gl_funcs",
                    "@AROS_GENMODULE|normal|stackstubs,regcallstubs,autoinit,getlibbase|gl|library|gl.conf",
                ],
            ),
            (
                "workbench-libs-gl-linklib-rel",
                vec![
                    "gl_funcs",
                    "@AROS_GENMODULE|rel|stackstubs,regcallstubs,autoinit,getlibbase|gl|library|gl.conf",
                ],
            ),
        ]);

        for (cpu, platform, float_abi) in [
            ("x86_64", "pc", ""),
            ("arm", "raspi", "hard"),
            ("aarch64", "raspi", ""),
        ] {
            let target_context = target_context(cpu, platform, float_abi);
            let mut targets = BTreeMap::new();
            let mut diagnostics = Vec::new();
            for file in [
                "compiler/crt/posixc/mmakefile.src",
                "workbench/libs/gl/mmakefile.src",
            ] {
                let parsed = super::parse_mmakefile_with_dirs_and_context(
                    &root.join(file),
                    &root,
                    &dirs,
                    &target_context,
                )
                .unwrap();
                diagnostics.extend(parsed.skipped_programs);
                diagnostics.extend(parsed.partial_source_lists);
                targets.extend(
                    parsed
                        .targets
                        .into_iter()
                        .filter(|target| expected.contains_key(target.mmake_name.as_str()))
                        .map(|target| (target.mmake_name.clone(), target)),
                );
            }

            assert_eq!(
                targets.len(),
                expected.len(),
                "{cpu}-{platform}: {diagnostics:#?}"
            );
            for (mmake, sources) in &expected {
                let target = targets.get(*mmake).unwrap_or_else(|| {
                    panic!("{cpu}-{platform}: missing {mmake}: {diagnostics:#?}")
                });
                assert_eq!(target.module_type, ModuleType::LinkLib);
                assert_eq!(
                    target
                        .source_files
                        .iter()
                        .map(String::as_str)
                        .collect::<Vec<_>>(),
                    *sources,
                    "{cpu}-{platform}: {mmake}"
                );
            }
            assert!(
                diagnostics
                    .iter()
                    .all(|message| { expected.keys().all(|mmake| !message.contains(mmake)) }),
                "{cpu}-{platform}: {diagnostics:#?}"
            );
        }
    }

    #[test]
    fn concrete_profiles_keep_webp_dsp_targets_and_select_only_x86_sse2() {
        let root = root();
        let dirs = dirs();
        let file = root.join("workbench/classes/datatypes/webp/mmakefile.src");
        for (cpu, platform, float_abi) in [
            ("x86_64", "pc", ""),
            ("arm", "raspi", "hard"),
            ("aarch64", "raspi", ""),
        ] {
            let parsed = super::parse_mmakefile_with_dirs_and_context(
                &file,
                &root,
                &dirs,
                &target_context(cpu, platform, float_abi),
            )
            .unwrap();
            let targets: BTreeMap<_, _> = parsed
                .targets
                .iter()
                .map(|target| (target.mmake_name.as_str(), target))
                .collect();
            let sharpyuv = targets
                .get("datatypes-webp-linklibs-sharpyuv")
                .unwrap_or_else(|| panic!("{cpu}-{platform}: {:#?}", parsed.skipped_programs));
            let webpdsp = targets
                .get("datatypes-webp-linklibs-webpdsp")
                .unwrap_or_else(|| panic!("{cpu}-{platform}: {:#?}", parsed.skipped_programs));
            let sources: Vec<_> = sharpyuv
                .source_files
                .iter()
                .chain(&webpdsp.source_files)
                .collect();
            assert_eq!(
                sources.iter().any(|source| source.contains("_sse2")),
                cpu == "x86_64",
                "{cpu}-{platform} selected the wrong WebP SSE2 branch"
            );
            assert!(
                sources.iter().all(|source| !source.contains("_sse41")),
                "{cpu}-{platform} unexpectedly selected disabled WebP SSE4.1"
            );
        }
    }

    #[test]
    fn the_two_mkamikeymap_programs_keep_distinct_output_directories() {
        let root = root();
        let parsed = super::parse_mmakefile_with_dirs(
            &root.join("tools/mkamikeymap/mmakefile.src"),
            &root,
            &dirs(),
        )
        .unwrap();
        let outputs: BTreeMap<_, _> = parsed
            .targets
            .iter()
            .map(|target| (target.mmake_name.as_str(), target.target_dir.as_deref()))
            .collect();

        assert_eq!(
            outputs["tools-mkkeymap"],
            Some("${AROS_BUILD_DIR}/hosttools/")
        );
        assert_eq!(
            outputs["tools-mkamikeymap"],
            Some("${AROS_BUILD_DIR}/SYS/Extras/Developer/Build")
        );
    }

    #[test]
    fn every_library_module_materialises_its_client_archive() {
        let tree = TempTree::new();
        let module = tree.0.join("rom/thing");
        fs::create_dir_all(&module).unwrap();
        fs::write(module.join("thing.c"), "").unwrap();
        fs::write(
            module.join("thing.conf"),
            "##begin config\n\
             basename Thing\n\
             libbasetype struct ThingBase\n\
             ##end config\n",
        )
        .unwrap();
        let file = module.join("mmakefile.src");
        fs::write(
            &file,
            "%build_module mmake=kernel-thing modname=thing modtype=library files=thing\n",
        )
        .unwrap();
        let dirs = DirVars::load(&tree.0);
        let parsed = super::parse_mmakefile_with_dirs_and_context(
            &file,
            &tree.0,
            &dirs,
            &target_context("x86_64", "pc", ""),
        )
        .unwrap();

        // No linklibname=, no linklibfiles=: upstream still archives
        // thing_getlibbase and thing_autoinit into libthing.a, because the
        // module type alone puts them into <mod>_LINKLIBFILES.
        let genmodule = parsed.targets[0]
            .genmodule_linklibs
            .as_ref()
            .expect("library client-archive metadata");
        assert!(genmodule.enabled);
        assert!(genmodule.source_files.is_empty());
        assert!(parsed.skipped_client_archives.is_empty(), "{parsed:#?}");
    }

    #[test]
    fn non_library_module_needing_a_client_archive_is_reported() {
        let tree = TempTree::new();
        let module = tree.0.join("rom/clock");
        fs::create_dir_all(&module).unwrap();
        fs::write(module.join("clock.c"), "").unwrap();
        fs::write(
            module.join("clock.conf"),
            "##begin config\n\
             basename Clock\n\
             options autoinit\n\
             ##end config\n",
        )
        .unwrap();
        let file = module.join("mmakefile.src");
        fs::write(
            &file,
            "%build_module mmake=kernel-clock modname=clock modtype=device files=clock\n",
        )
        .unwrap();
        let dirs = DirVars::load(&tree.0);
        let parsed = super::parse_mmakefile_with_dirs_and_context(
            &file,
            &tree.0,
            &dirs,
            &target_context("x86_64", "pc", ""),
        )
        .unwrap();

        assert!(parsed.targets[0].genmodule_linklibs.is_none());
        assert_eq!(parsed.skipped_client_archives.len(), 1, "{parsed:#?}");
        assert!(parsed.skipped_client_archives[0].contains("libclock.a"));
    }

    #[test]
    fn module_directory_expansion_is_positional_and_reports_unknowns() {
        let joined = join_continuations(
            "MODDIR := Devs/First\n\
             %build_module mmake=one modname=one modtype=device files=one moduledir=$(MODDIR)\n\
             MODDIR := Storage/Second\n\
             %build_module mmake=two modname=two modtype=device files=two moduledir=$(MODDIR)\n",
        );
        let scope = collect_vars(&joined);
        let invocations = macro_invocations(&joined);
        assert_eq!(
            resolve_module_target_dir(
                &invocations[0].args,
                &scope,
                &dirs(),
                invocations[0].line,
                "device",
                true,
                false,
            )
            .unwrap()
            .as_deref(),
            Some("Devs/First")
        );
        assert_eq!(
            resolve_module_target_dir(
                &invocations[1].args,
                &scope,
                &dirs(),
                invocations[1].line,
                "device",
                true,
                false,
            )
            .unwrap()
            .as_deref(),
            Some("Storage/Second")
        );

        let error = resolve_module_target_dir(
            "moduledir=$(NOT_DEFINED)",
            &scope,
            &dirs(),
            usize::MAX,
            "device",
            true,
            false,
        )
        .unwrap_err();
        assert!(error.contains("NOT_DEFINED"), "{error}");
    }

    #[test]
    fn explicit_prefix_and_arch_specific_defaults_are_complete_paths() {
        let scope = collect_vars("");
        assert_eq!(
            resolve_module_target_dir(
                "prefix=$(TARGETDIR)",
                &scope,
                &dirs(),
                0,
                "library",
                true,
                false,
            )
            .unwrap()
            .as_deref(),
            Some("${AROS_BUILD_DIR}/Libs")
        );
        assert_eq!(
            resolve_module_target_dir("", &scope, &dirs(), 0, "library", true, true)
                .unwrap()
                .as_deref(),
            Some("${AROS_BOOT_ARCH_DIR}/Libs")
        );
        assert_eq!(
            resolve_module_target_dir(
                "moduledir=Storage/Foo archspecific=yes",
                &scope,
                &dirs(),
                0,
                "library",
                true,
                true,
            )
            .unwrap()
            .as_deref(),
            Some("Storage/Foo")
        );
    }

    #[test]
    fn module_suffix_override_is_separate_from_declared_type() {
        let scope = collect_vars("");
        assert_eq!(
            resolve_module_suffix("modsuffix=logger", &scope, &dirs(), 0, "library")
                .unwrap()
                .as_deref(),
            Some("logger")
        );
        assert_eq!(
            resolve_module_suffix("", &scope, &dirs(), 0, "usbclass")
                .unwrap()
                .as_deref(),
            Some("class")
        );
        assert_eq!(
            resolve_module_suffix("", &scope, &dirs(), 0, "printer").unwrap(),
            None
        );
    }

    #[test]
    fn real_tree_retains_exactly_four_abi_skeletons_and_zero_source_version() {
        let root = root();
        let dirs = dirs();
        let skip_dirs = ["build", "target", ".git"];
        let abi_invocations = WalkDir::new(&root)
            .into_iter()
            .filter_entry(|entry| {
                !entry.file_type().is_dir()
                    || entry.depth() == 0
                    || !skip_dirs
                        .iter()
                        .any(|dir| entry.file_name().to_string_lossy() == *dir)
            })
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.file_type().is_file() && entry.file_name() == "mmakefile.src")
            .map(|entry| {
                read_source(entry.path())
                    .unwrap()
                    .matches("%build_module_abi")
                    .count()
            })
            .sum::<usize>();
        assert_eq!(abi_invocations, 4);

        let abi_files = [
            (
                "rom/bluetooth/classes/mmakefile.src",
                "kernel-bluetooth-btclass",
                "btclass",
            ),
            (
                "rom/usb/classes/mmakefile.src",
                "kernel-usb-usbclass",
                "usbclass",
            ),
            (
                "rom/usb/classes/arosx/include/mmakefile.src",
                "kernel-usb-classes-arosx-library",
                "arosx",
            ),
            (
                "workbench/libs/dxtn/mmakefile.src",
                "workbench-libs-dxtn",
                "dxtn",
            ),
        ];

        for (file, mmake, modname) in abi_files {
            let parsed = super::parse_mmakefile_with_dirs(&root.join(file), &root, &dirs).unwrap();
            let target = parsed
                .targets
                .iter()
                .find(|target| target.mmake_name == mmake)
                .unwrap_or_else(|| panic!("{file} did not retain {mmake}"));
            assert_eq!(target.module_type, ModuleType::Abi);
            assert_eq!(target.target_name, modname);
            assert_eq!(target.declared_mod_type.as_deref(), Some("library"));
            assert!(!target.genmodule_only);
            assert!(target.source_files.is_empty());
            assert!(target.cxx_source_files.is_empty());
            assert!(target.objc_source_files.is_empty());
            assert!(target.asm_source_files.is_empty());

            let mut metas: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
            for rule in &parsed.meta_rules {
                metas
                    .entry(&rule.name)
                    .or_default()
                    .extend(rule.dependencies.iter().map(String::as_str));
            }
            assert!(metas[mmake].contains(&format!("{mmake}-includes").as_str()));
            assert!(metas[&format!("linklibs-{modname}").as_str()]
                .contains(&format!("{mmake}-linklib").as_str()));
            assert!(metas.contains_key(format!("{mmake}-kobj").as_str()));
            assert!(metas.contains_key(
                format!(
                    "{mmake}-${{AROS_TARGET_PLATFORM}}-${{AROS_TARGET_CPU}}-${{AROS_TARGET_VARIANT}}-quick"
                )
                .as_str()
            ));
        }

        let parsed = super::parse_mmakefile_with_dirs(
            &root.join("workbench/libs/version/mmakefile.src"),
            &root,
            &dirs,
        )
        .unwrap();
        let version = parsed
            .targets
            .iter()
            .find(|target| target.mmake_name == "workbench-libs-version")
            .expect("version.library must be retained");
        assert_eq!(version.module_type, ModuleType::Library);
        assert!(version.genmodule_only);
        assert!(version.source_files.is_empty());
        assert!(parsed
            .meta_rules
            .iter()
            .any(|rule| rule.name == "linklibs-version"
                && rule.dependencies == ["workbench-libs-version-linklib"]));
    }

    #[test]
    fn sourceful_module_forms_keep_their_noncyclic_implicit_metamake_graph() {
        let root = root();
        let dirs = dirs();
        for (file, mmake, modname, has_abi) in [
            (
                "compiler/crt/stdc/mmakefile.src",
                "compiler-stdc",
                "stdc",
                true,
            ),
            (
                "rom/usb/classes/serialpl2303/mmakefile.src",
                "kernel-usb-classes-serialpl2303",
                "serialpl2303",
                false,
            ),
        ] {
            let parsed = super::parse_mmakefile_with_dirs(&root.join(file), &root, &dirs)
                .unwrap_or_else(|error| panic!("{file}: {error}"));
            let mut metas: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
            for rule in &parsed.meta_rules {
                metas
                    .entry(&rule.name)
                    .or_default()
                    .extend(rule.dependencies.iter().map(String::as_str));
            }

            let quick = format!("{mmake}-quick");
            let kobj = format!("{mmake}-kobj");
            assert!(metas[quick.as_str()].contains(mmake), "{file}");
            assert!(metas[kobj.as_str()].contains("core-linklibs"), "{file}");
            // MetaMake's virtual architecture chain returns to the concrete
            // sourceful producer, which MetaMake breaks through pre-marked
            // traversal. CMake rejects that cycle, so only ABI/genmodule-only
            // forms emit it in the translated graph.
            let arch_cpu = format!("{mmake}-${{AROS_TARGET_CPU}}");
            assert!(!metas[kobj.as_str()].contains(arch_cpu.as_str()), "{file}");

            let includes_alias = format!("includes-{modname}");
            if has_abi {
                let includes = format!("{mmake}-includes");
                assert!(
                    metas[includes_alias.as_str()].contains(includes.as_str()),
                    "{file}"
                );
            } else {
                assert!(!metas.contains_key(includes_alias.as_str()), "{file}");
            }
        }
    }

    #[test]
    fn real_tree_module_output_metadata_has_expected_coverage() {
        let root = root();
        let dirs = dirs();
        let target = target_context("x86_64", "pc", "");
        let mut install_dirs = Vec::new();
        let mut suffixes = Vec::new();
        let mut output_errors = Vec::new();

        let skip_dirs = ["build", "target", ".git"];
        for entry in WalkDir::new(&root)
            .into_iter()
            .filter_entry(|entry| {
                !entry.file_type().is_dir()
                    || entry.depth() == 0
                    || !skip_dirs
                        .iter()
                        .any(|dir| entry.file_name().to_string_lossy() == *dir)
            })
            .filter_map(std::result::Result::ok)
        {
            if !entry.file_type().is_file() || entry.file_name() != "mmakefile.src" {
                continue;
            }
            let source = read_source(entry.path()).unwrap();
            if !source.contains("moduledir=")
                && !source.contains("prefix=$(TARGETDIR)")
                && !source.contains("archspecific=yes")
                && !source.contains("modsuffix=")
            {
                continue;
            }
            let parsed =
                super::parse_mmakefile_with_dirs_and_context(entry.path(), &root, &dirs, &target)
                    .unwrap();
            install_dirs.extend(parsed.targets.iter().filter_map(|target| {
                if matches!(
                    target.module_type,
                    ModuleType::Program | ModuleType::ProgramGroup
                ) {
                    return None;
                }
                target
                    .target_dir
                    .as_ref()
                    .map(|directory| (target.mmake_name.clone(), directory.clone()))
            }));
            suffixes.extend(parsed.targets.iter().filter_map(|target| {
                target
                    .mod_suffix
                    .as_ref()
                    .map(|suffix| (target.mmake_name.clone(), suffix.clone()))
            }));
            output_errors.extend(parsed.skipped_programs.into_iter().filter(|message| {
                ["moduledir=", "prefix=", "archspecific=", "modsuffix="]
                    .iter()
                    .any(|needle| message.contains(needle))
            }));
        }

        assert!(output_errors.is_empty(), "{output_errors:#?}");
        assert_eq!(install_dirs.len(), 60);
        assert_eq!(suffixes.len(), 46);
        assert_eq!(
            install_dirs
                .iter()
                .filter(|(mmake, directory)| {
                    mmake.starts_with("test-library-")
                        && directory == "${AROS_BUILD_DIR}/SYS/Developer/Debug/Tests/Library/Libs"
                })
                .count(),
            4
        );
        assert_eq!(
            install_dirs
                .iter()
                .filter(|(_, directory)| directory.starts_with("${AROS_BOOT_ARCH_DIR}/"))
                .count(),
            12
        );
    }
}
