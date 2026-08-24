//! `%copy_dir_recursive`: staging a whole directory into the build tree.
//!
//! The recipe names a source and a destination, and both have to be checked
//! rather than trusted. A source may be an in-tree directory or one of a fixed
//! set of roots that map to stable CMake variables; an arbitrary host path would
//! make the configuration non-reproducible, and a destination outside the build
//! tree would let a declaration write anywhere. Anything that does not fit is
//! reported and staged by nobody.

use crate::ast::CopyDirectoryDecl;
use crate::make_expr::{evaluate_make_expr, MakeExprContext};
use crate::make_vars::{ConditionalTruth, VarScope};
use crate::parser::{macro_arg, macro_argument_names, Invocation};
use std::fs;
use std::path::{Component, Path};

/// The directory roots which map to stable CMake locations in a generated
/// copy target.  A `%copy_dir_recursive` recipe runs below the source tree;
/// accepting an arbitrary host path here would make the generated graph both
/// non-reproducible and unsafe to configure.
const COPY_DIRECTORY_CMAKE_ROOTS: &[&str] = &[
    "${CMAKE_SOURCE_DIR}",
    "${CMAKE_BINARY_DIR}",
    "${AROS_BUILD_DIR}",
    "${AROS_PORTS_DIR}",
    "${AROS_PORTS_SOURCE_DIR}",
    "${AROS_SDK_INCLUDE_DIR}",
    "${AROS_GENINC_DIR}",
];

/// Maps the two historic staging roots to the CMake roots which actually feed
/// compilation.  `AROS_INCLUDES` is the target SDK bootstrap tree in this
/// build, while `GENINCDIR` is the host-tool header tree; expanding the legacy
/// config literally would otherwise point at its unused `gen/include` and
/// `SYS/Developer/include` layouts.
fn normalize_copy_directory_root_alias(path: &str) -> String {
    for (legacy, cmake) in [
        ("${AROS_BUILD_DIR}/gen/include", "${AROS_GENINC_DIR}"),
        (
            "${AROS_BUILD_DIR}/SYS/Developer/include",
            "${AROS_SDK_INCLUDE_DIR}",
        ),
    ] {
        if path == legacy {
            return cmake.to_owned();
        }
        if let Some(tail) = path
            .strip_prefix(legacy)
            .filter(|tail| tail.starts_with('/'))
        {
            return format!("{cmake}{tail}");
        }
    }
    path.to_owned()
}

/// Accepts only ordinary path components.  CMake receives every path quoted,
/// but rejecting list separators, quotes, newlines and deferred variables here
/// keeps a declaration from changing CMake syntax or from acquiring a
/// machine-local meaning later.
fn safe_copy_directory_component(component: &str) -> bool {
    !component.is_empty()
        && component.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(character, '_' | '-' | '.' | ' ' | '+' | '@')
        })
}

/// Normalises a path already rooted in one CMake variable.
fn normalize_cmake_copy_directory_path(raw: &str) -> Option<String> {
    let raw = normalize_copy_directory_root_alias(raw.trim());
    let root = COPY_DIRECTORY_CMAKE_ROOTS.iter().find(|root| {
        raw == ***root
            || raw
                .strip_prefix(**root)
                .is_some_and(|tail| tail.starts_with('/'))
    })?;
    let tail = raw.strip_prefix(*root).unwrap_or_default();
    let mut components = Vec::new();
    for component in tail.split('/') {
        match component {
            "" | "." => {}
            // A CMake-rooted path has no lexical source-tree owner above its
            // root.  Rejecting this is both stricter and clearer than relying
            // on CMake's normalisation after a variable expansion.
            ".." => return None,
            value if safe_copy_directory_component(value) => components.push(value),
            _ => return None,
        }
    }
    if components.is_empty() {
        Some((*root).to_owned())
    } else {
        Some(format!("{root}/{}", components.join("/")))
    }
}

/// Normalises one path relative to the declaring mmakefile directory.
fn normalize_relative_copy_directory_path(raw: &str, relative_dir: &Path) -> Option<String> {
    let mut components = Vec::new();
    for component in relative_dir.components() {
        let Component::Normal(value) = component else {
            return None;
        };
        components.push(value.to_str()?.to_owned());
    }
    for component in raw.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                components.pop()?;
            }
            value if safe_copy_directory_component(value) => components.push(value.to_owned()),
            _ => return None,
        }
    }
    if components.is_empty() {
        Some("${CMAKE_SOURCE_DIR}".to_owned())
    } else {
        Some(format!("${{CMAKE_SOURCE_DIR}}/{}", components.join("/")))
    }
}

/// Renders a `%copy_dir_recursive` path at the declaration site.
fn render_copy_directory_path(
    raw: &str,
    context: &MakeExprContext<'_>,
    relative_dir: &Path,
) -> std::result::Result<String, String> {
    let value = evaluate_make_expr(raw, context)
        .map_err(|error| format!("cannot evaluate `{raw}`: {error}"))?;
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("`{raw}` expands to an empty path"));
    }
    if value.starts_with("${") {
        return normalize_cmake_copy_directory_path(value)
            .ok_or_else(|| format!("`{value}` is not a safe CMake-rooted path"));
    }
    if value.starts_with('/') || value.contains('$') {
        return Err(format!("`{value}` is not a safe source-tree-relative path"));
    }
    normalize_relative_copy_directory_path(value, relative_dir)
        .ok_or_else(|| format!("`{value}` escapes or is not a safe source-tree-relative path"))
}

fn copy_directory_source_is_owned_path(path: &str) -> bool {
    ["${CMAKE_SOURCE_DIR}", "${AROS_PORTS_DIR}"]
        .iter()
        .any(|root| {
            path == *root
                || path
                    .strip_prefix(root)
                    .is_some_and(|tail| tail.starts_with('/'))
        })
}

/// Ensures that a source-tree copy names a real directory below the checked
/// out tree.  Port paths deliberately cannot be tested here: their owner is
/// fetched at build time and may be absent during a clean configure.
fn in_tree_copy_directory_source_is_safe(path: &str, source_root: &Path) -> bool {
    let Some(tail) = path.strip_prefix("${CMAKE_SOURCE_DIR}") else {
        return true;
    };
    if !tail.is_empty() && !tail.starts_with('/') {
        return false;
    }
    let Ok(root) = fs::canonicalize(source_root) else {
        return false;
    };
    let Ok(candidate) = fs::canonicalize(root.join(tail.trim_start_matches('/'))) else {
        return false;
    };
    candidate.is_dir() && candidate.starts_with(root)
}

fn copy_directory_destination_is_build_path(path: &str) -> bool {
    [
        "${CMAKE_BINARY_DIR}",
        "${AROS_BUILD_DIR}",
        "${AROS_SDK_INCLUDE_DIR}",
        "${AROS_GENINC_DIR}",
    ]
    .iter()
    .any(|root| {
        path == *root
            || path
                .strip_prefix(root)
                .is_some_and(|tail| tail.starts_with('/'))
    })
}

fn valid_copy_directory_target_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
        })
}

/// Extracts the bounded `%copy_dir_recursive` capability.
///
/// The legacy macro accepts a general recursive copy and therefore could
/// receive arbitrary source/destination text.  Only declarations whose paths
/// reduce to a source-tree or known fetched-port root and to a build-owned
/// destination are admitted.  This means the CMake graph has a concrete owner
/// for every product rather than a configure-time host leak.
pub(crate) fn collect(
    invocations: &[Invocation],
    scope: &VarScope,
    dirs: &crate::dirs::DirVars,
    root: &Path,
    relative_dir: &Path,
    line_states: Option<&[ConditionalTruth]>,
) -> (Vec<CopyDirectoryDecl>, Vec<String>) {
    let file = if relative_dir.as_os_str().is_empty() {
        "mmakefile.src".to_owned()
    } else {
        format!("{}/mmakefile.src", relative_dir.display())
    };
    let mut declarations = Vec::new();
    let mut skipped = Vec::new();

    for invocation in invocations
        .iter()
        .filter(|invocation| invocation.name == "copy_dir_recursive")
    {
        match line_states
            .and_then(|states| states.get(invocation.line))
            .copied()
            .unwrap_or(ConditionalTruth::Unknown)
        {
            ConditionalTruth::False => continue,
            ConditionalTruth::Unknown => {
                skipped.push(format!(
                    "{}:{}: %copy_dir_recursive is guarded by an unresolved Make conditional",
                    file,
                    invocation.line + 1
                ));
                continue;
            }
            ConditionalTruth::True => {}
        }

        let names = macro_argument_names(&invocation.args);
        let mut unique_names = names.clone();
        unique_names.sort();
        unique_names.dedup();
        let has_only_supported_arguments = unique_names
            .iter()
            .all(|name| matches!(name.as_str(), "mmake" | "src" | "dst" | "excludefiles"));
        if names.len() != unique_names.len() || !has_only_supported_arguments {
            skipped.push(format!(
                "{}:{}: %copy_dir_recursive has unsupported or duplicate arguments",
                file,
                invocation.line + 1
            ));
            continue;
        }

        let Some(name) = macro_arg(&invocation.args, "mmake") else {
            skipped.push(format!(
                "{}:{}: %copy_dir_recursive has no concrete mmake= owner",
                file,
                invocation.line + 1
            ));
            continue;
        };
        if !valid_copy_directory_target_name(&name) {
            skipped.push(format!(
                "{}:{}: %copy_dir_recursive mmake={name} is not a concrete target name",
                file,
                invocation.line + 1
            ));
            continue;
        }
        if macro_arg(&invocation.args, "excludefiles").is_some_and(|value| !value.trim().is_empty())
        {
            skipped.push(format!(
                "{}:{}: %copy_dir_recursive mmake={name} uses excludefiles=, which has no audited CMake equivalent",
                file,
                invocation.line + 1
            ));
            continue;
        }

        let source_raw = macro_arg(&invocation.args, "src").unwrap_or_else(|| ".".to_owned());
        let Some(destination_raw) = macro_arg(&invocation.args, "dst") else {
            skipped.push(format!(
                "{}:{}: %copy_dir_recursive mmake={name} has no dst=",
                file,
                invocation.line + 1
            ));
            continue;
        };
        let context = MakeExprContext::new(scope, dirs, invocation.line, root, relative_dir);
        let source = match render_copy_directory_path(&source_raw, &context, relative_dir) {
            Ok(value) if !copy_directory_source_is_owned_path(&value) => {
                skipped.push(format!(
                    "{}:{}: %copy_dir_recursive mmake={name} source {value} has no source-tree or port owner",
                    file,
                    invocation.line + 1
                ));
                continue;
            }
            Ok(value) if !in_tree_copy_directory_source_is_safe(&value, root) => {
                skipped.push(format!(
                    "{}:{}: %copy_dir_recursive mmake={name} source {value} is not a real in-tree directory",
                    file,
                    invocation.line + 1
                ));
                continue;
            }
            Ok(value) => value,
            Err(reason) => {
                skipped.push(format!(
                    "{}:{}: %copy_dir_recursive mmake={name} src={source_raw} {reason}",
                    file,
                    invocation.line + 1
                ));
                continue;
            }
        };
        let destination = match render_copy_directory_path(&destination_raw, &context, relative_dir)
        {
            Ok(value) if copy_directory_destination_is_build_path(&value) => value,
            Ok(value) => {
                skipped.push(format!(
                    "{}:{}: %copy_dir_recursive mmake={name} destination {value} is not build-owned",
                    file,
                    invocation.line + 1
                ));
                continue;
            }
            Err(reason) => {
                skipped.push(format!(
                    "{}:{}: %copy_dir_recursive mmake={name} dst={destination_raw} {reason}",
                    file,
                    invocation.line + 1
                ));
                continue;
            }
        };

        declarations.push(CopyDirectoryDecl {
            name,
            source,
            destination,
            file: file.clone(),
            line: invocation.line + 1,
            dependencies: Vec::new(),
        });
    }

    (declarations, skipped)
}
