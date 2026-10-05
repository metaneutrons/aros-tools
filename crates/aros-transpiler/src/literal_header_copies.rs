//! Named `%copy_files_q` rules staging exact source-owned SDK headers.
//!
//! The capability preserves the source macro's single destination and file
//! list. It does not turn a file copy into a recursive copy, publish to both
//! include roots, or execute the Make macro's shell expansion.

use crate::copy_includes::HeaderTransformDecl;
use crate::dirs::DirVars;
use crate::make_expr::{evaluate_make_expr, evaluate_make_list, MakeExprContext};
use crate::make_vars::{ConditionalTruth, VarScope};
use crate::parser::{macro_arg, macro_argument_names, Invocation};
use std::path::{Component, Path};

pub struct Rejection {
    pub owner: Option<String>,
    pub reason: String,
}

pub fn collect(
    invocations: &[Invocation],
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    relative_dir: &Path,
    line_states: Option<&[ConditionalTruth]>,
) -> (Vec<HeaderTransformDecl>, Vec<Rejection>) {
    let mut declarations = Vec::new();
    let mut rejected = Vec::new();
    for invocation in invocations
        .iter()
        .filter(|item| item.name == "copy_files_q")
    {
        if line_states.and_then(|states| states.get(invocation.line))
            == Some(&ConditionalTruth::False)
        {
            continue;
        }
        let context = MakeExprContext::new(scope, dirs, invocation.line, root, relative_dir);
        let owner = macro_arg(&invocation.args, "mmake")
            .and_then(|raw| evaluate_make_expr(&raw, &context).ok())
            .filter(|value| safe_name(value));
        let Some(destination) = macro_arg(&invocation.args, "dst") else {
            continue;
        };
        let rendered = crate::copy_directories::render_copy_directory_path(
            &destination,
            &context,
            relative_dir,
        );
        // Other file-copy destinations remain outside this header capability.
        let header_root = rendered
            .as_ref()
            .is_ok_and(|path| include_destination(path));
        if !header_root
            && !destination.contains("$(AROS_INCLUDES)")
            && !destination.contains("$(GENINCDIR)")
        {
            continue;
        }
        let result = (|| -> Result<Vec<HeaderTransformDecl>, String> {
            if line_states.and_then(|states| states.get(invocation.line))
                != Some(&ConditionalTruth::True)
            {
                return Err("header copy is guarded by an unresolved Make conditional".into());
            }
            let owner = owner
                .as_ref()
                .ok_or("header copy has no safe named owner")?;
            let names = macro_argument_names(&invocation.args);
            let mut unique = names.clone();
            unique.sort();
            unique.dedup();
            if names.len() != unique.len()
                || unique
                    .iter()
                    .any(|name| !matches!(name.as_str(), "mmake" | "files" | "src" | "dst"))
            {
                return Err("header copy has unsupported or duplicate arguments".into());
            }
            let destination = rendered.map_err(|reason| format!("header destination: {reason}"))?;
            if !include_destination(&destination) {
                return Err("header copy destination is outside configured include roots".into());
            }
            let source = macro_arg(&invocation.args, "src").unwrap_or_else(|| ".".into());
            let source = crate::copy_directories::render_copy_directory_path(
                &source,
                &context,
                relative_dir,
            )?;
            let source_relative = source
                .strip_prefix("${AROS_SOURCE_DIR}/")
                .ok_or("header copy source must be inside the selected source tree")?;
            let files = macro_arg(&invocation.args, "files").unwrap_or_else(|| "$(FILES)".into());
            let files = evaluate_make_list(&files, &context)
                .map_err(|error| format!("cannot resolve header copy file list: {error}"))?;
            if files.is_empty() {
                return Err("header copy file list is empty".into());
            }
            let mut seen = std::collections::HashSet::new();
            let mut outputs = Vec::new();
            for file in files {
                if !safe_name(&file)
                    || Path::new(&file)
                        .extension()
                        .is_none_or(|extension| extension != "h")
                    || !seen.insert(file.clone())
                {
                    return Err(format!(
                        "header copy file `{file}` is not one unique literal header basename"
                    ));
                }
                regular_source_file(root, &Path::new(source_relative).join(&file))?;
                outputs.push(HeaderTransformDecl {
                    name: owner.clone(),
                    file: relative_dir.join("mmakefile.src").display().to_string(),
                    line: invocation.line + 1,
                    input: format!("{source}/{file}"),
                    output: format!("{destination}/{file}"),
                    match_text: String::new(),
                    replacement: String::new(),
                    copy_only: true,
                    replace_whole_line_containing: false,
                    substitutions: Vec::new(),
                    dependencies: Vec::new(),
                    consumers: Vec::new(),
                    generated_input_owner: None,
                });
            }
            Ok(outputs)
        })();
        match result {
            Ok(outputs) => declarations.extend(outputs),
            Err(reason) => rejected.push(Rejection { owner, reason }),
        }
    }
    (declarations, rejected)
}

fn include_destination(path: &str) -> bool {
    ["${AROS_SDK_INCLUDE_DIR}", "${AROS_GENINC_DIR}"]
        .iter()
        .any(|root| {
            path == *root
                || path
                    .strip_prefix(root)
                    .is_some_and(|tail| tail.starts_with('/'))
        })
}

fn safe_name(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | '+'))
}

fn regular_source_file(root: &Path, relative: &Path) -> Result<(), String> {
    let mut path = root.canonicalize().map_err(|error| error.to_string())?;
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err("header source escapes selected tree".into());
        };
        path.push(component);
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|error| format!("missing header source {}: {error}", path.display()))?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "header source crosses a symlink: {}",
                path.display()
            ));
        }
    }
    if !path.is_file() {
        return Err("header source is not a regular file".into());
    }
    Ok(())
}
