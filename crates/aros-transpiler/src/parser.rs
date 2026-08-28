use crate::arch_sources::collect_arch_sources;
use crate::ast::{
    GenmoduleLinklibs, MetaTargetRule, ModuleType, ParsedMmakefile, TargetDefinition,
};
use crate::capability::literal_defines::safe_build_tree_output_directory;
use crate::capability::mesa::{
    compile_contract, current_profile, remaining_linklib_sources, PRIVATE_LIBDIR,
};
use crate::capability::mesa::{generators, sse41};
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
            ) | (
                Some("riscv64"),
                Some("opensbi"),
                Some("llvm"),
                Some(""),
                Some("1"),
                Some("")
            )
        )
    })
}

fn expected_ahi_profile_exclusion(target: Option<&TargetContext>) -> bool {
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
                Some("riscv64"),
                Some("opensbi"),
                Some("llvm"),
                Some(""),
                Some("1"),
                Some("")
            )
        )
    })
}

#[path = "parser_pipeline.rs"]
mod pipeline;
use pipeline::parse_mmakefile_impl;

#[cfg(test)]
#[path = "parser_tests.rs"]
mod tests;
