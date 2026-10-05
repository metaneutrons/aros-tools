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
mod tests {
    use super::{
        capture_kobj_scoped_inputs, capture_kobj_scoped_inputs_for_module,
        capture_kobj_scoped_inputs_with_known_source_includes, collect_bounded_include_matches,
        KobjModuleArgs, KobjScopedInputs, ScopedMakeWords,
    };
    use crate::dirs::DirVars;
    use crate::parser::TargetContext;
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    fn fixture() -> (TempDir, PathBuf, DirVars, TargetContext) {
        let temp = tempfile::tempdir().expect("temporary source tree");
        let root = temp.path().to_path_buf();
        fs::create_dir_all(root.join("arch/pc/kernel")).expect("make option directory");
        let dirs = DirVars::load(&root);
        let target = TargetContext {
            platform: Some("pc".to_owned()),
            cpu: Some("x86_64".to_owned()),
            family: Some(String::new()),
            variant: Some(String::new()),
            ..TargetContext::default()
        };
        (temp, root, dirs, target)
    }

    fn capture(
        source: &str,
        line: usize,
        module: &str,
        flavour: Option<&str>,
        root: &Path,
        dirs: &DirVars,
        target: &TargetContext,
    ) -> KobjScopedInputs {
        capture_kobj_scoped_inputs(
            source,
            line,
            module,
            flavour,
            root,
            Path::new("rom/kernel/mmakefile.src"),
            dirs,
            target,
        )
    }

    fn capture_with_uselibs(
        source: &str,
        line: usize,
        module: &str,
        raw_uselibs: Option<&str>,
        root: &Path,
        dirs: &DirVars,
        target: &TargetContext,
    ) -> KobjScopedInputs {
        capture_kobj_scoped_inputs_for_module(
            source,
            line,
            KobjModuleArgs {
                module_name: module,
                flavour: None,
                raw_uselibs,
                raw_funcinstr: None,
            },
            root,
            Path::new("rom/kernel/mmakefile.src"),
            dirs,
            target,
            &[],
        )
    }

    fn capture_with_funcinstr(
        source: &str,
        line: usize,
        raw_funcinstr: Option<&str>,
        root: &Path,
        dirs: &DirVars,
        target: &TargetContext,
    ) -> KobjScopedInputs {
        capture_kobj_scoped_inputs_for_module(
            source,
            line,
            KobjModuleArgs {
                module_name: "kernel",
                flavour: None,
                raw_uselibs: None,
                raw_funcinstr,
            },
            root,
            Path::new("rom/kernel/mmakefile.src"),
            dirs,
            target,
            &[],
        )
    }

    fn exact(value: &ScopedMakeWords) -> (&str, &[String]) {
        match value {
            ScopedMakeWords::Exact { raw, words, .. } => (raw, words),
            other => panic!("expected exact Make words, got {other:?}"),
        }
    }

    fn known_empty(value: &ScopedMakeWords) {
        assert!(
            matches!(value, ScopedMakeWords::KnownEmpty { .. }),
            "{value:?}"
        );
    }

    fn unresolved(value: &ScopedMakeWords) {
        assert!(
            matches!(value, ScopedMakeWords::Unresolved { .. }),
            "{value:?}"
        );
    }

    #[test]
    fn wildcard_include_traversal_errors_cannot_publish_a_successful_subset() {
        let matches = [
            Ok(PathBuf::from("first/make.opts")),
            Err("unreadable source directory"),
            Ok(PathBuf::from("last/make.opts")),
        ];
        assert_eq!(
            collect_bounded_include_matches(matches.into_iter(), 3),
            Err("unreadable source directory")
        );
        let matches = [
            Ok::<_, &str>(PathBuf::from("first/make.opts")),
            Ok(PathBuf::from("last/make.opts")),
        ];
        assert_eq!(
            collect_bounded_include_matches(matches.into_iter(), 1).unwrap(),
            [PathBuf::from("first/make.opts")]
        );
    }

    #[test]
    fn repeated_make_opts_preserve_include_and_flag_order_duplicates() {
        let (_temp, root, dirs, target) = fixture();
        fs::write(
            root.join("arch/pc/kernel/make.opts"),
            "USER_LDFLAGS += -static -static\n",
        )
        .expect("write options");
        let source = "-include $(SRCDIR)/arch/$(ARCH)/kernel/make.opts\n-include $(SRCDIR)/arch/$(ARCH)/kernel/make.opts\n%build_module modname=kernel flavour=debug\n";
        let result = capture(source, 2, "kernel", Some("debug"), &root, &dirs, &target);
        assert_eq!(result.defname, "kernel_debug");
        assert_eq!(result.included_make_opts.len(), 2);
        assert_eq!(
            exact(&result.user_ldflags).1,
            ["-static", "-static", "-static", "-static"]
        );
    }

    #[test]
    fn uncertain_configuration_flavor_cannot_select_an_include_path() {
        let (_temp, root, dirs, target) = fixture();
        fs::create_dir_all(root.join("arch/early/kernel")).expect("early include directory");
        fs::create_dir_all(root.join("arch/late/kernel")).expect("late include directory");
        fs::write(
            root.join("arch/early/kernel/make.opts"),
            "USER_LDFLAGS := -early\n",
        )
        .expect("write early options");
        fs::write(
            root.join("arch/late/kernel/make.opts"),
            "USER_LDFLAGS := -late\n",
        )
        .expect("write late options");
        let target = TargetContext {
            make_variables: BTreeMap::from([("PATH".to_owned(), String::new())]),
            ..target
        };

        let uncertain = "EARLY := arch/early/kernel/make.opts\nPATH += $(EARLY)\nEARLY := arch/late/kernel/make.opts\n-include $(SRCDIR)/$(PATH)\n%build_module modname=kernel\n";
        let result = capture(uncertain, 4, "kernel", None, &root, &dirs, &target);
        unresolved(&result.user_ldflags);
        assert!(result.included_make_opts.is_empty());

        let simple = "EARLY := arch/early/kernel/make.opts\nPATH := $(EARLY)\nEARLY := arch/late/kernel/make.opts\n-include $(SRCDIR)/$(PATH)\n%build_module modname=kernel\n";
        let result = capture(simple, 4, "kernel", None, &root, &dirs, &target);
        assert_eq!(
            result
                .included_make_opts
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            ["arch/early/kernel/make.opts"]
        );
        assert_eq!(exact(&result.user_ldflags).1, ["-early"]);

        let recursive = "EARLY := arch/early/kernel/make.opts\nPATH = $(EARLY)\nEARLY := arch/late/kernel/make.opts\n-include $(SRCDIR)/$(PATH)\n%build_module modname=kernel\n";
        let result = capture(recursive, 4, "kernel", None, &root, &dirs, &target);
        assert_eq!(
            result
                .included_make_opts
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            ["arch/late/kernel/make.opts"]
        );
        assert_eq!(exact(&result.user_ldflags).1, ["-late"]);
    }

    #[test]
    fn source_local_recursive_overrides_are_evaluated_at_the_declaration() {
        let (_temp, root, dirs, target) = fixture();
        let source = "FLAGS = -first\nUSER_LDFLAGS = $(FLAGS)\n%build_module modname=kernel\nFLAGS = -later\n";
        let result = capture(source, 2, "kernel", None, &root, &dirs, &target);
        assert_eq!(exact(&result.user_ldflags).1, ["-first"]);
    }

    #[test]
    fn simple_assignment_freezes_its_local_reference_before_later_override() {
        let (_temp, root, dirs, target) = fixture();
        let source = "FLAGS = -initial\nUSER_LDFLAGS := $(FLAGS)\nFLAGS = -later\n%build_module modname=kernel\n";
        let result = capture(source, 3, "kernel", None, &root, &dirs, &target);
        assert_eq!(exact(&result.user_ldflags).1, ["-initial"]);
    }

    #[test]
    fn make_opts_replacement_and_following_local_append_keep_source_order() {
        let (_temp, root, dirs, target) = fixture();
        fs::write(
            root.join("arch/pc/kernel/make.opts"),
            "USER_LDFLAGS := -from-opts\n",
        )
        .expect("write options");
        let source = "USER_LDFLAGS := -before\n-include $(SRCDIR)/arch/$(ARCH)/kernel/make.opts\nUSER_LDFLAGS += -after\n%build_module modname=kernel\n";
        let result = capture(source, 3, "kernel", None, &root, &dirs, &target);
        assert_eq!(exact(&result.user_ldflags).1, ["-from-opts", "-after"]);
    }

    #[test]
    fn unknown_conditional_input_is_unresolved_until_a_proven_replacement() {
        let (_temp, root, dirs, target) = fixture();
        let source = "USER_LDFLAGS += -prefix\nifeq ($(NOT_IN_SOURCE_CONTRACT),1)\nUSER_LDFLAGS += -conditional\nendif\n%build_module modname=kernel\n";
        let result = capture(source, 4, "kernel", None, &root, &dirs, &target);
        unresolved(&result.user_ldflags);

        let reset = "ifeq ($(NOT_IN_SOURCE_CONTRACT),1)\nUSER_LDFLAGS += -conditional\nendif\nUSER_LDFLAGS := -reset\n%build_module modname=kernel\n";
        let result = capture(reset, 4, "kernel", None, &root, &dirs, &target);
        assert_eq!(exact(&result.user_ldflags).1, ["-reset"]);
    }

    #[test]
    fn simple_expansion_retains_dependency_taint_until_input_is_replaced() {
        let (_temp, root, dirs, target) = fixture();
        let source = "FLAGS = -base\nifeq ($(UNKNOWN),1)\nFLAGS = -conditional\nendif\nUSER_OBJS := $(FLAGS)\nFLAGS := -later\n%build_module modname=kernel\n";
        let result = capture(source, 6, "kernel", None, &root, &dirs, &target);
        unresolved(&result.user_objects);

        let replaced = "FLAGS = -base\nifeq ($(UNKNOWN),1)\nFLAGS = -conditional\nendif\nUSER_OBJS := $(FLAGS)\nFLAGS := -later\nUSER_OBJS := replacement.o\n%build_module modname=kernel\n";
        let result = capture(replaced, 7, "kernel", None, &root, &dirs, &target);
        assert_eq!(exact(&result.user_objects).1, ["replacement.o"]);
    }

    #[test]
    fn unresolved_include_taint_survives_freezing_but_literal_reset_clears_it() {
        let (_temp, root, dirs, target) = fixture();
        let source = "-include $(SRCDIR)/config/local.mk\nUSER_OBJS := $(FLAGS)\nFLAGS = -later\n%build_module modname=kernel\n";
        let result = capture(source, 3, "kernel", None, &root, &dirs, &target);
        unresolved(&result.user_objects);

        let replaced = "-include $(SRCDIR)/config/local.mk\nUSER_OBJS := $(FLAGS)\nFLAGS = -later\nUSER_OBJS := replacement.o\n%build_module modname=kernel\n";
        let result = capture(replaced, 4, "kernel", None, &root, &dirs, &target);
        assert_eq!(exact(&result.user_objects).1, ["replacement.o"]);

        let literal = "-include $(SRCDIR)/config/local.mk\nUSER_OBJS := literal.o\n%build_module modname=kernel\n";
        let result = capture(literal, 2, "kernel", None, &root, &dirs, &target);
        assert_eq!(exact(&result.user_objects).1, ["literal.o"]);
    }

    #[test]
    fn include_taint_flows_through_recursive_values_and_simple_appends() {
        let (_temp, root, dirs, target) = fixture();
        let recursive = "-include $(SRCDIR)/config/local.mk\nINTERMEDIATE = $(FLAGS)\nUSER_OBJS := $(INTERMEDIATE)\nFLAGS = -later\n%build_module modname=kernel\n";
        let result = capture(recursive, 4, "kernel", None, &root, &dirs, &target);
        unresolved(&result.user_objects);

        let appended = "-include $(SRCDIR)/config/local.mk\nUSER_OBJS := base.o\nUSER_OBJS += $(FLAGS)\nFLAGS = -later\n%build_module modname=kernel\n";
        let result = capture(appended, 4, "kernel", None, &root, &dirs, &target);
        unresolved(&result.user_objects);
    }

    #[test]
    fn append_and_optional_set_do_not_clear_an_unknown_assignment() {
        let (_temp, root, dirs, target) = fixture();
        let source = "ifeq ($(UNKNOWN),1)\nUSER_OBJS = conditional.o\nendif\nUSER_OBJS += suffix.o\n%build_module modname=kernel\n";
        let result = capture(source, 4, "kernel", None, &root, &dirs, &target);
        unresolved(&result.user_objects);

        let source = "ifeq ($(UNKNOWN),1)\nUSER_OBJS = conditional.o\nendif\nUSER_OBJS ?= fallback.o\n%build_module modname=kernel\n";
        let result = capture(source, 4, "kernel", None, &root, &dirs, &target);
        unresolved(&result.user_objects);
    }

    #[test]
    fn missing_required_include_is_unknown_but_missing_optional_include_is_empty() {
        let (_temp, root, dirs, target) = fixture();
        let required = "include $(SRCDIR)/arch/pc/kernel/make.opts\n%build_module modname=kernel\n";
        let result = capture(required, 1, "kernel", None, &root, &dirs, &target);
        unresolved(&result.user_objects);
        unresolved(&result.user_ldflags);

        let optional =
            "-include $(SRCDIR)/arch/pc/kernel/make.opts\n%build_module modname=kernel\n";
        let result = capture(optional, 1, "kernel", None, &root, &dirs, &target);
        known_empty(&result.user_objects);
        known_empty(&result.user_ldflags);
    }

    #[test]
    fn unresolved_optional_include_is_unknown_and_reset_can_clear_it() {
        let (_temp, root, dirs, target) = fixture();
        let unresolved_source =
            "-include $(UNKNOWN_OPTS_ROOT)/make.opts\n%build_module modname=kernel\n";
        let result = capture(unresolved_source, 1, "kernel", None, &root, &dirs, &target);
        unresolved(&result.user_objects);

        let reset =
            "-include $(UNKNOWN_OPTS_ROOT)/make.opts\nUSER_OBJS :=\n%build_module modname=kernel\n";
        let result = capture(reset, 2, "kernel", None, &root, &dirs, &target);
        known_empty(&result.user_objects);
    }

    #[test]
    fn unresolved_arbitrary_include_expression_cannot_become_known_empty() {
        let (_temp, root, dirs, target) = fixture();
        let source = "-include $(UNKNOWN_FRAGMENT)\n%build_module modname=kernel\n";
        let result = capture(source, 1, "kernel", None, &root, &dirs, &target);
        unresolved(&result.user_objects);
        unresolved(&result.user_ldflags);
    }

    #[test]
    fn non_makeopts_fragments_need_explicit_caller_binding() {
        let (_temp, root, dirs, target) = fixture();
        let source = "include $(SRCDIR)/config/make.cfg\n%build_module modname=kernel\n";
        let result = capture(source, 1, "kernel", None, &root, &dirs, &target);
        unresolved(&result.user_objects);

        let source = "-include $(TOP)/config/make.cfg\n%build_module modname=kernel\n";
        let result = capture_kobj_scoped_inputs_with_known_source_includes(
            source,
            1,
            "kernel",
            None,
            &root,
            Path::new("rom/kernel/mmakefile.src"),
            &dirs,
            &target,
            &["$(TOP)/config/make.cfg".to_owned()],
        );
        known_empty(&result.user_objects);
    }

    #[test]
    fn mixed_makeopts_and_non_makeopts_include_list_is_unresolved() {
        let (_temp, root, dirs, target) = fixture();
        fs::write(
            root.join("arch/pc/kernel/make.opts"),
            "USER_LDFLAGS := -from-options\n",
        )
        .expect("write options");
        let source = "-include $(SRCDIR)/arch/pc/kernel/make.opts $(SRCDIR)/config/local.mk\n%build_module modname=kernel\n";
        let result = capture(source, 1, "kernel", None, &root, &dirs, &target);
        unresolved(&result.user_ldflags);
    }

    #[test]
    fn target_context_selects_make_opts_conditionals() {
        let (_temp, root, dirs, target) = fixture();
        fs::write(
            root.join("arch/pc/kernel/make.opts"),
            "ifeq ($(AROS_TARGET_CPU),x86_64)\nUSER_LDFLAGS += -cpu\nelse\nUSER_LDFLAGS += -other\nendif\n",
        )
        .expect("write options");
        let source =
            "-include $(SRCDIR)/arch/$(ARCH)/kernel/make.opts\n%build_module modname=kernel\n";
        let result = capture(source, 1, "kernel", None, &root, &dirs, &target);
        assert_eq!(exact(&result.user_ldflags).1, ["-cpu"]);
    }

    #[test]
    fn defname_libs_uses_the_computed_flavoured_name() {
        let (_temp, root, dirs, target) = fixture();
        let source = "kernel_LIBS := wrong.a\nkernel_debug_LIBS := right.a\n%build_module modname=kernel flavour=debug\n";
        let result = capture(source, 2, "kernel", Some("debug"), &root, &dirs, &target);
        assert_eq!(result.defname, "kernel_debug");
        assert_eq!(exact(&result.defname_libs).1, ["right.a"]);

        let source = "kernel_LIBS := correct.a\nkernel_debug_LIBS := wrong.a\n%build_module modname=kernel\n";
        let result = capture(source, 2, "kernel", None, &root, &dirs, &target);
        assert_eq!(result.defname, "kernel");
        assert_eq!(exact(&result.defname_libs).1, ["correct.a"]);
    }

    #[test]
    fn additional_kobj_link_variables_capture_exact_empty_and_unresolved_states() {
        let (_temp, root, dirs, target) = fixture();
        let source = "KOBJ_LDFLAGS := -Wl,--gc-sections -Wl,--gc-sections\nKERNEL_KOBJ_LDSCRIPT := kernel.ld\nFUNCINSTR_LIBS := instr.a instr.a\n%build_module modname=kernel\n";
        let result = capture(source, 3, "kernel", None, &root, &dirs, &target);
        assert_eq!(
            exact(&result.kobj_ldflags).1,
            ["-Wl,--gc-sections", "-Wl,--gc-sections"]
        );
        assert_eq!(exact(&result.kernel_kobj_ldscript).1, ["kernel.ld"]);
        assert_eq!(exact(&result.funcinstr_libs).1, ["instr.a", "instr.a"]);
        known_empty(&result.use_libs);

        let empty = capture(
            "%build_module modname=kernel\n",
            0,
            "kernel",
            None,
            &root,
            &dirs,
            &target,
        );
        unresolved(&empty.kobj_ldflags);
        unresolved(&empty.kernel_kobj_ldscript);
        unresolved(&empty.funcinstr_libs);

        let conditional = "ifeq ($(UNBOUND_FEATURE),1)\nFUNCINSTR_LIBS := optional.a\nendif\n%build_module modname=kernel\n";
        let result = capture(conditional, 3, "kernel", None, &root, &dirs, &target);
        unresolved(&result.funcinstr_libs);

        let included = "-include $(SRCDIR)/config/local.mk\nKERNEL_KOBJ_LDSCRIPT := $(LDSCRIPT)\nLDSCRIPT = later.ld\n%build_module modname=kernel\n";
        let result = capture(included, 3, "kernel", None, &root, &dirs, &target);
        unresolved(&result.kernel_kobj_ldscript);
    }

    #[test]
    fn required_global_link_inputs_distinguish_absent_from_explicit_empty() {
        let (_temp, root, dirs, target) = fixture();
        let absent = capture(
            "%build_module modname=kernel\n",
            0,
            "kernel",
            None,
            &root,
            &dirs,
            &target,
        );
        for value in [
            &absent.kobj_ldflags,
            &absent.kernel_kobj_ldscript,
            &absent.funcinstr_libs,
        ] {
            let ScopedMakeWords::Unresolved { reason, .. } = value else {
                panic!("absent global input must remain unresolved: {value:?}");
            };
            assert!(
                reason.contains("no source or target definition"),
                "{reason}"
            );
        }

        let explicit_empty = "KOBJ_LDFLAGS :=\nKERNEL_KOBJ_LDSCRIPT :=\nFUNCINSTR_LIBS :=\n%build_module modname=kernel\n";
        let result = capture(explicit_empty, 3, "kernel", None, &root, &dirs, &target);
        known_empty(&result.kobj_ldflags);
        known_empty(&result.kernel_kobj_ldscript);
        known_empty(&result.funcinstr_libs);

        let target_nonempty = TargetContext {
            make_variables: [
                ("KOBJ_LDFLAGS".to_owned(), "-target-flag".to_owned()),
                ("KERNEL_KOBJ_LDSCRIPT".to_owned(), "target.ld".to_owned()),
                ("FUNCINSTR_LIBS".to_owned(), "target-instr.a".to_owned()),
            ]
            .into_iter()
            .collect(),
            ..target
        };
        let result = capture(
            "%build_module modname=kernel\n",
            0,
            "kernel",
            None,
            &root,
            &dirs,
            &target_nonempty,
        );
        assert_eq!(exact(&result.kobj_ldflags).1, ["-target-flag"]);
        assert_eq!(exact(&result.kernel_kobj_ldscript).1, ["target.ld"]);
        assert_eq!(exact(&result.funcinstr_libs).1, ["target-instr.a"]);

        let result = capture(
            "KOBJ_LDFLAGS :=\nKERNEL_KOBJ_LDSCRIPT :=\nFUNCINSTR_LIBS :=\n%build_module modname=kernel\n",
            3,
            "kernel",
            None,
            &root,
            &dirs,
            &target_nonempty,
        );
        known_empty(&result.kobj_ldflags);
        known_empty(&result.kernel_kobj_ldscript);
        known_empty(&result.funcinstr_libs);
    }

    #[test]
    fn macro_uselibs_preserves_order_duplicates_and_only_guards_references() {
        let (_temp, root, dirs, target) = fixture();
        let source = "LOCAL_LIBS := -lfirst -lsecond\n%build_module modname=kernel\n";
        let result = capture_with_uselibs(
            source,
            1,
            "kernel",
            Some("$(LOCAL_LIBS) -lrepeat -lrepeat"),
            &root,
            &dirs,
            &target,
        );
        assert_eq!(
            exact(&result.use_libs).1,
            ["-lfirst", "-lsecond", "-lrepeat", "-lrepeat"]
        );

        let unresolved_include =
            "-include $(SRCDIR)/config/unbound.mk\n%build_module modname=kernel\n";
        let absent =
            capture_with_uselibs(unresolved_include, 1, "kernel", None, &root, &dirs, &target);
        known_empty(&absent.use_libs);

        let literal = capture_with_uselibs(
            unresolved_include,
            1,
            "kernel",
            Some("-lliteral -lliteral"),
            &root,
            &dirs,
            &target,
        );
        assert_eq!(exact(&literal.use_libs).1, ["-lliteral", "-lliteral"]);

        let referenced = capture_with_uselibs(
            unresolved_include,
            1,
            "kernel",
            Some("$(LIBS_FROM_CONFIG)"),
            &root,
            &dirs,
            &target,
        );
        unresolved(&referenced.use_libs);

        let conditional = "ifeq ($(UNBOUND_FEATURE),1)\nMACRO_LIBS := -conditional\nendif\n%build_module modname=kernel\n";
        let referenced = capture_with_uselibs(
            conditional,
            3,
            "kernel",
            Some("$(MACRO_LIBS)"),
            &root,
            &dirs,
            &target,
        );
        unresolved(&referenced.use_libs);
    }

    #[test]
    fn function_instrumentation_uses_macro_or_source_default_without_guessing() {
        let (_temp, root, dirs, target) = fixture();
        let source = "%build_module modname=kernel\n";
        let explicit_no = capture_with_funcinstr(source, 0, Some("no"), &root, &dirs, &target);
        assert_eq!(exact(&explicit_no.function_instrumentation).1, ["no"]);
        let explicit_yes = capture_with_funcinstr(source, 0, Some("yes"), &root, &dirs, &target);
        assert_eq!(exact(&explicit_yes.function_instrumentation).1, ["yes"]);

        let source_default = "TARGET_FUNCINSTR := yes\n%build_module modname=kernel\n";
        let defaulted = capture_with_funcinstr(source_default, 1, None, &root, &dirs, &target);
        assert_eq!(exact(&defaulted.function_instrumentation).1, ["yes"]);

        let unbound = capture_with_funcinstr(source, 0, None, &root, &dirs, &target);
        unresolved(&unbound.function_instrumentation);

        let explicit_unbound = capture_with_funcinstr(
            source,
            0,
            Some("$(UNKNOWN_SELECTOR)"),
            &root,
            &dirs,
            &target,
        );
        unresolved(&explicit_unbound.function_instrumentation);

        let explicit_unbound_target_ref = capture_with_funcinstr(
            source,
            0,
            Some("$(TARGET_FUNCINSTR)"),
            &root,
            &dirs,
            &target,
        );
        unresolved(&explicit_unbound_target_ref.function_instrumentation);

        let indirect_unbound_source =
            "SELECTOR := $(UNKNOWN_SELECTOR)\n%build_module modname=kernel\n";
        let indirect_unbound = capture_with_funcinstr(
            indirect_unbound_source,
            1,
            Some("$(SELECTOR)"),
            &root,
            &dirs,
            &target,
        );
        unresolved(&indirect_unbound.function_instrumentation);

        let bound_source = "SELECTOR := yes\n%build_module modname=kernel\n";
        let bound =
            capture_with_funcinstr(bound_source, 1, Some("$(SELECTOR)"), &root, &dirs, &target);
        assert_eq!(exact(&bound.function_instrumentation).1, ["yes"]);

        let mut bound_target = target.clone();
        bound_target
            .make_variables
            .insert("TARGET_FUNCINSTR".to_owned(), "no".to_owned());
        let bound_default = capture_with_funcinstr(source, 0, None, &root, &dirs, &bound_target);
        assert_eq!(exact(&bound_default.function_instrumentation).1, ["no"]);
        let bound_explicit_target_ref = capture_with_funcinstr(
            source,
            0,
            Some("$(TARGET_FUNCINSTR)"),
            &root,
            &dirs,
            &bound_target,
        );
        assert_eq!(
            exact(&bound_explicit_target_ref.function_instrumentation).1,
            ["no"]
        );

        let indirect_target = TargetContext {
            make_variables: std::collections::BTreeMap::from([(
                "SELECTOR".to_owned(),
                "$(UNKNOWN_SELECTOR)".to_owned(),
            )]),
            ..target.clone()
        };
        let indirect_target = capture_with_funcinstr(
            source,
            0,
            Some("$(SELECTOR)"),
            &root,
            &dirs,
            &indirect_target,
        );
        unresolved(&indirect_target.function_instrumentation);

        let conditional =
            "ifeq ($(UNKNOWN),1)\nTARGET_FUNCINSTR := yes\nendif\n%build_module modname=kernel\n";
        let referenced = capture_with_funcinstr(
            conditional,
            3,
            Some("$(TARGET_FUNCINSTR)"),
            &root,
            &dirs,
            &target,
        );
        unresolved(&referenced.function_instrumentation);
    }

    #[test]
    fn undefined_variables_are_make_empty_only_in_a_complete_source_view() {
        let (_temp, root, dirs, target) = fixture();
        let source = "USER_LDFLAGS = $(NOT_DEFINED_HERE) -static\n%build_module modname=kernel\n";
        let result = capture(source, 1, "kernel", None, &root, &dirs, &target);
        assert_eq!(exact(&result.user_ldflags).1, ["-static"]);

        let source = "%build_module modname=kernel\n";
        let result = capture(source, 0, "kernel", None, &root, &dirs, &target);
        known_empty(&result.user_ldflags);
    }

    #[test]
    fn unresolved_include_inside_known_false_branch_does_not_poison_scope() {
        let (_temp, root, dirs, mut target) = fixture();
        target
            .make_variables
            .insert("FEATURE".to_owned(), "0".to_owned());
        let source = "ifeq ($(FEATURE),1)\ninclude $(SRCDIR)/missing/make.opts\nendif\n%build_module modname=kernel\n";
        let result = capture(source, 3, "kernel", None, &root, &dirs, &target);
        known_empty(&result.user_objects);
    }

    #[test]
    fn native_config_binding_replaces_source_at_both_root_spellings_and_keeps_order() {
        let (_temp, root, dirs, target) = fixture();
        fs::create_dir_all(root.join("config")).expect("configuration directory");
        fs::write(
            root.join("config/original.cfg"),
            "USER_LDFLAGS += -original\n",
        )
        .expect("write original configuration");
        fs::write(
            root.join("config/native.mk"),
            "USER_LDFLAGS += -config\nKOBJ_LDFLAGS += -kobj\nKERNEL_KOBJ_LDSCRIPT := kernel.ld\nFUNCINSTR_LIBS += instr.a\n",
        )
        .expect("write native configuration");
        let target = TargetContext {
            make_include_bindings: BTreeMap::from([(
                "config/original.cfg".to_owned(),
                "config/native.mk".to_owned(),
            )]),
            ..target
        };
        let source = "-include $(SRCDIR)/config/original.cfg\nUSER_LDFLAGS += -local\n-include $(TOP)/config/original.cfg\n%build_module modname=kernel\n";
        let result = capture(source, 3, "kernel", None, &root, &dirs, &target);

        assert_eq!(
            exact(&result.user_ldflags).1,
            ["-config", "-local", "-config"]
        );
        assert_eq!(exact(&result.kobj_ldflags).1, ["-kobj", "-kobj"]);
        assert_eq!(exact(&result.kernel_kobj_ldscript).1, ["kernel.ld"]);
        assert_eq!(exact(&result.funcinstr_libs).1, ["instr.a", "instr.a"]);
        assert!(result.included_make_opts.is_empty());
        assert_eq!(
            result
                .included_configuration_files
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            ["config/native.mk", "config/native.mk"]
        );
    }

    #[test]
    fn missing_optional_original_with_a_binding_remains_unresolved() {
        let (_temp, root, dirs, target) = fixture();
        fs::create_dir_all(root.join("config")).expect("configuration directory");
        fs::write(root.join("config/native.mk"), "USER_LDFLAGS := -config\n")
            .expect("write native configuration");
        let target = TargetContext {
            make_include_bindings: BTreeMap::from([(
                "config/original.cfg".to_owned(),
                "config/native.mk".to_owned(),
            )]),
            ..target
        };
        let source = "-include $(SRCDIR)/config/original.cfg\n%build_module modname=kernel\n";
        let result = capture(source, 1, "kernel", None, &root, &dirs, &target);

        unresolved(&result.user_ldflags);
        unresolved(&result.kobj_ldflags);
        assert!(result.included_configuration_files.is_empty());
    }

    #[test]
    fn nested_unmapped_include_in_a_replacement_stays_unresolved() {
        let (_temp, root, dirs, target) = fixture();
        fs::create_dir_all(root.join("config")).expect("configuration directory");
        fs::write(
            root.join("config/original.cfg"),
            "ignored by native replacement\n",
        )
        .expect("write original configuration");
        fs::write(
            root.join("config/native.mk"),
            "-include $(SRCDIR)/config/nested.cfg\n",
        )
        .expect("write native configuration");
        let target = TargetContext {
            make_include_bindings: BTreeMap::from([(
                "config/original.cfg".to_owned(),
                "config/native.mk".to_owned(),
            )]),
            ..target
        };
        let source = "-include $(SRCDIR)/config/original.cfg\n%build_module modname=kernel\n";
        let result = capture(source, 1, "kernel", None, &root, &dirs, &target);

        unresolved(&result.user_ldflags);
        assert_eq!(result.included_configuration_files.len(), 1);
    }

    #[test]
    fn configuration_replacement_cycle_escape_and_symlink_are_rejected() {
        let (_temp, root, dirs, target) = fixture();
        fs::create_dir_all(root.join("config")).expect("configuration directory");
        fs::write(root.join("config/original.cfg"), "ignored\n").expect("write original");
        fs::write(
            root.join("config/cycle.mk"),
            "-include $(SRCDIR)/config/original.cfg\n",
        )
        .expect("write cyclic replacement");
        let target = TargetContext {
            make_include_bindings: BTreeMap::from([(
                "config/original.cfg".to_owned(),
                "config/cycle.mk".to_owned(),
            )]),
            ..target
        };
        let source = "-include $(SRCDIR)/config/original.cfg\n%build_module modname=kernel\n";
        let cycle = capture(source, 1, "kernel", None, &root, &dirs, &target);
        unresolved(&cycle.user_ldflags);

        let (_temp, root, dirs, target) = fixture();
        fs::create_dir_all(root.join("config")).expect("configuration directory");
        fs::write(root.join("config/original.cfg"), "ignored\n").expect("write original");
        let target = TargetContext {
            make_include_bindings: BTreeMap::from([(
                "config/original.cfg".to_owned(),
                "../outside.mk".to_owned(),
            )]),
            ..target
        };
        let escaped = capture(source, 1, "kernel", None, &root, &dirs, &target);
        unresolved(&escaped.user_ldflags);

        #[cfg(unix)]
        {
            let (_temp, root, dirs, target) = fixture();
            fs::create_dir_all(root.join("config")).expect("configuration directory");
            fs::write(root.join("config/original.cfg"), "ignored\n").expect("write original");
            fs::write(root.join("config/real.mk"), "USER_LDFLAGS := -native\n")
                .expect("write replacement target");
            std::os::unix::fs::symlink(root.join("config/real.mk"), root.join("config/link.mk"))
                .expect("create replacement symlink");
            let target = TargetContext {
                make_include_bindings: BTreeMap::from([(
                    "config/original.cfg".to_owned(),
                    "config/link.mk".to_owned(),
                )]),
                ..target
            };
            let symlink = capture(source, 1, "kernel", None, &root, &dirs, &target);
            unresolved(&symlink.user_ldflags);
        }
    }

    #[test]
    fn mixed_mapped_configuration_and_make_opts_include_is_refused_as_a_unit() {
        let (_temp, root, dirs, target) = fixture();
        fs::create_dir_all(root.join("config")).expect("configuration directory");
        fs::write(root.join("config/original.cfg"), "ignored\n").expect("write original");
        fs::write(root.join("config/native.mk"), "USER_LDFLAGS := -native\n")
            .expect("write native configuration");
        fs::write(
            root.join("arch/pc/kernel/make.opts"),
            "USER_LDFLAGS := -make-opts\n",
        )
        .expect("write make.opts");
        let target = TargetContext {
            make_include_bindings: BTreeMap::from([(
                "config/original.cfg".to_owned(),
                "config/native.mk".to_owned(),
            )]),
            ..target
        };
        let source = "-include $(SRCDIR)/config/original.cfg $(SRCDIR)/arch/pc/kernel/make.opts\n%build_module modname=kernel\n";
        let result = capture(source, 1, "kernel", None, &root, &dirs, &target);

        unresolved(&result.user_ldflags);
        assert!(result.included_make_opts.is_empty());
        assert!(result.included_configuration_files.is_empty());
    }

    #[test]
    fn inactive_mapped_include_is_ignored_and_unsafe_original_paths_are_refused() {
        let (_temp, root, dirs, mut target) = fixture();
        target
            .make_variables
            .insert("FEATURE".to_owned(), "0".to_owned());
        target.make_include_bindings = BTreeMap::from([(
            "config/missing.cfg".to_owned(),
            "config/native.mk".to_owned(),
        )]);
        let inactive = "ifeq ($(FEATURE),1)\n-include $(SRCDIR)/config/missing.cfg\nendif\n%build_module modname=kernel\n";
        let result = capture(inactive, 3, "kernel", None, &root, &dirs, &target);
        known_empty(&result.user_ldflags);
        assert!(result.included_configuration_files.is_empty());

        fs::create_dir_all(root.join("config")).expect("configuration directory");
        fs::write(root.join("config/original.cfg"), "ignored\n").expect("write original");
        fs::write(root.join("config/native.mk"), "USER_LDFLAGS := -native\n")
            .expect("write native configuration");
        target.make_include_bindings = BTreeMap::from([(
            "config/original.cfg".to_owned(),
            "config/native.mk".to_owned(),
        )]);
        let unsafe_source =
            "-include $(SRCDIR)/../config/original.cfg\n%build_module modname=kernel\n";
        let result = capture(unsafe_source, 1, "kernel", None, &root, &dirs, &target);
        unresolved(&result.user_ldflags);
        assert!(result.included_configuration_files.is_empty());
    }

    #[test]
    fn undefine_is_declaration_scoped_known_empty_and_carries_source_provenance() {
        let (_temp, root, dirs, _) = fixture();
        let target = TargetContext {
            make_variables: BTreeMap::from([("KOBJ_LDFLAGS".to_owned(), "-target".to_owned())]),
            ..TargetContext::default()
        };
        let source = "KOBJ_LDFLAGS := -source\n%build_module modname=kernel\nundefine KOBJ_LDFLAGS\n%build_module modname=kernel\n";

        let before = capture(source, 1, "kernel", None, &root, &dirs, &target);
        assert_eq!(exact(&before.kobj_ldflags).1, ["-source"]);

        let after = capture(source, 3, "kernel", None, &root, &dirs, &target);
        let ScopedMakeWords::KnownEmpty { raw, source } = &after.kobj_ldflags else {
            panic!(
                "undefine must be a proven empty source value: {:?}",
                after.kobj_ldflags
            );
        };
        assert_eq!(raw.as_deref(), Some(""));
        assert_eq!(
            source,
            &[super::KobjSourceRef {
                path: "rom/kernel/mmakefile.src".to_owned(),
                line: 3,
            }]
        );
    }

    #[test]
    fn undefine_allows_set_if_unset_to_replace_a_target_default() {
        let (_temp, root, dirs, _) = fixture();
        let target = TargetContext {
            make_variables: BTreeMap::from([("KOBJ_LDFLAGS".to_owned(), "-target".to_owned())]),
            ..TargetContext::default()
        };
        let source =
            "undefine KOBJ_LDFLAGS\nKOBJ_LDFLAGS ?= -source\n%build_module modname=kernel\n";
        let result = capture(source, 2, "kernel", None, &root, &dirs, &target);

        assert_eq!(exact(&result.kobj_ldflags).1, ["-source"]);
    }

    #[test]
    fn unknown_conditional_undefine_is_unresolved_but_inactive_undefine_is_ignored() {
        let (_temp, root, dirs, _) = fixture();
        let target = TargetContext {
            make_variables: BTreeMap::from([("KOBJ_LDFLAGS".to_owned(), "-target".to_owned())]),
            ..TargetContext::default()
        };
        let source = "ifeq ($(UNKNOWN_FEATURE),1)\nundefine KOBJ_LDFLAGS\nendif\n%build_module modname=kernel\n";
        let result = capture(source, 3, "kernel", None, &root, &dirs, &target);
        unresolved(&result.kobj_ldflags);

        let target = TargetContext {
            make_variables: [
                ("KOBJ_LDFLAGS".to_owned(), "-target".to_owned()),
                ("FEATURE".to_owned(), "0".to_owned()),
            ]
            .into_iter()
            .collect(),
            ..TargetContext::default()
        };
        let source =
            "ifeq ($(FEATURE),1)\nundefine KOBJ_LDFLAGS\nendif\n%build_module modname=kernel\n";
        let result = capture(source, 3, "kernel", None, &root, &dirs, &target);
        assert_eq!(exact(&result.kobj_ldflags).1, ["-target"]);
    }

    #[test]
    fn assignments_after_undefine_rebind_with_recursive_append_or_simple_freeze() {
        let (_temp, root, dirs, target) = fixture();
        let recursive = "FLAGS := -early\nKOBJ_LDFLAGS := -before\nundefine KOBJ_LDFLAGS\nKOBJ_LDFLAGS += $(FLAGS)\nFLAGS := -late\n%build_module modname=kernel\n";
        let result = capture(recursive, 5, "kernel", None, &root, &dirs, &target);
        assert_eq!(exact(&result.kobj_ldflags).1, ["-late"]);

        let simple = "FLAGS := -early\nundefine KOBJ_LDFLAGS\nKOBJ_LDFLAGS := $(FLAGS)\nFLAGS := -late\n%build_module modname=kernel\n";
        let result = capture(simple, 4, "kernel", None, &root, &dirs, &target);
        assert_eq!(exact(&result.kobj_ldflags).1, ["-early"]);
    }

    #[test]
    fn configured_append_flavor_uncertainty_is_refused_and_source_resets_are_honored() {
        let (_temp, root, dirs, _) = fixture();
        let configured = TargetContext {
            make_variables: BTreeMap::from([("KOBJ_LDFLAGS".to_owned(), "-configured".to_owned())]),
            ..TargetContext::default()
        };
        let uncertain = "LATER := -early\nKOBJ_LDFLAGS += $(LATER)\nLATER := -late\n%build_module modname=kernel\n";
        let result = capture(uncertain, 3, "kernel", None, &root, &dirs, &configured);
        let ScopedMakeWords::Unresolved { reason, .. } = &result.kobj_ldflags else {
            panic!("configuration-only += must not guess the variable flavor");
        };
        assert!(reason.contains("flavor is unknown"));

        let literal = "KOBJ_LDFLAGS += -literal\n%build_module modname=kernel\n";
        let result = capture(literal, 1, "kernel", None, &root, &dirs, &configured);
        assert_eq!(exact(&result.kobj_ldflags).1, ["-configured", "-literal"]);

        let simple = "KOBJ_LDFLAGS := -source\nLATER := -early\nKOBJ_LDFLAGS += $(LATER)\nLATER := -late\n%build_module modname=kernel\n";
        let result = capture(simple, 4, "kernel", None, &root, &dirs, &configured);
        assert_eq!(exact(&result.kobj_ldflags).1, ["-source", "-early"]);

        let recursive = "KOBJ_LDFLAGS = -source\nLATER := -early\nKOBJ_LDFLAGS += $(LATER)\nLATER := -late\n%build_module modname=kernel\n";
        let result = capture(recursive, 4, "kernel", None, &root, &dirs, &configured);
        assert_eq!(exact(&result.kobj_ldflags).1, ["-source", "-late"]);

        let transitive = "LATER := -early\nFLAGS += $(LATER)\nKOBJ_LDFLAGS := $(FLAGS)\nLATER := -late\n%build_module modname=kernel\n";
        let transitive_context = TargetContext {
            make_variables: BTreeMap::from([("FLAGS".to_owned(), "-configured".to_owned())]),
            ..TargetContext::default()
        };
        let result = capture(
            transitive,
            4,
            "kernel",
            None,
            &root,
            &dirs,
            &transitive_context,
        );
        let ScopedMakeWords::Unresolved { reason, .. } = &result.kobj_ldflags else {
            panic!("simple assignments must retain flavor uncertainty from their inputs");
        };
        assert!(reason.contains("flavor"));

        let reset = "LATER := -early\nKOBJ_LDFLAGS += $(LATER)\nLATER := -late\nKOBJ_LDFLAGS := -reset\n%build_module modname=kernel\n";
        let result = capture(reset, 4, "kernel", None, &root, &dirs, &configured);
        assert_eq!(exact(&result.kobj_ldflags).1, ["-reset"]);

        let mut inactive_context = configured;
        inactive_context
            .make_variables
            .insert("FEATURE".to_owned(), "0".to_owned());
        let inactive =
            "ifeq ($(FEATURE),1)\nKOBJ_LDFLAGS += $(LATER)\nendif\n%build_module modname=kernel\n";
        let result = capture(inactive, 3, "kernel", None, &root, &dirs, &inactive_context);
        assert_eq!(exact(&result.kobj_ldflags).1, ["-configured"]);
    }

    #[test]
    fn dynamic_undefine_syntax_does_not_fall_back_to_a_target_value() {
        let (_temp, root, dirs, _) = fixture();
        let target = TargetContext {
            make_variables: BTreeMap::from([("KOBJ_LDFLAGS".to_owned(), "-target".to_owned())]),
            ..TargetContext::default()
        };
        let source = "undefine $(DYNAMIC_NAME)\n%build_module modname=kernel\n";
        let result = capture(source, 1, "kernel", None, &root, &dirs, &target);

        unresolved(&result.kobj_ldflags);
    }
}
