//! The AROS output directory layout, read from `config/make.cfg.in`.
//!
//! Every declaration that names an output location does it through a Make
//! variable: `%build_icons dir=$(AROS_PRESETS)/Icons/Gorilla/Small/$(AROS_DIR_AROS)`,
//! `%build_prog targetdir=$(AROS_C)`, `%make_package` writing to
//! `$(AROSARCHDIR)`. There are 36 such variables in use in the icon
//! declarations alone, and config/make.cfg.in defines all but six of them.
//!
//! Reading that file is the alternative to a hand-written match arm per
//! variable, which is what this replaces. A hand-written table is glue: it goes
//! out of date without anyone noticing, and a variable nobody thought of
//! resolves to nothing rather than to an error. `AROS_DIR__TOOLS` in
//! images/IconSets/Gorilla/Icons/Small/AROS/Tools/mmakefile.src:26 is a
//! misspelling of `AROS_DIR_TOOLS`; in Make it expands to the empty string and
//! the icons land one directory too high. Read generically, it is a variable
//! nothing defines, which is reportable.

use aros_common::read_source;
use std::collections::HashMap;
use std::path::Path;

/// How deep a `$(VAR)` chain may nest before it is treated as unresolvable.
///
/// The real chains are three or four deep -- AROS_WALLPAPERS -> AROS_PRESETS ->
/// AROS_PREFS -> AROSDIR -> TARGETDIR. The cap exists for a cycle, not for
/// depth.
const MAX_DEPTH: usize = 12;

/// The values this build supplies for variables config/make.cfg.in expects from
/// configure or from the environment.
///
/// Keeping them here rather than in the resolver means the difference between
/// this build and the historic one is one readable list.
const SEEDS: &[(&str, &str)] = &[
    // config/make.cfg.in:17 builds TARGETDIR from $(TOP)/bin/<arch>-<cpu>; the
    // CMake binary directory is its counterpart.
    ("TOP", "${CMAKE_SOURCE_DIR}"),
    ("SRCDIR", "${CMAKE_SOURCE_DIR}"),
    ("TARGETDIR", "${AROS_BUILD_DIR}"),
    ("GENDIR", "${AROS_BUILD_DIR}/gen"),
    ("HOSTDIR", "${AROS_BUILD_DIR}/hosttools"),
    ("TOOLDIR", "${AROS_BUILD_DIR}/hosttools"),
    // The system directory. The historic tree calls it AROS/
    // (config/make.cfg.in:51); this build calls it SYS/, after the volume it
    // becomes at runtime, and cmake/AROS.cmake:52-55 and the boot-iso target
    // both spell it that way. Overriding this one leaf is what makes every
    // AROS_* path below derive correctly.
    ("AROS_DIR_AROS", "SYS"),
    // Target parameters. The historic AROS_TARGET_ARCH names the machine, which
    // is AROS_TARGET_PLATFORM here; see the note in CMakeLists.txt.
    ("AROS_TARGET_CPU", "${AROS_TARGET_CPU}"),
    ("AROS_TARGET_ARCH", "${AROS_TARGET_PLATFORM}"),
    ("AROS_TARGET_PLATFORM", "${AROS_TARGET_LEGACY_PLATFORM}"),
    ("AROS_TARGET_FAMILY", "${AROS_TARGET_FAMILY}"),
    // Empty in every configuration this build supports. Named rather than left
    // undefined, so the ifeq at config/make.cfg.in:52 can be decided.
    ("AROS_TARGET_SUFFIX", ""),
    ("AROS_TARGET_CPU32", "${AROS_TARGET_CPU32}"),
    ("HOST_EXE_SUFFIX", ""),
    // configure's --with-iconset, default Gorilla (configure:12814). Nine
    // mmakefiles build a path from it.
    ("AROS_TARGET_ICONSET", "${AROS_TARGET_ICONSET}"),
];

/// Directory variables resolved to CMake expressions.
pub struct DirVars {
    resolved: HashMap<String, String>,
    /// Assignments that could not be resolved, and why. Retained for callers'
    /// diagnostics rather than inserted as misleading literal paths.
    pub unresolved: Vec<String>,
    /// Make conditionals whose truth could not be decided, so both branches
    /// were skipped.
    pub undecided_conditions: Vec<String>,
}

impl DirVars {
    /// Reads config/make.cfg.in under `root`.
    ///
    /// A missing or unreadable file yields an empty table rather than an error:
    /// the callers all degrade to reporting an unresolved path, which is the
    /// same outcome and says more about what went wrong.
    #[must_use]
    pub fn load(root: &Path) -> Self {
        let mut me = Self {
            resolved: SEEDS
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
            unresolved: Vec::new(),
            undecided_conditions: Vec::new(),
        };
        let path = root.join("config/make.cfg.in");
        let Ok(text) = read_source(&path) else {
            return me;
        };
        me.absorb(&text);
        me
    }

    /// Reads the assignments of one Make fragment in file order.
    fn absorb(&mut self, text: &str) {
        // Nesting is one level deep in the file as it stands, but the stack
        // costs nothing and a false `ifeq` inside a true one has to stay false.
        let mut taken: Vec<bool> = Vec::new();
        let mut undecided: Vec<bool> = Vec::new();

        for raw in text.lines() {
            let line = raw.trim();
            if line.starts_with('#') {
                continue;
            }

            if let Some(cond) = line
                .strip_prefix("ifeq")
                .map(|c| (c, true))
                .or_else(|| line.strip_prefix("ifneq").map(|c| (c, false)))
            {
                let (args, want_equal) = cond;
                if let Some(value) = self.condition_holds(args, want_equal) {
                    taken.push(value);
                    undecided.push(false);
                } else {
                    self.undecided_conditions.push(line.to_owned());
                    // Neither branch is safe to absorb: choosing the else
                    // side would be just as speculative as choosing the if.
                    taken.push(false);
                    undecided.push(true);
                }
                continue;
            }
            if line == "else" {
                if let (false, Some(last)) =
                    (undecided.last().copied().unwrap_or(false), taken.last_mut())
                {
                    *last = !*last;
                }
                continue;
            }
            if line == "endif" {
                taken.pop();
                undecided.pop();
                continue;
            }
            if taken.iter().any(|t| !t) {
                continue;
            }

            let Some((name, value)) = split_assignment(line) else {
                continue;
            };
            // An autoconf placeholder is filled in by configure, which this
            // build does not run. Recorded as unresolved rather than stored, so
            // a path built from it is reported instead of coming out with an
            // `@...@` in it.
            if value.contains('@') {
                self.unresolved
                    .push(format!("{name} = {value} (configure placeholder)"));
                continue;
            }
            // Seeds win: they are this build's answer where the historic file
            // has a different one.
            if SEEDS.iter().any(|(k, _)| *k == name) {
                continue;
            }
            self.resolved.insert(name.to_owned(), value.to_owned());
        }
    }

    /// Whether an `ifeq (a,b)` / `ifneq (a,b)` holds, or None if either side
    /// still contains a variable nothing defines.
    fn condition_holds(&self, args: &str, want_equal: bool) -> Option<bool> {
        let inner = args.trim().strip_prefix('(')?.strip_suffix(')')?;
        let (a, b) = inner.split_once(',')?;
        let a = self.expand(a.trim())?;
        let b = self.expand(b.trim())?;
        // Target parameters remain CMake expressions in the table. Their
        // value is deliberately not guessed while the Rust transpiler runs.
        if a.contains("${") || b.contains("${") {
            return None;
        }
        Some((a == b) == want_equal)
    }

    /// Expands `$(...)` references, returning None if any of them is unknown.
    ///
    /// The result is a CMake string, so `${AROS_BUILD_DIR}/SYS/Prefs/Presets`
    /// rather than a filesystem path: the value is written into generated CMake
    /// and expanded there.
    #[must_use]
    pub fn expand(&self, raw: &str) -> Option<String> {
        self.expand_depth(raw, MAX_DEPTH)
    }

    fn expand_depth(&self, raw: &str, depth: usize) -> Option<String> {
        if depth == 0 {
            return None;
        }
        let mut out = String::with_capacity(raw.len());
        let mut rest = raw;
        while let Some(start) = rest.find("$(") {
            out.push_str(&rest[..start]);
            let after = &rest[start + 2..];
            let end = after.find(')')?;
            let name = &after[..end];
            // A nested reference or a function call is not a plain variable.
            if name.contains('$') || name.contains(' ') {
                return None;
            }
            let value = self.resolved.get(name)?;
            out.push_str(&self.expand_depth(value, depth - 1)?);
            rest = &after[end + 1..];
        }
        out.push_str(rest);
        Some(out)
    }

    /// Expands `$(...)` against the declaring mmakefile first, then this table.
    ///
    /// Five of the 36 variables the icon declarations build a path from are
    /// local to their mmakefile -- `EXEDIR := $(AROS_TOOLS)/QuickPart`,
    /// `PCIDEVSDIR`, `PCDEVSDIR`, `AMIGADEVSDIR`, `DEVS_DIR` -- and those
    /// values themselves reference this table, so the two have to resolve
    /// together rather than one after the other.
    ///
    /// `local` returns the raw right-hand side as written, not a word list: a
    /// path keeps its slashes and its own references.
    ///
    /// # Errors
    ///
    /// The names that could not be resolved, so the caller can report which
    /// variable is missing rather than only that a path failed.
    pub fn expand_with<F>(&self, raw: &str, local: &F) -> std::result::Result<String, Vec<String>>
    where
        F: Fn(&str) -> Option<String>,
    {
        let Some(value) = self.expand_with_depth(raw, local, MAX_DEPTH) else {
            let mut missing = self.missing_in_with(raw, local);
            if missing.is_empty() {
                // Resolvable names, but something in the value is not a plain
                // reference: a function call, or a cycle.
                missing.push(format!("{raw} (not a plain variable reference)"));
            }
            return Err(missing);
        };
        Ok(value)
    }

    fn expand_with_depth<F>(&self, raw: &str, local: &F, depth: usize) -> Option<String>
    where
        F: Fn(&str) -> Option<String>,
    {
        if depth == 0 {
            return None;
        }
        let mut out = String::with_capacity(raw.len());
        let mut rest = raw;
        while let Some(start) = rest.find("$(") {
            out.push_str(&rest[..start]);
            let after = &rest[start + 2..];
            let end = after.find(')')?;
            let name = &after[..end];
            if name.contains('$') || name.contains(' ') {
                return None;
            }
            let value = local(name).or_else(|| self.resolved.get(name).cloned())?;
            out.push_str(&self.expand_with_depth(&value, local, depth - 1)?);
            rest = &after[end + 1..];
        }
        out.push_str(rest);
        Some(out)
    }

    /// The unresolvable names in `raw`, checking the local scope too.
    fn missing_in_with<F>(&self, raw: &str, local: &F) -> Vec<String>
    where
        F: Fn(&str) -> Option<String>,
    {
        let mut out = Vec::new();
        let mut queue = vec![raw.to_owned()];
        let mut seen = 0usize;
        while let Some(text) = queue.pop() {
            seen += 1;
            if seen > 64 {
                break;
            }
            let mut rest = text.as_str();
            while let Some(start) = rest.find("$(") {
                let after = &rest[start + 2..];
                let Some(end) = after.find(')') else {
                    break;
                };
                let name = &after[..end];
                if !name.contains('$') && !name.contains(' ') {
                    match local(name).or_else(|| self.resolved.get(name).cloned()) {
                        Some(v) => queue.push(v),
                        None => {
                            if !out.iter().any(|n| n == name) {
                                out.push(name.to_owned());
                            }
                        }
                    }
                }
                rest = &after[end + 1..];
            }
        }
        out
    }

    /// The names a raw value references that nothing defines.
    ///
    /// Used to say which variable is missing rather than only that a path could
    /// not be resolved.
    #[must_use]
    pub fn missing_in(&self, raw: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = raw;
        while let Some(start) = rest.find("$(") {
            let after = &rest[start + 2..];
            let Some(end) = after.find(')') else {
                break;
            };
            let name = &after[..end];
            if !name.contains('$')
                && !name.contains(' ')
                && !self.resolved.contains_key(name)
                && !out.iter().any(|n| n == name)
            {
                out.push(name.to_owned());
            }
            rest = &after[end + 1..];
        }
        out
    }
}

/// Splits `NAME := value`, `NAME = value` or `NAME ?= value`.
///
/// `+=` is rejected: appending needs the prior value and none of the directory
/// variables use it. A name with a character Make would not accept is rejected
/// too, which is what keeps rule lines such as `$(X)/%.info : ...` out.
fn split_assignment(line: &str) -> Option<(&str, &str)> {
    let idx = line.find('=')?;
    if idx == 0 {
        return None;
    }
    let (lhs, rhs) = line.split_at(idx);
    let rhs = &rhs[1..];
    let name = lhs.trim_end().trim_end_matches([':', '?']).trim_end();
    if lhs.trim_end().ends_with('+') {
        return None;
    }
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return None;
    }
    Some((name, rhs.trim()))
}

#[cfg(test)]
mod tests {
    use super::{split_assignment, DirVars};

    fn from_text(text: &str) -> DirVars {
        let mut d = DirVars {
            resolved: super::SEEDS
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
            unresolved: Vec::new(),
            undecided_conditions: Vec::new(),
        };
        d.absorb(text);
        d
    }

    #[test]
    fn resolves_a_chain_down_to_the_build_directory() {
        // The real chain from config/make.cfg.in, abridged.
        let d = from_text(
            "AROS_DIR_PREFS := Prefs\n\
             AROS_DIR_PRESETS := Presets\n\
             AROSDIR := $(TARGETDIR)/$(AROS_DIR_AROS)\n\
             AROS_PREFS := $(AROSDIR)/$(AROS_DIR_PREFS)\n\
             AROS_PRESETS := $(AROS_PREFS)/$(AROS_DIR_PRESETS)\n",
        );
        assert_eq!(
            d.expand("$(AROS_PRESETS)/Icons/Gorilla").unwrap(),
            "${AROS_BUILD_DIR}/SYS/Prefs/Presets/Icons/Gorilla"
        );
    }

    #[test]
    fn the_system_directory_is_named_sys_here() {
        let d = from_text("AROSDIR := $(TARGETDIR)/$(AROS_DIR_AROS)\n");
        // config/make.cfg.in:51 says AROS; this build says SYS, and the seed is
        // what decides it.
        assert_eq!(d.expand("$(AROSDIR)").unwrap(), "${AROS_BUILD_DIR}/SYS");
    }

    #[test]
    fn the_32_bit_companion_cpu_is_resolved_by_cmake() {
        let d = from_text("");
        assert_eq!(
            d.expand("$(AROS_TARGET_CPU32)").unwrap(),
            "${AROS_TARGET_CPU32}"
        );
    }

    #[test]
    fn the_legacy_platform_is_the_compound_metamake_selector() {
        let d = from_text("");
        assert_eq!(
            d.expand("$(AROS_TARGET_PLATFORM)").unwrap(),
            "${AROS_TARGET_LEGACY_PLATFORM}"
        );
        assert_eq!(
            d.expand("$(AROS_TARGET_ARCH)").unwrap(),
            "${AROS_TARGET_PLATFORM}"
        );
    }

    #[test]
    fn an_undefined_variable_is_named_not_dropped() {
        let d = from_text("AROS_DIR_TOOLS := Tools\n");
        assert!(d.expand("$(AROS_DIR__TOOLS)").is_none());
        assert_eq!(d.missing_in("$(AROS_DIR__TOOLS)"), vec!["AROS_DIR__TOOLS"]);
    }

    #[test]
    fn a_decidable_conditional_picks_one_branch() {
        // config/make.cfg.in:52-56 with an empty AROS_TARGET_SUFFIX.
        let d = from_text(
            "ifeq ($(AROS_TARGET_SUFFIX),)\n\
             AROS_DIR_ARCH := $(AROS_TARGET_ARCH)\n\
             else\n\
             AROS_DIR_ARCH := other\n\
             endif\n",
        );
        assert_eq!(
            d.expand("$(AROS_DIR_ARCH)").unwrap(),
            "${AROS_TARGET_PLATFORM}"
        );
        assert!(d.undecided_conditions.is_empty());
    }

    #[test]
    fn an_undecidable_conditional_is_reported_and_skipped() {
        let d = from_text(
            "ifeq ($(SOMETHING_UNKNOWN),yes)\n\
             AROS_DIR_X := taken\n\
             else\n\
             AROS_DIR_X := also-not-safe\n\
             endif\n",
        );
        assert!(d.expand("$(AROS_DIR_X)").is_none());
        assert_eq!(d.undecided_conditions.len(), 1);
    }

    #[test]
    fn a_cmake_target_parameter_is_not_decided_during_transpilation() {
        let d = from_text(
            "ifeq ($(AROS_TARGET_CPU),aarch64)\n\
             AROS_DIR_X := arm64\n\
             else\n\
             AROS_DIR_X := another-target\n\
             endif\n",
        );
        assert!(d.expand("$(AROS_DIR_X)").is_none());
        assert_eq!(d.undecided_conditions.len(), 1);
    }

    #[test]
    fn a_configure_placeholder_is_reported() {
        let d = from_text("CROSSTOOLSDIR := @AROS_CROSSTOOLSDIR@\n");
        assert!(d.expand("$(CROSSTOOLSDIR)").is_none());
        assert_eq!(d.unresolved.len(), 1);
        assert!(d.unresolved[0].contains("CROSSTOOLSDIR"));
    }

    #[test]
    fn a_seed_is_not_overwritten_by_the_file() {
        let d = from_text("AROS_DIR_AROS := AROS\n");
        assert_eq!(d.expand("$(AROS_DIR_AROS)").unwrap(), "SYS");
    }

    #[test]
    fn assignment_forms() {
        assert_eq!(split_assignment("A := b"), Some(("A", "b")));
        assert_eq!(split_assignment("A = b"), Some(("A", "b")));
        assert_eq!(split_assignment("A ?= b"), Some(("A", "b")));
        assert_eq!(split_assignment("A += b"), None);
        assert_eq!(split_assignment("$(X)/%.info : y"), None);
        assert_eq!(split_assignment("\tcommand"), None);
    }

    #[test]
    fn a_local_variable_resolves_against_the_shared_table() {
        // images/IconSets/Gorilla/Icons/Medium/AROS/Devs/Monitors declares
        // PCIDEVSDIR locally, and its value references AROS_STORAGE from
        // config/make.cfg.in.
        let d = from_text(
            "AROS_DIR_STORAGE := Storage\n\
             AROSDIR := $(TARGETDIR)/$(AROS_DIR_AROS)\n\
             AROS_STORAGE := $(AROSDIR)/$(AROS_DIR_STORAGE)\n",
        );
        let local = |name: &str| match name {
            "PCIDEVSDIR" => Some("$(AROS_STORAGE)/Monitors/PCI".to_owned()),
            _ => None,
        };
        assert_eq!(
            d.expand_with("$(PCIDEVSDIR)", &local).unwrap(),
            "${AROS_BUILD_DIR}/SYS/Storage/Monitors/PCI"
        );
    }

    #[test]
    fn a_local_variable_shadows_the_shared_table() {
        let d = from_text("AROS_DIR_TOOLS := Tools\n");
        let local = |name: &str| match name {
            "AROS_DIR_TOOLS" => Some("Local".to_owned()),
            _ => None,
        };
        assert_eq!(d.expand_with("$(AROS_DIR_TOOLS)", &local).unwrap(), "Local");
    }

    #[test]
    fn a_missing_name_is_returned_as_the_error() {
        let d = from_text("A := b\n");
        let none = |_: &str| None;
        let err = d.expand_with("$(A)/$(NOPE)/x", &none).unwrap_err();
        assert_eq!(err, vec!["NOPE"]);
    }

    #[test]
    fn a_function_call_does_not_resolve() {
        let d = from_text("A := $(shell date)\n");
        assert!(d.expand("$(A)").is_none());
    }
}
