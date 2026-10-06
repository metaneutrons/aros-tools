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

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use aros_common::native_make_template::ResolvedGeneratedMakeTemplate;

const MAX_RESOLVED_TEMPLATE_BYTES: usize = 256 * 1024;
const MAX_TEMPLATE_BINDINGS: usize = 16;
const MAX_TEMPLATE_SUBSTITUTIONS: usize = 32;

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
    /// Accept one complete variable scope accompanied by one strictly literal
    /// generated `#define` header rule.
    ///
    /// The only admitted recipe is an unconditional literal overwrite followed
    /// by conditional literal appends to the same basename. Callers must still
    /// prove declaration ownership, select every conditional for a concrete
    /// target profile and materialise the header as a real build output.
    LiteralDefineHeader,
    /// Explicit source-native configuration. Only a caller-supplied binding
    /// can replace a global include; both source endpoints remain regular
    /// in-root files. This mode never reads generated configuration.
    NativeConfiguration,
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
    /// Whether the fragment passed the complete literal-define-header grammar.
    pub literal_define_header: bool,
    /// Generated output path for an explicitly bound configure template.
    /// `path` remains the sealed source template path in that case.
    pub generated_output: Option<PathBuf>,
    /// Exact source-owned substitutions used by the configure template.
    pub template_substitutions: Option<BTreeMap<String, String>>,
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
/// `source_root`. Include arguments rooted in both `$(SRCDIR)` and `$(CURDIR)`
/// are candidates, as are literal source-root `.cfg` fragments outside the
/// global `config/` directory. Other include families remain owned by the
/// existing architecture, generated-file and fetched-port collectors.
#[must_use]
pub fn inline_local_make_includes(
    content: &str,
    source_root: &Path,
    mmake_relative_path: &Path,
    limits: LocalMakeIncludeLimits,
    policy: LocalMakeFragmentPolicy,
) -> LocalMakeIncludeScan {
    inline_make_includes(
        content,
        source_root,
        mmake_relative_path,
        limits,
        policy,
        &BTreeMap::new(),
        &BTreeMap::new(),
    )
}

/// Expands explicitly bound native configuration at its original include
/// position.
///
/// An unbound global include stays visible, so a consuming
/// capability must refuse it instead of treating missing values as empty.
#[must_use]
pub fn inline_native_make_configuration(
    content: &str,
    source_root: &Path,
    mmake_relative_path: &Path,
    limits: LocalMakeIncludeLimits,
    bindings: &BTreeMap<String, String>,
) -> LocalMakeIncludeScan {
    inline_native_make_configuration_with_templates(
        content,
        source_root,
        mmake_relative_path,
        limits,
        bindings,
        &BTreeMap::new(),
    )
}

/// Expands explicitly bound native configuration and sealed configure-owned
/// Make templates at their original include positions.
///
/// A template is selected only by a source-relative generated-path key that
/// matches an explicit `$(TOP)/$(CURDIR)/<path>` include from this declaring
/// directory. `TOP` is deliberately not expanded or equated with `SRCDIR`,
/// and no generated output is opened. Missing template bindings remain as
/// unresolved includes in the text.
#[must_use]
pub fn inline_native_make_configuration_with_templates(
    content: &str,
    source_root: &Path,
    mmake_relative_path: &Path,
    limits: LocalMakeIncludeLimits,
    bindings: &BTreeMap<String, String>,
    templates: &BTreeMap<String, ResolvedGeneratedMakeTemplate>,
) -> LocalMakeIncludeScan {
    if let Err((kind, subject, detail)) =
        validate_template_bindings(bindings, templates, limits, mmake_relative_path)
    {
        return LocalMakeIncludeScan {
            expanded: content.to_owned(),
            fragments: Vec::new(),
            issues: vec![issue(mmake_relative_path, 1, kind, subject, detail)],
        };
    }
    inline_make_includes(
        content,
        source_root,
        mmake_relative_path,
        limits,
        LocalMakeFragmentPolicy::NativeConfiguration,
        bindings,
        templates,
    )
}

fn inline_make_includes(
    content: &str,
    source_root: &Path,
    mmake_relative_path: &Path,
    limits: LocalMakeIncludeLimits,
    policy: LocalMakeFragmentPolicy,
    bindings: &BTreeMap<String, String>,
    templates: &BTreeMap<String, ResolvedGeneratedMakeTemplate>,
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
        bindings: bindings.clone(),
        templates: templates.clone(),
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
    bindings: BTreeMap<String, String>,
    templates: BTreeMap<String, ResolvedGeneratedMakeTemplate>,
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
    literal_define_header: bool,
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

        if state.policy == LocalMakeFragmentPolicy::NativeConfiguration {
            match generated_template_include_key(include.path, state) {
                Ok(Some(generated_path)) => {
                    if !state.templates.contains_key(&generated_path) {
                        output.issues.push(issue(
                            source,
                            line_no,
                            LocalMakeIncludeIssueKind::UnresolvedPath,
                            include.path,
                            "generated Make include has no explicit source-owned template binding",
                        ));
                        if inside_fragment {
                            output.fatal = true;
                        }
                        continue;
                    }
                    match expand_generated_template(
                        &generated_path,
                        include.path,
                        source,
                        line_no,
                        state,
                    ) {
                        Ok(fragment) => {
                            output.text.truncate(output.text.len() - chunk.len());
                            output
                                .text
                                .push_str("# Verified source configuration template\n");
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
                    continue;
                }
                Ok(None) => {}
                Err(detail) => {
                    output.issues.push(issue(
                        source,
                        line_no,
                        LocalMakeIncludeIssueKind::UnresolvedPath,
                        include.path,
                        detail,
                    ));
                    if inside_fragment {
                        output.fatal = true;
                    }
                    continue;
                }
            }
        }

        let bound = bound_include_key(include.path, state)
            .is_some_and(|key| state.bindings.contains_key(&key));
        if !bound && !is_local_candidate(include.path) {
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
                if state.policy == LocalMakeFragmentPolicy::NativeConfiguration {
                    // Only a successfully expanded directive is removed.
                    // Rejected includes remain explicit uncertainty.
                    output.text.truncate(output.text.len() - chunk.len());
                    output
                        .text
                        .push_str("# Verified source configuration include\n");
                }
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
    let mut path = resolve_local_path(include.path, included_from, include_line, state)?;
    if let Some(key) = bound_include_key(include.path, state) {
        if let Some(replacement) = state.bindings.get(&key) {
            assert_bound_regular(
                &path,
                &state.root,
                included_from,
                include_line,
                include.path,
            )?;
            if !safe_bound_path(replacement) {
                return Err(vec![issue(
                    included_from,
                    include_line,
                    LocalMakeIncludeIssueKind::UnsafeSyntax,
                    include.path,
                    "native include replacement must be a normalized source-relative .mk file",
                )]);
            }
            path = resolve_local_path(replacement, included_from, include_line, state)?;
            assert_bound_regular(
                &path,
                &state.root,
                included_from,
                include_line,
                include.path,
            )?;
        }
    }
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
    let safety = if state.policy == LocalMakeFragmentPolicy::LiteralDefineHeader {
        validate_literal_define_header_fragment(&body, &relative)?
    } else {
        validate_fragment(
            &body,
            &relative,
            state.policy == LocalMakeFragmentPolicy::NativeConfiguration,
        )?
    };
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
        literal_define_header: safety.literal_define_header,
        generated_output: None,
        template_substitutions: None,
    });
    fragments.append(&mut nested.fragments);
    nested.fragments = fragments;
    Ok(nested)
}

fn expand_generated_template(
    generated_path: &str,
    include_path: &str,
    included_from: &Path,
    include_line: usize,
    state: &mut ExpansionState,
) -> Result<TextExpansion, Vec<LocalMakeIncludeIssue>> {
    let Some(template) = state.templates.get(generated_path).cloned() else {
        return Err(vec![issue(
            included_from,
            include_line,
            LocalMakeIncludeIssueKind::UnresolvedPath,
            include_path,
            "the generated Make template binding disappeared during expansion",
        )]);
    };

    if state.active.len() >= state.limits.depth {
        return Err(vec![issue(
            included_from,
            include_line,
            LocalMakeIncludeIssueKind::DepthLimit,
            include_path,
            "the local include nesting limit was reached",
        )]);
    }
    if state.files_read >= state.limits.files {
        return Err(vec![issue(
            included_from,
            include_line,
            LocalMakeIncludeIssueKind::FileLimit,
            include_path,
            "the local include file-count limit was reached",
        )]);
    }
    let Some(total_bytes) = state.bytes_read.checked_add(template.expanded_text.len()) else {
        return Err(vec![issue(
            included_from,
            include_line,
            LocalMakeIncludeIssueKind::ByteLimit,
            include_path,
            "the aggregate fragment byte count overflowed",
        )]);
    };
    if total_bytes > state.limits.bytes {
        return Err(vec![issue(
            included_from,
            include_line,
            LocalMakeIncludeIssueKind::ByteLimit,
            include_path,
            "the aggregate local include byte limit was reached",
        )]);
    }

    let template_path = PathBuf::from(&template.template_relative);
    let safety = validate_resolved_template(&template, &template_path)?;
    state.files_read += 1;
    state.bytes_read = total_bytes;

    Ok(TextExpansion {
        text: template.expanded_text,
        fragments: vec![IncludedLocalMakeFragment {
            path: template_path,
            included_from: included_from.to_path_buf(),
            include_line,
            assigned_variables: safety.assigned_variables,
            has_conditionals: false,
            plain_source_list: false,
            literal_define_header: false,
            generated_output: Some(PathBuf::from(generated_path)),
            template_substitutions: Some(template.substitutions),
        }],
        issues: Vec::new(),
        fatal: false,
    })
}

fn validate_resolved_template(
    template: &ResolvedGeneratedMakeTemplate,
    source: &Path,
) -> Result<FragmentSafety, Vec<LocalMakeIncludeIssue>> {
    let content = &template.expanded_text;
    if content.len() > MAX_RESOLVED_TEMPLATE_BYTES
        || !content.is_ascii()
        || content.contains('\t')
        || content
            .bytes()
            .any(|byte| byte.is_ascii_control() && !matches!(byte, b'\n' | b'\r'))
        || content
            .bytes()
            .enumerate()
            .any(|(index, byte)| byte == b'\r' && content.as_bytes().get(index + 1) != Some(&b'\n'))
        || content.contains('\\')
    {
        return Err(vec![issue(
            source,
            1,
            LocalMakeIncludeIssueKind::UnsafeSyntax,
            source.display().to_string(),
            "resolved configure template is not bounded plain ASCII Make text",
        )]);
    }

    let safety = validate_fragment(content, source, true)?;
    if safety.has_conditionals || safety.assigned_variables.is_empty() {
        return Err(vec![issue(
            source,
            1,
            LocalMakeIncludeIssueKind::UnsafeSyntax,
            source.display().to_string(),
            "resolved configure template must be a nonempty unconditional assignment scope",
        )]);
    }

    let mut marker_seen = false;
    let mut assignment_seen = false;
    let mut assigned = BTreeSet::new();
    let mut assignment_count = 0_usize;
    for logical in logical_lines(content) {
        let line = logical.text.as_str();
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if trimmed == "%common" {
            if line != "%common" || marker_seen || assignment_seen || logical.recipe {
                return Err(vec![issue(
                    source,
                    logical.line,
                    LocalMakeIncludeIssueKind::UnsafeSyntax,
                    trimmed,
                    "only one exact leading %common section marker is permitted",
                )]);
            }
            marker_seen = true;
            continue;
        }

        let uncommented = strip_make_comment(trimmed).trim();
        if uncommented.is_empty() {
            continue;
        }
        if parse_include_directive(uncommented).is_some() {
            return Err(vec![issue(
                source,
                logical.line,
                LocalMakeIncludeIssueKind::UnsafeSyntax,
                uncommented,
                "resolved configure templates may not include additional files",
            )]);
        }
        let Some((lhs, rhs)) = uncommented.split_once('=') else {
            return Err(vec![issue(
                source,
                logical.line,
                LocalMakeIncludeIssueKind::UnsafeSyntax,
                uncommented,
                "resolved configure templates may contain only plain assignments",
            )]);
        };
        let name = lhs.trim();
        if lhs.ends_with('+')
            || lhs.ends_with(':')
            || lhs.ends_with('?')
            || lhs.ends_with('!')
            || rhs.contains('=')
            || name.is_empty()
            || name.len() > 64
            || !name
                .as_bytes()
                .first()
                .is_some_and(|byte| byte.is_ascii_uppercase() || *byte == b'_')
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(vec![issue(
                source,
                logical.line,
                LocalMakeIncludeIssueKind::UnsafeSyntax,
                uncommented,
                "resolved configure template has an unsupported assignment form",
            )]);
        }
        let value = rhs.trim();
        // A substituted preprocessor line (e.g. "#define __AROSEXEC_SMP__")
        // leaves only its opening quote once Make cuts the comment.
        let cut_comment = uncommented.len() < trimmed.len();
        let admitted = safe_template_assignment_value(value)
            || cut_comment
                && value.strip_prefix('"').is_some_and(|rest| {
                    rest.bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"_.+-".contains(&byte))
                });
        if !admitted || !assigned.insert(name.to_owned()) {
            return Err(vec![issue(
                source,
                logical.line,
                LocalMakeIncludeIssueKind::UnsafeSyntax,
                uncommented,
                "resolved configure template has an unsafe or duplicate assignment",
            )]);
        }
        assignment_seen = true;
        assignment_count += 1;
        if assignment_count > 64 {
            return Err(vec![issue(
                source,
                logical.line,
                LocalMakeIncludeIssueKind::UnsafeSyntax,
                uncommented,
                "resolved configure template exceeds the assignment limit",
            )]);
        }
    }
    Ok(safety)
}

fn safe_template_assignment_value(value: &str) -> bool {
    if let Some(inner) = value
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    {
        return inner
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.+-".contains(&byte));
    }
    value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"_.+-".contains(&byte))
}

fn validate_template_bindings(
    bindings: &BTreeMap<String, String>,
    templates: &BTreeMap<String, ResolvedGeneratedMakeTemplate>,
    _limits: LocalMakeIncludeLimits,
    mmake_relative_path: &Path,
) -> Result<(), (LocalMakeIncludeIssueKind, String, String)> {
    if templates.is_empty() {
        return Ok(());
    }
    if templates.len() > MAX_TEMPLATE_BINDINGS {
        return Err((
            LocalMakeIncludeIssueKind::FileLimit,
            "generated Make template bindings".into(),
            format!("template map exceeds {MAX_TEMPLATE_BINDINGS} entries"),
        ));
    }
    if bindings.len() > MAX_TEMPLATE_BINDINGS {
        return Err((
            LocalMakeIncludeIssueKind::FileLimit,
            "native include bindings".into(),
            format!("normal include map exceeds {MAX_TEMPLATE_BINDINGS} entries"),
        ));
    }
    let mmake_path = mmake_relative_path.to_str().ok_or_else(|| {
        (
            LocalMakeIncludeIssueKind::InvalidContext,
            mmake_relative_path.display().to_string(),
            "the declaring mmakefile path is not valid UTF-8".into(),
        )
    })?;
    if !safe_source_relative_key(mmake_path) {
        return Err((
            LocalMakeIncludeIssueKind::InvalidContext,
            mmake_path.to_owned(),
            "template expansion requires a canonical source-relative mmakefile path".into(),
        ));
    }

    for (key, replacement) in bindings {
        if !safe_source_relative_key(key)
            || !safe_source_relative_key(replacement)
            || Path::new(replacement)
                .extension()
                .is_none_or(|extension| extension != "mk")
        {
            return Err((
                LocalMakeIncludeIssueKind::UnsafeSyntax,
                key.clone(),
                "native include binding keys/replacements must be canonical source-relative paths and replacements must be .mk files".into(),
            ));
        }
    }

    for (index, generated_path) in templates.keys().enumerate() {
        let Some(template) = templates.get(generated_path) else {
            return Err((
                LocalMakeIncludeIssueKind::UnsafeSyntax,
                generated_path.clone(),
                "generated template map entry is unexpectedly absent".into(),
            ));
        };
        if !safe_source_relative_key(generated_path)
            || !safe_source_relative_key(&template.template_relative)
            || Path::new(&template.template_relative)
                .extension()
                .is_none_or(|extension| extension != "in")
        {
            return Err((
                LocalMakeIncludeIssueKind::UnsafeSyntax,
                generated_path.clone(),
                "generated output and template must be normalized source-relative paths, with a .in template".into(),
            ));
        }
        if paths_overlap(generated_path, &template.template_relative) {
            return Err((
                LocalMakeIncludeIssueKind::UnsafeSyntax,
                generated_path.clone(),
                "generated output overlaps its source template path".into(),
            ));
        }
        if template.expanded_text.len() > MAX_RESOLVED_TEMPLATE_BYTES
            || template.expanded_text.contains('@')
        {
            return Err((
                LocalMakeIncludeIssueKind::ByteLimit,
                generated_path.clone(),
                "resolved template text exceeds its byte limit or contains a residual token marker"
                    .into(),
            ));
        }
        if template.substitutions.len() > MAX_TEMPLATE_SUBSTITUTIONS
            || template.substitutions.iter().any(|(token, value)| {
                !safe_substitution_key(token)
                    || !aros_common::native_make_template::admitted_substitution_value(value)
            })
        {
            return Err((
                LocalMakeIncludeIssueKind::UnsafeSyntax,
                generated_path.clone(),
                "resolved template substitutions are unsafe or exceed their entry limit".into(),
            ));
        }

        for other in templates.keys().skip(index + 1) {
            if paths_overlap(generated_path, other) {
                return Err((
                    LocalMakeIncludeIssueKind::UnsafeSyntax,
                    generated_path.clone(),
                    format!("generated template path overlaps {other:?}"),
                ));
            }
        }
        for normal_key in bindings.keys() {
            if paths_overlap(generated_path, normal_key) {
                return Err((
                    LocalMakeIncludeIssueKind::UnsafeSyntax,
                    generated_path.clone(),
                    format!(
                        "generated template key overlaps normal native include binding {normal_key:?}"
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn generated_template_include_key(
    raw: &str,
    state: &ExpansionState,
) -> std::result::Result<Option<String>, &'static str> {
    const PREFIXES: [&str; 4] = [
        "$(TOP)/$(CURDIR)/",
        "$(TOP)/${CURDIR}/",
        "${TOP}/$(CURDIR)/",
        "${TOP}/${CURDIR}/",
    ];
    let Some(prefix) = PREFIXES.iter().find(|prefix| raw.starts_with(**prefix)) else {
        return Ok(None);
    };
    let suffix = &raw[prefix.len()..];
    if !safe_source_relative_key(suffix) {
        return Err("generated include suffix is not a canonical relative path");
    }
    let generated_path = state.curdir.join(suffix);
    let Some(generated_path) = generated_path.to_str() else {
        return Err("generated include path is not valid UTF-8");
    };
    if !safe_source_relative_key(generated_path) {
        return Err("generated include does not normalize to a source-relative output");
    }
    Ok(Some(generated_path.to_owned()))
}

fn safe_source_relative_key(value: &str) -> bool {
    if value.is_empty()
        || value.len() > 4096
        || value.starts_with('/')
        || value.contains(['\\', ':', '$', '*', '?'])
        || value.chars().any(char::is_whitespace)
    {
        return false;
    }
    let mut normalized = PathBuf::new();
    for segment in value.split('/') {
        if segment.is_empty()
            || segment == "."
            || segment == ".."
            || !segment
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._+-".contains(&byte))
        {
            return false;
        }
        normalized.push(segment);
    }
    normalized.to_str() == Some(value)
}

fn safe_substitution_key(token: &str) -> bool {
    let Some(name) = token
        .strip_prefix('@')
        .and_then(|token| token.strip_suffix('@'))
    else {
        return false;
    };
    (1..=64).contains(&name.len())
        && name
            .as_bytes()
            .first()
            .is_some_and(|byte| byte.is_ascii_uppercase() || *byte == b'_')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

fn paths_overlap(left: &str, right: &str) -> bool {
    let left = left.to_ascii_lowercase();
    let right = right.to_ascii_lowercase();
    left == right
        || left
            .strip_prefix(&right)
            .is_some_and(|suffix| suffix.starts_with('/'))
        || right
            .strip_prefix(&left)
            .is_some_and(|suffix| suffix.starts_with('/'))
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
    if state.policy == LocalMakeFragmentPolicy::NativeConfiguration {
        if canonical != lexical {
            return Err(vec![issue(
                source,
                line,
                LocalMakeIncludeIssueKind::OutsideSourceTree,
                raw,
                "native configuration may not traverse symlinks",
            )]);
        }
        assert_bound_regular(&canonical, &state.root, source, line, raw)?;
    }
    Ok(canonical)
}

fn safe_bound_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains(['$', '\\'])
        && !path.contains(char::is_whitespace)
        && Path::new(path)
            .extension()
            .is_some_and(|extension| extension == "mk")
        && Path::new(path)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn bound_include_key(raw: &str, state: &ExpansionState) -> Option<String> {
    let relative = raw
        .trim()
        .strip_prefix("$(SRCDIR)/")
        .or_else(|| raw.trim().strip_prefix("${SRCDIR}/"))
        .unwrap_or_else(|| raw.trim());
    let expanded = relative
        .replace("$(CURDIR)", &state.curdir.to_string_lossy())
        .replace("${CURDIR}", &state.curdir.to_string_lossy());
    if expanded.is_empty()
        || expanded.contains(['$', '\\'])
        || expanded.contains(char::is_whitespace)
        || !Path::new(&expanded)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return None;
    }
    Some(expanded)
}

fn assert_bound_regular(
    path: &Path,
    root: &Path,
    source: &Path,
    line: usize,
    subject: &str,
) -> Result<(), Vec<LocalMakeIncludeIssue>> {
    let mut cursor = path.to_path_buf();
    while cursor != root {
        let valid = fs::symlink_metadata(&cursor).is_ok_and(|metadata| {
            !metadata.file_type().is_symlink()
                && if cursor == path {
                    metadata.is_file()
                } else {
                    metadata.is_dir()
                }
        });
        if !cursor.starts_with(root) || !valid || !cursor.pop() {
            return Err(vec![issue(source, line, LocalMakeIncludeIssueKind::OutsideSourceTree,
                subject, "native configuration endpoints must be regular source files without symlink ancestors")]);
        }
    }
    Ok(())
}

fn validate_fragment(
    content: &str,
    source: &Path,
    native_configuration: bool,
) -> Result<FragmentSafety, Vec<LocalMakeIncludeIssue>> {
    let mut assigned = BTreeSet::new();
    let mut conditional_depth = 0usize;
    let mut has_conditionals = false;
    let mut has_includes = false;
    let mut all_assignments_are_plain_lists = true;
    let mut assignment_count = 0usize;
    let mut common_section_seen = false;
    let mut active_content_seen = false;
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
        if native_configuration && trimmed == "%common" {
            if common_section_seen || active_content_seen {
                issues.push(issue(
                    source,
                    logical.line,
                    LocalMakeIncludeIssueKind::UnsafeSyntax,
                    trimmed,
                    "only one leading %common section marker is permitted",
                ));
            } else {
                common_section_seen = true;
                active_content_seen = true;
            }
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
        active_content_seen = true;
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
        if native_configuration {
            if let Ok(Some(name)) = crate::make_vars::undefine_directive(uncommented) {
                assigned.insert(name.to_owned());
                all_assignments_are_plain_lists = false;
                continue;
            }
            // A configuration guard. The variable scan stops proving
            // anything after an $(error) that may run, so admitting the
            // line cannot let an invalid configuration through.
            if crate::make_vars::is_make_error_directive(uncommented) {
                all_assignments_are_plain_lists = false;
                continue;
            }
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
            literal_define_header: false,
        })
    } else {
        Err(issues)
    }
}

/// Validates the one recipe-bearing local fragment shape which can be
/// represented without executing Make or a shell.
///
/// The complete fragment is checked atomically. Assignments and balanced Make
/// conditionals may compute the source inventory before the output rule. The
/// rule itself has no prerequisites and its recipe is exactly one literal
/// overwrite followed by literal appends to the same header basename. No
/// assignment or second rule may follow the output rule.
fn validate_literal_define_header_fragment(
    content: &str,
    source: &Path,
) -> Result<FragmentSafety, Vec<LocalMakeIncludeIssue>> {
    let mut assigned = BTreeSet::new();
    let mut conditional_depth = 0usize;
    let mut has_conditionals = false;
    let mut rule_seen = false;
    let mut recipe_count = 0usize;
    let mut recipe_destination: Option<String> = None;
    let mut issues = Vec::new();

    for logical in logical_lines(content) {
        let trimmed = logical.text.trim();
        if trimmed.is_empty() {
            continue;
        }
        if logical.recipe {
            let Some((definition, destination, append)) = literal_define_recipe(trimmed) else {
                issues.push(issue(
                    source,
                    logical.line,
                    LocalMakeIncludeIssueKind::UnsafeSyntax,
                    trimmed,
                    "only literal `echo \"#define IDENT VALUE\" >header` recipes are permitted",
                ));
                continue;
            };
            if !rule_seen {
                issues.push(issue(
                    source,
                    logical.line,
                    LocalMakeIncludeIssueKind::UnsafeSyntax,
                    trimmed,
                    "a define recipe must follow its single header output rule",
                ));
                continue;
            }
            if recipe_count == 0 {
                if conditional_depth != 0 || append {
                    issues.push(issue(
                        source,
                        logical.line,
                        LocalMakeIncludeIssueKind::UnsafeSyntax,
                        trimmed,
                        "the first define must unconditionally overwrite the output",
                    ));
                }
            } else if !append {
                issues.push(issue(
                    source,
                    logical.line,
                    LocalMakeIncludeIssueKind::UnsafeSyntax,
                    trimmed,
                    "every define after the first must append to the output",
                ));
            }
            if let Some(previous) = &recipe_destination {
                if previous != destination {
                    issues.push(issue(
                        source,
                        logical.line,
                        LocalMakeIncludeIssueKind::UnsafeSyntax,
                        trimmed,
                        "all literal defines must redirect to the same header basename",
                    ));
                }
            } else {
                recipe_destination = Some(destination.to_owned());
            }
            debug_assert!(!definition.is_empty());
            recipe_count += 1;
            continue;
        }
        if trimmed.starts_with("#MM") {
            issues.push(issue(
                source,
                logical.line,
                LocalMakeIncludeIssueKind::UnsafeSyntax,
                trimmed,
                "MetaMake graph declarations are not permitted in local variable fragments",
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
            issues.push(issue(
                source,
                logical.line,
                LocalMakeIncludeIssueKind::UnsafeSyntax,
                uncommented,
                "a literal define-header fragment may not import another Make scope",
            ));
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
        if let Some((name, _)) = assignment(uncommented) {
            if rule_seen {
                issues.push(issue(
                    source,
                    logical.line,
                    LocalMakeIncludeIssueKind::UnsafeSyntax,
                    uncommented,
                    "assignments after a generated-header rule are not permitted",
                ));
            } else {
                assigned.insert(name.to_owned());
            }
            continue;
        }
        if let Some(target) = literal_header_rule_target(uncommented) {
            if rule_seen || conditional_depth != 0 {
                issues.push(issue(
                    source,
                    logical.line,
                    LocalMakeIncludeIssueKind::UnsafeSyntax,
                    uncommented,
                    "the fragment must contain one unconditional header output rule",
                ));
            } else {
                debug_assert!(!target.is_empty());
                rule_seen = true;
            }
            continue;
        }

        issues.push(issue(
            source,
            logical.line,
            LocalMakeIncludeIssueKind::UnsafeSyntax,
            uncommented,
            "only assignments, conditionals and one literal define-header rule are permitted",
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
    if !rule_seen || recipe_count == 0 || assigned.is_empty() {
        issues.push(issue(
            source,
            1,
            LocalMakeIncludeIssueKind::DeferredScope,
            source.display().to_string(),
            "the fragment is not a complete declaration-owned literal define-header scope",
        ));
    }

    if issues.is_empty() {
        Ok(FragmentSafety {
            assigned_variables: assigned.into_iter().collect(),
            has_conditionals,
            plain_source_list: false,
            literal_define_header: true,
        })
    } else {
        Err(issues)
    }
}

/// Returns the rule target only for a single-token rule with no prerequisites.
fn literal_header_rule_target(line: &str) -> Option<&str> {
    if line.contains("::") || line.contains('|') || line.contains(';') {
        return None;
    }
    let (target, prerequisites) = line.split_once(':')?;
    let target = target.trim();
    if !prerequisites.trim().is_empty()
        || target.is_empty()
        || target.chars().any(char::is_whitespace)
        || target.contains('%')
        || target.contains('`')
        || target.contains('\\')
    {
        return None;
    }
    Some(target)
}

/// Parses exactly `echo "#define IDENT VALUE" >header` or its append form.
fn literal_define_recipe(line: &str) -> Option<(&str, &str, bool)> {
    let quoted = line.strip_prefix("echo \"")?;
    let close = quoted.rfind('"')?;
    let definition = quoted[..close].strip_prefix("#define ")?.trim();
    let redirect = quoted[close + 1..].trim();
    let (append, destination) = if let Some(destination) = redirect.strip_prefix(">>") {
        (true, destination.trim())
    } else {
        (false, redirect.strip_prefix('>')?.trim())
    };
    if redirect
        .strip_prefix(if append { ">>" } else { ">" })?
        .trim()
        != destination
        || destination.is_empty()
        || destination == "."
        || destination == ".."
        || destination.contains('/')
        || destination.contains('\\')
        || Path::new(destination).extension() != Some(std::ffi::OsStr::new("h"))
        || !destination.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
        })
    {
        return None;
    }

    let mut words = definition.split_whitespace();
    let name = words.next()?;
    let value = words.next()?;
    if name.is_empty()
        || value.is_empty()
        || words.next().is_some()
        || !name.chars().enumerate().all(|(index, character)| {
            character == '_'
                || character.is_ascii_alphabetic()
                || (index > 0 && character.is_ascii_digit())
        })
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(
                    character,
                    '_' | '+'
                        | '.'
                        | ','
                        | ':'
                        | '/'
                        | '<'
                        | '>'
                        | '='
                        | '!'
                        | '&'
                        | '|'
                        | '%'
                        | '*'
                        | '~'
                        | '?'
                        | '@'
                        | '#'
                        | '^'
                        | '('
                        | ')'
                        | '-'
                )
        })
    {
        return None;
    }
    Some((definition, destination, append))
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
    let line = strip_make_comment(line);
    // A continued compiler flag such as
    // `    -include $(SRCDIR)/$(CURDIR)/override.h` is not a Make include
    // directive. All real source-tree include directives are at column zero;
    // retaining that boundary prevents a C header from being parsed as a safe
    // variable fragment.
    if line.starts_with(char::is_whitespace) {
        return None;
    }
    let line = line.trim_end();
    let (optional, tail) = if let Some(tail) = directive_tail(line, "-include") {
        (true, tail)
    } else {
        let tail = directive_tail(line, "include")?;
        (false, tail)
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
    if has_source && has_curdir && !owned_make_opts {
        return true;
    }

    // Shared, literal configuration fragments such as Mesa's mesa.cfg are
    // just as local and bounded as a CURDIR-relative inventory. Admit only a
    // single source-root path with no remaining Make expansion. The global
    // config/aros.cfg is deliberately left to DirVars and the existing
    // collectors; attempting to inline it in every mmakefile would duplicate
    // the build-wide configuration scope.
    let trimmed = path.trim();
    let relative = trimmed
        .strip_prefix("$(SRCDIR)/")
        .or_else(|| trimmed.strip_prefix("${SRCDIR}/"));
    relative.is_some_and(|relative| {
        Path::new(relative)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("cfg"))
            && !relative.starts_with("config/")
            && !relative.contains('$')
            && !relative.contains(char::is_whitespace)
    })
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

#[cfg(test)]
mod tests {
    use super::{
        inline_native_make_configuration, inline_native_make_configuration_with_templates,
        LocalMakeIncludeLimits,
    };
    use super::{is_local_candidate, parse_include_directive};
    use aros_common::native_make_template::ResolvedGeneratedMakeTemplate;
    use std::collections::BTreeMap;
    use std::path::Path;

    fn native_fixture() -> (tempfile::TempDir, BTreeMap<String, String>) {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(directory.path().join("config")).unwrap();
        std::fs::create_dir_all(directory.path().join("arch/native")).unwrap();
        std::fs::write(
            directory.path().join("config/aros.cfg"),
            "include generated/target.cfg\n",
        )
        .unwrap();
        std::fs::write(
            directory.path().join("arch/native/compile.mk"),
            "undefine UNUSED\nFLAGS = -DFLAG=1 -UFLAG -DFLAG=2 -DFLAG=2\n",
        )
        .unwrap();
        (
            directory,
            BTreeMap::from([("config/aros.cfg".into(), "arch/native/compile.mk".into())]),
        )
    }

    #[test]
    fn native_configuration_expands_only_explicit_regular_endpoints_in_order() {
        let (directory, bindings) = native_fixture();
        let scan = inline_native_make_configuration(
            "BEFORE := first\ninclude $(SRCDIR)/config/aros.cfg\nFLAGS += -DTAIL\n",
            directory.path(),
            std::path::Path::new("compiler/libinit/mmakefile.src"),
            LocalMakeIncludeLimits::default(),
            &bindings,
        );
        assert!(scan.issues.is_empty(), "{:#?}", scan.issues);
        assert_eq!(scan.fragments.len(), 1);
        assert!(!scan.expanded.contains("include generated/target.cfg"));
        assert!(!scan.expanded.contains("include $(SRCDIR)/config/aros.cfg"));
        assert!(scan.expanded.contains("undefine UNUSED"));
        assert!(scan.expanded.contains("-DFLAG=1 -UFLAG -DFLAG=2 -DFLAG=2"));
        assert!(scan.expanded.find("BEFORE").unwrap() < scan.expanded.find("FLAGS =").unwrap());
        assert!(scan.expanded.find("FLAGS =").unwrap() < scan.expanded.find("FLAGS +=").unwrap());
    }

    #[test]
    fn native_configuration_keeps_unbound_or_unsafe_includes_explicit() {
        let (directory, bindings) = native_fixture();
        let input = "include $(SRCDIR)/config/aros.cfg\n";
        let unbound = inline_native_make_configuration(
            input,
            directory.path(),
            std::path::Path::new("compiler/libinit/mmakefile.src"),
            LocalMakeIncludeLimits::default(),
            &BTreeMap::new(),
        );
        assert_eq!(unbound.expanded, input);
        for body in [
            "FLAGS = -DGOOD\ninclude generated/target.cfg\n",
            "FLAGS = $(shell touch sentinel)\n",
            "entry.o : entry.c\n\tcc -c entry.c\n",
        ] {
            std::fs::write(directory.path().join("arch/native/compile.mk"), body).unwrap();
            let scan = inline_native_make_configuration(
                input,
                directory.path(),
                std::path::Path::new("compiler/libinit/mmakefile.src"),
                LocalMakeIncludeLimits::default(),
                &bindings,
            );
            assert!(!scan.issues.is_empty());
            assert_eq!(scan.expanded, input);
            assert!(scan.fragments.is_empty());
            assert!(!directory.path().join("sentinel").exists());
        }
    }

    #[test]
    fn native_configuration_refuses_missing_endpoints_limits_and_bad_replacements() {
        let (directory, mut bindings) = native_fixture();
        for replacement in [
            "../compile.mk",
            "/tmp/compile.mk",
            "arch/native/compile.cfg",
            "arch/native/missing.mk",
            "arch/native/$(NAME).mk",
        ] {
            bindings.insert("config/aros.cfg".into(), replacement.into());
            let scan = inline_native_make_configuration(
                "include $(SRCDIR)/config/aros.cfg\n",
                directory.path(),
                std::path::Path::new("compiler/libinit/mmakefile.src"),
                LocalMakeIncludeLimits::default(),
                &bindings,
            );
            assert!(!scan.issues.is_empty(), "{replacement}");
            assert!(scan.fragments.is_empty());
        }
        bindings.insert("config/aros.cfg".into(), "arch/native/compile.mk".into());
        let scan = inline_native_make_configuration(
            "include $(SRCDIR)/config/aros.cfg\n",
            directory.path(),
            std::path::Path::new("compiler/libinit/mmakefile.src"),
            LocalMakeIncludeLimits {
                bytes: 4,
                ..LocalMakeIncludeLimits::default()
            },
            &bindings,
        );
        assert!(!scan.issues.is_empty());
        assert!(scan.fragments.is_empty());
        std::fs::remove_file(directory.path().join("config/aros.cfg")).unwrap();
        let scan = inline_native_make_configuration(
            "-include $(SRCDIR)/config/aros.cfg\n",
            directory.path(),
            std::path::Path::new("compiler/libinit/mmakefile.src"),
            LocalMakeIncludeLimits::default(),
            &bindings,
        );
        assert!(!scan.issues.is_empty());
        assert!(scan.fragments.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn native_configuration_refuses_source_and_projection_symlinks() {
        use std::os::unix::fs::symlink;
        let (directory, bindings) = native_fixture();
        for path in ["config/aros.cfg", "arch/native/compile.mk"] {
            let original = directory.path().join(path);
            let saved = original.with_extension("saved");
            std::fs::rename(&original, &saved).unwrap();
            symlink(&saved, &original).unwrap();
            let scan = inline_native_make_configuration(
                "include $(SRCDIR)/config/aros.cfg\n",
                directory.path(),
                std::path::Path::new("compiler/libinit/mmakefile.src"),
                LocalMakeIncludeLimits::default(),
                &bindings,
            );
            assert!(!scan.issues.is_empty(), "{path}");
            assert!(scan.fragments.is_empty());
            std::fs::remove_file(&original).unwrap();
            std::fs::rename(&saved, &original).unwrap();
        }
    }

    fn resolved_geninc_template() -> BTreeMap<String, ResolvedGeneratedMakeTemplate> {
        BTreeMap::from([(
            "compiler/include/geninc.cfg".into(),
            ResolvedGeneratedMakeTemplate {
                template_relative: "compiler/include/geninc.cfg.in".into(),
                expanded_text: "%common\nEXECSMP=\"\"\n".into(),
                substitutions: BTreeMap::from([("@ENABLE_EXECSMP@".into(), String::new())]),
            },
        )])
    }

    #[test]
    fn generated_template_uses_only_sealed_text_and_preserves_provenance() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(directory.path().join("compiler/include")).unwrap();
        std::fs::write(
            directory.path().join("compiler/include/geninc.cfg.in"),
            "%common\nEXECSMP=\"@ENABLE_EXECSMP@\"\n",
        )
        .unwrap();
        let generated = directory.path().join("compiler/include/geninc.cfg");
        assert!(!generated.exists());

        let scan = inline_native_make_configuration_with_templates(
            "include $(TOP)/$(CURDIR)/geninc.cfg\nAFTER=present\n",
            directory.path(),
            Path::new("compiler/include/mmakefile.src"),
            LocalMakeIncludeLimits::default(),
            &BTreeMap::new(),
            &resolved_geninc_template(),
        );

        assert!(scan.issues.is_empty(), "{:#?}", scan.issues);
        assert_eq!(scan.fragments.len(), 1);
        assert_eq!(
            scan.fragments[0].path,
            Path::new("compiler/include/geninc.cfg.in")
        );
        assert_eq!(
            scan.fragments[0].generated_output.as_deref(),
            Some(Path::new("compiler/include/geninc.cfg"))
        );
        assert_eq!(
            scan.fragments[0].template_substitutions.as_ref().unwrap()["@ENABLE_EXECSMP@"],
            ""
        );
        assert!(scan.expanded.contains("%common\nEXECSMP=\"\"\n"));
        assert!(scan.expanded.contains("AFTER=present"));
        assert!(!scan
            .expanded
            .contains("include $(TOP)/$(CURDIR)/geninc.cfg"));
        assert!(!generated.exists());
    }

    #[test]
    fn generated_template_never_reads_a_hostile_existing_output() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(directory.path().join("compiler/include")).unwrap();
        std::fs::write(
            directory.path().join("compiler/include/geninc.cfg"),
            "$(shell touch should-not-run)\n",
        )
        .unwrap();
        let scan = inline_native_make_configuration_with_templates(
            "include $(TOP)/$(CURDIR)/geninc.cfg\n",
            directory.path(),
            Path::new("compiler/include/mmakefile.src"),
            LocalMakeIncludeLimits::default(),
            &BTreeMap::new(),
            &resolved_geninc_template(),
        );

        assert!(scan.issues.is_empty(), "{:#?}", scan.issues);
        assert!(scan.expanded.contains("EXECSMP=\"\""));
        assert!(!scan.expanded.contains("should-not-run"));
        assert!(!directory.path().join("should-not-run").exists());
    }

    #[test]
    fn generated_template_without_binding_is_left_visible_and_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let input = "include $(TOP)/$(CURDIR)/geninc.cfg\n";
        let scan = inline_native_make_configuration(
            input,
            directory.path(),
            Path::new("compiler/include/mmakefile.src"),
            LocalMakeIncludeLimits::default(),
            &BTreeMap::new(),
        );
        assert_eq!(scan.expanded, input);
        assert!(scan.fragments.is_empty());
        assert_eq!(scan.issues.len(), 1);
        assert_eq!(
            scan.issues[0].kind,
            super::LocalMakeIncludeIssueKind::UnresolvedPath
        );
    }

    #[test]
    fn generated_template_rejects_overlapping_keys_and_resource_overruns_atomically() {
        let directory = tempfile::tempdir().unwrap();
        let input = "include $(TOP)/$(CURDIR)/geninc.cfg\n";
        let mut overlapping = BTreeMap::new();
        overlapping.insert(
            "compiler/include/geninc.cfg".into(),
            "compiler/include/other.mk".into(),
        );
        let scan = inline_native_make_configuration_with_templates(
            input,
            directory.path(),
            Path::new("compiler/include/mmakefile.src"),
            LocalMakeIncludeLimits::default(),
            &overlapping,
            &resolved_geninc_template(),
        );
        assert_eq!(scan.expanded, input);
        assert!(scan.fragments.is_empty());
        assert_eq!(scan.issues.len(), 1);

        let templates = resolved_geninc_template();
        for limits in [
            LocalMakeIncludeLimits {
                files: 0,
                ..LocalMakeIncludeLimits::default()
            },
            LocalMakeIncludeLimits {
                bytes: 4,
                ..LocalMakeIncludeLimits::default()
            },
        ] {
            let scan = inline_native_make_configuration_with_templates(
                input,
                directory.path(),
                Path::new("compiler/include/mmakefile.src"),
                limits,
                &BTreeMap::new(),
                &templates,
            );
            assert_eq!(scan.expanded, input);
            assert!(scan.fragments.is_empty());
            assert_eq!(scan.issues.len(), 1);
        }
    }

    #[test]
    fn generated_template_rejects_noncanonical_paths() {
        let directory = tempfile::tempdir().unwrap();
        let mut templates = resolved_geninc_template();
        let value = templates.remove("compiler/include/geninc.cfg").unwrap();
        templates.insert("compiler/include/../geninc.cfg".into(), value);
        let input = "include $(TOP)/$(CURDIR)/geninc.cfg\n";
        let scan = inline_native_make_configuration_with_templates(
            input,
            directory.path(),
            Path::new("compiler/include/mmakefile.src"),
            LocalMakeIncludeLimits::default(),
            &BTreeMap::new(),
            &templates,
        );
        assert_eq!(scan.expanded, input);
        assert!(scan.fragments.is_empty());
        assert_eq!(scan.issues.len(), 1);
    }

    #[test]
    fn literal_source_root_cfg_is_local_but_global_and_dynamic_scopes_are_not() {
        assert!(is_local_candidate("$(SRCDIR)/workbench/libs/mesa/mesa.cfg"));
        assert!(is_local_candidate("$(SRCDIR)/$(CURDIR)/sources.inc"));
        assert!(!is_local_candidate("$(SRCDIR)/config/aros.cfg"));
        assert!(!is_local_candidate(
            "$(SRCDIR)/tools/crosstools/$(AROS_TOOLCHAIN).cfg"
        ));
    }

    #[test]
    fn an_indented_compiler_include_option_is_not_a_make_include_directive() {
        assert!(
            parse_include_directive("include $(SRCDIR)/workbench/libs/mesa/mesa.cfg").is_some()
        );
        assert!(
            parse_include_directive("    -include $(SRCDIR)/$(CURDIR)/v3d_aros_override.h")
                .is_none()
        );
    }
}
