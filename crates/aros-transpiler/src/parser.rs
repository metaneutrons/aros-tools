use crate::arch_sources::collect_arch_sources;
use crate::ast::{MetaTargetRule, ModuleType, ParsedMmakefile, TargetDefinition};
use crate::copy_includes::collect_copy_includes;
use crate::fetch::collect_fetches;
use crate::flags::collect_flags;
use crate::includes::{collect_arch_decls, collect_includes};
use crate::make_opts::collect_make_opts;
use aros_common::{read_source, Result};
use regex::Regex;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::LazyLock;

/// One non-empty #MM rule. Horizontal whitespace is intentional: `\s*` also
/// consumes a newline, so an empty `#MM setup-ppc :` used to steal the next
/// ordinary Make rule and manufacture `setup-ppc -> setup-ppc` self-cycles.
static META_RULE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^#MM-?[ \t]+([^ \t\r\n:]+)[ \t]*:[ \t]*([^\r\n]+)").unwrap());
static CONTINUATION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\[ \t]*\r?\n[ \t]*").unwrap());

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

fn expand_file_list(raw: &str, vars: &HashMap<String, Vec<String>>) -> Vec<String> {
    expand_file_list_depth(raw, vars, 8)
}

/// Expands a file list, following variable references.
///
/// A list routinely names other lists, and those name further ones:
/// muimaster builds its sources from `$(FUNCS) $(FILES)` where `FILES` is
/// itself `$(FILES) $(CLASSFILES)`. Expanding only one level left it with 26
/// sources where the reference has about 94. Bounded, so a variable defined in
/// terms of itself cannot loop.
fn expand_file_list_depth(
    raw: &str,
    vars: &HashMap<String, Vec<String>>,
    depth: usize,
) -> Vec<String> {
    let mut result = Vec::new();
    for token in raw.split_whitespace() {
        let cleaned = token.replace(['"', '\\'], "").trim().to_string();

        // A plain `$(VAR)` expands to its list, whose items may be references
        // in turn.
        if let Some(name) = cleaned.strip_prefix("$(").and_then(|t| t.strip_suffix(')')) {
            if depth > 0 && !name.contains(' ') && !name.contains(',') {
                if let Some(list) = vars.get(name) {
                    for item in list {
                        if item.contains("$(") {
                            result.extend(expand_file_list_depth(item, vars, depth - 1));
                        } else if keep_source_name(item) {
                            result.push(item.clone());
                        }
                    }
                }
            }
            continue;
        }

        // Anything still carrying Make syntax is not a file name.
        if cleaned.contains('$') || cleaned.contains('(') || cleaned.contains(',') {
            continue;
        }
        if keep_source_name(&cleaned) {
            result.push(cleaned);
        }
    }
    result.dedup();
    result
}

/// Whether a token names a source file.
///
/// Names are kept verbatim rather than passed through sanitize_ident: a source
/// is routinely a path relative to the mmakefile, and turning `libudis86/decode`
/// into `libudis86_decode` produced a name matching no file on disk. Only the
/// CMake target name needs sanitising, not the sources it is built from.
fn keep_source_name(s: &str) -> bool {
    !s.is_empty()
        && !s.contains('$')
        && !s.contains('(')
        // A stray closing paren is the tail of a `$(call ...)` the tokeniser
        // split apart. Emitted verbatim it ended a CMake argument list early:
        // `SOURCES autoinit-aros)` made the whole generated file unparsable.
        && !s.contains(')')
        && !s.contains(',')
}

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
struct Invocation {
    name: String,
    args: String,
    line: usize,
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

/// Variable assignments in the order the file makes them.
///
/// Make expands a declaration's arguments where the declaration stands.
/// `%build_progs files=$(FILES)` therefore takes the value FILES held at that
/// line, because the macro emits `<mmake>_FILES := %(files)` -- a simple
/// assignment, evaluated in place (config/make.tmpl:1868).
///
/// Reading one file-global value instead gave every declaration the file's last
/// assignment. arch/m68k-amiga/c declares `FILES := gdbstub`, a %build_progs,
/// `FILES := gdbstop`, and a second %build_progs; both came out building
/// gdbstop, two targets claimed the output SYS/C/.../gdbstop, and Ninja refused
/// to generate the build at all. 16 declarations across 9 mmakefiles read a
/// variable that is reassigned later in the same file.
pub struct VarScope {
    /// Per name, the assignments in file order as (line, values).
    assignments: HashMap<String, Vec<(usize, Vec<String>)>>,
    /// Per name, the right-hand side as written, in file order.
    ///
    /// A list is not enough for a path. `EXEDIR := $(AROS_TOOLS)/QuickPart` is
    /// one word either way, but `dir=$(AROS_PRESETS)/Icons/Gorilla/Small/$(AROS_DIR_AROS)`
    /// has to keep its slashes and its references, so path resolution reads
    /// this instead of the word list.
    raw: HashMap<String, Vec<(usize, String)>>,
}

impl VarScope {
    /// The variable state as Make would see it at `line`.
    ///
    /// A declaration on line N sees every assignment made before it and none of
    /// those made after.
    fn snapshot(&self, line: usize) -> HashMap<String, Vec<String>> {
        self.assignments
            .iter()
            .filter_map(|(name, history)| {
                history
                    .iter()
                    .rev()
                    .find(|(at, _)| *at < line)
                    .map(|(_, values)| (name.clone(), values.clone()))
            })
            .collect()
    }

    /// The right-hand side of `name` as written, as of `line`.
    #[must_use]
    pub fn raw_at(&self, name: &str, line: usize) -> Option<String> {
        self.raw
            .get(name)?
            .iter()
            .rev()
            .find(|(at, _)| *at < line)
            .map(|(_, v)| v.clone())
    }

    /// The most recent value of `name`, used to resolve a self-referential
    /// assignment while the scan is still running.
    fn latest(&self, name: &str) -> &[String] {
        self.assignments
            .get(name)
            .and_then(|h| h.last())
            .map_or(&[][..], |(_, v)| v.as_slice())
    }

    /// The most recent raw value of `name` while the assignment scan is in
    /// progress. Appending is defined in terms of the value accumulated so
    /// far, not merely the last right-hand side.
    fn latest_raw(&self, name: &str) -> Option<&str> {
        self.raw
            .get(name)
            .and_then(|h| h.last())
            .map(|(_, value)| value.as_str())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AssignmentKind {
    Set,
    SetIfUnset,
    Append,
}

/// Splits a plain Make variable assignment without mistaking a rule for one.
///
/// The tree uses `:=`, `=`, `?=` and `+=`. Keeping the operator is important:
/// two icon lists are built incrementally, and treating their `+=` lines as
/// either invalid or ordinary assignments silently drops 118 generated files.
fn variable_assignment(line: &str) -> Option<(&str, &str, AssignmentKind)> {
    let trimmed = line.trim();
    let (at, width, kind) = [
        (":=", AssignmentKind::Set),
        ("+=", AssignmentKind::Append),
        ("?=", AssignmentKind::SetIfUnset),
        ("=", AssignmentKind::Set),
    ]
    .into_iter()
    .filter_map(|(op, kind)| trimmed.find(op).map(|at| (at, op.len(), kind)))
    .min_by_key(|(at, _, _)| *at)?;

    let name = trimmed[..at].trim();
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return None;
    }
    Some((name, trimmed[at + width..].trim(), kind))
}

/// Reads every variable assignment from continuation-joined mmakefile text.
#[must_use]
pub fn collect_vars(joined: &str) -> VarScope {
    let mut scope = VarScope {
        assignments: HashMap::new(),
        raw: HashMap::new(),
    };

    for (line_no, line) in joined.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.starts_with('%') {
            continue;
        }

        // Make has four assignment forms and the tree uses all of them.
        // Reading only `:=` lost every list written with `=` or `?=`:
        // rom/hidds/pci/pcitool declares `FILES = main pciids support locale`
        // that way, while the icon sets append to two lists with `+=`.
        let Some((var_name, value, kind)) = variable_assignment(line) else {
            continue;
        };

        if kind == AssignmentKind::SetIfUnset && scope.assignments.contains_key(var_name) {
            continue;
        }

        // `FILES := $(FILES) $(CLASSFILES)` has to keep what FILES already
        // held. Inserting the new list would discard it, and the surviving
        // `$(FILES)` reference then resolves to itself: muimaster came out with
        // 26 sources against the reference's ~94.
        let self_ref = format!("$({var_name})");
        let prior = scope.latest(var_name);
        let expanded_rhs = if value.contains(&self_ref) && !prior.is_empty() {
            value.replace(&self_ref, &prior.join(" "))
        } else {
            value.to_owned()
        };
        let expanded = if kind == AssignmentKind::Append {
            match scope.latest_raw(var_name) {
                Some(old) if !old.is_empty() && !expanded_rhs.is_empty() => {
                    format!("{old} {expanded_rhs}")
                }
                Some(old) if !old.is_empty() => old.to_owned(),
                _ => expanded_rhs,
            }
        } else {
            expanded_rhs
        };

        let values: Vec<String> = expanded
            .split_whitespace()
            .filter(|s| *s != "\\")
            .map(|s| s.replace(['"', '\\'], "").trim().to_owned())
            .filter(|s| keep_list_item(s))
            .collect();
        scope
            .raw
            .entry(var_name.to_owned())
            .or_default()
            .push((line_no, expanded.trim().to_owned()));
        scope
            .assignments
            .entry(var_name.to_owned())
            .or_default()
            .push((line_no, values));
    }

    scope
}

/// Whether a word from a Make list is usable as a list item.
///
/// A slash used to disqualify one, which threw away most of what these lists
/// hold: a source name is routinely a path relative to the mmakefile, as in
/// `libudis86/decode` or `../locale`. 58 declarations came out with an empty
/// file list for that reason alone. An unresolved `$(...)` is still dropped,
/// since substituting nothing would silently compile the wrong set.
fn keep_list_item(s: &str) -> bool {
    if s.is_empty() || s.contains(',') {
        return false;
    }
    // A whole `$(VAR)` reference is kept, so expand_file_list can follow it:
    // `FILES := $(FILES) $(CLASSFILES)` has to survive collection or the list
    // it names is lost. A fragment carrying a stray paren is Make syntax the
    // tokeniser split apart and cannot be resolved.
    if s.starts_with("$(") && s.ends_with(')') && !s[2..s.len() - 1].contains(')') {
        return true;
    }
    !s.contains('$') && !s.contains(')')
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
fn macro_invocations(joined: &str) -> Vec<Invocation> {
    let mut out = Vec::new();
    for (line_no, line) in joined.lines().enumerate() {
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
            line: line_no,
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

/// The relative module directory genmodule chooses for a full module when no
/// `moduledir=` override is present (tools/genmodule/config.c:250-333).
///
/// This is normally left to the CMake module builder. It is needed here only
/// when a declaration explicitly changes `prefix=`, because that prefix and
/// the relative default together determine the complete output directory.
fn default_relative_module_dir(mod_type: &str) -> Option<&'static str> {
    match mod_type {
        "library" => Some("Libs"),
        "class" => Some("Classes"),
        "mcc" | "mui" | "mcp" => Some("Classes/Zune"),
        "device" | "resource" | "hook" => Some("Devs"),
        "gadget" => Some("Classes/Gadgets"),
        "image" => Some("Classes/Images"),
        "datatype" => Some("Classes/DataTypes"),
        "usbclass" => Some("Classes/USB"),
        "btclass" => Some("Classes/Bluetooth"),
        "hidd" => Some("Devs/Drivers"),
        "handler" => Some("L"),
        _ => None,
    }
}

fn rendered_absolute(path: &str) -> bool {
    Path::new(path).is_absolute()
        || path == "${AROS_BUILD_DIR}"
        || path.starts_with("${AROS_BUILD_DIR}/")
}

fn join_module_prefix(prefix: &str, directory: &str) -> String {
    if rendered_absolute(directory) {
        return directory.to_owned();
    }
    let prefix = prefix.trim_end_matches('/');
    let directory = directory.trim_start_matches('/');
    if prefix.is_empty() {
        directory.to_owned()
    } else if directory.is_empty() {
        prefix.to_owned()
    } else {
        format!("{prefix}/{directory}")
    }
}

fn expand_module_arg(
    raw: &str,
    scope: &VarScope,
    dirs: &crate::dirs::DirVars,
    line: usize,
) -> std::result::Result<String, Vec<String>> {
    let local = |name: &str| scope.raw_at(name, line);

    // A whole local variable names the value of its assignment, not a fresh
    // recursive lookup of that name. This matters when a file shadows a
    // configured variable with a simple assignment such as
    // TARGETDIR := $(AROS_TESTS)/Library: AROS_TESTS was derived from the
    // configured TARGETDIR before the local assignment took effect.
    if let Some(name) = raw
        .strip_prefix("$(")
        .and_then(|value| value.strip_suffix(')'))
    {
        if !name.contains(['$', ' ', ')']) {
            if let Some(value) = local(name) {
                return dirs.expand_with(&value, &|nested| {
                    if nested == name {
                        None
                    } else {
                        local(nested)
                    }
                });
            }
        }
    }

    dirs.expand_with(raw, &local)
}

/// Resolves a module's explicit output arguments at the declaration line.
///
/// Local variables shadow the shared `make.cfg.in` directory table. An
/// explicit but unresolved value is an error: treating it like an absent
/// override would silently install the module into its type's default path.
fn resolve_module_target_dir(
    args: &str,
    scope: &VarScope,
    dirs: &crate::dirs::DirVars,
    line: usize,
    mod_type: &str,
    uses_prefix: bool,
    arch_specific: bool,
) -> std::result::Result<Option<String>, String> {
    let module_dir = match macro_arg(args, "moduledir") {
        Some(raw) => Some(
            expand_module_arg(&raw, scope, dirs, line)
                .map_err(|missing| format!("moduledir={raw} references {}", missing.join(", ")))?,
        ),
        None => None,
    };

    if !uses_prefix && !arch_specific {
        return Ok(module_dir);
    }

    let prefix = if uses_prefix {
        match macro_arg(args, "prefix") {
            Some(raw) => Some(
                expand_module_arg(&raw, scope, dirs, line)
                    .map_err(|missing| format!("prefix={raw} references {}", missing.join(", ")))?,
            ),
            None => None,
        }
    } else {
        None
    };

    // An explicit moduledir replaces DEFMODDIR after the archspecific prefix
    // is computed (make.tmpl:2398-2407), so it must never inherit boot/<arch>.
    // CMake supplies the ordinary AROSDIR prefix for an otherwise relative
    // override; only an explicitly changed prefix has to be joined here.
    if let Some(directory) = module_dir {
        if rendered_absolute(&directory) {
            return Ok(Some(directory));
        }
        return Ok(Some(prefix.map_or_else(
            || directory.clone(),
            |prefix| join_module_prefix(&prefix, &directory),
        )));
    }

    if prefix.is_none() && !arch_specific {
        return Ok(None);
    }

    let directory = default_relative_module_dir(mod_type)
        .ok_or_else(|| format!("no known default moduledir for modtype={mod_type}"))?
        .to_owned();
    if rendered_absolute(&directory) {
        return Ok(Some(directory));
    }

    if arch_specific {
        // build_module_core inserts AROS_DIR_BOOTARCH between prefix and the
        // module's relative default (make.tmpl:2400-2407). With the ordinary
        // prefix, use the canonical CMake directory directly. An explicitly
        // changed prefix instead receives the same relative boot path.
        return Ok(Some(prefix.map_or_else(
            || join_module_prefix("${AROS_BOOT_ARCH_DIR}", &directory),
            |prefix| {
                join_module_prefix(
                    &prefix,
                    &format!("boot/${{AROS_TARGET_PLATFORM}}/{directory}"),
                )
            },
        )));
    }

    Ok(prefix.map(|prefix| join_module_prefix(&prefix, &directory)))
}

fn resolve_yes_argument(
    args: &str,
    key: &str,
    scope: &VarScope,
    dirs: &crate::dirs::DirVars,
    line: usize,
) -> std::result::Result<bool, String> {
    let Some(raw) = macro_arg(args, key) else {
        return Ok(false);
    };
    let local = |name: &str| scope.raw_at(name, line);
    dirs.expand_with(&raw, &local)
        .map(|value| value == "yes")
        .map_err(|missing| format!("{key}={raw} references {}", missing.join(", ")))
}

fn resolve_module_suffix(
    args: &str,
    scope: &VarScope,
    dirs: &crate::dirs::DirVars,
    line: usize,
    mod_type: &str,
) -> std::result::Result<Option<String>, String> {
    if let Some(raw) = macro_arg(args, "modsuffix") {
        if raw.is_empty() {
            return Ok(None);
        }
        let local = |name: &str| scope.raw_at(name, line);
        return dirs
            .expand_with(&raw, &local)
            .map(|value| (!value.is_empty()).then_some(value))
            .map_err(|missing| format!("modsuffix={raw} references {}", missing.join(", ")));
    }
    Ok(matches!(mod_type, "usbclass" | "btclass").then(|| "class".to_owned()))
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
    let content = read_source(path)?;
    let parent_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let rel_dir = parent_dir
        .strip_prefix(root)
        .unwrap_or(parent_dir)
        .to_path_buf();
    let mut targets = Vec::new();
    let mut meta_rules = Vec::new();
    let mut skipped_meta_rules = Vec::new();

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
        let Ok(body) = read_source(&root.join(&f.path)) else {
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

    // Make evaluates a declaration's arguments where the declaration stands, so
    // the variable state is positional. Both scans read the same
    // continuation-joined text, which is what makes their line numbers
    // comparable.
    let joined = join_continuations(&content);
    let scope = collect_vars(&joined);
    let icon_scan = crate::icons::collect_icons_all(&joined, dirs, &rel_dir);
    let invocations = macro_invocations(&joined);
    let mut skipped_programs: Vec<String> = Vec::new();
    let re_libs = Regex::new(r#"uselibs=(?:"([^"]+)"|([^\s\\]+))"#).unwrap();

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
        let vars = scope.snapshot(inv.line);
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

        // The same source-list rules as every other build macro: the union of
        // the four lists, and the reference's default of every *.c in the
        // directory when none is given (make.tmpl:2802). This loop used to read
        // `files=` alone and silently yield nothing otherwise, which left 21
        // declarations with no sources at all and never reported it.
        let (mut source_files, declared_any) = macro_sources(rest, &vars);
        if source_files.is_empty() {
            if declared_any {
                skipped_programs.push(format!(
                    "{}: %{} mmake={mmake_raw} modname={mod_raw} has an unresolved file list",
                    rel_dir.display(),
                    inv.name
                ));
                continue;
            }
            source_files = wildcard_c_sources(parent_dir);
            if source_files.is_empty() {
                skipped_programs.push(format!(
                    "{}: %{} mmake={mmake_raw} modname={mod_raw} declares no sources",
                    rel_dir.display(),
                    inv.name
                ));
                continue;
            }
        }

        let use_libs: Vec<String> = re_libs.captures(rest).map_or_else(Vec::new, |lcap| {
            let libs_str = lcap
                .get(1)
                .or_else(|| lcap.get(2))
                .map_or("", |m| m.as_str());
            expand_file_list(libs_str, &vars)
        });
        let declared_mod_type =
            matches!(module_type, ModuleType::Custom).then(|| mod_type_owned.clone());

        targets.push(TargetDefinition {
            mmake_name,
            target_name: mod_name,
            module_type,
            source_files,
            use_libs,
            dependencies: Vec::new(),
            dir_path: rel_dir.clone(),
            target_dir,
            link_libs: Vec::new(),
            variant_32bit: false,
            declared_mod_type,
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
            link_libs: Vec::new(),
            variant_32bit: false,
            declared_mod_type: None,
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
        let vars = scope.snapshot(inv.line);
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
        let is_simple_module = matches!(module_type, ModuleType::SimpleModule);
        let declared_mod_type = if is_simple_module {
            macro_arg(&inv.args, "modtype")
        } else {
            None
        };
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

        targets.push(TargetDefinition {
            mmake_name,
            target_name,
            module_type,
            source_files,
            use_libs,
            dependencies: Vec::new(),
            dir_path: rel_dir.clone(),
            target_dir,
            link_libs: Vec::new(),
            variant_32bit,
            declared_mod_type,
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

    Ok(ParsedMmakefile {
        targets,
        meta_rules,
        icon_targets: icon_scan.targets,
        icons: icon_scan.sets,
        skipped_icons: icon_scan.skipped,
        skipped_meta_rules,
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
    use super::{
        collect_vars, join_continuations, join_mm_continuations, macro_arg, macro_invocations,
        render_meta_token, resolve_module_suffix, resolve_module_target_dir, sanitize_ident,
        META_RULE_RE,
    };
    use crate::dirs::DirVars;
    use aros_common::read_source;
    use std::path::Path;
    use walkdir::WalkDir;

    fn root() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..")
    }

    fn dirs() -> DirVars {
        DirVars::load(&root())
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
    fn a_conditional_assignment_does_not_overwrite_an_existing_value() {
        let scope = collect_vars("A := first\nA ?= second\n%build_prog mmake=x progname=X\n");
        assert_eq!(scope.raw_at("A", usize::MAX).as_deref(), Some("first"));
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
    fn real_tree_module_output_metadata_has_expected_coverage() {
        let root = root();
        let dirs = dirs();
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
            let parsed = super::parse_mmakefile_with_dirs(entry.path(), &root, &dirs).unwrap();
            install_dirs.extend(parsed.targets.iter().filter_map(|target| {
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
        assert_eq!(install_dirs.len(), 61);
        assert_eq!(suffixes.len(), 44);
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
