//! Declaration-scoped, source-only inputs consumed by native KOBJ link rules.
//!
//! The scanner expands active `make.opts` includes at their source positions,
//! then asks the existing Make variable and expression layers for each value
//! at the declaration line. It never reads the process environment, executes
//! Make functions with side effects, or converts an unresolved input to an
//! empty list.

use crate::dirs::DirVars;
use crate::make_expr::{evaluate_make_list, MakeExprContext};
use crate::make_opts::MakeOptsFile;
use crate::make_vars::{
    collect_vars_with_context, strip_make_comment, undefine_directive, variable_assignment,
    AssignmentKind, ConditionalTruth, VarScope,
};
use crate::parser::TargetContext;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_INCLUDE_DEPTH: usize = 8;
const MAX_INCLUDE_FILES: usize = 64;
const MAX_INCLUDE_BYTES: usize = 1024 * 1024;

/// The provenance of a source assignment contributing to a KOBJ input.
///
/// `line` is one-based in the continuation-joined source view. Repeated
/// includes intentionally produce repeated references.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KobjSourceRef {
    pub path: String,
    pub line: usize,
}

/// A KOBJ Make list at one declaration, retaining uncertainty explicitly.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ScopedMakeWords {
    KnownEmpty {
        raw: Option<String>,
        source: Vec<KobjSourceRef>,
    },
    Exact {
        raw: String,
        words: Vec<String>,
        source: Vec<KobjSourceRef>,
    },
    Unresolved {
        raw: Option<String>,
        reason: String,
        source: Vec<KobjSourceRef>,
    },
}

/// Lossless source inputs captured for one full-module KOBJ declaration.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KobjScopedInputs {
    /// The template's `DEFNAME`: `modname` or `modname_flavour`.
    pub defname: String,
    /// Zero-based line in the caller's joined source, matching `Invocation.line`.
    pub declaration_line: usize,
    /// Zero-based line after ordered `make.opts` contents were inserted.
    pub scoped_declaration_line: usize,
    /// Option files actually read, including repeated includes in source order.
    pub included_make_opts: Vec<MakeOptsFile>,
    /// Native configuration fragments substituted for explicit source includes.
    pub included_configuration_files: Vec<MakeOptsFile>,
    pub user_objects: ScopedMakeWords,
    pub defname_libs: ScopedMakeWords,
    pub user_ldflags: ScopedMakeWords,
    /// The source `%build_module` macro's `uselibs` list; absent defaults empty.
    pub use_libs: ScopedMakeWords,
    pub kobj_ldflags: ScopedMakeWords,
    pub kernel_kobj_ldscript: ScopedMakeWords,
    pub funcinstr_libs: ScopedMakeWords,
    /// Effective `funcinstr` selector (`yes`/`no` by source contract).
    pub function_instrumentation: ScopedMakeWords,
}

/// Evaluated module identity and the raw optional `uselibs` macro argument.
#[derive(Clone, Copy, Debug)]
pub struct KobjModuleArgs<'a> {
    pub module_name: &'a str,
    pub flavour: Option<&'a str>,
    /// Raw Make expression from the macro, or `None` when the argument is absent.
    pub raw_uselibs: Option<&'a str>,
    /// Raw Make expression from the macro, or `None` to use `$(TARGET_FUNCINSTR)`.
    pub raw_funcinstr: Option<&'a str>,
}

/// Captures `USER_OBJS`, `<DEFNAME>_LIBS`, and ordered `USER_LDFLAGS` at one
/// declaration in a source-only Make scope.
///
/// `joined_source` must be the same continuation-joined view used to obtain
/// `declaration_line` (it may already have local source fragments inlined).
/// `source_relative_path` names that declaring file below `source_root`.
/// `module_name` and `flavour` are the values evaluated at this declaration by
/// the caller; this helper applies the full-module template's DEFNAME rule.
///
/// Reads of `make.opts` are source-root confined, preserve include order and
/// duplicates, and are bounded to eight nested levels, 64 file reads and one
/// MiB total. A proven absent optional include is Make-empty. A missing
/// required include, unresolved include expression, rejected read, or limit
/// failure makes all three inputs uncertain until an unconditional `=` or
/// `:=` replacement of the corresponding variable.
#[must_use]
#[expect(
    clippy::too_many_arguments,
    reason = "the positional API mirrors one invocation and its source/target context"
)]
pub fn capture_kobj_scoped_inputs(
    joined_source: &str,
    declaration_line: usize,
    module_name: &str,
    flavour: Option<&str>,
    source_root: &Path,
    source_relative_path: &Path,
    dirs: &DirVars,
    target: &TargetContext,
) -> KobjScopedInputs {
    capture_kobj_scoped_inputs_with_known_source_includes(
        joined_source,
        declaration_line,
        module_name,
        flavour,
        source_root,
        source_relative_path,
        dirs,
        target,
        &[],
    )
}

/// As [`capture_kobj_scoped_inputs`], with explicit proof for non-`make.opts`
/// includes that the caller has already inlined or represented in the source
/// contract.
///
/// Entries are the trimmed include argument as it appears in the joined
/// source. Any other surviving non-`make.opts` include is unresolved. Mixed
/// `make.opts` and non-`make.opts` include arguments are always rejected because
/// their exact merge order is not represented by this input.
#[must_use]
#[expect(
    clippy::too_many_arguments,
    reason = "the public source-context API is kept stable for parser integration"
)]
pub fn capture_kobj_scoped_inputs_with_known_source_includes(
    joined_source: &str,
    declaration_line: usize,
    module_name: &str,
    flavour: Option<&str>,
    source_root: &Path,
    source_relative_path: &Path,
    dirs: &DirVars,
    target: &TargetContext,
    known_non_makeopts_include_patterns: &[String],
) -> KobjScopedInputs {
    capture_kobj_scoped_inputs_for_module(
        joined_source,
        declaration_line,
        KobjModuleArgs {
            module_name,
            flavour,
            raw_uselibs: None,
            raw_funcinstr: None,
        },
        source_root,
        source_relative_path,
        dirs,
        target,
        known_non_makeopts_include_patterns,
    )
}

/// Captures KOBJ inputs with source macro metadata and explicit source include
/// bindings.
///
/// `raw_uselibs: None` is the template's default-empty value. A provided
/// expression is evaluated independently, so unrelated unresolved inputs do
/// not poison a literal `uselibs` list.
#[must_use]
#[expect(
    clippy::too_many_arguments,
    reason = "keeps the existing source scope inputs positional while grouping macro arguments"
)]
pub fn capture_kobj_scoped_inputs_for_module(
    joined_source: &str,
    declaration_line: usize,
    module_args: KobjModuleArgs<'_>,
    source_root: &Path,
    source_relative_path: &Path,
    dirs: &DirVars,
    target: &TargetContext,
    known_non_makeopts_include_patterns: &[String],
) -> KobjScopedInputs {
    let KobjModuleArgs {
        module_name,
        flavour,
        raw_uselibs,
        raw_funcinstr,
    } = module_args;
    let defname = flavour
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map_or_else(
            || module_name.trim().to_owned(),
            |flavour| format!("{}_{}", module_name.trim(), flavour),
        );

    let mut builder = ScopedSourceBuilder::new(
        source_root,
        source_relative_path,
        dirs,
        target,
        known_non_makeopts_include_patterns,
    );
    let main_lines = joined_source.lines().map(str::to_owned).collect::<Vec<_>>();
    let line_exists = declaration_line < main_lines.len();
    if line_exists {
        builder.expand_main(&main_lines, declaration_line);
    }

    let Some(scoped_declaration_line) = builder.declaration_line else {
        let reason = if line_exists {
            "the declaration line could not be mapped into the scoped Make view".to_owned()
        } else {
            format!("declaration line {declaration_line} is outside the joined source")
        };
        let unresolved = || ScopedMakeWords::Unresolved {
            raw: None,
            reason: reason.clone(),
            source: Vec::new(),
        };
        return KobjScopedInputs {
            defname,
            declaration_line,
            scoped_declaration_line: 0,
            included_make_opts: builder.included_make_opts,
            included_configuration_files: builder.included_configuration_files,
            user_objects: unresolved(),
            defname_libs: unresolved(),
            user_ldflags: unresolved(),
            use_libs: raw_uselibs.map_or_else(
                || ScopedMakeWords::KnownEmpty {
                    raw: None,
                    source: Vec::new(),
                },
                |raw| ScopedMakeWords::Unresolved {
                    raw: Some(raw.to_owned()),
                    reason: reason.clone(),
                    source: Vec::new(),
                },
            ),
            kobj_ldflags: unresolved(),
            kernel_kobj_ldscript: unresolved(),
            funcinstr_libs: unresolved(),
            function_instrumentation: ScopedMakeWords::Unresolved {
                raw: Some(raw_funcinstr.unwrap_or("$(TARGET_FUNCINSTR)").to_owned()),
                reason: reason.clone(),
                source: Vec::new(),
            },
        };
    };

    let target_libs = format!("{defname}_LIBS");
    let original_scope = collect_vars_with_context(&builder.view.text, target);
    let (_, line_states) = crate::make_vars::collect_vars_impl(&builder.view.text, Some(target));
    let uncertainty = ScopeUncertainty::new(
        &builder.view,
        &line_states,
        scoped_declaration_line,
        &original_scope,
    );
    let clean_text = uncertainty.clear_reset_assignments(&builder.view);
    let scope = collect_vars_with_context(&clean_text, target);

    let capture = ScopedValueCapture {
        original_scope: &original_scope,
        scope: &scope,
        line: scoped_declaration_line,
        view: &builder.view,
        uncertainty: &uncertainty,
        dirs,
        target,
        source_root,
        relative_dir: &builder.relative_dir,
    };
    let user_objects = capture.capture("USER_OBJS");
    let defname_libs = if defname.is_empty() {
        ScopedMakeWords::Unresolved {
            raw: capture
                .original_scope
                .raw_at(&target_libs, scoped_declaration_line),
            reason: "the module name is empty, so DEFNAME_LIBS cannot be derived".to_owned(),
            source: capture.view.sources_for(
                &target_libs,
                scoped_declaration_line,
                &capture.uncertainty.line_states,
                capture.uncertainty,
            ),
        }
    } else {
        capture.capture(&target_libs)
    };
    let user_ldflags = capture.capture("USER_LDFLAGS");
    let declaration_source = builder.view.refs[scoped_declaration_line].clone();
    let use_libs = capture.capture_expression(raw_uselibs, vec![declaration_source.clone()]);
    let kobj_ldflags = capture.capture_required_global("KOBJ_LDFLAGS");
    let kernel_kobj_ldscript = capture.capture_required_global("KERNEL_KOBJ_LDSCRIPT");
    let funcinstr_libs = capture.capture_required_global("FUNCINSTR_LIBS");
    let function_instrumentation = raw_funcinstr.map_or_else(
        || capture.capture_funcinstr("$(TARGET_FUNCINSTR)", vec![declaration_source.clone()]),
        |raw| capture.capture_funcinstr(raw, vec![declaration_source.clone()]),
    );

    KobjScopedInputs {
        defname,
        declaration_line,
        scoped_declaration_line,
        included_make_opts: builder.included_make_opts,
        included_configuration_files: builder.included_configuration_files,
        user_objects,
        defname_libs,
        user_ldflags,
        use_libs,
        kobj_ldflags,
        kernel_kobj_ldscript,
        funcinstr_libs,
        function_instrumentation,
    }
}

struct ScopedValueCapture<'a> {
    original_scope: &'a VarScope,
    scope: &'a VarScope,
    line: usize,
    view: &'a ExpandedView,
    uncertainty: &'a ScopeUncertainty,
    dirs: &'a DirVars,
    target: &'a TargetContext,
    source_root: &'a Path,
    relative_dir: &'a Path,
}

impl ScopedValueCapture<'_> {
    fn capture(&self, name: &str) -> ScopedMakeWords {
        let raw = self.original_scope.raw_at(name, self.line);
        let source = self.view.sources_for(
            name,
            self.line,
            &self.uncertainty.line_states,
            self.uncertainty,
        );
        if let Some(reason) = self.reference_reason(name) {
            return ScopedMakeWords::Unresolved {
                raw,
                reason,
                source,
            };
        }
        self.capture_value(raw, source)
    }

    fn capture_required_global(&self, name: &str) -> ScopedMakeWords {
        let raw = self.original_scope.raw_at(name, self.line);
        let source = self.view.sources_for(
            name,
            self.line,
            &self.uncertainty.line_states,
            self.uncertainty,
        );
        if let Some(reason) = self.reference_reason(name) {
            return ScopedMakeWords::Unresolved {
                raw,
                reason,
                source,
            };
        }
        let effective_raw = raw.or_else(|| self.target.value_of(name));
        if effective_raw.is_none() {
            return ScopedMakeWords::Unresolved {
                raw: None,
                reason: format!("{name} has no source or target definition proving its value"),
                source,
            };
        }
        self.capture_value(effective_raw, source)
    }

    fn capture_expression(&self, raw: Option<&str>, source: Vec<KobjSourceRef>) -> ScopedMakeWords {
        self.capture_value(raw.map(str::to_owned), source)
    }

    fn capture_funcinstr(&self, raw: &str, source: Vec<KobjSourceRef>) -> ScopedMakeWords {
        if let Some(reason) = self.unbound_reference_reason(raw) {
            return ScopedMakeWords::Unresolved {
                raw: Some(raw.to_owned()),
                reason,
                source,
            };
        }
        self.capture_expression(Some(raw), source)
    }

    fn unbound_reference_reason(&self, raw: &str) -> Option<String> {
        let references = variable_references(raw);
        if references.opaque {
            return Some(
                "function instrumentation selector uses a Make expression outside the bounded reference subset"
                    .to_owned(),
            );
        }
        let mut visiting = HashSet::new();
        references
            .names
            .iter()
            .find_map(|name| self.selector_dependency_reason(name, &mut visiting, 16))
    }

    fn selector_dependency_reason(
        &self,
        name: &str,
        visiting: &mut HashSet<String>,
        depth: usize,
    ) -> Option<String> {
        if let Some(reason) = self.reference_reason(name) {
            return Some(reason);
        }
        if depth == 0 || !visiting.insert(name.to_owned()) {
            return Some(format!(
                "function instrumentation selector has a cyclic or over-deep reference through {name}"
            ));
        }

        let source_value = self.scope.raw_at(name, self.line);
        let target_value = self.target.value_of(name);
        if source_value.is_none()
            && target_value.is_none()
            && self.dirs.expand(&format!("$({name})")).is_none()
        {
            visiting.remove(name);
            return Some(format!(
                "function instrumentation selector references {name}, which is not defined in the source-only target scope"
            ));
        }

        let nested = source_value.or(target_value).map_or_else(
            || Some(Vec::new()),
            |value| {
                let references = variable_references(&value);
                if references.opaque {
                    return None;
                }
                Some(references.names.into_iter().collect())
            },
        );
        let reason = nested.map_or_else(
            || {
                Some(format!(
                "function instrumentation selector dependency {name} uses a Make expression outside the bounded reference subset"
                ))
            },
            |nested| {
                nested.into_iter().find_map(|dependency| {
                self.selector_dependency_reason(&dependency, visiting, depth - 1)
                })
            },
        );
        visiting.remove(name);
        reason
    }

    fn capture_value(&self, raw: Option<String>, source: Vec<KobjSourceRef>) -> ScopedMakeWords {
        let Some(raw_value) = raw.as_deref() else {
            return ScopedMakeWords::KnownEmpty { raw: None, source };
        };

        let lookup = |variable: &str| {
            self.scope
                .raw_at(variable, self.line)
                .or_else(|| self.target.value_of(variable))
                .or_else(|| self.dirs.expand(&format!("$({variable})")))
                .or_else(|| Some(String::new()))
        };
        let guard = |variable: &str| self.reference_reason(variable);
        let context = MakeExprContext::new(
            self.scope,
            self.dirs,
            self.line,
            self.source_root,
            self.relative_dir,
        )
        .with_lookup(&lookup)
        .with_guard(&guard);
        match evaluate_make_list(raw_value, &context) {
            Ok(words) if words.is_empty() => ScopedMakeWords::KnownEmpty { raw, source },
            Ok(words) => ScopedMakeWords::Exact {
                raw: raw_value.to_owned(),
                words,
                source,
            },
            Err(error) => ScopedMakeWords::Unresolved {
                raw,
                reason: format!("Make list expansion failed: {error}"),
                source,
            },
        }
    }

    fn reference_reason(&self, name: &str) -> Option<String> {
        self.uncertainty
            .reason(name, self.line)
            .or_else(|| {
                self.original_scope
                    .path_is_conditional_at(name, self.line)
                    .then(|| {
                        format!(
                            "{name} was simply expanded from a value that may depend on an unresolved Make conditional"
                        )
                    })
            })
            .or_else(|| {
                self.original_scope
                    .flavor_uncertainty_reason_at(name, self.line)
            })
    }
}

#[derive(Default)]
struct ExpandedView {
    text: String,
    refs: Vec<KobjSourceRef>,
    issues: Vec<IncludeIssue>,
}

impl ExpandedView {
    fn push_line(&mut self, line: &str, source: KobjSourceRef) -> usize {
        let index = self.refs.len();
        self.text.push_str(line);
        self.text.push('\n');
        self.refs.push(source);
        index
    }

    fn sources_for(
        &self,
        name: &str,
        line: usize,
        line_states: &[ConditionalTruth],
        uncertainty: &ScopeUncertainty,
    ) -> Vec<KobjSourceRef> {
        let start = uncertainty.last_reset_before(name, line).unwrap_or(0);
        let mut sources = Vec::new();
        for (index, raw_line) in self.text.lines().enumerate().take(line) {
            if index < start || !branch_may_be_active(line_states.get(index)) {
                continue;
            }
            if assignment_name(raw_line).as_deref() == Some(name) {
                sources.push((index, self.refs[index].clone()));
            }
        }
        for issue in &self.issues {
            if issue.line >= line || issue.line < start {
                continue;
            }
            if branch_may_be_active(line_states.get(issue.line)) {
                sources.push((issue.line, issue.source.clone()));
            }
        }
        sources.sort_by_key(|(index, _)| *index);
        sources.into_iter().map(|(_, source)| source).collect()
    }
}

#[derive(Clone)]
struct IncludeIssue {
    line: usize,
    source: KobjSourceRef,
    reason: String,
}

#[derive(Clone)]
struct AssignmentEvent {
    line: usize,
    name: String,
    branch: ConditionalTruth,
    kind: AssignmentKind,
    value: String,
}

#[derive(Clone)]
struct FrozenIncludeTaint {
    line: usize,
    name: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AssignmentFlavor {
    Simple,
    Recursive,
}

#[derive(Default)]
struct VariableReferences {
    names: HashSet<String>,
    opaque: bool,
}

struct ScopeUncertainty {
    events: Vec<AssignmentEvent>,
    include_issues: Vec<IncludeIssue>,
    line_states: Vec<ConditionalTruth>,
    reset_history: std::collections::HashMap<String, Vec<usize>>,
    frozen_include_taint: Vec<FrozenIncludeTaint>,
    declaration_line: usize,
}

impl ScopeUncertainty {
    fn new(
        view: &ExpandedView,
        line_states: &[ConditionalTruth],
        declaration_line: usize,
        scope: &VarScope,
    ) -> Self {
        let mut events = Vec::new();
        let mut reset_history = std::collections::HashMap::new();
        for (line, raw_line) in view.text.lines().enumerate().take(declaration_line) {
            let branch = line_states
                .get(line)
                .copied()
                .unwrap_or(ConditionalTruth::True);
            let parsed_assignment = variable_assignment(strip_make_comment(raw_line));
            let parsed_undefine = undefine_directive(raw_line)
                .ok()
                .flatten()
                .map(|name| (name, "", AssignmentKind::Undefine));
            let Some((name, value, kind)) = parsed_assignment.or(parsed_undefine) else {
                continue;
            };
            events.push(AssignmentEvent {
                line,
                name: name.to_owned(),
                branch,
                kind,
                value: value.to_owned(),
            });
            if branch == ConditionalTruth::True
                && matches!(
                    kind,
                    AssignmentKind::SimpleSet
                        | AssignmentKind::RecursiveSet
                        | AssignmentKind::Undefine
                )
            {
                reset_history
                    .entry(name.to_owned())
                    .or_insert_with(Vec::new)
                    .push(line);
            }
        }
        let mut uncertainty = Self {
            events,
            include_issues: view.issues.clone(),
            line_states: line_states.to_vec(),
            reset_history,
            frozen_include_taint: Vec::new(),
            declaration_line,
        };
        uncertainty.track_frozen_include_taint(scope);
        uncertainty
    }

    fn reason(&self, name: &str, line: usize) -> Option<String> {
        let reset = self.last_reset_before(name, line);
        for event in &self.events {
            if event.name == name
                && event.line < line
                && event.branch == ConditionalTruth::Unknown
                && reset.is_none_or(|reset| event.line > reset)
            {
                return Some(format!(
                    "{name} may be assigned or undefined in a Make conditional whose truth is unresolved"
                ));
            }
        }
        for issue in &self.include_issues {
            if issue.line >= line
                || reset.is_some_and(|reset| issue.line < reset)
                || !branch_may_be_active(self.line_states.get(issue.line))
            {
                continue;
            }
            return Some(format!(
                "source Make input at {}:{} is unresolved: {}",
                issue.source.path, issue.source.line, issue.reason
            ));
        }
        for taint in &self.frozen_include_taint {
            if taint.name == name
                && taint.line < line
                && reset.is_none_or(|reset| taint.line >= reset)
            {
                return Some(format!(
                    "{name} freezes a value that may depend on an unresolved source Make input"
                ));
            }
        }
        None
    }

    fn last_reset_before(&self, name: &str, line: usize) -> Option<usize> {
        self.reset_history
            .get(name)
            .and_then(|history| history.iter().rev().find(|at| **at < line).copied())
    }

    fn include_taints_variable_at(&self, name: &str, line: usize) -> bool {
        let reset = self.last_reset_before(name, line);
        self.include_issues.iter().any(|issue| {
            issue.line < line
                && reset.is_none_or(|reset| issue.line >= reset)
                && branch_may_be_active(self.line_states.get(issue.line))
        }) || self.frozen_include_taint.iter().any(|taint| {
            taint.name == name && taint.line < line && reset.is_none_or(|reset| taint.line >= reset)
        })
    }

    fn has_active_include_before(&self, line: usize) -> bool {
        self.include_issues.iter().any(|issue| {
            issue.line < line && branch_may_be_active(self.line_states.get(issue.line))
        })
    }

    fn expression_include_tainted(
        &self,
        name: &str,
        line: usize,
        scope: &VarScope,
        visiting: &mut HashSet<String>,
        depth: usize,
    ) -> bool {
        if self.include_taints_variable_at(name, line) {
            return true;
        }
        if depth == 0 || !visiting.insert(name.to_owned()) {
            return self.has_active_include_before(line);
        }
        let tainted = scope.raw_at(name, line).is_some_and(|raw| {
            let references = variable_references(&raw);
            references.opaque && self.has_active_include_before(line)
                || references.names.iter().any(|dependency| {
                    self.expression_include_tainted(dependency, line, scope, visiting, depth - 1)
                })
        });
        visiting.remove(name);
        tainted
    }

    fn track_frozen_include_taint(&mut self, scope: &VarScope) {
        let mut flavors = std::collections::HashMap::new();
        for event in self.events.clone() {
            if event.branch != ConditionalTruth::True {
                continue;
            }
            let prior_flavor = flavors
                .get(&event.name)
                .copied()
                .unwrap_or(AssignmentFlavor::Recursive);
            let expands_now = match event.kind {
                AssignmentKind::SimpleSet => true,
                AssignmentKind::Append => prior_flavor == AssignmentFlavor::Simple,
                AssignmentKind::RecursiveSet
                | AssignmentKind::SetIfUnset
                | AssignmentKind::Undefine => false,
            };
            if expands_now {
                let references = variable_references(&event.value);
                let dependent_taint = references.names.iter().any(|name| {
                    self.expression_include_tainted(
                        name,
                        event.line,
                        scope,
                        &mut HashSet::new(),
                        16,
                    )
                });
                if dependent_taint
                    || (references.opaque && self.has_active_include_before(event.line))
                {
                    self.frozen_include_taint.push(FrozenIncludeTaint {
                        line: event.line,
                        name: event.name.clone(),
                    });
                }
            }
            match event.kind {
                AssignmentKind::SimpleSet => {
                    flavors.insert(event.name, AssignmentFlavor::Simple);
                }
                AssignmentKind::RecursiveSet | AssignmentKind::Undefine => {
                    flavors.insert(event.name, AssignmentFlavor::Recursive);
                }
                AssignmentKind::SetIfUnset => {
                    flavors
                        .entry(event.name)
                        .or_insert(AssignmentFlavor::Recursive);
                }
                AssignmentKind::Append => {}
            }
        }
    }

    fn clear_reset_assignments(&self, view: &ExpandedView) -> String {
        let mut output = String::with_capacity(view.text.len());
        for (line, raw_line) in view.text.lines().enumerate() {
            let remove = line < self.declaration_line
                && self.line_states.get(line) == Some(&ConditionalTruth::Unknown)
                && assignment_name(raw_line).is_some_and(|name| {
                    self.last_reset_before(&name, self.declaration_line)
                        .is_some_and(|reset| line < reset)
                });
            if !remove {
                output.push_str(raw_line);
            }
            output.push('\n');
        }
        output
    }
}

const fn branch_may_be_active(branch: Option<&ConditionalTruth>) -> bool {
    !matches!(branch, Some(ConditionalTruth::False))
}

fn assignment_name(line: &str) -> Option<String> {
    let uncommented = strip_make_comment(line);
    variable_assignment(uncommented)
        .map(|(name, _, _)| name.to_owned())
        .or_else(|| {
            undefine_directive(uncommented)
                .ok()
                .flatten()
                .map(str::to_owned)
        })
}

fn variable_references(raw: &str) -> VariableReferences {
    let mut references = VariableReferences::default();
    collect_variable_references(raw, &mut references, 16);
    references
}

fn collect_variable_references(raw: &str, references: &mut VariableReferences, depth: usize) {
    if depth == 0 {
        references.opaque = true;
        return;
    }
    let mut cursor = 0;
    while cursor < raw.len() {
        let Some(relative) = raw[cursor..].find('$') else {
            return;
        };
        let dollar = cursor + relative;
        if raw.as_bytes().get(dollar + 1) == Some(&b'$') {
            cursor = dollar + 2;
            continue;
        }
        let Some((_, end)) = reference_end(raw, dollar) else {
            cursor = dollar + 1;
            continue;
        };
        let body = raw[dollar + 2..end].trim();
        if is_make_name(body) {
            references.names.insert(body.to_owned());
        } else {
            let substitution = body
                .split_once(':')
                .filter(|(name, _)| is_make_name(name.trim()));
            if let Some((name, suffix)) = substitution {
                references.names.insert(name.trim().to_owned());
                collect_variable_references(suffix, references, depth - 1);
            } else {
                let function = body
                    .split(|character: char| character.is_whitespace() || character == ',')
                    .next()
                    .unwrap_or_default();
                if !is_supported_make_function(function)
                    || matches!(function, "call" | "foreach" | "value")
                {
                    references.opaque = true;
                }
                collect_variable_references(body, references, depth - 1);
            }
        }
        cursor = end + 1;
    }
}

fn is_supported_make_function(name: &str) -> bool {
    matches!(
        name,
        "addprefix"
            | "addsuffix"
            | "basename"
            | "call"
            | "dir"
            | "filter"
            | "filter-out"
            | "findstring"
            | "firstword"
            | "foreach"
            | "if"
            | "join"
            | "lastword"
            | "notdir"
            | "or"
            | "and"
            | "patsubst"
            | "sort"
            | "strip"
            | "subst"
            | "suffix"
            | "value"
            | "wildcard"
            | "word"
            | "wordlist"
            | "words"
    )
}

/// Do not turn an incomplete wildcard traversal into a successful subset.
/// The caller takes one match beyond its remaining budget to detect overflow.
fn collect_bounded_include_matches<E>(
    paths: impl Iterator<Item = std::result::Result<PathBuf, E>>,
    limit: usize,
) -> std::result::Result<Vec<PathBuf>, E> {
    paths.take(limit).collect()
}

struct ScopedSourceBuilder<'a> {
    root: PathBuf,
    source_relative_path: PathBuf,
    relative_dir: PathBuf,
    dirs: &'a DirVars,
    target: &'a TargetContext,
    known_non_makeopts_include_patterns: HashSet<String>,
    view: ExpandedView,
    declaration_line: Option<usize>,
    included_make_opts: Vec<MakeOptsFile>,
    included_configuration_files: Vec<MakeOptsFile>,
    active: Vec<PathBuf>,
    files_read: usize,
    bytes_read: usize,
}

#[derive(Clone, Copy)]
enum IncludedFileKind {
    MakeOpts,
    Configuration,
}

impl<'a> ScopedSourceBuilder<'a> {
    fn new(
        source_root: &Path,
        source_relative_path: &Path,
        dirs: &'a DirVars,
        target: &'a TargetContext,
        known_non_makeopts_include_patterns: &[String],
    ) -> Self {
        let root = fs::canonicalize(source_root).unwrap_or_else(|_| source_root.to_path_buf());
        let relative_dir = source_relative_path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        Self {
            root,
            source_relative_path: source_relative_path.to_path_buf(),
            relative_dir,
            dirs,
            target,
            known_non_makeopts_include_patterns: known_non_makeopts_include_patterns
                .iter()
                .map(|pattern| pattern.trim().to_owned())
                .collect(),
            view: ExpandedView::default(),
            declaration_line: None,
            included_make_opts: Vec::new(),
            included_configuration_files: Vec::new(),
            active: Vec::new(),
            files_read: 0,
            bytes_read: 0,
        }
    }

    fn expand_main(&mut self, lines: &[String], declaration_line: usize) {
        for (line_number, line) in lines.iter().enumerate().take(declaration_line + 1) {
            let source = KobjSourceRef {
                path: display_path(&self.source_relative_path),
                line: line_number + 1,
            };
            let view_line = self.view.push_line(line, source.clone());
            if line_number == declaration_line {
                self.declaration_line = Some(view_line);
            }
            self.expand_include_if_needed(line, source, view_line, 0);
        }
    }

    fn expand_include_if_needed(
        &mut self,
        line: &str,
        source: KobjSourceRef,
        view_line: usize,
        depth: usize,
    ) {
        if undefine_directive(line).is_err() {
            if self.branch_state(view_line) != ConditionalTruth::False {
                self.issue(
                    view_line,
                    source,
                    "unsupported or dynamic undefine syntax is unresolved",
                );
            }
            return;
        }
        let Some((optional, raw_path)) = include_directive(line) else {
            return;
        };
        let raw_path = raw_path.trim();
        if self.branch_state(view_line) == ConditionalTruth::False {
            return;
        }
        if !raw_path.to_ascii_lowercase().contains("make.opts")
            && self.known_non_makeopts_include_patterns.contains(raw_path)
        {
            return;
        }
        let Some(paths) = self.expand_include_paths(raw_path, view_line, &source) else {
            return;
        };
        if paths.is_empty() {
            if raw_path.to_ascii_lowercase().contains("make.opts") && !optional {
                self.issue(
                    view_line,
                    source,
                    "required make.opts include matched no file",
                );
            }
            return;
        }
        let has_make_opts = paths.iter().any(|path| path.ends_with("make.opts"));
        let has_other_include = paths.iter().any(|path| !path.ends_with("make.opts"));
        if has_other_include && has_make_opts {
            self.issue(
                view_line,
                source,
                "mixed make.opts and configuration include order is unresolved",
            );
            return;
        }
        if has_other_include {
            if paths.len() != 1 {
                self.issue(
                    view_line,
                    source,
                    "configuration include must resolve to exactly one source path",
                );
                return;
            }
            self.read_bound_configuration_include(
                &paths[0],
                optional,
                source,
                view_line,
                depth + 1,
            );
            return;
        }
        for expanded in paths {
            self.read_include(&expanded, optional, source.clone(), view_line, depth + 1);
        }
    }

    fn expand_include_paths(
        &mut self,
        raw_path: &str,
        line: usize,
        source: &KobjSourceRef,
    ) -> Option<Vec<String>> {
        let scope = collect_vars_with_context(&self.view.text, self.target);
        let include_context = IncludeExpressionContext {
            scope: &scope,
            line,
            target: self.target,
            root: &self.root,
            relative_dir: &self.relative_dir,
        };
        let mut flavor_uncertainty = None;
        let rewritten =
            include_context.rewrite(raw_path, &mut HashSet::new(), 24, &mut flavor_uncertainty);
        if let Some(reason) = flavor_uncertainty {
            self.issue(
                line,
                source.clone(),
                format!("include path depends on an unproven Make variable flavor: {reason}"),
            );
            return None;
        }
        if contains_make_function(&rewritten) {
            self.issue(
                line,
                source.clone(),
                "Make functions are outside the bounded make.opts path subset",
            );
            return None;
        }
        if contains_make_reference(&rewritten) {
            self.issue(
                line,
                source.clone(),
                "include path contains a variable that the source-only scope cannot resolve",
            );
            return None;
        }
        let context = MakeExprContext::new(&scope, self.dirs, line, &self.root, &self.relative_dir);
        match evaluate_make_list(&rewritten, &context) {
            Ok(paths)
                if paths.len() <= MAX_INCLUDE_FILES
                    && paths.iter().map(String::len).sum::<usize>() <= MAX_INCLUDE_BYTES =>
            {
                Some(paths)
            }
            Ok(_) => {
                self.issue(
                    line,
                    source.clone(),
                    "make.opts path expansion exceeds the bounded path count or size",
                );
                None
            }
            Err(error) => {
                self.issue(
                    line,
                    source.clone(),
                    format!("include expression could not be resolved: {error}"),
                );
                None
            }
        }
    }

    fn read_include(
        &mut self,
        word: &str,
        optional: bool,
        include_source: KobjSourceRef,
        include_line: usize,
        depth: usize,
    ) {
        let path = PathBuf::from(word);
        let path = if path.is_absolute() {
            path
        } else {
            self.root.join(path)
        };
        let matches = match glob::glob(&path.to_string_lossy()) {
            Ok(paths) => {
                let remaining_files = MAX_INCLUDE_FILES.saturating_sub(self.files_read);
                let mut matches =
                    match collect_bounded_include_matches(paths, remaining_files.saturating_add(1))
                    {
                        Ok(matches) => matches,
                        Err(error) => {
                            self.issue(
                                include_line,
                                include_source,
                                format!("make.opts include traversal is incomplete: {error}"),
                            );
                            return;
                        }
                    };
                matches.sort();
                matches
            }
            Err(error) => {
                self.issue(
                    include_line,
                    include_source,
                    format!("make.opts include pattern is invalid: {error}"),
                );
                return;
            }
        };
        let remaining_files = MAX_INCLUDE_FILES.saturating_sub(self.files_read);
        let too_many_matches = matches.len() > remaining_files;
        let mut matches = matches;
        matches.truncate(remaining_files);
        if matches.is_empty() && !too_many_matches {
            if !optional {
                self.issue(
                    include_line,
                    include_source,
                    format!("required make.opts include does not exist: {word}"),
                );
            }
            return;
        }
        for path in matches {
            self.read_include_file(
                &path,
                include_source.clone(),
                include_line,
                depth,
                IncludedFileKind::MakeOpts,
            );
        }
        if too_many_matches {
            self.issue(
                include_line,
                include_source,
                format!("make.opts include matches exceed {MAX_INCLUDE_FILES}"),
            );
        }
    }

    fn read_bound_configuration_include(
        &mut self,
        original_word: &str,
        optional: bool,
        include_source: KobjSourceRef,
        include_line: usize,
        depth: usize,
    ) {
        let relative = match self.safe_source_relative_path(original_word) {
            Ok(relative) => relative,
            Err(reason) => {
                self.issue(include_line, include_source, reason);
                return;
            }
        };
        let original_key = display_path(&relative);
        let Some(replacement_word) = self.target.make_include_bindings.get(&original_key) else {
            self.issue(
                include_line,
                include_source,
                "non-make.opts include is not bound by the native source contract",
            );
            return;
        };
        let original_path = match self.validate_regular_source_file(original_word) {
            Ok(path) => path,
            Err(reason) => {
                let required = if optional { "optional" } else { "required" };
                self.issue(
                    include_line,
                    include_source,
                    format!("bound {required} original configuration include is invalid: {reason}"),
                );
                return;
            }
        };
        let Ok(original_canonical) = fs::canonicalize(&original_path) else {
            self.issue(
                include_line,
                include_source,
                "bound original configuration include cannot be canonicalized",
            );
            return;
        };
        let Ok(canonical_original_relative) = original_canonical.strip_prefix(&self.root) else {
            self.issue(
                include_line,
                include_source,
                "bound original configuration include leaves the source root",
            );
            return;
        };
        if display_path(canonical_original_relative) != original_key {
            self.issue(
                include_line,
                include_source,
                "bound original configuration include is not canonical",
            );
            return;
        }
        if replacement_word.strip_suffix(".mk").is_none() {
            self.issue(
                include_line,
                include_source,
                "configuration replacement must name a source .mk file",
            );
            return;
        }
        let replacement_path = match self.validate_regular_source_file(replacement_word) {
            Ok(path) => path,
            Err(reason) => {
                self.issue(
                    include_line,
                    include_source,
                    format!("configuration replacement is invalid: {reason}"),
                );
                return;
            }
        };
        self.read_include_file(
            &replacement_path,
            include_source,
            include_line,
            depth,
            IncludedFileKind::Configuration,
        );
    }

    fn safe_source_relative_path(&self, word: &str) -> Result<PathBuf, String> {
        if word.is_empty() || word.contains('\\') || has_glob_metacharacters(word) {
            return Err("configuration include path is empty, escaped, or globbed".to_owned());
        }
        let path = Path::new(word);
        let relative = if path.is_absolute() {
            path.strip_prefix(&self.root)
                .map_err(|_| "configuration include leaves the source root".to_owned())?
        } else {
            path
        };
        if relative.as_os_str().is_empty()
            || relative
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return Err(
                "configuration include path is not a canonical source-relative path".to_owned(),
            );
        }
        Ok(relative.to_path_buf())
    }

    fn validate_regular_source_file(&self, word: &str) -> Result<PathBuf, String> {
        let relative = self.safe_source_relative_path(word)?;
        let mut candidate = self.root.clone();
        let component_count = relative.components().count();
        for (index, component) in relative.components().enumerate() {
            candidate.push(component.as_os_str());
            let metadata = fs::symlink_metadata(&candidate)
                .map_err(|error| format!("source path cannot be read: {error}"))?;
            if metadata.file_type().is_symlink() {
                return Err(format!(
                    "source path contains a symlink: {}",
                    candidate.display()
                ));
            }
            if index + 1 == component_count {
                if !metadata.is_file() {
                    return Err(format!(
                        "source path is not a regular file: {}",
                        candidate.display()
                    ));
                }
            } else if !metadata.is_dir() {
                return Err(format!(
                    "source path component is not a directory: {}",
                    candidate.display()
                ));
            }
        }
        let canonical = fs::canonicalize(&candidate)
            .map_err(|error| format!("source path cannot be canonicalized: {error}"))?;
        if !canonical.starts_with(&self.root) || canonical != candidate {
            return Err("source path is not canonical below the source root".to_owned());
        }
        Ok(canonical)
    }

    fn read_include_file(
        &mut self,
        path: &Path,
        include_source: KobjSourceRef,
        include_line: usize,
        depth: usize,
        kind: IncludedFileKind,
    ) {
        let canonical = match fs::canonicalize(path) {
            Ok(path) if path.starts_with(&self.root) => path,
            Ok(_) => {
                self.issue(
                    include_line,
                    include_source,
                    format!("included file leaves the source root: {}", path.display()),
                );
                return;
            }
            Err(error) => {
                self.issue(
                    include_line,
                    include_source,
                    format!("included file cannot be resolved: {error}"),
                );
                return;
            }
        };
        if depth > MAX_INCLUDE_DEPTH {
            self.issue(
                include_line,
                include_source,
                format!("include nesting exceeds {MAX_INCLUDE_DEPTH}"),
            );
            return;
        }
        if self.active.contains(&canonical) {
            self.issue(
                include_line,
                include_source,
                format!("include cycle reaches {}", canonical.display()),
            );
            return;
        }
        if self.files_read >= MAX_INCLUDE_FILES {
            self.issue(
                include_line,
                include_source,
                format!("include count exceeds {MAX_INCLUDE_FILES}"),
            );
            return;
        }
        self.files_read += 1;
        let remaining_bytes = MAX_INCLUDE_BYTES.saturating_sub(self.bytes_read);
        let file = match fs::File::open(&canonical) {
            Ok(file) => file,
            Err(error) => {
                self.issue(
                    include_line,
                    include_source,
                    format!("included file could not be read: {error}"),
                );
                return;
            }
        };
        let mut bytes = Vec::new();
        if let Err(error) = file
            .take(remaining_bytes.saturating_add(1) as u64)
            .read_to_end(&mut bytes)
        {
            self.issue(
                include_line,
                include_source,
                format!("included file could not be read: {error}"),
            );
            return;
        }
        if bytes.len() > remaining_bytes {
            self.issue(
                include_line,
                include_source,
                format!("included file bytes exceed {MAX_INCLUDE_BYTES}"),
            );
            return;
        }
        self.bytes_read += bytes.len();
        let text = match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(error) => {
                self.issue(
                    include_line,
                    include_source,
                    format!("included file is not UTF-8: {error}"),
                );
                return;
            }
        };
        let Ok(relative) = canonical.strip_prefix(&self.root) else {
            self.issue(
                include_line,
                include_source,
                "included file cannot be mapped below the source root",
            );
            return;
        };
        let relative_text = display_path(relative);
        let entry = MakeOptsFile {
            tag: None,
            path: relative_text.clone(),
        };
        match kind {
            IncludedFileKind::MakeOpts => self.included_make_opts.push(entry),
            IncludedFileKind::Configuration => self.included_configuration_files.push(entry),
        }
        let lines = continuation_joined_lines(&text);
        self.active.push(canonical);
        for (line_number, line) in lines {
            let source = KobjSourceRef {
                path: relative_text.clone(),
                line: line_number,
            };
            let view_line = self.view.push_line(&line, source.clone());
            self.expand_include_if_needed(&line, source, view_line, depth);
        }
        self.active.pop();
    }

    fn issue(&mut self, _line: usize, source: KobjSourceRef, reason: impl Into<String>) {
        let marker = self
            .view
            .push_line("# KOBJ scoped include unresolved", source.clone());
        self.view.issues.push(IncludeIssue {
            line: marker,
            source,
            reason: reason.into(),
        });
    }

    fn branch_state(&self, line: usize) -> ConditionalTruth {
        let (_, states) = crate::make_vars::collect_vars_impl(&self.view.text, Some(self.target));
        states.get(line).copied().unwrap_or(ConditionalTruth::True)
    }
}

fn include_directive(line: &str) -> Option<(bool, &str)> {
    let trimmed = strip_make_comment(line).trim();
    for (word, optional) in [("-include", true), ("sinclude", true), ("include", false)] {
        if let Some(tail) = trimmed.strip_prefix(word) {
            if tail.chars().next().is_some_and(char::is_whitespace) {
                return Some((optional, tail.trim()));
            }
        }
    }
    None
}

struct IncludeExpressionContext<'a> {
    scope: &'a VarScope,
    line: usize,
    target: &'a TargetContext,
    root: &'a Path,
    relative_dir: &'a Path,
}

impl IncludeExpressionContext<'_> {
    fn rewrite(
        &self,
        raw: &str,
        visiting: &mut HashSet<String>,
        depth: usize,
        flavor_uncertainty: &mut Option<String>,
    ) -> String {
        if depth == 0 {
            return raw.to_owned();
        }
        let mut output = String::with_capacity(raw.len());
        let mut cursor = 0;
        while cursor < raw.len() {
            let Some(relative) = raw[cursor..].find('$') else {
                output.push_str(&raw[cursor..]);
                break;
            };
            let dollar = cursor + relative;
            output.push_str(&raw[cursor..dollar]);
            if raw.as_bytes().get(dollar + 1) == Some(&b'$') {
                output.push_str("$$");
                cursor = dollar + 2;
                continue;
            }
            let Some((close, end)) = reference_end(raw, dollar) else {
                output.push('$');
                cursor = dollar + 1;
                continue;
            };
            let body = &raw[dollar + 2..end];
            let name = body.trim();
            if is_make_name(name) {
                let replacement = match name {
                    "SRCDIR" | "TOP" => Some(self.root.to_string_lossy().into_owned()),
                    "CURDIR" => Some(
                        self.root
                            .join(self.relative_dir)
                            .to_string_lossy()
                            .into_owned(),
                    ),
                    _ if self.scope.conditionally_assigned_before(name, self.line) => None,
                    _ => self
                        .scope
                        .flavor_uncertainty_reason_at(name, self.line)
                        .map_or_else(
                            || {
                                self.scope
                                    .raw_at(name, self.line)
                                    .or_else(|| self.target.value_of(name))
                            },
                            |reason| {
                                *flavor_uncertainty = Some(reason);
                                None
                            },
                        ),
                };
                if let Some(replacement) = replacement {
                    if visiting.insert(name.to_owned()) {
                        output.push_str(&self.rewrite(
                            &replacement,
                            visiting,
                            depth - 1,
                            flavor_uncertainty,
                        ));
                        visiting.remove(name);
                    } else {
                        output.push_str(&raw[dollar..=end]);
                    }
                } else {
                    output.push_str(&raw[dollar..=end]);
                }
            } else {
                output.push('$');
                output.push(raw.as_bytes()[dollar + 1] as char);
                output.push_str(&self.rewrite(body, visiting, depth - 1, flavor_uncertainty));
                output.push(close as char);
            }
            cursor = end + 1;
        }
        output
    }
}

fn reference_end(raw: &str, dollar: usize) -> Option<(u8, usize)> {
    let open = *raw.as_bytes().get(dollar + 1)?;
    let close = match open {
        b'(' => b')',
        b'{' => b'}',
        _ => return None,
    };
    let mut stack = vec![close];
    let mut index = dollar + 2;
    while index < raw.len() {
        if raw.as_bytes()[index] == b'$'
            && matches!(raw.as_bytes().get(index + 1), Some(b'(' | b'{'))
        {
            stack.push(if raw.as_bytes()[index + 1] == b'(' {
                b')'
            } else {
                b'}'
            });
            index += 2;
            continue;
        }
        if raw.as_bytes()[index] == *stack.last()? {
            stack.pop();
            if stack.is_empty() {
                return Some((close, index));
            }
        }
        index += 1;
    }
    None
}

fn contains_make_function(raw: &str) -> bool {
    let mut cursor = 0;
    while cursor < raw.len() {
        let Some(relative) = raw[cursor..].find('$') else {
            return false;
        };
        let dollar = cursor + relative;
        if raw.as_bytes().get(dollar + 1) == Some(&b'$') {
            cursor = dollar + 2;
            continue;
        }
        let Some((_, end)) = reference_end(raw, dollar) else {
            cursor = dollar + 1;
            continue;
        };
        if !is_make_name(raw[dollar + 2..end].trim()) {
            return true;
        }
        cursor = end + 1;
    }
    false
}

fn contains_make_reference(raw: &str) -> bool {
    let mut cursor = 0;
    while cursor < raw.len() {
        let Some(relative) = raw[cursor..].find('$') else {
            return false;
        };
        let dollar = cursor + relative;
        if raw.as_bytes().get(dollar + 1) == Some(&b'$') {
            cursor = dollar + 2;
            continue;
        }
        if reference_end(raw, dollar).is_some() {
            return true;
        }
        cursor = dollar + 1;
    }
    false
}

fn is_make_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
}

fn continuation_joined_lines(text: &str) -> Vec<(usize, String)> {
    let mut output = Vec::new();
    let mut pending = String::new();
    let mut start_line = 1;
    for (index, physical) in text.lines().enumerate() {
        let line_number = index + 1;
        if pending.is_empty() {
            start_line = line_number;
        }
        let trimmed_end = physical.trim_end_matches([' ', '\t']);
        if let Some(prefix) = trimmed_end.strip_suffix('\\') {
            pending.push_str(prefix);
            pending.push(' ');
            continue;
        }
        pending.push_str(physical.trim_start_matches([' ', '\t']));
        output.push((start_line, std::mem::take(&mut pending)));
    }
    if !pending.is_empty() {
        output.push((start_line, pending));
    }
    output
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn has_glob_metacharacters(path: &str) -> bool {
    path.chars()
        .any(|character| matches!(character, '*' | '?' | '[' | ']'))
}

#[cfg(test)]
#[path = "kobj_scoped_inputs_tests.rs"]
mod tests;
