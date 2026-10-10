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
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};

/// How deep a `$(VAR)` chain may nest before it is treated as unresolvable.
///
/// The real chains are three or four deep -- AROS_WALLPAPERS -> AROS_PRESETS ->
/// AROS_PREFS -> AROSDIR -> TARGETDIR. The cap exists for a cycle, not for
/// depth.
const MAX_DEPTH: usize = 12;
const MAX_EXPANSION_VALUE_BYTES: usize = 4 * 1024 * 1024;
const MAX_EXPANSION_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
const MAX_EXPANSION_SCANNED_BYTES: usize = 64 * 1024 * 1024;
const MAX_EXPANSION_WORK_UNITS: usize = 1_048_576;
const MAX_EXPANSION_LIST_ITEMS: usize = 65_536;
const MAX_SOURCE_DEPENDENCY_ITEMS: usize = MAX_EXPANSION_WORK_UNITS;
const MAX_SOURCE_DEPENDENCY_BYTES: usize = MAX_EXPANSION_OUTPUT_BYTES;
const MAX_SOURCE_DEPENDENCY_WORK: usize = MAX_EXPANSION_WORK_UNITS;
const EXPANSION_RESOURCE_ERROR: &str = "directory expansion exceeded its resource budget";

#[derive(Default)]
struct ExpansionBudget {
    output_bytes: usize,
    scanned_bytes: usize,
    work_units: usize,
    list_items: usize,
}

impl ExpansionBudget {
    fn charge_output(&mut self, bytes: usize) -> Option<()> {
        let total = self.output_bytes.checked_add(bytes)?;
        if total > MAX_EXPANSION_OUTPUT_BYTES {
            return None;
        }
        self.output_bytes = total;
        Some(())
    }

    fn charge_scan(&mut self, bytes: usize) -> Option<()> {
        let total = self.scanned_bytes.checked_add(bytes)?;
        if total > MAX_EXPANSION_SCANNED_BYTES {
            return None;
        }
        self.scanned_bytes = total;
        Some(())
    }

    fn charge_work(&mut self, units: usize) -> Option<()> {
        let total = self.work_units.checked_add(units)?;
        if total > MAX_EXPANSION_WORK_UNITS {
            return None;
        }
        self.work_units = total;
        Some(())
    }

    const fn charge_item(&mut self) -> Option<()> {
        if self.list_items >= MAX_EXPANSION_LIST_ITEMS {
            return None;
        }
        self.list_items += 1;
        Some(())
    }
}

fn append_expansion(output: &mut String, text: &str, budget: &mut ExpansionBudget) -> Option<()> {
    let next_len = output.len().checked_add(text.len())?;
    if next_len > MAX_EXPANSION_VALUE_BYTES {
        return None;
    }
    budget.charge_output(text.len())?;
    // Charge limits before asking the allocator to grow the output buffer.
    output.try_reserve(text.len()).ok()?;
    output.push_str(text);
    Some(())
}

fn push_diagnostic_value(
    queue: &mut VecDeque<String>,
    value: String,
    budget: &mut ExpansionBudget,
    already_charged: bool,
) -> Option<()> {
    if value.len() > MAX_EXPANSION_VALUE_BYTES {
        return None;
    }
    budget.charge_item()?;
    if !already_charged {
        budget.charge_output(value.len())?;
    }
    queue.try_reserve(1).ok()?;
    queue.push_back(value);
    Some(())
}

/// The values this build supplies for variables config/make.cfg.in expects from
/// configure or from the environment.
///
/// Keeping them here rather than in the resolver means the difference between
/// this build and the historic one is one readable list.
const SEEDS: &[(&str, &str)] = &[
    // config/make.cfg.in:17 builds TARGETDIR from $(TOP)/bin/<arch>-<cpu>; the
    // CMake binary directory is its counterpart.
    ("TOP", "${AROS_BUILD_DIR}"),
    ("SRCDIR", "${AROS_SOURCE_DIR}"),
    ("TARGETDIR", "${AROS_BUILD_DIR}"),
    ("GENDIR", "${AROS_BUILD_DIR}/gen"),
    ("HOSTDIR", "${AROS_BUILD_DIR}/hosttools"),
    ("TOOLDIR", "${AROS_BUILD_DIR}/hosttools"),
    // Keep source expressions and %fetch on the same configurable roots.
    // Expanding PORTSDIR through TARGETDIR here would hard-code the default
    // `${AROS_BUILD_DIR}/Ports` and diverge from `-DAROS_PORTS_DIR=...`.
    ("PORTSDIR", "${AROS_PORTS_DIR}"),
    ("PORTSSOURCEDIR", "${AROS_PORTS_SOURCE_DIR}"),
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
#[derive(Default)]
pub struct DirVars {
    resolved: HashMap<String, String>,
    /// Make-flavoured source values: recursive assignments retain their text,
    /// while simple assignments capture the expansion available at that line.
    /// This remains separate from the legacy resolver table below.
    source_proven_resolved: HashMap<String, String>,
    /// Names whose source value cannot be proven from the supported Make
    /// subset. Legacy expansion still uses `resolved` as before; consumers
    /// which need source authority must use `expand_source_proven`.
    unproven_variables: BTreeSet<String>,
    /// Names assigned with unsupported Make `override` priority. A later
    /// ordinary assignment cannot replace these values.
    sticky_unproven_variables: BTreeSet<String>,
    /// Active dynamic Make evaluation may mutate any variable, so strict
    /// source expansion is unavailable after an `eval`/dynamic macro use.
    /// This is intentionally generic; consumers decide which roots they query.
    source_evaluation_unproven: bool,
    /// Direct Make-variable references in the source assignment for each name.
    /// Kept separately because `:=` captures its value but the source
    /// dependency still matters when rejecting command-line overrides.
    source_dependencies: HashMap<String, BTreeSet<String>>,
    source_dependency_items: usize,
    source_dependency_bytes: usize,
    source_dependency_work: usize,
    source_dependency_scan_bytes: usize,
    /// Exhausting dependency tracking means source authority can no longer be
    /// established. Legacy expansion remains available to ordinary callers.
    source_dependency_tracking_unproven: bool,
    /// Physical configure-time counterparts for selected deferred CMake
    /// roots.  Values written to generated CMake remain the symbolic entries
    /// in `resolved`; this map exists only so filesystem-dependent Make
    /// functions can inspect an already fetched build tree.
    materialized: HashMap<String, PathBuf>,
    /// Assignments that could not be resolved, and why. Retained for callers'
    /// diagnostics rather than inserted as misleading literal paths.
    pub unresolved: Vec<String>,
    /// Make conditionals whose truth could not be decided, so both branches
    /// were skipped.
    pub undecided_conditions: Vec<String>,
}

impl DirVars {
    /// Makes admitted target-tool roles available to an explicitly bound
    /// native Make configuration. These are not ambient Make defaults and
    /// must never be enabled for an ordinary unqualified tree export.
    pub fn bind_native_target_tool_roles(&mut self) {
        for (name, value) in [
            ("NATIVE_TARGET_CC", "${CMAKE_C_COMPILER}"),
            ("NATIVE_TARGET_AR", "${CMAKE_AR}"),
            ("NATIVE_TARGET_RANLIB", "${CMAKE_RANLIB}"),
        ] {
            self.resolved.insert(name.to_owned(), value.to_owned());
            self.source_proven_resolved
                .insert(name.to_owned(), value.to_owned());
        }
    }

    /// Reads config/make.cfg.in under `root`.
    ///
    /// A missing or unreadable file yields an empty table rather than an error:
    /// the callers all degrade to reporting an unresolved path, which is the
    /// same outcome and says more about what went wrong.
    #[must_use]
    pub fn load(root: &Path) -> Self {
        let path = root.join("config/make.cfg.in");
        read_source(&path).map_or_else(
            |_| Self::from_config_text(""),
            |text| Self::from_config_text(&text),
        )
    }

    /// Resolves directory variables from an already-read `config/make.cfg.in`
    /// snapshot, using the same build-specific defaults as [`Self::load`].
    ///
    /// Source-bound consumers use this constructor after measuring and
    /// validating the exact bytes against their sealed input digest. Keeping
    /// the seed table shared prevents the checked path from diverging from
    /// ordinary graph generation.
    #[must_use]
    pub fn from_config_text(text: &str) -> Self {
        let seeds = SEEDS
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect::<HashMap<_, _>>();
        let mut me = Self {
            resolved: seeds.clone(),
            source_proven_resolved: seeds,
            unproven_variables: BTreeSet::new(),
            sticky_unproven_variables: BTreeSet::new(),
            source_evaluation_unproven: false,
            source_dependencies: HashMap::new(),
            source_dependency_items: 0,
            source_dependency_bytes: 0,
            source_dependency_work: 0,
            source_dependency_scan_bytes: 0,
            source_dependency_tracking_unproven: false,
            materialized: HashMap::new(),
            unresolved: Vec::new(),
            undecided_conditions: Vec::new(),
        };
        me.absorb(text);
        me
    }

    /// Supplies the physical path behind one deferred CMake directory.
    ///
    /// The logical value is deliberately not replaced: emitted paths must
    /// continue to use `${AROS_PORTS_DIR}` rather than capture one build
    /// directory in a portable generated graph.
    pub fn set_materialized_path(&mut self, name: &str, path: PathBuf) {
        self.materialized.insert(name.to_owned(), path);
    }

    /// Returns the configure-time filesystem path for a deferred root.
    #[must_use]
    pub fn materialized_path(&self, name: &str) -> Option<&Path> {
        self.materialized.get(name).map(PathBuf::as_path)
    }

    /// Returns the bounded transitive source-variable closure for proven
    /// assignments. Captured `:=` references remain dependencies even though
    /// their current values no longer contain Make variable syntax.
    #[must_use]
    pub fn source_dependency_closure(&self, roots: &[&str]) -> Option<BTreeSet<String>> {
        if self.source_dependency_tracking_unproven {
            return None;
        }
        self.source_dependency_closure_with_limits(
            roots,
            MAX_SOURCE_DEPENDENCY_ITEMS,
            MAX_SOURCE_DEPENDENCY_BYTES,
            MAX_SOURCE_DEPENDENCY_WORK,
        )
        .map(|(closure, _)| closure)
    }

    fn source_dependency_closure_with_limits(
        &self,
        roots: &[&str],
        max_items: usize,
        max_bytes: usize,
        max_work: usize,
    ) -> Option<(BTreeSet<String>, usize)> {
        if roots.len() > max_items {
            return None;
        }
        let mut closure = BTreeSet::new();
        let mut pending = VecDeque::new();
        let mut closure_bytes = 0usize;
        let mut work = 0usize;
        pending.try_reserve(roots.len()).ok()?;
        for root in roots {
            if !closure.contains(*root) {
                let next_bytes = closure_bytes.checked_add(root.len())?;
                if closure.len() >= max_items || next_bytes > max_bytes {
                    return None;
                }
                closure.insert((*root).to_owned());
                closure_bytes = next_bytes;
                pending.push_back((*root).to_owned());
            }
        }
        while let Some(name) = pending.pop_front() {
            work = work.checked_add(1)?;
            if work > max_work {
                return None;
            }
            if let Some(dependencies) = self.source_dependencies.get(&name) {
                for dependency in dependencies {
                    work = work.checked_add(1)?;
                    if work > max_work {
                        return None;
                    }
                    if !closure.contains(dependency) {
                        let next_bytes = closure_bytes.checked_add(dependency.len())?;
                        if closure.len() >= max_items || next_bytes > max_bytes {
                            return None;
                        }
                        pending.try_reserve(1).ok()?;
                        closure.insert(dependency.clone());
                        closure_bytes = next_bytes;
                        pending.push_back(dependency.clone());
                    }
                }
            }
        }
        Some((closure, work))
    }

    fn replace_source_dependencies(
        &mut self,
        name: &str,
        dependencies: Option<BTreeSet<String>>,
    ) -> bool {
        self.remove_source_dependencies(name);
        let Some(dependencies) = dependencies else {
            return true;
        };
        if dependencies.is_empty() {
            return true;
        }
        let Some(bytes) = dependencies
            .iter()
            .try_fold(name.len(), |total, dependency| {
                total.checked_add(dependency.len())
            })
        else {
            self.source_dependency_tracking_unproven = true;
            return false;
        };
        let Some(items) = self
            .source_dependency_items
            .checked_add(dependencies.len())
            .and_then(|items| items.checked_add(1))
        else {
            self.source_dependency_tracking_unproven = true;
            return false;
        };
        let Some(total_bytes) = self.source_dependency_bytes.checked_add(bytes) else {
            self.source_dependency_tracking_unproven = true;
            return false;
        };
        if items > MAX_SOURCE_DEPENDENCY_ITEMS || total_bytes > MAX_SOURCE_DEPENDENCY_BYTES {
            self.source_dependency_tracking_unproven = true;
            return false;
        }
        self.source_dependency_items = items;
        self.source_dependency_bytes = total_bytes;
        self.source_dependencies
            .insert(name.to_owned(), dependencies);
        true
    }

    fn remove_source_dependencies(&mut self, name: &str) {
        if let Some(previous) = self.source_dependencies.remove(name) {
            self.source_dependency_items = self
                .source_dependency_items
                .saturating_sub(previous.len().saturating_add(1));
            let previous_bytes = previous.iter().fold(name.len(), |total, dependency| {
                total.saturating_add(dependency.len())
            });
            self.source_dependency_bytes =
                self.source_dependency_bytes.saturating_sub(previous_bytes);
        }
    }

    /// Reads the assignments of one Make fragment in file order.
    fn absorb(&mut self, text: &str) {
        // These two stacks retain the historical resolver's behavior for
        // supported `ifeq`/`ifneq` syntax. `source_conditions` is stricter: it
        // also tracks unsupported conditional forms so source-bound consumers
        // cannot mistake a value from one possible branch for a proven root.
        let mut taken: Vec<bool> = Vec::new();
        let mut undecided: Vec<bool> = Vec::new();
        let mut source_conditions: Vec<SourceCondition> = Vec::new();
        let mut define_depth = 0usize;
        let mut define_name = None;
        let mut define_is_simple = false;
        let mut define_is_active = false;
        let mut define_may_evaluate = false;
        let mut dynamic_variables = BTreeSet::new();
        let mut continuation_pending = false;

        for raw in text.lines() {
            let line = raw.trim();
            let continued_from_previous = continuation_pending;
            continuation_pending = has_make_line_continuation(line);

            // A define body is a value, not a sequence of top-level Make
            // directives. Track nested define/endef pairs without trying to
            // interpret the macro body as directory assignments.
            if define_depth > 0 {
                if !continued_from_previous && define_variable_name(line).is_some() {
                    define_depth = define_depth.saturating_add(1);
                    if source_branch_certainty(&source_conditions) != BranchCertainty::Inactive {
                        if let Some(name) = define_variable_name(line) {
                            self.mark_unproven(name);
                        }
                    }
                } else if !continued_from_previous && line == "endef" {
                    define_depth -= 1;
                    if define_depth == 0 {
                        if define_is_active && define_may_evaluate {
                            if let Some(name) = define_name.take() {
                                if define_is_simple {
                                    self.mark_source_evaluation_unproven();
                                } else {
                                    self.mark_dynamic_variable(&mut dynamic_variables, name);
                                }
                            }
                        }
                        define_name = None;
                        define_is_simple = false;
                        define_is_active = false;
                        define_may_evaluate = false;
                    }
                } else if define_is_active
                    && (contains_make_eval(line)
                        || contains_make_call(line)
                        || self.references_dynamic_variable(line, &dynamic_variables))
                {
                    define_may_evaluate = true;
                }
                continue;
            }
            if line.starts_with('#') && !continued_from_previous {
                continue;
            }

            if !continued_from_previous {
                if let Some((args, want_equal)) = condition_directive(line) {
                    let parent_certainty = source_branch_certainty(&source_conditions);
                    if parent_certainty != BranchCertainty::Inactive
                        && (contains_make_eval(line)
                            || contains_make_call(line)
                            || self.references_dynamic_variable(line, &dynamic_variables))
                    {
                        self.mark_source_evaluation_unproven();
                    }
                    let legacy_value = self.condition_holds_legacy(args, want_equal);
                    if let Some(value) = legacy_value {
                        taken.push(value);
                        undecided.push(false);
                    } else {
                        taken.push(false);
                        undecided.push(true);
                    }
                    let source_value = self.condition_holds(args, want_equal);
                    if source_value.is_none() {
                        self.undecided_conditions.push(line.to_owned());
                    }
                    source_conditions.push(SourceCondition::from_condition(source_value, true));
                    continue;
                }

                if is_ifdef_or_ifndef(line) {
                    self.undecided_conditions.push(line.to_owned());
                    source_conditions.push(SourceCondition::unknown());
                    continue;
                }

                if let Some((args, want_equal)) = else_if_condition(line) {
                    let parent_certainty = source_branch_certainty(
                        &source_conditions[..source_conditions.len().saturating_sub(1)],
                    );
                    if parent_certainty != BranchCertainty::Inactive
                        && (contains_make_eval(line)
                            || contains_make_call(line)
                            || self.references_dynamic_variable(line, &dynamic_variables))
                    {
                        self.mark_source_evaluation_unproven();
                    }
                    let value = self.condition_holds(args, want_equal);
                    if value.is_none() {
                        self.undecided_conditions.push(line.to_owned());
                    }
                    let legacy_supported = if let Some(current) = source_conditions.last_mut() {
                        current.next_alternative(value);
                        current.legacy_supported
                    } else {
                        false
                    };
                    if legacy_supported {
                        match source_branch_certainty(&source_conditions) {
                            BranchCertainty::Inactive => {
                                if let Some(last) = taken.last_mut() {
                                    *last = false;
                                }
                                if let Some(last) = undecided.last_mut() {
                                    *last = false;
                                }
                            }
                            BranchCertainty::Uncertain => {
                                if let Some(last) = taken.last_mut() {
                                    *last = false;
                                }
                                if let Some(last) = undecided.last_mut() {
                                    *last = true;
                                }
                            }
                            BranchCertainty::Definite => {
                                if let Some(last) = taken.last_mut() {
                                    *last = true;
                                }
                                if let Some(last) = undecided.last_mut() {
                                    *last = false;
                                }
                            }
                        }
                    }
                    continue;
                }

                if else_if_unsupported_condition(line) {
                    let parent_certainty = source_branch_certainty(
                        &source_conditions[..source_conditions.len().saturating_sub(1)],
                    );
                    if parent_certainty != BranchCertainty::Inactive
                        && (contains_make_eval(line)
                            || contains_make_call(line)
                            || self.references_dynamic_variable(line, &dynamic_variables))
                    {
                        self.mark_source_evaluation_unproven();
                    }
                    self.undecided_conditions.push(line.to_owned());
                    if let Some(current) = source_conditions.last_mut() {
                        current.next_alternative(None);
                        current.legacy_supported = false;
                    } else {
                        source_conditions.push(SourceCondition::unknown());
                    }
                    continue;
                }

                if line == "else" {
                    if let (false, Some(last)) =
                        (undecided.last().copied().unwrap_or(false), taken.last_mut())
                    {
                        *last = !*last;
                    }
                    if let Some(current) = source_conditions.last_mut() {
                        current.else_alternative();
                    }
                    continue;
                }
                if line == "endif" {
                    taken.pop();
                    undecided.pop();
                    source_conditions.pop();
                    continue;
                }
            }

            let certainty = source_branch_certainty(&source_conditions);
            if let Some(name) = define_variable_name(line) {
                if certainty != BranchCertainty::Inactive {
                    self.mark_unproven(name);
                }
                define_name = Some(name.to_owned());
                define_is_simple = define_is_simple_assignment(line);
                define_is_active = certainty != BranchCertainty::Inactive;
                define_may_evaluate = false;
                define_depth = 1;
                continue;
            }

            if certainty != BranchCertainty::Inactive {
                if let Some(target) = shell_assignment_target(line) {
                    if contains_make_eval(line)
                        || contains_make_call(line)
                        || self.references_dynamic_variable(line, &dynamic_variables)
                    {
                        self.mark_source_evaluation_unproven();
                    }
                    self.mark_unsupported_assignment_target(target, has_override_prefix(line));
                    continue;
                }
                if let Some(target) = unsupported_assignment_target(line) {
                    if contains_make_eval(line)
                        || contains_make_call(line)
                        || self.references_dynamic_variable(line, &dynamic_variables)
                    {
                        self.mark_source_evaluation_unproven();
                    }
                    self.mark_unsupported_assignment_target(target, has_override_prefix(line));
                    // Preserve the historical raw-value resolver for this
                    // spelling without admitting it as source proof. The old
                    // splitter trimmed all trailing colons; native consumers
                    // must not inherit that unsupported Make interpretation.
                    if taken.iter().all(|branch_taken| *branch_taken) {
                        if let Some((name, value, _)) = split_assignment_details(line) {
                            if value.contains('@') {
                                self.unresolved
                                    .push(format!("{name} = {value} (configure placeholder)"));
                            } else if !SEEDS.iter().any(|(seed, _)| *seed == name) {
                                self.resolved.insert(name.to_owned(), value.to_owned());
                            }
                        }
                    }
                    continue;
                }
                if let Some(name) = modified_assignment_name(line) {
                    if contains_make_eval(line)
                        || contains_make_call(line)
                        || self.references_dynamic_variable(line, &dynamic_variables)
                    {
                        self.mark_source_evaluation_unproven();
                    }
                    if has_override_prefix(line) {
                        self.mark_sticky_unproven(name);
                    } else {
                        self.mark_unproven(name);
                    }
                    continue;
                }
                if let Some(names) = undefine_names(line) {
                    for name in names {
                        self.mark_unproven(name);
                    }
                    continue;
                }
                if let Some(name) = append_assignment_name(line) {
                    if contains_make_eval(line)
                        || contains_make_call(line)
                        || self.references_dynamic_variable(line, &dynamic_variables)
                    {
                        self.mark_source_evaluation_unproven();
                    }
                    self.mark_unproven(name);
                    continue;
                }
                if continued_from_previous || continuation_pending {
                    if let Some(name) = assignment_target_name(line) {
                        self.mark_unproven(name);
                    }
                }
            }

            if certainty == BranchCertainty::Inactive {
                continue;
            }

            let Some((name, value, operator)) = split_assignment_details(line) else {
                if certainty != BranchCertainty::Inactive
                    && (contains_make_eval(line)
                        || contains_make_call(line)
                        || self.references_dynamic_variable(line, &dynamic_variables))
                {
                    self.mark_source_evaluation_unproven();
                }
                continue;
            };
            if certainty == BranchCertainty::Uncertain {
                self.mark_unproven(name);
            }
            // Keep the pre-existing resolver behavior for unsupported
            // `ifdef`/`ifndef` and `else ifeq` source, but never let a value
            // absorbed from those possibly active branches pass strict source
            // expansion.
            if taken.iter().any(|branch_taken| !branch_taken) {
                continue;
            }
            let already_defined = SEEDS.iter().any(|(seed, _)| *seed == name)
                || self.source_proven_resolved.contains_key(name)
                || self.unproven_variables.contains(name);
            // Make's `?=` assigns only when a variable has no prior value. Do
            // not let this supported form replace a previously proven or
            // possibly-defined value in the source snapshot.
            if operator == AssignmentOperator::IfUndefined && already_defined {
                continue;
            }
            // An autoconf placeholder is filled in by configure, which this
            // build does not run. Recorded as unresolved rather than stored, so
            // a path built from it is reported instead of coming out with an
            // `@...@` in it.
            if value.contains('@') {
                self.unresolved
                    .push(format!("{name} = {value} (configure placeholder)"));
                self.mark_unproven(name);
                continue;
            }
            // Seeds win: they are this build's answer where the historic file
            // has a different one.
            if SEEDS.iter().any(|(k, _)| *k == name) {
                continue;
            }
            // `${...}` in the sealed Make source is not part of the supported
            // variable syntax. The same spelling may appear after expanding a
            // trusted seed such as TARGETDIR; only raw source text is rejected.
            if value.contains("${") {
                self.mark_unproven(name);
                self.source_proven_resolved.remove(name);
                self.remove_source_dependencies(name);
                self.resolved.insert(name.to_owned(), value.to_owned());
                continue;
            }
            // An ordinary assignment replaces an earlier source value, so it
            // also resolves prior uncertainty. `?=` reaches here only when
            // the source variable has not already been defined and the branch
            // is definitely active.
            if certainty == BranchCertainty::Definite
                && !continued_from_previous
                && !continuation_pending
            {
                self.unproven_variables.remove(name);
                dynamic_variables.remove(name);
            } else if continued_from_previous || continuation_pending {
                self.mark_unproven(name);
            }
            let mut dependency_tracking_exhausted = false;
            let dependencies = if let Some(direct) = variable_references(value) {
                let mut dependencies = direct;
                if operator == AssignmentOperator::Simple && !dependencies.is_empty() {
                    let roots = dependencies.iter().map(String::as_str).collect::<Vec<_>>();
                    let remaining_work =
                        MAX_SOURCE_DEPENDENCY_WORK.saturating_sub(self.source_dependency_work);
                    match self.source_dependency_closure_with_limits(
                        &roots,
                        MAX_SOURCE_DEPENDENCY_ITEMS,
                        MAX_SOURCE_DEPENDENCY_BYTES,
                        remaining_work,
                    ) {
                        Some((closure, work)) => {
                            self.source_dependency_work += work;
                            dependencies = closure;
                        }
                        None => dependency_tracking_exhausted = true,
                    }
                }
                Some(dependencies)
            } else {
                None
            };
            let may_evaluate = contains_make_eval(value)
                || contains_make_call(value)
                || self.references_dynamic_variable(value, &dynamic_variables);
            if dependency_tracking_exhausted {
                self.source_dependency_tracking_unproven = true;
            }
            let dependencies_available = dependencies.is_some();
            if !self.replace_source_dependencies(name, dependencies) || !dependencies_available {
                self.mark_unproven(name);
            }
            if may_evaluate {
                if operator == AssignmentOperator::Simple {
                    self.mark_source_evaluation_unproven();
                } else {
                    self.mark_dynamic_variable(&mut dynamic_variables, name.to_owned());
                    self.mark_unproven(name);
                }
            }
            let source_value = match operator {
                AssignmentOperator::Simple => self.expand_source_proven(value),
                AssignmentOperator::Recursive | AssignmentOperator::IfUndefined => {
                    Some(value.to_owned())
                }
            };
            if let Some(source_value) = source_value {
                self.source_proven_resolved
                    .insert(name.to_owned(), source_value);
            } else {
                // Simple assignments capture their expansion now. A later
                // definition of a dependency must not retroactively make an
                // unknown captured value look source-proven.
                self.mark_unproven(name);
                self.source_proven_resolved.remove(name);
            }
            self.resolved.insert(name.to_owned(), value.to_owned());
        }
    }

    /// Whether an `ifeq (a,b)` / `ifneq (a,b)` holds, or None if either side
    /// still contains a variable nothing defines.
    fn condition_holds(&self, args: &str, want_equal: bool) -> Option<bool> {
        let inner = args.trim().strip_prefix('(')?.strip_suffix(')')?;
        let (a, b) = inner.split_once(',')?;
        let a = self.expand_source_proven(a.trim())?;
        let b = self.expand_source_proven(b.trim())?;
        // Target parameters remain CMake expressions in the table. Their
        // value is deliberately not guessed while the Rust transpiler runs.
        if a.contains("${") || b.contains("${") {
            return None;
        }
        Some((a == b) == want_equal)
    }

    fn condition_holds_legacy(&self, args: &str, want_equal: bool) -> Option<bool> {
        let inner = args.trim().strip_prefix('(')?.strip_suffix(')')?;
        let (a, b) = inner.split_once(',')?;
        let a = self.expand(a.trim())?;
        let b = self.expand(b.trim())?;
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
        self.expand_depth(raw, MAX_DEPTH, &mut ExpansionBudget::default(), false)
    }

    /// Expands source assignments only when every referenced variable has a
    /// value proven by the supported Make subset. Unlike [`Self::expand`], this
    /// rejects values that may have been changed by an unsupported directive
    /// or a possibly active undecidable branch.
    #[must_use]
    pub fn expand_source_proven(&self, raw: &str) -> Option<String> {
        self.expand_depth(raw, MAX_DEPTH, &mut ExpansionBudget::default(), true)
    }

    fn expand_depth(
        &self,
        raw: &str,
        depth: usize,
        budget: &mut ExpansionBudget,
        require_source_proven: bool,
    ) -> Option<String> {
        if require_source_proven
            && (self.source_evaluation_unproven || self.source_dependency_tracking_unproven)
        {
            return None;
        }
        if depth == 0 {
            return None;
        }
        if raw.len() > MAX_EXPANSION_VALUE_BYTES {
            return None;
        }
        budget.charge_scan(raw.len())?;
        budget.charge_work(1)?;

        let bytes = raw.as_bytes();
        let mut out = String::new();
        let mut cursor = 0usize;
        let mut segment_start = 0usize;
        while cursor + 1 < bytes.len() {
            if bytes[cursor] != b'$' || bytes[cursor + 1] != b'(' {
                cursor += 1;
                continue;
            }

            append_expansion(&mut out, &raw[segment_start..cursor], budget)?;
            cursor += 2;
            let name_start = cursor;
            while cursor < bytes.len() && bytes[cursor] != b')' {
                cursor += 1;
            }
            if cursor == bytes.len() {
                return None;
            }
            let name = raw.get(name_start..cursor)?;
            // A nested reference or a function call is not a plain variable.
            if name.contains('$') || name.contains(' ') {
                return None;
            }
            budget.charge_work(1)?;
            if require_source_proven
                && (self.unproven_variables.contains(name)
                    || self.sticky_unproven_variables.contains(name))
            {
                return None;
            }
            let value = if require_source_proven {
                self.source_proven_resolved.get(name)?
            } else {
                self.resolved.get(name)?
            };
            let expanded = self.expand_depth(value, depth - 1, budget, require_source_proven)?;
            append_expansion(&mut out, &expanded, budget)?;
            cursor += 1;
            segment_start = cursor;
        }
        append_expansion(&mut out, &raw[segment_start..], budget)?;
        Some(out)
    }

    fn mark_unproven(&mut self, name: &str) {
        if !SEEDS.iter().any(|(seed, _)| *seed == name) {
            self.unproven_variables.insert(name.to_owned());
        }
    }

    fn mark_sticky_unproven(&mut self, name: &str) {
        if !SEEDS.iter().any(|(seed, _)| *seed == name) {
            self.sticky_unproven_variables.insert(name.to_owned());
        }
    }

    fn mark_unsupported_assignment_target(
        &mut self,
        target: UnsupportedAssignmentTarget<'_>,
        override_priority: bool,
    ) {
        match target {
            UnsupportedAssignmentTarget::Known(name)
                if SEEDS.iter().any(|(seed, _)| *seed == name) =>
            {
                self.mark_source_evaluation_unproven();
            }
            UnsupportedAssignmentTarget::Known(name) if override_priority => {
                self.mark_sticky_unproven(name);
            }
            UnsupportedAssignmentTarget::Known(name) => self.mark_unproven(name),
            UnsupportedAssignmentTarget::Unknown => self.mark_source_evaluation_unproven(),
        }
    }

    fn mark_dynamic_variable(&mut self, variables: &mut BTreeSet<String>, name: String) {
        if !variables.contains(&name) && variables.len() >= MAX_EXPANSION_LIST_ITEMS {
            self.mark_source_evaluation_unproven();
            return;
        }
        variables.insert(name);
    }

    const fn mark_source_evaluation_unproven(&mut self) {
        self.source_evaluation_unproven = true;
    }

    fn references_dynamic_variable(
        &mut self,
        raw: &str,
        dynamic_variables: &BTreeSet<String>,
    ) -> bool {
        if dynamic_variables.is_empty() {
            return false;
        }
        let Some(initial_names) = self.dynamic_expansion_names(raw) else {
            return true;
        };
        let mut pending = VecDeque::new();
        if pending.try_reserve(initial_names.len()).is_err() {
            self.source_dependency_tracking_unproven = true;
            return true;
        }
        pending.extend(initial_names);
        let mut visited = BTreeSet::new();
        while let Some(name) = pending.pop_front() {
            if !self.charge_source_dependency_work(1) {
                return true;
            }
            if dynamic_variables.contains(&name) {
                return true;
            }
            if !visited.insert(name.clone()) {
                continue;
            }
            let Some(value) = self.source_proven_resolved.get(&name).cloned() else {
                continue;
            };
            let Some(names) = self.dynamic_expansion_names(&value) else {
                return true;
            };
            if pending.try_reserve(names.len()).is_err() {
                self.source_dependency_tracking_unproven = true;
                return true;
            }
            pending.extend(names);
        }
        false
    }

    fn dynamic_expansion_names(&mut self, raw: &str) -> Option<BTreeSet<String>> {
        let Some(total_bytes) = self.source_dependency_scan_bytes.checked_add(raw.len()) else {
            self.source_dependency_tracking_unproven = true;
            return None;
        };
        if total_bytes > MAX_EXPANSION_SCANNED_BYTES {
            self.source_dependency_tracking_unproven = true;
            return None;
        }
        self.source_dependency_scan_bytes = total_bytes;
        let Some(names) = make_expansion_names(raw) else {
            self.source_dependency_tracking_unproven = true;
            return None;
        };
        if !self.charge_source_dependency_work(names.len()) {
            return None;
        }
        Some(names)
    }

    const fn charge_source_dependency_work(&mut self, amount: usize) -> bool {
        let Some(total) = self.source_dependency_work.checked_add(amount) else {
            self.source_dependency_tracking_unproven = true;
            return false;
        };
        if total > MAX_SOURCE_DEPENDENCY_WORK {
            self.source_dependency_tracking_unproven = true;
            return false;
        }
        self.source_dependency_work = total;
        true
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
        let mut budget = ExpansionBudget::default();
        if let Some(value) = self.expand_with_depth(raw, local, MAX_DEPTH, &mut budget) {
            return Ok(value);
        }

        let Some(mut missing) = self.missing_in_with(raw, local, &mut budget) else {
            return Err(vec![EXPANSION_RESOURCE_ERROR.to_owned()]);
        };
        if missing.is_empty() {
            // Resolvable names, but something in the value is not a plain
            // reference: a function call, a cycle, or a bounded-work refusal.
            // Preserve the old contextual message only when it also fits the
            // same call's output and diagnostic budgets.
            let suffix = " (not a plain variable reference)";
            let message_len = raw.len().checked_add(suffix.len());
            if raw.len() <= MAX_EXPANSION_VALUE_BYTES
                && budget.charge_item().is_some()
                && message_len.is_some_and(|size| {
                    size <= MAX_EXPANSION_VALUE_BYTES && budget.charge_output(size).is_some()
                })
            {
                let mut message = String::new();
                if message_len.is_some_and(|size| message.try_reserve(size).is_ok()) {
                    message.push_str(raw);
                    message.push_str(suffix);
                    if missing.try_reserve(1).is_ok() {
                        missing.push(message);
                    } else {
                        return Err(vec![EXPANSION_RESOURCE_ERROR.to_owned()]);
                    }
                } else {
                    return Err(vec![EXPANSION_RESOURCE_ERROR.to_owned()]);
                }
            } else {
                return Err(vec![EXPANSION_RESOURCE_ERROR.to_owned()]);
            }
        }
        Err(missing)
    }

    fn expand_with_depth<F>(
        &self,
        raw: &str,
        local: &F,
        depth: usize,
        budget: &mut ExpansionBudget,
    ) -> Option<String>
    where
        F: Fn(&str) -> Option<String>,
    {
        if depth == 0 {
            return None;
        }
        if raw.len() > MAX_EXPANSION_VALUE_BYTES {
            return None;
        }
        budget.charge_scan(raw.len())?;
        budget.charge_work(1)?;

        let bytes = raw.as_bytes();
        let mut out = String::new();
        let mut cursor = 0usize;
        let mut segment_start = 0usize;
        while cursor + 1 < bytes.len() {
            if bytes[cursor] != b'$' || bytes[cursor + 1] != b'(' {
                cursor += 1;
                continue;
            }

            append_expansion(&mut out, &raw[segment_start..cursor], budget)?;
            cursor += 2;
            let name_start = cursor;
            while cursor < bytes.len() && bytes[cursor] != b')' {
                cursor += 1;
            }
            if cursor == bytes.len() {
                return None;
            }
            let name = raw.get(name_start..cursor)?;
            if name.contains('$') || name.contains(' ') {
                return None;
            }
            budget.charge_work(1)?;
            let expanded = if let Some(value) = local(name) {
                // The callback's String allocation occurs before it returns;
                // reject it immediately and account for its retained bytes
                // before doing any further work. This module cannot bound the
                // callback's allocation itself.
                if value.len() > MAX_EXPANSION_VALUE_BYTES {
                    return None;
                }
                budget.charge_output(value.len())?;
                self.expand_with_depth(&value, local, depth - 1, budget)?
            } else {
                self.expand_with_depth(self.resolved.get(name)?, local, depth - 1, budget)?
            };
            append_expansion(&mut out, &expanded, budget)?;
            cursor += 1;
            segment_start = cursor;
        }
        append_expansion(&mut out, &raw[segment_start..], budget)?;
        Some(out)
    }

    /// The unresolvable names in `raw`, checking the local scope too.
    fn missing_in_with<F>(
        &self,
        raw: &str,
        local: &F,
        budget: &mut ExpansionBudget,
    ) -> Option<Vec<String>>
    where
        F: Fn(&str) -> Option<String>,
    {
        if raw.len() > MAX_EXPANSION_VALUE_BYTES {
            return None;
        }
        budget.charge_item()?;
        budget.charge_output(raw.len())?;
        let mut queue = VecDeque::new();
        queue.try_reserve(1).ok()?;
        queue.push_back(raw.to_owned());
        let mut missing = BTreeSet::<String>::new();

        while let Some(text) = queue.pop_back() {
            if text.len() > MAX_EXPANSION_VALUE_BYTES {
                return None;
            }
            budget.charge_scan(text.len())?;
            budget.charge_work(1)?;
            let bytes = text.as_bytes();
            let mut cursor = 0usize;
            while cursor + 1 < bytes.len() {
                if bytes[cursor] != b'$' || bytes[cursor + 1] != b'(' {
                    cursor += 1;
                    continue;
                }
                cursor += 2;
                let name_start = cursor;
                while cursor < bytes.len() && bytes[cursor] != b')' {
                    cursor += 1;
                }
                if cursor == bytes.len() {
                    break;
                }
                let name = text.get(name_start..cursor)?;
                if !name.contains('$') && !name.contains(' ') {
                    budget.charge_work(1)?;
                    if let Some(value) = local(name) {
                        if value.len() > MAX_EXPANSION_VALUE_BYTES {
                            return None;
                        }
                        // The local callback allocates before returning. Its
                        // size is checked/accounted before queue growth.
                        budget.charge_output(value.len())?;
                        push_diagnostic_value(&mut queue, value, budget, true)?;
                    } else if let Some(value) = self.resolved.get(name) {
                        if value.len() > MAX_EXPANSION_VALUE_BYTES {
                            return None;
                        }
                        budget.charge_output(value.len())?;
                        push_diagnostic_value(&mut queue, value.clone(), budget, true)?;
                    } else if !missing.contains(name) {
                        budget.charge_item()?;
                        budget.charge_output(name.len())?;
                        missing.insert(name.to_owned());
                    }
                }
                cursor += 1;
            }
        }

        let mut result = Vec::new();
        result.try_reserve_exact(missing.len()).ok()?;
        result.extend(missing);
        Some(result)
    }

    /// The names a raw value references that nothing defines.
    ///
    /// Used to say which variable is missing rather than only that a path could
    /// not be resolved.
    #[must_use]
    pub fn missing_in(&self, raw: &str) -> Vec<String> {
        let Some(missing) = self.missing_in_bounded(raw, &mut ExpansionBudget::default()) else {
            return vec![EXPANSION_RESOURCE_ERROR.to_owned()];
        };
        missing
    }

    fn missing_in_bounded(&self, raw: &str, budget: &mut ExpansionBudget) -> Option<Vec<String>> {
        if raw.len() > MAX_EXPANSION_VALUE_BYTES {
            return None;
        }
        budget.charge_scan(raw.len())?;
        budget.charge_work(1)?;
        let bytes = raw.as_bytes();
        let mut cursor = 0usize;
        let mut missing = BTreeSet::<String>::new();
        while cursor + 1 < bytes.len() {
            if bytes[cursor] != b'$' || bytes[cursor + 1] != b'(' {
                cursor += 1;
                continue;
            }
            cursor += 2;
            let name_start = cursor;
            while cursor < bytes.len() && bytes[cursor] != b')' {
                cursor += 1;
            }
            if cursor == bytes.len() {
                break;
            }
            let name = raw.get(name_start..cursor)?;
            if !name.contains('$') && !name.contains(' ') {
                budget.charge_work(1)?;
                if !self.resolved.contains_key(name) && !missing.contains(name) {
                    budget.charge_item()?;
                    budget.charge_output(name.len())?;
                    missing.insert(name.to_owned());
                }
            }
            cursor += 1;
        }
        let mut result = Vec::new();
        result.try_reserve_exact(missing.len()).ok()?;
        result.extend(missing);
        Some(result)
    }
}

/// Splits `NAME := value`, `NAME = value` or `NAME ?= value`.
///
/// `+=` is rejected: appending needs the prior value and none of the directory
/// variables use it. A name with a character Make would not accept is rejected
/// too, which is what keeps rule lines such as `$(X)/%.info : ...` out.
#[cfg(test)]
fn split_assignment(line: &str) -> Option<(&str, &str)> {
    split_assignment_details(line).map(|(name, value, _)| (name, value))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AssignmentOperator {
    IfUndefined,
    Recursive,
    Simple,
}

fn split_assignment_details(line: &str) -> Option<(&str, &str, AssignmentOperator)> {
    let idx = line.find('=')?;
    if idx == 0 {
        return None;
    }
    let (lhs, rhs) = line.split_at(idx);
    let rhs = &rhs[1..];
    let lhs = lhs.trim_end();
    if lhs.ends_with('+') {
        return None;
    }
    let operator = if lhs.ends_with('?') {
        AssignmentOperator::IfUndefined
    } else if lhs.ends_with(':') {
        AssignmentOperator::Simple
    } else {
        AssignmentOperator::Recursive
    };
    let name = lhs.trim_end_matches([':', '?']).trim_end();
    if !valid_variable_name(name) {
        return None;
    }
    Some((name, rhs.trim(), operator))
}

fn valid_variable_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BranchCertainty {
    Inactive,
    Uncertain,
    Definite,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PriorSelection {
    None,
    Selected,
    Maybe,
}

struct SourceCondition {
    prior_selection: PriorSelection,
    current: BranchCertainty,
    legacy_supported: bool,
}

impl SourceCondition {
    const fn from_condition(value: Option<bool>, legacy_supported: bool) -> Self {
        Self {
            prior_selection: PriorSelection::None,
            current: condition_certainty(value),
            legacy_supported,
        }
    }

    const fn unknown() -> Self {
        Self::from_condition(None, false)
    }

    const fn else_alternative(&mut self) {
        self.prior_selection = merge_prior_selection(self.prior_selection, self.current);
        self.current = BranchCertainty::Definite;
    }

    const fn next_alternative(&mut self, value: Option<bool>) {
        self.prior_selection = merge_prior_selection(self.prior_selection, self.current);
        self.current = condition_certainty(value);
    }

    fn certainty(&self) -> BranchCertainty {
        match self.prior_selection {
            PriorSelection::Selected => BranchCertainty::Inactive,
            PriorSelection::None => self.current,
            PriorSelection::Maybe if self.current == BranchCertainty::Inactive => {
                BranchCertainty::Inactive
            }
            PriorSelection::Maybe => BranchCertainty::Uncertain,
        }
    }
}

const fn condition_certainty(value: Option<bool>) -> BranchCertainty {
    match value {
        Some(true) => BranchCertainty::Definite,
        Some(false) => BranchCertainty::Inactive,
        None => BranchCertainty::Uncertain,
    }
}

const fn merge_prior_selection(prior: PriorSelection, current: BranchCertainty) -> PriorSelection {
    match prior {
        PriorSelection::Selected => PriorSelection::Selected,
        PriorSelection::Maybe => PriorSelection::Maybe,
        PriorSelection::None => match current {
            BranchCertainty::Inactive => PriorSelection::None,
            BranchCertainty::Uncertain => PriorSelection::Maybe,
            BranchCertainty::Definite => PriorSelection::Selected,
        },
    }
}

fn source_branch_certainty(conditions: &[SourceCondition]) -> BranchCertainty {
    let mut uncertain = false;
    for condition in conditions {
        match condition.certainty() {
            BranchCertainty::Inactive => return BranchCertainty::Inactive,
            BranchCertainty::Uncertain => uncertain = true,
            BranchCertainty::Definite => {}
        }
    }
    if uncertain {
        BranchCertainty::Uncertain
    } else {
        BranchCertainty::Definite
    }
}

fn condition_directive(line: &str) -> Option<(&str, bool)> {
    line.strip_prefix("ifeq")
        .map(|args| (args, true))
        .or_else(|| line.strip_prefix("ifneq").map(|args| (args, false)))
}

fn else_if_condition(line: &str) -> Option<(&str, bool)> {
    let alternative = line.strip_prefix("else")?;
    if !alternative.chars().next().is_some_and(char::is_whitespace) {
        return None;
    }
    condition_directive(alternative.trim_start())
}

fn else_if_unsupported_condition(line: &str) -> bool {
    let Some(alternative) = line.strip_prefix("else") else {
        return false;
    };
    if !alternative.chars().next().is_some_and(char::is_whitespace) {
        return false;
    }
    is_ifdef_or_ifndef(alternative.trim_start())
}

fn is_ifdef_or_ifndef(line: &str) -> bool {
    ["ifdef", "ifndef"].iter().any(|directive| {
        line.strip_prefix(directive)
            .and_then(|arguments| arguments.chars().next())
            .is_some_and(char::is_whitespace)
    })
}

fn directive_arguments<'a>(line: &'a str, keyword: &str) -> Option<&'a str> {
    let arguments = line.strip_prefix(keyword)?;
    arguments
        .chars()
        .next()
        .filter(|character| character.is_whitespace())?;
    Some(arguments.trim_start())
}

fn has_make_line_continuation(line: &str) -> bool {
    let trailing_slashes = line
        .trim_end()
        .chars()
        .rev()
        .take_while(|character| *character == '\\')
        .count();
    trailing_slashes % 2 == 1
}

fn assignment_target_name(line: &str) -> Option<&str> {
    modified_assignment_name(line)
        .or_else(|| append_assignment_name(line))
        .or_else(|| split_assignment_details(line).map(|(name, _, _)| name))
}

fn modified_assignment_name(line: &str) -> Option<&str> {
    let remainder = assignment_after_modifiers(line)?;
    split_assignment_details(remainder)
        .map(|(name, _, _)| name)
        .or_else(|| append_assignment_name(remainder))
}

fn assignment_after_modifiers(line: &str) -> Option<&str> {
    let mut remainder = line;
    let mut modified = false;
    loop {
        let mut next = None;
        for keyword in ["override", "export"] {
            if let Some(arguments) = directive_arguments(remainder, keyword) {
                next = Some(arguments);
                break;
            }
        }
        let Some(arguments) = next else {
            break;
        };
        modified = true;
        remainder = arguments;
    }
    modified.then_some(remainder)
}

#[derive(Clone, Copy)]
enum UnsupportedAssignmentTarget<'a> {
    Known(&'a str),
    Unknown,
}

fn shell_assignment_target(line: &str) -> Option<UnsupportedAssignmentTarget<'_>> {
    let remainder = assignment_after_modifiers(line).unwrap_or(line);
    let (name, _) = remainder.split_once("!=")?;
    let name = name.trim();
    Some(if valid_variable_name(name) {
        UnsupportedAssignmentTarget::Known(name)
    } else {
        UnsupportedAssignmentTarget::Unknown
    })
}

fn unsupported_assignment_target(line: &str) -> Option<UnsupportedAssignmentTarget<'_>> {
    let remainder = assignment_after_modifiers(line).unwrap_or(line);
    let (name, _) = remainder.split_once(":::=")?;
    let name = name.trim();
    Some(if valid_variable_name(name) {
        UnsupportedAssignmentTarget::Known(name)
    } else {
        UnsupportedAssignmentTarget::Unknown
    })
}

fn has_override_prefix(line: &str) -> bool {
    let mut remainder = line;
    loop {
        if directive_arguments(remainder, "override").is_some() {
            return true;
        }
        let Some(arguments) = directive_arguments(remainder, "export") else {
            return false;
        };
        remainder = arguments;
    }
}

fn append_assignment_name(line: &str) -> Option<&str> {
    let (name, _) = line.split_once("+=")?;
    let name = name.trim();
    valid_variable_name(name).then_some(name)
}

fn undefine_names(line: &str) -> Option<Vec<&str>> {
    let arguments = line.strip_prefix("undefine")?;
    if !arguments.chars().next().is_some_and(char::is_whitespace) {
        return None;
    }
    Some(
        arguments
            .split_whitespace()
            .filter(|name| valid_variable_name(name))
            .collect(),
    )
}

fn define_variable_name(line: &str) -> Option<&str> {
    let arguments = line.strip_prefix("define")?;
    if !arguments.chars().next().is_some_and(char::is_whitespace) {
        return None;
    }
    let token = arguments.split_whitespace().next()?;
    let name = token.trim_end_matches(['=', ':', '?', '+']);
    valid_variable_name(name).then_some(name)
}

fn define_is_simple_assignment(line: &str) -> bool {
    let Some(arguments) = directive_arguments(line, "define") else {
        return false;
    };
    let mut parts = arguments.split_whitespace();
    let _name = parts.next();
    matches!(parts.next(), Some(":=" | "::=" | ":::="))
}

fn variable_references(raw: &str) -> Option<BTreeSet<String>> {
    if raw.len() > MAX_EXPANSION_VALUE_BYTES {
        return None;
    }
    let bytes = raw.as_bytes();
    let mut references = BTreeSet::new();
    let mut cursor = 0usize;
    while cursor + 1 < bytes.len() {
        if bytes[cursor] != b'$' || bytes[cursor + 1] != b'(' {
            cursor += 1;
            continue;
        }
        cursor += 2;
        let start = cursor;
        while cursor < bytes.len() && bytes[cursor] != b')' {
            cursor += 1;
        }
        if cursor == bytes.len() {
            return None;
        }
        let name = raw.get(start..cursor)?;
        if !valid_variable_name(name) {
            return None;
        }
        references.insert(name.to_owned());
        if references.len() > MAX_EXPANSION_LIST_ITEMS {
            return None;
        }
        cursor += 1;
    }
    Some(references)
}

fn contains_make_eval(raw: &str) -> bool {
    if raw.len() > MAX_EXPANSION_VALUE_BYTES {
        return true;
    }
    make_expansion_names(raw).is_none_or(|names| names.iter().any(|name| name == "eval"))
}

fn contains_make_call(raw: &str) -> bool {
    if raw.len() > MAX_EXPANSION_VALUE_BYTES {
        return true;
    }
    make_expansion_names(raw).is_none_or(|names| names.iter().any(|name| name == "call"))
}

fn make_expansion_names(raw: &str) -> Option<BTreeSet<String>> {
    if raw.len() > MAX_EXPANSION_VALUE_BYTES {
        return None;
    }
    let bytes = raw.as_bytes();
    let mut names = BTreeSet::new();
    let mut stack: Vec<(u8, usize)> = Vec::new();
    let mut name_bytes = 0usize;
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if cursor + 1 < bytes.len()
            && bytes[cursor] == b'$'
            && matches!(bytes[cursor + 1], b'(' | b'{')
        {
            if stack.len() >= MAX_EXPANSION_LIST_ITEMS || stack.try_reserve(1).is_err() {
                return None;
            }
            let close = if bytes[cursor + 1] == b'(' {
                b')'
            } else {
                b'}'
            };
            stack.push((close, cursor + 2));
            cursor += 2;
            continue;
        }
        if matches!(bytes[cursor], b')' | b'}') {
            if let Some((close, start)) = stack.last().copied() {
                if bytes[cursor] == close {
                    stack.pop();
                    let expression = raw.get(start..cursor)?.trim_start();
                    let mut parts = expression
                        .split(|character: char| character.is_whitespace() || character == ',');
                    let name = parts.next()?;
                    if !name.is_empty() {
                        name_bytes = name_bytes.checked_add(name.len())?;
                        if name_bytes > MAX_SOURCE_DEPENDENCY_BYTES {
                            return None;
                        }
                        names.insert(name.to_owned());
                    }
                    if name == "call" {
                        if let Some(macro_name) = parts.next() {
                            if !macro_name.is_empty() {
                                name_bytes = name_bytes.checked_add(macro_name.len())?;
                                if name_bytes > MAX_SOURCE_DEPENDENCY_BYTES {
                                    return None;
                                }
                                names.insert(macro_name.to_owned());
                            }
                        }
                    }
                    if names.len() > MAX_EXPANSION_LIST_ITEMS {
                        return None;
                    }
                } else if stack.iter().any(|(expected, _)| *expected == bytes[cursor]) {
                    // A close matching an outer frame before the current frame
                    // is malformed nesting. Fail closed instead of losing an
                    // inner eval while scanning its enclosing function call.
                    return None;
                }
            }
        }
        cursor += 1;
    }
    if !stack.is_empty() {
        return None;
    }
    Some(names)
}

#[cfg(test)]
mod tests {
    use super::{split_assignment, DirVars};

    fn from_text(text: &str) -> DirVars {
        let resolved = super::SEEDS
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect::<std::collections::HashMap<_, _>>();
        let mut d = DirVars {
            resolved: resolved.clone(),
            source_proven_resolved: resolved,
            unproven_variables: std::collections::BTreeSet::new(),
            sticky_unproven_variables: std::collections::BTreeSet::new(),
            source_evaluation_unproven: false,
            source_dependencies: std::collections::HashMap::new(),
            source_dependency_items: 0,
            source_dependency_bytes: 0,
            source_dependency_work: 0,
            source_dependency_scan_bytes: 0,
            source_dependency_tracking_unproven: false,
            materialized: std::collections::HashMap::new(),
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
    fn active_dynamic_make_evaluation_invalidates_any_strict_query_only() {
        let d = from_text("UNRELATED_ROOT := proven\n$(eval AROS_INCLUDES := changed)\n");
        assert_eq!(d.expand("$(UNRELATED_ROOT)").as_deref(), Some("proven"));
        assert!(d.expand_source_proven("$(UNRELATED_ROOT)").is_none());
    }

    #[test]
    fn unsupported_triple_colon_assignment_preserves_legacy_expansion_but_not_source_proof() {
        let d = from_text("LEGACY_VALUE :::= $(TARGETDIR)/legacy\n");
        assert_eq!(
            d.expand("$(LEGACY_VALUE)").as_deref(),
            Some("${AROS_BUILD_DIR}/legacy")
        );
        assert!(d.expand_source_proven("$(LEGACY_VALUE)").is_none());

        let placeholder = from_text("TARGETDIR :::= @TARGETDIR@\n");
        assert_eq!(placeholder.unresolved.len(), 1);
        assert_eq!(
            placeholder.expand("$(TARGETDIR)").as_deref(),
            Some("${AROS_BUILD_DIR}")
        );
        assert!(placeholder.expand_source_proven("$(TARGETDIR)").is_none());
    }

    #[test]
    fn source_dependency_closure_honors_small_explicit_resource_budgets() {
        let d = from_text("LEAF := include\nROOT := $(LEAF)\n");
        assert!(d.source_dependency_closure(&["ROOT"]).is_some());
        assert!(d
            .source_dependency_closure_with_limits(&["ROOT"], 8, 128, 0)
            .is_none());
        assert!(d
            .source_dependency_closure_with_limits(&["ROOT"], 8, 1, 128)
            .is_none());
    }

    #[test]
    fn a_seed_is_not_overwritten_by_the_file() {
        let d = from_text("AROS_DIR_AROS := AROS\n");
        assert_eq!(d.expand("$(AROS_DIR_AROS)").unwrap(), "SYS");
    }

    #[test]
    fn native_target_tool_roles_require_explicit_admission() {
        let mut dirs = from_text("");
        for name in [
            "NATIVE_TARGET_CC",
            "NATIVE_TARGET_AR",
            "NATIVE_TARGET_RANLIB",
        ] {
            assert!(dirs.expand(&format!("$({name})")).is_none());
        }
        dirs.bind_native_target_tool_roles();
        assert_eq!(
            dirs.expand("$(NATIVE_TARGET_CC)").as_deref(),
            Some("${CMAKE_C_COMPILER}")
        );
        assert_eq!(
            dirs.expand("$(NATIVE_TARGET_AR)").as_deref(),
            Some("${CMAKE_AR}")
        );
        assert_eq!(
            dirs.expand("$(NATIVE_TARGET_RANLIB)").as_deref(),
            Some("${CMAKE_RANLIB}")
        );
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
    fn bounded_expansion_preserves_local_shadowing_through_a_normal_chain() {
        let d = from_text("BASE := global\nLOCAL_CHAIN := $(BASE)/shared\n");
        let local = |name: &str| match name {
            "BASE" => Some("local".to_owned()),
            _ => None,
        };
        assert_eq!(
            d.expand_with("$(LOCAL_CHAIN)/leaf", &local).unwrap(),
            "local/shared/leaf"
        );
    }

    #[test]
    fn recursive_fanout_and_large_leaf_fail_closed_at_the_expansion_budgets() {
        let mut d = from_text("");
        d.resolved
            .insert("FANOUT_9".to_owned(), "x".repeat(64 * 1024));
        for index in (0..9).rev() {
            let next = index + 1;
            d.resolved.insert(
                format!("FANOUT_{index}"),
                format!("$(FANOUT_{next})$(FANOUT_{next})"),
            );
        }
        assert!(d.expand("$(FANOUT_0)").is_none());

        d.resolved.insert(
            "LARGE_LEAF".to_owned(),
            "x".repeat(super::MAX_EXPANSION_VALUE_BYTES + 1),
        );
        assert!(d.expand("$(LARGE_LEAF)").is_none());
    }

    #[test]
    fn missing_variable_diagnostics_bound_fanout_and_return_a_refusal() {
        let d = from_text("");
        let aliases = (0..=super::MAX_EXPANSION_LIST_ITEMS)
            .map(|index| format!("$(MISSING_{index})"))
            .collect::<Vec<_>>()
            .join(" ");
        let local = |name: &str| (name == "ROOT").then(|| aliases.clone());
        let error = d.expand_with("$(ROOT)", &local).unwrap_err();
        assert_eq!(error, vec![super::EXPANSION_RESOURCE_ERROR.to_owned()]);
        assert_eq!(
            d.missing_in(&aliases),
            vec![super::EXPANSION_RESOURCE_ERROR.to_owned()]
        );
    }

    #[test]
    fn missing_diagnostics_reject_an_oversized_global_before_copying_it() {
        let mut d = from_text("");
        d.resolved.insert(
            "LARGE".to_owned(),
            "x".repeat(super::MAX_EXPANSION_VALUE_BYTES + 1),
        );
        let mut budget = super::ExpansionBudget::default();
        let raw = "$(LARGE)/$(MISSING)";
        assert!(d.missing_in_with(raw, &|_| None, &mut budget).is_none());
        // Only the initial diagnostic input may have been retained. The
        // rejected global must not consume output budget or be cloned.
        assert_eq!(budget.output_bytes, raw.len());
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
