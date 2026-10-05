//! Bounded parser for source-declared MetaMake `#MM` ownership metadata.
//!
//! It models only already-GenMF-expanded makefile text and explicit project-global
//! bindings supplied by the caller. This parser is not a GNU Make evaluator, producer
//! proof, or admission decision; callers remain responsible for discovery and evidence.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

// Reference buffers include a terminating NUL. Larger configurable resource
// budgets must not admit identities outside this safe reference subset.
const REFERENCE_IDENTITY_BYTES: usize = 4095;
const REFERENCE_VARIABLE_BYTES: usize = 255;

pub type ParseResult<T> = Result<T, String>;

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Preserve reference empty substitutions as unresolved metadata endpoints.
    /// They can never satisfy a selected endpoint, even if declared as a target.
    pub preserve_empty_endpoints: bool,
    pub max_files: usize,
    pub max_total_bytes: usize,
    pub max_file_bytes: usize,
    pub max_total_lines: usize,
    pub max_line_bytes: usize,
    pub max_tokens: usize,
    pub max_targets: usize,
    pub max_edges: usize,
    pub max_work: usize,
    pub max_global_bindings: usize,
    pub max_global_bytes: usize,
    pub max_global_value_bytes: usize,
    pub max_identity_bytes: usize,
    pub max_total_identity_copy_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            preserve_empty_endpoints: false,
            max_files: 20_000,
            max_total_bytes: 64 * 1024 * 1024,
            max_file_bytes: 8 * 1024 * 1024,
            max_total_lines: 2_000_000,
            max_line_bytes: 64 * 1024,
            max_tokens: 2_000_000,
            max_targets: 500_000,
            max_edges: 1_000_000,
            max_work: 4_000_000,
            max_global_bindings: 16_384,
            max_global_bytes: 16 * 1024 * 1024,
            max_global_value_bytes: 1024 * 1024,
            max_identity_bytes: REFERENCE_IDENTITY_BYTES,
            max_total_identity_copy_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct LocalTarget {
    // `readtargets()` AND-merges this flag for repeated declarations in one
    // makefile. A real declaration therefore makes the local target real.
    virtual_target: bool,
    dependencies: BTreeSet<String>,
}

#[derive(Debug, Default)]
struct LocalFileParse {
    targets: BTreeMap<String, LocalTarget>,
    declarations: Vec<TargetDeclarationProvenance>,
}

/// One dependency token as parsed from a `#MM` declaration, before and after
/// the parser's single project-global substitution pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DependencyProvenance {
    /// The exact token returned by the `#MM` tokenizer.
    pub raw_expression: String,
    /// The endpoint after substituting explicitly bound project globals once.
    pub concrete: String,
}

/// Source-token provenance for one named target token in an expanded Makefile.
///
/// A rule with multiple targets yields one record per target token, with its
/// dependency tokens repeated on each record. Repeated declarations remain
/// separate records, in file-map and source order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetDeclarationProvenance {
    /// One-based physical line of the expanded #MM directive. This binds
    /// identical declarations to their individual GenMF expansion origins.
    pub expanded_line: usize,
    /// Generated Makefile key supplied to `parse_expanded_files`.
    pub file: String,
    /// Exact target token returned by the `#MM` tokenizer.
    pub raw_target: String,
    /// Target identity after substituting explicitly bound project globals once.
    pub target: String,
    /// Whether this directive used the `#MM-` marker.
    ///
    /// For a bare marker followed by a target line, this preserves the source
    /// directive even though MetaMake treats the declaration as nonvirtual.
    pub virtual_target: bool,
    /// Whether this target came from a bare `#MM` / `#MM-` marker and the next
    /// physical line, rather than a target token on the directive itself.
    pub bare_marker: bool,
    /// Whether MetaMake treats this declaration as a Makefile owner claim.
    /// Bare-marker declarations claim ownership even when their marker is
    /// `#MM-`, matching the parser's existing reference behavior.
    pub claims_make_owner: bool,
    /// Dependency tokens from the same declaration, preserving duplicates and
    /// token order. Bare-marker declarations have no dependencies.
    pub dependencies: Vec<DependencyProvenance>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MetaMakeOwnerGraph {
    known_targets: BTreeSet<String>,
    dependencies: BTreeMap<String, BTreeSet<String>>,
    // Only nonvirtual Makefile refs are owners that `maketarget()` passes to
    // `callmake()`. Virtual declarations still contribute dependencies.
    owners: BTreeMap<String, BTreeSet<String>>,
    declarations: Vec<TargetDeclarationProvenance>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub reached_targets: BTreeSet<String>,
    pub missing_endpoints: BTreeSet<String>,
    pub selected_owner_files: BTreeSet<String>,
}

impl MetaMakeOwnerGraph {
    /// Project the reference's missing-variable marker for explicitly proven
    /// absent names. This does not establish their absence: the caller must
    /// bind complete project/global snapshots and exclude ambient fallback.
    /// Unlisted missing bindings retain the strict parser's refusal.
    ///
    /// Absent variables become literal `?$(NAME)` identities, not empty
    /// strings. They are not rescanned and cannot satisfy a native producer.
    ///
    /// # Errors
    /// Refuses conflicting bound/absent names, unsafe or excessive absence
    /// declarations, and every error from the strict metadata parser.
    pub fn parse_with_declared_absence(
        files: &BTreeMap<String, String>,
        globals: &BTreeMap<String, String>,
        absent: &BTreeSet<String>,
        limits: Limits,
    ) -> ParseResult<Self> {
        if globals.len().saturating_add(absent.len()) > limits.max_global_bindings {
            return Err("bound/absent project variables exceed entry limit".into());
        }
        validate_global_limits(globals, limits)?;
        let mut closed = globals.clone();
        for name in absent {
            // var.c's 256-byte missing-value buffer includes ?$(...), plus NUL.
            if name.is_empty()
                || name.len() > 251
                || name.bytes().any(|byte| {
                    !byte.is_ascii_graphic() || matches!(byte, b'$' | b'(' | b')' | b'"')
                })
            {
                return Err("invalid explicitly absent project variable".into());
            }
            if closed.insert(name.clone(), format!("?$({name})")).is_some() {
                return Err(format!("project variable {name} is both bound and absent"));
            }
        }
        Self::parse_expanded_files(files, &closed, limits)
    }

    /// Parse a sorted map of generated Makefile path to contents. `globals`
    /// must be the caller's proven project-global MetaMake bindings.
    ///
    /// # Errors
    /// Refuses unsupported declarations, unbound identities, malformed or
    /// whitespace-containing endpoints and resource excess.
    pub fn parse_expanded_files(
        files: &BTreeMap<String, String>,
        globals: &BTreeMap<String, String>,
        limits: Limits,
    ) -> ParseResult<Self> {
        if files.len() > limits.max_files {
            return Err("file limit exceeded".into());
        }
        validate_global_limits(globals, limits)?;

        let mut total_bytes = 0usize;
        let mut total_lines = 0usize;
        let mut tokens_seen = 0usize;
        let mut provenance_edges_seen = 0usize;
        let mut work = 0usize;
        let mut identity_copy_bytes = 0usize;
        let mut parsed_files = Vec::with_capacity(files.len());

        for (path, contents) in files {
            if path.len() > limits.max_identity_bytes {
                return Err("makefile path byte limit exceeded".into());
            }
            let file_bytes = contents.len();
            total_bytes = checked_add(
                total_bytes,
                file_bytes,
                limits.max_total_bytes,
                "total byte",
            )?;
            if file_bytes > limits.max_file_bytes {
                return Err(format!("file byte limit exceeded in {path}"));
            }

            let mut physical_lines = Vec::new();
            for line in contents.split_terminator('\n') {
                if line.len() > limits.max_line_bytes {
                    return Err(format!("line byte limit exceeded in {path}"));
                }
                total_lines = checked_add(total_lines, 1, limits.max_total_lines, "line")?;
                physical_lines.push(line.to_owned());
            }
            // `split_terminator` yields no lines for an empty file, matching
            // the absence of any `fgets()` result.
            let local = parse_file_lines(
                &physical_lines,
                path,
                globals,
                limits,
                FileParseBudget {
                    tokens_seen: &mut tokens_seen,
                    provenance_edges_seen: &mut provenance_edges_seen,
                    work: &mut work,
                    identity_copy_bytes: &mut identity_copy_bytes,
                },
            )?;
            charge_identity_copy(
                &mut identity_copy_bytes,
                path.len(),
                limits,
                "makefile path copy",
            )?;
            parsed_files.push((path.clone(), local));
        }

        let mut graph = Self::default();
        let mut global_edge_count = 0usize;
        for (path, local) in parsed_files {
            let declaration_count = local.declarations.len();
            work = checked_add(
                work,
                declaration_count,
                limits.max_work,
                "provenance merge work",
            )?;
            graph.declarations.extend(local.declarations);
            for (name, target) in local.targets {
                bump(&mut work, limits.max_work, "graph merge work")?;
                charge_identity_copy(
                    &mut identity_copy_bytes,
                    name.len(),
                    limits,
                    "global target identity copy",
                )?;
                graph.known_targets.insert(name.clone());
                if graph.known_targets.len() > limits.max_targets {
                    return Err("global target limit exceeded".into());
                }
                charge_identity_copy(
                    &mut identity_copy_bytes,
                    name.len(),
                    limits,
                    "global dependency-map key copy",
                )?;
                let edges = graph.dependencies.entry(name.clone()).or_default();
                for dependency in target.dependencies {
                    bump(&mut work, limits.max_work, "graph merge work")?;
                    if edges.insert(dependency) {
                        global_edge_count =
                            checked_add(global_edge_count, 1, limits.max_edges, "global edge")?;
                    }
                }
                if !target.virtual_target {
                    bump(&mut work, limits.max_work, "graph merge work")?;
                    charge_identity_copy(
                        &mut identity_copy_bytes,
                        path.len(),
                        limits,
                        "owner path copy",
                    )?;
                    graph.owners.entry(name).or_default().insert(path.clone());
                }
            }
        }
        Ok(graph)
    }

    #[must_use]
    pub const fn known_targets(&self) -> &BTreeSet<String> {
        &self.known_targets
    }

    #[must_use]
    pub fn owner_files(&self, target: &str) -> Option<&BTreeSet<String>> {
        self.owners.get(target)
    }

    #[must_use]
    pub fn dependencies(&self, target: &str) -> Option<&BTreeSet<String>> {
        self.dependencies.get(target)
    }

    /// Return every parsed named-target declaration in generated-file and
    /// source order, including virtual declarations and repeated tokens.
    #[must_use]
    pub fn declarations(&self) -> &[TargetDeclarationProvenance] {
        &self.declarations
    }

    /// Return owner files reachable from command-line roots, and report every
    /// missing endpoint rather than treating it as absent or satisfied.
    ///
    /// # Errors
    /// Refuses excessive roots, traversal work or copied identity bytes.
    pub fn select(&self, roots: &[String], limits: Limits) -> ParseResult<Selection> {
        let mut selection = Selection::default();
        let mut queue = VecDeque::new();
        let mut work = 0usize;
        let mut identity_copy_bytes = 0usize;

        for root in roots {
            bump(&mut work, limits.max_work, "selection work")?;
            if root.len() > limits.max_identity_bytes {
                return Err("selection root identity byte limit exceeded".into());
            }
            if !root.is_empty() && self.known_targets.contains(root) {
                charge_identity_copy(
                    &mut identity_copy_bytes,
                    root.len(),
                    limits,
                    "selection root copy",
                )?;
                queue.push_back(root.clone());
            } else {
                charge_identity_copy(
                    &mut identity_copy_bytes,
                    root.len(),
                    limits,
                    "missing-root identity copy",
                )?;
                selection.missing_endpoints.insert(root.clone());
            }
        }

        while let Some(target) = queue.pop_front() {
            bump(&mut work, limits.max_work, "selection work")?;
            charge_identity_copy(
                &mut identity_copy_bytes,
                target.len(),
                limits,
                "reached-target copy",
            )?;
            if !selection.reached_targets.insert(target.clone()) {
                continue;
            }
            if let Some(owners) = self.owners.get(&target) {
                for owner in owners {
                    charge_identity_copy(
                        &mut identity_copy_bytes,
                        owner.len(),
                        limits,
                        "selected-owner path copy",
                    )?;
                    selection.selected_owner_files.insert(owner.clone());
                }
            }
            if let Some(dependencies) = self.dependencies.get(&target) {
                for dependency in dependencies {
                    bump(&mut work, limits.max_work, "selection work")?;
                    if !dependency.is_empty() && self.known_targets.contains(dependency) {
                        charge_identity_copy(
                            &mut identity_copy_bytes,
                            dependency.len(),
                            limits,
                            "dependency queue copy",
                        )?;
                        queue.push_back(dependency.clone());
                    } else {
                        charge_identity_copy(
                            &mut identity_copy_bytes,
                            dependency.len(),
                            limits,
                            "missing-dependency identity copy",
                        )?;
                        selection.missing_endpoints.insert(dependency.clone());
                    }
                }
            }
        }
        Ok(selection)
    }
}

fn validate_global_limits(globals: &BTreeMap<String, String>, limits: Limits) -> ParseResult<()> {
    if globals.len() > limits.max_global_bindings {
        return Err("project-global binding count limit exceeded".into());
    }
    let mut bytes = 0usize;
    for (name, value) in globals {
        if name.len() > limits.max_identity_bytes.min(REFERENCE_VARIABLE_BYTES) {
            return Err(format!("project-global variable name too long: {name}"));
        }
        if value.len() > limits.max_global_value_bytes {
            return Err(format!("project-global value limit exceeded for {name}"));
        }
        bytes = checked_add(bytes, name.len(), limits.max_global_bytes, "global byte")?;
        bytes = checked_add(bytes, value.len(), limits.max_global_bytes, "global byte")?;
    }
    Ok(())
}

struct FileParseBudget<'a> {
    tokens_seen: &'a mut usize,
    provenance_edges_seen: &'a mut usize,
    work: &'a mut usize,
    identity_copy_bytes: &'a mut usize,
}

fn parse_file_lines(
    lines: &[String],
    path: &str,
    globals: &BTreeMap<String, String>,
    limits: Limits,
    budget: FileParseBudget<'_>,
) -> ParseResult<LocalFileParse> {
    let FileParseBudget {
        tokens_seen,
        provenance_edges_seen,
        work,
        identity_copy_bytes,
    } = budget;
    let mut targets = BTreeMap::<String, LocalTarget>::new();
    let mut declarations = Vec::new();
    let mut line_index = 0usize;
    let mut local_edge_pairs = 0usize;
    while line_index < lines.len() {
        bump(work, limits.max_work, "parse work")?;
        let line = &lines[line_index];
        line_index += 1;
        let expanded_line = line_index;
        if !line.as_bytes().starts_with(b"#MM") {
            continue;
        }

        let logical = collect_continuations(line, lines, &mut line_index, path, limits, work)?;
        if logical.as_bytes().iter().any(|byte| !byte.is_ascii()) {
            return Err(format!("non-ASCII #MM directive is unsupported in {path}"));
        }
        let mut rest = &logical.as_bytes()[3..];
        let virtual_target = if rest.first() == Some(&b'-') {
            rest = &rest[1..];
            true
        } else {
            false
        };
        let rest = trim_ascii_start(rest);
        if rest.first() == Some(&b'-') {
            return Err(format!("malformed virtual marker in {path}: {logical}"));
        }

        if rest.is_empty() {
            // `readtargets()` consumes the next physical line, takes the text
            // before its first colon, and only registers its first token.
            // It hardcodes virtual=false in this bare form, even after #MM-.
            let next = lines
                .get(line_index)
                .ok_or_else(|| format!("bare #MM has no following target in {path}"))?;
            line_index += 1;
            let target_text = next
                .as_bytes()
                .iter()
                .position(|b| *b == b':')
                .map_or_else(|| next.as_bytes(), |colon| &next.as_bytes()[..colon]);
            let mut parsed = tokenize_make_args(target_text, path)?;
            count_tokens(parsed.len(), tokens_seen, limits)?;
            if parsed.len() != 1 {
                return Err(format!(
                    "bare #MM must have exactly one target token in {path}: {next}"
                ));
            }
            let raw_target = parsed.remove(0);
            let name =
                substitute_identity(&raw_target, globals, path, limits, identity_copy_bytes)?;
            bump(work, limits.max_work, "provenance declaration work")?;
            charge_identity_copy(
                identity_copy_bytes,
                path.len(),
                limits,
                "provenance makefile path copy",
            )?;
            charge_identity_copy(
                identity_copy_bytes,
                raw_target.len(),
                limits,
                "provenance raw target copy",
            )?;
            charge_identity_copy(
                identity_copy_bytes,
                name.len(),
                limits,
                "target name copy for provenance",
            )?;
            declarations.push(TargetDeclarationProvenance {
                expanded_line,
                file: path.to_owned(),
                raw_target,
                target: name.clone(),
                virtual_target,
                bare_marker: true,
                claims_make_owner: true,
                dependencies: Vec::new(),
            });
            merge_local_target(&mut targets, name, false, BTreeSet::new(), limits)?;
            continue;
        }

        let colon = rest.iter().position(|b| *b == b':');
        let (target_text, dependency_text) = colon.map_or_else(
            || (rest, &[][..]),
            |colon| (&rest[..colon], &rest[colon + 1..]),
        );
        let raw_targets = tokenize_make_args(target_text, path)?;
        let raw_dependencies = tokenize_make_args(dependency_text, path)?;
        count_tokens(
            raw_targets.len() + raw_dependencies.len(),
            tokens_seen,
            limits,
        )?;
        if raw_targets.is_empty() {
            return Err(format!(
                "#MM declaration has no target in {path}: {logical}"
            ));
        }

        let mut target_tokens = Vec::with_capacity(raw_targets.len());
        for raw in raw_targets {
            let name = substitute_identity(&raw, globals, path, limits, identity_copy_bytes)?;
            target_tokens.push((raw, name));
        }
        let mut dependency_names = BTreeSet::new();
        let mut dependency_tokens = Vec::with_capacity(raw_dependencies.len());
        for raw in raw_dependencies {
            let endpoint = substitute_identity(&raw, globals, path, limits, identity_copy_bytes)?;
            charge_identity_copy(
                identity_copy_bytes,
                endpoint.len(),
                limits,
                "dependency identity copy for graph set",
            )?;
            dependency_names.insert(endpoint.clone());
            dependency_tokens.push((raw, endpoint));
        }
        if dependency_names.len() > limits.max_edges {
            return Err(format!("per-rule edge limit exceeded in {path}"));
        }
        let edge_pairs = target_tokens
            .len()
            .checked_mul(dependency_names.len())
            .ok_or_else(|| format!("per-file edge counter overflow in {path}"))?;
        let provenance_edge_pairs = target_tokens
            .len()
            .checked_mul(dependency_tokens.len())
            .ok_or_else(|| format!("per-file provenance edge counter overflow in {path}"))?;
        local_edge_pairs = checked_add(
            local_edge_pairs,
            edge_pairs,
            limits.max_edges,
            "per-file declared edge",
        )?;
        *provenance_edges_seen = checked_add(
            *provenance_edges_seen,
            provenance_edge_pairs,
            limits.max_edges,
            "global provenance edge",
        )?;
        // Charge fan-out before cloning dependency identities into local target
        // records; otherwise a finite but wide rule can allocate far more
        // edges than its token count suggests.
        *work = checked_add(*work, edge_pairs, limits.max_work, "parse work")?;
        *work = checked_add(
            *work,
            provenance_edge_pairs,
            limits.max_work,
            "provenance parse work",
        )?;
        for _ in 0..target_tokens.len() {
            for dependency in &dependency_names {
                charge_identity_copy(
                    identity_copy_bytes,
                    dependency.len(),
                    limits,
                    "per-target dependency clone",
                )?;
            }
        }

        for (raw_target, name) in target_tokens {
            bump(work, limits.max_work, "parse work")?;
            let mut provenance_dependencies = Vec::with_capacity(dependency_tokens.len());
            for (raw_expression, concrete) in &dependency_tokens {
                charge_identity_copy(
                    identity_copy_bytes,
                    raw_expression.len(),
                    limits,
                    "provenance raw dependency copy",
                )?;
                charge_identity_copy(
                    identity_copy_bytes,
                    concrete.len(),
                    limits,
                    "provenance dependency endpoint copy",
                )?;
                provenance_dependencies.push(DependencyProvenance {
                    raw_expression: raw_expression.clone(),
                    concrete: concrete.clone(),
                });
            }
            charge_identity_copy(
                identity_copy_bytes,
                path.len(),
                limits,
                "provenance makefile path copy",
            )?;
            charge_identity_copy(
                identity_copy_bytes,
                raw_target.len(),
                limits,
                "provenance raw target copy",
            )?;
            charge_identity_copy(
                identity_copy_bytes,
                name.len(),
                limits,
                "local target name copy for merged map",
            )?;
            let local_name = name.clone();
            declarations.push(TargetDeclarationProvenance {
                expanded_line,
                file: path.to_owned(),
                raw_target,
                target: name,
                virtual_target,
                bare_marker: false,
                claims_make_owner: !virtual_target,
                dependencies: provenance_dependencies,
            });
            merge_local_target(
                &mut targets,
                local_name,
                virtual_target,
                dependency_names.clone(),
                limits,
            )?;
        }
    }
    Ok(LocalFileParse {
        targets,
        declarations,
    })
}

/// Reconstruct one source declaration with the same bounded continuation
/// semantics used when capturing its MetaMake provenance.
pub(crate) fn source_declaration_at(
    source: &str,
    source_line: usize,
    path: &str,
) -> ParseResult<String> {
    let lines: Vec<_> = source.lines().map(str::to_owned).collect();
    let index = source_line
        .checked_sub(1)
        .ok_or("invalid #MM source line")?;
    let first = lines.get(index).ok_or("missing #MM source line")?;
    if !first.starts_with("#MM") {
        return Err("source line is not a column-zero #MM declaration".into());
    }
    let limits = Limits::default();
    if lines.len() > limits.max_total_lines || first.len() > limits.max_line_bytes {
        return Err("source #MM declaration exceeds its parsing limits".into());
    }
    let mut cursor = index + 1;
    let mut work = 0;
    collect_continuations(first, &lines, &mut cursor, path, limits, &mut work)
}

fn collect_continuations(
    first: &str,
    lines: &[String],
    line_index: &mut usize,
    path: &str,
    limits: Limits,
    work: &mut usize,
) -> ParseResult<String> {
    let mut logical = first.as_bytes().to_vec();
    let mut continuation_work = 0usize;
    loop {
        bump(work, limits.max_work, "continuation work")?;
        let last_nonspace = logical.iter().rposition(|b| !b.is_ascii_whitespace());
        if last_nonspace.is_none_or(|index| logical[index] != b'\\') {
            break;
        }
        continuation_work += 1;
        if continuation_work > limits.max_total_lines {
            return Err(format!("continuation limit exceeded in {path}"));
        }
        let next = lines
            .get(*line_index)
            .ok_or_else(|| format!("unterminated #MM continuation in {path}"))?;
        *line_index += 1;
        let next_bytes = next.as_bytes();
        // dirnode.c compares only three bytes of "##MM". It replaces the
        // overwritten last byte with the new line's final byte and terminates
        // there. Usually that byte is a backslash: the disabled dependency is
        // skipped but the continuation remains active. Do not approximate it
        // by dropping the whole line or applying GNU Make comment semantics.
        if next_bytes.starts_with(b"##M") {
            logical
                .pop()
                .ok_or_else(|| format!("invalid empty continuation in {path}"))?;
            logical.push(
                *next_bytes
                    .last()
                    .ok_or_else(|| format!("empty disabled continuation in {path}"))?,
            );
            continue;
        }
        if !next_bytes.starts_with(b"#MM") || next_bytes.len() < 4 {
            return Err(format!(
                "continuation must start with #MM in {path}: {next}"
            ));
        }
        // dirnode.c writes the next fgets() result at line+strlen(line)-1,
        // then removes exactly four bytes (#MM plus one following byte).
        if logical.is_empty() {
            return Err(format!("invalid empty continuation in {path}"));
        }
        logical.pop();
        logical.extend_from_slice(&next_bytes[4..]);
        if logical.len() > limits.max_line_bytes {
            return Err(format!("continued line byte limit exceeded in {path}"));
        }
    }
    let logical =
        String::from_utf8(logical).map_err(|_| format!("non-UTF-8 #MM directive in {path}"))?;
    Ok(logical)
}

fn tokenize_make_args(bytes: &[u8], path: &str) -> ParseResult<Vec<String>> {
    let mut tokens = Vec::new();
    let mut index = 0usize;
    while index < bytes.len() {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index == bytes.len() {
            break;
        }

        let start = index;
        while index < bytes.len() && !bytes[index].is_ascii_whitespace() {
            if bytes[index] == b'"' {
                // MetaMake's getargs() special-cases a leading quote without
                // advancing past it, yielding an empty token; quotes elsewhere
                // also have surprising split behavior. Reject the syntax
                // instead of inventing a quoted-token interpretation.
                return Err(format!("unsupported quoted #MM token in {path}"));
            }
            if !bytes[index].is_ascii_graphic() {
                return Err(format!("control byte in #MM identity in {path}"));
            }
            index += 1;
        }
        tokens.push(
            String::from_utf8(bytes[start..index].to_vec())
                .map_err(|_| format!("non-UTF-8 token in {path}"))?,
        );
        if tokens.len() > 254 {
            return Err(format!("MetaMake getargs() token cap exceeded in {path}"));
        }
    }
    Ok(tokens)
}

fn substitute_identity(
    token: &str,
    globals: &BTreeMap<String, String>,
    path: &str,
    limits: Limits,
    identity_copy_bytes: &mut usize,
) -> ParseResult<String> {
    let bytes = token.as_bytes();
    let identity_limit = limits.max_identity_bytes.min(REFERENCE_IDENTITY_BYTES);
    let mut index = 0usize;
    let mut output_len = 0usize;
    while index < bytes.len() {
        if bytes[index] != b'$' {
            output_len = checked_add(output_len, 1, identity_limit, "identity byte")?;
            index += 1;
            continue;
        }
        let (close, value) = global_expansion(bytes, index, globals, path, token)?;
        if value.bytes().any(|b| !b.is_ascii_graphic()) {
            return Err(format!("control/non-ASCII expansion in {path}: {token}"));
        }
        output_len = checked_add(output_len, value.len(), identity_limit, "identity byte")?;
        index = close + 1;
    }
    if output_len == 0 && !limits.preserve_empty_endpoints {
        return Err(format!("empty target identity in {path}: {token}"));
    }
    charge_identity_copy(identity_copy_bytes, output_len, limits, "expanded identity")?;
    let mut result = Vec::with_capacity(output_len);
    index = 0;
    while index < bytes.len() {
        if bytes[index] == b'$' {
            let (close, value) = global_expansion(bytes, index, globals, path, token)?;
            // MetaMake substvars() copies a value without rescanning it.
            result.extend_from_slice(value.as_bytes());
            index = close + 1;
        } else {
            result.push(bytes[index]);
            index += 1;
        }
    }
    if result.iter().any(u8::is_ascii_whitespace) {
        return Err(format!("whitespace target identity in {path}: {token}"));
    }
    String::from_utf8(result).map_err(|_| format!("non-UTF-8 identity in {path}"))
}

fn global_expansion<'a>(
    bytes: &[u8],
    index: usize,
    globals: &'a BTreeMap<String, String>,
    path: &str,
    token: &str,
) -> ParseResult<(usize, &'a str)> {
    if bytes.get(index + 1) != Some(&b'(') {
        return Err(format!("unsupported $ identity in {path}: {token}"));
    }
    let name_start = index + 2;
    let close = bytes[name_start..]
        .iter()
        .position(|byte| *byte == b')')
        .map(|offset| name_start + offset)
        .ok_or_else(|| format!("unterminated $(...) identity in {path}: {token}"))?;
    let name = std::str::from_utf8(&bytes[name_start..close])
        .map_err(|_| format!("non-UTF-8 variable name in {path}"))?;
    if name.is_empty()
        || name.len() > REFERENCE_VARIABLE_BYTES
        || name
            .bytes()
            .any(|b| !b.is_ascii_graphic() || b == b'$' || b == b'(')
    {
        return Err(format!("unsupported variable identity in {path}: {token}"));
    }
    let value = globals
        .get(name)
        .ok_or_else(|| format!("unbound project-global variable {name} in {path}"))?;
    Ok((close, value))
}

fn charge_identity_copy(
    current: &mut usize,
    amount: usize,
    limits: Limits,
    label: &str,
) -> ParseResult<()> {
    *current = checked_add(
        *current,
        amount,
        limits.max_total_identity_copy_bytes,
        label,
    )?;
    Ok(())
}

fn merge_local_target(
    targets: &mut BTreeMap<String, LocalTarget>,
    name: String,
    virtual_target: bool,
    dependencies: BTreeSet<String>,
    limits: Limits,
) -> ParseResult<()> {
    if !targets.contains_key(&name) && targets.len() >= limits.max_targets {
        return Err("per-file target limit exceeded".into());
    }
    match targets.entry(name) {
        std::collections::btree_map::Entry::Vacant(entry) => {
            if dependencies.len() > limits.max_edges {
                return Err("per-target edge limit exceeded".into());
            }
            entry.insert(LocalTarget {
                virtual_target,
                dependencies,
            });
        }
        std::collections::btree_map::Entry::Occupied(mut entry) => {
            let target = entry.get_mut();
            if target.dependencies.len().saturating_add(dependencies.len()) > limits.max_edges {
                return Err("per-target edge limit exceeded".into());
            }
            target.virtual_target &= virtual_target;
            target.dependencies.extend(dependencies);
        }
    }
    Ok(())
}

fn count_tokens(count: usize, seen: &mut usize, limits: Limits) -> ParseResult<()> {
    *seen = checked_add(*seen, count, limits.max_tokens, "token")?;
    Ok(())
}

fn checked_add(current: usize, amount: usize, max: usize, label: &str) -> ParseResult<usize> {
    let next = current
        .checked_add(amount)
        .ok_or_else(|| format!("{label} counter overflow"))?;
    if next > max {
        return Err(format!("{label} limit exceeded"));
    }
    Ok(next)
}

fn bump(work: &mut usize, max: usize, label: &str) -> ParseResult<()> {
    *work = checked_add(*work, 1, max, label)?;
    Ok(())
}

fn trim_ascii_start(mut bytes: &[u8]) -> &[u8] {
    while bytes.first().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[1..];
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_buffer_limits_hold_even_with_larger_configured_budgets() {
        let limits = Limits {
            max_identity_bytes: 8192,
            ..Limits::default()
        };
        let globals = BTreeMap::from([("BOUND".into(), "x".repeat(4095))]);
        assert!(MetaMakeOwnerGraph::parse_expanded_files(
            &file_map(&[("rules", "#MM $(BOUND)\n")]),
            &globals,
            limits,
        )
        .is_ok());
        assert!(MetaMakeOwnerGraph::parse_expanded_files(
            &file_map(&[("rules", "#MM x$(BOUND)\n")]),
            &globals,
            limits,
        )
        .unwrap_err()
        .contains("identity byte limit"));
        for length in [255, 256] {
            let name = "V".repeat(length);
            let globals = BTreeMap::from([(name.clone(), "target".into())]);
            let files = file_map(&[("rules", &format!("#MM $({name})\n"))]);
            let result = MetaMakeOwnerGraph::parse_expanded_files(&files, &globals, limits);
            assert_eq!(result.is_ok(), length == 255);
        }
        let files = file_map(&[("rules", &format!("#MM $({})\n", "V".repeat(256)))]);
        assert!(
            MetaMakeOwnerGraph::parse_expanded_files(&files, &BTreeMap::new(), limits)
                .unwrap_err()
                .contains("unsupported variable identity")
        );
    }

    #[test]
    fn explicit_absence_preserves_reference_marker_and_missing_endpoint() {
        let graph = MetaMakeOwnerGraph::parse_with_declared_absence(
            &file_map(&[("rules", "#MM root : child-$(ABSENT)\n")]),
            &BTreeMap::new(),
            &BTreeSet::from(["ABSENT".into()]),
            Limits::default(),
        )
        .unwrap();
        let selection = graph.select(&["root".into()], Limits::default()).unwrap();
        assert_eq!(
            selection.missing_endpoints,
            BTreeSet::from(["child-?$(ABSENT)".into()])
        );
        assert_eq!(
            selection.selected_owner_files,
            BTreeSet::from(["rules".into()])
        );
        assert!(!selection.reached_targets.contains("child-"));
    }

    #[test]
    fn bound_empty_profile_selector_is_distinct_from_a_declared_absent_selector() {
        let files = file_map(&[("rules", "#MM- root : child-$(VARIANT)\n#MM- child-\n")]);
        let bound = MetaMakeOwnerGraph::parse_expanded_files(
            &files,
            &BTreeMap::from([("VARIANT".into(), String::new())]),
            Limits::default(),
        )
        .unwrap()
        .select(&["root".into()], Limits::default())
        .unwrap();
        assert!(bound.reached_targets.contains("child-"));
        assert!(bound.missing_endpoints.is_empty());

        let absent = MetaMakeOwnerGraph::parse_with_declared_absence(
            &files,
            &BTreeMap::new(),
            &BTreeSet::from(["VARIANT".into()]),
            Limits::default(),
        )
        .unwrap()
        .select(&["root".into()], Limits::default())
        .unwrap();
        assert!(!absent.reached_targets.contains("child-"));
        assert_eq!(
            absent.missing_endpoints,
            BTreeSet::from(["child-?$(VARIANT)".into()])
        );
    }

    #[test]
    fn explicit_absence_is_not_a_default_for_other_unbound_variables() {
        let files = file_map(&[("rules", "#MM root-$(UNPROVEN)\n")]);
        assert!(MetaMakeOwnerGraph::parse_with_declared_absence(
            &files,
            &BTreeMap::new(),
            &BTreeSet::from(["ABSENT".into()]),
            Limits::default(),
        )
        .unwrap_err()
        .contains("unbound"));
    }

    #[test]
    fn explicit_absence_rejects_conflict_unsafe_names_and_reference_buffer_overflow() {
        for absent in [
            BTreeSet::from(["VALUE".into()]),
            BTreeSet::from(["BAD NAME".into()]),
            BTreeSet::from(["BAD)".into()]),
            BTreeSet::from(["x".repeat(252)]),
        ] {
            assert!(MetaMakeOwnerGraph::parse_with_declared_absence(
                &BTreeMap::new(),
                &globals(&[("VALUE", "bound")]),
                &absent,
                Limits::default(),
            )
            .is_err());
        }
    }

    #[test]
    fn bound_values_and_missing_markers_are_not_recursively_substituted() {
        let graph = MetaMakeOwnerGraph::parse_with_declared_absence(
            &file_map(&[("rules", "#MM root : $(ALIAS) child-$(ABSENT)\n")]),
            &globals(&[("ALIAS", "$(ABSENT)")]),
            &BTreeSet::from(["ABSENT".into()]),
            Limits::default(),
        )
        .unwrap();
        assert_eq!(
            graph.dependencies("root").unwrap(),
            &BTreeSet::from(["$(ABSENT)".into(), "child-?$(ABSENT)".into()])
        );
    }

    fn file_map(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(path, text)| ((*path).to_owned(), (*text).to_owned()))
            .collect()
    }

    fn globals(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect()
    }

    fn parse(entries: &[(&str, &str)], vars: &[(&str, &str)]) -> ParseResult<MetaMakeOwnerGraph> {
        MetaMakeOwnerGraph::parse_expanded_files(
            &file_map(entries),
            &globals(vars),
            Limits::default(),
        )
    }

    #[test]
    fn byte_zero_mm_lines_and_local_virtual_and_global_edge_union() {
        let graph = parse(
            &[
                (
                    "a/mmakefile",
                    " #MM ignored : ghost\n#MM- root : left\n#MM root : right\n#MM- left\n#MM right\n",
                ),
                ("b/mmakefile", "#MM root : third\n#MM third\n"),
            ],
            &[],
        )
        .unwrap();
        assert!(!graph.known_targets().contains("ignored"));
        assert_eq!(graph.owner_files("root").unwrap().len(), 2);
        assert_eq!(graph.dependencies("root").unwrap().len(), 3);
        let selected = graph.select(&["root".into()], Limits::default()).unwrap();
        assert_eq!(selected.selected_owner_files.len(), 2);
        assert!(selected.reached_targets.contains("left"));
        assert!(selected.missing_endpoints.is_empty());
    }

    #[test]
    fn explicit_continuation_and_bare_target_form_are_bounded() {
        let graph = parse(
            &[(
                "x",
                "#MM build : \\\n#MM dep-a dep-b\n#MM\nbare : ignored-by-mmake\n",
            )],
            &[],
        )
        .unwrap();
        assert_eq!(
            graph.dependencies("build").unwrap(),
            &BTreeSet::from(["dep-a".into(), "dep-b".into()])
        );
        assert_eq!(graph.dependencies("bare").unwrap(), &BTreeSet::new());
        assert_eq!(
            graph.owner_files("bare").unwrap(),
            &BTreeSet::from(["x".into()])
        );
    }

    #[test]
    fn disabled_continuation_matches_reference_last_byte_replacement() {
        let graph = parse(&[("rules", "#MM- root : active \\\n##MM disabled \\\n##M also-disabled \\\n#MM tail\n#MM active\n#MM tail\n")], &[]).unwrap();
        assert_eq!(
            graph.dependencies("root").unwrap(),
            &BTreeSet::from(["active".into(), "tail".into()])
        );
        assert!(graph
            .select(&["root".into()], Limits::default())
            .unwrap()
            .missing_endpoints
            .is_empty());
        let final_byte =
            parse(&[("rules", "#MM- root : active \\\n##MM ends-in-x\n")], &[]).unwrap();
        assert_eq!(
            final_byte.dependencies("root").unwrap(),
            &BTreeSet::from(["active".into(), "x".into()])
        );
    }

    #[test]
    fn matches_private_reference_binary_virtual_chain_continuation_and_real_owner() {
        // This matches the Makefile used in the fresh private reference
        // fixture. The reference mmake run with root selected only the leaf
        // target's makefile (configured maketool `echo`) and its dependency
        // log listed root -> mid -> leaf -> tail.
        let graph = parse(
            &[(
                "src/Makefile",
                "#MM- root : mid\n#MM- mid : \\\n#MM leaf missing\n#MM leaf : tail\n#MM- tail\n",
            )],
            &[],
        )
        .unwrap();
        let selected = graph.select(&["root".into()], Limits::default()).unwrap();
        assert_eq!(
            selected.reached_targets,
            BTreeSet::from(["root".into(), "mid".into(), "leaf".into(), "tail".into()])
        );
        assert_eq!(
            selected.selected_owner_files,
            BTreeSet::from(["src/Makefile".into()])
        );
        assert_eq!(
            selected.missing_endpoints,
            BTreeSet::from(["missing".into()])
        );
    }

    #[test]
    fn configured_cpu_globalvarfile_virtual_chain_continuation_and_bare_marker_match_reference() {
        let graph = parse(
            &[(
                "src/Makefile",
                r"#MM- root-$(CPU) : mid-$(CPU) bare-$(CPU)
#MM- mid-$(CPU) : \
#MM leaf-$(CPU)
#MM leaf-$(CPU) : tail-$(CPU)
#MM- tail-$(CPU)
#MM-
bare-$(CPU) : ignored-prereq
#MM bare-$(CPU) : tail-$(CPU)
",
            )],
            &[("CPU", "arm")],
        )
        .unwrap();
        let selected = graph
            .select(&["root-arm".into()], Limits::default())
            .unwrap();
        assert_eq!(
            selected.reached_targets,
            BTreeSet::from([
                "bare-arm".into(),
                "leaf-arm".into(),
                "mid-arm".into(),
                "root-arm".into(),
                "tail-arm".into(),
            ])
        );
        assert_eq!(
            selected.selected_owner_files,
            BTreeSet::from(["src/Makefile".into()])
        );
        assert!(selected.missing_endpoints.is_empty());
    }

    #[test]
    fn bare_form_requires_single_target_instead_of_dropping_extra_names() {
        assert!(parse(&[("x", "#MM\none two : deps\n")], &[]).is_err());
    }

    #[test]
    fn project_globals_substitute_after_tokenization_once() {
        let graph = parse(
            &[("x", "#MM build-$(CPU) : $(BASE)\n#MM $(BASE)\n")],
            &[("CPU", "arm"), ("BASE", "seed")],
        )
        .unwrap();
        assert!(graph.known_targets().contains("build-arm"));
        let selected = graph
            .select(&["build-arm".into()], Limits::default())
            .unwrap();
        assert_eq!(
            selected.reached_targets,
            BTreeSet::from(["build-arm".into(), "seed".into()])
        );

        let once = parse(&[("x", "#MM $(A)\n")], &[("A", "$(B)"), ("B", "expanded")]).unwrap();
        assert!(once.known_targets().contains("$(B)"));
        assert!(!once.known_targets().contains("expanded"));
    }

    #[test]
    fn unbound_unsupported_and_whitespace_expansions_fail_closed() {
        assert!(parse(&[("x", "#MM $(MISSING)\n")], &[]).is_err());
        assert!(parse(&[("x", "#MM $CPU\n")], &[("CPU", "arm")]).is_err());
        assert!(parse(&[("x", "#MM $(CPU)\n")], &[("CPU", "arm board")]).is_err());
    }

    #[test]
    fn empty_global_component_is_allowed_but_empty_final_identity_is_not() {
        let graph = parse(&[("x", "#MM root-$(EMPTY)\n")], &[("EMPTY", "")]).unwrap();
        assert!(graph.known_targets().contains("root-"));
        assert!(parse(&[("x", "#MM $(EMPTY)\n")], &[("EMPTY", "")]).is_err());
    }

    #[test]
    fn reference_empty_endpoints_are_preserved_but_never_satisfy_selection() {
        let files = file_map(&[(
            "rules",
            "#MM- selected : safe\n#MM safe\n#MM- foreign : $(EMPTY)\n#MM $(EMPTY)\n",
        )]);
        let limits = Limits {
            preserve_empty_endpoints: true,
            ..Limits::default()
        };
        let graph =
            MetaMakeOwnerGraph::parse_expanded_files(&files, &globals(&[("EMPTY", "")]), limits)
                .unwrap();
        assert!(graph.known_targets().contains(""));
        assert!(graph
            .select(&["selected".into()], limits)
            .unwrap()
            .missing_endpoints
            .is_empty());
        assert_eq!(
            graph
                .select(&["foreign".into()], limits)
                .unwrap()
                .missing_endpoints,
            BTreeSet::from([String::new()])
        );
        assert_eq!(
            graph
                .select(&[String::new()], limits)
                .unwrap()
                .missing_endpoints,
            BTreeSet::from([String::new()])
        );
    }

    #[test]
    fn quoted_tokens_are_rejected_instead_of_misparsed() {
        assert!(parse(&[("x", "#MM \"root\"\n")], &[]).is_err());
        assert!(parse(&[("x", "#MM root\"suffix\n")], &[]).is_err());
    }

    #[test]
    fn control_nul_and_non_ascii_identity_bytes_are_rejected() {
        assert!(parse(&[("x", "#MM root\0 : dep\n")], &[]).is_err());
        assert!(parse(&[("x", "#MM root\u{00a0}\n")], &[]).is_err());
        assert!(parse(&[("x", "#MM $(ROOT)\n")], &[("ROOT", "bad\0value")]).is_err());
        assert!(parse(&[("x", "#MM $(BAD\0KEY)\n")], &[("BAD\0KEY", "safe")]).is_err());
    }

    #[test]
    fn target_fanout_is_budgeted_before_dependency_copies() {
        let limits = Limits {
            max_edges: 3,
            ..Limits::default()
        };
        assert!(MetaMakeOwnerGraph::parse_expanded_files(
            &file_map(&[("x", "#MM one two : a b\n")]),
            &globals(&[]),
            limits,
        )
        .is_err());
    }

    #[test]
    fn identity_copy_budget_is_charged_before_fanout_global_merge_and_selection() {
        let parse_limits = Limits {
            max_total_identity_copy_bytes: 10,
            ..Limits::default()
        };
        assert!(MetaMakeOwnerGraph::parse_expanded_files(
            &file_map(&[("x", "#MM one two : a b\n")]),
            &globals(&[]),
            parse_limits,
        )
        .is_err());

        let graph = parse(&[("x", "#MM root : child\n#MM child\n")], &[]).unwrap();
        let selection_limits = Limits {
            max_total_identity_copy_bytes: 5,
            ..Limits::default()
        };
        assert!(graph.select(&["root".into()], selection_limits).is_err());
    }

    #[test]
    fn project_global_input_count_and_bytes_are_bounded() {
        let bindings = globals(&[("A", "one"), ("B", "two")]);
        let limits = Limits {
            max_global_bindings: 1,
            ..Limits::default()
        };
        assert!(MetaMakeOwnerGraph::parse_expanded_files(
            &file_map(&[("x", "#MM root\n")]),
            &bindings,
            limits,
        )
        .is_err());

        let bindings = globals(&[("A", "123456")]);
        let limits = Limits {
            max_global_bytes: 3,
            ..Limits::default()
        };
        assert!(MetaMakeOwnerGraph::parse_expanded_files(
            &file_map(&[("x", "#MM root\n")]),
            &bindings,
            limits,
        )
        .is_err());
    }

    #[test]
    fn bare_virtual_marker_matches_dirnode_hardcoded_real_owner_behavior() {
        let graph = parse(&[("x", "#MM-\nbare\n")], &[]).unwrap();
        let declaration = &graph.declarations()[0];
        assert!(declaration.bare_marker);
        assert!(declaration.virtual_target);
        assert!(declaration.claims_make_owner);
        assert_eq!(declaration.raw_target, "bare");
        assert_eq!(declaration.target, "bare");
        assert_eq!(
            graph.owner_files("bare").unwrap(),
            &BTreeSet::from(["x".into()])
        );
    }

    #[test]
    fn virtual_owner_is_not_selected_but_its_edges_are_followed() {
        let graph = parse(
            &[
                ("virtual", "#MM- alias : concrete\n"),
                ("real", "#MM concrete\n"),
            ],
            &[],
        )
        .unwrap();
        let selected = graph.select(&["alias".into()], Limits::default()).unwrap();
        assert_eq!(
            selected.selected_owner_files,
            BTreeSet::from(["real".into()])
        );
        assert!(selected.reached_targets.contains("alias"));
    }

    #[test]
    fn declaration_provenance_keeps_virtual_real_and_repeated_origins() {
        let graph = parse(
            &[
                ("a/virtual.mk", "#MM- alias-$(CPU) : concrete\n"),
                (
                    "b/real.mk",
                    "#MM alias-arm : concrete\n#MM alias-arm : concrete\n#MM concrete\n",
                ),
            ],
            &[("CPU", "arm")],
        )
        .unwrap();
        let declarations = graph.declarations();
        assert_eq!(declarations.len(), 4);
        assert_eq!(declarations[0].file, "a/virtual.mk");
        assert_eq!(declarations[0].raw_target, "alias-$(CPU)");
        assert_eq!(declarations[0].target, "alias-arm");
        assert!(declarations[0].virtual_target);
        assert!(!declarations[0].claims_make_owner);
        assert_eq!(declarations[0].dependencies[0].raw_expression, "concrete");
        assert_eq!(declarations[0].dependencies[0].concrete, "concrete");

        assert_eq!(declarations[1].file, "b/real.mk");
        assert!(!declarations[1].virtual_target);
        assert!(declarations[1].claims_make_owner);
        assert_eq!(declarations[1].expanded_line, 1);
        assert_eq!(declarations[2].expanded_line, 2);
        let mut repeated = declarations[2].clone();
        repeated.expanded_line = declarations[1].expanded_line;
        assert_eq!(declarations[1], repeated);
        assert_eq!(
            graph.owner_files("alias-arm").unwrap(),
            &BTreeSet::from(["b/real.mk".into()])
        );
    }

    #[test]
    fn declaration_provenance_keeps_empty_selectors_and_unknown_roots_missing() {
        let files = file_map(&[
            (
                "one/Makefile",
                "#MM- root-$(CPU) : child-$(VARIANT) $(VARIANT)\n",
            ),
            ("two/Makefile", "#MM $(VARIANT)\n"),
        ]);
        let globals = globals(&[("CPU", "arm"), ("VARIANT", "")]);
        let limits = Limits {
            preserve_empty_endpoints: true,
            ..Limits::default()
        };
        let graph = MetaMakeOwnerGraph::parse_expanded_files(&files, &globals, limits).unwrap();
        let declarations = graph.declarations();
        assert_eq!(declarations.len(), 2);
        assert_eq!(declarations[0].file, "one/Makefile");
        assert_eq!(declarations[0].raw_target, "root-$(CPU)");
        assert_eq!(declarations[0].target, "root-arm");
        assert_eq!(
            declarations[0].dependencies[0].raw_expression,
            "child-$(VARIANT)"
        );
        assert_eq!(declarations[0].dependencies[0].concrete, "child-");
        assert_eq!(declarations[0].dependencies[1].raw_expression, "$(VARIANT)");
        assert_eq!(declarations[0].dependencies[1].concrete, "");
        assert_eq!(declarations[1].raw_target, "$(VARIANT)");
        assert_eq!(declarations[1].target, "");

        let unknown = graph.select(&["not-declared".into()], limits).unwrap();
        assert_eq!(
            unknown.missing_endpoints,
            BTreeSet::from(["not-declared".into()])
        );
        assert!(unknown.selected_owner_files.is_empty());
        let empty_root = graph.select(&[String::new()], limits).unwrap();
        assert_eq!(
            empty_root.missing_endpoints,
            BTreeSet::from([String::new()])
        );
    }

    #[test]
    fn repeated_provenance_edges_are_bounded_by_edge_limit() {
        let limits = Limits {
            max_edges: 1,
            ..Limits::default()
        };
        let result = MetaMakeOwnerGraph::parse_expanded_files(
            &file_map(&[("rules", "#MM root : dep dep\n")]),
            &BTreeMap::new(),
            limits,
        );
        assert!(result.unwrap_err().contains("provenance edge limit"));
    }

    #[test]
    fn missing_root_and_dependency_endpoints_are_reported() {
        let graph = parse(&[("x", "#MM root : absent\n")], &[]).unwrap();
        let selected = graph
            .select(&["root".into(), "unknown-root".into()], Limits::default())
            .unwrap();
        assert_eq!(
            selected.missing_endpoints,
            BTreeSet::from(["absent".into(), "unknown-root".into()])
        );
    }

    #[test]
    fn malformed_virtual_marker_and_unterminated_continuation_reject() {
        assert!(parse(&[("x", "#MM -virtual\n")], &[]).is_err());
        assert!(parse(&[("x", "#MM root : \\")], &[]).is_err());
    }

    #[test]
    fn resource_limits_are_enforced() {
        let limits = Limits {
            max_work: 1,
            ..Limits::default()
        };
        assert!(MetaMakeOwnerGraph::parse_expanded_files(
            &file_map(&[("x", "#MM root\n")]),
            &globals(&[]),
            limits,
        )
        .is_err());
    }
}
