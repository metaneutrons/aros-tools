//! Source-backed effects of architecture-specific Make endpoints.
//!
//! This module deliberately admits only three narrow effects:
//!
//! * `%build_archspecific`'s `-linklib` leaf when its module source list is
//!   known and nonempty, and both linklib-object lists are provably empty;
//! * `%set_archincludes` when its endpoint, output path, and include paths can
//!   all be stated without guessing.
//! * the corresponding target-compiler architecture object group, only with
//!   an exact source/module compilation binding and no custom object root.
//!
//! A nonempty linklib-object lane is not a producer here. Architecture objects
//! require an exact native object-compilation binding and must not be
//! represented by an alias to the module target.

use crate::make_expr::{evaluate_make_expr, MakeExprContext, MakeExprError};
use crate::make_vars::{ConditionalTruth, VarScope};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchEndpointEffect {
    /// Source-relative `mmakefile.src` path.
    pub recipe: String,
    /// One-based source line in the original recipe text.
    pub line: usize,
    /// Exact generated Make endpoint spelling.
    pub endpoint: String,
    /// Named MetaMake dependencies proved by the source/template contract.
    pub dependencies: Vec<String>,
    pub data: ArchEndpointEffectData,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ArchEndpointEffectData {
    /// The exact architecture source group, compiled separately with the
    /// already-qualified module's flags. Never an alias to that module.
    ArchModuleObjects {
        mainmmake: String,
        tag: String,
        module_sources: Vec<String>,
        directory: String,
    },
    /// A valid arch-specific invocation with no objects in its linklib lane.
    /// `module_sources` are declaration validity evidence only; this endpoint
    /// does not compile them or claim their object outputs.
    EmptyLinklibAggregate {
        mainmmake: String,
        tag: String,
        module_sources: Vec<String>,
        compiler: String,
    },
    /// `%set_archincludes`' generated flag-file effect. The CMake projection
    /// should bind the include metadata, not claim a compilation/archive.
    SetArchIncludes {
        mainmmake: String,
        tag: String,
        modname: String,
        maindir: String,
        priority: u32,
        priority_token: String,
        include_dirs: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        definitions: Vec<String>,
        /// Exact argument order written by the source-owned flag producer.
        /// Unlike the directory index, repeated flags are retained.
        #[serde(default)]
        arguments: Vec<String>,
        generated_file: String,
        order_only_directory: String,
    },
}

impl ArchEndpointEffect {
    /// Exact selector tags, never a CPU inferred from an endpoint spelling.
    #[must_use]
    pub fn applies_to(&self, context: &crate::TargetContext) -> bool {
        let tag = match &self.data {
            ArchEndpointEffectData::ArchModuleObjects { tag, .. }
            | ArchEndpointEffectData::EmptyLinklibAggregate { tag, .. }
            | ArchEndpointEffectData::SetArchIncludes { tag, .. } => tag,
        };
        let (Some(cpu), Some(platform)) = (&context.cpu, &context.platform) else {
            return false;
        };
        tag == cpu
            || tag == platform
            || tag == &format!("{platform}-{cpu}")
            || tag == "native"
            || context
                .family
                .as_ref()
                .is_some_and(|family| !family.is_empty() && tag == family)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RejectedArchEndpointEffect {
    pub recipe: String,
    /// One-based source line in the supplied text.
    pub line: usize,
    pub directive: String,
    /// Exact candidate endpoint when the invocation proves its literal owner
    /// and architecture tag, even though another part of the invocation was
    /// rejected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchEndpointEffectScan {
    pub effects: Vec<ArchEndpointEffect>,
    pub rejected: Vec<RejectedArchEndpointEffect>,
}

/// Builds a source-only Make scope while keeping every lookup index aligned
/// with the physical recipe lines consumed by the endpoint scanner.
///
/// Make continuations are joined onto their first physical line. The remaining
/// physical slots become blank lines, so a declaration at physical line N
/// still queries the scope at index N - 1. This preserves declaration-time
/// assignment semantics without borrowing the parser pipeline's inlined or
/// ambient configuration.
pub(crate) fn collect_arch_effect_scope(content: &str) -> (VarScope, Vec<ConditionalTruth>) {
    collect_arch_effect_scope_with_context(content, None)
}

/// Uses the explicitly selected source configuration, never ambient Make
/// values. The same context must be used for parsing and sealed replay.
pub(crate) fn collect_arch_effect_scope_with_context(
    content: &str,
    target: Option<&crate::TargetContext>,
) -> (VarScope, Vec<ConditionalTruth>) {
    let physical_lines = content.split_inclusive('\n').collect::<Vec<_>>();
    let mut physical_view = String::with_capacity(content.len());
    let mut start = 0usize;

    while start < physical_lines.len() {
        let mut end = start;
        let mut has_continuation = false;
        while has_make_line_continuation(physical_lines[end]) {
            has_continuation = true;
            if end + 1 == physical_lines.len() {
                break;
            }
            end += 1;
        }

        if has_continuation {
            let raw_group = physical_lines[start..=end].concat();
            let mut logical_line = crate::parser::join_continuations(&raw_group);
            if let Some(without_newline) = logical_line.strip_suffix('\n') {
                logical_line = without_newline
                    .strip_suffix('\r')
                    .unwrap_or(without_newline)
                    .to_owned();
            }
            physical_view.push_str(&logical_line);
            // One newline per physical line yields the logical line at `start`
            // followed by blank slots for every consumed continuation tail.
            for _ in start..=end {
                physical_view.push('\n');
            }
        } else {
            physical_view.push_str(physical_lines[start]);
        }

        start = end + 1;
    }

    debug_assert_eq!(physical_view.lines().count(), content.lines().count());
    crate::make_vars::collect_vars_impl(&physical_view, target)
}

fn has_make_line_continuation(physical_line: &str) -> bool {
    let Some(without_newline) = physical_line.strip_suffix('\n') else {
        return false;
    };
    let without_cr = without_newline
        .strip_suffix('\r')
        .unwrap_or(without_newline);
    without_cr.trim_end_matches([' ', '\t']).ends_with('\\')
}

/// Collects strictly source-proved architecture endpoint effects.
///
/// `content`, `scope`, and `line_states` must describe the original recipe,
/// before inlining or continuation rewriting. Unknown branches fail closed;
/// architecture-tag selection is intentionally left to the caller.
///
/// # Errors
/// Returns an error when `recipe` is not one safe source-relative path. An
/// unsupported individual invocation is instead recorded in `rejected`.
pub(crate) fn collect_arch_endpoint_effects(
    content: &str,
    recipe: &Path,
    scope: &VarScope,
    line_states: Option<&[ConditionalTruth]>,
) -> Result<ArchEndpointEffectScan, String> {
    collect_arch_endpoint_effects_at_positions(content, recipe, scope, line_states, None, false)
}

/// Source-native include scopes are adopted atomically and replayed using the
/// same explicit configuration. Never use ambient or generated Make files.
pub(crate) fn collect_source_arch_endpoint_effects(
    content: &str,
    recipe: &Path,
    root: &Path,
    target: Option<&crate::TargetContext>,
) -> Result<ArchEndpointEffectScan, String> {
    let scan = crate::local_make_includes::inline_native_make_configuration_with_templates(
        content,
        root,
        recipe,
        crate::local_make_includes::LocalMakeIncludeLimits::default(),
        &target.map_or_else(BTreeMap::new, |target| target.make_include_bindings.clone()),
        &target.map_or_else(BTreeMap::new, |target| {
            target.generated_make_templates.clone()
        }),
    );
    if scan.issues.is_empty() && !scan.fragments.is_empty() {
        let joined = crate::parser::join_continuations(&scan.expanded);
        let positions =
            crate::parser::architecture_scope_positions(root, recipe, content, &scan, &joined)
                .ok_or("architecture configuration lacks exact physical source positions")?;
        let (scope, states) = crate::make_vars::collect_vars_impl(&joined, target);
        return collect_arch_endpoint_effects_at_positions(
            content,
            recipe,
            &scope,
            Some(&states),
            Some(&positions),
            target.is_some_and(|target| target.native_kernel_sources_in_target_role),
        );
    }
    // An incomplete include traversal cannot lend even a partially resolved
    // value to an effect. The original source-only scope still diagnoses any
    // invocation which needs such a value.
    let (scope, states) = collect_arch_effect_scope_with_context(content, target);
    collect_arch_endpoint_effects(content, recipe, &scope, Some(&states))
}

fn collect_arch_endpoint_effects_at_positions(
    content: &str,
    recipe: &Path,
    scope: &VarScope,
    line_states: Option<&[ConditionalTruth]>,
    positions: Option<&[Option<usize>]>,
    kernel_in_target_role: bool,
) -> Result<ArchEndpointEffectScan, String> {
    let recipe = normalize_recipe(recipe)?;
    let recipe_dir = recipe
        .rsplit_once('/')
        .map_or("", |(directory, _)| directory);
    let fallback_states;
    let line_states = if let Some(states) = line_states {
        states
    } else {
        fallback_states = collect_arch_effect_scope(content).1;
        &fallback_states
    };
    let mut scan = ArchEndpointEffectScan::default();

    for (line, body) in crate::includes::directive_bodies_at(content, "%build_archspecific") {
        let position = positions.map_or(Some(line), |positions| {
            positions.get(line).copied().flatten()
        });
        match position
            .and_then(|position| line_states.get(position))
            .copied()
            .unwrap_or(ConditionalTruth::Unknown)
        {
            ConditionalTruth::False => {}
            ConditionalTruth::Unknown => {
                scan.rejected.push(rejected(
                    &recipe,
                    line,
                    "%build_archspecific",
                    "directive is inside a conditional branch not proven active",
                    &body,
                ));
            }
            ConditionalTruth::True => match parse_empty_linklib(
                &body,
                position.expect("active source position"),
                scope,
                recipe_dir,
            ) {
                Ok(Some((endpoint, dependencies, data))) => {
                    if let ArchEndpointEffectData::EmptyLinklibAggregate {
                        mainmmake,
                        tag,
                        module_sources,
                        compiler,
                    } = &data
                    {
                        // Kernel sources get an object group only when the
                        // source contract declares they build in the target
                        // role; otherwise their objects stay without producer.
                        if compiler == "target" || kernel_in_target_role && compiler == "kernel" {
                            scan.effects.push(ArchEndpointEffect {
                                recipe: recipe.clone(),
                                line: line + 1,
                                endpoint: format!("{mainmmake}-{tag}"),
                                dependencies: dependencies.clone(),
                                data: ArchEndpointEffectData::ArchModuleObjects {
                                    mainmmake: mainmmake.clone(),
                                    tag: tag.clone(),
                                    module_sources: module_sources.clone(),
                                    directory: recipe_dir.to_owned(),
                                },
                            });
                        }
                    }
                    scan.effects.push(ArchEndpointEffect {
                        recipe: recipe.clone(),
                        line: line + 1,
                        endpoint,
                        dependencies,
                        data,
                    });
                }
                Ok(None) => {}
                Err(reason) => scan.rejected.push(RejectedArchEndpointEffect {
                    recipe: recipe.clone(),
                    line: line + 1,
                    directive: "%build_archspecific".into(),
                    endpoint: candidate_endpoint_in(
                        "%build_archspecific",
                        &body,
                        position.map(|position| (scope, position, recipe_dir)),
                    ),
                    reason,
                }),
            },
        }
    }

    for (line, body) in crate::includes::directive_bodies_at(content, "%set_archincludes") {
        let position = positions.map_or(Some(line), |positions| {
            positions.get(line).copied().flatten()
        });
        match position
            .and_then(|position| line_states.get(position))
            .copied()
            .unwrap_or(ConditionalTruth::Unknown)
        {
            ConditionalTruth::False => {}
            ConditionalTruth::Unknown => {
                scan.rejected.push(rejected(
                    &recipe,
                    line,
                    "%set_archincludes",
                    "directive is inside a conditional branch not proven active",
                    &body,
                ));
            }
            ConditionalTruth::True => match parse_arch_includes(
                &body,
                position.expect("active source position"),
                scope,
                recipe_dir,
            ) {
                Ok(Some((endpoint, dependencies, data))) => {
                    scan.effects.push(ArchEndpointEffect {
                        recipe: recipe.clone(),
                        line: line + 1,
                        endpoint,
                        dependencies,
                        data,
                    });
                }
                Ok(None) => {}
                Err(reason) => scan.rejected.push(RejectedArchEndpointEffect {
                    recipe: recipe.clone(),
                    line: line + 1,
                    directive: "%set_archincludes".into(),
                    endpoint: candidate_endpoint_in(
                        "%set_archincludes",
                        &body,
                        position.map(|position| (scope, position, recipe_dir)),
                    ),
                    reason,
                }),
            },
        }
    }

    scan.effects
        .sort_by(|a, b| (&a.recipe, a.line, &a.endpoint).cmp(&(&b.recipe, b.line, &b.endpoint)));
    scan.rejected.sort_by(|a, b| {
        (&a.recipe, a.line, &a.directive, &a.reason).cmp(&(
            &b.recipe,
            b.line,
            &b.directive,
            &b.reason,
        ))
    });
    Ok(scan)
}

fn rejected(
    recipe: &str,
    line: usize,
    directive: &str,
    reason: &str,
    body: &str,
) -> RejectedArchEndpointEffect {
    RejectedArchEndpointEffect {
        recipe: recipe.to_owned(),
        line: line + 1,
        directive: directive.to_owned(),
        endpoint: candidate_endpoint(directive, body),
        reason: reason.to_owned(),
    }
}

fn candidate_endpoint(directive: &str, body: &str) -> Option<String> {
    candidate_endpoint_in(directive, body, None)
}

/// The endpoint a rejected declaration would have produced, so its failure
/// is attributed to that owner. With the declaration's scope, a resolvable
/// `arch=` expression names it too.
fn candidate_endpoint_in(
    directive: &str,
    body: &str,
    scope: Option<(&VarScope, usize, &str)>,
) -> Option<String> {
    let fields = parse_arguments(body, directive).ok()?;
    let mainmmake = required_literal(&fields, "mainmmake").ok()?;
    let tag = match scope {
        Some((scope, line, recipe_dir)) => {
            architecture_tag(&fields, scope, line, recipe_dir).ok()?
        }
        None => required_literal(&fields, "arch").ok()?,
    };
    Some(match directive {
        "%build_archspecific" => format!("{mainmmake}-{tag}"),
        "%set_archincludes" => format!("{mainmmake}-{tag}-set-archincludes"),
        _ => return None,
    })
}

#[derive(Debug, Clone)]
struct Argument {
    value: String,
}

fn parse_arguments(body: &str, directive: &str) -> Result<BTreeMap<String, Argument>, String> {
    let tail = body
        .strip_prefix(directive)
        .ok_or_else(|| format!("malformed {directive} directive name"))?;
    if tail
        .chars()
        .next()
        .is_some_and(|character| !character.is_whitespace())
    {
        return Err(format!("malformed {directive} directive boundary"));
    }

    let mut fields = BTreeMap::new();
    let bytes = tail.as_bytes();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor == bytes.len() {
            break;
        }

        let key_start = cursor;
        while cursor < bytes.len()
            && (bytes[cursor].is_ascii_alphanumeric() || bytes[cursor] == b'_')
        {
            cursor += 1;
        }
        if cursor == key_start || bytes.get(cursor) != Some(&b'=') {
            return Err(format!("malformed argument near {:?}", &tail[key_start..]));
        }
        let key = &tail[key_start..cursor];
        cursor += 1;

        let value = if bytes.get(cursor) == Some(&b'"') {
            cursor += 1;
            let value_start = cursor;
            while cursor < bytes.len() && bytes[cursor] != b'"' {
                // Escaped Make quotes are not part of the supported source
                // grammar. Refuse them instead of misparsing a later token.
                if bytes[cursor] == b'\\' {
                    return Err(format!("escaped quote in {key} is unsupported"));
                }
                cursor += 1;
            }
            if cursor == bytes.len() {
                return Err(format!("unterminated quoted value for {key}"));
            }
            let value = tail[value_start..cursor].to_owned();
            cursor += 1;
            if cursor < bytes.len() && !bytes[cursor].is_ascii_whitespace() {
                return Err(format!("characters follow quoted {key} value"));
            }
            value
        } else {
            let value_start = cursor;
            while cursor < bytes.len() && !bytes[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
            tail[value_start..cursor].to_owned()
        };

        if fields.insert(key.to_owned(), Argument { value }).is_some() {
            return Err(format!("duplicate {key} argument"));
        }
    }
    Ok(fields)
}

fn parse_empty_linklib(
    body: &str,
    line: usize,
    scope: &VarScope,
    recipe_dir: &str,
) -> Result<Option<(String, Vec<String>, ArchEndpointEffectData)>, String> {
    let fields = parse_arguments(body, "%build_archspecific")?;
    let mainmmake = required_literal(&fields, "mainmmake")?;
    let tag = architecture_tag(&fields, scope, line, recipe_dir)?;

    // The source template rejects other compiler values while parsing the
    // macro, even when the linklib-object lane is empty.
    let compiler = fields
        .get("compiler")
        .map(|argument| expand_scalar(&argument.value, scope, line, recipe_dir))
        .transpose()
        .map_err(|reason| format!("compiler= cannot be resolved: {reason}"))?
        .unwrap_or_else(|| "target".to_owned());
    if !matches!(compiler.as_str(), "host" | "kernel" | "target") {
        return Err("compiler= must resolve to host, kernel, or target".into());
    }

    let mut module_sources = Vec::new();
    for key in ["files", "asmfiles"] {
        if let Some(argument) = fields.get(key) {
            module_sources.extend(expand_source_names(&argument.value, scope, line)?);
        }
    }
    deduplicate(&mut module_sources);
    if module_sources.is_empty() {
        return Err("requires a known, nonempty files= or asmfiles= source list".into());
    }

    for key in ["cxxfiles", "objcfiles"] {
        if fields
            .get(key)
            .is_some_and(|argument| !argument.value.trim().is_empty())
        {
            return Err(format!(
                "nonempty {key}= is outside the supported macro lane"
            ));
        }
    }

    for key in ["linklibfiles", "linklibobjs"] {
        if let Some(argument) = fields.get(key) {
            if !argument.value.trim().is_empty() {
                return Err(format!(
                    "{key}= is not a literal empty list; nonempty/variable linklib lanes are not modeled"
                ));
            }
        }
    }

    // maindir= is a required build_archspecific argument. Its value names the
    // object root even though this effect does not claim the module objects.
    let maindir = required_scalar(&fields, "maindir", scope, line, recipe_dir)?;
    validate_relative_path(&maindir).map_err(|reason| format!("unsafe maindir=: {reason}"))?;
    if let Some(argument) = fields.get("modname") {
        let resolved = expand_scalar(&argument.value, scope, line, recipe_dir)?;
        validate_identifier(&resolved).map_err(|reason| format!("unsafe modname=: {reason}"))?;
    }
    if fields
        .get("objdir")
        .is_some_and(|argument| !argument.value.trim().is_empty())
    {
        return Err("custom objdir= architecture effects are not modeled".into());
    }

    let endpoint = format!("{mainmmake}-{tag}-linklib");
    let dependencies = vec![format!("{mainmmake}-{tag}-includes")];
    Ok(Some((
        endpoint,
        dependencies,
        ArchEndpointEffectData::EmptyLinklibAggregate {
            mainmmake,
            tag,
            module_sources,
            compiler,
        },
    )))
}

fn parse_arch_includes(
    body: &str,
    line: usize,
    scope: &VarScope,
    recipe_dir: &str,
) -> Result<Option<(String, Vec<String>, ArchEndpointEffectData)>, String> {
    let fields = parse_arguments(body, "%set_archincludes")?;
    let mainmmake = required_literal(&fields, "mainmmake")?;
    let tag = architecture_tag(&fields, scope, line, recipe_dir)?;
    let modname = required_literal(&fields, "modname")?;
    let maindir = required_scalar(&fields, "maindir", scope, line, recipe_dir)?;
    validate_relative_path(&maindir).map_err(|reason| format!("unsafe maindir=: {reason}"))?;
    let priority = fields
        .get("pri")
        .ok_or_else(|| "missing required pri=".to_owned())?
        .value
        .trim()
        .to_owned();
    if !priority.bytes().all(|byte| byte.is_ascii_digit()) || priority.is_empty() {
        return Err("pri= must be a literal unsigned integer".into());
    }
    let priority_value = priority
        .parse::<u32>()
        .map_err(|_| "pri= is outside the supported unsigned range".to_owned())?;

    match fields.get("genincdir").map(|arg| arg.value.trim()) {
        None | Some("yes") => {}
        Some("no") => {
            return Err(
                "genincdir=no leaves the generated include directory externally owned".into(),
            )
        }
        Some(_) => return Err("genincdir= must be the literal yes when supplied".into()),
    }

    // The template's default for includes= is the empty string. Do not invent
    // the declaration directory as a fallback; that is a different effect.
    let include_raw = fields
        .get("includes")
        .map_or("", |argument| &argument.value);
    let expanded = expand_scalar(include_raw, scope, line, recipe_dir)?;
    let IncludeFlags {
        dirs: include_dirs,
        definitions,
        arguments,
    } = parse_include_flags(&expanded, recipe_dir)?;

    let include_dir = format!("gen/{maindir}/{modname}/include");
    let generated_file = format!("{include_dir}/.{modname}.includeflag.{priority}.{tag}");
    let endpoint = format!("{mainmmake}-{tag}-set-archincludes");
    Ok(Some((
        endpoint,
        Vec::new(),
        ArchEndpointEffectData::SetArchIncludes {
            mainmmake,
            tag,
            modname,
            maindir,
            priority: priority_value,
            priority_token: priority,
            include_dirs,
            definitions,
            arguments,
            generated_file,
            order_only_directory: include_dir,
        },
    )))
}

fn required_literal(fields: &BTreeMap<String, Argument>, name: &str) -> Result<String, String> {
    let value = fields
        .get(name)
        .ok_or_else(|| format!("missing required {name}="))?
        .value
        .trim()
        .to_owned();
    if value.contains('$') {
        return Err(format!("{name}= must be literal"));
    }
    validate_identifier(&value).map_err(|reason| format!("invalid {name}=: {reason}"))?;
    Ok(value)
}

/// `arch=` as one identifier: literal, or an expression the declaration's
/// scope resolves to exactly one (pfs3 uses `arch=$(AROS_TARGET_CPU)`).
fn architecture_tag(
    fields: &BTreeMap<String, Argument>,
    scope: &VarScope,
    line: usize,
    recipe_dir: &str,
) -> Result<String, String> {
    let raw = fields.get("arch").ok_or("missing required arch=")?;
    if !raw.value.contains('$') {
        return required_literal(fields, "arch");
    }
    let value = expand_scalar(&raw.value, scope, line, recipe_dir)?;
    let value = value.trim();
    if value.split_whitespace().count() != 1 {
        return Err("arch= must resolve to exactly one tag".into());
    }
    validate_identifier(value).map_err(|reason| format!("invalid arch=: {reason}"))?;
    Ok(value.to_owned())
}

fn required_scalar(
    fields: &BTreeMap<String, Argument>,
    name: &str,
    scope: &VarScope,
    line: usize,
    recipe_dir: &str,
) -> Result<String, String> {
    let raw = fields
        .get(name)
        .ok_or_else(|| format!("missing required {name}="))?;
    let value = expand_scalar(&raw.value, scope, line, recipe_dir)?;
    if value.trim().is_empty() {
        return Err(format!("{name}= resolves to an empty value"));
    }
    Ok(value)
}

fn expand_source_names(raw: &str, scope: &VarScope, line: usize) -> Result<Vec<String>, String> {
    let expanded = expand_arch_expression(raw, scope, line, "").map_err(|error| match error {
        MakeExprError::UnsafeVariable { name, .. } => {
            format!("source list variable {name} is conditionally assigned or otherwise unsafe")
        }
        MakeExprError::UnresolvedVariables { names, .. } => {
            format!("unresolved source list variable {}", names.join(", "))
        }
        other => other.to_string(),
    })?;
    expanded
        .split_whitespace()
        .map(|token| {
            validate_identifier(token)
                .map_err(|reason| format!("invalid source basename {token:?}: {reason}"))?;
            Ok(token.to_owned())
        })
        .collect()
}

fn expand_scalar(
    raw: &str,
    scope: &VarScope,
    line: usize,
    recipe_dir: &str,
) -> Result<String, String> {
    expand_arch_expression(raw, scope, line, recipe_dir)
        .map(|value| value.trim().to_owned())
        .map_err(|error| error.to_string())
}

fn expand_arch_expression(
    raw: &str,
    scope: &VarScope,
    line: usize,
    recipe_dir: &str,
) -> Result<String, MakeExprError> {
    // No ambient directory configuration or filesystem enumeration belongs
    // in an effect derived only from a sealed physical recipe and context.
    let dirs = crate::dirs::DirVars::default();
    let lookup = |name: &str| match name {
        "CURDIR" => Some(recipe_dir.to_owned()),
        "SRCDIR" => Some("${AROS_SOURCE_DIR}".into()),
        "TOP" => Some("${AROS_BUILD_DIR}".into()),
        "GENINCDIR" => Some("${CMAKE_BINARY_DIR}/GENINCDIR".into()),
        "GENDIR" => Some("${CMAKE_BINARY_DIR}/gen".into()),
        "AROS_INCLUDES" => Some("${AROS_SDK_INCLUDE_DIR}".into()),
        "PORTSDIR" => Some("${AROS_PORTS_DIR}".into()),
        "PORTSSOURCEDIR" => Some("${AROS_PORTS_SOURCE_DIR}".into()),
        "CPU" => Some("${AROS_TARGET_CPU}".into()),
        "ARCH" => Some("${AROS_TARGET_PLATFORM}".into()),
        "FAMILY" => Some("${AROS_TARGET_FAMILY}".into()),
        _ => None,
    };
    let guard = |name: &str| scope.flavor_uncertainty_reason_at(name, line);
    let context = MakeExprContext::new(scope, &dirs, line, Path::new("/"), Path::new(recipe_dir))
        .with_lookup(&lookup)
        .with_guard(&guard)
        .without_filesystem();
    evaluate_make_expr(raw, &context)
}

/// Parsed `includes=` value of one `%set_archincludes` producer.
struct IncludeFlags {
    /// Normalized, de-duplicated include directories.
    dirs: Vec<String>,
    /// Literal `-D` definitions without the flag prefix.
    definitions: Vec<String>,
    /// Ordered compiler arguments, repetitions and interleaving preserved.
    arguments: Vec<String>,
}

fn parse_include_flags(raw: &str, recipe_dir: &str) -> Result<IncludeFlags, String> {
    let tokens = raw.split_whitespace().collect::<Vec<_>>();
    let mut dirs = Vec::new();
    let mut definitions = Vec::new();
    let mut arguments = Vec::new();
    let mut cursor = 0usize;
    while cursor < tokens.len() {
        let token = tokens[cursor];
        if let Some(definition) = token.strip_prefix("-D") {
            validate_definition(definition)?;
            definitions.push(definition.to_owned());
            arguments.push(token.to_owned());
            cursor += 1;
            continue;
        }
        let value = if token == "-I" {
            cursor += 1;
            tokens
                .get(cursor)
                .copied()
                .ok_or_else(|| "-I is missing its path".to_owned())?
        } else if let Some(path) = token.strip_prefix("-I") {
            if path.is_empty() {
                return Err("-I is missing its path".into());
            }
            path
        } else {
            return Err(format!(
                "unsupported includes= token {token:?}; only -I paths and literal -D definitions are modeled"
            ));
        };
        let resolved = normalize_include_path(value, recipe_dir)?;
        arguments.push(format!("-I{resolved}"));
        if !dirs.contains(&resolved) {
            dirs.push(resolved);
        }
        cursor += 1;
    }
    Ok(IncludeFlags {
        dirs,
        definitions,
        arguments,
    })
}

fn validate_definition(definition: &str) -> Result<(), String> {
    let (name, value) = definition
        .split_once('=')
        .map_or((definition, None), |(name, value)| (name, Some(value)));
    let mut chars = name.chars();
    if !chars
        .next()
        .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
        || !chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        || value.is_some_and(|value| {
            !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_.+-".contains(&byte))
        })
    {
        return Err(format!("unsafe preprocessor definition {definition:?}"));
    }
    Ok(())
}

fn normalize_include_path(raw: &str, recipe_dir: &str) -> Result<String, String> {
    if raw.is_empty() || raw.contains([';', '|', '\\']) || raw.chars().any(char::is_whitespace) {
        return Err(format!("malformed include path {raw:?}"));
    }
    let roots = [
        "${AROS_SOURCE_DIR}",
        "${AROS_BUILD_DIR}",
        "${CMAKE_BINARY_DIR}",
        "${AROS_PORTS_DIR}",
        "${AROS_PORTS_SOURCE_DIR}",
    ];
    let expanded = if raw.starts_with("${") {
        raw.to_owned()
    } else if Path::new(raw).is_absolute() {
        return Err("absolute include paths are not admitted".into());
    } else if recipe_dir.is_empty() || raw == "." {
        if raw == "." {
            "${AROS_SOURCE_DIR}".into()
        } else {
            format!("${{AROS_SOURCE_DIR}}/{raw}")
        }
    } else {
        format!("${{AROS_SOURCE_DIR}}/{recipe_dir}/{raw}")
    };

    let root = roots
        .iter()
        .copied()
        .find(|root| expanded == *root || expanded.starts_with(&format!("{root}/")))
        .ok_or_else(|| format!("include path has an unsupported root: {expanded:?}"))?;
    let suffix = expanded
        .strip_prefix(root)
        .unwrap_or_default()
        .trim_start_matches('/');
    validate_path_suffix(suffix)?;
    if suffix.is_empty() {
        Ok(root.to_owned())
    } else {
        Ok(format!("{root}/{suffix}"))
    }
}

fn normalize_recipe(recipe: &Path) -> Result<String, String> {
    let raw = recipe
        .to_str()
        .ok_or_else(|| "recipe path is not UTF-8".to_owned())?;
    if raw.is_empty() || raw.contains('\\') || Path::new(raw).is_absolute() {
        return Err("recipe must be a nonempty source-relative path".into());
    }
    validate_relative_path(raw)?;
    Ok(raw.trim_matches('/').to_owned())
}

fn validate_relative_path(raw: &str) -> Result<(), String> {
    if raw.is_empty() || raw.starts_with('/') || raw.contains(['\\', ';', '|', '$']) {
        return Err("path must be a literal relative path".into());
    }
    for component in raw.split('/') {
        if component.is_empty() || component == "." || component == ".." {
            return Err("path contains an empty, dot, or parent component".into());
        }
        validate_path_component(component)?;
    }
    Ok(())
}

fn validate_path_suffix(raw: &str) -> Result<(), String> {
    if raw.is_empty() {
        return Ok(());
    }
    for component in raw.split('/') {
        if component.is_empty() || component == "." || component == ".." {
            return Err("path contains an empty, dot, or parent component".into());
        }
        validate_path_component(component)?;
    }
    Ok(())
}

fn validate_path_component(component: &str) -> Result<(), String> {
    if component
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'+'))
    {
        Ok(())
    } else {
        Err(format!("unsupported path component {component:?}"))
    }
}

fn validate_identifier(value: &str) -> Result<(), String> {
    if value.is_empty() || value == "." || value == ".." {
        return Err("identifier is empty or reserved".into());
    }
    validate_path_component(value)
}

fn deduplicate(values: &mut Vec<String>) {
    let mut seen = BTreeSet::new();
    values.retain(|value| seen.insert(value.clone()));
}

#[cfg(test)]
#[path = "arch_endpoint_context_tests.rs"]
mod context_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(content: &str, recipe: &str) -> ArchEndpointEffectScan {
        let (scope, line_states) = collect_arch_effect_scope(content);
        collect_arch_endpoint_effects(content, Path::new(recipe), &scope, Some(&line_states))
            .unwrap()
    }

    #[test]
    fn multiline_source_lists_and_directives_keep_physical_positions() {
        let input = concat!(
            "ifeq ($(UNKNOWN),1)\n",
            "IGNORED := branch-only\n",
            "endif\n",
            "SOURCES := \\\n",
            "  sdcard_esp32p4_init \\\n",
            "  sdcard_esp32p4_bus \\\n",
            "  sdcard_esp32p4_time\n",
            "%build_archspecific \\\n",
            "  mainmmake=kernel-sdcard maindir=rom/devs/sdcard \\\n",
            "  arch=esp32p4-riscv modname=sdcard \\\n",
            "  files=\"$(SOURCES)\"\n",
        );
        let result = scan(input, "arch/riscv-esp32p4/sdcard/mmakefile.src");

        assert!(result.rejected.is_empty(), "{:?}", result.rejected);
        assert_eq!(result.effects.len(), 2);
        assert!(result.effects.iter().all(|effect| effect.line == 8));
        let objects = result
            .effects
            .iter()
            .find(|effect| effect.endpoint == "kernel-sdcard-esp32p4-riscv")
            .unwrap();
        assert!(matches!(
            &objects.data,
            ArchEndpointEffectData::ArchModuleObjects { module_sources, .. }
                if module_sources == &["sdcard_esp32p4_init", "sdcard_esp32p4_bus", "sdcard_esp32p4_time"]
        ));
    }

    #[test]
    fn unknown_conditional_inside_multiline_invocation_is_rejected_with_owner() {
        let input = concat!(
            "SOURCES := \\\n",
            "  module\n",
            "ifdef UNKNOWN\n",
            "%build_archspecific \\\n",
            "  mainmmake=m maindir=rom/module arch=pc \\\n",
            "  files=\"$(SOURCES)\"\n",
            "endif\n",
        );
        let result = scan(input, "arch/all-pc/module/mmakefile.src");

        assert!(result.effects.is_empty());
        assert_eq!(result.rejected.len(), 1);
        assert_eq!(result.rejected[0].line, 4);
        assert_eq!(result.rejected[0].endpoint.as_deref(), Some("m-pc"));
        assert!(result.rejected[0]
            .reason
            .contains("conditional branch not proven active"));
    }

    #[test]
    fn assignment_after_multiline_invocation_is_not_borrowed() {
        let input = concat!(
            "PREFIX := \\\n",
            "  one \\\n",
            "  two\n",
            "%build_archspecific \\\n",
            "  mainmmake=m maindir=rom/module arch=pc \\\n",
            "  files=\"$(LATE)\"\n",
            "LATE := future\n",
        );
        let result = scan(input, "arch/all-pc/module/mmakefile.src");

        assert!(result.effects.is_empty());
        assert_eq!(result.rejected.len(), 1);
        assert_eq!(result.rejected[0].line, 4);
        assert_eq!(result.rejected[0].endpoint.as_deref(), Some("m-pc"));
        assert!(result.rejected[0]
            .reason
            .contains("unresolved source list variable LATE"));
    }

    #[test]
    fn horizontal_space_crlf_and_eof_continuations_follow_make_joining() {
        let input = concat!(
            "SOURCES := \\\t \r\n",
            "  module\r\n",
            "%build_archspecific mainmmake=m maindir=rom/module arch=pc files=\"$(SOURCES)\"\r\n",
        );
        let result = scan(input, "arch/all-pc/module/mmakefile.src");
        assert!(result.rejected.is_empty(), "{:?}", result.rejected);
        assert_eq!(result.effects.len(), 2);

        let unfinished = scan(
            "%build_archspecific \\\n",
            "arch/all-pc/module/mmakefile.src",
        );
        assert!(unfinished.effects.is_empty());
        assert_eq!(unfinished.rejected.len(), 1);
        assert_eq!(unfinished.rejected[0].endpoint, None);
    }

    #[test]
    fn literal_linklibfiles_key_does_not_collide_with_files() {
        let result = scan(
            "%build_archspecific mainmmake=m maindir=rom/module arch=pc files=module linklibfiles=\"\"\n",
            "arch/all-pc/module/mmakefile.src",
        );
        assert!(result.rejected.is_empty(), "{:?}", result.rejected);
        assert_eq!(result.effects.len(), 2);
        let effect = result
            .effects
            .iter()
            .find(|effect| effect.endpoint.ends_with("-linklib"))
            .unwrap();
        assert_eq!(effect.endpoint, "m-pc-linklib");
        assert_eq!(effect.dependencies, vec!["m-pc-includes"]);
        assert_eq!(effect.line, 1);
        assert!(matches!(
            &effect.data,
            ArchEndpointEffectData::EmptyLinklibAggregate { module_sources, .. }
                if module_sources.len() == 1 && module_sources[0] == "module"
        ));
    }

    #[test]
    fn omitted_linklib_lists_use_the_template_empty_defaults() {
        let result = scan(
            "%build_archspecific mainmmake=m maindir=rom/module arch=pc files=module\n",
            "arch/all-pc/module/mmakefile.src",
        );
        assert!(result.rejected.is_empty(), "{:?}", result.rejected);
        assert_eq!(result.effects.len(), 2);
        assert!(result
            .effects
            .iter()
            .any(|effect| effect.endpoint == "m-pc-linklib"));
        let objects = result
            .effects
            .iter()
            .find(|effect| effect.endpoint == "m-pc")
            .unwrap();
        assert!(
            matches!(&objects.data, ArchEndpointEffectData::ArchModuleObjects { directory, module_sources, .. }
            if directory == "arch/all-pc/module" && module_sources == &["module"])
        );
    }

    #[test]
    fn nonempty_linklib_object_lanes_are_not_admitted() {
        for argument in ["linklibfiles=obj", "linklibobjs=obj.o"] {
            let input =
                format!("%build_archspecific mainmmake=m arch=pc files=module {argument}\n");
            let result = scan(&input, "arch/all-pc/module/mmakefile.src");
            assert!(result.effects.is_empty(), "{argument}");
            assert_eq!(result.rejected.len(), 1, "{argument}");
        }
    }

    #[test]
    fn variable_linklib_empty_is_not_literal_empty_proof() {
        let input = "EMPTY :=\n%build_archspecific mainmmake=m arch=pc files=module linklibfiles=$(EMPTY)\n";
        let result = scan(input, "arch/all-pc/module/mmakefile.src");
        assert!(result.effects.is_empty());
        assert_eq!(result.rejected.len(), 1);
        assert!(
            result.rejected[0].reason.contains("literal empty"),
            "{:?}",
            result.rejected
        );
    }

    #[test]
    fn unknown_linklib_lists_fail_closed() {
        for argument in [
            "linklibfiles=$(UNKNOWN_LINKLIBFILES)",
            "linklibobjs=$(UNKNOWN_LINKLIBOBJS)",
        ] {
            let input = format!(
                "%build_archspecific mainmmake=m maindir=rom/module arch=pc files=module {argument}\n"
            );
            let result = scan(&input, "arch/all-pc/module/mmakefile.src");
            assert!(result.effects.is_empty(), "{argument}");
            assert_eq!(result.rejected.len(), 1, "{argument}");
            assert!(
                result.rejected[0].reason.contains("literal empty"),
                "{argument}: {:?}",
                result.rejected
            );
        }
    }

    #[test]
    fn unknown_module_source_list_is_not_admitted() {
        let result = scan(
            "%build_archspecific mainmmake=m maindir=rom/module arch=pc files=$(UNKNOWN_FILES)\n",
            "arch/all-pc/module/mmakefile.src",
        );
        assert!(result.effects.is_empty());
        assert_eq!(result.rejected.len(), 1);
        assert!(result.rejected[0]
            .reason
            .contains("unresolved source list variable"));
        assert_eq!(result.rejected[0].endpoint.as_deref(), Some("m-pc"));
    }

    #[test]
    fn invalid_or_unknown_compiler_fails_closed() {
        for compiler in ["compiler=bogus", "compiler=$(UNKNOWN_COMPILER)"] {
            let input = format!(
                "%build_archspecific mainmmake=m maindir=rom/module arch=pc files=module {compiler}\n"
            );
            let result = scan(&input, "arch/all-pc/module/mmakefile.src");
            assert!(result.effects.is_empty(), "{compiler}");
            assert_eq!(result.rejected.len(), 1, "{compiler}");
            assert!(
                result.rejected[0].reason.contains("compiler="),
                "{compiler}: {:?}",
                result.rejected
            );
        }
    }

    #[test]
    fn arch_expression_resolves_to_one_tag_or_stays_unowned() {
        let input = "%build_archspecific mainmmake=m maindir=rom/module arch=$(AROS_TARGET_CPU) asmfiles=\"$(AROS_TARGET_CPU)/stackswap\"\n";
        let recipe = Path::new("rom/module/mmakefile.src");
        let riscv = crate::TargetContext {
            cpu: Some("riscv".into()),
            ..crate::TargetContext::default()
        };
        let (scope, states) = collect_arch_effect_scope_with_context(input, Some(&riscv));
        let result = collect_arch_endpoint_effects(input, recipe, &scope, Some(&states)).unwrap();
        // The tag resolves, so the rejection names its owner.
        assert_eq!(result.rejected.len(), 1);
        assert_eq!(result.rejected[0].endpoint.as_deref(), Some("m-riscv"));

        let (scope, states) = collect_arch_effect_scope(input);
        let result = collect_arch_endpoint_effects(input, recipe, &scope, Some(&states)).unwrap();
        assert_eq!(result.rejected.len(), 1);
        assert_eq!(result.rejected[0].endpoint, None);
    }

    #[test]
    fn kernel_lane_compiles_in_target_role_only_when_the_contract_declares_it() {
        let input = "%build_archspecific mainmmake=m maindir=rom/module arch=pc files=module compiler=kernel\n";
        let recipe = Path::new("arch/all-pc/module/mmakefile.src");
        let (scope, states) = collect_arch_effect_scope(input);
        for (declared, groups) in [(false, 0), (true, 1)] {
            let result = collect_arch_endpoint_effects_at_positions(
                input,
                recipe,
                &scope,
                Some(&states),
                None,
                declared,
            )
            .unwrap();
            assert_eq!(
                result
                    .effects
                    .iter()
                    .filter(|effect| matches!(
                        effect.data,
                        ArchEndpointEffectData::ArchModuleObjects { .. }
                    ))
                    .count(),
                groups,
                "declared={declared}"
            );
        }
        // Host sources never get a target-role object group.
        let host = input.replace("compiler=kernel", "compiler=host");
        let (scope, states) = collect_arch_effect_scope(&host);
        let result = collect_arch_endpoint_effects_at_positions(
            &host,
            recipe,
            &scope,
            Some(&states),
            None,
            true,
        )
        .unwrap();
        assert!(!result.effects.iter().any(|effect| matches!(
            effect.data,
            ArchEndpointEffectData::ArchModuleObjects { .. }
        )));
    }

    #[test]
    fn host_and_kernel_lanes_do_not_invent_target_compilation_effects() {
        for compiler in ["host", "kernel"] {
            let input = format!("%build_archspecific mainmmake=m maindir=rom/module arch=pc files=module compiler={compiler}\n");
            let result = scan(&input, "arch/all-pc/module/mmakefile.src");
            assert!(result.rejected.is_empty());
            assert_eq!(result.effects.len(), 1);
            assert!(
                matches!(&result.effects[0].data, ArchEndpointEffectData::EmptyLinklibAggregate { compiler: value, .. } if value == compiler)
            );
        }
    }

    #[test]
    fn custom_object_root_and_unselected_architecture_are_not_inferred() {
        let result = scan("%build_archspecific mainmmake=m maindir=rom/module arch=pc files=module objdir=custom\n", "arch/all-pc/module/mmakefile.src");
        assert!(result.effects.is_empty());
        assert_eq!(result.rejected.len(), 1);
        let result = scan(
            "%build_archspecific mainmmake=m maindir=rom/module arch=pc files=module\n",
            "arch/all-pc/module/mmakefile.src",
        );
        let context = crate::TargetContext {
            cpu: Some("riscv".into()),
            platform: Some("esp32p4".into()),
            ..crate::TargetContext::default()
        };
        assert!(result
            .effects
            .iter()
            .all(|effect| !effect.applies_to(&context)));
    }

    #[test]
    fn set_archincludes_captures_exact_endpoint_and_generated_output() {
        let result = scan(
            "%set_archincludes mainmmake=kernel-exec maindir=rom/exec modname=exec pri=5 arch=riscv includes=\"-I$(SRCDIR)/$(CURDIR)\"\n",
            "arch/riscv-all/exec/mmakefile.src",
        );
        assert!(result.rejected.is_empty(), "{:?}", result.rejected);
        assert_eq!(result.effects.len(), 1);
        let effect = &result.effects[0];
        assert_eq!(effect.endpoint, "kernel-exec-riscv-set-archincludes");
        assert!(effect.dependencies.is_empty());
        assert_eq!(effect.line, 1);
        assert!(matches!(
            &effect.data,
            ArchEndpointEffectData::SetArchIncludes {
                include_dirs,
                generated_file,
                order_only_directory,
                priority: 5,
                ..
            } if include_dirs == &["${AROS_SOURCE_DIR}/arch/riscv-all/exec"]
                && generated_file == "gen/rom/exec/exec/include/.exec.includeflag.5.riscv"
                && order_only_directory == "gen/rom/exec/exec/include"
        ));
    }

    #[test]
    fn malformed_paths_and_macro_arguments_fail_closed() {
        let unsafe_path = scan(
            "%set_archincludes mainmmake=m maindir=../rom modname=exec pri=5 arch=riscv includes=\"-I$(SRCDIR)/$(CURDIR)\"\n",
            "arch/riscv-all/exec/mmakefile.src",
        );
        assert!(unsafe_path.effects.is_empty());
        assert_eq!(unsafe_path.rejected.len(), 1);

        let malformed = scan(
            "%set_archincludes mainmmake=m mainmmake=n maindir=rom/exec modname=exec pri=5 arch=riscv\n",
            "arch/riscv-all/exec/mmakefile.src",
        );
        assert!(malformed.effects.is_empty());
        assert_eq!(malformed.rejected.len(), 1);
        assert_eq!(malformed.rejected[0].endpoint, None);
    }

    #[test]
    fn unknown_include_variable_is_not_treated_as_an_empty_path() {
        let result = scan(
            "%set_archincludes mainmmake=m maindir=rom/exec modname=exec pri=5 arch=riscv includes=\"-I$(UNKNOWN_INCLUDE)\"\n",
            "arch/riscv-all/exec/mmakefile.src",
        );
        assert!(result.effects.is_empty());
        assert_eq!(result.rejected.len(), 1);
        assert!(
            result.rejected[0]
                .reason
                .contains("unresolved Make variable"),
            "{:?}",
            result.rejected
        );
        assert_eq!(
            result.rejected[0].endpoint.as_deref(),
            Some("m-riscv-set-archincludes")
        );
    }
}
