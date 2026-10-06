//! Closed projection for source-owned C-to-assembly generated headers.
//!
//! The admitted shape compiles one regular source file to a build-tree `.s`
//! file, then extracts quoted assembler strings into a build-tree `.h` file
//! with the bounded `grep | cut | sed` recipe. A finite `#MM` provider and a
//! finite aggregate declaration must own that header. This module reads no
//! generated files, does not execute Make or shell commands, and does not
//! infer compiler flags.

use crate::dirs::DirVars;
use crate::make_expr::{evaluate_make_expr, evaluate_make_list, MakeExprContext};
use crate::make_vars::{
    collect_vars_impl, strip_make_comment, undefine_directive, variable_assignment, AssignmentKind,
    ConditionalTruth, VarScope,
};
use crate::parser::TargetContext;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

const SOURCE_ALIAS: &str = "${AROS_SOURCE_DIR}";
const BUILD_ALIAS: &str = "${AROS_BUILD_DIR}";
const MAX_SNAPSHOT_BYTES: usize = 2 * 1024 * 1024;
const MAX_RULES: usize = 16_384;
const MAX_REJECTIONS: usize = 256;

/// One closed C-to-assembly-to-header producer and its owning Make targets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssemblyHeaderDecl {
    /// Source-local `#MM` provider target for the generated header.
    pub owner: String,
    /// Source-local aggregate target that exposes the provider.
    pub aggregate_owner: String,
    /// Expanded ordered prerequisites of the aggregate declaration.
    pub aggregate_dependencies: Vec<String>,
    /// Source-relative mmakefile path.
    pub file: String,
    /// 1-based physical source line of the provider target rule.
    pub line: usize,
    /// Regular C source under `${AROS_SOURCE_DIR}`.
    pub source: String,
    /// Assembly output under `${AROS_BUILD_DIR}`.
    pub assembly_output: String,
    /// Header output under `${AROS_BUILD_DIR}`.
    pub header_output: String,
    /// Resolved source-configured Make `GENINCDIR` root for this header.
    pub header_root: String,
    /// Ordered compiler arguments, excluding the compiler, source, `-S`, and
    /// output pair. Every argument comes from the explicit Make scope.
    pub arguments: Vec<String>,
    /// The admitted grep selector, `.asciz` or `.ascii`.
    pub token: String,
}

/// A relevant assembly-header producer that could not be proved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssemblyHeaderRejection {
    /// Best-known local owner, or `<unknown>` when ownership is ambiguous.
    pub owner: String,
    /// Source-relative mmakefile path.
    pub file: String,
    /// 1-based physical source line before Make continuation joining.
    pub line: usize,
    /// Why the producer is outside this closed capability.
    pub reason: String,
}

#[derive(Debug, Clone)]
struct Recipe {
    text: String,
    state: ConditionalTruth,
}

#[derive(Debug, Clone)]
struct Rule {
    target: String,
    prerequisites: String,
    order_only: Option<String>,
    line: usize,
    state: ConditionalTruth,
    bare_meta_line: Option<usize>,
    inline_recipe: Option<String>,
    recipes: Vec<Recipe>,
}

#[derive(Debug, Clone)]
struct MetaRule {
    target: String,
    prerequisites: String,
    line: usize,
    state: ConditionalTruth,
}

#[derive(Debug, Clone)]
struct SnapshotRules {
    rules: Vec<Rule>,
    meta_rules: Vec<MetaRule>,
}

struct PreparedNativeScope {
    /// Joined native Make scope after explicit includes and arch include
    /// invocations have been expanded. Source lines retain their mapped slots.
    joined: String,
    /// Rule-only view with the same joined line positions. Inserted include
    /// contents are blank, so they can contribute variables but never owners.
    rule_view: String,
    /// For each joined scope line, the corresponding zero-based physical
    /// source line. Inserted configuration lines inherit their include line.
    physical_lines: Vec<usize>,
    /// Exact physical owner of each joined line, without inheritance.
    physical_owner_lines: Vec<Option<usize>>,
    /// Architecture-provided `-D` argv which may appear through
    /// `PRIV_EXEC_INCLUDES` despite that Make variable normally being
    /// include-only.
    arch_definitions: Vec<String>,
    scope: VarScope,
    line_states: Vec<ConditionalTruth>,
}

struct GetArchIncludesRequest {
    modname: String,
    maindir: String,
    includeflag: String,
}

/// Expands one physical Make recipe through explicitly bound native
/// configuration and the target's source-proved `%get_archincludes` effects.
///
/// The returned text is joined like a Make parser input. Physical source
/// positions are maintained internally while includes are expanded, so
/// inserted configuration can supply variable values without becoming a
/// physical declaration owner.
///
/// # Errors
/// Returns an error when a local include is incomplete or unbound, a source
/// position cannot be reconstructed, or an active architecture include macro
/// lacks an unambiguous source-proved provider.
pub(crate) fn native_configuration_snapshot(
    snapshot: &str,
    target: &TargetContext,
    dirs: &DirVars,
    root: &Path,
    relative_recipe: &Path,
) -> Result<NativeConfigurationSnapshot, String> {
    prepare_native_scope(snapshot, target, dirs, root, relative_recipe).map(|prepared| {
        NativeConfigurationSnapshot {
            joined: prepared.joined,
            physical_owner_lines: prepared.physical_owner_lines,
        }
    })
}

/// Joined native Make scope plus the physical ownership of each joined line.
pub(crate) struct NativeConfigurationSnapshot {
    /// Joined scope text after explicit includes and arch include invocations.
    pub(crate) joined: String,
    /// For each joined line, the zero-based physical recipe line that starts
    /// there. Continuation tails and inserted configuration lines are `None`
    /// and can never own a declaration.
    pub(crate) physical_owner_lines: Vec<Option<usize>>,
}

fn prepare_native_scope(
    snapshot: &str,
    target: &TargetContext,
    dirs: &DirVars,
    root: &Path,
    relative_recipe: &Path,
) -> Result<PreparedNativeScope, String> {
    if relative_recipe.is_absolute()
        || relative_recipe
            .file_name()
            .is_none_or(|name| name != "mmakefile.src")
    {
        return Err("native Make scope needs a source-relative mmakefile.src path".into());
    }
    let canonical_root = root
        .canonicalize()
        .map_err(|error| format!("native source root is unavailable: {error}"))?;
    if !canonical_root.is_dir() {
        return Err("native source root is not a directory".into());
    }
    let scan = crate::local_make_includes::inline_native_make_configuration_with_templates(
        snapshot,
        &canonical_root,
        relative_recipe,
        crate::local_make_includes::LocalMakeIncludeLimits::default(),
        &target.make_include_bindings,
        &target.generated_make_templates,
    );
    if !scan.issues.is_empty() {
        return Err(format!(
            "native Make configuration is incomplete: {}",
            scan.issues
                .iter()
                .take(8)
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }

    let (mut joined, _) = join_continuations_with_origins(&scan.expanded);
    let physical_positions = crate::parser::architecture_scope_positions(
        &canonical_root,
        relative_recipe,
        snapshot,
        &scan,
        &joined,
    )
    .ok_or_else(|| {
        "native Make configuration lost its exact physical source-position mapping".to_owned()
    })?;
    let (original_joined, original_physical_lines) = join_continuations_with_origins(snapshot);
    let original_positions = original_physical_lines
        .iter()
        .map(|physical| physical_positions.get(*physical).copied().flatten())
        .collect::<Vec<_>>();

    let mut physical_owner_lines = vec![None; joined.lines().count()];
    for (physical_line, position) in physical_positions.iter().copied().enumerate() {
        if let Some(position) = position {
            let Some(slot) = physical_owner_lines.get_mut(position) else {
                return Err("native Make source-position mapping is out of range".into());
            };
            if slot.replace(physical_line).is_some() {
                return Err("native Make source-position mapping is ambiguous".into());
            }
        }
    }
    let mut physical_lines = Vec::with_capacity(physical_owner_lines.len());
    let mut latest_source_line = 0usize;
    for owner in &physical_owner_lines {
        if let Some(line) = owner {
            latest_source_line = *line;
        }
        physical_lines.push(latest_source_line);
    }

    let (_, initial_states) = collect_vars_impl(&joined, Some(target));
    let mut define_depth = 0usize;
    let mut unresolved_include = None;
    for (line, raw) in joined.lines().enumerate() {
        let clean = strip_make_comment(raw).trim();
        if starts_make_define(clean) {
            define_depth += 1;
            continue;
        }
        if clean == "endef" {
            define_depth = define_depth.saturating_sub(1);
            continue;
        }
        if define_depth == 0
            && line_state(&initial_states, line) != ConditionalTruth::False
            && is_make_include_line(raw)
        {
            unresolved_include = Some((line, raw));
            break;
        }
    }
    if let Some((line, raw)) = unresolved_include {
        return Err(format!(
            "native Make include at physical source line {} remains unbound: {}",
            physical_line(&physical_lines, line + 1),
            raw.trim()
        ));
    }

    let mut arch_definitions = Vec::new();
    let mut define_depth = 0usize;
    for (original_line, raw) in original_joined.lines().enumerate() {
        let clean = strip_make_comment(raw).trim();
        if starts_make_define(clean) {
            define_depth += 1;
            continue;
        }
        if clean == "endef" {
            define_depth = define_depth.saturating_sub(1);
            continue;
        }
        if define_depth != 0 || raw.starts_with('\t') {
            continue;
        }
        if !is_get_archincludes_line(clean) {
            continue;
        }
        let source_line = original_physical_lines
            .get(original_line)
            .copied()
            .unwrap_or(original_line);
        let Some(scope_line) = original_positions.get(original_line).copied().flatten() else {
            return Err(format!(
                "`%get_archincludes` at physical source line {} has no mapped Make scope position",
                source_line + 1
            ));
        };

        let (_, states) = collect_vars_impl(&joined, Some(target));
        match line_state(&states, scope_line) {
            ConditionalTruth::False => continue,
            ConditionalTruth::Unknown => {
                return Err(format!(
                    "`%get_archincludes` at physical source line {} is inside an unresolved Make conditional",
                    source_line + 1
                ));
            }
            ConditionalTruth::True => {}
        }
        let Some(request) = parse_get_archincludes(clean)? else {
            continue;
        };
        let (scope, _) = collect_vars_impl(&joined, Some(target));
        let request_context = expression_context(
            &scope,
            dirs,
            &canonical_root,
            relative_recipe.parent().unwrap_or_else(|| Path::new("")),
            scope_line,
        );
        let maindir =
            evaluate_make_expr(&format!("$(strip {})", request.maindir), &request_context)
                .map_err(|error| {
                    format!(
                "`%get_archincludes` at physical source line {} has an unresolved maindir: {error}",
                source_line + 1
            )
                })?;
        if !safe_arch_maindir(&maindir) {
            return Err(format!(
                "`%get_archincludes` at physical source line {} has an unsafe maindir `{maindir}`",
                source_line + 1
            ));
        }
        if !target.native_arch_include_errors.is_empty() {
            return Err(format!(
                "active `%get_archincludes modname={}` at physical source line {} has an unproved architecture provider: {}",
                request.modname,
                source_line + 1,
                target.native_arch_include_errors.join("; ")
            ));
        }

        let mut providers = target
            .native_arch_include_effects
            .iter()
            .filter(|effect| effect.applies_to(target))
            .filter_map(|effect| match &effect.data {
                crate::arch_endpoint_effects::ArchEndpointEffectData::SetArchIncludes {
                    modname: provider_modname,
                    maindir: provider_maindir,
                    generated_file,
                    arguments,
                    ..
                } if provider_modname == &request.modname && provider_maindir == &maindir => {
                    Some((generated_file.as_str(), arguments.as_slice()))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        if providers.is_empty() {
            if !target.native_arch_include_catalog_closed {
                return Err(format!(
                    "active `%get_archincludes modname={}` at physical source line {} has no matching source-proved providers in `{maindir}`",
                    request.modname,
                    source_line + 1
                ));
            }
            // Make's wildcard finds no flag file, so the macro adds nothing and
            // the variable expands empty. An empty append expresses that as long
            // as the recipe never asks whether the variable is defined.
            if tests_definition(&original_joined, &request.includeflag) {
                return Err(format!(
                    "`%get_archincludes` at physical source line {} finds no flag file, and {} is tested for definition, which an empty value cannot express",
                    source_line + 1,
                    request.includeflag
                ));
            }
            replace_joined_line(
                &mut joined,
                scope_line,
                &format!("{} +=", request.includeflag),
            )?;
            continue;
        }
        providers.sort_by(|left, right| left.0.cmp(right.0));
        let mut generated_files = BTreeSet::new();
        for (generated_file, _) in &providers {
            if generated_file.is_empty() || !generated_files.insert(*generated_file) {
                return Err(format!(
                    "active `%get_archincludes modname={}` at physical source line {} has duplicate or empty generated flag-file providers",
                    request.modname,
                    source_line + 1
                ));
            }
        }
        let mut arguments = Vec::new();
        for (_, provider_arguments) in providers {
            parse_compiler_arguments(provider_arguments, false, &[])?;
            arch_definitions.extend(architecture_definitions(provider_arguments));
            arguments.extend(provider_arguments.iter().cloned());
        }
        let replacement = format!("{} += {}", request.includeflag, arguments.join(" "));
        replace_joined_line(&mut joined, scope_line, &replacement)?;
    }

    let (scope, line_states) = collect_vars_impl(&joined, Some(target));
    let mut rule_lines = vec![String::new(); joined.lines().count()];
    for (original_line, raw) in original_joined.lines().enumerate() {
        let Some(position) = original_positions.get(original_line).copied().flatten() else {
            continue;
        };
        let Some(slot) = rule_lines.get_mut(position) else {
            return Err("native Make physical rule mapping is out of range".into());
        };
        raw.clone_into(slot);
    }

    Ok(PreparedNativeScope {
        joined,
        rule_view: rule_lines.join("\n"),
        physical_lines,
        physical_owner_lines,
        arch_definitions,
        scope,
        line_states,
    })
}

fn is_make_include_line(raw: &str) -> bool {
    if raw.starts_with(char::is_whitespace) {
        return false;
    }
    let clean = strip_make_comment(raw).trim_end();
    ["include", "-include", "sinclude", "-sinclude"]
        .iter()
        .any(|directive| {
            clean == *directive
                || clean
                    .strip_prefix(directive)
                    .is_some_and(|tail| tail.starts_with(char::is_whitespace))
        })
}

fn is_get_archincludes_line(line: &str) -> bool {
    let Some(tail) = line.strip_prefix("%get_archincludes") else {
        return false;
    };
    tail.is_empty() || tail.starts_with(char::is_whitespace)
}

fn starts_make_define(line: &str) -> bool {
    [
        "define",
        "override define",
        "export define",
        "private define",
    ]
    .iter()
    .any(|prefix| {
        line == *prefix
            || line
                .strip_prefix(prefix)
                .is_some_and(|tail| tail.starts_with(char::is_whitespace))
    })
}

fn parse_get_archincludes(line: &str) -> Result<Option<GetArchIncludesRequest>, String> {
    let Some(tail) = line.strip_prefix("%get_archincludes") else {
        return Ok(None);
    };
    if !tail.is_empty() && !tail.starts_with(char::is_whitespace) {
        return Ok(None);
    }
    let mut modname = None;
    let mut maindir = None;
    let mut includeflag = None;
    for argument in tail.split_whitespace() {
        let Some((name, value)) = argument.split_once('=') else {
            return Err("`%get_archincludes` has a malformed argument".into());
        };
        if value.is_empty() {
            return Err("`%get_archincludes` has an empty argument value".into());
        }
        match name {
            "modname" if modname.is_none() => {
                if !value.chars().all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
                }) {
                    return Err(
                        "`%get_archincludes` modname is outside the finite name vocabulary".into(),
                    );
                }
                modname = Some(value.to_owned());
            }
            "maindir" if maindir.is_none() => maindir = Some(value.to_owned()),
            "includeflag" if includeflag.is_none() => {
                if !value.chars().all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
                }) {
                    return Err(
                        "`%get_archincludes` includeflag is outside the variable-name vocabulary"
                            .into(),
                    );
                }
                includeflag = Some(value.to_owned());
            }
            _ => {
                return Err(format!(
                    "`%get_archincludes` has a duplicate or unsupported `{name}` argument"
                ));
            }
        }
    }
    let modname = modname.ok_or_else(|| "`%get_archincludes` has no literal modname".to_owned())?;
    let maindir =
        maindir.ok_or_else(|| "`%get_archincludes` has no explicit maindir".to_owned())?;
    Ok(Some(GetArchIncludesRequest {
        modname,
        maindir,
        includeflag: includeflag.unwrap_or_else(|| "USER_INCLUDES".to_owned()),
    }))
}

fn safe_arch_maindir(maindir: &str) -> bool {
    !maindir.is_empty()
        && !Path::new(maindir).is_absolute()
        && Path::new(maindir)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
        && !maindir.contains(['$', '\\'])
}

/// Whether a recipe distinguishes an undefined variable from an empty one.
fn tests_definition(source: &str, name: &str) -> bool {
    source.lines().any(|line| {
        let line = strip_make_comment(line).trim();
        ["ifdef", "ifndef", "else ifdef", "else ifndef"]
            .iter()
            .any(|directive| {
                line.strip_prefix(directive)
                    .is_some_and(|rest| rest.split_whitespace().next() == Some(name))
            })
            || line.contains(&format!("$(origin {name})"))
            || line.contains(&format!("$(flavor {name})"))
    })
}

fn replace_joined_line(
    snapshot: &mut String,
    line: usize,
    replacement: &str,
) -> Result<(), String> {
    let mut lines = snapshot.split('\n').map(str::to_owned).collect::<Vec<_>>();
    let Some(slot) = lines.get_mut(line) else {
        return Err("native Make macro position is out of range".into());
    };
    replacement.clone_into(slot);
    *snapshot = lines.join("\n");
    Ok(())
}

fn architecture_definitions(arguments: &[String]) -> Vec<String> {
    let mut definitions = Vec::new();
    let mut index = 0;
    while index < arguments.len() {
        let argument = &arguments[index];
        if argument == "-D" {
            if let Some(value) = arguments.get(index + 1) {
                definitions.push(format!("-D{value}"));
                index += 2;
                continue;
            }
        } else if argument.starts_with("-D") && argument.len() > 2 {
            definitions.push(argument.clone());
        }
        index += 1;
    }
    definitions
}

struct CandidateContext<'a> {
    scope: &'a VarScope,
    line_states: &'a [ConditionalTruth],
    arch_definitions: &'a [String],
    dirs: &'a DirVars,
    root: &'a Path,
    relative_dir: &'a Path,
    file: &'a str,
}

#[derive(Debug)]
struct CandidateError {
    owner: String,
    line: usize,
    reason: String,
}

/// Collects the bounded assembly-header pipeline from a physical mmakefile snapshot.
///
/// Make continuations are joined internally while original line anchors are
/// retained. Make variables and conditional states are
/// reconstructed from this snapshot plus `target`; reached Make expressions
/// run with filesystem enumeration disabled. Source file reads only verify
/// that the mmakefile and its C input are regular files below `root`.
#[must_use]
pub fn collect_from_snapshot(
    snapshot: &str,
    target: &TargetContext,
    dirs: &DirVars,
    root: &Path,
    relative_dir: &Path,
) -> (Vec<AssemblyHeaderDecl>, Vec<AssemblyHeaderRejection>) {
    let file_hint = relative_mmakefile(relative_dir);
    if snapshot.len() > MAX_SNAPSHOT_BYTES {
        return (
            Vec::new(),
            with_file(
                vec![rejection(
                    "<unknown>",
                    1,
                    "assembly-header mmakefile exceeds the 2 MiB scan limit",
                )],
                &file_hint,
            ),
        );
    }

    let (source_joined, source_physical_lines) = join_continuations_with_origins(snapshot);
    let (source_scope, source_states) = collect_vars_impl(&source_joined, Some(target));
    let source_parsed = parse_snapshot(&source_joined, &source_states);
    let source_candidate_indices = source_parsed
        .rules
        .iter()
        .enumerate()
        .filter_map(|(index, rule)| {
            looks_like_header_pipeline(rule, &source_scope, dirs, root, relative_dir)
                .then_some(index)
        })
        .collect::<Vec<_>>();
    if source_candidate_indices.is_empty() {
        return (Vec::new(), Vec::new());
    }

    let file = match safe_relative_file(root, relative_dir, "mmakefile.src") {
        Ok(path) => path,
        Err(reason) => {
            return (
                Vec::new(),
                finalize_rejections(
                    source_candidate_indices
                        .iter()
                        .map(|index| {
                            rejection("<unknown>", source_parsed.rules[*index].line + 1, &reason)
                        })
                        .take(MAX_REJECTIONS)
                        .collect(),
                    &file_hint,
                    &source_physical_lines,
                ),
            );
        }
    };
    let canonical_root = match root.canonicalize() {
        Ok(path) if path.is_dir() => path,
        _ => {
            return (
                Vec::new(),
                finalize_rejections(
                    vec![rejection(
                        "<unknown>",
                        source_parsed.rules[source_candidate_indices[0]].line + 1,
                        "selected source root is unavailable or not a directory",
                    )],
                    &file_hint,
                    &source_physical_lines,
                ),
            );
        }
    };
    let observed_snapshot = match read_bounded_snapshot(&file) {
        Ok(text) => text,
        Err(reason) => {
            return (
                Vec::new(),
                finalize_rejections(
                    vec![rejection(
                        "<unknown>",
                        source_parsed.rules[source_candidate_indices[0]].line + 1,
                        &reason,
                    )],
                    &file_hint,
                    &source_physical_lines,
                ),
            );
        }
    };
    if observed_snapshot != snapshot {
        return (
            Vec::new(),
            finalize_rejections(
                vec![rejection(
                    "<unknown>",
                    source_parsed.rules[source_candidate_indices[0]].line + 1,
                    "provided physical snapshot differs from the current source mmakefile",
                )],
                &file_hint,
                &source_physical_lines,
            ),
        );
    }
    let file = match file.strip_prefix(&canonical_root) {
        Ok(path) => path.to_string_lossy().replace('\\', "/"),
        Err(_) => {
            return (
                Vec::new(),
                finalize_rejections(
                    vec![rejection(
                        "<unknown>",
                        source_parsed.rules[source_candidate_indices[0]].line + 1,
                        "physical mmakefile is outside the selected source root",
                    )],
                    &file_hint,
                    &source_physical_lines,
                ),
            );
        }
    };

    let relative_recipe = relative_dir.join("mmakefile.src");
    let prepared =
        match prepare_native_scope(snapshot, target, dirs, &canonical_root, &relative_recipe) {
            Ok(prepared) => prepared,
            Err(reason) => {
                return (
                    Vec::new(),
                    finalize_rejections(
                        source_candidate_indices
                            .iter()
                            .map(|index| {
                                rejection(
                                    "<unknown>",
                                    source_parsed.rules[*index].line + 1,
                                    &reason,
                                )
                            })
                            .take(MAX_REJECTIONS)
                            .collect(),
                        &file,
                        &source_physical_lines,
                    ),
                );
            }
        };
    let parsed = parse_snapshot(&prepared.rule_view, &prepared.line_states);
    let candidate_indices = parsed
        .rules
        .iter()
        .enumerate()
        .filter_map(|(index, rule)| {
            looks_like_header_pipeline(rule, &prepared.scope, dirs, &canonical_root, relative_dir)
                .then_some(index)
        })
        .collect::<Vec<_>>();
    if candidate_indices.is_empty() {
        return (Vec::new(), Vec::new());
    }

    if let Some((line, reason)) = role_mutation_issue(&prepared.joined, &prepared.line_states) {
        return (
            Vec::new(),
            finalize_rejections(
                candidate_indices
                    .iter()
                    .map(|index| {
                        rejection(
                            "<unknown>",
                            parsed.rules[*index].line + 1,
                            &format!(
                                "{reason} (source line {})",
                                physical_line(&prepared.physical_lines, line + 1)
                            ),
                        )
                    })
                    .collect(),
                &file,
                &prepared.physical_lines,
            ),
        );
    }

    let mut declarations = Vec::new();
    let mut rejections = Vec::new();
    let context = CandidateContext {
        scope: &prepared.scope,
        line_states: &prepared.line_states,
        arch_definitions: &prepared.arch_definitions,
        dirs,
        root: &canonical_root,
        relative_dir,
        file: &file,
    };
    for index in candidate_indices {
        match collect_one(&parsed, index, &context) {
            Ok(declaration) => declarations.push(declaration),
            Err(error) => rejections.push(rejection(&error.owner, error.line, &error.reason)),
        }
        if rejections.len() >= MAX_REJECTIONS {
            break;
        }
    }

    declarations.sort_by(|left, right| {
        (&left.owner, &left.header_output, left.line).cmp(&(
            &right.owner,
            &right.header_output,
            right.line,
        ))
    });
    for declaration in &mut declarations {
        declaration.line = physical_line(&prepared.physical_lines, declaration.line);
    }

    let mut claimed_outputs = BTreeMap::<String, usize>::new();
    for declaration in &declarations {
        *claimed_outputs
            .entry(declaration.header_output.clone())
            .or_default() += 1;
    }
    if claimed_outputs.values().any(|count| *count > 1) {
        let duplicate_outputs = claimed_outputs
            .into_iter()
            .filter_map(|(output, count)| (count > 1).then_some(output))
            .collect::<BTreeSet<_>>();
        declarations.retain(|declaration| !duplicate_outputs.contains(&declaration.header_output));
        rejections.extend(duplicate_outputs.into_iter().map(|output| {
            rejection(
                "<unknown>",
                1,
                &format!("multiple assembly-header declarations claim `{output}`"),
            )
        }));
    }
    rejections.sort_by(|left, right| {
        (&left.owner, left.line, &left.reason).cmp(&(&right.owner, right.line, &right.reason))
    });
    rejections.dedup();
    (
        declarations,
        finalize_rejections(rejections, &file, &prepared.physical_lines),
    )
}

fn collect_one(
    parsed: &SnapshotRules,
    header_index: usize,
    context: &CandidateContext<'_>,
) -> Result<AssemblyHeaderDecl, CandidateError> {
    let CandidateContext {
        scope,
        line_states,
        arch_definitions,
        dirs,
        root: canonical_root,
        relative_dir,
        file,
    } = context;
    let header_rule = &parsed.rules[header_index];
    let fallback_line = header_rule.line + 1;
    require_active_rule(header_rule, "<unknown>", "header", line_states)?;
    let header_path = eval_single_path(
        &header_rule.target,
        scope,
        dirs,
        canonical_root,
        relative_dir,
        header_rule.line,
    )
    .map_err(|reason| candidate_error("<unknown>", fallback_line, reason))?;
    if !is_build_output(&header_path, ".h") {
        return Err(candidate_error(
            "<unknown>",
            fallback_line,
            format!(
                "header rule target `{header_path}` is not a `.h` output below the configured build root"
            ),
        ));
    }
    let header_root = eval_single_path(
        "$(GENINCDIR)",
        scope,
        dirs,
        canonical_root,
        relative_dir,
        header_rule.line,
    )
    .map_err(|reason| candidate_error("<unknown>", fallback_line, reason))?;
    if !is_build_directory(&header_root)
        || !header_path
            .strip_prefix(&format!("{header_root}/"))
            .is_some_and(valid_relative_tail)
    {
        return Err(candidate_error(
            "<unknown>",
            fallback_line,
            "header output is not below the source-configured GENINCDIR build root",
        ));
    }
    reject_duplicate_rule_target(parsed, &header_path, header_index, context, fallback_line)?;

    let prerequisites = eval_words(
        &header_rule.prerequisites,
        scope,
        dirs,
        canonical_root,
        relative_dir,
        header_rule.line,
    )
    .map_err(|reason| candidate_error("<unknown>", fallback_line, reason))?;
    let [assembly_path] = prerequisites.as_slice() else {
        return Err(candidate_error(
            "<unknown>",
            fallback_line,
            "header rule must have exactly one normal prerequisite",
        ));
    };
    if !is_generated_output(assembly_path, ".s") {
        return Err(candidate_error(
            "<unknown>",
            fallback_line,
            "header rule prerequisite is not a `.s` output below the configured generated root",
        ));
    }
    require_one_build_directory(
        header_rule,
        scope,
        dirs,
        canonical_root,
        relative_dir,
        fallback_line,
    )?;
    require_active_recipes(header_rule, fallback_line, "header")?;
    if header_rule.inline_recipe.is_some() {
        return Err(candidate_error(
            "<unknown>",
            fallback_line,
            "inline recipes are outside the assembly-header grammar",
        ));
    }
    let header_recipes = active_recipe_texts(header_rule, fallback_line, "header")?;
    if header_recipes.as_slice()
        != [
            "@$(ECHO) Generating $@...",
            "@grep $(GREPTOKEN) $< | cut -d'\"' -f2 | sed 's/\\$$//g' >$@",
        ]
    {
        return Err(candidate_error(
            "<unknown>",
            fallback_line,
            "header recipe differs from the closed grep/cut/sed pipeline",
        ));
    }

    let providers = find_providers(
        parsed,
        &header_path,
        scope,
        dirs,
        canonical_root,
        relative_dir,
        fallback_line,
    )?;
    let [(owner, provider_index)] = providers.as_slice() else {
        return Err(candidate_error(
            "<unknown>",
            fallback_line,
            "generated header must have exactly one bare-`#MM` provider target",
        ));
    };
    let provider = &parsed.rules[*provider_index];
    require_active_rule(provider, owner.as_str(), "provider", line_states)?;
    if provider.inline_recipe.is_some()
        || !active_recipe_texts(provider, provider.line + 1, "provider")?.is_empty()
    {
        return Err(candidate_error(
            owner,
            provider.line + 1,
            "provider target must be a recipe-free Make alias",
        ));
    }
    let provider_prerequisites = eval_words(
        &provider.prerequisites,
        scope,
        dirs,
        canonical_root,
        relative_dir,
        provider.line,
    )
    .map_err(|reason| candidate_error(owner, provider.line + 1, reason))?;
    if provider_prerequisites != [header_path.clone()] || provider.order_only.is_some() {
        return Err(candidate_error(
            owner,
            provider.line + 1,
            "provider must depend on only the generated header",
        ));
    }

    let (aggregate_owner, aggregate_dependencies, aggregate_line, aggregate_rule_index) =
        find_aggregate(
            parsed,
            owner,
            provider.line + 1,
            scope,
            dirs,
            canonical_root,
            relative_dir,
        )?;
    let aggregate_rule = &parsed.rules[aggregate_rule_index];
    require_active_rule(aggregate_rule, &aggregate_owner, "aggregate", line_states)?;
    if aggregate_rule.inline_recipe.is_some()
        || !aggregate_rule.prerequisites.trim().is_empty()
        || aggregate_rule
            .order_only
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
        || active_recipe_texts(aggregate_rule, aggregate_rule.line + 1, "aggregate")? != ["@$(NOP)"]
    {
        return Err(candidate_error(
            &aggregate_owner,
            aggregate_line,
            "aggregate target must be a no-prerequisite `@$(NOP)` rule",
        ));
    }

    let assembly_rules = matching_rules(
        parsed,
        assembly_path,
        scope,
        dirs,
        canonical_root,
        relative_dir,
    );
    let [assembly_rule_index] = assembly_rules.as_slice() else {
        return Err(candidate_error(
            owner,
            provider.line + 1,
            "assembly output must have exactly one source-local compile rule",
        ));
    };
    let assembly_rule = &parsed.rules[*assembly_rule_index];
    require_active_rule(assembly_rule, owner, "assembly compile", line_states)?;
    if assembly_rule.inline_recipe.is_some() {
        return Err(candidate_error(
            owner,
            assembly_rule.line + 1,
            "inline compile recipes are outside the assembly-header grammar",
        ));
    }
    require_one_build_directory(
        assembly_rule,
        scope,
        dirs,
        canonical_root,
        relative_dir,
        assembly_rule.line + 1,
    )
    .map_err(|mut error| {
        error.owner.clone_from(owner);
        error
    })?;
    let assembly_recipes =
        active_recipe_texts(assembly_rule, assembly_rule.line + 1, "assembly compile")?;
    let compile_echo_matches = matches!(
        assembly_recipes.first().copied(),
        Some("@$(ECHO) \"Compiling $<...\"" | "@$(ECHO) \"Compiling  $<...\"")
    );
    if assembly_recipes.len() != 2
        || !compile_echo_matches
        || assembly_recipes.get(1)
            != Some(&"@$(TARGET_CC) $(TARGET_SYSROOT) $(CFLAGS) $(PRIV_EXEC_INCLUDES) -S $< -o $@")
    {
        return Err(candidate_error(
            owner,
            assembly_rule.line + 1,
            "assembly recipe differs from the closed target-C compile-to-assembly command",
        ));
    }

    let compile_prerequisites = eval_words(
        &assembly_rule.prerequisites,
        scope,
        dirs,
        canonical_root,
        relative_dir,
        assembly_rule.line,
    )
    .map_err(|reason| candidate_error(owner, assembly_rule.line + 1, reason))?;
    let [source] = compile_prerequisites.as_slice() else {
        return Err(candidate_error(
            owner,
            assembly_rule.line + 1,
            "assembly compile rule must have exactly one normal prerequisite",
        ));
    };
    let source_relative = source_relative_path(source).ok_or_else(|| {
        candidate_error(
            owner,
            assembly_rule.line + 1,
            "assembly compiler input is not a source-root-relative path",
        )
    })?;
    if !has_extension(&source_relative, "c") {
        return Err(candidate_error(
            owner,
            assembly_rule.line + 1,
            "assembly compile input is not a C source file",
        ));
    }
    safe_relative_file(canonical_root, Path::new(&source_relative), "")
        .map_err(|reason| candidate_error(owner, assembly_rule.line + 1, reason))?;

    let arguments = compiler_arguments(scope, arch_definitions, dirs, canonical_root, relative_dir)
        .map_err(|reason| candidate_error(owner, assembly_rule.line + 1, reason))?;
    let token = evaluate_make_expr(
        "$(GREPTOKEN)",
        &expression_context(scope, dirs, canonical_root, relative_dir, usize::MAX),
    )
    .map_err(|error| {
        candidate_error(
            owner,
            fallback_line,
            format!("cannot resolve GREPTOKEN: {error}"),
        )
    })?;
    if let Some(reason) = scope.flavor_uncertainty_reason_at("GREPTOKEN", usize::MAX) {
        return Err(candidate_error(
            owner,
            fallback_line,
            format!("cannot safely resolve GREPTOKEN: {reason}"),
        ));
    }
    let token = unquote_grep_token(&token).ok_or_else(|| {
        candidate_error(
            owner,
            fallback_line,
            "GREPTOKEN must resolve to exactly `.asciz` or `.ascii`",
        )
    })?;

    if !safe_owner_name(owner) || !safe_owner_name(&aggregate_owner) {
        return Err(candidate_error(
            owner,
            provider.line + 1,
            "provider or aggregate owner is not a finite Make target name",
        ));
    }
    if aggregate_dependencies.iter().collect::<BTreeSet<_>>().len() != aggregate_dependencies.len()
    {
        return Err(candidate_error(
            owner,
            aggregate_line,
            "aggregate declaration contains duplicate prerequisites",
        ));
    }

    Ok(AssemblyHeaderDecl {
        owner: owner.clone(),
        aggregate_owner,
        aggregate_dependencies,
        file: (*file).to_owned(),
        line: provider.line + 1,
        source: source.clone(),
        assembly_output: assembly_path.clone(),
        header_output: header_path,
        header_root,
        arguments,
        token,
    })
}

fn find_providers(
    parsed: &SnapshotRules,
    header_path: &str,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    relative_dir: &Path,
    line: usize,
) -> Result<Vec<(String, usize)>, CandidateError> {
    let mut matches = Vec::new();
    for (index, rule) in parsed.rules.iter().enumerate() {
        if rule.state == ConditionalTruth::False || rule.bare_meta_line.is_none() {
            continue;
        }
        let target = eval_single_path(&rule.target, scope, dirs, root, relative_dir, rule.line);
        let prerequisites = eval_words(
            &rule.prerequisites,
            scope,
            dirs,
            root,
            relative_dir,
            rule.line,
        );
        if let (Ok(owner), Ok(values)) = (target, prerequisites) {
            if values == [header_path.to_owned()] {
                matches.push((owner, index));
            }
        }
    }
    if matches.len() > 1 {
        return Err(candidate_error(
            "<unknown>",
            line,
            "multiple bare-`#MM` provider targets claim the generated header",
        ));
    }
    Ok(matches)
}

fn find_aggregate(
    parsed: &SnapshotRules,
    provider_owner: &str,
    line: usize,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    relative_dir: &Path,
) -> Result<(String, Vec<String>, usize, usize), CandidateError> {
    let mut matches = Vec::new();
    for meta in &parsed.meta_rules {
        if meta.state == ConditionalTruth::False {
            continue;
        }
        let context = expression_context(scope, dirs, root, relative_dir, meta.line);
        let owner = evaluate_make_expr(&meta.target, &context);
        let dependencies = evaluate_make_list(&meta.prerequisites, &context);
        match (owner, dependencies) {
            (Ok(owner), Ok(dependencies))
                if dependencies.iter().any(|dep| dep == provider_owner) =>
            {
                if meta.state == ConditionalTruth::Unknown {
                    return Err(candidate_error(
                        provider_owner,
                        meta.line + 1,
                        "aggregate declaration is inside an unresolved Make conditional",
                    ));
                }
                matches.push((owner, dependencies, meta.line));
            }
            _ => {}
        }
    }
    if matches.len() != 1 {
        return Err(candidate_error(
            provider_owner,
            line,
            if matches.is_empty() {
                "provider has no finite `#MM` aggregate declaration"
            } else {
                "provider appears in multiple `#MM` aggregate declarations"
            },
        ));
    }
    let (aggregate_owner, dependencies, aggregate_line) = matches.pop().expect("one match");
    if !safe_owner_name(&aggregate_owner)
        || dependencies.is_empty()
        || dependencies
            .iter()
            .any(|dependency| !safe_owner_name(dependency))
    {
        return Err(candidate_error(
            provider_owner,
            aggregate_line + 1,
            "aggregate declaration is not a finite list of Make target names",
        ));
    }
    if aggregate_owner == provider_owner {
        return Err(candidate_error(
            provider_owner,
            aggregate_line + 1,
            "aggregate owner cannot be its own provider",
        ));
    }

    let mut aggregate_rules = Vec::new();
    for (index, rule) in parsed.rules.iter().enumerate() {
        if rule.state == ConditionalTruth::False || rule.bare_meta_line.is_none() {
            continue;
        }
        if eval_single_path(&rule.target, scope, dirs, root, relative_dir, rule.line)
            .is_ok_and(|name| name == aggregate_owner)
        {
            aggregate_rules.push(index);
        }
    }
    let [aggregate_rule_index] = aggregate_rules.as_slice() else {
        return Err(candidate_error(
            &aggregate_owner,
            aggregate_line + 1,
            "aggregate declaration must have exactly one matching bare-`#MM` Make rule",
        ));
    };
    Ok((
        aggregate_owner,
        dependencies,
        aggregate_line + 1,
        *aggregate_rule_index,
    ))
}

fn compiler_arguments(
    scope: &VarScope,
    arch_definitions: &[String],
    dirs: &DirVars,
    source_root: &Path,
    relative_dir: &Path,
) -> Result<Vec<String>, String> {
    for name in [
        "TARGET_CC",
        "TARGET_SYSROOT",
        "CFLAGS",
        "PRIV_EXEC_INCLUDES",
    ] {
        if let Some(reason) = scope.flavor_uncertainty_reason_at(name, usize::MAX) {
            return Err(format!("cannot safely resolve `{name}`: {reason}"));
        }
    }
    let lookup = |name: &str| {
        if scope
            .flavor_uncertainty_reason_at(name, usize::MAX)
            .is_some()
        {
            return None;
        }
        if matches!(
            name,
            "AROS_SOURCE_DIR" | "AROS_BUILD_DIR" | "AROS_PORTS_DIR" | "AROS_PORTS_SOURCE_DIR"
        ) && scope.raw_at(name, usize::MAX).is_none()
        {
            return Some(format!("$${{{name}}}"));
        }
        let value = scope.raw_at(name, usize::MAX)?;
        Some(escape_cmake_references_for_make(&value))
    };
    let guard = |name: &str| scope.flavor_uncertainty_reason_at(name, usize::MAX);
    let context = MakeExprContext::new(scope, dirs, usize::MAX, source_root, relative_dir)
        .with_lookup(&lookup)
        .with_guard(&guard)
        .without_filesystem();
    let admitted_compiler = dirs
        .expand("$(NATIVE_TARGET_CC)")
        .ok_or_else(|| "native target C compiler role was not explicitly admitted".to_owned())?;
    if admitted_compiler != "${CMAKE_C_COMPILER}" {
        return Err("native target compiler role is not `${CMAKE_C_COMPILER}`".into());
    }
    let target_compiler = scope
        .raw_at("TARGET_CC", usize::MAX)
        .ok_or_else(|| "cannot resolve TARGET_CC from the explicit Make context".to_owned())?;
    if target_compiler.trim() != "$(NATIVE_TARGET_CC)"
        || scope.raw_at("NATIVE_TARGET_CC", usize::MAX).is_some()
    {
        return Err("TARGET_CC does not resolve to the admitted native target compiler".into());
    }

    let sysroot = evaluate_make_list("$(strip $(TARGET_SYSROOT))", &context)
        .map_err(|error| format!("cannot resolve TARGET_SYSROOT: {error}"))?;
    let mut arguments = Vec::new();
    match sysroot.as_slice() {
        [] => {}
        [value] if value.starts_with("--sysroot=") => {
            let configured = evaluate_make_expr("$(strip $(AROS_DEVELOPER))", &context)
                .map_err(|error| format!("cannot resolve configured Developer root: {error}"))?;
            let expected = format!("--sysroot={configured}");
            if value != &expected || !is_build_path(&configured) {
                return Err(
                    "TARGET_SYSROOT is outside the configured build-tree Developer root".into(),
                );
            }
            arguments.push(value.clone());
        }
        _ => {
            return Err(
                "TARGET_SYSROOT must be empty or one configured `--sysroot` argument".into(),
            );
        }
    }

    let cflags = evaluate_make_list("$(strip $(CFLAGS))", &context)
        .map_err(|error| format!("cannot resolve CFLAGS: {error}"))?;
    let includes = evaluate_make_list("$(strip $(PRIV_EXEC_INCLUDES))", &context)
        .map_err(|error| format!("cannot resolve PRIV_EXEC_INCLUDES: {error}"))?;
    arguments.extend(parse_compiler_arguments(&cflags, false, &[])?);
    arguments.extend(parse_compiler_arguments(&includes, true, arch_definitions)?);
    Ok(arguments)
}

fn escape_cmake_references_for_make(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut escaped = String::with_capacity(value.len());
    let mut cursor = 0;
    while cursor < bytes.len() {
        if bytes[cursor] == b'$'
            && bytes.get(cursor + 1) == Some(&b'{')
            && (cursor == 0 || bytes[cursor - 1] != b'$')
        {
            escaped.push_str("$$");
            escaped.push('{');
            cursor += 2;
        } else {
            let character = value[cursor..]
                .chars()
                .next()
                .expect("valid UTF-8 boundary");
            escaped.push(character);
            cursor += character.len_utf8();
        }
    }
    escaped
}

fn parse_compiler_arguments(
    words: &[String],
    includes_only: bool,
    architecture_definitions: &[String],
) -> Result<Vec<String>, String> {
    if words.len() > 512 {
        return Err("compiler argument list exceeds the 512-word limit".into());
    }
    let mut remaining_architecture_definitions = BTreeMap::<String, usize>::new();
    for definition in architecture_definitions {
        *remaining_architecture_definitions
            .entry(definition.clone())
            .or_default() += 1;
    }
    let mut result = Vec::with_capacity(words.len());
    let mut index = 0;
    while index < words.len() {
        let word = &words[index];
        if let Some(kind) = include_option(word) {
            let (option, value, consumed) = if kind.attached {
                let value = word
                    .strip_prefix(kind.name)
                    .ok_or_else(|| "malformed include option".to_owned())?;
                (kind.name.to_owned(), value.to_owned(), 1)
            } else {
                let value = words
                    .get(index + 1)
                    .ok_or_else(|| format!("include option `{word}` has no path"))?
                    .clone();
                (word.clone(), value, 2)
            };
            validate_include_path(&value)?;
            result.push(option);
            result.push(value);
            index += consumed;
            continue;
        }
        if word == "-include" || word == "-imacros" {
            let value = words
                .get(index + 1)
                .ok_or_else(|| format!("include option `{word}` has no path"))?
                .clone();
            validate_include_path(&value)?;
            result.push(word.clone());
            result.push(value);
            index += 2;
            continue;
        }
        if word.starts_with("-include=") || word.starts_with("-imacros=") {
            let (_, value) = word.split_once('=').expect("prefix contains equals");
            validate_include_path(value)?;
            result.push(word.clone());
            index += 1;
            continue;
        }
        if word == "-D" {
            let value = words
                .get(index + 1)
                .ok_or_else(|| "macro definition option `-D` has no name".to_owned())?;
            let canonical = format!("-D{value}");
            validate_macro_definition(&canonical)?;
            if includes_only {
                consume_architecture_definition(
                    &canonical,
                    &mut remaining_architecture_definitions,
                )?;
            }
            result.push(word.clone());
            result.push(value.clone());
            index += 2;
            continue;
        }
        if let Some(name) = word.strip_prefix("-D").filter(|name| !name.is_empty()) {
            validate_macro_definition(&format!("-D{name}"))?;
            if includes_only {
                consume_architecture_definition(word, &mut remaining_architecture_definitions)?;
            }
            result.push(word.clone());
            index += 1;
            continue;
        }
        if includes_only {
            return Err(format!(
                "PRIV_EXEC_INCLUDES contains non-include argument `{word}`"
            ));
        }
        validate_compiler_flag(word)?;
        result.push(word.clone());
        index += 1;
    }
    Ok(result)
}

fn validate_macro_definition(argument: &str) -> Result<(), String> {
    let Some(definition) = argument.strip_prefix("-D") else {
        return Err(format!("compiler definition `{argument}` is malformed"));
    };
    let name = definition
        .split_once('=')
        .map_or(definition, |(name, _)| name);
    if name.is_empty()
        || !name.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_alphabetic() || byte == b'_' || (index > 0 && byte.is_ascii_digit())
        })
        || !name
            .as_bytes()
            .first()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
        || !safe_argument_text(argument)
    {
        return Err(format!(
            "compiler definition `{argument}` is outside the safe macro vocabulary"
        ));
    }
    validate_compiler_flag(argument)
}

fn consume_architecture_definition(
    definition: &str,
    remaining: &mut BTreeMap<String, usize>,
) -> Result<(), String> {
    let Some(count) = remaining.get_mut(definition) else {
        return Err(format!(
            "PRIV_EXEC_INCLUDES contains non-include argument `{definition}` without a matching architecture metadata definition"
        ));
    };
    *count -= 1;
    if *count == 0 {
        remaining.remove(definition);
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct IncludeOption {
    name: &'static str,
    attached: bool,
}

fn include_option(word: &str) -> Option<IncludeOption> {
    for name in ["-isystem", "-iquote", "-idirafter"] {
        if word == name {
            return Some(IncludeOption {
                name,
                attached: false,
            });
        }
        if word.starts_with(name) && word.len() > name.len() {
            return Some(IncludeOption {
                name,
                attached: true,
            });
        }
    }
    if word == "-I" {
        return Some(IncludeOption {
            name: "-I",
            attached: false,
        });
    }
    if word.starts_with("-I") && word.len() > 2 {
        return Some(IncludeOption {
            name: "-I",
            attached: true,
        });
    }
    None
}

fn validate_compiler_flag(word: &str) -> Result<(), String> {
    if !word.starts_with('-')
        || word == "-c"
        || word == "-S"
        || word.starts_with("-o")
        || word == "-E"
        || word == "--"
        || word == "-fsyntax-only"
        || word.starts_with("-specs")
        || word.starts_with("--specs")
        || word.starts_with("-fplugin")
        || word.starts_with("-plugin")
        || word.starts_with("-B")
        || word.starts_with("-X")
        || word.starts_with("-Wl,")
        || word.starts_with("-Wa,")
        || word.starts_with("-Wp,")
        || word.starts_with("-MF")
        || word.starts_with("-MT")
        || word.starts_with("-MQ")
        || word.starts_with("-MJ")
        || word == "-MD"
        || word == "-MMD"
        || word == "-MP"
        || word.starts_with("-dumpbase")
        || word.starts_with("-save-temps")
        || word.starts_with("--output")
    {
        return Err(format!(
            "compiler flag `{word}` is outside the guarded compile argv"
        ));
    }
    if !safe_argument_text(word) {
        return Err(format!(
            "compiler flag `{word}` contains unsafe shell or path syntax"
        ));
    }
    Ok(())
}

fn safe_argument_text(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut cursor = 0;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'$' => {
                if bytes.get(cursor + 1) != Some(&b'{') {
                    return false;
                }
                let Some(end) = text[cursor + 2..].find('}') else {
                    return false;
                };
                let end = cursor + 2 + end;
                let name = &text[cursor + 2..end];
                if !matches!(
                    name,
                    "AROS_SOURCE_DIR"
                        | "AROS_BUILD_DIR"
                        | "AROS_PORTS_DIR"
                        | "AROS_PORTS_SOURCE_DIR"
                ) {
                    return false;
                }
                cursor = end + 1;
            }
            byte if byte.is_ascii_alphanumeric() || b"_-.+=,:/@%".contains(&byte) => cursor += 1,
            _ => return false,
        }
    }
    !text.is_empty()
}

fn validate_include_path(value: &str) -> Result<(), String> {
    if ![
        SOURCE_ALIAS,
        BUILD_ALIAS,
        "${AROS_PORTS_DIR}",
        "${AROS_PORTS_SOURCE_DIR}",
    ]
    .iter()
    .any(|root| value.strip_prefix(&format!("{root}/")).is_some())
    {
        return Err(format!(
            "include path `{value}` is not contained by a configured source or build root"
        ));
    }
    if !safe_argument_text(value) || has_parent_components(value) {
        return Err(format!(
            "include path `{value}` escapes or cannot be represented safely"
        ));
    }
    Ok(())
}

fn has_parent_components(value: &str) -> bool {
    value
        .split('/')
        .any(|component| component == ".." || component.is_empty())
}

fn unquote_grep_token(value: &str) -> Option<String> {
    let token = match value {
        "\".asciz\"" | ".asciz" => ".asciz",
        "\".ascii\"" | ".ascii" => ".ascii",
        _ => return None,
    };
    Some(token.to_owned())
}

fn looks_like_header_pipeline(
    rule: &Rule,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    relative_dir: &Path,
) -> bool {
    let has_extraction_signal = rule
        .recipes
        .iter()
        .map(|recipe| recipe.text.as_str())
        .chain(rule.inline_recipe.iter().map(String::as_str))
        .any(|recipe| {
            recipe.contains("$(GREPTOKEN)")
                || recipe.contains(".asciz")
                || recipe.contains(".ascii")
        });
    if has_extraction_signal {
        return true;
    }
    let target_looks_header = has_extension(&rule.target, "h")
        || eval_single_path(&rule.target, scope, dirs, root, relative_dir, rule.line)
            .is_ok_and(|path| has_extension(&path, "h"));
    let prereq_looks_assembly = eval_words(
        &rule.prerequisites,
        scope,
        dirs,
        root,
        relative_dir,
        rule.line,
    )
    .is_ok_and(|words| words.iter().any(|word| has_extension(word, "s")));
    target_looks_header && prereq_looks_assembly
}

fn reject_duplicate_rule_target(
    parsed: &SnapshotRules,
    output: &str,
    selected: usize,
    context: &CandidateContext<'_>,
    line: usize,
) -> Result<(), CandidateError> {
    let matches = matching_rules(
        parsed,
        output,
        context.scope,
        context.dirs,
        context.root,
        context.relative_dir,
    );
    if matches.len() != 1 || matches[0] != selected {
        return Err(candidate_error(
            "<unknown>",
            line,
            "multiple Make rules claim the generated header output",
        ));
    }
    Ok(())
}

fn matching_rules(
    parsed: &SnapshotRules,
    output: &str,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    relative_dir: &Path,
) -> Vec<usize> {
    parsed
        .rules
        .iter()
        .enumerate()
        .filter_map(|(index, rule)| {
            (rule.state != ConditionalTruth::False
                && eval_single_path(&rule.target, scope, dirs, root, relative_dir, rule.line)
                    .is_ok_and(|target| target == output))
            .then_some(index)
        })
        .collect()
}

fn require_one_build_directory(
    rule: &Rule,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    relative_dir: &Path,
    line: usize,
) -> Result<(), CandidateError> {
    let Some(raw) = rule.order_only.as_deref() else {
        return Err(candidate_error(
            "<unknown>",
            line,
            "producer must declare one order-only build directory",
        ));
    };
    let words = eval_words(raw, scope, dirs, root, relative_dir, rule.line)
        .map_err(|reason| candidate_error("<unknown>", line, reason))?;
    if words.len() != 1 || !is_build_directory(&words[0]) {
        return Err(candidate_error(
            "<unknown>",
            line,
            "producer order-only prerequisite must be one directory below the build root",
        ));
    }
    Ok(())
}

fn require_active_rule(
    rule: &Rule,
    owner: &str,
    description: &str,
    line_states: &[ConditionalTruth],
) -> Result<(), CandidateError> {
    if rule.state != ConditionalTruth::True
        || rule
            .bare_meta_line
            .is_some_and(|line| line_state(line_states, line) != ConditionalTruth::True)
    {
        return Err(candidate_error(
            owner,
            rule.line + 1,
            format!("{description} rule is inside an unresolved Make conditional"),
        ));
    }
    Ok(())
}

fn require_active_recipes(
    rule: &Rule,
    line: usize,
    description: &str,
) -> Result<(), CandidateError> {
    if rule
        .recipes
        .iter()
        .any(|recipe| recipe.state == ConditionalTruth::Unknown)
    {
        return Err(candidate_error(
            "<unknown>",
            line,
            format!("{description} recipe is inside an unresolved Make conditional"),
        ));
    }
    Ok(())
}

fn active_recipe_texts<'a>(
    rule: &'a Rule,
    line: usize,
    description: &str,
) -> Result<Vec<&'a str>, CandidateError> {
    require_active_recipes(rule, line, description)?;
    Ok(rule
        .recipes
        .iter()
        .filter(|recipe| recipe.state == ConditionalTruth::True)
        .map(|recipe| recipe.text.trim())
        .collect())
}

fn eval_single_path(
    raw: &str,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    relative_dir: &Path,
    line: usize,
) -> Result<String, String> {
    let values = eval_words(raw, scope, dirs, root, relative_dir, line)?;
    match values.as_slice() {
        [value] => Ok(value.clone()),
        _ => Err("Make path expression must resolve to exactly one word".into()),
    }
}

fn eval_words(
    raw: &str,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    relative_dir: &Path,
    line: usize,
) -> Result<Vec<String>, String> {
    evaluate_make_list(
        raw,
        &expression_context(scope, dirs, root, relative_dir, line),
    )
    .map_err(|error| format!("cannot resolve Make expression `{raw}`: {error}"))
}

const fn expression_context<'a>(
    scope: &'a VarScope,
    dirs: &'a DirVars,
    root: &'a Path,
    relative_dir: &'a Path,
    line: usize,
) -> MakeExprContext<'a> {
    MakeExprContext::new(scope, dirs, line, root, relative_dir).without_filesystem()
}

fn parse_snapshot(snapshot: &str, line_states: &[ConditionalTruth]) -> SnapshotRules {
    let mut parsed = SnapshotRules {
        rules: Vec::new(),
        meta_rules: Vec::new(),
    };
    let mut pending_bare_meta = None;
    for (line, raw) in snapshot.lines().enumerate() {
        if raw.starts_with('\t') {
            if let Some(rule) = parsed.rules.last_mut() {
                rule.recipes.push(Recipe {
                    text: raw.trim().to_owned(),
                    state: line_state(line_states, line),
                });
            }
            continue;
        }

        let trimmed = raw.trim();
        if let Some(meta) = parse_meta(trimmed, line, line_state(line_states, line)) {
            match meta {
                ParsedMeta::Bare => pending_bare_meta = Some(line),
                ParsedMeta::Rule(meta_rule) => {
                    pending_bare_meta = None;
                    parsed.meta_rules.push(meta_rule);
                }
            }
            continue;
        }
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let clean = strip_make_comment(raw).trim();
        if let Some((target, body)) = parse_rule(clean) {
            if parsed.rules.len() < MAX_RULES {
                parsed.rules.push(Rule {
                    target: target.to_owned(),
                    prerequisites: body.prerequisites,
                    order_only: body.order_only,
                    line,
                    state: line_state(line_states, line),
                    bare_meta_line: pending_bare_meta.take(),
                    inline_recipe: body.inline_recipe,
                    recipes: Vec::new(),
                });
            }
        } else if !clean.starts_with('.') && !clean.starts_with('%') {
            // A non-rule statement ends a pending bare marker; Make would not
            // associate the marker across an assignment or directive.
            if !clean.starts_with("ifeq")
                && !clean.starts_with("ifneq")
                && clean != "else"
                && clean != "endif"
            {
                pending_bare_meta = None;
            }
        }
    }
    parsed
}

#[derive(Debug)]
enum ParsedMeta {
    Bare,
    Rule(MetaRule),
}

fn parse_meta(line: &str, source_line: usize, state: ConditionalTruth) -> Option<ParsedMeta> {
    let after = line.strip_prefix("#MM")?;
    let mut rest = after.trim_start();
    if let Some(without_dash) = rest.strip_prefix('-') {
        if without_dash.starts_with(char::is_whitespace) {
            rest = without_dash.trim_start();
        }
    }
    if rest.is_empty() {
        return Some(ParsedMeta::Bare);
    }
    let (target, prerequisites) = split_target_colon(rest)?;
    (!target.is_empty()).then(|| {
        ParsedMeta::Rule(MetaRule {
            target: target.trim().to_owned(),
            prerequisites: prerequisites.to_owned(),
            line: source_line,
            state,
        })
    })
}

#[derive(Debug)]
struct RuleBody {
    prerequisites: String,
    order_only: Option<String>,
    inline_recipe: Option<String>,
}

fn parse_rule(line: &str) -> Option<(&str, RuleBody)> {
    let (target, rest) = split_target_colon(line)?;
    let target = target.trim();
    if target.is_empty() || target.contains(char::is_whitespace) {
        return None;
    }
    let (body, inline_recipe) = split_unquoted_semicolon(rest);
    let (prerequisites, order_only) = split_order_only(body);
    Some((
        target.trim(),
        RuleBody {
            prerequisites: prerequisites.trim().to_owned(),
            order_only: order_only.map(|value| value.trim().to_owned()),
            inline_recipe: inline_recipe
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned),
        },
    ))
}

fn split_target_colon(text: &str) -> Option<(&str, &str)> {
    let bytes = text.as_bytes();
    let mut make_depth = 0usize;
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if bytes[cursor] == b'$' && bytes.get(cursor + 1) == Some(&b'(') {
            make_depth += 1;
            cursor += 2;
            continue;
        }
        if bytes[cursor] == b')' && make_depth > 0 {
            make_depth -= 1;
        } else if bytes[cursor] == b':' && make_depth == 0 {
            if bytes.get(cursor + 1) == Some(&b'=') {
                return None;
            }
            return Some((&text[..cursor], &text[cursor + 1..]));
        }
        cursor += 1;
    }
    None
}

fn split_order_only(text: &str) -> (&str, Option<&str>) {
    let bytes = text.as_bytes();
    let mut make_depth = 0usize;
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if bytes[cursor] == b'$' && bytes.get(cursor + 1) == Some(&b'(') {
            make_depth += 1;
            cursor += 2;
            continue;
        }
        if bytes[cursor] == b')' && make_depth > 0 {
            make_depth -= 1;
        } else if bytes[cursor] == b'|' && make_depth == 0 {
            return (&text[..cursor], Some(&text[cursor + 1..]));
        }
        cursor += 1;
    }
    (text, None)
}

fn split_unquoted_semicolon(text: &str) -> (&str, Option<&str>) {
    let bytes = text.as_bytes();
    let mut make_depth = 0usize;
    let mut quote = None;
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        let byte = bytes[cursor];
        if let Some(delimiter) = quote {
            if byte == delimiter {
                quote = None;
            }
        } else if matches!(byte, b'\'' | b'"') {
            quote = Some(byte);
        } else if byte == b'$' && bytes.get(cursor + 1) == Some(&b'(') {
            make_depth += 1;
            cursor += 2;
            continue;
        } else if byte == b')' && make_depth > 0 {
            make_depth -= 1;
        } else if byte == b';' && make_depth == 0 {
            return (&text[..cursor], Some(&text[cursor + 1..]));
        }
        cursor += 1;
    }
    (text, None)
}

fn line_state(states: &[ConditionalTruth], line: usize) -> ConditionalTruth {
    states
        .get(line)
        .copied()
        .unwrap_or(ConditionalTruth::Unknown)
}

fn safe_owner_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "_.-".contains(character))
}

fn is_build_output(path: &str, extension: &str) -> bool {
    path.strip_prefix(&format!("{BUILD_ALIAS}/"))
        .is_some_and(|tail| valid_relative_tail(tail) && tail.ends_with(extension))
}

fn is_generated_output(path: &str, extension: &str) -> bool {
    path.strip_prefix(&format!("{BUILD_ALIAS}/gen/"))
        .is_some_and(|tail| valid_relative_tail(tail) && tail.ends_with(extension))
}

fn is_build_directory(path: &str) -> bool {
    path.strip_prefix(&format!("{BUILD_ALIAS}/"))
        .is_some_and(valid_relative_tail)
}

fn is_build_path(path: &str) -> bool {
    path == BUILD_ALIAS || is_build_directory(path)
}

fn valid_relative_tail(tail: &str) -> bool {
    !tail.is_empty()
        && !tail
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
        && Path::new(tail)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
        && !tail.contains('\\')
}

fn source_relative_path(path: &str) -> Option<String> {
    let tail = path.strip_prefix(&format!("{SOURCE_ALIAS}/"))?;
    if valid_relative_tail(tail) {
        Some(tail.to_owned())
    } else {
        None
    }
}

fn has_extension(path: &str, extension: &str) -> bool {
    Path::new(path)
        .extension()
        .is_some_and(|actual| actual.eq_ignore_ascii_case(extension))
}

fn safe_relative_file(root: &Path, relative: &Path, file_name: &str) -> Result<PathBuf, String> {
    let canonical_root = root
        .canonicalize()
        .map_err(|error| format!("selected source root cannot be canonicalized: {error}"))?;
    if !canonical_root.is_dir() {
        return Err("selected source root is not a directory".into());
    }
    let mut candidate = relative.to_path_buf();
    if !file_name.is_empty() {
        candidate.push(file_name);
    }
    if candidate.as_os_str().is_empty()
        || candidate.is_absolute()
        || candidate
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err("source path is not a normal source-root-relative path".into());
    }
    let mut current = canonical_root.clone();
    let components = candidate.components().collect::<Vec<_>>();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(name) = component else {
            return Err("source path contains a non-normal component".into());
        };
        current.push(name);
        let metadata = fs::symlink_metadata(&current).map_err(|error| {
            format!(
                "source path `{}` is unavailable: {error}",
                current.display()
            )
        })?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "source path `{}` is a symlink and is not admitted",
                current.display()
            ));
        }
        let last = index + 1 == components.len();
        if last && !metadata.is_file() {
            return Err(format!(
                "source path `{}` is not a regular file",
                current.display()
            ));
        }
        if !last && !metadata.is_dir() {
            return Err(format!(
                "source path parent `{}` is not a directory",
                current.display()
            ));
        }
    }
    if !current.starts_with(&canonical_root) {
        return Err("source path resolves outside the selected source root".into());
    }
    Ok(current)
}

fn read_bounded_snapshot(path: &Path) -> Result<String, String> {
    let measured = aros_common::measure_regular_file_bounded(path, MAX_SNAPSHOT_BYTES as u64)
        .map_err(|error| format!("physical mmakefile cannot be read: {error}"))?;
    let Some((_, bytes)) = measured else {
        return Err("physical mmakefile is missing".into());
    };
    String::from_utf8(bytes).map_err(|_| "physical mmakefile is not valid UTF-8".into())
}

fn role_mutation_issue(
    snapshot: &str,
    line_states: &[ConditionalTruth],
) -> Option<(usize, String)> {
    let mut target_cc_count = 0usize;
    for (line, raw) in snapshot.lines().enumerate() {
        if raw.starts_with('\t') || line_state(line_states, line) == ConditionalTruth::False {
            continue;
        }
        let clean = strip_make_comment(raw).trim();
        if clean.contains("$(eval") || clean.contains("${eval") {
            return Some((line, "Make `eval` could mutate compiler arguments".into()));
        }
        if let Some((name, value, kind)) = variable_assignment(clean) {
            match name {
                "NATIVE_TARGET_CC" => {
                    return Some((
                        line,
                        "source rebinds the admitted native compiler alias".into(),
                    ));
                }
                "TARGET_CC" => {
                    target_cc_count += 1;
                    if value.trim() != "$(NATIVE_TARGET_CC)"
                        || kind != AssignmentKind::RecursiveSet
                        || target_cc_count > 1
                    {
                        return Some((
                            line,
                            "source rebinds TARGET_CC outside its admitted native alias".into(),
                        ));
                    }
                }
                "TARGET_SYSROOT" if value.trim() != "--sysroot=$(AROS_DEVELOPER)" => {
                    return Some((
                        line,
                        "source rebinds TARGET_SYSROOT outside the configured Developer root"
                            .into(),
                    ));
                }
                _ => {}
            }
        }
        if let Some((_, remainder)) = split_target_colon(clean) {
            if variable_assignment(remainder).is_some_and(|(name, _, _)| {
                matches!(name, "TARGET_CC" | "NATIVE_TARGET_CC" | "TARGET_SYSROOT")
            }) {
                return Some((
                    line,
                    "source uses a target-specific compiler-role assignment".into(),
                ));
            }
        }
        if let Ok(Some(name)) = undefine_directive(clean) {
            if matches!(name, "TARGET_CC" | "NATIVE_TARGET_CC" | "TARGET_SYSROOT") {
                return Some((line, format!("source undefines compiler role `{name}`")));
            }
        }
        if clean.starts_with("define TARGET_CC")
            || clean.starts_with("define NATIVE_TARGET_CC")
            || clean.starts_with("define TARGET_SYSROOT")
        {
            return Some((line, "source defines a compiler role opaquely".into()));
        }
    }
    None
}

fn rejection(owner: &str, line: usize, reason: &str) -> AssemblyHeaderRejection {
    AssemblyHeaderRejection {
        owner: owner.to_owned(),
        file: String::new(),
        line,
        reason: reason.to_owned(),
    }
}

fn relative_mmakefile(relative_dir: &Path) -> String {
    let path = relative_dir.join("mmakefile.src");
    path.to_string_lossy().replace('\\', "/")
}

fn with_file(
    mut rejections: Vec<AssemblyHeaderRejection>,
    file: &str,
) -> Vec<AssemblyHeaderRejection> {
    for rejection in &mut rejections {
        file.clone_into(&mut rejection.file);
    }
    rejections
}

fn finalize_rejections(
    mut rejections: Vec<AssemblyHeaderRejection>,
    file: &str,
    physical_lines: &[usize],
) -> Vec<AssemblyHeaderRejection> {
    for rejection in &mut rejections {
        file.clone_into(&mut rejection.file);
        rejection.line = physical_line(physical_lines, rejection.line);
    }
    rejections
}

fn physical_line(physical_lines: &[usize], joined_line: usize) -> usize {
    physical_lines
        .get(joined_line.saturating_sub(1))
        .map_or(joined_line, |line| line + 1)
}

fn join_continuations_with_origins(source: &str) -> (String, Vec<usize>) {
    crate::parser::join_continuations_with_origins(source)
}

fn candidate_error(owner: &str, line: usize, reason: impl Into<String>) -> CandidateError {
    CandidateError {
        owner: owner.to_owned(),
        line,
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use tempfile::TempDir;

    const SNAPSHOT: &str = r#"# physical line mapping \
continuation line
OBJDIR := $(GENDIR)/header
GENINCDIR := $(GENDIR)/include
CPU := $(AROS_TARGET_CPU)
#MM
provider-$(CPU) : $(GENINCDIR)/generated/$(CPU)/table.h
#MM aggregate : prep provider-$(CPU)
#MM
aggregate:
	@$(NOP)
ifeq ($(AROS_TOOLCHAIN),gnu)
GREPTOKEN := ".asciz"
else
GREPTOKEN := ".ascii"
endif
$(OBJDIR)/table.s : $(SRCDIR)/$(CURDIR)/table.c | $(OBJDIR)
	@$(ECHO) "Compiling  $<..."
	@$(TARGET_CC) $(TARGET_SYSROOT) $(CFLAGS) $(PRIV_EXEC_INCLUDES) -S $< -o $@
$(GENINCDIR)/generated/$(CPU)/table.h : $(OBJDIR)/table.s | $(GENINCDIR)/generated/$(AROS_TARGET_CPU)
	@$(ECHO) Generating $@...
	@grep $(GREPTOKEN) $< | cut -d'"' -f2 | sed 's/\$$//g' >$@
"#;

    fn fixture() -> (TempDir, PathBuf, TargetContext, DirVars) {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        let directory = root.join("hardware/header");
        fs::create_dir_all(&directory).expect("source directory");
        fs::write(directory.join("mmakefile.src"), SNAPSHOT).expect("mmakefile");
        fs::write(directory.join("table.c"), "int table;\n").expect("C input");

        let mut make_variables = BTreeMap::new();
        make_variables.insert("TARGET_CC".into(), "$(NATIVE_TARGET_CC)".into());
        make_variables.insert(
            "TARGET_SYSROOT".into(),
            "--sysroot=$(AROS_DEVELOPER)".into(),
        );
        make_variables.insert(
            "AROS_DEVELOPER".into(),
            "$(AROS_BUILD_DIR)/Developer".into(),
        );
        make_variables.insert("CFLAGS".into(), "-O2 -DTEST_FLAG=1".into());
        make_variables.insert(
            "PRIV_EXEC_INCLUDES".into(),
            "-I$(SRCDIR)/rom/exec -I$(SRCDIR)/rom/kernel".into(),
        );
        let target = TargetContext {
            cpu: Some("othercpu".into()),
            toolchain: Some("gnu".into()),
            make_variables,
            ..TargetContext::default()
        };
        let mut dirs = DirVars::load(root);
        dirs.bind_native_target_tool_roles();
        (temp, directory, target, dirs)
    }

    #[test]
    fn collects_generic_provider_and_resolved_assembly_argv() {
        let (_temp, directory, target, dirs) = fixture();
        let (decls, rejections) = collect_from_snapshot(
            SNAPSHOT,
            &target,
            &dirs,
            directory.parent().expect("fixture root"),
            Path::new("header"),
        );

        assert!(rejections.is_empty(), "{rejections:?}");
        let [decl] = decls.as_slice() else {
            panic!("expected one declaration, got {decls:?}");
        };
        assert_eq!(decl.owner, "provider-othercpu");
        assert_eq!(
            decl.line,
            SNAPSHOT
                .lines()
                .position(|line| line.starts_with("provider-"))
                .expect("provider physical line")
                + 1
        );
        assert_eq!(decl.aggregate_owner, "aggregate");
        assert_eq!(decl.aggregate_dependencies, ["prep", "provider-othercpu"]);
        assert_eq!(decl.source, "${AROS_SOURCE_DIR}/header/table.c");
        assert_eq!(decl.assembly_output, "${AROS_BUILD_DIR}/gen/header/table.s");
        assert_eq!(
            decl.header_output,
            "${AROS_BUILD_DIR}/gen/include/generated/othercpu/table.h"
        );
        assert_eq!(decl.header_root, "${AROS_BUILD_DIR}/gen/include");
        assert_eq!(decl.token, ".asciz");
        assert_eq!(
            decl.arguments,
            [
                "--sysroot=${AROS_BUILD_DIR}/Developer",
                "-O2",
                "-DTEST_FLAG=1",
                "-I",
                "${AROS_SOURCE_DIR}/rom/exec",
                "-I",
                "${AROS_SOURCE_DIR}/rom/kernel",
            ]
        );
    }

    #[test]
    fn rejects_an_oversized_physical_mmakefile_even_with_small_snapshot() {
        let (_temp, directory, target, dirs) = fixture();
        let oversized = format!("{SNAPSHOT}{}", "x".repeat(MAX_SNAPSHOT_BYTES + 1));
        fs::write(directory.join("mmakefile.src"), oversized).expect("oversized mmakefile");

        let (decls, rejections) = collect_from_snapshot(
            SNAPSHOT,
            &target,
            &dirs,
            directory.parent().expect("fixture root"),
            Path::new("header"),
        );

        assert!(decls.is_empty());
        assert!(rejections
            .iter()
            .any(|item| item.reason.contains("read limit")));
    }

    #[test]
    fn ignores_ordinary_sed_and_source_header_rules() {
        let (_temp, directory, target, dirs) = fixture();
        let extended = format!(
            "{SNAPSHOT}\n$(GENINCDIR)/pkgconfig.h : $(SRCDIR)/pkgconfig.in\n\t@sed 's/old/new/' $< >$@\n$(GENINCDIR)/source.h : $(SRCDIR)/include.src\n\t@sed 's/old/new/' $< >$@\n"
        );
        fs::write(directory.join("mmakefile.src"), &extended).expect("extended mmakefile");

        let (decls, rejections) = collect_from_snapshot(
            &extended,
            &target,
            &dirs,
            directory.parent().expect("fixture root"),
            Path::new("header"),
        );

        assert_eq!(decls.len(), 1);
        assert!(rejections.is_empty(), "{rejections:?}");
    }

    #[test]
    fn rejects_unresolved_compiler_flags_instead_of_guessing() {
        let (_temp, directory, mut target, dirs) = fixture();
        target.make_variables.remove("CFLAGS");
        let (decls, rejections) = collect_from_snapshot(
            SNAPSHOT,
            &target,
            &dirs,
            directory.parent().expect("fixture root"),
            Path::new("header"),
        );
        assert!(decls.is_empty());
        assert!(rejections.iter().any(|item| item.reason.contains("CFLAGS")));
    }

    #[test]
    fn rejects_compiler_output_and_mode_flags() {
        for flag in ["-oother", "-E", "--", "-fsyntax-only"] {
            let (_temp, directory, mut target, dirs) = fixture();
            target.make_variables.insert("CFLAGS".into(), flag.into());
            let (decls, rejections) = collect_from_snapshot(
                SNAPSHOT,
                &target,
                &dirs,
                directory.parent().expect("fixture root"),
                Path::new("header"),
            );
            assert!(decls.is_empty(), "accepted forbidden mode flag {flag}");
            assert!(
                rejections
                    .iter()
                    .any(|item| item.reason.contains("compiler flag")),
                "missing rejection for {flag}: {rejections:?}"
            );
        }
    }

    #[test]
    fn rejects_unknown_token_condition() {
        let (_temp, directory, mut target, dirs) = fixture();
        target.toolchain = None;
        let (decls, rejections) = collect_from_snapshot(
            SNAPSHOT,
            &target,
            &dirs,
            directory.parent().expect("fixture root"),
            Path::new("header"),
        );
        assert!(decls.is_empty());
        assert!(rejections.iter().any(|item| {
            item.reason.contains("GREPTOKEN") || item.reason.contains("conditional")
        }));
    }

    #[test]
    fn rejects_include_escape_and_changed_pipeline_recipe() {
        let (_temp, directory, mut target, dirs) = fixture();
        target.make_variables.insert(
            "PRIV_EXEC_INCLUDES".into(),
            "-I$(SRCDIR)/../../outside".into(),
        );
        let changed = SNAPSHOT.replace(
            "@grep $(GREPTOKEN) $< | cut -d'\"' -f2 | sed 's/\\$$//g' >$@",
            "@grep $(GREPTOKEN) $< >$@",
        );
        fs::write(directory.join("mmakefile.src"), &changed).expect("changed source");
        let (decls, rejections) = collect_from_snapshot(
            &changed,
            &target,
            &dirs,
            directory.parent().expect("fixture root"),
            Path::new("header"),
        );
        assert!(decls.is_empty());
        assert!(rejections.iter().any(|item| {
            item.reason.contains("pipeline") || item.reason.contains("include path")
        }));
    }

    #[test]
    fn rejects_duplicate_header_rules() {
        let (_temp, directory, target, dirs) = fixture();
        let duplicated = format!(
            "{SNAPSHOT}\n$(GENINCDIR)/generated/$(CPU)/table.h : $(OBJDIR)/table.s | $(OBJDIR)\n\t@$(ECHO) Generating $@...\n\t@grep $(GREPTOKEN) $< | cut -d'\"' -f2 | sed 's/\\$$//g' >$@\n"
        );
        fs::write(directory.join("mmakefile.src"), &duplicated).expect("duplicated source");
        let (decls, rejections) = collect_from_snapshot(
            &duplicated,
            &target,
            &dirs,
            directory.parent().expect("fixture root"),
            Path::new("header"),
        );
        assert!(decls.is_empty());
        assert!(rejections
            .iter()
            .any(|item| item.reason.contains("multiple Make rules")));
    }
}
