//! Bounded evaluation of the GNU Make list expressions used by the AROS tree.
//!
//! The source inventory currently contains 175 `addprefix`, 71 `addsuffix`,
//! 116 `filter`, 15 `filter-out`, 119 `patsubst`, 24 `subst`, 28 `notdir`, 73 `dir`, 11
//! `basename`, seven `sort`, 20 `strip`, 41 `wildcard`, 143
//! `call WILDCARD`, and 44 substitution-reference occurrences. The evaluator
//! deliberately implements that finite, side-effect-free subset instead of
//! trying to embed GNU Make. Unsupported functions and unresolved variables
//! are errors; they never turn into an empty list by accident.
//!
//! Evaluation is positional through [`VarScope::raw_at`]. Directory variables
//! fall back to [`DirVars::expand_with`]. `SRCDIR` and `CURDIR` can be
//! materialised temporarily so a source-tree `wildcard` is evaluated during
//! transpilation; `TOP` remains the CMake build root. Other deferred CMake
//! paths remain strings, but are rejected if used as filesystem glob patterns.

use crate::dirs::DirVars;
use crate::make_vars::VarScope;
use glob::{glob_with, MatchOptions, Pattern};
use std::cell::RefCell;
use std::fmt;
use std::path::{Path, PathBuf};

const MAX_EXPANSION_DEPTH: usize = 32;
const ESCAPED_DOLLAR: char = '\u{e000}';

/// Collector-specific raw variable lookup used to extend the shared scopes.
pub type MakeVariableLookup<'a> = dyn Fn(&str) -> Option<String> + 'a;

/// Returns a diagnostic when a collector knows a variable is unsafe to use.
pub type MakeVariableGuard<'a> = dyn Fn(&str) -> Option<String> + 'a;

/// Inputs needed to evaluate one expression at its declaration site.
#[derive(Clone, Copy)]
pub struct MakeExprContext<'a> {
    scope: &'a VarScope,
    dirs: &'a DirVars,
    line: usize,
    source_dir: &'a Path,
    relative_dir: &'a Path,
    lookup: Option<&'a MakeVariableLookup<'a>>,
    guard: Option<&'a MakeVariableGuard<'a>>,
}

impl<'a> MakeExprContext<'a> {
    /// Creates a context for an mmakefile below `source_dir`.
    ///
    /// `relative_dir` is the directory containing the mmakefile, relative to
    /// `source_dir`. `line` uses the same zero-based, continuation-joined line
    /// coordinates as [`VarScope::raw_at`].
    #[must_use]
    pub const fn new(
        scope: &'a VarScope,
        dirs: &'a DirVars,
        line: usize,
        source_dir: &'a Path,
        relative_dir: &'a Path,
    ) -> Self {
        Self {
            scope,
            dirs,
            line,
            source_dir,
            relative_dir,
            lookup: None,
            guard: None,
        }
    }

    /// Adds a raw-value lookup checked before `VarScope` and `DirVars`.
    ///
    /// This keeps the evaluator usable with collector-specific maps without
    /// coupling it to their value representation. Returned values may contain
    /// further Make expressions; they are recursively evaluated and can fall
    /// back to the positional local scope and then the global directory table.
    #[must_use]
    pub const fn with_lookup(mut self, lookup: &'a MakeVariableLookup<'a>) -> Self {
        self.lookup = Some(lookup);
        self
    }

    /// Adds a guard for conditional or otherwise ambiguous local variables.
    ///
    /// The callback returns a human-readable reason for rejection. It is
    /// checked before collector values, `VarScope`, and nested local overrides
    /// used while resolving `DirVars`.
    #[must_use]
    pub const fn with_guard(mut self, guard: &'a MakeVariableGuard<'a>) -> Self {
        self.guard = Some(guard);
        self
    }

    /// Returns a positional local value only when it is safe to expand before
    /// expression evaluation. Conditional values must reach the evaluator so
    /// its `UnsafeVariable` error remains fatal for the complete source lane.
    pub(crate) fn safe_local_raw(&self, name: &str) -> Option<String> {
        if self.scope.conditionally_assigned_before(name, self.line) {
            None
        } else {
            self.scope.raw_at(name, self.line)
        }
    }
}

/// Why a bounded Make expression could not be evaluated faithfully.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum MakeExprError {
    /// Parentheses, arguments, or the context itself are malformed.
    InvalidSyntax { expression: String, detail: String },
    /// A variable was not present in either the local or directory scope.
    UnresolvedVariables {
        names: Vec<String>,
        expansion_chain: Vec<String>,
    },
    /// Recursive variable definitions formed a cycle.
    VariableCycle { expansion_chain: Vec<String> },
    /// A collector identified a conditional or ambiguous variable definition.
    UnsafeVariable {
        name: String,
        detail: String,
        expansion_chain: Vec<String>,
    },
    /// The bounded recursion limit was reached.
    ExpansionLimit { expression: String },
    /// The expression asks for a GNU Make function outside the safe subset.
    UnsupportedFunction { name: String },
    /// A single-character or automatic Make reference cannot be decided here.
    UnsupportedReference { reference: String },
    /// A glob over a build-tree location the transpiler does not resolve, so
    /// the fragment containing it is dropped. Named "deferred" because the
    /// intent was for CMake to expand it; nothing does, and the glob results
    /// feed further Make functions (`:%.c=%`, `filter-out`, `addprefix`) that
    /// only this evaluator can apply, so an opaque marker would not work
    /// either. The real blocker is ordering: these globs cover Ports content
    /// that a build step fetches, and a source list is needed at configure
    /// time.
    DeferredWildcard { pattern: String },
    /// A glob pattern or one of its filesystem results was invalid.
    Wildcard { pattern: String, detail: String },
}

impl fmt::Display for MakeExprError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSyntax { expression, detail } => {
                write!(f, "invalid Make expression `{expression}`: {detail}")
            }
            Self::UnresolvedVariables {
                names,
                expansion_chain,
            } => {
                write!(f, "unresolved Make variable(s): {}", names.join(", "))?;
                if !expansion_chain.is_empty() {
                    write!(f, " while expanding {}", expansion_chain.join(" -> "))?;
                }
                Ok(())
            }
            Self::VariableCycle { expansion_chain } => {
                write!(f, "Make variable cycle: {}", expansion_chain.join(" -> "))
            }
            Self::UnsafeVariable {
                name,
                detail,
                expansion_chain,
            } => {
                write!(f, "unsafe Make variable `{name}`: {detail}")?;
                if !expansion_chain.is_empty() {
                    write!(f, " while expanding {}", expansion_chain.join(" -> "))?;
                }
                Ok(())
            }
            Self::ExpansionLimit { expression } => {
                write!(
                    f,
                    "Make expression exceeded the expansion limit: `{expression}`"
                )
            }
            Self::UnsupportedFunction { name } => {
                write!(f, "unsupported Make function `{name}`")
            }
            Self::UnsupportedReference { reference } => {
                write!(f, "unsupported Make reference `{reference}`")
            }
            Self::DeferredWildcard { pattern } => {
                write!(
                    f,
                    "wildcard over an unresolved build-tree path was dropped, \
                     not deferred: `{pattern}`"
                )
            }
            Self::Wildcard { pattern, detail } => {
                write!(f, "cannot evaluate wildcard `{pattern}`: {detail}")
            }
        }
    }
}

impl std::error::Error for MakeExprError {}

/// Evaluates an expression to GNU Make's whitespace-separated string form.
///
/// This supports nested `$(...)` references, suffix and `%` substitution
/// references, and the functions documented by this module. Quotes retain
/// their ordinary Make meaning: they are characters, not shell quoting.
pub fn evaluate_make_expr(
    raw: &str,
    context: &MakeExprContext<'_>,
) -> Result<String, MakeExprError> {
    let mut evaluator = Evaluator::new(context)?;
    let value = evaluator.expand_text(raw, MAX_EXPANSION_DEPTH)?;
    reject_unsupported_references(&value)?;
    Ok(value.replace(ESCAPED_DOLLAR, "$"))
}

/// Evaluates an expression and splits its result into Make list words.
pub fn evaluate_make_list(
    raw: &str,
    context: &MakeExprContext<'_>,
) -> Result<Vec<String>, MakeExprError> {
    Ok(evaluate_make_expr(raw, context)?
        .split_whitespace()
        .map(str::to_owned)
        .collect())
}

struct Evaluator<'a> {
    scope: &'a VarScope,
    dirs: &'a DirVars,
    line: usize,
    source_text: String,
    relative_text: String,
    wildcard_root: PathBuf,
    lookup: Option<&'a MakeVariableLookup<'a>>,
    guard: Option<&'a MakeVariableGuard<'a>>,
    expansion_chain: Vec<String>,
    /// Innermost-last bindings of `$(foreach var,...)` loop variables. Make
    /// gives the loop variable a temporary value that shadows any global of
    /// the same name, so this is consulted before every other source.
    loop_vars: Vec<(String, String)>,
}

impl<'a> Evaluator<'a> {
    fn new(context: &MakeExprContext<'a>) -> Result<Self, MakeExprError> {
        if context.relative_dir.is_absolute() {
            return Err(MakeExprError::InvalidSyntax {
                expression: context.relative_dir.display().to_string(),
                detail: "the mmakefile directory must be relative to the source tree".to_owned(),
            });
        }
        let source_text = path_text(context.source_dir, "source directory")?;
        let relative_text = if context.relative_dir.as_os_str().is_empty() {
            ".".to_owned()
        } else {
            path_text(context.relative_dir, "relative mmakefile directory")?
        };
        Ok(Self {
            scope: context.scope,
            dirs: context.dirs,
            line: context.line,
            source_text,
            relative_text,
            wildcard_root: context.source_dir.join(context.relative_dir),
            lookup: context.lookup,
            guard: context.guard,
            expansion_chain: Vec::new(),
            loop_vars: Vec::new(),
        })
    }

    fn expand_text(&mut self, raw: &str, depth: usize) -> Result<String, MakeExprError> {
        if depth == 0 {
            return Err(MakeExprError::ExpansionLimit {
                expression: raw.to_owned(),
            });
        }

        let mut out = String::with_capacity(raw.len());
        let mut cursor = 0usize;
        while cursor < raw.len() {
            let Some(relative_dollar) = raw[cursor..].find('$') else {
                out.push_str(&raw[cursor..]);
                break;
            };
            let dollar = cursor + relative_dollar;
            out.push_str(&raw[cursor..dollar]);
            match raw.as_bytes().get(dollar + 1) {
                Some(b'$') => {
                    out.push(ESCAPED_DOLLAR);
                    cursor = dollar + 2;
                }
                Some(b'(') => {
                    let end = reference_end(raw, dollar)?;
                    out.push_str(&self.evaluate_reference(&raw[dollar + 2..end], depth - 1)?);
                    cursor = end + 1;
                }
                Some(b'{') => {
                    let Some(relative_end) = raw[dollar + 2..].find('}') else {
                        return Err(MakeExprError::InvalidSyntax {
                            expression: raw[dollar..].to_owned(),
                            detail: "unclosed `${...}` reference".to_owned(),
                        });
                    };
                    let end = dollar + 2 + relative_end;
                    let body = &raw[dollar + 2..end];
                    if is_deferred_cmake_reference(body) {
                        out.push_str(&raw[dollar..=end]);
                    } else {
                        out.push_str(&self.evaluate_reference(body, depth - 1)?);
                    }
                    cursor = end + 1;
                }
                _ => {
                    out.push('$');
                    cursor = dollar + 1;
                }
            }
        }
        Ok(out)
    }

    fn evaluate_reference(&mut self, body: &str, depth: usize) -> Result<String, MakeExprError> {
        let trimmed = body.trim();
        if trimmed.is_empty() {
            return Err(MakeExprError::InvalidSyntax {
                expression: "$()".to_owned(),
                detail: "empty variable name".to_owned(),
            });
        }

        if let Some(head_end) = top_level_whitespace(trimmed) {
            let name = &trimmed[..head_end];
            let args = trimmed[head_end..].trim_start();
            return self.evaluate_function(name, args, depth);
        }

        self.evaluate_variable(trimmed, depth)
    }

    fn evaluate_variable(&mut self, body: &str, depth: usize) -> Result<String, MakeExprError> {
        let (raw_name, substitution) = split_substitution_reference(body)?;
        let name = self.expand_text(raw_name, depth)?.trim().to_owned();
        if name.is_empty() || name.chars().any(char::is_whitespace) {
            return Err(MakeExprError::InvalidSyntax {
                expression: format!("$({body})"),
                detail: format!("invalid expanded variable name `{name}`"),
            });
        }

        if let Some(at) = self.expansion_chain.iter().position(|item| item == &name) {
            let mut chain = self.expansion_chain[at..].to_vec();
            chain.push(name);
            return Err(MakeExprError::VariableCycle {
                expansion_chain: chain,
            });
        }

        let raw_value = self.resolve_variable(&name)?;
        self.expansion_chain.push(name);
        let expanded = self.expand_text(&raw_value, depth);
        self.expansion_chain.pop();
        let expanded = expanded?;

        let Some((raw_from, raw_to)) = substitution else {
            return Ok(expanded);
        };
        let from = self.expand_text(raw_from, depth)?.trim().to_owned();
        let to = self.expand_text(raw_to, depth)?.trim().to_owned();
        let words = make_words(&expanded);
        let transformed = if from.contains('%') {
            patsubst(&from, &to, &words)
        } else {
            words
                .iter()
                .map(|word| suffix_substitute(word, &from, &to))
                .collect()
        };
        Ok(join_words(&transformed))
    }

    fn resolve_variable(&self, name: &str) -> Result<String, MakeExprError> {
        if let Some((_, value)) = self.loop_vars.iter().rev().find(|(bound, _)| bound == name) {
            return Ok(value.clone());
        }
        if let Some(value) = self.context_value(name) {
            return Ok(value);
        }
        self.check_guard(name)?;
        if let Some(value) = self.lookup.and_then(|lookup| lookup(name)) {
            return Ok(value);
        }
        if let Some(value) = self.scope.raw_at(name, self.line) {
            return Ok(value);
        }

        let reference = format!("$({name})");
        // Variables imported from make.cfg were simply expanded before a
        // project mmakefile can shadow names such as TARGETDIR. Resolve that
        // configured chain on its own first. Falling through to expand_with is
        // reserved for the few collector/local expressions that genuinely
        // need values from both scopes.
        if let Some(expanded) = self.dirs.expand(&reference) {
            return Ok(expanded);
        }
        let guarded = RefCell::new(None);
        let local = |nested: &str| {
            if let Some(value) = self.context_value(nested) {
                return Some(value);
            }
            if let Some(detail) = self.guard_reason(nested) {
                *guarded.borrow_mut() = Some((nested.to_owned(), detail));
                return None;
            }
            self.lookup
                .and_then(|lookup| lookup(nested))
                // A global directory value can legitimately refer to a name
                // shadowed by the local variable currently being expanded.
                // `TARGETDIR := $(AROS_TESTS)/Library` is the real example:
                // the configured AROS_TESTS chain reaches the global
                // TARGETDIR. Re-entering the local assignment creates a fake
                // cycle; skipping active names gives that nested lookup the
                // configured value Make had when `:=` ran.
                .or_else(|| {
                    (!self.expansion_chain.iter().any(|item| item == nested))
                        .then(|| self.scope.raw_at(nested, self.line))
                        .flatten()
                })
        };
        let expanded = self.dirs.expand_with(&reference, &local);
        if let Some((name, detail)) = guarded.into_inner() {
            return Err(MakeExprError::UnsafeVariable {
                name,
                detail,
                expansion_chain: self.expansion_chain.clone(),
            });
        }
        expanded.map_err(|names| MakeExprError::UnresolvedVariables {
            names,
            expansion_chain: self.expansion_chain.clone(),
        })
    }

    fn context_value(&self, name: &str) -> Option<String> {
        match name {
            "SRCDIR" => Some("${CMAKE_SOURCE_DIR}".to_owned()),
            "TOP" => Some("${AROS_BUILD_DIR}".to_owned()),
            "CURDIR" => Some(self.relative_text.clone()),
            _ => None,
        }
    }

    fn check_guard(&self, name: &str) -> Result<(), MakeExprError> {
        let Some(detail) = self.guard_reason(name) else {
            return Ok(());
        };
        Err(MakeExprError::UnsafeVariable {
            name: name.to_owned(),
            detail,
            expansion_chain: self.expansion_chain.clone(),
        })
    }

    fn guard_reason(&self, name: &str) -> Option<String> {
        if self.scope.conditionally_assigned_before(name, self.line) {
            return Some("assigned inside an unevaluated Make conditional".to_owned());
        }
        self.guard.and_then(|guard| guard(name))
    }

    fn evaluate_function(
        &mut self,
        name: &str,
        raw_args: &str,
        depth: usize,
    ) -> Result<String, MakeExprError> {
        match name {
            // $(foreach var,list,text): bind var to each word of list in turn
            // and expand text, joining the results with a single space. The
            // binding is temporary and shadows a global of the same name.
            //
            // rom/dos needs this for its image loaders,
            // `$(foreach img, aos elf, internalloadseg_$(img))`, without which
            // dos.library is built with no ELF loader at all. muimaster needs
            // it for its 44 classes.
            "foreach" => {
                let args = function_arguments(name, raw_args, 3)?;
                let variable = self.expand_text(args[0].trim(), depth)?;
                let variable = variable.trim().to_owned();
                if variable.is_empty() {
                    return Err(MakeExprError::InvalidSyntax {
                        expression: raw_args.to_owned(),
                        detail: "foreach has an empty loop variable name".to_owned(),
                    });
                }
                let list = make_words(&self.expand_text(args[1].trim(), depth)?);
                let mut output: Vec<String> = Vec::with_capacity(list.len());
                for word in list {
                    self.loop_vars.push((variable.clone(), word));
                    let expanded = self.expand_text(args[2], depth);
                    self.loop_vars.pop();
                    output.push(expanded?);
                }
                Ok(join_words(&make_words(&output.join(" "))))
            }
            "addprefix" | "addsuffix" | "filter" | "filter-out" => {
                let args = function_arguments(name, raw_args, 2)?;
                let first = self.expand_text(args[0].trim(), depth)?;
                let words = make_words(&self.expand_text(args[1].trim(), depth)?);
                let output = match name {
                    "addprefix" => words
                        .iter()
                        .map(|word| format!("{}{word}", first.trim()))
                        .collect(),
                    "addsuffix" => words
                        .iter()
                        .map(|word| format!("{word}{}", first.trim()))
                        .collect(),
                    "filter" => filter_words(&make_words(&first), &words, true),
                    "filter-out" => filter_words(&make_words(&first), &words, false),
                    _ => unreachable!(),
                };
                Ok(join_words(&output))
            }
            "patsubst" => {
                let args = function_arguments(name, raw_args, 3)?;
                let pattern = self.expand_text(args[0].trim(), depth)?;
                let replacement = self.expand_text(args[1].trim(), depth)?;
                let words = make_words(&self.expand_text(args[2].trim(), depth)?);
                Ok(join_words(&patsubst(
                    pattern.trim(),
                    replacement.trim(),
                    &words,
                )))
            }
            "subst" => {
                let args = function_arguments(name, raw_args, 3)?;
                let from = self.expand_text(args[0], depth)?;
                if from.is_empty() {
                    return Err(MakeExprError::InvalidSyntax {
                        expression: format!("$(subst {raw_args})"),
                        detail: "subst with an empty search string is not supported".to_owned(),
                    });
                }
                let to = self.expand_text(args[1], depth)?;
                let text = self.expand_text(args[2], depth)?;
                Ok(text.replace(&from, &to))
            }
            "notdir" | "dir" | "basename" | "suffix" | "sort" | "strip" | "wildcard" => {
                let args = function_arguments(name, raw_args, 1)?;
                let expanded = self.expand_text(args[0].trim(), depth)?;
                match name {
                    "notdir" => Ok(join_words(
                        &make_words(&expanded)
                            .iter()
                            .filter_map(|word| notdir(word))
                            .collect::<Vec<_>>(),
                    )),
                    "dir" => Ok(join_words(
                        &make_words(&expanded)
                            .iter()
                            .map(|word| directory_part(word))
                            .collect::<Vec<_>>(),
                    )),
                    "basename" => Ok(join_words(
                        &make_words(&expanded)
                            .iter()
                            .filter_map(|word| basename(word))
                            .collect::<Vec<_>>(),
                    )),
                    "suffix" => Ok(join_words(
                        &make_words(&expanded)
                            .iter()
                            .filter_map(|word| suffix(word))
                            .collect::<Vec<_>>(),
                    )),
                    "sort" => {
                        let mut words = make_words(&expanded);
                        words.sort();
                        words.dedup();
                        Ok(join_words(&words))
                    }
                    "strip" => Ok(join_words(&make_words(&expanded))),
                    "wildcard" => Ok(join_words(&self.wildcard(&expanded, false)?)),
                    _ => unreachable!(),
                }
            }
            "call" => {
                let args = function_arguments(name, raw_args, 2)?;
                let callee = self.expand_text(args[0].trim(), depth)?;
                if callee.trim() != "WILDCARD" {
                    return Err(MakeExprError::UnsupportedFunction {
                        name: format!("call {}", callee.trim()),
                    });
                }
                let patterns = self.expand_text(args[1].trim(), depth)?;
                Ok(join_words(&self.wildcard(&patterns, true)?))
            }
            _ => Err(MakeExprError::UnsupportedFunction {
                name: name.to_owned(),
            }),
        }
    }

    fn wildcard(
        &self,
        expanded_patterns: &str,
        regular_files_only: bool,
    ) -> Result<Vec<String>, MakeExprError> {
        reject_unsupported_references(expanded_patterns)?;
        let options = MatchOptions {
            case_sensitive: true,
            require_literal_separator: true,
            require_literal_leading_dot: true,
        };
        let root_text = path_text(&self.wildcard_root, "wildcard root")?;
        let escaped_root = Pattern::escape(&root_text);
        let escaped_source = Pattern::escape(&self.source_text);
        let mut output = Vec::new();

        for original_pattern in expanded_patterns.split_whitespace() {
            let (materialized_pattern, glob_pattern, source_backed) =
                if let Some(suffix) = original_pattern.strip_prefix("${CMAKE_SOURCE_DIR}") {
                    if suffix.contains("${") {
                        return Err(MakeExprError::DeferredWildcard {
                            pattern: original_pattern.to_owned(),
                        });
                    }
                    (
                        concatenate_path_prefix(&self.source_text, suffix),
                        concatenate_path_prefix(&escaped_source, suffix),
                        true,
                    )
                } else {
                    if original_pattern.contains("${") {
                        return Err(MakeExprError::DeferredWildcard {
                            pattern: original_pattern.to_owned(),
                        });
                    }
                    let absolute = Path::new(original_pattern).is_absolute();
                    let glob_pattern = if absolute || escaped_root.is_empty() {
                        original_pattern.to_owned()
                    } else {
                        concatenate_path_prefix(&escaped_root, original_pattern)
                    };
                    (original_pattern.to_owned(), glob_pattern, false)
                };
            if materialized_pattern.contains("${") {
                return Err(MakeExprError::DeferredWildcard {
                    pattern: original_pattern.to_owned(),
                });
            }
            let absolute = Path::new(&materialized_pattern).is_absolute();
            let paths =
                glob_with(&glob_pattern, options).map_err(|error| MakeExprError::Wildcard {
                    pattern: original_pattern.to_owned(),
                    detail: error.to_string(),
                })?;
            let mut matches = Vec::new();
            for result in paths {
                let path = result.map_err(|error| MakeExprError::Wildcard {
                    pattern: original_pattern.to_owned(),
                    detail: error.to_string(),
                })?;
                if regular_files_only && !path.is_file() {
                    continue;
                }
                if source_backed {
                    let relative =
                        path.strip_prefix(Path::new(&self.source_text))
                            .map_err(|_| MakeExprError::Wildcard {
                                pattern: original_pattern.to_owned(),
                                detail: format!(
                                    "match `{}` escaped the source directory",
                                    path.display()
                                ),
                            })?;
                    let relative = path_text(relative, "source-relative wildcard result")?;
                    matches.push(if relative.is_empty() {
                        "${CMAKE_SOURCE_DIR}".to_owned()
                    } else {
                        format!("${{CMAKE_SOURCE_DIR}}/{relative}")
                    });
                    continue;
                }
                let shown = if absolute {
                    path.as_path()
                } else {
                    path.strip_prefix(&self.wildcard_root)
                        .unwrap_or(path.as_path())
                };
                matches.push(path_text(shown, "wildcard result")?);
            }
            matches.sort();
            output.extend(matches);
        }
        Ok(output)
    }
}

fn concatenate_path_prefix(prefix: &str, suffix: &str) -> String {
    match (prefix.ends_with('/'), suffix.starts_with('/')) {
        (true, true) => format!("{}{}", prefix, &suffix[1..]),
        (false, false) if !prefix.is_empty() && !suffix.is_empty() => {
            format!("{prefix}/{suffix}")
        }
        _ => format!("{prefix}{suffix}"),
    }
}

fn path_text(path: &Path, purpose: &str) -> Result<String, MakeExprError> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| MakeExprError::InvalidSyntax {
            expression: path.display().to_string(),
            detail: format!("{purpose} is not valid UTF-8"),
        })
}

fn reference_end(raw: &str, start: usize) -> Result<usize, MakeExprError> {
    let bytes = raw.as_bytes();
    let mut depth = 1usize;
    let mut cursor = start + 2;
    while cursor < bytes.len() {
        if bytes[cursor] == b'$' && bytes.get(cursor + 1) == Some(&b'(') {
            depth += 1;
            cursor += 2;
            continue;
        }
        if bytes[cursor] == b')' {
            depth -= 1;
            if depth == 0 {
                return Ok(cursor);
            }
        }
        cursor += 1;
    }
    Err(MakeExprError::InvalidSyntax {
        expression: raw[start..].to_owned(),
        detail: "unclosed `$(...)` reference".to_owned(),
    })
}

fn function_arguments<'a>(
    function: &str,
    raw: &'a str,
    expected: usize,
) -> Result<Vec<&'a str>, MakeExprError> {
    let args = split_top_level(raw, ',')?;
    if args.len() != expected {
        return Err(MakeExprError::InvalidSyntax {
            expression: format!("$({function} {raw})"),
            detail: format!(
                "function `{function}` expects {expected} argument(s), got {}",
                args.len()
            ),
        });
    }
    Ok(args)
}

fn split_top_level(raw: &str, separator: char) -> Result<Vec<&str>, MakeExprError> {
    let bytes = raw.as_bytes();
    let separator = separator as u8;
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut depth = 0usize;
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if bytes[cursor] == b'$' && bytes.get(cursor + 1) == Some(&b'(') {
            depth += 1;
            cursor += 2;
            continue;
        }
        if bytes[cursor] == b')' && depth > 0 {
            depth -= 1;
        } else if bytes[cursor] == separator && depth == 0 {
            out.push(&raw[start..cursor]);
            start = cursor + 1;
        }
        cursor += 1;
    }
    if depth != 0 {
        return Err(MakeExprError::InvalidSyntax {
            expression: raw.to_owned(),
            detail: "unclosed nested reference in function arguments".to_owned(),
        });
    }
    out.push(&raw[start..]);
    Ok(out)
}

type SubstitutionReference<'a> = (&'a str, Option<(&'a str, &'a str)>);

fn split_substitution_reference(body: &str) -> Result<SubstitutionReference<'_>, MakeExprError> {
    let Some(colon) = top_level_byte(body, b':') else {
        return Ok((body, None));
    };
    let remainder = &body[colon + 1..];
    let Some(equal) = top_level_byte(remainder, b'=') else {
        return Err(MakeExprError::InvalidSyntax {
            expression: format!("$({body})"),
            detail: "substitution reference has `:` but no `=`".to_owned(),
        });
    };
    Ok((
        &body[..colon],
        Some((&remainder[..equal], &remainder[equal + 1..])),
    ))
}

fn top_level_byte(raw: &str, needle: u8) -> Option<usize> {
    let bytes = raw.as_bytes();
    let mut depth = 0usize;
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if bytes[cursor] == b'$' && bytes.get(cursor + 1) == Some(&b'(') {
            depth += 1;
            cursor += 2;
            continue;
        }
        if bytes[cursor] == b')' && depth > 0 {
            depth -= 1;
        } else if bytes[cursor] == needle && depth == 0 {
            return Some(cursor);
        }
        cursor += 1;
    }
    None
}

fn top_level_whitespace(raw: &str) -> Option<usize> {
    let bytes = raw.as_bytes();
    let mut depth = 0usize;
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if bytes[cursor] == b'$' && bytes.get(cursor + 1) == Some(&b'(') {
            depth += 1;
            cursor += 2;
            continue;
        }
        if bytes[cursor] == b')' && depth > 0 {
            depth -= 1;
        } else if bytes[cursor].is_ascii_whitespace() && depth == 0 {
            return Some(cursor);
        }
        cursor += 1;
    }
    None
}

fn make_words(raw: &str) -> Vec<String> {
    raw.split_whitespace().map(str::to_owned).collect()
}

fn join_words(words: &[String]) -> String {
    words.join(" ")
}

fn pattern_stem<'a>(pattern: &str, word: &'a str) -> Option<&'a str> {
    let Some(percent) = pattern.find('%') else {
        return (pattern == word).then_some("");
    };
    let prefix = &pattern[..percent];
    let suffix = &pattern[percent + 1..];
    if word.len() < prefix.len() + suffix.len()
        || !word.starts_with(prefix)
        || !word.ends_with(suffix)
    {
        return None;
    }
    Some(&word[prefix.len()..word.len() - suffix.len()])
}

fn pattern_replacement(replacement: &str, stem: &str) -> String {
    replacement.find('%').map_or_else(
        || replacement.to_owned(),
        |percent| {
            format!(
                "{}{}{}",
                &replacement[..percent],
                stem,
                &replacement[percent + 1..]
            )
        },
    )
}

fn filter_words(patterns: &[String], words: &[String], keep_matches: bool) -> Vec<String> {
    words
        .iter()
        .filter(|word| {
            let matched = patterns
                .iter()
                .any(|pattern| pattern_stem(pattern, word).is_some());
            matched == keep_matches
        })
        .cloned()
        .collect()
}

fn patsubst(pattern: &str, replacement: &str, words: &[String]) -> Vec<String> {
    words
        .iter()
        .map(|word| {
            pattern_stem(pattern, word).map_or_else(
                || word.clone(),
                |stem| pattern_replacement(replacement, stem),
            )
        })
        .collect()
}

fn suffix_substitute(word: &str, from: &str, to: &str) -> String {
    word.strip_suffix(from)
        .map_or_else(|| word.to_owned(), |stem| format!("{stem}{to}"))
}

fn notdir(word: &str) -> Option<String> {
    let value = word.rsplit_once('/').map_or(word, |(_, tail)| tail);
    (!value.is_empty()).then(|| value.to_owned())
}

fn directory_part(word: &str) -> String {
    word.rfind('/')
        .map_or_else(|| "./".to_owned(), |slash| word[..=slash].to_owned())
}

fn basename(word: &str) -> Option<String> {
    let component = word.rsplit_once('/').map_or(word, |(_, tail)| tail);
    let value = component.rfind('.').map_or_else(
        || word.to_owned(),
        |dot| word[..word.len() - component.len() + dot].to_owned(),
    );
    (!value.is_empty()).then_some(value)
}

fn suffix(word: &str) -> Option<String> {
    let component = word.rsplit_once('/').map_or(word, |(_, tail)| tail);
    component.rfind('.').map(|dot| component[dot..].to_owned())
}

fn is_deferred_cmake_reference(name: &str) -> bool {
    matches!(
        name,
        "CMAKE_SOURCE_DIR"
            | "CMAKE_BINARY_DIR"
            | "AROS_BUILD_DIR"
            | "AROS_SYS_DIR"
            | "AROS_BOOT_DIR"
            | "AROS_BOOT_ARCH_DIR"
            | "AROS_PORTS_DIR"
            | "AROS_PORTS_SOURCE_DIR"
            | "AROS_TARGET_CPU"
            | "AROS_TARGET_CPU32"
            | "AROS_TARGET_PLATFORM"
            | "AROS_TARGET_LEGACY_PLATFORM"
            | "AROS_TARGET_FAMILY"
            | "AROS_TARGET_VARIANT"
            | "AROS_TARGET_ICONSET"
            | "AROS_BUILD_DATE_DMY"
            | "AROS_BUILD_DATE_ISO"
    )
}

fn reject_unsupported_references(raw: &str) -> Result<(), MakeExprError> {
    let bytes = raw.as_bytes();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if bytes[cursor] != b'$' {
            cursor += 1;
            continue;
        }
        match bytes.get(cursor + 1) {
            Some(b'$') => cursor += 2,
            Some(b'{') => {
                let Some(relative_end) = raw[cursor + 2..].find('}') else {
                    return Err(MakeExprError::UnsupportedReference {
                        reference: raw[cursor..].to_owned(),
                    });
                };
                cursor += relative_end + 3;
            }
            Some(_) => {
                let end = (cursor + 2).min(raw.len());
                return Err(MakeExprError::UnsupportedReference {
                    reference: raw[cursor..end].to_owned(),
                });
            }
            None => {
                return Err(MakeExprError::UnsupportedReference {
                    reference: "$".to_owned(),
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{evaluate_make_expr, evaluate_make_list, MakeExprContext, MakeExprError};
    use crate::dirs::DirVars;
    use crate::make_vars::collect_vars;
    use crate::parser::join_continuations;
    use aros_common::read_source;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    struct TempTree(PathBuf);

    impl TempTree {
        fn new() -> Self {
            let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("aros-make-expr-{}-{serial}", std::process::id()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..")
    }

    fn evaluate(src: &str, expression: &str) -> Result<String, MakeExprError> {
        let scope = collect_vars(src);
        let dirs = DirVars::load(Path::new("/path/which/does/not/exist"));
        let context = MakeExprContext::new(
            &scope,
            &dirs,
            usize::MAX,
            Path::new("."),
            Path::new("fixture"),
        );
        evaluate_make_expr(expression, &context)
    }

    #[test]
    fn nested_prefix_and_suffix_match_compiler_startup() {
        let value = evaluate(
            "NIXFILES := startup crt\n",
            "$(addprefix $(GENDIR)/$(CURDIR)/nix/,$(addsuffix .o,$(NIXFILES)))",
        )
        .unwrap();
        assert_eq!(
            value,
            "${AROS_BUILD_DIR}/gen/fixture/nix/startup.o \
             ${AROS_BUILD_DIR}/gen/fixture/nix/crt.o"
        );
    }

    #[test]
    fn filters_and_both_substitution_reference_forms_are_aligned() {
        let value = evaluate(
            "BASE := source/a.c source/b.cpp source/c.c\nSKIP := source/c\n",
            "$(filter-out $(SKIP),$(BASE:%.c=%)) $(BASE:.cpp=.cc)",
        )
        .unwrap();
        assert_eq!(
            value,
            "source/a source/b.cpp source/a.c source/b.cc source/c.c"
        );
    }

    #[test]
    fn subst_handles_archive_version_spellings() {
        assert_eq!(
            evaluate(
                "VERSION := 2.14.3\n",
                "$(subst .,,$(VERSION)) $(subst .,_,$(VERSION))",
            ),
            Ok("2143 2_14_3".to_owned())
        );
    }

    #[test]
    fn patsubst_filter_and_path_functions_cover_real_shapes() {
        let value = evaluate(
            "FILES := src/a.c src/b.cpp include/c.h archive.tar.gz plain\n",
            "$(patsubst src/%,gen/%,$(filter %.c %.cpp,$(FILES)))",
        )
        .unwrap();
        assert_eq!(value, "gen/a.c gen/b.cpp");
        assert_eq!(
            evaluate("", "$(notdir a/b.c plain tail/)"),
            Ok("b.c plain".to_owned())
        );
        assert_eq!(
            evaluate("", "$(dir a/b.c plain /root.c)"),
            Ok("a/ ./ /".to_owned())
        );
        assert_eq!(
            evaluate("", "$(basename a/b.c plain archive.tar.gz)"),
            Ok("a/b plain archive.tar".to_owned())
        );
        assert_eq!(
            evaluate("", "$(suffix a/b.c plain archive.tar.gz)"),
            Ok(".c .gz".to_owned())
        );
    }

    #[test]
    fn sort_and_strip_use_make_word_semantics() {
        assert_eq!(
            evaluate("LIST := z a z b\n", "$(sort $(strip   $(LIST)   c  ))"),
            Ok("a b c z".to_owned())
        );
    }

    #[test]
    fn computed_variable_names_and_source_order_are_supported() {
        let joined = "ID := 2\nFILES_2 := old.c\nuse\nFILES_2 := new.c\n";
        let scope = collect_vars(joined);
        let dirs = DirVars::load(Path::new("/path/which/does/not/exist"));
        let context = MakeExprContext::new(&scope, &dirs, 2, Path::new("."), Path::new("fixture"));
        assert_eq!(
            evaluate_make_list("$($(addprefix FILES_,$(ID)))", &context).unwrap(),
            vec!["old.c"]
        );
    }

    #[test]
    fn simple_and_recursive_assignments_observe_different_times() {
        let source = "BASE = old\n\
                      RECURSIVE_BASE = $(BASE)\n\
                      SIMPLE := $(RECURSIVE_BASE)\n\
                      RECURSIVE = $(BASE)\n\
                      BASE = new\n";
        assert_eq!(evaluate(source, "$(SIMPLE)"), Ok("old".to_owned()));
        assert_eq!(evaluate(source, "$(RECURSIVE)"), Ok("new".to_owned()));

        let appended = "BASE = old\n\
                        SIMPLE := first\n\
                        SIMPLE += $(BASE)\n\
                        RECURSIVE = first\n\
                        RECURSIVE += $(BASE)\n\
                        BASE = new\n";
        assert_eq!(
            evaluate(appended, "$(SIMPLE) $(RECURSIVE)"),
            Ok("first old first new".to_owned())
        );
    }

    #[test]
    fn collector_lookup_values_fall_back_to_global_directory_variables() {
        let source = root();
        let scope = collect_vars("");
        let dirs = DirVars::load(&source);
        let lookup = |name: &str| {
            (name == "LOCAL_PORT_DIR").then(|| "$(PORTSDIR)/Example/source".to_owned())
        };
        let context = MakeExprContext::new(
            &scope,
            &dirs,
            usize::MAX,
            &source,
            Path::new("external/example"),
        )
        .with_lookup(&lookup);

        assert_eq!(
            evaluate_make_expr("$(LOCAL_PORT_DIR)/file.c", &context).unwrap(),
            "${AROS_PORTS_DIR}/Example/source/file.c"
        );
        assert_eq!(
            evaluate_make_expr("$(TOP)/generated", &context).unwrap(),
            "${AROS_BUILD_DIR}/generated"
        );
    }

    #[test]
    fn a_simple_local_directory_does_not_reshadow_its_global_base() {
        let source = root();
        let scope = collect_vars(
            "TARGETDIR := $(AROS_TESTS)/Library\n\
             CUNITEXEDIR := $(AROS_TESTS)/cunit/genmodule/library\n",
        );
        let dirs = DirVars::load(&source);
        let context = MakeExprContext::new(
            &scope,
            &dirs,
            usize::MAX,
            &source,
            Path::new("developer/debug/test/library"),
        );

        assert_eq!(
            evaluate_make_expr("$(TARGETDIR)", &context).unwrap(),
            "${AROS_BUILD_DIR}/SYS/Developer/Debug/Tests/Library"
        );
        assert_eq!(
            evaluate_make_expr("$(CUNITEXEDIR)", &context).unwrap(),
            "${AROS_BUILD_DIR}/SYS/Developer/Debug/Tests/cunit/genmodule/library"
        );
    }

    #[test]
    fn wildcard_is_sorted_and_call_wildcard_keeps_only_regular_files() {
        let tree = TempTree::new();
        let rel = Path::new("locale");
        fs::create_dir_all(tree.0.join(rel).join("directory.po")).unwrap();
        fs::write(tree.0.join(rel).join("z.po"), "").unwrap();
        fs::write(tree.0.join(rel).join("a.po"), "").unwrap();
        let scope = collect_vars("");
        let dirs = DirVars::load(Path::new("/path/which/does/not/exist"));
        let context = MakeExprContext::new(&scope, &dirs, usize::MAX, &tree.0, rel);

        let rendered_source = evaluate_make_expr("$(SRCDIR)/$(CURDIR)/a.po", &context).unwrap();
        assert_eq!(rendered_source, "${CMAKE_SOURCE_DIR}/locale/a.po");
        assert!(!rendered_source.contains(&tree.0.display().to_string()));

        assert_eq!(
            evaluate_make_list("$(wildcard *.po)", &context).unwrap(),
            vec!["a.po", "directory.po", "z.po"]
        );
        assert_eq!(
            evaluate_make_list("$(call WILDCARD,*.po)", &context).unwrap(),
            vec!["a.po", "z.po"]
        );
        let source_matches =
            evaluate_make_list("$(call WILDCARD,$(SRCDIR)/$(CURDIR)/*.po)", &context).unwrap();
        assert_eq!(
            source_matches,
            vec![
                "${CMAKE_SOURCE_DIR}/locale/a.po",
                "${CMAKE_SOURCE_DIR}/locale/z.po"
            ]
        );
        assert!(!source_matches
            .join(" ")
            .contains(&tree.0.display().to_string()));
        assert_eq!(
            evaluate_make_list(
                "$(basename $(notdir $(call WILDCARD,$(SRCDIR)/$(CURDIR)/*.po)))",
                &context,
            )
            .unwrap(),
            vec!["a", "z"]
        );
    }

    #[test]
    fn missing_cycles_and_unsupported_syntax_are_never_empty_successes() {
        let missing = evaluate("", "$(DOES_NOT_EXIST)").unwrap_err();
        assert!(matches!(
            missing,
            MakeExprError::UnresolvedVariables { names, .. }
                if names == vec!["DOES_NOT_EXIST"]
        ));

        let cycle = evaluate("A := $(B)\nB := $(A)\n", "$(A)").unwrap_err();
        assert!(matches!(cycle, MakeExprError::VariableCycle { .. }));

        assert!(matches!(
            evaluate("", "$(eval SOMETHING := x)"),
            Err(MakeExprError::UnsupportedFunction { name }) if name == "eval"
        ));
        assert!(matches!(
            evaluate("", "$(call SOMETHING,x)"),
            Err(MakeExprError::UnsupportedFunction { .. })
        ));
        assert!(matches!(
            evaluate("", "$(notdir $@)"),
            Err(MakeExprError::UnsupportedReference { reference }) if reference == "$@"
        ));
        assert!(matches!(
            evaluate("", "$(BROKEN"),
            Err(MakeExprError::InvalidSyntax { .. })
        ));
        assert_eq!(evaluate("A := foo\n", "${A} $$x"), Ok("foo $x".to_owned()));
    }

    #[test]
    fn conditional_variable_guard_wins_over_all_value_lookups() {
        let conditional_scope =
            collect_vars("ifeq ($(ARCH),pc)\nFILES := pc.c\nelse\nFILES := other.c\nendif\n");
        let dirs = DirVars::load(Path::new("/path/which/does/not/exist"));
        let conditional_context = MakeExprContext::new(
            &conditional_scope,
            &dirs,
            usize::MAX,
            Path::new("."),
            Path::new("fixture"),
        );
        assert!(matches!(
            evaluate_make_expr("$(FILES)", &conditional_context),
            Err(MakeExprError::UnsafeVariable { name, detail, .. })
                if name == "FILES" && detail.contains("unevaluated Make conditional")
        ));

        let scope = collect_vars("FILES := last-branch.c\n");
        let lookup = |name: &str| (name == "FILES").then(|| "collector.c".to_owned());
        let guard = |name: &str| {
            (name == "FILES").then(|| "assigned in both sides of an undecidable ifeq".to_owned())
        };
        let context = MakeExprContext::new(
            &scope,
            &dirs,
            usize::MAX,
            Path::new("."),
            Path::new("fixture"),
        )
        .with_lookup(&lookup)
        .with_guard(&guard);

        assert!(matches!(
            evaluate_make_expr("$(FILES)", &context),
            Err(MakeExprError::UnsafeVariable { name, detail, .. })
                if name == "FILES" && detail.contains("undecidable ifeq")
        ));
    }

    #[test]
    fn deferred_cmake_paths_cannot_silently_become_empty_wildcards() {
        assert!(matches!(
            evaluate("FILES := $(wildcard $(GENDIR)/*.c)\n", "$(FILES)"),
            Err(MakeExprError::DeferredWildcard { pattern })
                if pattern == "${AROS_BUILD_DIR}/gen/*.c"
        ));
    }

    #[test]
    fn real_language_module_expression_matches_the_source_tree() {
        let source = root();
        let relative = Path::new("workbench/locale/languages");
        let text = read_source(&source.join(relative).join("mmakefile.src")).unwrap();
        let joined = join_continuations(&text);
        let scope = collect_vars(&joined);
        let dirs = DirVars::load(&source);
        let context = MakeExprContext::new(&scope, &dirs, usize::MAX, &source, relative);

        let languages = evaluate_make_list("$(LANGUAGES)", &context).unwrap();
        assert_eq!(languages.len(), 30);
        assert_eq!(languages.first().map(String::as_str), Some("albanian"));
        assert!(languages.iter().any(|name| name == "portuguese-brazil"));

        let modules = evaluate_make_list("$(MODULES)", &context).unwrap();
        assert_eq!(modules.len(), languages.len());
        assert_eq!(
            modules.first().map(String::as_str),
            Some("${AROS_BUILD_DIR}/SYS/Locale/Languages/albanian.language")
        );
    }

    #[test]
    fn foreach_binds_its_loop_variable_and_shadows_a_global() {
        // rom/dos:42 verbatim: without this, dos.library has no ELF loader.
        assert_eq!(
            evaluate("", "$(foreach img, aos elf, internalloadseg_$(img))").unwrap(),
            "internalloadseg_aos internalloadseg_elf"
        );
        // The binding is temporary: a global of the same name is shadowed
        // inside the body and intact outside it.
        assert_eq!(
            evaluate("f := global\n", "$(foreach f,one two,classes/$(f)) $(f)").unwrap(),
            "classes/one classes/two global"
        );
        // Nesting, and an empty list yielding nothing.
        assert_eq!(
            evaluate("", "$(foreach a,x y,$(foreach b,1 2,$(a)$(b)))").unwrap(),
            "x1 x2 y1 y2"
        );
        assert_eq!(evaluate("", "[$(foreach a,,body)]").unwrap(), "[]");
    }
}
