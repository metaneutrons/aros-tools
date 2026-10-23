//! Projection of one narrowly supported Make target-specific C option rule.
//!
//! The helper admits a rule only when its evaluated target set covers every
//! object and dependency file produced by one C-only `build_linklib`
//! declaration. It neither runs a shell nor materializes deferred directories.

use crate::make_expr::{evaluate_make_expr, evaluate_make_list, MakeExprContext, MakeExprError};
use crate::parser::{macro_arg, macro_argument_names};
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

const TARGET_OPTIONS_SUFFIX: &str = "_MC_EXTRA_CPPFLAGS";
const REQUIRED_ARGUMENTS: &[&str] = &["files", "objdir", "srcdir", "usetree"];
const NON_C_ARGUMENTS: &[&str] = &["objcfiles", "cxxfiles", "asmfiles", "objs"];

/// One supported literal target-specific Make assignment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TargetOptionsRule {
    /// The literal mmake owner name before `_MC_EXTRA_CPPFLAGS`.
    pub owner: String,
    /// The raw Make target-list expression from the rule header.
    pub targets: String,
    /// The raw right-hand side of the immediate `:=` assignment.
    pub value: String,
}

/// Parse a target-specific `:=` rule with a small, explicit syntax surface.
///
/// Targets must be one to eight plain `$(ASCII_IDENT)` references. The
/// assigned variable must be a literal safe mmake name followed by
/// `_MC_EXTRA_CPPFLAGS`. Unknown syntax is intentionally left to the caller's
/// existing guard.
pub(super) fn parse(line: &str) -> Option<TargetOptionsRule> {
    let line = line.trim();
    let (raw_targets, raw_assignment) = line.split_once(':')?;
    let targets = raw_targets.split_ascii_whitespace().collect::<Vec<_>>();
    if !(1..=8).contains(&targets.len()) || !targets.iter().all(|target| plain_reference(target)) {
        return None;
    }

    let (raw_owner_variable, value) = raw_assignment.trim().split_once(":=")?;
    let raw_owner_variable = raw_owner_variable.trim();
    let owner = raw_owner_variable.strip_suffix(TARGET_OPTIONS_SUFFIX)?;
    if !safe_mmake_owner(owner) {
        return None;
    }

    Some(TargetOptionsRule {
        owner: owner.to_owned(),
        targets: raw_targets.trim().to_owned(),
        value: value.trim().to_owned(),
    })
}

/// Return pure option words only when the rule covers the complete C output
/// set of the selected `build_linklib` declaration.
pub(super) fn project_complete_c(
    rule: &TargetOptionsRule,
    rule_context: &MakeExprContext<'_>,
    declaration_context: &MakeExprContext<'_>,
    declaration_args: &str,
) -> Result<Vec<String>, String> {
    if !safe_mmake_owner(&rule.owner) {
        return Err("target-specific C option rule has an unsafe mmake owner".into());
    }
    if rule.value.contains('$') {
        return Err("target-specific C options require literal RHS until target-local references are modeled".into());
    }
    if rule
        .value
        .chars()
        .any(|character| matches!(character, '\'' | '"' | '\\'))
    {
        return Err(
            "target-specific C options with shell quoting or escapes are not modeled".into(),
        );
    }
    if rule.value.chars().any(|character| {
        !(character.is_ascii_whitespace()
            || character.is_ascii_alphanumeric()
            || "_-+=.,:/".contains(character))
    }) {
        return Err("target-specific C options require shell-inert literal words".into());
    }

    let argument_names = macro_argument_names(declaration_args);
    for key in ["cflags", "cppflags", "dflags", "includedir", "lto"] {
        if argument_names
            .iter()
            .filter(|name| name.as_str() == key)
            .count()
            > 1
        {
            return Err(format!("ambiguous build_linklib {key}= override"));
        }
        if argument_names.iter().any(|name| name == key) {
            let value = macro_arg(declaration_args, key)
                .ok_or_else(|| format!("empty or malformed build_linklib {key}= override"))?;
            let unchanged = match key {
                "cppflags" => value == "$(CPPFLAGS)",
                // The default is configured TARGET_LTO, not universally no.
                "lto" => false,
                _ => value.is_empty(),
            };
            if !unchanged {
                return Err(format!(
                    "build_linklib {key}= override is not part of the global option projection"
                ));
            }
        }
    }
    for key in REQUIRED_ARGUMENTS {
        if argument_names
            .iter()
            .filter(|name| name.as_str() == *key)
            .count()
            != 1
        {
            return Err(format!(
                "build_linklib declaration must contain exactly one {key}= argument"
            ));
        }
    }
    for key in NON_C_ARGUMENTS {
        if argument_names
            .iter()
            .filter(|name| name.as_str() == *key)
            .count()
            > 1
        {
            return Err(format!(
                "build_linklib declaration contains ambiguous {key}= arguments"
            ));
        }
    }

    let compiler_count = argument_names
        .iter()
        .filter(|name| name.as_str() == "compiler")
        .count();
    if compiler_count > 1
        || (compiler_count == 1
            && macro_arg(declaration_args, "compiler").as_deref() != Some("target"))
    {
        return Err("build_linklib compiler= must be absent or the literal target".into());
    }

    let files_raw = required_argument(declaration_args, "files")?;
    let files = evaluate_make_list(&files_raw, declaration_context)
        .map_err(|error| format!("cannot resolve build_linklib files=: {error}"))?;
    if files.is_empty() {
        return Err("build_linklib files= must resolve to a nonempty C source list".into());
    }
    for file in &files {
        validate_relative_stem(file)?;
    }

    let objdir_raw = required_argument(declaration_args, "objdir")?;
    let objdir = evaluate_make_expr(&objdir_raw, declaration_context)
        .map_err(|error| format!("cannot resolve build_linklib objdir=: {error}"))?;
    let objdir = validate_absolute_directory(&objdir, "objdir")?;
    let objdir_name = format!("{}_OBJDIR", rule.owner);
    match evaluate_make_expr(&format!("$({objdir_name})"), declaration_context) {
        Ok(effective) if validate_absolute_directory(&effective, "effective OBJDIR")? == objdir => {
        }
        Ok(_) => return Err("effective owner_OBJDIR differs from the declaration objdir".into()),
        // This is the reviewed build_linklib ?= operation, not a guessed
        // value: only an exactly unresolved owner variable selects its default.
        Err(MakeExprError::UnresolvedVariables { names, .. }) if names == [objdir_name] => {}
        Err(error) => return Err(format!("cannot resolve effective owner_OBJDIR: {error}")),
    }

    let srcdir_raw = required_argument(declaration_args, "srcdir")?;
    let srcdir = evaluate_make_expr(&srcdir_raw, declaration_context)
        .map_err(|error| format!("cannot resolve build_linklib srcdir=: {error}"))?;
    let srcdir = validate_absolute_directory(&srcdir, "srcdir")?;

    let usetree_raw = required_argument(declaration_args, "usetree")?;
    let usetree = evaluate_make_expr(&usetree_raw, declaration_context)
        .map_err(|error| format!("cannot resolve build_linklib usetree=: {error}"))?;
    if usetree != "yes" {
        return Err("build_linklib usetree= must resolve to yes".into());
    }

    for key in NON_C_ARGUMENTS {
        if let Some(raw) = macro_arg(declaration_args, key) {
            let values = evaluate_make_list(&raw, declaration_context)
                .map_err(|error| format!("cannot resolve build_linklib {key}=: {error}"))?;
            if !values.is_empty() {
                return Err(format!(
                    "build_linklib {key}= must be empty for a complete C target set"
                ));
            }
        }
    }

    let expected_sources = files
        .iter()
        .map(|file| path_text(&srcdir.join(file.as_str()), "expected C source path"))
        .collect::<Result<Vec<_>, _>>()?;
    let source_variable = format!("$({}_C_FILES)", rule.owner);
    let resolved_sources = evaluate_make_list(&source_variable, declaration_context)
        .map_err(|error| format!("cannot resolve {}_C_FILES: {error}", rule.owner))?;
    if resolved_sources != expected_sources {
        return Err(format!(
            "{}_C_FILES does not exactly match srcdir plus files= in declaration order",
            rule.owner
        ));
    }

    let mut expected_targets = BTreeSet::new();
    for file in &files {
        expected_targets.insert(path_text(
            &objdir.join(Path::new(&format!("{file}.o"))),
            "object path",
        )?);
        expected_targets.insert(path_text(
            &objdir.join(Path::new(&format!("{file}.d"))),
            "dependency path",
        )?);
    }

    let evaluated_targets = evaluate_make_list(&rule.targets, rule_context)
        .map_err(|error| format!("cannot resolve target-specific rule targets: {error}"))?;
    let actual_targets = evaluated_targets.iter().collect::<BTreeSet<_>>();
    if actual_targets.len() != evaluated_targets.len() {
        return Err("target-specific rule target set contains duplicates".into());
    }
    if actual_targets != expected_targets.iter().collect::<BTreeSet<_>>() {
        return Err(
            "target-specific rule must cover exactly every C object and dependency output".into(),
        );
    }

    let options = evaluate_make_list(&rule.value, rule_context)
        .map_err(|error| format!("cannot resolve target-specific C options: {error}"))?;
    if options.is_empty() {
        return Err("target-specific C option rule resolves to no option words".into());
    }
    Ok(options)
}

fn plain_reference(value: &str) -> bool {
    let Some(name) = value
        .strip_prefix("$(")
        .and_then(|value| value.strip_suffix(')'))
    else {
        return false;
    };
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn safe_mmake_owner(owner: &str) -> bool {
    let mut bytes = owner.bytes();
    bytes
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn required_argument(args: &str, key: &str) -> Result<String, String> {
    macro_arg(args, key).ok_or_else(|| format!("build_linklib declaration is missing {key}="))
}

fn validate_absolute_directory(value: &str, label: &str) -> Result<PathBuf, String> {
    if value.is_empty() || value.contains('$') || value.chars().any(char::is_whitespace) {
        return Err(format!(
            "build_linklib {label}= must resolve to a literal path without dollar signs or whitespace"
        ));
    }
    let path = Path::new(value);
    if !path.is_absolute() {
        return Err(format!("build_linklib {label}= must be absolute"));
    }

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir | Component::Normal(_) => normalized.push(component.as_os_str()),
            Component::CurDir | Component::ParentDir | Component::Prefix(_) => {
                return Err(format!(
                    "build_linklib {label}= must not contain dot or parent components"
                ));
            }
        }
    }
    if normalized.as_os_str() != path.as_os_str() {
        return Err(format!(
            "build_linklib {label}= must be lexically normalized"
        ));
    }
    Ok(normalized)
}

fn validate_relative_stem(value: &str) -> Result<(), String> {
    if value.is_empty() || value.contains('$') || value.chars().any(char::is_whitespace) {
        return Err(format!(
            "build_linklib files= contains an empty or unsafe source stem `{value}`"
        ));
    }
    let path = Path::new(value);
    if path.is_absolute() || path.extension().is_some() {
        return Err(format!(
            "build_linklib files= source stem must be relative and suffixless: `{value}`"
        ));
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(_) => normalized.push(component.as_os_str()),
            Component::RootDir
            | Component::CurDir
            | Component::ParentDir
            | Component::Prefix(_) => {
                return Err(format!(
                    "build_linklib files= source stem must be normalized and contain no root or parent components: `{value}`"
                ));
            }
        }
    }
    if normalized.as_os_str() != path.as_os_str() || normalized.as_os_str().is_empty() {
        return Err(format!(
            "build_linklib files= source stem must be lexically normalized: `{value}`"
        ));
    }
    Ok(())
}

fn path_text(path: &Path, label: &str) -> Result<String, String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("{label} is not valid UTF-8"))
}

#[cfg(test)]
#[path = "configured_target_options_tests.rs"]
mod tests;
