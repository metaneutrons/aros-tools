//! Closed model for explicit, finite file copies into configured Developer
//! output directories.
//!
//! This capability does not expand directories, interpret globs, or run the
//! legacy copy macro. It preserves only a named `%copy_files_q` declaration
//! whose inputs are either proven below one local `%fetch` destination or are
//! ordinary files in the selected source tree. Developer lib/bin/man copies
//! additionally require a finite source-owned
//! file list and a local MetaMake consumer.

use crate::dirs::DirVars;
use crate::fetch::FetchDecl;
use crate::make_expr::{evaluate_make_expr, evaluate_make_list, MakeExprContext};
use crate::make_vars::{ConditionalTruth, VarScope};
use crate::parser::{macro_arg, macro_argument_names, Invocation};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path};

const FD_DIRECTORY_ALIAS: &str = "${AROS_DEVELOPER_FD_DIR}";
const DEVELOPER_LIB_DIRECTORY_ALIAS: &str = "${AROS_DEVELOPER_LIB_DIR}";
const DEVELOPER_BIN_DIRECTORY_ALIAS: &str = "${AROS_DEVELOPER_BIN_DIR}";
const DEVELOPER_MAN1_DIRECTORY_ALIAS: &str = "${AROS_DEVELOPER_MAN1_DIR}";

/// One exact, source-derived file copy into a configured Developer directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkFileCopyDecl {
    /// Named Make target owning this copy.
    pub owner: String,
    /// Source-relative mmakefile path.
    pub file: String,
    /// One-based line of `%copy_files_q`.
    pub line: usize,
    /// Resolved source directory, rooted in the source tree or fetched ports.
    pub source_dir: String,
    /// Configured Developer destination directory, normalized for CMake.
    pub destination: String,
    /// Exact, finite list of safe basenames to copy.
    pub files: Vec<String>,
    /// Unique local `%fetch` target when the source is fetched.
    pub fetch_owner: Option<String>,
}

/// A relevant file-copy declaration that does not fit the closed model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkFileCopyRejection {
    /// Best available named Make owner, or `<unknown>`.
    pub owner: String,
    /// Source-relative mmakefile path.
    pub file: String,
    /// One-based line of the declaration.
    pub line: usize,
    /// Why the declaration was refused.
    pub reason: String,
}

#[derive(Debug)]
struct MetaEdge {
    owner: String,
    prerequisites: Vec<String>,
    state: ConditionalTruth,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CopyDestination {
    DeveloperSdkFd,
    DeveloperLib,
    DeveloperBin,
    DeveloperMan1,
}

impl CopyDestination {
    const fn alias(self) -> &'static str {
        match self {
            Self::DeveloperSdkFd => FD_DIRECTORY_ALIAS,
            Self::DeveloperLib => DEVELOPER_LIB_DIRECTORY_ALIAS,
            Self::DeveloperBin => DEVELOPER_BIN_DIRECTORY_ALIAS,
            Self::DeveloperMan1 => DEVELOPER_MAN1_DIRECTORY_ALIAS,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::DeveloperSdkFd => "SDK fd copy",
            Self::DeveloperLib => "Developer lib copy",
            Self::DeveloperBin => "Developer bin copy",
            Self::DeveloperMan1 => "Developer man copy",
        }
    }

    const fn uses_evaluated_file_list(self) -> bool {
        matches!(
            self,
            Self::DeveloperLib | Self::DeveloperBin | Self::DeveloperMan1
        )
    }

    const fn file_list_label(self) -> &'static str {
        match self {
            Self::DeveloperSdkFd => "SDK fd",
            Self::DeveloperLib => "Developer lib",
            Self::DeveloperBin => "Developer bin",
            Self::DeveloperMan1 => "Developer man",
        }
    }
}

/// Collects `%copy_files_q` declarations targeting a supported configured
/// Developer directory. `line_states` uses the zero-based, continuation-joined
/// source coordinates used by [`Invocation::line`].
#[must_use]
#[cfg(test)]
pub(crate) fn collect(
    invocations: &[Invocation],
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
    line_states: Option<&[ConditionalTruth]>,
    fetches: &[FetchDecl],
) -> (Vec<SdkFileCopyDecl>, Vec<SdkFileCopyRejection>) {
    let path = root.join(rel_dir).join("mmakefile.src");
    let source = aros_common::read_source(&path)
        .expect("test fixture source mmakefile should exist and be readable");
    let source_snapshot = crate::parser::join_continuations(&source);
    collect_from_snapshot(
        invocations,
        scope,
        dirs,
        root,
        rel_dir,
        line_states,
        fetches,
        &source_snapshot,
    )
}

/// Collects copy declarations using the same joined source snapshot from
/// which `invocations`, variable scope, and line states were derived.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub(crate) fn collect_from_snapshot(
    invocations: &[Invocation],
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
    line_states: Option<&[ConditionalTruth]>,
    fetches: &[FetchDecl],
    source_snapshot: &str,
) -> (Vec<SdkFileCopyDecl>, Vec<SdkFileCopyRejection>) {
    let file = match source_relative_file(rel_dir) {
        Ok(file) => file,
        Err(reason) => {
            return (
                Vec::new(),
                vec![SdkFileCopyRejection {
                    owner: "<unknown>".into(),
                    file: rel_dir.to_string_lossy().replace('\\', "/"),
                    line: 1,
                    reason,
                }],
            );
        }
    };
    let mut declarations = Vec::new();
    let mut rejections = Vec::new();
    let mut owners = BTreeSet::new();

    for invocation in invocations
        .iter()
        .filter(|invocation| invocation.name == "copy_files_q")
    {
        let context = MakeExprContext::new(scope, dirs, invocation.line, root, rel_dir);
        let raw_destination = macro_arg(&invocation.args, "dst");
        let Some(destination_kind) = raw_destination
            .as_deref()
            .and_then(|raw| classify_destination(raw, &context))
        else {
            // Other `%copy_files_q` destinations belong to other capabilities.
            continue;
        };
        if destination_kind.uses_evaluated_file_list()
            && source_snapshot
                .lines()
                .nth(invocation.line)
                .is_some_and(|line| line.starts_with('\t'))
        {
            continue;
        }

        let owner_hint = macro_arg(&invocation.args, "mmake")
            .and_then(|raw| evaluate_make_expr(&raw, &context).ok())
            .filter(|value| safe_target_name(value))
            .unwrap_or_else(|| "<unknown>".into());
        let reject = |reason: String| SdkFileCopyRejection {
            owner: owner_hint.clone(),
            file: file.clone(),
            line: invocation.line + 1,
            reason,
        };

        match state_at(line_states, invocation.line) {
            ConditionalTruth::False => continue,
            ConditionalTruth::Unknown => {
                rejections.push(reject(format!(
                    "{} is guarded by an unresolved Make conditional",
                    destination_kind.label()
                )));
                continue;
            }
            ConditionalTruth::True => {}
        }

        if destination_kind.uses_evaluated_file_list() {
            let source_lines = source_snapshot.lines().collect::<Vec<_>>();
            match source_make_control_suppression(&source_lines) {
                Ok(defined_lines)
                    if defined_lines.get(invocation.line).copied().unwrap_or(true) =>
                {
                    continue;
                }
                Ok(_) => {}
                Err(reason) => {
                    rejections.push(reject(reason));
                    continue;
                }
            }
        }

        let result = (|| -> Result<SdkFileCopyDecl, String> {
            validate_argument_shape(&invocation.args, destination_kind)?;
            let names = macro_argument_names(&invocation.args);
            let unique = names.iter().collect::<BTreeSet<_>>();
            let has_macro_arguments = match destination_kind {
                CopyDestination::DeveloperSdkFd
                | CopyDestination::DeveloperBin
                | CopyDestination::DeveloperMan1 => names.len() == 4,
                CopyDestination::DeveloperLib => {
                    (2..=4).contains(&names.len())
                        && unique.contains(&"mmake".to_owned())
                        && unique.contains(&"dst".to_owned())
                }
            };
            if names.len() != unique.len()
                || unique
                    .iter()
                    .any(|name| !matches!(name.as_str(), "mmake" | "files" | "src" | "dst"))
                || !has_macro_arguments
            {
                return Err(format!(
                    "{} has duplicate or unsupported arguments",
                    destination_kind.label()
                ));
            }

            let owner = owner_hint.clone();
            if !safe_target_name(&owner) {
                return Err(format!(
                    "{} has no safe named Make owner",
                    destination_kind.label()
                ));
            }
            if !owners.insert(owner.clone()) {
                declarations.retain(|declaration: &SdkFileCopyDecl| declaration.owner != owner);
                return Err(format!(
                    "{} owner has duplicate producer declarations",
                    destination_kind.label()
                ));
            }

            let raw_destination = raw_destination
                .as_deref()
                .ok_or("file copy has no destination")?;
            let destination_value =
                evaluate_make_expr(raw_destination, &context).map_err(|error| {
                    format!(
                        "cannot resolve {} destination: {error}",
                        destination_kind.label()
                    )
                })?;
            let (configured_expression, configured_description) = match destination_kind {
                CopyDestination::DeveloperSdkFd => (
                    "$(AROS_DEVELOPER)/$(AROS_DIR_SDK)/$(AROS_DIR_FD)",
                    "configured Developer SDK fd path",
                ),
                CopyDestination::DeveloperLib => (
                    "$(AROS_DEVELOPER)/$(AROS_DIR_LIB)",
                    "configured Developer lib path",
                ),
                CopyDestination::DeveloperBin => {
                    ("$(AROS_DEVELOPER)/bin", "configured Developer bin path")
                }
                CopyDestination::DeveloperMan1 => (
                    "$(AROS_DEVELOPER)/man/man1",
                    "configured Developer man/man1 path",
                ),
            };
            let configured_destination = evaluate_make_expr(configured_expression, &context)
                .map_err(|error| format!("cannot resolve {configured_description}: {error}"))?;
            if destination_value != configured_destination {
                return Err(format!(
                    "{} destination is not the {configured_description}",
                    destination_kind.label()
                ));
            }

            let raw_source = macro_arg(&invocation.args, "src").unwrap_or_else(|| {
                if destination_kind == CopyDestination::DeveloperLib {
                    ".".into()
                } else {
                    String::new()
                }
            });
            if raw_source.is_empty() {
                return Err(format!(
                    "{} has no explicit source directory",
                    destination_kind.label()
                ));
            }
            let source_dir =
                crate::copy_directories::render_copy_directory_path(&raw_source, &context, rel_dir)
                    .map_err(|error| format!("{} source: {error}", destination_kind.label()))?;
            let source_root_kind = if source_dir == "${AROS_SOURCE_DIR}"
                || source_dir.starts_with("${AROS_SOURCE_DIR}/")
            {
                SourceRoot::SelectedSource
            } else if source_dir.starts_with("${AROS_PORTS_DIR}/") {
                SourceRoot::FetchedPorts
            } else {
                return Err(format!(
                    "{} source must be below the selected source or fetched ports root",
                    destination_kind.label()
                ));
            };

            let files_argument = macro_arg(&invocation.args, "files");
            let raw_files = files_argument.clone().unwrap_or_else(|| {
                if destination_kind == CopyDestination::DeveloperLib {
                    "$(FILES)".into()
                } else {
                    String::new()
                }
            });
            if !destination_kind.uses_evaluated_file_list() && files_argument.is_none() {
                return Err(format!(
                    "{} has no explicit file list",
                    destination_kind.label()
                ));
            }
            let files = match destination_kind {
                CopyDestination::DeveloperSdkFd => parse_literal_file_list(&raw_files)?,
                CopyDestination::DeveloperLib
                | CopyDestination::DeveloperBin
                | CopyDestination::DeveloperMan1 => evaluate_source_file_list(
                    &raw_files,
                    &context,
                    scope,
                    invocation.line,
                    destination_kind.file_list_label(),
                )?,
            };

            if destination_kind == CopyDestination::DeveloperLib {
                if source_root_kind != SourceRoot::SelectedSource {
                    return Err(
                        "Developer lib copy source must be local to the selected source tree"
                            .into(),
                    );
                }
                let meta_edges = parse_meta_edges_from_snapshot(
                    root,
                    rel_dir,
                    line_states,
                    source_snapshot,
                    true,
                )?;
                require_exclusive_copy_owner(
                    invocation,
                    scope,
                    dirs,
                    root,
                    rel_dir,
                    line_states,
                    source_snapshot,
                    &meta_edges,
                    &owner,
                    destination_kind,
                    false,
                )?;
                require_source_local_consumer(&meta_edges, &owner, destination_kind)?;
                validate_local_sources(root, &source_dir, &files, destination_kind)?;
                return Ok(SdkFileCopyDecl {
                    owner,
                    file: file.clone(),
                    line: invocation.line + 1,
                    source_dir,
                    destination: destination_kind.alias().into(),
                    files,
                    fetch_owner: None,
                });
            }

            if matches!(
                destination_kind,
                CopyDestination::DeveloperBin | CopyDestination::DeveloperMan1
            ) {
                let local_edges = parse_meta_edges_from_snapshot(
                    root,
                    rel_dir,
                    line_states,
                    source_snapshot,
                    true,
                )?;
                let matching_edges = local_edges
                    .iter()
                    .filter(|edge| edge.owner == owner && edge.state != ConditionalTruth::False)
                    .collect::<Vec<_>>();
                if matching_edges.len() > 1 {
                    return Err(format!(
                        "{} owner has duplicate local `#MM` edges",
                        destination_kind.label()
                    ));
                }
                if matching_edges
                    .iter()
                    .any(|edge| edge.state != ConditionalTruth::True)
                {
                    return Err(format!(
                        "{} owner `#MM` edge is guarded by an unresolved conditional",
                        destination_kind.label()
                    ));
                }

                // These named copy macros define their own producer. The
                // source may attach ordinary MetaMake prerequisites to that
                // target (for example, generated manual aliases); retain those
                // edges while recording the independently proven fetch input.
                require_exclusive_copy_owner(
                    invocation,
                    scope,
                    dirs,
                    root,
                    rel_dir,
                    line_states,
                    source_snapshot,
                    &local_edges,
                    &owner,
                    destination_kind,
                    true,
                )?;
                require_source_local_consumer(&local_edges, &owner, destination_kind)?;

                let fetch_owner = match source_root_kind {
                    SourceRoot::SelectedSource => {
                        validate_local_sources(root, &source_dir, &files, destination_kind)?;
                        None
                    }
                    SourceRoot::FetchedPorts => Some(resolve_local_fetch_source(
                        &source_dir,
                        invocations,
                        scope,
                        dirs,
                        root,
                        rel_dir,
                        line_states,
                        fetches,
                        destination_kind,
                    )?),
                };

                return Ok(SdkFileCopyDecl {
                    owner,
                    file: file.clone(),
                    line: invocation.line + 1,
                    source_dir,
                    destination: destination_kind.alias().into(),
                    files,
                    fetch_owner,
                });
            }

            let local_edges =
                parse_meta_edges_from_snapshot(root, rel_dir, line_states, source_snapshot, false)?;
            let matching_edges = local_edges
                .iter()
                .filter(|edge| edge.owner == owner && edge.state != ConditionalTruth::False)
                .collect::<Vec<_>>();
            // The named copy macro itself creates its producer in GenMF. A
            // source-local regular input needs no artificial handwritten edge,
            // but the owner must be exclusive and have a declared consumer.
            if matching_edges.is_empty() && source_root_kind == SourceRoot::SelectedSource {
                require_exclusive_copy_owner(
                    invocation,
                    scope,
                    dirs,
                    root,
                    rel_dir,
                    line_states,
                    source_snapshot,
                    &local_edges,
                    &owner,
                    destination_kind,
                    false,
                )?;
                require_source_local_consumer(&local_edges, &owner, destination_kind)?;
                validate_in_tree_sources(root, &source_dir, &files)?;
                return Ok(SdkFileCopyDecl {
                    owner,
                    file: file.clone(),
                    line: invocation.line + 1,
                    source_dir,
                    destination: destination_kind.alias().into(),
                    files,
                    fetch_owner: None,
                });
            }
            let [edge] = matching_edges.as_slice() else {
                return Err(if matching_edges.is_empty() {
                    "SDK fd copy owner has no local `#MM owner :` edge".into()
                } else {
                    "SDK fd copy owner has duplicate local `#MM` edges".into()
                });
            };
            if edge.state != ConditionalTruth::True {
                return Err(
                    "SDK fd copy `#MM` edge is guarded by an unresolved conditional".into(),
                );
            }

            let fetch_owner = match source_root_kind {
                SourceRoot::SelectedSource => {
                    if !edge.prerequisites.is_empty() {
                        return Err("in-tree SDK fd copy cannot discard `#MM` prerequisites".into());
                    }
                    validate_in_tree_sources(root, &source_dir, &files)?;
                    None
                }
                SourceRoot::FetchedPorts => {
                    let [fetch_owner] = edge.prerequisites.as_slice() else {
                        return Err(
                            "fetched SDK fd copy must name exactly one `%fetch` prerequisite"
                                .into(),
                        );
                    };
                    validate_fetch_source(
                        fetch_owner,
                        &source_dir,
                        invocations,
                        scope,
                        dirs,
                        root,
                        rel_dir,
                        line_states,
                        fetches,
                        destination_kind,
                    )?;
                    Some(fetch_owner.clone())
                }
            };

            Ok(SdkFileCopyDecl {
                owner,
                file: file.clone(),
                line: invocation.line + 1,
                source_dir,
                destination: destination_kind.alias().into(),
                files,
                fetch_owner,
            })
        })();

        match result {
            Ok(declaration) => declarations.push(declaration),
            Err(reason) => rejections.push(reject(reason)),
        }
    }

    (declarations, rejections)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SourceRoot {
    SelectedSource,
    FetchedPorts,
}

// This capability accepts named arguments only. The shared discovery scanner
// intentionally ignores free words; here that would silently drop copy inputs.
fn validate_argument_shape(
    arguments: &str,
    destination_kind: CopyDestination,
) -> Result<(), String> {
    let bytes = arguments.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        while bytes.get(at).is_some_and(u8::is_ascii_whitespace) {
            at += 1;
        }
        if at == bytes.len() {
            break;
        }
        let start = at;
        while bytes
            .get(at)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        {
            at += 1;
        }
        if at == start || bytes.get(at) != Some(&b'=') {
            return Err(format!(
                "{} contains an unnamed or malformed argument",
                destination_kind.label()
            ));
        }
        at += 1;
        if bytes.get(at) == Some(&b'"') {
            at += 1;
            while bytes.get(at).is_some_and(|byte| *byte != b'"') {
                if bytes[at] == b'\\' {
                    return Err(format!(
                        "{} quoted argument has unsupported escapes",
                        destination_kind.label()
                    ));
                }
                at += 1;
            }
            if bytes.get(at) != Some(&b'"') {
                return Err(format!(
                    "{} contains an unterminated quoted argument",
                    destination_kind.label()
                ));
            }
            at += 1;
            if bytes
                .get(at)
                .is_some_and(|byte| !byte.is_ascii_whitespace())
            {
                return Err(format!(
                    "{} has data after a quoted argument",
                    destination_kind.label()
                ));
            }
        } else {
            let start = at;
            while bytes
                .get(at)
                .is_some_and(|byte| !byte.is_ascii_whitespace())
            {
                if matches!(bytes[at], b'"' | b'\'') {
                    return Err(format!(
                        "{} has an unsupported argument quote",
                        destination_kind.label()
                    ));
                }
                at += 1;
            }
            if at == start {
                return Err(format!(
                    "{} contains an empty unquoted argument",
                    destination_kind.label()
                ));
            }
        }
    }
    Ok(())
}

fn state_at(states: Option<&[ConditionalTruth]>, line: usize) -> ConditionalTruth {
    states
        .and_then(|states| states.get(line))
        .copied()
        .unwrap_or(ConditionalTruth::Unknown)
}

fn parse_literal_file_list(raw: &str) -> Result<Vec<String>, String> {
    if raw.contains(['$', '"', '\'', '\\', '*', '?', '[', ']']) {
        return Err("SDK fd file list must be explicit literals without expansion or globs".into());
    }
    let files = raw
        .split_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if files.is_empty() {
        return Err("SDK fd file list is empty".into());
    }
    if files.len() > 128 {
        return Err("SDK fd file list exceeds 128 entries".into());
    }
    let mut seen = BTreeSet::new();
    for file in &files {
        if !safe_basename(file) || !seen.insert(file.to_ascii_lowercase()) {
            return Err(format!(
                "SDK fd file `{file}` is not a unique safe basename"
            ));
        }
    }
    Ok(files)
}

fn parse_meta_edges_from_snapshot(
    root: &Path,
    rel_dir: &Path,
    line_states: Option<&[ConditionalTruth]>,
    source_snapshot: &str,
    require_active_controls: bool,
) -> Result<Vec<MetaEdge>, String> {
    validate_source_mmakefile_path(root, rel_dir)?;
    let lines = source_snapshot.lines().collect::<Vec<_>>();
    let defined_lines = if require_active_controls {
        source_make_control_suppression(&lines)?
    } else {
        vec![false; lines.len()]
    };

    let mut edges = Vec::new();
    for (line_number, raw_line) in lines.iter().enumerate() {
        if require_active_controls && raw_line.starts_with('\t') {
            continue;
        }
        if defined_lines.get(line_number).copied().unwrap_or(true) {
            continue;
        }
        let line = raw_line.trim_start();
        let Some(body) = line
            .strip_prefix("#MM- ")
            .or_else(|| line.strip_prefix("#MM "))
        else {
            continue;
        };
        if body.trim_end().ends_with('\\') {
            return Err("MetaMake edge snapshot contains an unjoined continuation".into());
        }
        let (owner, prerequisites) = body.split_once(':').ok_or("malformed local `#MM` edge")?;
        let owner = owner.trim();
        if safe_target_name(owner) {
            edges.push(MetaEdge {
                owner: owner.to_owned(),
                prerequisites: prerequisites
                    .split_whitespace()
                    .filter(|word| !matches!(*word, "#MM" | "#MM-"))
                    .map(str::to_owned)
                    .collect(),
                state: state_at(line_states, line_number),
            });
        }
    }
    Ok(edges)
}

fn validate_source_mmakefile_path(root: &Path, rel_dir: &Path) -> Result<(), String> {
    let canonical_root = root
        .canonicalize()
        .map_err(|error| format!("cannot canonicalize source root: {error}"))?;
    let mut path = canonical_root;
    for component in rel_dir.components() {
        let Component::Normal(component) = component else {
            return Err("mmakefile directory is not source-relative".into());
        };
        path.push(component);
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("cannot inspect mmakefile directory: {error}"))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err("mmakefile directory is symlinked or not a directory".into());
        }
    }
    path.push("mmakefile.src");
    let metadata = fs::symlink_metadata(&path)
        .map_err(|error| format!("cannot inspect source mmakefile: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("source mmakefile is symlinked or not a regular file".into());
    }
    Ok(())
}

fn source_make_control_suppression(lines: &[&str]) -> Result<Vec<bool>, String> {
    let mut suppressed = Vec::with_capacity(lines.len());
    let mut define_depth = 0usize;
    let mut controls = Vec::<bool>::new();

    for line in lines {
        if line.starts_with('\t') {
            suppressed.push(define_depth > 0);
            continue;
        }
        let trimmed = line.trim();
        let Some((word, arguments)) = make_directive_word(trimmed) else {
            suppressed.push(define_depth > 0);
            continue;
        };
        let inside_define = define_depth > 0;
        suppressed.push(inside_define || word == "define");

        if word == "define" {
            if arguments.trim().is_empty() {
                return Err("malformed `define` in source mmakefile".into());
            }
            define_depth += 1;
            continue;
        }
        if word == "endef" {
            if define_depth == 0 || !arguments.trim().is_empty() {
                return Err("unmatched or malformed `endef` in source mmakefile".into());
            }
            define_depth -= 1;
            continue;
        }
        if inside_define {
            continue;
        }

        match word {
            "ifeq" | "ifneq" | "ifdef" | "ifndef" => {
                if arguments.trim().is_empty() {
                    return Err(format!("malformed `{word}` in source mmakefile"));
                }
                controls.push(false);
            }
            "else" => {
                let Some(saw_else) = controls.last_mut() else {
                    return Err("unmatched `else` in source mmakefile".into());
                };
                if *saw_else || !arguments.trim().is_empty() {
                    return Err("malformed or duplicate `else` in source mmakefile".into());
                }
                *saw_else = true;
            }
            "endif" if !arguments.trim().is_empty() || controls.pop().is_none() => {
                return Err("unmatched or malformed `endif` in source mmakefile".into());
            }
            _ => {}
        }
    }
    if define_depth != 0 {
        return Err("unterminated `define` in source mmakefile".into());
    }
    if !controls.is_empty() {
        return Err("unterminated Make conditional in source mmakefile".into());
    }
    Ok(suppressed)
}

fn make_directive_word(line: &str) -> Option<(&str, &str)> {
    let line = line
        .split_once(" #")
        .map_or(line, |(before_comment, _)| before_comment)
        .trim_start();
    if line.is_empty() || line.starts_with('#') || line.starts_with('%') {
        return None;
    }
    let mut words = line.splitn(2, char::is_whitespace);
    let first = words.next()?;
    let rest = words.next().unwrap_or_default().trim();
    if matches!(first, "override" | "export" | "private") {
        let mut rest_words = rest.splitn(2, char::is_whitespace);
        let word = rest_words.next()?;
        if matches!(word, "define" | "endef") {
            return Some((word, rest_words.next().unwrap_or_default().trim()));
        }
    }
    Some((first, rest))
}

#[allow(clippy::too_many_arguments)]
fn validate_fetch_source(
    fetch_owner: &str,
    source_dir: &str,
    invocations: &[Invocation],
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
    line_states: Option<&[ConditionalTruth]>,
    fetches: &[FetchDecl],
    destination_kind: CopyDestination,
) -> Result<(), String> {
    if !safe_target_name(fetch_owner) {
        return Err(format!(
            "{} has an unsafe fetch prerequisite name",
            destination_kind.label()
        ));
    }
    let matching_invocations = invocations
        .iter()
        .filter(|invocation| invocation.name == "fetch")
        .filter_map(|invocation| {
            let context = MakeExprContext::new(scope, dirs, invocation.line, root, rel_dir);
            let name = macro_arg(&invocation.args, "mmake")?;
            let name = evaluate_make_expr(&name, &context).ok()?;
            (name == fetch_owner).then_some(invocation)
        })
        .filter(|invocation| state_at(line_states, invocation.line) != ConditionalTruth::False)
        .collect::<Vec<_>>();
    let [fetch_invocation] = matching_invocations.as_slice() else {
        return Err(if matching_invocations.is_empty() {
            format!("`%fetch` target `{fetch_owner}` has no matching local invocation")
        } else {
            format!("`%fetch` target `{fetch_owner}` has duplicate local invocations")
        });
    };
    if state_at(line_states, fetch_invocation.line) != ConditionalTruth::True {
        return Err(format!(
            "`%fetch` target `{fetch_owner}` is guarded by an unresolved conditional"
        ));
    }

    let fetch_argument_names = macro_argument_names(&fetch_invocation.args);
    let unique_fetch_argument_names = fetch_argument_names.iter().collect::<BTreeSet<_>>();
    if fetch_argument_names.len() != unique_fetch_argument_names.len()
        || !unique_fetch_argument_names.contains(&"mmake".to_owned())
        || !unique_fetch_argument_names.contains(&"destination".to_owned())
    {
        return Err(format!(
            "`%fetch` target `{fetch_owner}` has duplicate or missing ownership arguments"
        ));
    }

    let expected_dir = rel_dir.to_string_lossy().replace('\\', "/");
    let fetch_context = MakeExprContext::new(scope, dirs, fetch_invocation.line, root, rel_dir);
    let raw_destination = macro_arg(&fetch_invocation.args, "destination")
        .ok_or("local `%fetch` invocation has no destination")?;
    let declared_destination = evaluate_make_expr(&raw_destination, &fetch_context)
        .map_err(|error| format!("cannot resolve `%fetch` destination: {error}"))?;
    let declarations = fetches
        .iter()
        .filter(|fetch| {
            fetch.name == fetch_owner
                && fetch.destination == declared_destination
                && fetch.dir.trim_matches('/') == expected_dir.trim_matches('/')
        })
        .collect::<Vec<_>>();
    let [fetch] = declarations.as_slice() else {
        return Err(if declarations.is_empty() {
            format!("`#MM` fetch prerequisite `{fetch_owner}` has no matching `%fetch` declaration")
        } else {
            format!("`%fetch` target `{fetch_owner}` is declared more than once")
        });
    };
    if declared_destination != fetch.destination {
        return Err("local `%fetch` destination disagrees with its proven declaration".into());
    }
    let source_prefix = format!("{}/", fetch.destination.trim_end_matches('/'));
    if source_dir != fetch.destination
        && source_dir
            .strip_prefix(&source_prefix)
            .is_none_or(str::is_empty)
    {
        return Err(format!(
            "{} source `{source_dir}` is outside `%fetch` destination `{}`",
            destination_kind.label(),
            fetch.destination,
        ));
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn resolve_local_fetch_source(
    source_dir: &str,
    invocations: &[Invocation],
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
    line_states: Option<&[ConditionalTruth]>,
    fetches: &[FetchDecl],
    destination_kind: CopyDestination,
) -> Result<String, String> {
    let expected_dir = rel_dir.to_string_lossy().replace('\\', "/");
    let mut matching_owners = Vec::new();
    for invocation in invocations
        .iter()
        .filter(|invocation| invocation.name == "fetch")
    {
        if state_at(line_states, invocation.line) == ConditionalTruth::False {
            continue;
        }
        let context = MakeExprContext::new(scope, dirs, invocation.line, root, rel_dir);
        let raw_destination = macro_arg(&invocation.args, "destination").ok_or_else(|| {
            format!(
                "{} cannot prove local `%fetch` ownership: a `%fetch` has no destination",
                destination_kind.label()
            )
        })?;
        let declared_destination =
            evaluate_make_expr(&raw_destination, &context).map_err(|error| {
                format!(
                    "{} cannot prove local `%fetch` ownership: destination is unresolved: {error}",
                    destination_kind.label()
                )
            })?;
        if !at_or_below(source_dir, declared_destination.trim_end_matches('/')) {
            continue;
        }

        if state_at(line_states, invocation.line) != ConditionalTruth::True {
            return Err(format!(
                "{} matching `%fetch` is guarded by an unresolved conditional",
                destination_kind.label()
            ));
        }
        let raw_name = macro_arg(&invocation.args, "mmake").ok_or_else(|| {
            format!(
                "{} matching `%fetch` has no named local owner",
                destination_kind.label()
            )
        })?;
        let name = evaluate_make_expr(&raw_name, &context).map_err(|error| {
            format!(
                "{} matching `%fetch` owner is unresolved: {error}",
                destination_kind.label()
            )
        })?;
        if !safe_target_name(&name) {
            return Err(format!(
                "{} matching `%fetch` owner is not a safe target name",
                destination_kind.label()
            ));
        }

        let declarations = fetches
            .iter()
            .filter(|fetch| {
                fetch.name == name
                    && fetch.destination == declared_destination
                    && fetch.dir.trim_matches('/') == expected_dir.trim_matches('/')
            })
            .collect::<Vec<_>>();
        if declarations.len() != 1 {
            return Err(if declarations.is_empty() {
                format!(
                    "{} matching `%fetch` `{name}` has no typed local declaration",
                    destination_kind.label()
                )
            } else {
                format!(
                    "{} matching `%fetch` `{name}` has duplicate typed local declarations",
                    destination_kind.label()
                )
            });
        }
        matching_owners.push(name);
    }

    let [fetch_owner] = matching_owners.as_slice() else {
        return Err(if matching_owners.is_empty() {
            format!(
                "{} source has no uniquely matching local `%fetch` destination",
                destination_kind.label()
            )
        } else {
            format!(
                "{} source has ambiguous matching local `%fetch` destinations",
                destination_kind.label()
            )
        });
    };
    validate_fetch_source(
        fetch_owner,
        source_dir,
        invocations,
        scope,
        dirs,
        root,
        rel_dir,
        line_states,
        fetches,
        destination_kind,
    )?;
    Ok(fetch_owner.clone())
}

fn validate_in_tree_sources(root: &Path, source_dir: &str, files: &[String]) -> Result<(), String> {
    let relative_dir = source_dir
        .strip_prefix("${AROS_SOURCE_DIR}/")
        .ok_or("in-tree SDK fd source is not below the selected source root")?;
    for file in files {
        regular_source_file(root, &Path::new(relative_dir).join(file))?;
    }
    Ok(())
}

fn regular_source_file(root: &Path, relative: &Path) -> Result<(), String> {
    let root = root
        .canonicalize()
        .map_err(|error| format!("cannot canonicalize selected source root: {error}"))?;
    let mut path = root;
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err("SDK fd input path escapes the selected source tree".into());
        };
        path.push(component);
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("missing SDK fd input {}: {error}", path.display()))?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "SDK fd input crosses a symlink: {}",
                path.display()
            ));
        }
    }
    if !path.is_file() {
        return Err(format!(
            "SDK fd input is not a regular file: {}",
            path.display()
        ));
    }
    Ok(())
}

fn source_relative_file(rel_dir: &Path) -> Result<String, String> {
    let mut components = Vec::new();
    for component in rel_dir.components() {
        let Component::Normal(value) = component else {
            return Err("mmakefile directory is not source-relative".into());
        };
        components.push(value.to_str().ok_or("mmakefile path is not UTF-8")?);
    }
    if components.is_empty() {
        Ok("mmakefile.src".into())
    } else {
        Ok(format!("{}/mmakefile.src", components.join("/")))
    }
}

fn safe_target_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.' | '+')
        })
}

fn safe_basename(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && !matches!(value, "." | "..")
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.' | '+')
        })
}

fn classify_destination(raw: &str, context: &MakeExprContext<'_>) -> Option<CopyDestination> {
    let resolved = evaluate_make_expr(raw, context).ok();
    let fd_root =
        evaluate_make_expr("$(AROS_DEVELOPER)/$(AROS_DIR_SDK)/$(AROS_DIR_FD)", context).ok();
    let lib_root = evaluate_make_expr("$(AROS_DEVELOPER)/$(AROS_DIR_LIB)", context).ok();
    let bin_root = evaluate_make_expr("$(AROS_DEVELOPER)/bin", context).ok();
    let man1_root = evaluate_make_expr("$(AROS_DEVELOPER)/man/man1", context).ok();

    let refers_to_fd = contains_make_variable(raw, "AROS_SDK_FD")
        || ((contains_make_variable(raw, "AROS_DEVELOPER")
            || contains_make_variable(raw, "AROS_SDK"))
            && contains_make_variable(raw, "AROS_DIR_FD"));
    let refers_to_lib = contains_make_variable(raw, "AROS_LIB")
        || (contains_make_variable(raw, "AROS_DEVELOPER")
            && contains_make_variable(raw, "AROS_DIR_LIB"));
    let refers_to_bin = contains_make_variable(raw, "AROS_DEVELOPER") && raw.contains("/bin");
    let refers_to_man1 = contains_make_variable(raw, "AROS_DEVELOPER") && raw.contains("/man/man1");

    if refers_to_fd
        || resolved
            .as_deref()
            .zip(fd_root.as_deref())
            .is_some_and(|(v, r)| at_or_below(v, r))
    {
        return Some(CopyDestination::DeveloperSdkFd);
    }
    if refers_to_lib
        || resolved
            .as_deref()
            .zip(lib_root.as_deref())
            .is_some_and(|(v, r)| at_or_below(v, r))
    {
        return Some(CopyDestination::DeveloperLib);
    }
    if refers_to_bin
        || resolved
            .as_deref()
            .zip(bin_root.as_deref())
            .is_some_and(|(v, r)| at_or_below(v, r))
    {
        return Some(CopyDestination::DeveloperBin);
    }
    if refers_to_man1
        || resolved
            .as_deref()
            .zip(man1_root.as_deref())
            .is_some_and(|(v, r)| at_or_below(v, r))
    {
        return Some(CopyDestination::DeveloperMan1);
    }
    None
}

fn at_or_below(value: &str, root: &str) -> bool {
    value == root
        || value
            .strip_prefix(root)
            .is_some_and(|tail| tail.starts_with('/'))
}

fn contains_make_variable(expression: &str, expected: &str) -> bool {
    let bytes = expression.as_bytes();
    let mut at = 0;
    while at + 2 < bytes.len() {
        if bytes[at] != b'$' || !matches!(bytes[at + 1], b'(' | b'{') {
            at += 1;
            continue;
        }
        let close = if bytes[at + 1] == b'(' { b')' } else { b'}' };
        let start = at + 2;
        let Some(end_offset) = bytes[start..].iter().position(|byte| *byte == close) else {
            return false;
        };
        let end = start + end_offset;
        if &expression[start..end] == expected {
            return true;
        }
        at = end + 1;
    }
    false
}

fn evaluate_source_file_list(
    raw: &str,
    context: &MakeExprContext<'_>,
    scope: &VarScope,
    line: usize,
    label: &str,
) -> Result<Vec<String>, String> {
    validate_source_list_expression(raw, scope, line, label, &mut BTreeSet::new(), 32)?;
    let files = evaluate_make_list(raw, context)
        .map_err(|error| format!("cannot resolve {label} file list: {error}"))?;
    if files.is_empty() {
        return Err(format!("{label} file list is empty"));
    }
    if files.len() > 128 {
        return Err(format!("{label} file list exceeds 128 entries"));
    }
    let mut seen = BTreeSet::new();
    for file in &files {
        if !safe_basename(file) || !seen.insert(file.to_ascii_lowercase()) {
            return Err(format!(
                "{label} file `{file}` is not a unique safe basename"
            ));
        }
    }
    Ok(files)
}

fn validate_source_list_expression(
    raw: &str,
    scope: &VarScope,
    line: usize,
    label: &str,
    visiting: &mut BTreeSet<String>,
    depth: usize,
) -> Result<(), String> {
    if depth == 0 {
        return Err(format!("{label} file-list expansion exceeds 32 levels"));
    }
    let bytes = raw.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        let byte = bytes[at];
        if byte.is_ascii_whitespace() {
            at += 1;
            continue;
        }
        if byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'+') {
            at += 1;
            continue;
        }
        if byte != b'$' || at + 1 >= bytes.len() || !matches!(bytes[at + 1], b'(' | b'{') {
            return Err(format!(
                "{label} file list contains unsupported syntax near `{}`",
                &raw[at..]
            ));
        }
        let close = if bytes[at + 1] == b'(' { b')' } else { b'}' };
        let start = at + 2;
        let Some(end_offset) = bytes[start..]
            .iter()
            .position(|candidate| *candidate == close)
        else {
            return Err(format!(
                "{label} file list has an unterminated variable reference"
            ));
        };
        let end = start + end_offset;
        let name = &raw[start..end];
        if name.is_empty()
            || !name.as_bytes()[0].is_ascii_alphabetic() && name.as_bytes()[0] != b'_'
            || !name
                .bytes()
                .all(|character| character.is_ascii_alphanumeric() || character == b'_')
        {
            return Err(format!(
                "{label} file list has unsupported Make reference `$({name})`"
            ));
        }
        if scope.conditionally_assigned_before(name, line) {
            return Err(format!(
                "{label} file-list variable `{name}` has a conditional assignment"
            ));
        }
        if let Some(reason) = scope.flavor_uncertainty_reason_at(name, line) {
            return Err(format!(
                "{label} file-list variable `{name}` has uncertain Make flavor: {reason}"
            ));
        }
        let Some(value) = scope.raw_at(name, line) else {
            return Err(format!(
                "{label} file-list variable `{name}` is unresolved or not source-owned"
            ));
        };
        if !visiting.insert(name.to_owned()) {
            return Err(format!(
                "{label} file-list variable cycle includes `{name}`"
            ));
        }
        let nested =
            validate_source_list_expression(&value, scope, line, label, visiting, depth - 1);
        visiting.remove(name);
        nested?;
        at = end + 1;
    }
    Ok(())
}

fn require_source_local_consumer(
    edges: &[MetaEdge],
    owner: &str,
    destination_kind: CopyDestination,
) -> Result<(), String> {
    let consumers = edges
        .iter()
        .filter(|edge| {
            edge.owner != owner
                && edge
                    .prerequisites
                    .iter()
                    .any(|prerequisite| prerequisite == owner)
                && edge.state != ConditionalTruth::False
        })
        .collect::<Vec<_>>();
    if consumers.is_empty() {
        return Err(format!(
            "{} owner has no active local `#MM` consumer dependency",
            destination_kind.label()
        ));
    }
    if consumers
        .iter()
        .any(|consumer| consumer.state != ConditionalTruth::True)
    {
        return Err(format!(
            "{} `#MM` consumer dependency is conditional or unknown",
            destination_kind.label()
        ));
    }
    let mut consumer_owners = BTreeSet::new();
    if consumers
        .iter()
        .any(|consumer| !consumer_owners.insert(consumer.owner.as_str()))
    {
        return Err(format!(
            "{} owner has a duplicate local `#MM` consumer dependency",
            destination_kind.label()
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn require_exclusive_copy_owner(
    selected_copy: &Invocation,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
    line_states: Option<&[ConditionalTruth]>,
    source_snapshot: &str,
    meta_edges: &[MetaEdge],
    owner: &str,
    destination_kind: CopyDestination,
    allow_owner_meta_edge: bool,
) -> Result<(), String> {
    if !allow_owner_meta_edge
        && meta_edges
            .iter()
            .any(|edge| edge.owner == owner && edge.state != ConditionalTruth::False)
    {
        return Err(format!(
            "{} owner `{owner}` also has a local `#MM` producer edge",
            destination_kind.label()
        ));
    }

    let lines = source_snapshot.lines().collect::<Vec<_>>();
    let defined_lines = source_make_control_suppression(&lines)?;
    let source_invocations = crate::parser::macro_invocations(source_snapshot);
    for other in &source_invocations {
        if (other.line == selected_copy.line
            && other.name == selected_copy.name
            && other.args == selected_copy.args)
            || defined_lines.get(other.line).copied().unwrap_or(true)
        {
            continue;
        }
        if state_at(line_states, other.line) == ConditionalTruth::False {
            continue;
        }
        let context = MakeExprContext::new(scope, dirs, other.line, root, rel_dir);
        for field in ["mmake", "mainmmake", "parentmmake"] {
            let Some(raw_owner) = macro_arg(&other.args, field) else {
                continue;
            };
            match evaluate_make_expr(&raw_owner, &context) {
                Ok(value) if value.split_whitespace().any(|name| name == owner) => {
                    return Err(format!(
                        "{} owner `{owner}` also has a `%{} {field}=` provider",
                        destination_kind.label(),
                        other.name,
                    ));
                }
                Ok(_) => {}
                Err(_) => {
                    return Err(format!(
                        "another `%{}` {field} is unresolved, so ownership of `{owner}` is ambiguous",
                        other.name
                    ));
                }
            }
        }
    }

    for (line_number, raw_line) in lines.iter().enumerate() {
        if defined_lines.get(line_number).copied().unwrap_or(true)
            || raw_line.starts_with('\t')
            || state_at(line_states, line_number) == ConditionalTruth::False
        {
            continue;
        }
        let line = raw_line.trim_start();
        if line.is_empty() || line.starts_with('#') || line.starts_with('%') {
            continue;
        }
        if make_directive_word(line).is_some_and(|(word, _)| {
            matches!(
                word,
                "ifeq" | "ifneq" | "ifdef" | "ifndef" | "else" | "endif"
            )
        }) {
            continue;
        }
        let Some(colon) = line.find(':') else {
            continue;
        };
        let after_colon = line[colon + 1..].trim_start();
        if ["=", ":=", "?=", "+=", "!="]
            .iter()
            .any(|operator| after_colon.starts_with(operator))
        {
            continue;
        }
        let targets = line[..colon]
            .split_once(" #")
            .map_or(&line[..colon], |(targets, _)| targets)
            .split_whitespace();
        let context = MakeExprContext::new(scope, dirs, line_number, root, rel_dir);
        for target in targets {
            if target.contains('$') {
                let resolved = evaluate_make_expr(target, &context).map_err(|_| {
                    format!(
                        "handwritten Make rule target `{target}` is unresolved and may own `{owner}`"
                    )
                })?;
                if resolved.split_whitespace().any(|name| name == owner) {
                    return Err(format!(
                        "handwritten Make rule also defines {} owner `{owner}`",
                        destination_kind.label()
                    ));
                }
            } else if target == owner {
                return Err(format!(
                    "handwritten Make rule also defines {} owner `{owner}`",
                    destination_kind.label()
                ));
            }
        }
    }
    Ok(())
}

fn validate_local_sources(
    root: &Path,
    source_dir: &str,
    files: &[String],
    destination_kind: CopyDestination,
) -> Result<(), String> {
    let relative = source_dir
        .strip_prefix("${AROS_SOURCE_DIR}")
        .ok_or_else(|| {
            format!(
                "{} source is not rooted in the selected source tree",
                destination_kind.label()
            )
        })?;
    if !relative.is_empty() && !relative.starts_with('/') {
        return Err(format!(
            "{} source is outside the selected source tree",
            destination_kind.label()
        ));
    }
    let relative = Path::new(relative.trim_start_matches('/'));
    for file in files {
        regular_source_file(root, &relative.join(file))?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "sdk_file_copies_tests.rs"]
mod tests;
