//! Interpret one reviewed pure printf idiom; never execute a Make shell.

use crate::dirs::DirVars;
use crate::make_expr::{evaluate_make_expr, MakeExprContext};
use crate::make_vars::{collect_vars_impl, ConditionalTruth};
use crate::parser::TargetContext;
use std::path::Path;

const VERSION_ASSIGNMENT: &str = "LLVM_VERSION_HEX := 0x$(shell printf \"%02x%02x\" $(LLVM_VERSION_MAJOR) $(LLVM_VERSION_MINOR))";

pub(super) fn project_version_format(
    joined: &str,
    selected: &TargetContext,
    dirs: &DirVars,
    root: &Path,
    relative: &Path,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<String, String> {
    if !matches!(
        relative.to_str(),
        Some("workbench/libs/mesa/libgalliumvm" | "workbench/libs/mesa/libllvmpipe")
    ) {
        return Ok(joined.into());
    }
    let mut lines: Vec<String> = joined.lines().map(str::to_owned).collect();
    let (scope, states) = collect_vars_impl(joined, Some(selected));
    let mut define_depth = 0usize;
    let mut count = 0usize;
    for (line_no, line) in joined.lines().enumerate() {
        let visible = line.trim();
        let directive = visible
            .split_whitespace()
            .find(|word| !matches!(*word, "override" | "export" | "private"));
        if directive.is_some_and(|word| word.starts_with(".RECIPEPREFIX")) {
            return Err(
                "reviewed LLVM formatting does not support a custom Make recipe prefix".into(),
            );
        }
        if let Some(name) = super::configured_scope::defined_macro_name(line) {
            if !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
            {
                return Err(
                    "reviewed LLVM formatting requires literal ordinary Make definition names"
                        .into(),
                );
            }
            define_depth += 1;
        } else if visible == "endef" {
            define_depth = define_depth
                .checked_sub(1)
                .ok_or("unmatched LLVM scope endef")?;
        }
        if visible != VERSION_ASSIGNMENT {
            continue;
        }
        if line.starts_with('\t')
            || define_depth != 0
            || states.get(line_no) != Some(&ConditionalTruth::True)
        {
            return Err(
                "reviewed LLVM version formatting must be an active top-level assignment".into(),
            );
        }
        count += 1;
        let context = MakeExprContext::new(&scope, dirs, line_no, root, relative)
            .with_lookup(lookup)
            .without_filesystem();
        let component = |name: &str| -> Result<u8, String> {
            let value = evaluate_make_expr(&format!("$({name})"), &context).map_err(|error| {
                format!("cannot resolve reviewed LLVM version component: {error}")
            })?;
            if value.is_empty()
                || !value.bytes().all(|byte| byte.is_ascii_digit())
                || (value.len() > 1 && value.starts_with('0'))
            {
                return Err("LLVM printf component is not canonical bounded decimal".into());
            }
            value
                .parse()
                .map_err(|_| "LLVM printf component exceeds its reviewed byte range".into())
        };
        let major = component("LLVM_VERSION_MAJOR")?;
        let minor = component("LLVM_VERSION_MINOR")?;
        lines[line_no] = format!("LLVM_VERSION_HEX := 0x{major:02x}{minor:02x}");
    }
    if count != 1 {
        return Err(
            "selected LLVM recipe must have exactly one reviewed version-format assignment".into(),
        );
    }
    let mut projected = lines.join("\n");
    if joined.ends_with('\n') {
        projected.push('\n');
    }
    Ok(projected)
}

#[cfg(test)]
#[path = "configured_llvm_tests.rs"]
mod tests;
