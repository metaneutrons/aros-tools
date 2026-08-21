//! Safe, bounded expansion of local source-tree Make include fragments.
//!
//! Some MetaMake declarations keep their source inventory in a sibling file:
//!
//! ```text
//! include $(SRCDIR)/$(CURDIR)/core.files
//! %build_linklib files="$(addprefix $(SRCDIR)/$(CURDIR)/,$(BTCORE_FILES))"
//! ```
//!
//! Reading the fragment at its include site lets the ordinary positional Make
//! evaluator see `BTCORE_FILES` without teaching it a project-specific name.
//! This module deliberately does less than GNU Make: it only accepts local,
//! side-effect-free fragments made from assignments, conditionals and further
//! local includes. Rules, recipes, MetaMake declarations, dynamic paths and
//! fragments which escape the source tree remain unexpanded and are returned
//! as structured diagnostics.
//!
//! Expansion is atomic. If a required nested include is unsafe or unresolved,
//! none of its parent fragment is inserted. This prevents a partial variable
//! scope from looking authoritative. The caller also gets assignment
//! provenance, so it can opt declarations in independently rather than making
//! every target which happens to include a syntactically safe file concrete.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

/// Default limits for one mmakefile expansion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalMakeIncludeLimits {
    /// Maximum number of nested local fragments below the mmakefile.
    pub depth: usize,
    /// Maximum number of fragment reads, including repeated non-cyclic reads.
    pub files: usize,
    /// Maximum total bytes read from fragments.
    pub bytes: usize,
}

/// Which proven-safe fragment shapes a caller wants to insert.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LocalMakeFragmentPolicy {
    /// Accept exactly one plain source-list variable, with no nested references
    /// or fragment-local conditionals.
    ///
    /// This intentionally small first tranche covers sibling inventories such
    /// as `core.files`, while leaving configuration fragments which can imply
    /// generated headers or fetched inputs visible for a later, declaration-
    /// aware implementation.
    #[default]
    PlainSourceLists,
    /// Accept the complete safe syntax subset documented by this module.
    /// Callers using this mode must separately account for generated outputs
    /// and recipes required by each declaration which consumes the variables.
    SafeVariableScopes,
}

impl Default for LocalMakeIncludeLimits {
    fn default() -> Self {
        Self {
            depth: 8,
            files: 64,
            bytes: 1024 * 1024,
        }
    }
}

/// The stable category of an include-expansion diagnostic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalMakeIncludeIssueKind {
    /// The source root or declaring mmakefile path was invalid.
    InvalidContext,
    /// A local include path still contains a variable or names several files.
    UnresolvedPath,
    /// A required or optional fragment does not exist.
    Missing,
    /// The resolved path leaves the source tree, including through a symlink.
    OutsideSourceTree,
    /// The fragment could not be read as UTF-8 text.
    Read,
    /// The include graph contains a recursion cycle.
    Cycle,
    /// The configured include nesting limit was reached.
    DepthLimit,
    /// The configured fragment-count limit was reached.
    FileLimit,
    /// The configured aggregate byte limit was reached.
    ByteLimit,
    /// A fragment contains a rule, recipe, build declaration or unsafe form.
    UnsafeSyntax,
    /// A candidate fragment imports a non-local scope which this module cannot
    /// prove safe, so the candidate is rejected atomically.
    NestedNonLocalInclude,
    /// The fragment is safe to read but broader than the caller-selected
    /// declaration scope, so it remains deferred and reportable.
    DeferredScope,
}

/// One local include which could not be expanded faithfully.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalMakeIncludeIssue {
    /// Source-relative file containing the include or rejected syntax.
    pub source: PathBuf,
    /// One-based physical line in `source`.
    pub line: usize,
    /// Stable diagnostic category.
    pub kind: LocalMakeIncludeIssueKind,
    /// Include argument or rejected logical line.
    pub subject: String,
    /// Human-readable reason suitable for a generated report.
    pub detail: String,
}

impl std::fmt::Display for LocalMakeIncludeIssue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{}:{}: {}: {}",
            self.source.display(),
            self.line,
            self.subject,
            self.detail
        )
    }
}

/// Provenance for one fragment inserted at an include site.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IncludedLocalMakeFragment {
    /// Source-relative path of the fragment.
    pub path: PathBuf,
    /// Source-relative file containing the include directive.
    pub included_from: PathBuf,
    /// One-based physical include line in `included_from`.
    pub include_line: usize,
    /// Variables assigned by this fragment, in lexical order.
    pub assigned_variables: Vec<String>,
    /// Whether this fragment contains an `ifeq`-family conditional.
    ///
    /// A caller can use this to introduce a narrower first tranche without
    /// silently opting a large conditional configuration fragment into target
    /// generation.
    pub has_conditionals: bool,
    /// Whether this is exactly one plain, self-contained list assignment.
    pub plain_source_list: bool,
}

/// Result of scanning one mmakefile for local source-tree includes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalMakeIncludeScan {
    /// Original mmakefile text with every proven-safe fragment inserted at its
    /// include site. Rejected and unrelated include lines remain untouched.
    pub expanded: String,
    /// Every fragment actually inserted, including nested and repeated ones.
    pub fragments: Vec<IncludedLocalMakeFragment>,
    /// Every candidate which was not expanded faithfully.
    pub issues: Vec<LocalMakeIncludeIssue>,
}

/// Expands safe sibling fragments referenced by one mmakefile.
///
/// `mmake_relative_path` must name the declaring mmakefile below
/// `source_root`. Only include arguments rooted in both `$(SRCDIR)` and
/// `$(CURDIR)` are candidates. Other include families remain owned by the
/// existing architecture, generated-file and fetched-port collectors.
#[must_use]
pub fn inline_local_make_includes(
    content: &str,
    source_root: &Path,
    mmake_relative_path: &Path,
    limits: LocalMakeIncludeLimits,
    policy: LocalMakeFragmentPolicy,
) -> LocalMakeIncludeScan {
    let original = || LocalMakeIncludeScan {
        expanded: content.to_owned(),
        fragments: Vec::new(),
        issues: Vec::new(),
    };

    if mmake_relative_path.is_absolute() || limits.depth == 0 {
        let mut scan = original();
        scan.issues.push(issue(
            mmake_relative_path,
            1,
            LocalMakeIncludeIssueKind::InvalidContext,
            mmake_relative_path.display().to_string(),
            "the mmakefile path must be source-relative and the depth limit must be non-zero",
        ));
        return scan;
    }

    let Ok(root) = fs::canonicalize(source_root) else {
        let mut scan = original();
        scan.issues.push(issue(
            mmake_relative_path,
            1,
            LocalMakeIncludeIssueKind::InvalidContext,
            source_root.display().to_string(),
            "the source root could not be canonicalized",
        ));
        return scan;
    };
    let Some(curdir) = mmake_relative_path.parent() else {
        let mut scan = original();
        scan.issues.push(issue(
            mmake_relative_path,
            1,
            LocalMakeIncludeIssueKind::InvalidContext,
            mmake_relative_path.display().to_string(),
            "the mmakefile path has no declaring directory",
        ));
        return scan;
    };
    let declared = lexical_normalize(&root.join(mmake_relative_path));
    if !declared.starts_with(&root) {
        let mut scan = original();
        scan.issues.push(issue(
            mmake_relative_path,
            1,
            LocalMakeIncludeIssueKind::InvalidContext,
            mmake_relative_path.display().to_string(),
            "the declaring mmakefile leaves the source tree",
        ));
        return scan;
    }

    let mut state = ExpansionState {
        root,
        curdir: curdir.to_path_buf(),
        limits,
        policy,
        files_read: 0,
        bytes_read: 0,
        active: Vec::new(),
    };
    let expanded = expand_text(content, mmake_relative_path, false, &mut state);
    LocalMakeIncludeScan {
        expanded: expanded.text,
        fragments: expanded.fragments,
        issues: expanded.issues,
    }
}

struct ExpansionState {
    root: PathBuf,
    /// GNU Make's `CURDIR` remains the declaring mmakefile directory while an
    /// included fragment is parsed.
    curdir: PathBuf,
    limits: LocalMakeIncludeLimits,
    policy: LocalMakeFragmentPolicy,
    files_read: usize,
    bytes_read: usize,
    active: Vec<PathBuf>,
}

#[derive(Default)]
struct TextExpansion {
    text: String,
    fragments: Vec<IncludedLocalMakeFragment>,
    issues: Vec<LocalMakeIncludeIssue>,
    fatal: bool,
}

#[derive(Clone, Copy)]
struct IncludeDirective<'a> {
    optional: bool,
    path: &'a str,
}

struct FragmentSafety {
    assigned_variables: Vec<String>,
    has_conditionals: bool,
    plain_source_list: bool,
}

fn expand_text(
    content: &str,
    source: &Path,
    inside_fragment: bool,
    state: &mut ExpansionState,
) -> TextExpansion {
    let mut output = TextExpansion {
        text: String::with_capacity(content.len()),
        ..TextExpansion::default()
    };

    for (index, chunk) in content.split_inclusive('\n').enumerate() {
        output.text.push_str(chunk);
        let line = chunk.strip_suffix('\n').unwrap_or(chunk);
        let Some(include) = parse_include_directive(line) else {
            continue;
        };
        let line_no = index + 1;
        if !is_local_candidate(include.path) {
            if inside_fragment {
                output.issues.push(issue(
                    source,
                    line_no,
                    LocalMakeIncludeIssueKind::NestedNonLocalInclude,
                    include.path,
                    "a local fragment imports a non-local or generated Make scope",
                ));
                output.fatal = true;
            }
            continue;
        }

        match expand_one_fragment(include, source, line_no, state) {
            Ok(fragment) => {
                if !output.text.ends_with('\n') {
                    output.text.push('\n');
                }
                output.text.push_str(&fragment.text);
                if !output.text.ends_with('\n') {
                    output.text.push('\n');
                }
                output.fragments.extend(fragment.fragments);
                output.issues.extend(fragment.issues);
            }
            Err(mut issues) => {
                output.issues.append(&mut issues);
                if inside_fragment {
                    output.fatal = true;
                }
            }
        }
    }

    // `split_inclusive` yields nothing for an empty string and retains a final
    // unterminated line, so no separate trailing-text path is needed.
    output
}

fn expand_one_fragment(
    include: IncludeDirective<'_>,
    included_from: &Path,
    include_line: usize,
    state: &mut ExpansionState,
) -> Result<TextExpansion, Vec<LocalMakeIncludeIssue>> {
    let path = resolve_local_path(include.path, included_from, include_line, state)?;
    if state.active.len() >= state.limits.depth {
        return Err(vec![issue(
            included_from,
            include_line,
            LocalMakeIncludeIssueKind::DepthLimit,
            include.path,
            "the local include nesting limit was reached",
        )]);
    }
    if state.active.contains(&path) {
        return Err(vec![issue(
            included_from,
            include_line,
            LocalMakeIncludeIssueKind::Cycle,
            include.path,
            "the local include graph is cyclic",
        )]);
    }
    if state.files_read >= state.limits.files {
        return Err(vec![issue(
            included_from,
            include_line,
            LocalMakeIncludeIssueKind::FileLimit,
            include.path,
            "the local include file-count limit was reached",
        )]);
    }

    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            let detail = if include.optional {
                format!("optional local fragment is absent: {error}")
            } else {
                format!("required local fragment is absent: {error}")
            };
            return Err(vec![issue(
                included_from,
                include_line,
                LocalMakeIncludeIssueKind::Missing,
                include.path,
                detail,
            )]);
        }
    };
    state.files_read += 1;
    let Some(total_bytes) = state.bytes_read.checked_add(bytes.len()) else {
        return Err(vec![issue(
            included_from,
            include_line,
            LocalMakeIncludeIssueKind::ByteLimit,
            include.path,
            "the aggregate fragment byte count overflowed",
        )]);
    };
    if total_bytes > state.limits.bytes {
        return Err(vec![issue(
            included_from,
            include_line,
            LocalMakeIncludeIssueKind::ByteLimit,
            include.path,
            "the aggregate local include byte limit was reached",
        )]);
    }
    state.bytes_read = total_bytes;
    let Ok(body) = String::from_utf8(bytes) else {
        return Err(vec![issue(
            included_from,
            include_line,
            LocalMakeIncludeIssueKind::Read,
            include.path,
            "the local fragment is not valid UTF-8 text",
        )]);
    };
    let relative = path
        .strip_prefix(&state.root)
        .unwrap_or(&path)
        .to_path_buf();
    let safety = validate_fragment(&body, &relative)?;
    if state.policy == LocalMakeFragmentPolicy::PlainSourceLists && !safety.plain_source_list {
        return Err(vec![issue(
            included_from,
            include_line,
            LocalMakeIncludeIssueKind::DeferredScope,
            include.path,
            "the safe fragment is broader than one plain source-list assignment",
        )]);
    }

    state.active.push(path);
    let mut nested = expand_text(&body, &relative, true, state);
    state.active.pop();
    if nested.fatal {
        return Err(nested.issues);
    }

    let mut fragments = Vec::with_capacity(nested.fragments.len() + 1);
    fragments.push(IncludedLocalMakeFragment {
        path: relative,
        included_from: included_from.to_path_buf(),
        include_line,
        assigned_variables: safety.assigned_variables,
        has_conditionals: safety.has_conditionals,
        plain_source_list: safety.plain_source_list,
    });
    fragments.append(&mut nested.fragments);
    nested.fragments = fragments;
    Ok(nested)
}

fn resolve_local_path(
    raw: &str,
    source: &Path,
    line: usize,
    state: &ExpansionState,
) -> Result<PathBuf, Vec<LocalMakeIncludeIssue>> {
    let raw = raw.trim();
    if raw.is_empty() || raw.split_whitespace().count() != 1 {
        return Err(vec![issue(
            source,
            line,
            LocalMakeIncludeIssueKind::UnresolvedPath,
            raw,
            "a local include must name exactly one fragment",
        )]);
    }
    let root_text = state.root.to_string_lossy();
    let curdir_text = state.curdir.to_string_lossy();
    let expanded = raw
        .replace("$(SRCDIR)", &root_text)
        .replace("${SRCDIR}", &root_text)
        .replace("$(CURDIR)", &curdir_text)
        .replace("${CURDIR}", &curdir_text);
    if expanded.contains('$') || expanded.contains('*') || expanded.contains('?') {
        return Err(vec![issue(
            source,
            line,
            LocalMakeIncludeIssueKind::UnresolvedPath,
            raw,
            "the local include path is dynamic",
        )]);
    }

    let candidate = PathBuf::from(expanded);
    let candidate = if candidate.is_absolute() {
        candidate
    } else {
        state.root.join(candidate)
    };
    let lexical = lexical_normalize(&candidate);
    if !lexical.starts_with(&state.root) {
        return Err(vec![issue(
            source,
            line,
            LocalMakeIncludeIssueKind::OutsideSourceTree,
            raw,
            "the local include path leaves the source tree",
        )]);
    }
    let canonical = match fs::canonicalize(&lexical) {
        Ok(path) => path,
        Err(error) => {
            return Err(vec![issue(
                source,
                line,
                LocalMakeIncludeIssueKind::Missing,
                raw,
                format!("the local fragment cannot be resolved: {error}"),
            )]);
        }
    };
    if !canonical.starts_with(&state.root) {
        return Err(vec![issue(
            source,
            line,
            LocalMakeIncludeIssueKind::OutsideSourceTree,
            raw,
            "the local include resolves outside the source tree",
        )]);
    }
    Ok(canonical)
}

fn validate_fragment(
    content: &str,
    source: &Path,
) -> Result<FragmentSafety, Vec<LocalMakeIncludeIssue>> {
    let mut assigned = BTreeSet::new();
    let mut conditional_depth = 0usize;
    let mut has_conditionals = false;
    let mut has_includes = false;
    let mut all_assignments_are_plain_lists = true;
    let mut assignment_count = 0usize;
    let mut issues = Vec::new();

    for logical in logical_lines(content) {
        let trimmed = logical.text.trim();
        if trimmed.is_empty() {
            continue;
        }
        if logical.recipe {
            issues.push(issue(
                source,
                logical.line,
                LocalMakeIncludeIssueKind::UnsafeSyntax,
                trimmed,
                "Make recipes are not evaluated during transpilation",
            ));
            continue;
        }
        if trimmed.starts_with("#MM") {
            issues.push(issue(
                source,
                logical.line,
                LocalMakeIncludeIssueKind::UnsafeSyntax,
                trimmed,
                "MetaMake graph declarations are not permitted in variable fragments",
            ));
            continue;
        }
        if trimmed.starts_with('#') {
            continue;
        }
        let uncommented = strip_make_comment(trimmed).trim();
        if uncommented.is_empty() {
            continue;
        }
        if let Some(function) = unsafe_make_function(uncommented) {
            issues.push(issue(
                source,
                logical.line,
                LocalMakeIncludeIssueKind::UnsafeSyntax,
                uncommented,
                format!("the side-effecting Make function `{function}` is not permitted"),
            ));
            continue;
        }
        if parse_include_directive(uncommented).is_some() {
            has_includes = true;
            continue;
        }
        if starts_directive(uncommented, "ifeq")
            || starts_directive(uncommented, "ifneq")
            || starts_directive(uncommented, "ifdef")
            || starts_directive(uncommented, "ifndef")
        {
            conditional_depth += 1;
            has_conditionals = true;
            continue;
        }
        if uncommented == "else"
            || starts_directive(uncommented, "else ifeq")
            || starts_directive(uncommented, "else ifneq")
            || starts_directive(uncommented, "else ifdef")
            || starts_directive(uncommented, "else ifndef")
        {
            if conditional_depth == 0 {
                issues.push(issue(
                    source,
                    logical.line,
                    LocalMakeIncludeIssueKind::UnsafeSyntax,
                    uncommented,
                    "an else directive has no matching local conditional",
                ));
            }
            continue;
        }
        if uncommented == "endif" {
            if conditional_depth == 0 {
                issues.push(issue(
                    source,
                    logical.line,
                    LocalMakeIncludeIssueKind::UnsafeSyntax,
                    uncommented,
                    "an endif directive has no matching local conditional",
                ));
            } else {
                conditional_depth -= 1;
            }
            continue;
        }
        if let Some((name, value)) = assignment(uncommented) {
            assigned.insert(name.to_owned());
            assignment_count += 1;
            all_assignments_are_plain_lists &= is_plain_source_list(value);
            continue;
        }

        issues.push(issue(
            source,
            logical.line,
            LocalMakeIncludeIssueKind::UnsafeSyntax,
            uncommented,
            "only variable assignments, conditionals and local includes are permitted",
        ));
    }

    if conditional_depth != 0 {
        issues.push(issue(
            source,
            content.lines().count().max(1),
            LocalMakeIncludeIssueKind::UnsafeSyntax,
            "conditional",
            "a local fragment leaves a Make conditional open",
        ));
    }
    if issues.is_empty() {
        Ok(FragmentSafety {
            plain_source_list: assigned.len() == 1
                && assignment_count > 0
                && !has_conditionals
                && !has_includes
                && all_assignments_are_plain_lists,
            assigned_variables: assigned.into_iter().collect(),
            has_conditionals,
        })
    } else {
        Err(issues)
    }
}

struct LogicalLine {
    line: usize,
    text: String,
    recipe: bool,
}

fn logical_lines(content: &str) -> Vec<LogicalLine> {
    let mut output = Vec::new();
    let mut pending = String::new();
    let mut start_line = 1usize;
    let mut recipe = false;

    for (index, physical) in content.lines().enumerate() {
        let line = index + 1;
        if pending.is_empty() {
            start_line = line;
            recipe = physical.starts_with('\t');
        }
        let trimmed = physical.trim_end();
        let continued = trimmed.ends_with('\\');
        let payload = trimmed.strip_suffix('\\').unwrap_or(trimmed);
        if !pending.is_empty() {
            pending.push(' ');
        }
        pending.push_str(payload);
        if continued {
            continue;
        }
        output.push(LogicalLine {
            line: start_line,
            text: std::mem::take(&mut pending),
            recipe,
        });
    }
    if !pending.is_empty() {
        output.push(LogicalLine {
            line: start_line,
            text: pending,
            recipe,
        });
    }
    output
}

fn assignment(line: &str) -> Option<(&str, &str)> {
    let (at, width) = ["::=", ":=", "+=", "?=", "="]
        .into_iter()
        .filter_map(|operator| line.find(operator).map(|at| (at, operator.len())))
        .min_by_key(|(at, _)| *at)?;
    let name = line[..at].trim();
    if width == 1 && name.ends_with('!') {
        return None;
    }
    let valid = !name.is_empty()
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'));
    valid.then(|| (name, line[at + width..].trim()))
}

fn is_plain_source_list(value: &str) -> bool {
    let mut words = value.split_whitespace().peekable();
    words.peek().is_some()
        && words.all(|word| {
            let word = word.trim_matches('"');
            !word.is_empty()
                && !word.starts_with('-')
                && word.chars().all(|character| {
                    character.is_ascii_alphanumeric()
                        || matches!(character, '_' | '-' | '.' | '/' | '+')
                })
        })
}

fn parse_include_directive(line: &str) -> Option<IncludeDirective<'_>> {
    let line = strip_make_comment(line).trim();
    let (optional, tail) = if let Some(tail) = directive_tail(line, "-include") {
        (true, tail)
    } else if let Some(tail) = directive_tail(line, "include") {
        (false, tail)
    } else {
        return None;
    };
    Some(IncludeDirective {
        optional,
        path: tail.trim(),
    })
}

fn directive_tail<'a>(line: &'a str, directive: &str) -> Option<&'a str> {
    let tail = line.strip_prefix(directive)?;
    (tail.is_empty() || tail.chars().next().is_some_and(char::is_whitespace)).then(|| tail.trim())
}

fn starts_directive(line: &str, directive: &str) -> bool {
    directive_tail(line, directive).is_some()
}

fn is_local_candidate(path: &str) -> bool {
    let has_source = path.contains("$(SRCDIR)") || path.contains("${SRCDIR}");
    let has_curdir = path.contains("$(CURDIR)") || path.contains("${CURDIR}");
    // Local make.opts files already have a target-tagged collector which also
    // propagates their flags and include directories. Treating them as source
    // inventory would duplicate that ownership and manufacture skip noise.
    let owned_make_opts = path.trim().ends_with("/make.opts");
    has_source && has_curdir && !owned_make_opts
}

fn unsafe_make_function(line: &str) -> Option<&'static str> {
    ["shell", "eval", "file", "guile", "load"]
        .into_iter()
        .find(|name| contains_make_function(line, name))
}

fn contains_make_function(line: &str, name: &str) -> bool {
    ["$(", "${"].into_iter().any(|opening| {
        let mut rest = line;
        while let Some(start) = rest.find(opening) {
            let body = &rest[start + opening.len()..];
            if let Some(tail) = body.strip_prefix(name) {
                if tail.is_empty()
                    || tail
                        .chars()
                        .next()
                        .is_some_and(|next| next.is_whitespace() || matches!(next, ',' | ')' | '}'))
                {
                    return true;
                }
            }
            rest = &body[body.len().min(1)..];
        }
        false
    })
}

fn strip_make_comment(line: &str) -> &str {
    for (at, character) in line.char_indices() {
        if character != '#' {
            continue;
        }
        let escaped = line[..at]
            .bytes()
            .rev()
            .take_while(|byte| *byte == b'\\')
            .count()
            % 2
            == 1;
        if !escaped {
            return &line[..at];
        }
    }
    line
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut output = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                output.pop();
            }
            other => output.push(other.as_os_str()),
        }
    }
    output
}

fn issue(
    source: &Path,
    line: usize,
    kind: LocalMakeIncludeIssueKind,
    subject: impl Into<String>,
    detail: impl Into<String>,
) -> LocalMakeIncludeIssue {
    LocalMakeIncludeIssue {
        source: source.to_path_buf(),
        line,
        kind,
        subject: subject.into(),
        detail: detail.into(),
    }
}
