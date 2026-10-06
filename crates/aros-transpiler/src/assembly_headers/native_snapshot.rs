//! Native configuration snapshot: include expansion and architecture include handling.

use super::{
    collect_vars_impl, evaluate_make_expr, expression_context, join_continuations_with_origins,
    line_state, parse_compiler_arguments, physical_line, strip_make_comment, BTreeSet, Component,
    ConditionalTruth, DirVars, Path, TargetContext, VarScope,
};

pub(super) struct PreparedNativeScope {
    /// Joined native Make scope after explicit includes and arch include
    /// invocations have been expanded. Source lines retain their mapped slots.
    pub(super) joined: String,
    /// Rule-only view with the same joined line positions. Inserted include
    /// contents are blank, so they can contribute variables but never owners.
    pub(super) rule_view: String,
    /// For each joined scope line, the corresponding zero-based physical
    /// source line. Inserted configuration lines inherit their include line.
    pub(super) physical_lines: Vec<usize>,
    /// Exact physical owner of each joined line, without inheritance.
    physical_owner_lines: Vec<Option<usize>>,
    /// Architecture-provided `-D` argv which may appear through
    /// `PRIV_EXEC_INCLUDES` despite that Make variable normally being
    /// include-only.
    pub(super) arch_definitions: Vec<String>,
    pub(super) scope: VarScope,
    pub(super) line_states: Vec<ConditionalTruth>,
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
pub fn native_configuration_snapshot(
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
pub struct NativeConfigurationSnapshot {
    /// Joined scope text after explicit includes and arch include invocations.
    pub(crate) joined: String,
    /// For each joined line, the zero-based physical recipe line that starts
    /// there. Continuation tails and inserted configuration lines are `None`
    /// and can never own a declaration.
    pub(crate) physical_owner_lines: Vec<Option<usize>>,
}

pub(super) fn prepare_native_scope(
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
