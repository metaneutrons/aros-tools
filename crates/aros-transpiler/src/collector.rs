//! Make includes inlined for the collectors, and the port scope that follows.
//!
//! `%fetch` and `%copy_includes` read a variable scope, and a mmakefile may put
//! the values they need behind an `include`. Inlining those includes for the
//! collectors -- and only for them -- keeps the declarations owned by their
//! original file, which is what stops a shared fragment from manufacturing the
//! same target in every file that includes it.
//!
//! The supported path form is bounded on purpose: `SRCDIR` and `CURDIR` only,
//! with `CURDIR` staying the original mmakefile's directory, because Make
//! resolves a relative include against its own source root rather than against
//! the including file.
//!
//! The port scope is the second half. A declaration whose sources all belong to
//! a fetched archive is a port, and it is compiled against that archive's
//! layout rather than the tree's; deciding that needs the fetches, which is why
//! it sits here rather than with the ordinary source resolution.

use crate::includes::collect_includes_at;
use crate::local_make_includes::LocalMakeIncludeScan;
use crate::make_expr::MakeExprContext;
use crate::make_vars::{
    collect_vars_impl_with_forward_locals, strip_make_comment, variable_assignment,
};
use crate::parser::{
    is_concrete_build_invocation, join_continuations, select_target_invocations, TargetContext,
};
use crate::sources::{evaluate_macro_sources, EvaluatedSources};
use aros_common::read_source;
use std::collections::HashSet;
use std::fs;
use std::path::Path;

/// Inlines source-tree Make includes for collector variable evaluation.
///
/// Build declarations remain owned by their original mmakefile.  Only the
/// variable scope used by `%fetch` and `%copy_includes` sees these files; this
/// avoids manufacturing duplicate targets from common included fragments.
/// The supported path form is deliberately bounded to paths made concrete by
/// `SRCDIR` and `CURDIR`.  Includes rooted in the build or fetched sources stay
/// deferred and continue to be reported by the collector that needs them.
/// `CURDIR` remains the original mmakefile directory through recursion, and a
/// relative include stays relative to Make's source/build root rather than to
/// the directory of the including file.
pub(crate) fn inline_collector_make_includes(
    content: &str,
    root: &Path,
    mmake_curdir: &Path,
    visited: &mut HashSet<std::path::PathBuf>,
    depth: usize,
) -> String {
    if depth == 0 {
        return content.to_owned();
    }

    let root_abs = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let rel_text = mmake_curdir.to_string_lossy().replace('\\', "/");
    let mut output = String::with_capacity(content.len());
    for line in content.lines() {
        output.push_str(line);
        output.push('\n');

        let trimmed = line.trim();
        let path_text = trimmed
            .strip_prefix("-include ")
            .or_else(|| trimmed.strip_prefix("include "))
            .map(str::trim);
        let Some(path_text) = path_text else { continue };
        if path_text.is_empty() || path_text.split_whitespace().count() != 1 {
            continue;
        }

        let expanded = path_text
            .replace("$(SRCDIR)", &root_abs.to_string_lossy())
            .replace("$(CURDIR)", &rel_text);
        if expanded.contains('$') {
            continue;
        }
        let candidate = Path::new(&expanded);
        let candidate = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            // GNU Make resolves an include without a directory against its
            // invocation working directory. It does not switch to the
            // directory of the file which contained the include.
            root_abs.join(candidate)
        };
        let Ok(candidate) = fs::canonicalize(candidate) else {
            continue;
        };
        if !candidate.starts_with(&root_abs) || !visited.insert(candidate.clone()) {
            continue;
        }
        let Ok(included) = read_source(&candidate) else {
            continue;
        };
        output.push_str(&inline_collector_make_includes(
            &included,
            &root_abs,
            mmake_curdir,
            visited,
            depth - 1,
        ));
    }
    output
}

/// Marks forward local variables without defining them.  GNU Make expands an
/// as-yet undefined local to the empty string, which is how option defaults
/// such as Mesa's `ifeq ($(OPT_MESAGL),)` become decidable.  The parser already
/// treats commented assignments as local-name declarations, so `?=` retains
/// its correct undefined-variable behaviour.
pub(crate) fn collector_forward_local_prelude(content: &str) -> String {
    let mut names = HashSet::new();
    for line in content.lines() {
        let line = strip_make_comment(line);
        if let Some((name, _, _)) = variable_assignment(line) {
            names.insert(name.to_owned());
        }
    }
    let mut names: Vec<_> = names.into_iter().collect();
    names.sort();
    let mut prelude = String::new();
    for name in names {
        prelude.push_str("# ");
        prelude.push_str(&name);
        prelude.push_str(" =\n");
    }
    prelude
}

/// Whether a broad-but-syntactically-safe local variable fragment may be used
/// for this mmakefile's concrete declarations.
///
/// This is deliberately stricter than enabling `SafeVariableScopes` for every
/// file. The candidate is accepted only for a concrete target profile, only
/// when every build declaration resolves to sources owned by a `%fetch` from
/// the same mmakefile, and only when it makes at least one previously
/// unresolved declaration concrete. Consequently a safe-looking configuration
/// fragment cannot silently affect an unrelated in-tree target or imply an
/// unmodelled source owner.
pub(crate) fn owns_fetched_source(source: &str, fetches: &[crate::fetch::FetchDecl]) -> bool {
    const PORTS_ROOT: &str = "${AROS_PORTS_DIR}";
    if source == PORTS_ROOT || !source.starts_with("${AROS_PORTS_DIR}/") {
        return false;
    }
    fetches.iter().any(|fetch| {
        let destination = fetch.destination.trim_end_matches('/');
        (destination == PORTS_ROOT || destination.starts_with("${AROS_PORTS_DIR}/"))
            && (source == destination
                || source
                    .strip_prefix(destination)
                    .is_some_and(|tail| tail.starts_with('/')))
    })
}

pub(crate) fn all_sources_are_fetch_owned(
    sources: &EvaluatedSources,
    fetches: &[crate::fetch::FetchDecl],
) -> bool {
    !sources.is_empty()
        && sources
            .c
            .iter()
            .chain(&sources.cxx)
            .chain(&sources.objc)
            .chain(&sources.asm)
            .all(|source| owns_fetched_source(source, fetches))
}

pub(crate) fn declaration_owned_port_scope(
    plain: &LocalMakeIncludeScan,
    candidate: &LocalMakeIncludeScan,
    target: Option<&TargetContext>,
    dirs: &crate::dirs::DirVars,
    root: &Path,
    rel_dir: &Path,
    fetches: &[crate::fetch::FetchDecl],
) -> bool {
    let Some(target) = target else {
        return false;
    };
    if candidate.expanded == plain.expanded
        || candidate.fragments.is_empty()
        || !candidate.issues.is_empty()
        || fetches.is_empty()
    {
        return false;
    }

    let plain_joined = join_continuations(&plain.expanded);
    let candidate_joined = join_continuations(&candidate.expanded);
    let (plain_scope, plain_states) =
        collect_vars_impl_with_forward_locals(&plain_joined, Some(target), true);
    let (candidate_scope, candidate_states) =
        collect_vars_impl_with_forward_locals(&candidate_joined, Some(target), true);
    let mut ignored = Vec::new();
    let plain_invocations =
        select_target_invocations(&plain_joined, Some(&plain_states), rel_dir, &mut ignored)
            .into_iter()
            .filter(|invocation| is_concrete_build_invocation(&invocation.name))
            .collect::<Vec<_>>();
    ignored.clear();
    let candidate_invocations = select_target_invocations(
        &candidate_joined,
        Some(&candidate_states),
        rel_dir,
        &mut ignored,
    )
    .into_iter()
    .filter(|invocation| is_concrete_build_invocation(&invocation.name))
    .collect::<Vec<_>>();

    if candidate_invocations.is_empty()
        || candidate_invocations.len() != plain_invocations.len()
        || candidate_invocations
            .iter()
            .zip(&plain_invocations)
            .any(|(candidate, plain)| candidate.name != plain.name || candidate.args != plain.args)
    {
        return false;
    }

    let mut newly_resolved = false;
    for (candidate_invocation, plain_invocation) in
        candidate_invocations.iter().zip(&plain_invocations)
    {
        let candidate_vars = candidate_scope.snapshot(candidate_invocation.line);
        let candidate_context = MakeExprContext::new(
            &candidate_scope,
            dirs,
            candidate_invocation.line,
            root,
            rel_dir,
        );
        let Ok(candidate_sources) = evaluate_macro_sources(
            &candidate_invocation.args,
            &candidate_vars,
            &candidate_context,
        ) else {
            return false;
        };
        if !candidate_sources.declared || candidate_sources.is_empty() {
            return false;
        }
        let plain_vars = plain_scope.snapshot(plain_invocation.line);
        let plain_context =
            MakeExprContext::new(&plain_scope, dirs, plain_invocation.line, root, rel_dir);
        let plain_sources =
            evaluate_macro_sources(&plain_invocation.args, &plain_vars, &plain_context).ok();
        for source in candidate_sources
            .c
            .iter()
            .chain(&candidate_sources.cxx)
            .chain(&candidate_sources.objc)
            .chain(&candidate_sources.asm)
        {
            if source.starts_with("${AROS_PORTS_DIR}/") {
                if !owns_fetched_source(source, fetches) {
                    return false;
                }
                let existed = plain_sources.as_ref().is_some_and(|sources| {
                    sources
                        .c
                        .iter()
                        .chain(&sources.cxx)
                        .chain(&sources.objc)
                        .chain(&sources.asm)
                        .any(|plain| plain == source)
                });
                newly_resolved |= !existed;
            }
        }

        let candidate_includes = collect_includes_at(
            &candidate_joined,
            &candidate_scope,
            candidate_invocation.line,
            rel_dir,
        );
        if !candidate_includes.unresolved.is_empty() {
            return false;
        }
        let plain_includes =
            collect_includes_at(&plain_joined, &plain_scope, plain_invocation.line, rel_dir);
        for directory in &candidate_includes.dirs {
            if directory.starts_with("${AROS_PORTS_DIR}/") {
                if !owns_fetched_source(directory, fetches) {
                    return false;
                }
                newly_resolved |= !plain_includes.dirs.contains(directory);
            }
        }
    }
    newly_resolved
}
