//! Source-location, rejection and native-scope helpers for the MetaMake pipeline.

use super::{
    capability_diagnostic, capability_diagnostic_for_target, collect_vars_impl, evaluate_name,
    exact_mmake_target, is_concrete_build_invocation, join_continuations, macro_arg, Diagnostic,
    Invocation, MakeExprContext, Path, Regex, TargetContext,
};

pub(super) fn capability_diagnostic_with_owner(
    relative_path: &Path,
    line: Option<usize>,
    owner: Option<&str>,
    message: impl Into<String>,
) -> Diagnostic {
    match owner {
        Some(owner) => capability_diagnostic_for_target(relative_path, line, owner, message),
        None => capability_diagnostic(relative_path, line, message),
    }
}

/// A rejected source-written MetaMake provider may retain its already
/// rendered selector spelling. This proves ownership, not executability;
/// graph selection must still bind every selector or reject the endpoint.
pub(super) fn source_meta_provider_diagnostic(
    relative_path: &Path,
    owner: &str,
    reason: String,
) -> Diagnostic {
    let mut residual = owner.to_owned();
    for selector in [
        "AROS_TARGET_CPU",
        "AROS_TARGET_PLATFORM",
        "AROS_TARGET_LEGACY_PLATFORM",
        "AROS_TARGET_FAMILY",
        "AROS_TARGET_VARIANT",
        "AROS_TARGET_CPU32",
    ] {
        residual = residual.replace(&format!("${{{selector}}}"), "selector");
    }
    let mut diagnostic = capability_diagnostic(relative_path, None, reason);
    if !residual.is_empty()
        && residual
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        diagnostic = diagnostic.with_context(aros_common::DiagnosticContext {
            target: Some(owner.to_owned()),
            ..Default::default()
        });
    }
    diagnostic
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LiteralObjectSourceAnchor {
    pub(super) source: std::path::PathBuf,
    pub(super) line: usize,
}

pub(super) fn rejection_source_location<'a>(
    fallback: &'a Path,
    anchors: Option<&'a [Option<LiteralObjectSourceAnchor>]>,
    joined_line: usize,
) -> (&'a Path, Option<usize>) {
    joined_line
        .checked_sub(1)
        .and_then(|index| anchors.and_then(|anchors| anchors.get(index)))
        .and_then(Option::as_ref)
        .map_or((fallback, None), |anchor| {
            (anchor.source.as_path(), Some(anchor.line))
        })
}

pub(super) fn source_rejection_diagnostics(
    path: &Path,
    line: Option<usize>,
    fallback_owner: Option<&str>,
    message: String,
    proofs: Option<Vec<crate::source_rule_ownership::SourceRuleOwnership>>,
) -> Vec<Diagnostic> {
    match proofs {
        Some(proofs) if !proofs.is_empty() => proofs
            .into_iter()
            .map(|proof| {
                capability_diagnostic_with_owner(
                    path,
                    line,
                    Some(&proof.owner),
                    format!(
                        "{message}; exact source consumer chain: {}",
                        proof.chain.join(" -> ")
                    ),
                )
            })
            .collect(),
        _ => vec![capability_diagnostic_with_owner(
            path,
            line,
            fallback_owner,
            message,
        )],
    }
}

pub(super) fn rejected_rule_owner_proofs(
    owner: &str,
    configuration_is_complete: bool,
    snapshot: (&str, Option<&[crate::make_vars::ConditionalTruth]>),
    scope: &crate::make_vars::VarScope,
    dirs: &crate::dirs::DirVars,
    source_location: (&Path, &Path),
    line: usize,
) -> Option<Vec<crate::source_rule_ownership::SourceRuleOwnership>> {
    if !configuration_is_complete || exact_mmake_target(owner).is_some() {
        return None;
    }
    crate::source_rule_ownership::attribute_rejected_rule_owners(
        snapshot.0,
        scope,
        dirs,
        source_location.0,
        source_location.1,
        snapshot.1,
        line,
    )
}

#[derive(Default)]
pub(super) struct ReconstructedLiteralSource {
    pub(super) text: String,
    pub(super) physical_anchors: Vec<Option<LiteralObjectSourceAnchor>>,
}

/// Reconstructs the exact native-configuration expansion while retaining the
/// original file and physical line for every emitted line. A mismatch means
/// the include provenance cannot safely support a source location.
pub(super) fn literal_object_source_anchors(
    root: &Path,
    source: &Path,
    source_text: &str,
    scan: &crate::local_make_includes::LocalMakeIncludeScan,
    joined_snapshot: &str,
) -> Option<Vec<Option<LiteralObjectSourceAnchor>>> {
    let canonical_root = std::fs::canonicalize(root).ok()?;
    let mut next_fragment = 0;
    let reconstructed = reconstruct_literal_source(
        &canonical_root,
        source,
        source_text,
        &scan.fragments,
        &mut next_fragment,
    )?;
    if next_fragment != scan.fragments.len() || reconstructed.text != scan.expanded {
        return None;
    }

    let joined = join_continuations(&reconstructed.text);
    if joined != joined_snapshot {
        return None;
    }

    let physical_lines = reconstructed.text.split_inclusive('\n').collect::<Vec<_>>();
    if physical_lines.len() != reconstructed.physical_anchors.len() {
        return None;
    }

    // Keep this expression in lockstep with parser::join_continuations. It
    // tells us which adjacent physical lines became one parser line; the
    // joined bytes themselves are independently checked above.
    let continuation = Regex::new(r"\\[ \t]*\r?\n[ \t]*").ok()?;
    let mut logical_anchors = Vec::new();
    for (index, physical_line) in physical_lines.iter().enumerate() {
        if index == 0 {
            logical_anchors.push(reconstructed.physical_anchors[index].clone());
            continue;
        }
        let previous_line = physical_lines[index - 1];
        let boundary = previous_line.len();
        let mut pair = String::with_capacity(boundary + physical_line.len());
        pair.push_str(previous_line);
        pair.push_str(physical_line);
        let is_continuation = continuation
            .find_iter(&pair)
            .any(|matched| matched.start() < boundary && matched.end() >= boundary);
        if !is_continuation {
            logical_anchors.push(reconstructed.physical_anchors[index].clone());
        }
    }
    if logical_anchors.len() != joined_snapshot.lines().count() {
        return None;
    }
    Some(logical_anchors)
}

/// Evaluate `%make_package`/`%link_kickstart` in the selected native scope,
/// with diagnostics reported at physical source lines.
pub(super) fn collect_native_packages(
    content: &str,
    target: &TargetContext,
    dirs: &crate::dirs::DirVars,
    root: &Path,
    relative_path: &Path,
    rel_dir: &Path,
) -> (Vec<crate::packages::PackageDecl>, Vec<String>) {
    let snapshot = match crate::assembly_headers::native_configuration_snapshot(
        content,
        target,
        dirs,
        root,
        relative_path,
    ) {
        Ok(snapshot) => snapshot,
        Err(reason) => {
            return (
                Vec::new(),
                vec![format!(
                    "{}: native package context is unproven: {reason}",
                    relative_path.display()
                )],
            )
        }
    };
    let (native_scope, native_states) = collect_vars_impl(&snapshot.joined, Some(target));
    let (packages, skipped) = crate::packages::collect_packages_with_scope(
        &snapshot.joined,
        rel_dir,
        &native_scope,
        dirs,
        root,
        &native_states,
    );
    // Messages carry native scope lines; report physical ones.
    let file_prefix = format!("{}:", relative_path.display());
    let skipped = skipped
        .into_iter()
        .map(|message| {
            let physical = message
                .strip_prefix(&file_prefix)
                .and_then(|rest| rest.split_once(':'))
                .and_then(|(line, tail)| {
                    let line = line.parse::<usize>().ok()?.checked_sub(1)?;
                    let physical = snapshot.physical_owner_lines.get(line).copied().flatten()?;
                    Some(format!("{file_prefix}{}:{tail}", physical + 1))
                });
            physical.unwrap_or(message)
        })
        .collect();
    (packages, skipped)
}

/// Map physical declaration starts to an independently reconstructed, inlined
/// scope. Continuation tails and inserted configuration lines never acquire
/// the identity of a physical declaration in the invoking recipe.
pub fn architecture_scope_positions(
    root: &Path,
    source: &Path,
    source_text: &str,
    scan: &crate::local_make_includes::LocalMakeIncludeScan,
    joined: &str,
) -> Option<Vec<Option<usize>>> {
    let anchors = literal_object_source_anchors(root, source, source_text, scan, joined)?;
    let mut positions = vec![None; source_text.lines().count()];
    for (position, anchor) in anchors.into_iter().enumerate() {
        if let Some(anchor) = anchor.filter(|anchor| anchor.source == source) {
            let slot = positions.get_mut(anchor.line.checked_sub(1)?)?;
            if slot.replace(position).is_some() {
                return None;
            }
        }
    }
    Some(positions)
}

pub(super) fn reconstruct_literal_source(
    root: &Path,
    source: &Path,
    source_text: &str,
    fragments: &[crate::local_make_includes::IncludedLocalMakeFragment],
    next_fragment: &mut usize,
) -> Option<ReconstructedLiteralSource> {
    let mut reconstructed = ReconstructedLiteralSource::default();
    for (index, chunk) in source_text.split_inclusive('\n').enumerate() {
        let line = index + 1;
        let fragment = fragments.get(*next_fragment);
        if let Some(fragment) = fragment {
            if fragment.included_from == source && fragment.include_line < line {
                return None;
            }
            if fragment.included_from == source && fragment.include_line == line {
                if fragment.path.is_absolute()
                    || fragment
                        .path
                        .components()
                        .any(|component| !matches!(component, std::path::Component::Normal(_)))
                {
                    return None;
                }
                let fragment_path = fragment.path.clone();
                *next_fragment += 1;
                let resolved_fragment_path = root.join(&fragment_path);
                if std::fs::canonicalize(&resolved_fragment_path)
                    .ok()
                    .as_deref()
                    != Some(resolved_fragment_path.as_path())
                    || !std::fs::metadata(&resolved_fragment_path).ok()?.is_file()
                {
                    return None;
                }
                reconstructed
                    .text
                    .push_str(if fragment.generated_output.is_some() {
                        "# Verified source configuration template\n"
                    } else {
                        "# Verified source configuration include\n"
                    });
                reconstructed.physical_anchors.push(None);

                let mut fragment_text = std::fs::read_to_string(&resolved_fragment_path).ok()?;
                if let Some(substitutions) = &fragment.template_substitutions {
                    fragment.generated_output.as_ref()?;
                    for (token, value) in substitutions {
                        if token.is_empty() || !fragment_text.contains(token) {
                            return None;
                        }
                        fragment_text = fragment_text.replace(token, value);
                    }
                }
                let expanded = reconstruct_literal_source(
                    root,
                    &fragment_path,
                    &fragment_text,
                    fragments,
                    next_fragment,
                )?;
                let has_text = !expanded.text.is_empty();
                let ends_with_newline = expanded.text.ends_with('\n');
                reconstructed.text.push_str(&expanded.text);
                reconstructed
                    .physical_anchors
                    .extend(expanded.physical_anchors);
                if has_text && !ends_with_newline {
                    reconstructed.text.push('\n');
                }
                continue;
            }
        }
        reconstructed.text.push_str(chunk);
        reconstructed
            .physical_anchors
            .push(Some(LiteralObjectSourceAnchor {
                source: source.to_path_buf(),
                line,
            }));
    }
    if fragments
        .get(*next_fragment)
        .is_some_and(|fragment| fragment.included_from == source)
    {
        return None;
    }
    Some(reconstructed)
}

pub(in crate::parser) fn invocation_owner_registry(
    invocations: &[Invocation],
    scope: &crate::make_vars::VarScope,
    dirs: &crate::dirs::DirVars,
    root: &Path,
    rel_dir: &Path,
) -> std::collections::BTreeMap<String, usize> {
    let mut owners = std::collections::BTreeMap::new();
    for invocation in invocations
        .iter()
        .filter(|invocation| is_concrete_build_invocation(&invocation.name))
    {
        let Some(raw) = macro_arg(&invocation.args, "mmake") else {
            continue;
        };
        let expression_context = MakeExprContext::new(scope, dirs, invocation.line, root, rel_dir);
        let Some(owner) = evaluate_name(&raw, &expression_context)
            .ok()
            .and_then(|name| exact_mmake_target(&name))
        else {
            continue;
        };
        *owners.entry(owner).or_insert(0) += 1;
    }
    owners
}
