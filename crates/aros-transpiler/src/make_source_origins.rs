//! Source origins for bounded literal Make include expansion.

use crate::parser::join_continuations_with_origins;
use crate::{local_make_includes, LocalMakeIncludeLimits};
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiteralObjectSourceAnchor {
    pub(crate) source: PathBuf,
    pub(crate) line: usize,
}

#[derive(Default)]
struct ReconstructedLiteralSource {
    pub(crate) text: String,
    pub(crate) physical_anchors: Vec<Option<LiteralObjectSourceAnchor>>,
}

/// Reconstructs the exact source include expansion while retaining the
/// original file and physical line for every emitted line. A mismatch means
/// the include provenance cannot safely support a source location.
pub fn literal_object_source_anchors(
    root: &Path,
    source: &Path,
    source_text: &str,
    scan: &local_make_includes::LocalMakeIncludeScan,
    joined_snapshot: &str,
) -> Option<Vec<Option<LiteralObjectSourceAnchor>>> {
    let canonical_root = std::fs::canonicalize(root).ok()?;
    let mut next_fragment = 0;
    let mut remaining_fragment_bytes = LocalMakeIncludeLimits::default().bytes;
    let mut reconstructed = reconstruct_literal_source(
        &canonical_root,
        source,
        source_text,
        &scan.fragments,
        &mut next_fragment,
        false,
        &mut remaining_fragment_bytes,
    )?;
    if next_fragment != scan.fragments.len() || reconstructed.text != scan.expanded {
        // Ordinary local scopes retain their include directive; explicit
        // native configuration replaces it with a verification marker. Both
        // grammars must reproduce the complete observed expansion exactly.
        next_fragment = 0;
        remaining_fragment_bytes = LocalMakeIncludeLimits::default().bytes;
        reconstructed = reconstruct_literal_source(
            &canonical_root,
            source,
            source_text,
            &scan.fragments,
            &mut next_fragment,
            true,
            &mut remaining_fragment_bytes,
        )?;
        if next_fragment != scan.fragments.len() || reconstructed.text != scan.expanded {
            return None;
        }
    }

    let (joined, physical_origins) = join_continuations_with_origins(&reconstructed.text);
    if joined != joined_snapshot {
        return None;
    }

    if reconstructed.text.split_inclusive('\n').count() != reconstructed.physical_anchors.len() {
        return None;
    }

    let logical_anchors = physical_origins
        .into_iter()
        .map(|origin| reconstructed.physical_anchors.get(origin).cloned())
        .collect::<Option<Vec<_>>>()?;
    if logical_anchors.len() != joined_snapshot.lines().count() {
        return None;
    }
    Some(logical_anchors)
}

/// Reconstruct one bounded source file and its recursively included fragments.
/// Every emitted physical line retains either its literal source location or
/// an absent marker when source configuration was verified but not rendered.
fn reconstruct_literal_source(
    root: &Path,
    source: &Path,
    source_text: &str,
    fragments: &[local_make_includes::IncludedLocalMakeFragment],
    next_fragment: &mut usize,
    retain_include_directive: bool,
    remaining_fragment_bytes: &mut usize,
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
                        .any(|component| !matches!(component, Component::Normal(_)))
                {
                    return None;
                }
                let fragment_path = fragment.path.clone();
                *next_fragment += 1;
                if fragment.is_proven_absent() {
                    local_make_includes::verify_absent_source_options(root, &fragment_path).ok()?;
                    if fragment.generated_output.is_some()
                        || fragment.template_substitutions.is_some()
                        || retain_include_directive
                    {
                        return None;
                    }
                    reconstructed
                        .text
                        .push_str("# Verified source configuration include\n");
                    reconstructed.physical_anchors.push(None);
                    continue;
                }
                let resolved_fragment_path = root.join(&fragment_path);
                if std::fs::canonicalize(&resolved_fragment_path)
                    .ok()
                    .as_deref()
                    != Some(resolved_fragment_path.as_path())
                    || !std::fs::metadata(&resolved_fragment_path).ok()?.is_file()
                {
                    return None;
                }
                if retain_include_directive && fragment.generated_output.is_none() {
                    reconstructed.text.push_str(chunk);
                    reconstructed
                        .physical_anchors
                        .push(Some(LiteralObjectSourceAnchor {
                            source: source.to_path_buf(),
                            line,
                        }));
                } else {
                    reconstructed
                        .text
                        .push_str(if fragment.generated_output.is_some() {
                            "# Verified source configuration template\n"
                        } else {
                            "# Verified source configuration include\n"
                        });
                    reconstructed.physical_anchors.push(None);
                }

                let (_, bytes) = aros_common::publication::measure_regular_file_bounded(
                    &resolved_fragment_path,
                    *remaining_fragment_bytes as u64,
                )
                .ok()??;
                *remaining_fragment_bytes = remaining_fragment_bytes.checked_sub(bytes.len())?;
                let mut fragment_text = String::from_utf8(bytes).ok()?;
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
                    retain_include_directive,
                    remaining_fragment_bytes,
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
