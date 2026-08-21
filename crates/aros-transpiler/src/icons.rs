//! `%build_icons`: Workbench icon declarations.
//!
//! An icon declaration has two deliberately separate representations. The
//! [`IconTarget`] is the nameable mmake identity and is retained even when the
//! declaration has no icons for the selected architecture or cannot be
//! resolved. An [`IconSet`] is one concrete output rule attached to it.

use crate::dirs::DirVars;
use crate::parser::{collect_vars, VarScope};
use std::path::Path;

const MAX_EXPANSION_DEPTH: usize = 32;

/// One resolved `%build_icons` output rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconSet {
    /// The mmake id, which is also the outer CMake target name.
    pub mmake: String,
    /// Destination directory as a CMake expression.
    pub dir: String,
    /// Where the `.info.src` and image files live, relative to the source tree.
    pub srcdir: String,
    /// Icon base names, without `.info`. May legitimately be empty.
    pub icons: Vec<String>,
    /// Shared images. Empty means each icon takes its own `<name>.<fmt>`.
    pub images: Vec<String>,
    /// Image extension, `fmt=`, default `png`.
    pub fmt: String,
    /// CMake `if()` expression, without the surrounding `if(...)`.
    pub condition: Option<String>,
    /// Configured icon family for an active-icon-set rule.
    pub iconset: Option<String>,
    /// One-based line in the continuation-joined mmakefile.
    pub line: usize,
}

/// The persistent identity of a `%build_icons` declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconTarget {
    /// The mmake id / CMake target name.
    pub mmake: String,
    /// Source directory containing the declaring mmakefile.
    pub directory: String,
}

/// Complete icon scan of one mmakefile.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IconScan {
    /// One entry for every declaration, including duplicate mmake ids.
    pub targets: Vec<IconTarget>,
    /// Resolved, condition-grouped output rules.
    pub sets: Vec<IconSet>,
    /// Declarations or values which could not be represented safely.
    pub skipped: Vec<String>,
}

#[derive(Debug, Clone)]
struct IconInvocation {
    args: String,
    /// Zero-based line for [`VarScope`].
    line: usize,
}

/// A representative configuration for the only target-dependent icon
/// declarations in the tree. Together these rows exhaustively partition the
/// `ifeq`/`ifneq` forms used by both monitor mmakefiles.
struct TargetContext {
    arch: &'static str,
    cpu: &'static str,
    cmake: &'static str,
}

struct RuleGroup {
    set: IconSet,
    contexts: Vec<usize>,
}

const TARGET_CONTEXTS: &[TargetContext] = &[
    TargetContext {
        arch: "pc",
        cpu: "x86_64",
        cmake: "AROS_TARGET_PLATFORM STREQUAL \"pc\"",
    },
    TargetContext {
        arch: "opensbi",
        cpu: "riscv64",
        cmake: "AROS_TARGET_PLATFORM STREQUAL \"opensbi\"",
    },
    TargetContext {
        arch: "amiga",
        cpu: "m68k",
        cmake: "(AROS_TARGET_PLATFORM STREQUAL \"amiga\") AND (AROS_TARGET_CPU STREQUAL \"m68k\")",
    },
    TargetContext {
        arch: "amiga",
        cpu: "ppc",
        cmake: "(AROS_TARGET_PLATFORM STREQUAL \"amiga\") AND (NOT AROS_TARGET_CPU STREQUAL \"m68k\")",
    },
    TargetContext {
        arch: "raspi",
        cpu: "aarch64",
        cmake: "(NOT AROS_TARGET_PLATFORM STREQUAL \"pc\") AND (NOT AROS_TARGET_PLATFORM STREQUAL \"opensbi\") AND (NOT AROS_TARGET_PLATFORM STREQUAL \"amiga\")",
    },
];

/// Collects every `%build_icons` identity and all representable output rules.
///
/// For a conditional file this evaluates five representative, exhaustive
/// target contexts, groups equal results by declaration line, and emits one
/// CMake condition per distinct result.
#[must_use]
pub fn collect_icons_all(joined: &str, dirs: &DirVars, rel_dir: &Path) -> IconScan {
    let invocations = icon_invocations(joined);
    let directory = slash_path(rel_dir);
    let mut scan = IconScan::default();

    for invocation in &invocations {
        match arg(&invocation.args, "mmake") {
            Some(mmake) if !mmake.is_empty() => scan.targets.push(IconTarget {
                mmake,
                directory: directory.clone(),
            }),
            _ => push_unique(
                &mut scan.skipped,
                format!(
                    "{}:{}: %build_icons without an mmake id",
                    rel_dir.display(),
                    invocation.line + 1
                ),
            ),
        }
    }

    // The parser calls this for every mmakefile. Conditionals in an unrelated
    // file must not be interpreted as icon conditionals (or reported five
    // times through the representative target contexts).
    if invocations.is_empty() {
        return scan;
    }

    if !has_make_conditionals(joined) {
        let scope = collect_vars(joined);
        let (sets, skipped) = collect_icons_with_scope(joined, &scope, dirs, rel_dir, None);
        scan.sets = sets;
        for message in skipped {
            push_unique(&mut scan.skipped, message);
        }
        return scan;
    }

    let mut groups: Vec<RuleGroup> = Vec::new();
    for (context_index, context) in TARGET_CONTEXTS.iter().enumerate() {
        let filtered = match filter_conditionals(joined, context) {
            Ok(filtered) => filtered,
            Err(reason) => {
                push_unique(
                    &mut scan.skipped,
                    format!(
                        "{}: cannot evaluate icon conditionals for {}/{}: {reason}",
                        rel_dir.display(),
                        context.arch,
                        context.cpu
                    ),
                );
                continue;
            }
        };
        let scope = collect_vars(&filtered);
        let (sets, skipped) = collect_icons_with_scope(&filtered, &scope, dirs, rel_dir, None);
        for message in skipped {
            push_unique(&mut scan.skipped, message);
        }
        for set in sets {
            if let Some(group) = groups.iter_mut().find(|group| same_rule(&group.set, &set)) {
                if !group.contexts.contains(&context_index) {
                    group.contexts.push(context_index);
                }
            } else {
                groups.push(RuleGroup {
                    set,
                    contexts: vec![context_index],
                });
            }
        }
    }

    scan.sets = groups
        .into_iter()
        .map(|mut group| {
            group.set.condition = if group.contexts.len() == TARGET_CONTEXTS.len() {
                None
            } else {
                let expressions: Vec<&str> = group
                    .contexts
                    .iter()
                    .map(|index| TARGET_CONTEXTS[*index].cmake)
                    .collect();
                Some(if expressions.len() == 1 {
                    expressions[0].to_owned()
                } else {
                    expressions
                        .iter()
                        .map(|expression| format!("({expression})"))
                        .collect::<Vec<_>>()
                        .join(" OR ")
                })
            };
            group.set
        })
        .collect();
    scan.sets.sort_by(|a, b| {
        a.line
            .cmp(&b.line)
            .then_with(|| a.mmake.cmp(&b.mmake))
            .then_with(|| a.dir.cmp(&b.dir))
            .then_with(|| a.icons.cmp(&b.icons))
            .then_with(|| a.condition.cmp(&b.condition))
    });
    scan
}

/// Reads `%build_icons` declarations using a caller-supplied variable scope.
/// Prefer [`collect_icons_all`] for a complete conditional-aware scan.
#[must_use]
pub fn collect_icons(
    joined: &str,
    scope: &VarScope,
    dirs: &DirVars,
    rel_dir: &Path,
) -> (Vec<IconSet>, Vec<String>) {
    collect_icons_with_scope(joined, scope, dirs, rel_dir, None)
}

fn collect_icons_with_scope(
    joined: &str,
    scope: &VarScope,
    dirs: &DirVars,
    rel_dir: &Path,
    condition: Option<&str>,
) -> (Vec<IconSet>, Vec<String>) {
    let mut out = Vec::new();
    let mut skipped = Vec::new();

    for invocation in icon_invocations(joined) {
        let line = invocation.line;
        let line_display = line + 1;
        let Some(mmake) = arg(&invocation.args, "mmake").filter(|value| !value.is_empty()) else {
            skipped.push(format!(
                "{}:{line_display}: %build_icons without an mmake id",
                rel_dir.display()
            ));
            continue;
        };

        let Some(dir_raw) = arg(&invocation.args, "dir") else {
            skipped.push(format!(
                "{}:{line_display}: %build_icons mmake={mmake} without a dir",
                rel_dir.display()
            ));
            continue;
        };
        let dir = match expand_scoped(&dir_raw, scope, dirs, line) {
            Ok(value) if !value.trim().is_empty() => value,
            Ok(_) => {
                skipped.push(format!(
                    "{}:{line_display}: %build_icons mmake={mmake} dir={dir_raw} resolved to an empty path",
                    rel_dir.display()
                ));
                continue;
            }
            Err(reason) => {
                skipped.push(format!(
                    "{}:{line_display}: %build_icons mmake={mmake} dir={dir_raw}: {reason}",
                    rel_dir.display()
                ));
                continue;
            }
        };

        let srcdir = match arg(&invocation.args, "srcdir") {
            None => slash_path(rel_dir),
            Some(raw) => match expand_scoped(&raw, scope, dirs, line) {
                Ok(value) if !value.trim().is_empty() => value,
                Ok(_) => {
                    skipped.push(format!(
                        "{}:{line_display}: %build_icons mmake={mmake} srcdir={raw} resolved to an empty path",
                        rel_dir.display()
                    ));
                    continue;
                }
                Err(reason) => {
                    skipped.push(format!(
                        "{}:{line_display}: %build_icons mmake={mmake} srcdir={raw}: {reason}",
                        rel_dir.display()
                    ));
                    continue;
                }
            },
        };

        let Some(icons_raw) = arg(&invocation.args, "icons") else {
            skipped.push(format!(
                "{}:{line_display}: %build_icons mmake={mmake} without icons=",
                rel_dir.display()
            ));
            continue;
        };
        let icons = match expand_list(&icons_raw, scope, dirs, line) {
            Ok(icons) => icons,
            Err(reason) => {
                skipped.push(format!(
                    "{}:{line_display}: %build_icons mmake={mmake} icons={icons_raw}: {reason}",
                    rel_dir.display()
                ));
                continue;
            }
        };

        let images = match arg(&invocation.args, "image") {
            None => Vec::new(),
            Some(raw) => match expand_list(&raw, scope, dirs, line) {
                Ok(images) if images.len() <= 2 => images,
                Ok(images) => {
                    skipped.push(format!(
                        "{}:{line_display}: %build_icons mmake={mmake} image={raw} resolved to {} images; ilbmtoicon accepts at most two",
                        rel_dir.display(),
                        images.len()
                    ));
                    continue;
                }
                Err(reason) => {
                    skipped.push(format!(
                        "{}:{line_display}: %build_icons mmake={mmake} image={raw}: {reason}",
                        rel_dir.display()
                    ));
                    continue;
                }
            },
        };

        let fmt = match arg(&invocation.args, "fmt") {
            None => "png".to_owned(),
            Some(raw) => match expand_scoped(&raw, scope, dirs, line) {
                Ok(value) => {
                    let words = make_words(&value);
                    match words.as_slice() {
                        [] => "png".to_owned(),
                        [one] => one.clone(),
                        _ => {
                            skipped.push(format!(
                                "{}:{line_display}: %build_icons mmake={mmake} fmt={raw} resolved to more than one value",
                                rel_dir.display()
                            ));
                            continue;
                        }
                    }
                }
                Err(reason) => {
                    skipped.push(format!(
                        "{}:{line_display}: %build_icons mmake={mmake} fmt={raw}: {reason}",
                        rel_dir.display()
                    ));
                    continue;
                }
            },
        };

        out.push(IconSet {
            iconset: infer_iconset(&mmake, rel_dir),
            mmake,
            dir,
            srcdir,
            icons,
            images,
            fmt,
            condition: condition.map(str::to_owned),
            line: line_display,
        });
    }

    (out, skipped)
}

fn same_rule(a: &IconSet, b: &IconSet) -> bool {
    a.line == b.line
        && a.mmake == b.mmake
        && a.dir == b.dir
        && a.srcdir == b.srcdir
        && a.icons == b.icons
        && a.images == b.images
        && a.fmt == b.fmt
        && a.iconset == b.iconset
}

fn icon_invocations(joined: &str) -> Vec<IconInvocation> {
    joined
        .lines()
        .enumerate()
        .filter_map(|(line, raw)| {
            let trimmed = raw.trim_start();
            let args = trimmed.strip_prefix("%build_icons")?;
            if !args.is_empty() && !args.starts_with(char::is_whitespace) {
                return None;
            }
            Some(IconInvocation {
                args: args.trim_start().to_owned(),
                line,
            })
        })
        .collect()
}

fn has_make_conditionals(joined: &str) -> bool {
    joined
        .lines()
        .any(|line| conditional_directive(line.trim()).is_some())
}

struct ConditionalFrame {
    parent_active: bool,
    condition: bool,
    in_else: bool,
}

impl ConditionalFrame {
    const fn active(&self) -> bool {
        self.parent_active
            && if self.in_else {
                !self.condition
            } else {
                self.condition
            }
    }
}

fn filter_conditionals(joined: &str, context: &TargetContext) -> Result<String, String> {
    let mut out = String::with_capacity(joined.len());
    let mut stack: Vec<ConditionalFrame> = Vec::new();

    for (line_number, line) in joined.split('\n').enumerate() {
        if line_number != 0 {
            out.push('\n');
        }
        let trimmed = line.trim();
        if let Some((want_equal, arguments)) = conditional_directive(trimmed) {
            let parent_active = stack.last().is_none_or(ConditionalFrame::active);
            let condition = if parent_active {
                evaluate_conditional(arguments, want_equal, context)
                    .map_err(|reason| format!("line {} (`{trimmed}`): {reason}", line_number + 1))?
            } else {
                false
            };
            stack.push(ConditionalFrame {
                parent_active,
                condition,
                in_else: false,
            });
            continue;
        }
        if trimmed == "else" {
            let Some(frame) = stack.last_mut() else {
                return Err(format!("line {}: else without ifeq/ifneq", line_number + 1));
            };
            if frame.in_else {
                return Err(format!("line {}: duplicate else", line_number + 1));
            }
            frame.in_else = true;
            continue;
        }
        if trimmed == "endif" {
            if stack.pop().is_none() {
                return Err(format!(
                    "line {}: endif without ifeq/ifneq",
                    line_number + 1
                ));
            }
            continue;
        }

        if stack.last().is_none_or(ConditionalFrame::active) {
            out.push_str(line);
        }
    }

    if stack.is_empty() {
        Ok(out)
    } else {
        Err(format!("{} unterminated ifeq/ifneq block(s)", stack.len()))
    }
}

/// Returns `(want_equal, arguments)` for a Make conditional directive.
fn conditional_directive(line: &str) -> Option<(bool, &str)> {
    for (keyword, want_equal) in [("ifeq", true), ("ifneq", false)] {
        let Some(rest) = line.strip_prefix(keyword) else {
            continue;
        };
        if rest.is_empty() || rest.starts_with(char::is_whitespace) || rest.starts_with('(') {
            return Some((want_equal, rest.trim()));
        }
    }
    None
}

fn evaluate_conditional(
    arguments: &str,
    want_equal: bool,
    context: &TargetContext,
) -> Result<bool, String> {
    let inner = arguments
        .strip_prefix('(')
        .and_then(|value| value.strip_suffix(')'))
        .ok_or_else(|| "expected ifeq/ifneq (left,right)".to_owned())?;
    let (left, right) = split_top_level_comma(inner)
        .ok_or_else(|| "expected a top-level comma in ifeq/ifneq".to_owned())?;
    let mut expander = MakeExpander::for_context(context);
    let left = expander.expand(left)?.trim().trim_matches('"').to_owned();
    let right = expander.expand(right)?.trim().trim_matches('"').to_owned();
    Ok((left == right) == want_equal)
}

enum ExpansionEnvironment<'a> {
    Scoped {
        scope: &'a VarScope,
        dirs: &'a DirVars,
        line: usize,
    },
    Target(&'a TargetContext),
}

struct MakeExpander<'a> {
    environment: ExpansionEnvironment<'a>,
    stack: Vec<String>,
}

impl<'a> MakeExpander<'a> {
    const fn for_scope(scope: &'a VarScope, dirs: &'a DirVars, line: usize) -> Self {
        Self {
            environment: ExpansionEnvironment::Scoped { scope, dirs, line },
            stack: Vec::new(),
        }
    }

    const fn for_context(context: &'a TargetContext) -> Self {
        Self {
            environment: ExpansionEnvironment::Target(context),
            stack: Vec::new(),
        }
    }

    fn expand(&mut self, raw: &str) -> Result<String, String> {
        self.expand_depth(raw, MAX_EXPANSION_DEPTH)
    }

    fn expand_depth(&mut self, raw: &str, depth: usize) -> Result<String, String> {
        if depth == 0 {
            return Err("Make expansion exceeded its recursion limit".to_owned());
        }

        let mut out = String::with_capacity(raw.len());
        let mut rest = raw;
        while let Some(start) = rest.find("$(") {
            out.push_str(&rest[..start]);
            let open = start + 1;
            let close = matching_parenthesis(rest, open)
                .ok_or_else(|| format!("unclosed Make reference in `{raw}`"))?;
            let inner = &rest[open + 1..close];
            out.push_str(&self.expand_reference(inner, depth - 1)?);
            rest = &rest[close + 1..];
        }
        out.push_str(rest);
        Ok(out)
    }

    fn expand_reference(&mut self, inner: &str, depth: usize) -> Result<String, String> {
        let trimmed = inner.trim();
        if let Some(arguments) = function_arguments(trimmed, "filter-out") {
            return self.expand_filter(arguments, true, depth);
        }
        if let Some(arguments) = function_arguments(trimmed, "filter") {
            return self.expand_filter(arguments, false, depth);
        }
        if trimmed.contains(char::is_whitespace) {
            let function = trimmed.split_whitespace().next().unwrap_or(trimmed);
            return Err(format!("unsupported Make function `{function}`"));
        }
        self.expand_variable(trimmed, depth)
    }

    fn expand_filter(
        &mut self,
        arguments: &str,
        invert: bool,
        depth: usize,
    ) -> Result<String, String> {
        let (patterns, words) = split_top_level_comma(arguments)
            .ok_or_else(|| "filter/filter-out requires pattern,text".to_owned())?;
        let patterns = make_words(&self.expand_depth(patterns, depth)?);
        let words = make_words(&self.expand_depth(words, depth)?);
        let selected = words.into_iter().filter(|word| {
            let matches = patterns
                .iter()
                .any(|pattern| make_pattern_matches(pattern, word));
            matches != invert
        });
        Ok(selected.collect::<Vec<_>>().join(" "))
    }

    fn expand_variable(&mut self, name: &str, depth: usize) -> Result<String, String> {
        if name.is_empty() {
            return Err("empty Make variable reference".to_owned());
        }
        if self.stack.iter().any(|entry| entry == name) {
            let mut cycle = self.stack.clone();
            cycle.push(name.to_owned());
            return Err(format!(
                "cyclic Make variable reference: {}",
                cycle.join(" -> ")
            ));
        }

        let raw = match &self.environment {
            ExpansionEnvironment::Target(context) => match name {
                "AROS_TARGET_ARCH" => Some(context.arch.to_owned()),
                "AROS_TARGET_CPU" => Some(context.cpu.to_owned()),
                _ => None,
            },
            ExpansionEnvironment::Scoped { scope, line, .. } => scope.raw_at(name, *line),
        };

        if let Some(raw) = raw {
            self.stack.push(name.to_owned());
            let result = self.expand_depth(&raw, depth);
            self.stack.pop();
            return result;
        }

        match &self.environment {
            ExpansionEnvironment::Target(_) => Err(format!("undefined Make variable `{name}`")),
            ExpansionEnvironment::Scoped { scope, dirs, line } => {
                let local = |local_name: &str| scope.raw_at(local_name, *line);
                dirs.expand_with(&format!("$({name})"), &local)
                    .map_err(|missing| {
                        format!("unresolved Make variable(s): {}", missing.join(", "))
                    })
            }
        }
    }
}

fn expand_scoped(
    raw: &str,
    scope: &VarScope,
    dirs: &DirVars,
    line: usize,
) -> Result<String, String> {
    MakeExpander::for_scope(scope, dirs, line).expand(raw)
}

/// Expands a complete expression before splitting. The `Result` distinguishes
/// a defined empty list from an unresolved one.
fn expand_list(
    raw: &str,
    scope: &VarScope,
    dirs: &DirVars,
    line: usize,
) -> Result<Vec<String>, String> {
    expand_scoped(raw, scope, dirs, line).map(|value| make_words(&value))
}

fn make_words(value: &str) -> Vec<String> {
    value
        .split_whitespace()
        .map(|word| word.trim_matches(['"', '\\']))
        .filter(|word| !word.is_empty() && *word != "\\")
        .map(str::to_owned)
        .collect()
}

fn function_arguments<'a>(inner: &'a str, function: &str) -> Option<&'a str> {
    let rest = inner.strip_prefix(function)?;
    rest.strip_prefix(char::is_whitespace).map(str::trim_start)
}

fn matching_parenthesis(text: &str, open: usize) -> Option<usize> {
    debug_assert_eq!(text.as_bytes().get(open), Some(&b'('));
    let mut depth = 0usize;
    for (offset, character) in text[open..].char_indices() {
        match character {
            '(' => depth += 1,
            ')' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(open + offset);
                }
            }
            _ => {}
        }
    }
    None
}

fn split_top_level_comma(value: &str) -> Option<(&str, &str)> {
    let mut depth = 0usize;
    for (index, character) in value.char_indices() {
        match character {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => return Some((&value[..index], &value[index + 1..])),
            _ => {}
        }
    }
    None
}

fn make_pattern_matches(pattern: &str, word: &str) -> bool {
    let Some(wildcard) = pattern.find('%') else {
        return pattern == word;
    };
    let prefix = &pattern[..wildcard];
    let suffix = &pattern[wildcard + 1..];
    word.len() >= prefix.len() + suffix.len() && word.starts_with(prefix) && word.ends_with(suffix)
}

/// Infers the configured icon family from an active-icon-set or theme mmake id.
/// The longest case-insensitive source-component match keeps `Gorilla-old`
/// intact and normalizes the historic `themes-gorilla-*` spelling to
/// `Gorilla`; the prefix fallback still recognizes `GorillaSmall` below
/// `Gorilla/`.
fn infer_iconset(mmake: &str, rel_dir: &Path) -> Option<String> {
    let suffix = mmake
        .strip_prefix("iconset-")
        .or_else(|| mmake.strip_prefix("themes-"))?;
    if suffix.is_empty() {
        return None;
    }

    let mut components: Vec<String> = rel_dir
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .filter(|component| !component.is_empty())
        .collect();
    components.sort_by_key(|component| std::cmp::Reverse(component.len()));
    if let Some(component) = components.into_iter().find(|component| {
        let suffix = suffix.to_ascii_lowercase();
        let component = component.to_ascii_lowercase();
        suffix == component
            || suffix
                .strip_prefix(&component)
                .is_some_and(|rest| rest.starts_with('-'))
    }) {
        return Some(component);
    }

    Some(suffix.split('-').next().unwrap_or(suffix).to_owned())
}

/// Reads `key=value` or `key="value"` at a word boundary. Unquoted values may
/// contain whitespace inside nested Make expressions. `Some("")` represents
/// an explicitly empty argument.
fn arg(args: &str, key: &str) -> Option<String> {
    let mut from = 0usize;
    loop {
        let hit = args[from..].find(key)? + from;
        let at_boundary = hit == 0
            || args[..hit]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace);
        let rest = &args[hit + key.len()..];
        if at_boundary {
            if let Some(value) = rest.strip_prefix("=\"") {
                let end = value.find('"')?;
                return Some(value[..end].to_owned());
            }
            if let Some(value) = rest.strip_prefix('=') {
                let mut depth = 0usize;
                let mut end = value.len();
                for (index, character) in value.char_indices() {
                    match character {
                        '(' => depth += 1,
                        ')' => depth = depth.saturating_sub(1),
                        _ if character.is_whitespace() && depth == 0 => {
                            end = index;
                            break;
                        }
                        _ => {}
                    }
                }
                return Some(value[..end].to_owned());
            }
        }
        from = hit + 1;
    }
}

fn slash_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.contains(&value) {
        values.push(value);
    }
}

#[cfg(test)]
mod tests {
    use super::{arg, collect_icons, collect_icons_all, infer_iconset};
    use crate::dirs::DirVars;
    use crate::parser::{collect_vars, join_continuations};
    use aros_common::read_source;
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};
    use walkdir::WalkDir;

    fn root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..")
    }

    fn dirs() -> DirVars {
        DirVars::load(&root())
    }

    fn collect(src: &str) -> (Vec<super::IconSet>, Vec<String>) {
        let joined = join_continuations(src);
        let scope = collect_vars(&joined);
        collect_icons(&joined, &scope, &dirs(), Path::new("x"))
    }

    #[test]
    fn arguments_observe_boundaries_and_nested_whitespace() {
        let args =
            "mmake=x icons=$(filter-out VMWare,$(ALL)) srcdir=$(SRCDIR)/other dir=$(AROSDIR)";
        assert_eq!(arg(args, "dir").unwrap(), "$(AROSDIR)");
        assert_eq!(arg(args, "srcdir").unwrap(), "$(SRCDIR)/other");
        assert_eq!(arg(args, "icons").unwrap(), "$(filter-out VMWare,$(ALL))");
    }

    #[test]
    fn resolved_rule_has_new_metadata_and_one_based_line() {
        let src = "\
BASEICONS := Developer Devs Fonts

%build_icons mmake=iconset-GorillaSmall-wbench-icons-aros icons=$(BASEICONS) dir=$(AROSDIR)
";
        let joined = join_continuations(src);
        let scope = collect_vars(&joined);
        let (sets, skipped) = collect_icons(
            &joined,
            &scope,
            &dirs(),
            Path::new("images/IconSets/Gorilla/Icons/Small/AROS"),
        );
        assert!(skipped.is_empty(), "{skipped:?}");
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].dir, "${AROS_BUILD_DIR}/SYS");
        assert_eq!(sets[0].icons, ["Developer", "Devs", "Fonts"]);
        assert_eq!(sets[0].srcdir, "images/IconSets/Gorilla/Icons/Small/AROS");
        assert_eq!(sets[0].iconset.as_deref(), Some("GorillaSmall"));
        assert_eq!(sets[0].line, 3);
        assert!(sets[0].condition.is_none());
    }

    #[test]
    fn full_list_expansion_handles_filter_out_and_two_images() {
        let src = "\
ALL := IntelGMA VMWare
ICONS := $(filter-out VMWare,$(ALL))
IMAGES := normal.png selected.png
%build_icons mmake=filtered icons=$(ICONS) image=$(IMAGES) dir=$(AROS_DEVS)
";
        let (sets, skipped) = collect(src);
        assert!(skipped.is_empty(), "{skipped:?}");
        assert_eq!(sets[0].icons, ["IntelGMA"]);
        assert_eq!(sets[0].images, ["normal.png", "selected.png"]);
    }

    #[test]
    fn unresolved_word_rejects_whole_list_and_names_variable() {
        let (sets, skipped) =
            collect("%build_icons mmake=broken icons=Good-$(MISSING) dir=$(AROS_DEVS)\n");
        assert!(sets.is_empty());
        assert_eq!(skipped.len(), 1);
        assert!(skipped[0].contains("MISSING"));
    }

    #[test]
    fn defined_empty_lists_are_valid_but_three_images_are_not() {
        let (sets, skipped) = collect(
            "ICONS :=\nEMPTY :=\n%build_icons mmake=empty icons=$(ICONS) image=$(EMPTY) dir=$(AROS_DEVS)\n",
        );
        assert!(skipped.is_empty(), "{skipped:?}");
        assert_eq!(sets.len(), 1);
        assert!(sets[0].icons.is_empty());
        assert!(sets[0].images.is_empty());

        let (sets, skipped) = collect(
            "IMAGES := one.png two.png three.png\n%build_icons mmake=too-many icons=A image=$(IMAGES) dir=$(AROS_DEVS)\n",
        );
        assert!(sets.is_empty());
        assert_eq!(skipped.len(), 1);
        assert!(skipped[0].contains("at most two"));
    }

    #[test]
    fn misspelled_directory_is_reported_but_target_survives() {
        let src = "ICONS := Calculator\n%build_icons mmake=broken icons=$(ICONS) dir=$(AROS_DIR__TOOLS)\n";
        let joined = join_continuations(src);
        let scan = collect_icons_all(&joined, &dirs(), Path::new("x"));
        assert_eq!(scan.targets.len(), 1);
        assert!(scan.sets.is_empty());
        assert_eq!(scan.skipped.len(), 1);
        assert!(scan.skipped[0].contains("AROS_DIR__TOOLS"));
    }

    #[test]
    fn iconset_inference_uses_longest_source_component_then_prefix() {
        assert_eq!(
            infer_iconset(
                "iconset-Gorilla-old-icons-aros",
                Path::new("images/IconSets/Gorilla-old/Icons/Medium/AROS")
            )
            .as_deref(),
            Some("Gorilla-old")
        );
        assert_eq!(
            infer_iconset(
                "iconset-GorillaSmall-wbench-icons-aros",
                Path::new("images/IconSets/Gorilla/Icons/Small/AROS")
            )
            .as_deref(),
            Some("GorillaSmall")
        );
        assert_eq!(
            infer_iconset("iconset-Gorilla-x11", Path::new("arch/all-hosted/x11")).as_deref(),
            Some("Gorilla")
        );
        assert_eq!(
            infer_iconset(
                "themes-gorilla-icons-computers",
                Path::new("images/IconSets/Gorilla/Icons/Medium/Computers")
            )
            .as_deref(),
            Some("Gorilla")
        );
        assert_eq!(
            infer_iconset(
                "themes-GorillaSmall-icons-computers",
                Path::new("images/IconSets/Gorilla/Icons/Small/Computers")
            )
            .as_deref(),
            Some("GorillaSmall")
        );
        assert_eq!(
            infer_iconset(
                "gorilla-icons-preset-copy",
                Path::new("images/IconSets/Gorilla")
            ),
            None
        );
    }

    #[test]
    fn small_monitor_conditionals_group_into_three_variants() {
        let src = "\
STORAGEICONS := Wrapper
INSTDEVSICONS := ATI IntelGMA NVidia
ifeq ($(AROS_TARGET_ARCH),pc)
INSTDEVSICONS += VMWare
endif
ifeq ($(AROS_TARGET_ARCH)-$(AROS_TARGET_CPU),amiga-m68k)
INSTDEVSICONS += SAGA Z3660 ZZ9000
endif
%build_icons mmake=iconset-GorillaSmall-wbench-icons-devs-monitors icons=$(INSTDEVSICONS) image=Wrapper.png dir=$(AROS_DEVS)/Monitors
%build_icons mmake=unconditional icons=$(STORAGEICONS) image=Wrapper.png dir=$(AROS_STORAGE)/Monitors
";
        let joined = join_continuations(src);
        let scan = collect_icons_all(
            &joined,
            &dirs(),
            Path::new("images/IconSets/Gorilla/Icons/Small/AROS/Devs/Monitors"),
        );
        assert!(scan.skipped.is_empty(), "{:?}", scan.skipped);
        assert_eq!(scan.targets.len(), 2);
        let conditional: Vec<_> = scan
            .sets
            .iter()
            .filter(|set| set.mmake.contains("devs-monitors"))
            .collect();
        assert_eq!(conditional.len(), 3);
        assert!(conditional
            .iter()
            .any(|set| set.icons == ["ATI", "IntelGMA", "NVidia", "VMWare"]));
        assert!(conditional
            .iter()
            .any(|set| { set.icons == ["ATI", "IntelGMA", "NVidia", "SAGA", "Z3660", "ZZ9000"] }));
        assert!(conditional
            .iter()
            .any(|set| set.icons == ["ATI", "IntelGMA", "NVidia"]));
        assert!(conditional.iter().all(|set| set.condition.is_some()));
        let unconditional = scan
            .sets
            .iter()
            .find(|set| set.mmake == "unconditional")
            .unwrap();
        assert!(unconditional.condition.is_none());
    }

    #[test]
    fn duplicate_declarations_keep_two_targets_and_two_lines() {
        let src = "\
ICONS := A
%build_icons mmake=same icons=$(ICONS) dir=$(AROS_DEVS)
%build_icons mmake=same icons=$(ICONS) dir=$(AROS_STORAGE)
";
        let joined = join_continuations(src);
        let scan = collect_icons_all(&joined, &dirs(), Path::new("x"));
        assert_eq!(scan.targets.len(), 2);
        assert_eq!(scan.sets.len(), 2);
        assert_ne!(scan.sets[0].line, scan.sets[1].line);
    }

    #[test]
    fn real_tree_has_all_181_declarations_and_178_identities() {
        let root = root();
        let dirs = dirs();
        let mut targets = Vec::new();
        let mut sets = Vec::new();
        let mut skipped = Vec::new();

        for entry in WalkDir::new(&root).into_iter().filter_map(Result::ok) {
            if !entry.file_type().is_file() || entry.file_name() != "mmakefile.src" {
                continue;
            }
            let text = read_source(entry.path()).unwrap();
            if !text.contains("%build_icons") {
                continue;
            }
            let joined = join_continuations(&text);
            let rel_dir = entry.path().parent().unwrap().strip_prefix(&root).unwrap();
            let scan = collect_icons_all(&joined, &dirs, rel_dir);
            targets.extend(scan.targets);
            sets.extend(scan.sets);
            skipped.extend(scan.skipped);
        }

        assert_eq!(targets.len(), 181);
        let ids: HashSet<_> = targets.iter().map(|target| target.mmake.as_str()).collect();
        assert_eq!(ids.len(), 178);
        assert_eq!(skipped.len(), 1, "{skipped:#?}");
        assert!(skipped[0].contains("AROS_DIR__TOOLS"));
        // 180 resolvable declarations plus six additional conditional
        // variants across the Medium and Small monitor files.
        assert_eq!(sets.len(), 186);

        let env_sets: Vec<_> = sets
            .iter()
            .filter(|set| {
                set.srcdir == "images/IconSets/Gorilla/Icons/Medium/AROS/Prefs/Env-Archive/SYS"
                    && set.mmake.ends_with("prefs-envarc")
            })
            .collect();
        assert_eq!(env_sets.len(), 2);
        assert!(env_sets.iter().all(|set| set.icons.len() == 80));
    }
}
