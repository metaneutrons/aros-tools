//! Bounded, source-local ownership evidence for rejected Make outputs.
//!
//! This is diagnostic attribution only. It does not create producers,
//! dependencies, or capabilities. The graph follows exact ordinary Make and
//! `#MM` prerequisite identities, plus output identities from a small set of
//! native macros whose definitions are verified by SHA-256 before use.

use crate::dirs::DirVars;
use crate::make_expr::{evaluate_make_expr, evaluate_make_list, MakeExprContext};
use crate::make_vars::{strip_make_comment, variable_assignment, ConditionalTruth, VarScope};
use crate::parser::macro_invocations;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_SNAPSHOT_BYTES: usize = 2 * 1024 * 1024;
const MAX_TEMPLATE_BYTES: usize = 2 * 1024 * 1024;
const MAX_LINES: usize = 16_384;
const MAX_IDENTITIES: usize = 65_536;
const MAX_PATTERN_MATCHES: usize = 65_536;
const MAX_DIAGNOSTIC_WORK: usize = 65_536;
const MAX_DIAGNOSTIC_PATH_BYTES: usize = 16 * 1024 * 1024;
const MAX_MACRO_OUTPUT_BYTES: usize = MAX_SNAPSHOT_BYTES;
const MAX_MACRO_ARGUMENT_BYTES: usize = 64 * 1024;
const MAX_MACRO_ARGUMENTS: usize = 256;
const MAX_TEMPLATE_CLOSURE_DEPTH: usize = 64;
const MAX_TEMPLATE_CLOSURE_WORK: usize = 8192;
const MAX_TEMPLATE_CLOSURE_DEFINITIONS: usize = 256;

const BUILD_PROG_SHA256: &str = "e276e953080db3a15e844057a5d2454c0c537da800904c9544c0c106566d375e";
const COMPILE_MULTI_SHA256: &str =
    "a55875ba53ff3445e8083c15f07348e7c294cf08c42235b142caf6af9ee50bc7";
const ASSEMBLE_MULTI_SHA256: &str =
    "b8718f311b850bb70a5c20a9ab5c703ee9e96858d15956b255bc065e7affeb36";
const LINK_BINARY_SHA256: &str = "5d2fade4d4af278786918729526950b2d010e06760fe682671de001542fbc120";
const ADD_COMPILERLINKFLAGS_SHA256: &str =
    "e7cf94a8764675c98d39f614d10339187821b7f535740b8a8fcf138191139163";
const ASSEMBLE_Q_SHA256: &str = "46dff6d25108327406ecb87f8cdf807aeefc6fcae10a75e5a6af5fd2766912ea";
const COMPILE_Q_SHA256: &str = "3412141396c0a321e8b14d632a412b7563981564d5bdeabe957fd67c788dd0b2";
const FILEACTIONMSG_SHA256: &str =
    "f812409a32e02d8f6535b8d4708de68aefde558396e47f6e304a91af79263cff";
const GEN_ARCHSPECIFICRULES_SHA256: &str =
    "fc50253aa57b06aac5fdd739e2e871391d4e9a78d84989f83e52af71b162a062";
const INCLUDE_DEPS_SHA256: &str =
    "8c5f632ffd7bb7ced511ee6441c5a1c272b382ccd1a4ed58c5391ae7febdee82";
const LINK_Q_SHA256: &str = "3b10e411a1974ff5c0b29b038e9db3a916826ce4abf8b682d17d7086bc3af7d4";
const MKDEPEND_Q_SHA256: &str = "3e6afefd7813acc49f0c80df32b99f9ae3e0976420f04ffbf6be7872d2ff1c66";
const MKDIR_Q_SHA256: &str = "9c2abcb165988c8032728270f471abf11fd80b3ac2b3c64841b2b8ff8126c5f9";
const RULE_COMPILE_CXX_MULTI_SHA256: &str =
    "6ae587679b601c38f074aa2e23c4b63ba859f0ca6d62a0b68b5212767566371c";
const RULE_COMPILE_OBJC_MULTI_SHA256: &str =
    "afad565befd2a2f9b9d6118c54711f9400538857f2855a00f648ac5668edd446";
const RULE_LINK_PROG_SHA256: &str =
    "2abb2a483e55353cbfad1397bd24fcf957e9c937a501e45b2c6124a62f25a3c6";
const RULE_MAKEDIRS_SHA256: &str =
    "8e010ec89890a894073dc26eb68ecf68a56e27faa4e69f4f6c17696098e12414";
const STRIP_Q_SHA256: &str = "94bf874bbdf35a70dc73feaba1f3f562126f27c27a47bf46bf81642cbc8f58f3";
const TRUSTED_TEMPLATE_CLOSURE_REFERENCES: &[&str] = &[
    "add_compilerlinkflags",
    "assemble_q",
    "compile_q",
    "fileactionmsg",
    "gen_archspecificrules",
    "include_deps",
    "link_q",
    "mkdepend_q",
    "mkdir_q",
    "rule_assemble_multi",
    "rule_compile_cxx_multi",
    "rule_compile_multi",
    "rule_compile_objc_multi",
    "rule_link_prog",
    "rule_makedirs",
    "strip_q",
];

/// A source-local owner reached from one rejected output identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRuleOwnership {
    /// Exact owner endpoint from a source `#MM` declaration or verified macro.
    pub owner: String,
    /// Exact source-consumer path supporting this owner attribution. For a
    /// paired compile sidecar, this is the associated object's path; no graph
    /// edge between the sidecar and object is implied.
    pub chain: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
enum MacroForm {
    BuildProg,
    CompileMulti,
    AssembleMulti,
    LinkBinary,
}

/// Finite subset of the four independently hash-verified macro forms.
#[derive(Debug, Clone, Copy, Default)]
struct VerifiedMacros([bool; 4]);

impl VerifiedMacros {
    const fn allows(self, form: MacroForm) -> bool {
        self.0[form as usize]
    }

    #[cfg(test)]
    fn from_forms(forms: &[MacroForm]) -> Self {
        let mut verified = Self::default();
        for form in forms {
            verified.0[*form as usize] = true;
        }
        verified
    }
}

#[derive(Debug, Default)]
struct SourceGraph {
    /// Unique identity vertices shared by ordinary rules and verified macro
    /// seeds. Checked before insertion so repeated expansions cannot multiply
    /// the graph without limit.
    identities: BTreeSet<String>,
    /// Retained endpoint occurrences across source rules and macro seeds.
    /// This also bounds line-to-output vectors when repeated variables expand
    /// to the same finite names.
    identity_references: usize,
    /// Aggregate byte budget for retained verified macro output identities.
    macro_output_bytes: usize,
    /// Prerequisite identity -> consumer target identities.
    consumers: BTreeMap<String, BTreeSet<String>>,
    /// Concrete endpoints which the source declares as MetaMake owners.
    owners: BTreeSet<String>,
    /// Producer output identity -> exact `%mmake` owner endpoint(s).
    macro_owners: BTreeMap<String, BTreeSet<String>>,
    /// Concrete ordinary-Make identities eligible to match one-stem rules.
    /// MetaMake owner labels are deliberately excluded unless the same value
    /// also occurs as an ordinary target or prerequisite.
    make_identities: BTreeSet<String>,
    /// Finite outputs of verified compile/assemble macros. The default
    /// `mmake=TMP` is a variable namespace, not a MetaMake owner.
    macro_outputs: BTreeSet<String>,
    /// Macro output identities emitted more than once by verified producers.
    /// These remain diagnostic ambiguity rather than graph edges.
    ambiguous_macro_outputs: BTreeSet<String>,
    /// Exact per-invocation `%rule_compile_multi` output pairs. This is
    /// diagnostic metadata only; it never enters `consumers` or providers.
    compile_multi_groups: Vec<CompileMultiGroup>,
    /// Bounded, one-stem Make pattern rules, instantiated only against finite
    /// identities already present in this source graph.
    patterns: Vec<PatternRule>,
    /// Ordinary rule targets at each joined snapshot line, for checking that a
    /// supplied rejection line really names its declared output.
    targets_by_line: BTreeMap<usize, Vec<String>>,
    /// Ordinary rules with a source recipe. Paired output fallback applies
    /// only to a declaration that adds no separate recipe producer.
    recipe_rule_lines: BTreeSet<usize>,
    /// Lines inside Make `define` bodies. Their contents are inert at parse
    /// time and must not seed owners or macro outputs.
    definition_lines: BTreeSet<usize>,
    edge_count: usize,
    /// Some source syntax could declare an additional consumer or owner but
    /// was conditional, malformed, or not resolvable by this bounded parser.
    uncertain: bool,
    overflow: bool,
}

#[derive(Debug)]
struct PatternRule {
    target: String,
    prerequisites: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CompileMultiPair {
    object: String,
    depfile: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CompileMultiGroup {
    invocation_line: usize,
    pairs: Vec<CompileMultiPair>,
}

#[derive(Debug, Default)]
struct DiagnosticBudget {
    work: usize,
    retained_path_bytes: usize,
}

impl DiagnosticBudget {
    const fn charge_work(&mut self, amount: usize) -> bool {
        let Some(total) = self.work.checked_add(amount) else {
            return false;
        };
        if total > MAX_DIAGNOSTIC_WORK {
            return false;
        }
        self.work = total;
        true
    }

    const fn charge_path_bytes(&mut self, amount: usize) -> bool {
        let Some(total) = self.retained_path_bytes.checked_add(amount) else {
            return false;
        };
        if total > MAX_DIAGNOSTIC_PATH_BYTES {
            return false;
        }
        self.retained_path_bytes = total;
        true
    }
}

type ClosedMacroArguments = BTreeMap<String, String>;
type MacroContract = BTreeMap<String, MacroArgumentSpec>;

#[derive(Debug)]
struct NativeMacroDefinition {
    sha256: String,
    body_lines: Vec<String>,
}

type NativeMacroDefinitions = BTreeMap<String, Vec<NativeMacroDefinition>>;
type NativeMacroFileParts = (Vec<(String, NativeMacroDefinition)>, Vec<PathBuf>);

#[derive(Debug, Clone, PartialEq, Eq)]
struct MacroArgumentSpec {
    default: Option<String>,
    required: bool,
}

/// Attributes the unique rejected Make target declared at `rejected_rule_line`.
///
/// This form is useful for a collector rejection that records the exact
/// physical joined-snapshot line but cannot resolve the target value itself.
/// It only uses the target identity independently resolved from that same rule;
/// unresolved variable-bearing target names do not become graph vertices.
#[must_use]
#[cfg(test)]
pub fn attribute_rejected_rule_line(
    snapshot: &str,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
    line_states: Option<&[ConditionalTruth]>,
    rejected_rule_line: usize,
) -> Option<SourceRuleOwnership> {
    let owners = attribute_rejected_rule_owners(
        snapshot,
        scope,
        dirs,
        root,
        rel_dir,
        line_states,
        rejected_rule_line,
    )?;
    (owners.len() == 1)
        .then(|| owners.into_iter().next())
        .flatten()
}

/// Attributes every fully proven source owner reachable from a rejected rule.
///
/// A multi-target line is accepted only if every exact target has at least one
/// completely proven owner. Unknown/conditional statements and unsupported
/// pattern forms still veto attribution for the complete snapshot.
#[must_use]
pub fn attribute_rejected_rule_owners(
    snapshot: &str,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
    line_states: Option<&[ConditionalTruth]>,
    rejected_rule_line: usize,
) -> Option<Vec<SourceRuleOwnership>> {
    let macros = verify_native_macros(root);
    let states = line_states?;
    if snapshot.len() > MAX_SNAPSHOT_BYTES || rejected_rule_line == 0 {
        return None;
    }
    let lines = snapshot.lines().collect::<Vec<_>>();
    if lines.len() > MAX_LINES || states.len() < lines.len() {
        return None;
    }
    let rejected_line = rejected_rule_line.checked_sub(1)?;
    let mut graph = parse_source_graph(&lines, scope, dirs, root, rel_dir, states);
    let outputs = graph.targets_by_line.get(&rejected_line)?.clone();
    if outputs.is_empty() {
        return None;
    }
    add_verified_macro_edges(
        &mut graph,
        &lines,
        scope,
        dirs,
        (root, rel_dir),
        states,
        macros,
    );
    instantiate_pattern_edges(&mut graph);
    if graph.overflow || graph.uncertain {
        return None;
    }
    attribute_graph_outputs(&graph, rejected_line, &outputs)
}

fn attribute_graph_outputs(
    graph: &SourceGraph,
    rejected_rule_line: usize,
    outputs: &[String],
) -> Option<Vec<SourceRuleOwnership>> {
    let mut budget = DiagnosticBudget::default();
    trace_all_then_paired(graph, outputs, rejected_rule_line, &mut budget)
}

fn trace_all_then_paired(
    graph: &SourceGraph,
    outputs: &[String],
    rejected_rule_line: usize,
    budget: &mut DiagnosticBudget,
) -> Option<Vec<SourceRuleOwnership>> {
    trace_all_targets(graph, outputs, budget)
        .or_else(|| trace_paired_compile_outputs(graph, outputs, rejected_rule_line, budget))
}

fn trace_all_targets(
    graph: &SourceGraph,
    outputs: &[String],
    budget: &mut DiagnosticBudget,
) -> Option<Vec<SourceRuleOwnership>> {
    let mut owners = BTreeMap::<String, SourceRuleOwnership>::new();
    for output in outputs {
        for proof in trace_all_owners(graph, output, budget)? {
            merge_ownership(&mut owners, proof, budget)?;
        }
    }
    (!owners.is_empty()).then(|| owners.into_values().collect())
}

fn merge_ownership(
    owners: &mut BTreeMap<String, SourceRuleOwnership>,
    proof: SourceRuleOwnership,
    budget: &mut DiagnosticBudget,
) -> Option<()> {
    if owners.contains_key(&proof.owner) {
        return Some(());
    }
    let key_bytes = std::mem::size_of::<String>().checked_add(proof.owner.len())?;
    if !budget.charge_path_bytes(key_bytes) {
        return None;
    }
    let owner_key = clone_path_string(&proof.owner)?;
    owners.insert(owner_key, proof);
    Some(())
}

#[cfg(test)]
fn attribute_with_verified_macros(
    snapshot: &str,
    scope: &VarScope,
    dirs: &DirVars,
    source_dirs: (&Path, &Path),
    line_states: Option<&[ConditionalTruth]>,
    rejected_rule: (&str, usize),
    macros: VerifiedMacros,
) -> Option<SourceRuleOwnership> {
    let (root, rel_dir) = source_dirs;
    let (output, rejected_rule_line) = rejected_rule;
    if snapshot.len() > MAX_SNAPSHOT_BYTES || output.is_empty() || rejected_rule_line == 0 {
        return None;
    }
    let states = line_states?;
    let lines = snapshot.lines().collect::<Vec<_>>();
    if lines.len() > MAX_LINES || states.len() < lines.len() {
        return None;
    }
    let rejected_line = rejected_rule_line.checked_sub(1)?;
    let mut graph = parse_source_graph(&lines, scope, dirs, root, rel_dir, states);
    let targets = graph.targets_by_line.get(&rejected_line)?;
    if targets.len() != 1 || targets[0] != output {
        return None;
    }
    add_verified_macro_edges(&mut graph, &lines, scope, dirs, source_dirs, states, macros);
    instantiate_pattern_edges(&mut graph);
    if graph.overflow || graph.uncertain {
        return None;
    }
    let owners = trace_all_owners(&graph, output, &mut DiagnosticBudget::default())?;
    (owners.len() == 1)
        .then(|| owners.into_iter().next())
        .flatten()
}

fn verify_native_macros(root: &Path) -> VerifiedMacros {
    let Some((template, definitions)) = read_native_template_set(root) else {
        return VerifiedMacros::default();
    };
    VerifiedMacros(std::array::from_fn(|index| {
        let Some((name, expected_sha256)) = macro_contract_identity(index) else {
            return false;
        };
        verified_macro_contract(&template, name, expected_sha256).is_some()
            && verified_macro_closure(name, &definitions)
    }))
}

#[cfg(test)]
fn verified_macro(template: &str, name: &str, expected_sha256: &str) -> bool {
    verified_macro_contract(template, name, expected_sha256).is_some()
}

fn read_native_make_template(root: &Path) -> Option<String> {
    let path = root.join("config/make.tmpl");
    // This is diagnostic attribution, but the hash-bound source read must
    // still be bounded and must not accept a symlink in place of the template.
    let metadata = fs::symlink_metadata(&path).ok()?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_TEMPLATE_BYTES as u64 {
        return None;
    }
    let file = fs::File::open(path).ok()?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    if file
        .take(MAX_TEMPLATE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() > MAX_TEMPLATE_BYTES
    {
        return None;
    }
    String::from_utf8(bytes)
        .ok()
        .map(|template| template.replace("\r\n", "\n"))
}

fn read_native_template_set(root: &Path) -> Option<(String, NativeMacroDefinitions)> {
    let main_template = read_native_make_template(root)?;
    let mut queued = VecDeque::from([Path::new("make.tmpl").to_path_buf()]);
    let mut visited_files = BTreeSet::new();
    let mut definitions = NativeMacroDefinitions::new();
    let mut total_bytes = 0usize;
    let mut total_lines = 0usize;

    while let Some(relative_path) = queued.pop_front() {
        if !visited_files.insert(relative_path.clone()) {
            continue;
        }
        if visited_files.len() > 32 {
            return None;
        }
        let template = if relative_path == Path::new("make.tmpl") {
            main_template.clone()
        } else {
            read_included_native_template(root, &relative_path)?
        };
        total_bytes = total_bytes.checked_add(template.len())?;
        total_lines = total_lines.checked_add(template.lines().count())?;
        if total_bytes > MAX_TEMPLATE_BYTES || total_lines > MAX_LINES {
            return None;
        }

        let (file_definitions, includes) = parse_native_macro_file(&template)?;
        for (name, definition) in file_definitions {
            definitions.entry(name).or_default().push(definition);
            if definitions.len() > MAX_TEMPLATE_CLOSURE_DEFINITIONS {
                return None;
            }
        }
        for include in includes {
            let parent = relative_path.parent().unwrap_or_else(|| Path::new(""));
            let include_path = parent.join(include);
            if !safe_template_relative_path(&include_path) {
                return None;
            }
            queued.push_back(include_path);
        }
    }

    Some((main_template, definitions))
}

fn read_included_native_template(root: &Path, relative_path: &Path) -> Option<String> {
    if !safe_template_relative_path(relative_path) {
        return None;
    }
    let config = root.join("config");
    let mut path = config.clone();
    for component in relative_path.components() {
        let std::path::Component::Normal(part) = component else {
            return None;
        };
        path.push(part);
        let metadata = fs::symlink_metadata(&path).ok()?;
        let is_final = path == config.join(relative_path);
        if is_final {
            if !metadata.file_type().is_file() || metadata.len() > MAX_TEMPLATE_BYTES as u64 {
                return None;
            }
        } else if !metadata.file_type().is_dir() {
            return None;
        }
    }

    let metadata = fs::symlink_metadata(&path).ok()?;
    let file = fs::File::open(path).ok()?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    if file
        .take(MAX_TEMPLATE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() > MAX_TEMPLATE_BYTES
    {
        return None;
    }
    String::from_utf8(bytes)
        .ok()
        .map(|template| template.replace("\r\n", "\n"))
}

fn safe_template_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
        && path.to_str().is_some_and(|value| {
            value.len() <= 256
                && path
                    .extension()
                    .is_some_and(|extension| extension == "tmpl")
                && !value.contains('\\')
        })
}

fn parse_native_macro_file(template: &str) -> Option<NativeMacroFileParts> {
    let lines = template.lines().collect::<Vec<_>>();
    if lines.len() > MAX_LINES {
        return None;
    }
    let mut definitions = Vec::new();
    let mut includes = Vec::new();
    let mut cursor = 0usize;
    while cursor < lines.len() {
        let raw = lines[cursor];
        if let Some(name) = native_define_name(raw) {
            let start = cursor;
            let name = name.to_owned();
            if !valid_genmf_name(&name) {
                return None;
            }
            let mut header_end = cursor;
            while lines[header_end].ends_with('\\') {
                header_end = header_end.checked_add(1)?;
                if header_end >= lines.len() || header_end - start > MAX_LINES {
                    return None;
                }
            }
            let end = lines[header_end + 1..]
                .iter()
                .position(|line| line.starts_with("%end"))?
                + header_end
                + 1;
            let body = lines[start..=end].join("\n");
            let body_lines = lines[header_end + 1..end]
                .iter()
                .map(|line| (*line).to_owned())
                .collect();
            definitions.push((
                name,
                NativeMacroDefinition {
                    sha256: aros_common::sha256_bytes(body.as_bytes()).to_string(),
                    body_lines,
                },
            ));
            cursor = end.checked_add(1)?;
            continue;
        }
        if let Some(include) = raw.strip_prefix("%include") {
            if include.chars().next().is_some_and(char::is_whitespace) {
                let include = include.trim();
                let include =
                    if include.starts_with('"') && include.ends_with('"') && include.len() >= 2 {
                        &include[1..include.len() - 1]
                    } else {
                        include
                    };
                if include.is_empty() || include.len() > 256 || include.contains(['$', '\\']) {
                    return None;
                }
                includes.push(PathBuf::from(include));
            }
        }
        cursor += 1;
    }
    Some((definitions, includes))
}

fn valid_genmf_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn native_define_name(line: &str) -> Option<&str> {
    let tail = line.strip_prefix("%define")?;
    if !tail.chars().next().is_some_and(char::is_whitespace) {
        return None;
    }
    tail.split_whitespace().next()
}

fn verified_macro_closure(root_name: &str, definitions: &NativeMacroDefinitions) -> bool {
    let mut verifier = MacroClosureVerifier::new(definitions, |name| {
        trusted_template_hash(name).map(str::to_owned)
    });
    verifier.visit(root_name, 0)
}

#[cfg(test)]
fn verified_macro_closure_with_hashes(
    root_name: &str,
    definitions: &NativeMacroDefinitions,
    trusted_hash: impl Fn(&str) -> Option<String>,
) -> bool {
    let mut verifier = MacroClosureVerifier::new(definitions, trusted_hash);
    verifier.visit(root_name, 0)
}

struct MacroClosureVerifier<'a, F> {
    definitions: &'a NativeMacroDefinitions,
    template_names: BTreeSet<String>,
    visited: BTreeSet<String>,
    active: BTreeSet<String>,
    work: usize,
    trusted_hash: F,
}

impl<'a, F> MacroClosureVerifier<'a, F>
where
    F: Fn(&str) -> Option<String>,
{
    fn new(definitions: &'a NativeMacroDefinitions, trusted_hash: F) -> Self {
        let mut template_names = definitions.keys().cloned().collect::<BTreeSet<_>>();
        template_names.extend(
            TRUSTED_TEMPLATE_CLOSURE_REFERENCES
                .iter()
                .map(|name| (*name).to_owned()),
        );
        Self {
            definitions,
            template_names,
            visited: BTreeSet::new(),
            active: BTreeSet::new(),
            work: 0,
            trusted_hash,
        }
    }

    fn visit(&mut self, name: &str, depth: usize) -> bool {
        if depth > MAX_TEMPLATE_CLOSURE_DEPTH || self.active.contains(name) {
            return false;
        }
        if self.visited.contains(name) {
            return true;
        }
        self.work = self.work.saturating_add(1);
        if self.work > MAX_TEMPLATE_CLOSURE_WORK {
            return false;
        }
        let Some(matches) = self.definitions.get(name) else {
            return false;
        };
        if matches.len() != 1 {
            return false;
        }
        let definition = &matches[0];
        if (self.trusted_hash)(name).as_deref() != Some(definition.sha256.as_str()) {
            return false;
        }
        // Copy the bounded body lines before recursive visits so mutable
        // traversal state does not overlap a borrow of the definition map.
        let body_lines = definition.body_lines.clone();

        self.active.insert(name.to_owned());
        for line in &body_lines {
            self.work = self.work.saturating_add(1);
            if self.work > MAX_TEMPLATE_CLOSURE_WORK {
                self.active.remove(name);
                return false;
            }
            let Some((reference, _)) =
                first_genmf_template_reference(line, Some(&self.template_names))
            else {
                continue;
            };
            if !self.visit(reference, depth + 1) {
                self.active.remove(name);
                return false;
            }
        }
        self.active.remove(name);
        self.visited.insert(name.to_owned());
        true
    }
}

fn first_genmf_template_reference<'a>(
    line: &'a str,
    template_names: Option<&BTreeSet<String>>,
) -> Option<(&'a str, usize)> {
    if line.is_empty() || line.as_bytes()[0] == b'#' {
        return None;
    }
    let bytes = line.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != b'%' || !bytes.get(index + 1).is_some_and(u8::is_ascii_alphanumeric) {
            index += 1;
            continue;
        }
        let start = index + 1;
        let mut end = start + 1;
        while bytes
            .get(end)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        {
            end += 1;
        }
        if line
            .get(end..)?
            .chars()
            .next()
            .is_some_and(|character| !character.is_whitespace())
        {
            index = end;
            continue;
        }
        if index > 0 && bytes[index - 1] == b'#' {
            return None;
        }
        let name = line.get(start..end)?;
        return template_names
            .is_none_or(|names| names.contains(name))
            .then_some((name, index));
    }
    None
}

fn has_inline_genmf_template_reference(
    line: &str,
    template_names: Option<&BTreeSet<String>>,
) -> bool {
    let Some((_, start)) = first_genmf_template_reference(line, template_names) else {
        return false;
    };
    let first_non_whitespace = line.len().saturating_sub(line.trim_start().len());
    start > first_non_whitespace
}

fn native_template_names(root: &Path) -> Option<BTreeSet<String>> {
    if let Some((_, definitions)) = read_native_template_set(root) {
        return Some(definitions.into_keys().collect());
    }

    #[cfg(test)]
    if fs::symlink_metadata(root.join("config/make.tmpl")).is_err() {
        return Some(
            [
                "build_prog",
                "rule_assemble_multi",
                "rule_compile",
                "rule_compile_multi",
                "rule_link_binary",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        );
    }
    None
}

fn trusted_template_hash(name: &str) -> Option<&'static str> {
    Some(match name {
        "add_compilerlinkflags" => ADD_COMPILERLINKFLAGS_SHA256,
        "assemble_q" => ASSEMBLE_Q_SHA256,
        "build_prog" => BUILD_PROG_SHA256,
        "compile_q" => COMPILE_Q_SHA256,
        "fileactionmsg" => FILEACTIONMSG_SHA256,
        "gen_archspecificrules" => GEN_ARCHSPECIFICRULES_SHA256,
        "include_deps" => INCLUDE_DEPS_SHA256,
        "link_q" => LINK_Q_SHA256,
        "mkdepend_q" => MKDEPEND_Q_SHA256,
        "mkdir_q" => MKDIR_Q_SHA256,
        "rule_assemble_multi" => ASSEMBLE_MULTI_SHA256,
        "rule_compile_cxx_multi" => RULE_COMPILE_CXX_MULTI_SHA256,
        "rule_compile_multi" => COMPILE_MULTI_SHA256,
        "rule_compile_objc_multi" => RULE_COMPILE_OBJC_MULTI_SHA256,
        "rule_link_binary" => LINK_BINARY_SHA256,
        "rule_link_prog" => RULE_LINK_PROG_SHA256,
        "rule_makedirs" => RULE_MAKEDIRS_SHA256,
        "strip_q" => STRIP_Q_SHA256,
        _ => return None,
    })
}

fn verified_macro_contract(
    template: &str,
    name: &str,
    expected_sha256: &str,
) -> Option<MacroContract> {
    let lines = template.lines().collect::<Vec<_>>();
    if lines.len() > MAX_LINES {
        return None;
    }
    let mut matches = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| native_define_name(line) == Some(name));
    let (start, _) = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    let end = lines[start + 1..]
        .iter()
        .position(|line| line.starts_with("%end"))?
        + start
        + 1;
    let body = lines[start..=end].join("\n");
    if aros_common::sha256_bytes(body.as_bytes()).as_str() != expected_sha256 {
        return None;
    }
    parse_macro_header(&lines, start, name)
}

fn parse_macro_header(lines: &[&str], start: usize, expected_name: &str) -> Option<MacroContract> {
    let mut header = String::new();
    let mut cursor = start;
    loop {
        let line = lines.get(cursor)?.trim_end();
        let continued = line.ends_with('\\');
        let segment = if continued {
            line.strip_suffix('\\')?
        } else {
            line
        };
        if !header.is_empty() {
            header.push(' ');
        }
        header.push_str(segment.trim());
        if !continued {
            break;
        }
        cursor = cursor.checked_add(1)?;
    }

    let rest = header.strip_prefix("%define")?;
    if !rest.chars().next().is_some_and(char::is_whitespace) {
        return None;
    }
    let rest = rest.trim_start();
    let (name, arguments) = rest.split_once(char::is_whitespace)?;
    if name != expected_name || arguments.len() > MAX_MACRO_ARGUMENT_BYTES {
        return None;
    }
    parse_macro_argument_specs(arguments.trim())
}

fn parse_macro_argument_specs(raw: &str) -> Option<MacroContract> {
    let bytes = raw.as_bytes();
    let mut cursor = 0usize;
    let mut specs = MacroContract::new();
    while cursor < bytes.len() {
        while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        if cursor == bytes.len() {
            break;
        }
        let key_start = cursor;
        if !bytes.get(cursor).is_some_and(u8::is_ascii_alphanumeric) {
            return None;
        }
        cursor += 1;
        while bytes
            .get(cursor)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        {
            cursor += 1;
        }
        if cursor - key_start > 64 || bytes.get(cursor) != Some(&b'=') {
            return None;
        }
        let key = std::str::from_utf8(&bytes[key_start..cursor])
            .ok()?
            .to_owned();
        cursor += 1;

        let (mut default, end) = if bytes.get(cursor) == Some(&b'"') {
            let (value, end) = genmf_argument_value(raw, cursor)?;
            (Some(value), end)
        } else {
            let start = cursor;
            while cursor < bytes.len() && !bytes[cursor].is_ascii_whitespace() {
                if bytes[cursor] == b'"' {
                    return None;
                }
                cursor += 1;
            }
            let value = if cursor > start {
                Some(raw.get(start..cursor)?.to_owned())
            } else {
                None
            };
            (value, cursor)
        };
        cursor = end;

        let mut required = false;
        if let Some(value) = default.as_mut() {
            if value.ends_with("/A") {
                value.truncate(value.len() - 2);
                required = true;
            } else if value.ends_with("/M") {
                // These four source graph macros have no GenMF multiarg field;
                // do not approximate its remainder-capture semantics.
                return None;
            }
        }
        if specs
            .insert(key, MacroArgumentSpec { default, required })
            .is_some()
            || specs.len() > MAX_MACRO_ARGUMENTS
        {
            return None;
        }
    }
    Some(specs)
}

fn effective_macro_arguments(
    raw: &str,
    contract: &MacroContract,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
    line: usize,
) -> Option<ClosedMacroArguments> {
    let supplied = closed_macro_arguments(raw)?;
    if supplied.keys().any(|key| !contract.contains_key(key))
        || contract
            .iter()
            .any(|(key, spec)| spec.required && !supplied.contains_key(key))
    {
        return None;
    }

    let effective = contract
        .iter()
        .map(|(key, spec)| {
            (
                key.clone(),
                supplied
                    .get(key)
                    .cloned()
                    .or_else(|| spec.default.clone())
                    .unwrap_or_default(),
            )
        })
        .collect::<ClosedMacroArguments>();
    let context = MakeExprContext::new(scope, dirs, line, root, rel_dir);
    for value in effective.values() {
        evaluate_make_expr(value, &context).ok()?;
    }
    Some(effective)
}

const fn macro_contract_identity(index: usize) -> Option<(&'static str, &'static str)> {
    match index {
        0 => Some(("build_prog", BUILD_PROG_SHA256)),
        1 => Some(("rule_compile_multi", COMPILE_MULTI_SHA256)),
        2 => Some(("rule_assemble_multi", ASSEMBLE_MULTI_SHA256)),
        3 => Some(("rule_link_binary", LINK_BINARY_SHA256)),
        _ => None,
    }
}

fn verified_macro_contracts(root: &Path, macros: VerifiedMacros) -> [Option<MacroContract>; 4] {
    if let Some(template) = read_native_make_template(root) {
        return std::array::from_fn(|index| {
            if !macros.0[index] {
                return None;
            }
            let (name, hash) = macro_contract_identity(index)?;
            verified_macro_contract(&template, name, hash)
        });
    }

    #[cfg(test)]
    if fs::symlink_metadata(root.join("config/make.tmpl")).is_err() {
        return std::array::from_fn(|index| {
            if !macros.0[index] {
                return None;
            }
            test_macro_contract(index)
        });
    }

    std::array::from_fn(|_| None)
}

#[cfg(test)]
fn test_macro_header(index: usize) -> Option<&'static str> {
    const HEADERS: [&str; 4] = [
        "%define build_prog mmake=/A progname=/A \\\n            files= objcfiles= cxxfiles= alwayscxxlink=no \\\n            asmfiles= objs= objdir=\"$(GENDIR)/$(CURDIR)\" targetdir=\"$(AROSDIR)/$(CURDIR)\" \\\n            cppflags=\"$(CPPFLAGS)\" cflags= dflags= cxxflags= dxxflags= ldflags= \\\n            aflags=\"$(AFLAGS)\" uselibs= usehostlibs= usestartup=yes detach=no nix=no \\\n            includedir= libdir= usetree=no \\\n            compiler=target linker= \\\n            coverageinstr=\"$(TARGET_COVERAGEINSTR)\" funcinstr=\"$(TARGET_FUNCINSTR)\" lto=\"$(TARGET_LTO)\"",
        "%define rule_compile_multi mmake=TMP basenames=/A cppflags=$(CPPFLAGS) cflags=$(CFLAGS) dflags= srcdir= targetdir= \\\n            compiler=target usetree=no incextra=\"$(TOP)/$(CURDIR)\"",
        "%define rule_assemble_multi mmake=TMP cmd=\"$(strip $(CC) $(TARGET_SYSROOT))\"  basenames=/A cppflags=$(CPPFLAGS) aflags=$(AFLAGS) targetdir= suffix=.s",
        "%define rule_link_binary mmake=BD file=/A name=/A objs= files= asmfiles= start=0 ldflags=",
    ];
    HEADERS.get(index).copied()
}

#[cfg(test)]
fn test_macro_contract(index: usize) -> Option<MacroContract> {
    // The synthetic headers mirror the hash-bound config/make.tmpl headers.
    let header = test_macro_header(index)?;
    let lines = header.lines().collect::<Vec<_>>();
    parse_macro_header(&lines, 0, macro_contract_identity(index)?.0)
}

fn parse_source_graph(
    lines: &[&str],
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
    states: &[ConditionalTruth],
) -> SourceGraph {
    let mut graph = SourceGraph::default();
    let mut line_owner_marker = false;
    let mut define_depth = 0usize;
    let mut pending_rule_line = None;

    for (line_no, raw) in lines.iter().enumerate() {
        if define_depth > 0 {
            graph.definition_lines.insert(line_no);
            match make_define_boundary(raw) {
                Some((true, valid)) => {
                    graph.uncertain |= !valid;
                    define_depth = define_depth.saturating_add(1);
                }
                Some((false, valid)) => {
                    graph.uncertain |= !valid;
                    define_depth -= 1;
                }
                None => {}
            }
            continue;
        }
        if let Some((true, valid)) = make_define_boundary(raw) {
            graph.definition_lines.insert(line_no);
            graph.uncertain |= !valid;
            define_depth = 1;
            line_owner_marker = false;
            pending_rule_line = None;
            continue;
        }
        if make_define_boundary(raw).is_some_and(|(start, _)| !start) {
            graph.definition_lines.insert(line_no);
            graph.uncertain = true;
            line_owner_marker = false;
            pending_rule_line = None;
            continue;
        }
        match state_at(states, line_no) {
            ConditionalTruth::False => {
                line_owner_marker = false;
                pending_rule_line = None;
                continue;
            }
            ConditionalTruth::Unknown => {
                graph.uncertain |= is_potential_graph_statement(raw)
                    || is_make_include(raw.trim())
                    || contains_make_eval(raw)
                    || has_unproven_make_expansion(raw, scope, dirs, root, rel_dir, line_no);
                line_owner_marker = false;
                pending_rule_line = None;
                continue;
            }
            ConditionalTruth::True => {}
        }
        if contains_make_eval(raw) {
            // GNU Make's eval function parses its expanded argument as new
            // Makefile syntax. Even when the text comes from an opaque define
            // body, it can add consumers or owners missing from this snapshot.
            graph.uncertain = true;
            line_owner_marker = false;
            continue;
        }
        let trimmed = raw.trim();
        if raw.starts_with('\t') {
            if let Some(rule_line) = pending_rule_line {
                graph.recipe_rule_lines.insert(rule_line);
            }
            line_owner_marker = false;
            continue;
        }
        if !trimmed.is_empty() && !trimmed.starts_with('#') {
            pending_rule_line = None;
        }
        if trimmed == "#MM" {
            line_owner_marker = true;
            continue;
        }
        if trimmed.starts_with("#MM") && !trimmed.starts_with("##MM") {
            let Some(edge) = parse_meta_edge(trimmed) else {
                graph.uncertain = true;
                line_owner_marker = false;
                continue;
            };
            line_owner_marker = false;
            let Some(owner) = evaluate_one(edge.target, scope, dirs, root, rel_dir, line_no)
                .and_then(|target| safe_owner(&target))
            else {
                graph.uncertain = true;
                continue;
            };
            let Some(prerequisites) =
                evaluate_words(edge.prerequisites, scope, dirs, root, rel_dir, line_no)
            else {
                graph.uncertain = true;
                continue;
            };
            if !charge_identity_references(&mut graph, prerequisites.len().saturating_add(1))
                || !record_identity(&mut graph, &owner)
            {
                return graph;
            }
            for prerequisite in &prerequisites {
                if !record_identity(&mut graph, prerequisite) {
                    return graph;
                }
            }
            graph.owners.insert(owner.clone());
            add_consumer_edges(&mut graph, &prerequisites, &owner);
            continue;
        }
        if trimmed.is_empty() || trimmed.starts_with('#') || raw.starts_with('\t') {
            line_owner_marker = false;
            continue;
        }
        if is_make_include(trimmed) {
            // Path resolution alone does not prove the included fragment's
            // active rules or side effects are represented in this snapshot.
            graph.uncertain = true;
            line_owner_marker = false;
            continue;
        }
        if is_make_directive(trimmed) || variable_assignment(trimmed).is_some() {
            graph.uncertain |=
                has_unproven_make_expansion(raw, scope, dirs, root, rel_dir, line_no);
            line_owner_marker = false;
            continue;
        }
        let uncommented = strip_make_comment(trimmed).trim();
        let Some((target_raw, prerequisites_raw)) = split_rule(uncommented) else {
            graph.uncertain |= looks_like_ordinary_rule(uncommented);
            if starts_with_make_expansion(uncommented) {
                graph.uncertain |=
                    has_unproven_make_expansion(raw, scope, dirs, root, rel_dir, line_no);
            }
            line_owner_marker = false;
            continue;
        };
        if variable_assignment(prerequisites_raw.trim()).is_some() {
            line_owner_marker = false;
            continue;
        }
        let Some(targets) = evaluate_words(target_raw, scope, dirs, root, rel_dir, line_no) else {
            graph.uncertain = true;
            line_owner_marker = false;
            continue;
        };
        if targets.is_empty() || targets.iter().any(|target| !safe_identity(target)) {
            graph.uncertain = true;
            line_owner_marker = false;
            continue;
        }
        let has_pattern = targets.iter().any(|target| target.contains('%'));
        if has_pattern {
            if line_owner_marker || targets.len() != 1 || !is_bounded_pattern(&targets[0]) {
                graph.uncertain = true;
                line_owner_marker = false;
                continue;
            }
            let Some(prerequisites) = evaluate_pattern_prerequisites(
                prerequisites_raw,
                scope,
                dirs,
                root,
                rel_dir,
                line_no,
            ) else {
                graph.uncertain = true;
                line_owner_marker = false;
                continue;
            };
            if !charge_identity_references(
                &mut graph,
                targets.len().saturating_add(prerequisites.len()),
            ) {
                return graph;
            }
            graph.patterns.push(PatternRule {
                target: targets[0].clone(),
                prerequisites,
            });
            line_owner_marker = false;
            continue;
        }
        let Some(prerequisites) =
            evaluate_rule_prerequisites(prerequisites_raw, scope, dirs, root, rel_dir, line_no)
        else {
            graph.uncertain = true;
            continue;
        };
        if !charge_identity_references(
            &mut graph,
            targets.len().saturating_add(prerequisites.len()),
        ) {
            return graph;
        }
        for target in &targets {
            if !record_identity(&mut graph, target) {
                return graph;
            }
        }
        for prerequisite in &prerequisites {
            if !record_identity(&mut graph, prerequisite) {
                return graph;
            }
        }
        graph.targets_by_line.insert(line_no, targets.clone());
        pending_rule_line = Some(line_no);
        for target in &targets {
            graph.make_identities.insert(target.clone());
        }
        if line_owner_marker {
            for target in &targets {
                if safe_owner(target).is_some() {
                    graph.owners.insert(target.clone());
                } else {
                    graph.uncertain = true;
                }
            }
        }
        line_owner_marker = false;
        let edge_count = targets.len().saturating_mul(prerequisites.len());
        if edge_count > MAX_IDENTITIES.saturating_sub(graph.edge_count) {
            graph.overflow = true;
            return graph;
        }
        for target in &targets {
            add_make_consumer_edges(&mut graph, &prerequisites, target);
        }
    }
    if define_depth != 0 {
        graph.uncertain = true;
    }
    graph
}

/// Whether this active line contains an unescaped GNU Make `eval` function.
///
/// The diagnostic graph deliberately does not interpret the text evaluated by
/// Make. Scan every expansion opener (including nested ones) so `eval` in an
/// assignment RHS or recipe is just as disqualifying as a top-level call.
fn contains_make_eval(line: &str) -> bool {
    let line = strip_make_comment(line);
    let bytes = line.as_bytes();
    let mut index = 0;
    while index + 1 < bytes.len() {
        if bytes[index] != b'$' || !matches!(bytes[index + 1], b'(' | b'{') {
            index += 1;
            continue;
        }
        // `$$(` and `$${` quote the following opener for Make expansion.
        let mut preceding_dollars = 0usize;
        let mut before = index;
        while before > 0 && bytes[before - 1] == b'$' {
            preceding_dollars += 1;
            before -= 1;
        }
        if preceding_dollars % 2 == 1 {
            index += 2;
            continue;
        }

        let mut function = index + 2;
        while function < bytes.len() && bytes[function].is_ascii_whitespace() {
            function += 1;
        }
        if bytes.get(function..function + 4) == Some(b"eval")
            && bytes
                .get(function + 4)
                .is_some_and(|next| next.is_ascii_whitespace() || *next == b',')
        {
            return true;
        }
        index += 2;
    }
    false
}

fn starts_with_make_expansion(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with("$(") || line.starts_with("${")
}

/// Checks expansions in syntax that this graph collector otherwise ignores
/// (assignments, directives, or a top-level expansion-only statement). An
/// unsupported or unresolved expansion could invoke an opaque user variable
/// containing `eval`, so it cannot be assumed side-effect-free.
fn has_unproven_make_expansion(
    line: &str,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
    line_no: usize,
) -> bool {
    let uncommented = strip_make_comment(line);
    let candidate = if let Some((_, rhs, _)) = variable_assignment(uncommented) {
        rhs
    } else if is_make_directive(uncommented.trim()) || starts_with_make_expansion(uncommented) {
        uncommented
    } else {
        return false;
    };
    has_unproven_make_expansion_in_text(candidate, scope, dirs, root, rel_dir, line_no)
}

/// Checks every Make expansion in an invocation's raw source arguments,
/// including invocations whose GenMF semantics are not modeled here. This is
/// only an expansion-safety check; it does not project outputs for unknown
/// macros.
fn has_unproven_make_expansion_in_text(
    candidate: &str,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
    line_no: usize,
) -> bool {
    let bytes = candidate.as_bytes();
    let context = MakeExprContext::new(scope, dirs, line_no, root, rel_dir);
    let mut index = 0usize;
    while index + 1 < bytes.len() {
        if bytes[index] != b'$' || !matches!(bytes[index + 1], b'(' | b'{') {
            index += 1;
            continue;
        }
        let mut preceding_dollars = 0usize;
        let mut before = index;
        while before > 0 && bytes[before - 1] == b'$' {
            preceding_dollars += 1;
            before -= 1;
        }
        if preceding_dollars % 2 == 1 {
            index += 2;
            continue;
        }
        let Some(end) = matching_make_expansion_end(bytes, index) else {
            return true;
        };
        let Ok(expression) = std::str::from_utf8(&bytes[index..=end]) else {
            return true;
        };
        if evaluate_make_expr(expression, &context).is_err() {
            return true;
        }
        index += 2;
    }
    false
}

fn matching_make_expansion_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut stack = vec![match bytes.get(start + 1)? {
        b'(' => b')',
        b'{' => b'}',
        _ => return None,
    }];
    let mut index = start + 2;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index = index.saturating_add(2),
            b'$' if matches!(bytes.get(index + 1), Some(b'(' | b'{')) => {
                stack.push(if bytes[index + 1] == b'(' { b')' } else { b'}' });
                index += 2;
            }
            b'(' => {
                stack.push(b')');
                index += 1;
            }
            b'{' => {
                stack.push(b'}');
                index += 1;
            }
            byte if stack.last() == Some(&byte) => {
                stack.pop();
                if stack.is_empty() {
                    return Some(index);
                }
                index += 1;
            }
            _ => index += 1,
        }
    }
    None
}

fn make_define_boundary(line: &str) -> Option<(bool, bool)> {
    let uncommented = strip_make_comment(line.trim_start()).trim_start();
    let mut words = uncommented.split_whitespace();
    let mut directive = words.next()?;
    let mut modifiers = 0usize;
    while matches!(directive, "override" | "export" | "private") {
        modifiers += 1;
        let next = words.next()?;
        directive = next;
    }
    match directive {
        "define" => Some((true, modifiers <= 3)),
        "endef" => Some((false, modifiers == 0)),
        _ => None,
    }
}

fn is_potential_graph_statement(raw: &str) -> bool {
    let trimmed = raw.trim();
    if trimmed.is_empty() || raw.starts_with('\t') {
        return false;
    }
    if trimmed == "#MM"
        || (trimmed.starts_with("#MM") && !trimmed.starts_with("##MM"))
        || trimmed.starts_with('%')
    {
        return true;
    }
    let uncommented = strip_make_comment(trimmed).trim();
    looks_like_ordinary_rule(uncommented)
}

fn looks_like_ordinary_rule(line: &str) -> bool {
    !line.is_empty()
        && !is_make_directive(line)
        && variable_assignment(line).is_none()
        && line.contains(':')
}

struct MetaEdge<'a> {
    target: &'a str,
    prerequisites: &'a str,
}

fn parse_meta_edge(line: &str) -> Option<MetaEdge<'_>> {
    let body = line
        .strip_prefix("#MM-")
        .or_else(|| line.strip_prefix("#MM"))?;
    if !body.chars().next().is_some_and(char::is_whitespace) {
        return None;
    }
    let body = body.trim_start();
    let (target, prerequisites) = body.split_once(':')?;
    let target = target.trim();
    Some(MetaEdge {
        target,
        prerequisites: strip_make_comment(prerequisites).trim(),
    })
}

fn split_rule(line: &str) -> Option<(&str, &str)> {
    if line.starts_with('#') || line.starts_with('%') || line.contains(';') {
        return None;
    }
    let (target, after_colon) = line.split_once(':')?;
    let prerequisites = if let Some(tail) = after_colon.strip_prefix(':') {
        if tail.starts_with(':') {
            return None;
        }
        tail
    } else {
        after_colon
    };
    if target.is_empty() || target.contains(':') {
        return None;
    }
    let target = target.trim();
    if target.is_empty() || target.contains(['*', '?', '[', ']', '|', '&', '\\']) {
        return None;
    }
    Some((target, prerequisites.trim()))
}

fn is_bounded_pattern(pattern: &str) -> bool {
    pattern.matches('%').count() == 1
        && !pattern.contains(['*', '?', '[', ']', '|', '&', '\\'])
        && safe_identity(pattern)
}

fn evaluate_pattern_prerequisites(
    raw: &str,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
    line: usize,
) -> Option<Vec<String>> {
    if raw.contains(['*', '?', '[', ']', '\\', ';']) {
        return None;
    }
    let context = MakeExprContext::new(scope, dirs, line, root, rel_dir);
    let words = evaluate_make_list(raw, &context).ok()?;
    if words.len() > MAX_IDENTITIES {
        return None;
    }
    if words.iter().filter(|word| word.as_str() == "|").count() > 1 {
        return None;
    }
    let prerequisites = words
        .into_iter()
        .filter(|word| word != "|")
        .collect::<Vec<_>>();
    prerequisites
        .iter()
        .all(|prerequisite| {
            safe_identity(prerequisite)
                && prerequisite.matches('%').count() <= 1
                && !prerequisite.contains(['*', '?', '[', ']', '|', '&', '\\'])
        })
        .then_some(prerequisites)
}

fn is_make_directive(line: &str) -> bool {
    [
        "ifeq", "ifneq", "ifdef", "ifndef", "else", "endif", "define", "endef", "export",
        "unexport", "include", "-include", "sinclude", "override", "vpath",
    ]
    .iter()
    .any(|directive| {
        line.strip_prefix(directive).is_some_and(|tail| {
            tail.is_empty()
                || tail
                    .chars()
                    .next()
                    .is_some_and(|character| character.is_whitespace() || character == '(')
        })
    })
}

fn is_make_include(line: &str) -> bool {
    ["include", "-include", "sinclude"].iter().any(|directive| {
        line.strip_prefix(directive).is_some_and(|tail| {
            tail.is_empty() || tail.chars().next().is_some_and(char::is_whitespace)
        })
    })
}

fn evaluate_rule_prerequisites(
    raw: &str,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
    line: usize,
) -> Option<Vec<String>> {
    if raw.contains(['*', '?', '[', ']', '\\', ';']) {
        return None;
    }
    let context = MakeExprContext::new(scope, dirs, line, root, rel_dir);
    let words = evaluate_make_list(raw, &context).ok()?;
    if words.len() > MAX_IDENTITIES {
        return None;
    }
    let separators = words.iter().filter(|word| word.as_str() == "|").count();
    if separators > 1 {
        return None;
    }
    let prerequisites = words
        .into_iter()
        .filter(|word| word != "|")
        .collect::<Vec<_>>();
    prerequisites
        .iter()
        .all(|item| safe_identity(item) && !item.contains('%'))
        .then_some(prerequisites)
}

fn evaluate_words(
    raw: &str,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
    line: usize,
) -> Option<Vec<String>> {
    if raw.len() > 64 * 1024 || raw.contains(['*', '?', '[', ']', '\\', ';']) {
        return None;
    }
    let context = MakeExprContext::new(scope, dirs, line, root, rel_dir);
    let words = evaluate_make_list(raw, &context).ok()?;
    (words.len() <= MAX_IDENTITIES && words.iter().all(|word| safe_identity(word))).then_some(words)
}

fn add_consumer_edges(graph: &mut SourceGraph, prerequisites: &[String], target: &str) {
    if !record_identity(graph, target) {
        return;
    }
    for prerequisite in prerequisites {
        if !record_identity(graph, prerequisite) {
            return;
        }
        let inserted = graph
            .consumers
            .entry(prerequisite.clone())
            .or_default()
            .insert(target.to_owned());
        if inserted {
            graph.edge_count = graph.edge_count.saturating_add(1);
            graph.overflow |= graph.edge_count > MAX_IDENTITIES;
        }
    }
}

fn add_make_consumer_edges(graph: &mut SourceGraph, prerequisites: &[String], target: &str) {
    if !record_identity(graph, target) {
        return;
    }
    graph.make_identities.insert(target.to_owned());
    for prerequisite in prerequisites {
        if !record_identity(graph, prerequisite) {
            return;
        }
        graph.make_identities.insert(prerequisite.clone());
    }
    add_consumer_edges(graph, prerequisites, target);
}

const fn charge_identity_references(graph: &mut SourceGraph, count: usize) -> bool {
    if count > MAX_IDENTITIES.saturating_sub(graph.identity_references) {
        graph.overflow = true;
        return false;
    }
    graph.identity_references += count;
    true
}

fn charge_macro_output_bytes(graph: &mut SourceGraph, outputs: &[String]) -> bool {
    let Some(bytes) = outputs
        .iter()
        .try_fold(0usize, |total, output| total.checked_add(output.len()))
    else {
        graph.overflow = true;
        return false;
    };
    if bytes > MAX_MACRO_OUTPUT_BYTES.saturating_sub(graph.macro_output_bytes) {
        graph.overflow = true;
        return false;
    }
    graph.macro_output_bytes += bytes;
    true
}

fn record_identity(graph: &mut SourceGraph, identity: &str) -> bool {
    if graph.identities.contains(identity) {
        return true;
    }
    if graph.identities.len() >= MAX_IDENTITIES {
        graph.overflow = true;
        return false;
    }
    graph.identities.insert(identity.to_owned());
    true
}

/// Parses the `name=value` token grammar used by `tools/genmf/genmf.py`.
/// Only double quotes group whitespace; a backslash does not escape a quote,
/// single quotes are ordinary value characters, and a balanced Make reference
/// does not extend an unquoted token across whitespace.
fn closed_macro_arguments(raw: &str) -> Option<ClosedMacroArguments> {
    if raw.len() > MAX_MACRO_ARGUMENT_BYTES
        || raw.contains(['\n', '\r'])
        || raw
            .chars()
            .any(|character| character.is_whitespace() && !character.is_ascii())
    {
        return None;
    }
    let bytes = raw.as_bytes();
    let mut cursor = 0usize;
    let mut arguments = BTreeMap::new();
    while cursor < bytes.len() {
        while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        if cursor == bytes.len() {
            break;
        }

        let key_start = cursor;
        if !bytes.get(cursor).is_some_and(u8::is_ascii_alphanumeric) {
            return None;
        }
        cursor += 1;
        while bytes
            .get(cursor)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        {
            cursor += 1;
        }
        if cursor - key_start > 64 || bytes.get(cursor) != Some(&b'=') {
            return None;
        }
        let key = std::str::from_utf8(&bytes[key_start..cursor])
            .ok()?
            .to_owned();
        cursor += 1;
        let (value, end) = genmf_argument_value(raw, cursor)?;
        cursor = end;
        if arguments.len() >= MAX_MACRO_ARGUMENTS || arguments.insert(key, value).is_some() {
            return None;
        }
    }
    Some(arguments)
}

/// Mirrors GenMF's `([!\\s\"]+|\".*?\")?` value capture. An absent value
/// becomes the empty string in `template.write`; quote escapes are not
/// interpreted by GenMF, so the first double quote always closes the value.
fn genmf_argument_value(raw: &str, start: usize) -> Option<(String, usize)> {
    let bytes = raw.as_bytes();
    if bytes.get(start) == Some(&b'"') {
        let value_start = start + 1;
        let end = raw.get(value_start..)?.find('"')? + value_start;
        return Some((raw.get(value_start..end)?.to_owned(), end + 1));
    }

    let mut end = start;
    while end < bytes.len() && !bytes[end].is_ascii_whitespace() && bytes[end] != b'"' {
        end += 1;
    }
    Some((raw.get(start..end)?.to_owned(), end))
}

fn macro_value<'a>(arguments: &'a ClosedMacroArguments, key: &str) -> Option<&'a str> {
    arguments.get(key).map(String::as_str)
}

fn add_verified_macro_edges(
    graph: &mut SourceGraph,
    lines: &[&str],
    scope: &VarScope,
    dirs: &DirVars,
    source_dirs: (&Path, &Path),
    states: &[ConditionalTruth],
    macros: VerifiedMacros,
) {
    let (root, rel_dir) = source_dirs;
    let invocations = macro_invocations(&lines.join("\n"));
    let template_names = native_template_names(root);
    for (line_no, raw) in lines.iter().enumerate() {
        if graph.definition_lines.contains(&line_no)
            || state_at(states, line_no) == ConditionalTruth::False
        {
            continue;
        }
        if has_inline_genmf_template_reference(raw, template_names.as_ref()) {
            // GenMF searches anywhere on a noncomment source line, while the
            // ownership collector models only full-line invocations.
            graph.uncertain = true;
        }
    }
    let contracts = verified_macro_contracts(root, macros);
    for invocation in &invocations {
        if graph.definition_lines.contains(&invocation.line) {
            continue;
        }
        match state_at(states, invocation.line) {
            ConditionalTruth::False => continue,
            ConditionalTruth::Unknown => {
                graph.uncertain = true;
                continue;
            }
            ConditionalTruth::True => {}
        }
        graph.uncertain |= has_unproven_make_expansion_in_text(
            &invocation.args,
            scope,
            dirs,
            root,
            rel_dir,
            invocation.line,
        );
    }
    let object_dirs = if macros.allows(MacroForm::BuildProg) {
        build_prog_object_dirs(
            graph,
            &invocations,
            scope,
            dirs,
            source_dirs,
            states,
            &contracts,
        )
    } else {
        BTreeMap::new()
    };
    for invocation in invocations {
        if graph.definition_lines.contains(&invocation.line) {
            continue;
        }
        match state_at(states, invocation.line) {
            ConditionalTruth::False => continue,
            ConditionalTruth::Unknown => {
                graph.uncertain = true;
                continue;
            }
            ConditionalTruth::True => {}
        }
        let form = match invocation.name.as_str() {
            "build_prog" => MacroForm::BuildProg,
            "rule_compile_multi" => MacroForm::CompileMulti,
            "rule_assemble_multi" => MacroForm::AssembleMulti,
            "rule_link_binary" => MacroForm::LinkBinary,
            // A macro outside this hash-verified subset may emit source rules,
            // owners, or hidden Make expansions. Visible arguments alone do
            // not prove the effects of its template body.
            _ => {
                graph.uncertain = true;
                continue;
            }
        };
        if !macros.allows(form) {
            graph.uncertain = true;
            continue;
        }
        let Some(contract) = contracts.get(form as usize).and_then(Option::as_ref) else {
            graph.uncertain = true;
            continue;
        };
        let Some(arguments) = effective_macro_arguments(
            &invocation.args,
            contract,
            scope,
            dirs,
            root,
            rel_dir,
            invocation.line,
        ) else {
            graph.uncertain = true;
            continue;
        };
        if matches!(form, MacroForm::LinkBinary) {
            if link_binary_edges(
                graph,
                &arguments,
                scope,
                dirs,
                source_dirs,
                invocation.line,
                &object_dirs,
            )
            .is_empty()
            {
                graph.uncertain = true;
            }
            continue;
        }
        let mut compile_multi_pairs = None;
        let outputs = match form {
            MacroForm::BuildProg => {
                // The verified template also reads USER_OBJS, namespace
                // overrides, arch/*.o and conditional generated depfiles.
                // Explicit files/objs are not a complete consumer projection.
                // Keep attribution uncertain until those inputs are sealed;
                // this does not disable the native program capability.
                graph.uncertain = true;
                continue;
            }
            MacroForm::CompileMulti | MacroForm::AssembleMulti => {
                // These macros use mmake solely as a variable namespace. Their
                // native defaults (`TMP`) are not MetaMake owners.
                let namespace = macro_value(&arguments, "mmake").unwrap_or("TMP");
                if evaluate_one(namespace, scope, dirs, root, rel_dir, invocation.line)
                    .and_then(|name| safe_owner(&name))
                    .is_none()
                {
                    graph.uncertain = true;
                    continue;
                }
                let outputs = if matches!(form, MacroForm::CompileMulti) {
                    let pairs = multi_compile_pairs(
                        &arguments,
                        scope,
                        dirs,
                        root,
                        rel_dir,
                        invocation.line,
                    );
                    let outputs = compile_multi_output_list(&pairs);
                    compile_multi_pairs = Some(pairs);
                    outputs
                } else {
                    multi_assemble_outputs(&arguments, scope, dirs, root, rel_dir, invocation.line)
                };
                if outputs.is_empty()
                    || !charge_identity_references(graph, outputs.len())
                    || !charge_macro_output_bytes(graph, &outputs)
                {
                    graph.uncertain |= outputs.is_empty();
                    continue;
                }
                let mut all_recorded = true;
                for output in &outputs {
                    if !record_identity(graph, output) {
                        all_recorded = false;
                        break;
                    }
                    if !graph.macro_outputs.insert(output.clone()) {
                        graph.ambiguous_macro_outputs.insert(output.clone());
                    }
                    graph.make_identities.insert(output.clone());
                }
                if !all_recorded {
                    continue;
                }
                if let Some(pairs) = compile_multi_pairs {
                    graph.compile_multi_groups.push(CompileMultiGroup {
                        invocation_line: invocation.line,
                        pairs,
                    });
                }
                outputs
            }
            MacroForm::LinkBinary => unreachable!("link binary is handled above"),
        };
        if outputs.is_empty() {
            graph.uncertain = true;
        }
    }
}

fn build_prog_object_dirs(
    graph: &mut SourceGraph,
    invocations: &[crate::parser::Invocation],
    scope: &VarScope,
    dirs: &DirVars,
    source_dirs: (&Path, &Path),
    states: &[ConditionalTruth],
    contracts: &[Option<MacroContract>; 4],
) -> BTreeMap<String, Option<String>> {
    let (root, rel_dir) = source_dirs;
    let mut object_dirs = BTreeMap::<String, Option<String>>::new();
    for invocation in invocations.iter().filter(|invocation| {
        invocation.name == "build_prog" && !graph.definition_lines.contains(&invocation.line)
    }) {
        if state_at(states, invocation.line) != ConditionalTruth::True {
            continue;
        }
        let Some(contract) = contracts[MacroForm::BuildProg as usize].as_ref() else {
            graph.uncertain = true;
            continue;
        };
        let Some(arguments) = effective_macro_arguments(
            &invocation.args,
            contract,
            scope,
            dirs,
            root,
            rel_dir,
            invocation.line,
        ) else {
            graph.uncertain = true;
            continue;
        };
        let owner = macro_value(&arguments, "mmake")
            .and_then(|raw| evaluate_one(raw, scope, dirs, root, rel_dir, invocation.line))
            .and_then(|owner| safe_owner(&owner));
        let Some(objdir_raw) = macro_value(&arguments, "objdir") else {
            graph.uncertain = true;
            continue;
        };
        let objdir = evaluate_one(objdir_raw, scope, dirs, root, rel_dir, invocation.line)
            .filter(|path| safe_identity(path));
        let (Some(owner), Some(objdir)) = (owner, objdir) else {
            graph.uncertain = true;
            continue;
        };
        object_dirs
            .entry(owner)
            .and_modify(|current| {
                if current.as_deref() != Some(objdir.as_str()) {
                    *current = None;
                }
            })
            .or_insert(Some(objdir));
    }
    object_dirs
}

fn multi_compile_pairs(
    args: &ClosedMacroArguments,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
    line: usize,
) -> Vec<CompileMultiPair> {
    let targetdir_raw = macro_value(args, "targetdir").unwrap_or_default();
    let basenames_raw = macro_value(args, "basenames");
    let Some(basenames_raw) = basenames_raw else {
        return Vec::new();
    };
    let targetdir = if targetdir_raw.trim().is_empty() {
        Some(String::new())
    } else {
        evaluate_one(targetdir_raw, scope, dirs, root, rel_dir, line)
    };
    let Some(targetdir) = targetdir else {
        return Vec::new();
    };
    let usetree = macro_value(args, "usetree").unwrap_or("no");
    if evaluate_one(usetree, scope, dirs, root, rel_dir, line).as_deref() != Some("no") {
        return Vec::new();
    }
    let Some(basenames) = evaluate_words(basenames_raw, scope, dirs, root, rel_dir, line) else {
        return Vec::new();
    };
    if basenames.len().saturating_mul(2) > MAX_IDENTITIES {
        return Vec::new();
    }
    let mut pairs = Vec::with_capacity(basenames.len());
    let mut output_bytes = 0usize;
    for basename in basenames {
        let base = if targetdir.is_empty() {
            if !safe_identity(&basename) {
                return Vec::new();
            }
            basename
        } else {
            let leaf = basename.rsplit('/').next().unwrap_or_default();
            if !safe_filename(leaf) {
                return Vec::new();
            }
            join_output(&targetdir, leaf)
        };
        let Some(pair_bytes) = base
            .len()
            .checked_mul(2)
            .and_then(|bytes| bytes.checked_add(4))
        else {
            return Vec::new();
        };
        let Some(total_bytes) = output_bytes.checked_add(pair_bytes) else {
            return Vec::new();
        };
        if total_bytes > MAX_MACRO_OUTPUT_BYTES {
            return Vec::new();
        }
        output_bytes = total_bytes;
        pairs.push(CompileMultiPair {
            object: format!("{base}.o"),
            depfile: format!("{base}.d"),
        });
    }
    if compile_multi_output_list(&pairs).is_empty() {
        Vec::new()
    } else {
        pairs
    }
}

fn compile_multi_output_list(pairs: &[CompileMultiPair]) -> Vec<String> {
    unique_outputs(
        pairs
            .iter()
            .flat_map(|pair| [pair.object.clone(), pair.depfile.clone()])
            .collect(),
    )
}

fn multi_assemble_outputs(
    args: &ClosedMacroArguments,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
    line: usize,
) -> Vec<String> {
    let targetdir_raw = macro_value(args, "targetdir").unwrap_or_default();
    let basenames_raw = macro_value(args, "basenames");
    let Some(basenames_raw) = basenames_raw else {
        return Vec::new();
    };
    let targetdir = if targetdir_raw.trim().is_empty() {
        Some(String::new())
    } else {
        evaluate_one(targetdir_raw, scope, dirs, root, rel_dir, line)
    };
    let Some(targetdir) = targetdir else {
        return Vec::new();
    };
    let Some(basenames) = evaluate_words(basenames_raw, scope, dirs, root, rel_dir, line) else {
        return Vec::new();
    };
    unique_outputs(
        basenames
            .into_iter()
            .map(|basename| {
                if targetdir.is_empty() {
                    safe_identity(&basename).then(|| format!("{basename}.o"))
                } else {
                    let basename = basename.rsplit('/').next().unwrap_or_default();
                    safe_filename(basename)
                        .then(|| join_output(&targetdir, &format!("{basename}.o")))
                }
            })
            .collect::<Option<Vec<_>>>()
            .unwrap_or_default(),
    )
}

fn link_binary_edges(
    graph: &mut SourceGraph,
    args: &ClosedMacroArguments,
    scope: &VarScope,
    dirs: &DirVars,
    source_dirs: (&Path, &Path),
    line: usize,
    object_dirs: &BTreeMap<String, Option<String>>,
) -> Vec<String> {
    let (root, rel_dir) = source_dirs;
    let Some(binary_raw) = macro_value(args, "file") else {
        return Vec::new();
    };
    let Some(binary_output) = evaluate_one(binary_raw, scope, dirs, root, rel_dir, line) else {
        return Vec::new();
    };
    let Some(name_raw) = macro_value(args, "name") else {
        return Vec::new();
    };
    let Some(name) = evaluate_one(name_raw, scope, dirs, root, rel_dir, line) else {
        return Vec::new();
    };
    if !safe_filename(&name) {
        return Vec::new();
    }
    let mmake_raw = macro_value(args, "mmake").unwrap_or("BD");
    let Some(mmake) = evaluate_one(mmake_raw, scope, dirs, root, rel_dir, line)
        .and_then(|mmake| safe_owner(&mmake))
    else {
        return Vec::new();
    };
    let objects_raw = macro_value(args, "objs").unwrap_or_default();
    let Some(objects) = evaluate_words(objects_raw, scope, dirs, root, rel_dir, line) else {
        return Vec::new();
    };
    let files_raw = macro_value(args, "files").unwrap_or_default();
    let Some(files) = evaluate_words(files_raw, scope, dirs, root, rel_dir, line) else {
        return Vec::new();
    };
    let asmfiles_raw = macro_value(args, "asmfiles").unwrap_or_default();
    let Some(asmfiles) = evaluate_words(asmfiles_raw, scope, dirs, root, rel_dir, line) else {
        return Vec::new();
    };
    let stem_count = files.len().saturating_add(asmfiles.len());
    if objects
        .len()
        .saturating_add(stem_count)
        .saturating_add(files.len())
        > MAX_IDENTITIES
    {
        return Vec::new();
    }
    let mut objects = objects;
    if stem_count > 0 {
        let Some(objdir) = object_dirs.get(&mmake).and_then(Option::as_deref) else {
            return Vec::new();
        };
        if objdir.is_empty() || !safe_identity(objdir) {
            return Vec::new();
        }
        for file_stem in files {
            let basename = file_stem.rsplit('/').next().unwrap_or_default();
            if !safe_filename(basename) {
                return Vec::new();
            }
            let object = join_output(objdir, &format!("{basename}.o"));
            objects.push(object.clone());
            objects.push(object.trim_end_matches(".o").to_owned() + ".d");
        }
        for asm_stem in asmfiles {
            let basename = asm_stem.rsplit('/').next().unwrap_or_default();
            if !safe_filename(basename) {
                return Vec::new();
            }
            objects.push(join_output(objdir, &format!("{basename}.o")));
        }
    }
    if objects.len() > MAX_IDENTITIES
        || objects.len() > MAX_IDENTITIES.saturating_sub(graph.edge_count)
        || !charge_identity_references(graph, objects.len().saturating_add(1))
    {
        return Vec::new();
    }
    add_make_consumer_edges(graph, &objects, &binary_output);
    vec![binary_output]
}

fn evaluate_one(
    raw: &str,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
    line: usize,
) -> Option<String> {
    let values = evaluate_words(raw, scope, dirs, root, rel_dir, line)?;
    (values.len() == 1).then(|| values[0].clone())
}

fn unique_outputs(outputs: Vec<String>) -> Vec<String> {
    let unique = outputs.iter().collect::<BTreeSet<_>>();
    if outputs.len() <= MAX_IDENTITIES
        && unique.len() == outputs.len()
        && outputs.iter().all(|output| safe_identity(output))
    {
        outputs
    } else {
        Vec::new()
    }
}

fn join_output(directory: &str, basename: &str) -> String {
    if directory.ends_with('/') {
        format!("{directory}{basename}")
    } else {
        format!("{directory}/{basename}")
    }
}

fn instantiate_pattern_edges(graph: &mut SourceGraph) {
    let mut applied = BTreeSet::<(usize, String)>::new();
    let mut match_attempts = 0usize;
    loop {
        let identities = graph.make_identities.iter().cloned().collect::<Vec<_>>();
        // Pattern references are snapshotted before edges are added so the
        // graph can be mutably budgeted while matching without holding an
        // immutable borrow into `graph.patterns`.
        let patterns = graph
            .patterns
            .iter()
            .map(|pattern| (pattern.target.clone(), pattern.prerequisites.clone()))
            .collect::<Vec<_>>();
        let mut additions = Vec::<(String, String)>::new();
        for (pattern_index, (pattern_target, prerequisites)) in patterns.iter().enumerate() {
            for target in &identities {
                match_attempts = match_attempts.saturating_add(1);
                if match_attempts > MAX_PATTERN_MATCHES {
                    graph.uncertain = true;
                    return;
                }
                let Some(stem) = pattern_stem(pattern_target, target) else {
                    continue;
                };
                if !applied.insert((pattern_index, target.clone())) {
                    continue;
                }
                if !charge_identity_references(graph, prerequisites.len().saturating_add(1))
                    || prerequisites.len() > MAX_IDENTITIES.saturating_sub(additions.len())
                {
                    graph.overflow = true;
                    return;
                }
                for prerequisite in prerequisites {
                    let prerequisite = if prerequisite.contains('%') {
                        substitute_stem(prerequisite, &stem)
                    } else {
                        prerequisite.clone()
                    };
                    if !safe_identity(&prerequisite) {
                        graph.uncertain = true;
                        return;
                    }
                    additions.push((prerequisite, target.clone()));
                }
            }
        }
        if additions.is_empty() {
            break;
        }
        let before = graph.edge_count;
        for (prerequisite, target) in additions {
            add_make_consumer_edges(graph, &[prerequisite], &target);
        }
        if graph.overflow {
            return;
        }
        if graph.edge_count == before {
            break;
        }
    }
}

fn pattern_stem(pattern: &str, identity: &str) -> Option<String> {
    let (prefix, suffix) = pattern.split_once('%')?;
    if !identity.starts_with(prefix)
        || !identity.ends_with(suffix)
        || identity.len() < prefix.len().saturating_add(suffix.len())
    {
        return None;
    }
    let end = identity.len().checked_sub(suffix.len())?;
    let stem = identity.get(prefix.len()..end)?;
    (!stem.is_empty()).then(|| stem.to_owned())
}

fn substitute_stem(pattern: &str, stem: &str) -> String {
    let (prefix, suffix) = pattern
        .split_once('%')
        .expect("pattern prerequisites are validated with at most one stem");
    format!("{prefix}{stem}{suffix}")
}

fn safe_filename(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'+' | b'.'))
}

fn safe_identity(value: &str) -> bool {
    let unresolved = value
        .replace("${AROS_BUILD_DIR}", "")
        .replace("${AROS_SOURCE_DIR}", "")
        .replace("${AROS_PORTS_DIR}", "")
        .replace("${AROS_PORTS_SOURCE_DIR}", "");
    !value.is_empty()
        && value.len() <= 4096
        && !unresolved.contains(['$', '*', '?', '[', ']', '|', '&', ';', '\\', '\n', '\r'])
        && Path::new(value)
            .components()
            .all(|component| !matches!(component, std::path::Component::ParentDir))
}

fn safe_owner(value: &str) -> Option<String> {
    (!value.is_empty()
        && value.len() <= 160
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'+')))
    .then(|| value.to_owned())
}

const fn state_at(states: &[ConditionalTruth], line: usize) -> ConditionalTruth {
    if line < states.len() {
        states[line]
    } else {
        ConditionalTruth::Unknown
    }
}

fn trace_all_owners(
    graph: &SourceGraph,
    output: &str,
    budget: &mut DiagnosticBudget,
) -> Option<Vec<SourceRuleOwnership>> {
    let start = graph.identities.get(output)?.as_str();
    let mut queue = VecDeque::new();
    queue.try_reserve(1).ok()?;
    queue.push_back(start);

    // BFS first discovery is the shortest path. Store one predecessor per
    // vertex instead of cloning every path prefix at every edge.
    let mut predecessors = BTreeMap::<&str, Option<&str>>::new();
    let mut indegree = BTreeMap::<&str, usize>::new();
    predecessors.insert(start, None);
    indegree.insert(start, 0);
    let mut candidates = BTreeSet::<&str>::new();

    while let Some(identity) = queue.pop_front() {
        if !budget.charge_work(1) {
            return None;
        }
        let is_owner = graph.owners.contains(identity);
        if is_owner {
            candidates.insert(identity);
        }
        let mut has_successor = false;
        let mut degree_overflow = false;
        if !visit_sorted_successors(graph, identity, budget, |consumer| {
            has_successor = true;
            let Some(next_degree) = indegree.get(consumer).copied().unwrap_or(0).checked_add(1)
            else {
                degree_overflow = true;
                return false;
            };
            indegree.insert(consumer, next_degree);
            if !predecessors.contains_key(consumer) {
                if predecessors.len() >= MAX_IDENTITIES {
                    return false;
                }
                predecessors.insert(consumer, Some(identity));
                if queue.try_reserve(1).is_err() {
                    return false;
                }
                queue.push_back(consumer);
            }
            true
        }) || degree_overflow
        {
            return None;
        }
        if !has_successor && !is_owner {
            // A known owner on one branch cannot hide a separate, unowned
            // consumer. Complete attribution requires every reachable branch
            // to terminate at a source-proven owner.
            return None;
        }
    }

    // Re-enumerate the borrowed edges for Kahn's cycle check. No cloned
    // adjacency or retained path vectors are needed.
    let mut ready = VecDeque::new();
    ready.try_reserve_exact(indegree.len()).ok()?;
    for (identity, degree) in &indegree {
        if *degree == 0 {
            ready.push_back(*identity);
        }
    }
    let mut visited = 0usize;
    while let Some(identity) = ready.pop_front() {
        if !budget.charge_work(1) {
            return None;
        }
        visited += 1;
        let mut invalid_indegree = false;
        if !visit_sorted_successors(graph, identity, budget, |consumer| {
            let Some(degree) = indegree.get_mut(consumer) else {
                invalid_indegree = true;
                return false;
            };
            if *degree == 0 {
                invalid_indegree = true;
                return false;
            }
            *degree -= 1;
            if *degree == 0 {
                if ready.try_reserve(1).is_err() {
                    return false;
                }
                ready.push_back(consumer);
            }
            true
        }) || invalid_indegree
        {
            return None;
        }
    }
    if visited != indegree.len() || candidates.is_empty() {
        return None;
    }

    let mut proofs = Vec::new();
    proofs.try_reserve_exact(candidates.len()).ok()?;
    for owner in candidates {
        proofs.push(materialize_ownership_path(
            &predecessors,
            start,
            owner,
            budget,
        )?);
    }
    Some(proofs)
}

fn visit_sorted_successors<'a>(
    graph: &'a SourceGraph,
    identity: &str,
    budget: &mut DiagnosticBudget,
    mut visit: impl FnMut(&'a str) -> bool,
) -> bool {
    let mut consumers = graph
        .consumers
        .get(identity)
        .into_iter()
        .flat_map(|set| set.iter())
        .peekable();
    let mut macro_owners = graph
        .macro_owners
        .get(identity)
        .into_iter()
        .flat_map(|set| set.iter())
        .peekable();

    loop {
        let ordering = match (consumers.peek(), macro_owners.peek()) {
            (None, None) => return true,
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (Some(left), Some(right)) => left.as_str().cmp(right.as_str()),
        };
        let next = match ordering {
            std::cmp::Ordering::Less => consumers.next(),
            std::cmp::Ordering::Greater => macro_owners.next(),
            std::cmp::Ordering::Equal => {
                let left = consumers.next();
                let _ = macro_owners.next();
                left
            }
        };
        let Some(next) = next else {
            return true;
        };
        if !budget.charge_work(1) || !visit(next.as_str()) {
            return false;
        }
    }
}

fn materialize_ownership_path(
    predecessors: &BTreeMap<&str, Option<&str>>,
    root: &str,
    owner: &str,
    budget: &mut DiagnosticBudget,
) -> Option<SourceRuleOwnership> {
    let mut path_len = 0usize;
    let mut path_bytes = 0usize;
    let mut cursor = owner;
    loop {
        if !budget.charge_work(1) {
            return None;
        }
        path_len = path_len.checked_add(1)?;
        path_bytes = path_bytes.checked_add(cursor.len())?;
        match predecessors.get(cursor)? {
            Some(parent) => cursor = parent,
            None if cursor == root => break,
            None => return None,
        }
    }
    let retained_bytes = std::mem::size_of::<SourceRuleOwnership>()
        .checked_add(owner.len())?
        .checked_add(path_len.checked_mul(std::mem::size_of::<String>())?)?
        .checked_add(path_bytes)?;
    if !budget.charge_path_bytes(retained_bytes) {
        return None;
    }

    let mut chain = Vec::new();
    chain.try_reserve_exact(path_len).ok()?;
    cursor = owner;
    loop {
        if !budget.charge_work(1) {
            return None;
        }
        chain.push(clone_path_string(cursor)?);
        match predecessors.get(cursor)? {
            Some(parent) => cursor = parent,
            None => break,
        }
    }
    chain.reverse();
    Some(SourceRuleOwnership {
        owner: clone_path_string(owner)?,
        chain,
    })
}

fn clone_path_string(value: &str) -> Option<String> {
    let mut owned = String::new();
    owned.try_reserve_exact(value.len()).ok()?;
    owned.push_str(value);
    Some(owned)
}

fn trace_paired_compile_outputs(
    graph: &SourceGraph,
    rejected_outputs: &[String],
    rejected_rule_line: usize,
    budget: &mut DiagnosticBudget,
) -> Option<Vec<SourceRuleOwnership>> {
    if rejected_outputs.len() < 2 || graph.recipe_rule_lines.contains(&rejected_rule_line) {
        return None;
    }
    let mut rejected_set = BTreeSet::<&str>::new();
    for output in rejected_outputs {
        if !budget.charge_work(1) || !rejected_set.insert(output.as_str()) {
            return None;
        }
    }

    // Build bounded indexes once. Re-scanning every invocation for each output
    // turns a finite snapshot into a groups × outputs walk.
    let mut groups_by_outputs = BTreeMap::<Vec<&str>, Vec<usize>>::new();
    let mut output_producers = BTreeMap::<&str, usize>::new();
    for (group_index, group) in graph.compile_multi_groups.iter().enumerate() {
        let output_count = group.pairs.len().checked_mul(2)?;
        if !budget.charge_work(output_count) {
            return None;
        }
        let group_outputs = compile_multi_group_outputs(group)?;
        for output in &group_outputs {
            let count = output_producers.entry(*output).or_default();
            *count = count.checked_add(1)?;
        }
        let mut output_key = Vec::new();
        output_key.try_reserve_exact(group_outputs.len()).ok()?;
        output_key.extend(group_outputs);
        let matching_groups = groups_by_outputs.entry(output_key).or_default();
        matching_groups.try_reserve(1).ok()?;
        matching_groups.push(group_index);
    }
    let mut rejected_key = Vec::new();
    rejected_key.try_reserve_exact(rejected_set.len()).ok()?;
    rejected_key.extend(rejected_set.iter().copied());
    let [group_index] = groups_by_outputs.get(&rejected_key)?.as_slice() else {
        return None;
    };
    let group = graph.compile_multi_groups.get(*group_index)?;
    if group.pairs.is_empty() {
        return None;
    }

    let group_outputs = compile_multi_group_outputs(group)?;
    let mut ordinary_targets = BTreeSet::<&str>::new();
    for (line, targets) in &graph.targets_by_line {
        if *line == rejected_rule_line {
            continue;
        }
        for target in targets {
            if !budget.charge_work(1) {
                return None;
            }
            ordinary_targets.insert(target.as_str());
        }
    }
    for output in &group_outputs {
        if !graph.macro_outputs.contains(*output)
            || graph.ambiguous_macro_outputs.contains(*output)
            || graph.macro_owners.contains_key(*output)
            || ordinary_targets.contains(*output)
            || output_producers.get(*output) != Some(&1)
        {
            return None;
        }
        for pattern in &graph.patterns {
            if !budget.charge_work(1) {
                return None;
            }
            if pattern_stem(&pattern.target, output).is_some() {
                return None;
            }
        }
    }

    // Keep the object's real consumer chain as the proof for an otherwise
    // ownerless sidecar; never invent a Make edge from `.d` to `.o`.
    let mut owners = BTreeMap::<String, SourceRuleOwnership>::new();
    for pair in &group.pairs {
        let object_owners = trace_all_owners(graph, &pair.object, budget)?;
        for proof in object_owners {
            merge_ownership(&mut owners, proof, budget)?;
        }

        // This detects source graph consumers/owners only. A `%include_deps`
        // invocation consumes generated text, but its runtime-expanded
        // prerequisites are not part of this source snapshot or this proof.
        let depfile_has_source_consumers = graph
            .consumers
            .get(&pair.depfile)
            .is_some_and(|consumers| !consumers.is_empty())
            || graph.owners.contains(&pair.depfile)
            || graph.macro_owners.contains_key(&pair.depfile);
        if depfile_has_source_consumers {
            for proof in trace_all_owners(graph, &pair.depfile, budget)? {
                merge_ownership(&mut owners, proof, budget)?;
            }
        }
    }
    (!owners.is_empty()).then(|| owners.into_values().collect())
}

fn compile_multi_group_outputs(group: &CompileMultiGroup) -> Option<BTreeSet<&str>> {
    if group.pairs.is_empty() || group.invocation_line >= MAX_LINES {
        return None;
    }
    let mut outputs = BTreeSet::new();
    for pair in &group.pairs {
        if !safe_identity(&pair.object)
            || !safe_identity(&pair.depfile)
            || !outputs.insert(pair.object.as_str())
            || !outputs.insert(pair.depfile.as_str())
        {
            return None;
        }
    }
    Some(outputs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::make_vars::collect_vars_impl;
    use std::fmt::Write as _;
    use tempfile::tempdir;

    fn fixture(
        source: &str,
        output: &str,
        verified: VerifiedMacros,
    ) -> Option<SourceRuleOwnership> {
        let root = tempdir().unwrap();
        let source = with_clean_macro_configuration(source);
        let joined = crate::parser::join_continuations(&source);
        let (scope, states) =
            collect_vars_impl(&joined, Some(&crate::parser::TargetContext::default()));
        let dirs = DirVars::load(root.path());
        let line = joined.lines().position(|candidate| {
            candidate
                .split_once(':')
                .is_some_and(|(target, _)| target.trim() == output)
        })? + 1;
        attribute_with_verified_macros(
            &joined,
            &scope,
            &dirs,
            (root.path(), Path::new("fixture")),
            Some(&states),
            (output, line),
            verified,
        )
    }

    fn owners_fixture(
        source: &str,
        rejected_rule_target: &str,
    ) -> Option<Vec<SourceRuleOwnership>> {
        owners_fixture_with_verified(source, rejected_rule_target, VerifiedMacros::default())
    }

    fn owners_fixture_with_verified(
        source: &str,
        rejected_rule_target: &str,
        verified: VerifiedMacros,
    ) -> Option<Vec<SourceRuleOwnership>> {
        let (graph, rejected_line, outputs) =
            source_graph_fixture(source, rejected_rule_target, verified)?;
        attribute_graph_outputs(&graph, rejected_line, &outputs)
    }

    fn source_graph_fixture(
        source: &str,
        rejected_rule_target: &str,
        verified: VerifiedMacros,
    ) -> Option<(SourceGraph, usize, Vec<String>)> {
        let root = tempdir().unwrap();
        let source = with_clean_macro_configuration(source);
        let joined = crate::parser::join_continuations(&source);
        let (scope, states) =
            collect_vars_impl(&joined, Some(&crate::parser::TargetContext::default()));
        let dirs = DirVars::load(root.path());
        let lines = joined.lines().collect::<Vec<_>>();
        let line = joined.lines().position(|candidate| {
            candidate
                .split_once(':')
                .is_some_and(|(target, _)| target.trim() == rejected_rule_target)
        })? + 1;
        let rejected_line = line.checked_sub(1)?;
        let mut graph = parse_source_graph(
            &lines,
            &scope,
            &dirs,
            root.path(),
            Path::new("fixture"),
            &states,
        );
        add_verified_macro_edges(
            &mut graph,
            &lines,
            &scope,
            &dirs,
            (root.path(), Path::new("fixture")),
            &states,
            verified,
        );
        instantiate_pattern_edges(&mut graph);
        if graph.overflow || graph.uncertain {
            return None;
        }
        let outputs = graph.targets_by_line.get(&rejected_line)?.clone();
        Some((graph, rejected_line, outputs))
    }

    fn with_clean_macro_configuration(source: &str) -> String {
        format!(
            "CPPFLAGS := -DTEST_CPPFLAGS\nCFLAGS := -DTEST_CFLAGS\nAFLAGS := -DTEST_AFLAGS\nCC := test-cc\nTARGET_SYSROOT := --sysroot=test\nTARGET_COVERAGEINSTR :=\nTARGET_FUNCINSTR :=\nTARGET_LTO :=\n{source}"
        )
    }

    #[test]
    fn follows_exact_ordinary_prerequisite_and_meta_owner_chain() {
        let ownership = fixture(
            "obj/unit.o: src/unit.c\narchive.a: obj/unit.o\n#MM library-owner : archive.a\n",
            "obj/unit.o",
            VerifiedMacros::default(),
        )
        .unwrap();
        assert_eq!(ownership.owner, "library-owner");
        assert_eq!(
            ownership.chain,
            ["obj/unit.o", "archive.a", "library-owner"]
        );
    }

    #[test]
    fn parser_pipeline_entrypoint_uses_the_rejected_rule_line() {
        let source =
            "obj/unit.o: src/unit.c\narchive.a: obj/unit.o\n#MM library-owner : archive.a\n";
        let root = tempdir().unwrap();
        let joined = crate::parser::join_continuations(source);
        let (scope, states) = collect_vars_impl(&joined, None);
        let dirs = DirVars::load(root.path());
        let rejected_line = 1;
        let result = attribute_rejected_rule_line(
            &joined,
            &scope,
            &dirs,
            root.path(),
            Path::new("fixture"),
            Some(&states),
            rejected_line,
        )
        .unwrap();
        assert_eq!(result.owner, "library-owner");
        assert!(attribute_rejected_rule_line(
            &joined,
            &scope,
            &dirs,
            root.path(),
            Path::new("fixture"),
            Some(&states),
            3,
        )
        .is_none());
    }

    #[test]
    fn build_prog_explicit_inputs_do_not_prove_implicit_consumer_closure() {
        let source = "obj/unit.o: src/unit.c\nobj/extra.o: src/extra.c\n%build_prog mmake=build-owner progname=app files=src/unit objs=obj/extra.o objdir=obj\n";
        for output in ["obj/unit.o", "obj/extra.o"] {
            assert!(fixture(
                source,
                output,
                VerifiedMacros::from_forms(&[MacroForm::BuildProg]),
            )
            .is_none());
        }
    }

    #[test]
    fn build_prog_user_objects_and_namespace_overrides_cannot_hide_consumers() {
        for ambient in [
            "USER_OBJS := obj/unit.o\n",
            "second_OBJS := obj/unit.o\n",
            "second_ARCHOBJS := obj/unit.o\n",
        ] {
            let source = format!(
                "obj/unit.o: src/unit.c\n#MM first : obj/unit.o\n{ambient}%build_prog mmake=second progname=other files=other objdir=obj\n"
            );
            assert!(
                fixture(
                    &source,
                    "obj/unit.o",
                    VerifiedMacros::from_forms(&[MacroForm::BuildProg]),
                )
                .is_none(),
                "{ambient}"
            );
        }
        let inactive = "obj/unit.o: src/unit.c\n#MM first : obj/unit.o\nifeq (0,1)\n%build_prog mmake=second progname=other files=other objdir=obj\nendif\n";
        assert_eq!(
            fixture(
                inactive,
                "obj/unit.o",
                VerifiedMacros::from_forms(&[MacroForm::BuildProg])
            )
            .unwrap()
            .owner,
            "first"
        );
    }

    #[test]
    fn direct_meta_owner_on_rejected_target_is_exact_proof() {
        let source = "direct.o: source.c\n#MM direct.o : source.c\n";
        let proof = fixture(source, "direct.o", VerifiedMacros::default()).unwrap();
        assert_eq!(proof.owner, "direct.o");
        assert_eq!(proof.chain, ["direct.o"]);
    }

    #[test]
    fn define_bodies_are_opaque_to_rules_meta_owners_and_macros() {
        let source = with_clean_macro_configuration("override define hidden_rules\nphantom.o: rejected.o\n#MM phantom-owner : rejected.o\n%build_prog mmake=macro-owner progname=phantom files=phantom objdir=obj\nexport define nested_hidden\nnested.o: rejected.o\n#MM nested-owner : rejected.o\n%rule_compile_multi mmake=nested-output basenames=nested targetdir=obj\nendef\nendef\nrejected.o: source.c\n#MM real-owner : rejected.o\n");
        let root = tempdir().unwrap();
        let joined = crate::parser::join_continuations(&source);
        let (scope, states) =
            collect_vars_impl(&joined, Some(&crate::parser::TargetContext::default()));
        let lines = joined.lines().collect::<Vec<_>>();
        let dirs = DirVars::load(root.path());
        let mut graph = parse_source_graph(
            &lines,
            &scope,
            &dirs,
            root.path(),
            Path::new("fixture"),
            &states,
        );
        add_verified_macro_edges(
            &mut graph,
            &lines,
            &scope,
            &dirs,
            (root.path(), Path::new("fixture")),
            &states,
            VerifiedMacros::from_forms(&[MacroForm::BuildProg]),
        );
        assert!(!graph.owners.contains("phantom-owner"));
        assert!(!graph.owners.contains("nested-owner"));
        assert!(!graph.owners.contains("macro-owner"));
        assert!(!graph.macro_owners.contains_key("obj/phantom.o"));
        assert!(!graph.uncertain);
        assert_eq!(
            fixture(
                &source,
                "rejected.o",
                VerifiedMacros::from_forms(&[MacroForm::BuildProg])
            )
            .unwrap()
            .owner,
            "real-owner"
        );
    }

    #[test]
    fn active_eval_of_an_opaque_define_vetoes_partial_owner_proof() {
        let source = "define HIDDEN\n#MM competing-owner : rejected.o\nendef\nrejected.o: source.c\n#MM real-owner : rejected.o\n$(eval $(HIDDEN))\n";
        assert!(fixture(source, "rejected.o", VerifiedMacros::default()).is_none());

        let braced = "define HIDDEN\n#MM competing-owner : rejected.o\nendef\nrejected.o: source.c\n#MM real-owner : rejected.o\n${eval ${HIDDEN}}\n";
        assert!(fixture(braced, "rejected.o", VerifiedMacros::default()).is_none());
    }

    #[test]
    fn assignment_expansion_must_be_proven_side_effect_free() {
        let hidden_variable = "define HIDDEN\n$(eval #MM competing-owner : rejected.o)\nendef\nDYNAMIC := $(HIDDEN)\nrejected.o: source.c\n#MM real-owner : rejected.o\n";
        assert!(fixture(hidden_variable, "rejected.o", VerifiedMacros::default()).is_none());

        let hidden_call = "define HIDDEN\n$(eval #MM competing-owner : rejected.o)\nendef\nDYNAMIC := $(call HIDDEN)\nrejected.o: source.c\n#MM real-owner : rejected.o\n";
        assert!(fixture(hidden_call, "rejected.o", VerifiedMacros::default()).is_none());
    }

    #[test]
    fn known_false_branch_and_inert_define_do_not_veto_owner_proof() {
        let source = "ifeq (0,1)\ndefine HIDDEN\n#MM inactive-owner : rejected.o\nendef\nDYNAMIC := $(HIDDEN)\n$(eval $(HIDDEN))\nendif\nrejected.o: source.c\n#MM real-owner : rejected.o\n";
        assert_eq!(
            fixture(source, "rejected.o", VerifiedMacros::default())
                .unwrap()
                .owner,
            "real-owner"
        );
    }

    #[test]
    fn ordinary_endpoint_budget_is_global_across_rules() {
        let target_names = (0..32_768)
            .map(|index| format!("output-{index}"))
            .collect::<Vec<_>>()
            .join(" ");
        let source =
            format!("TARGETS := {target_names}\n$(TARGETS): input.o\n$(TARGETS): input.o\n");
        let root = tempdir().unwrap();
        let joined = crate::parser::join_continuations(&source);
        let (scope, states) =
            collect_vars_impl(&joined, Some(&crate::parser::TargetContext::default()));
        let dirs = DirVars::load(root.path());
        let lines = joined.lines().collect::<Vec<_>>();
        let graph = parse_source_graph(
            &lines,
            &scope,
            &dirs,
            root.path(),
            Path::new("fixture"),
            &states,
        );
        assert!(graph.overflow);
        assert!(graph.identity_references <= MAX_IDENTITIES);
        assert!(graph.identities.len() <= MAX_IDENTITIES);
        assert_eq!(graph.targets_by_line.len(), 1);
    }

    #[test]
    fn repeated_finite_macro_seeds_share_one_global_identity_budget() {
        let basenames = (0..32_768)
            .map(|index| format!("unit-{index}"))
            .collect::<Vec<_>>()
            .join(" ");
        let source = format!(
            "BASENAMES := {basenames}\n%rule_compile_multi basenames=\"$(BASENAMES)\" targetdir=first\n%rule_compile_multi basenames=\"$(BASENAMES)\" targetdir=second\n"
        );
        let root = tempdir().unwrap();
        let source = with_clean_macro_configuration(&source);
        let joined = crate::parser::join_continuations(&source);
        let (scope, states) =
            collect_vars_impl(&joined, Some(&crate::parser::TargetContext::default()));
        let dirs = DirVars::load(root.path());
        let lines = joined.lines().collect::<Vec<_>>();
        let mut graph = parse_source_graph(
            &lines,
            &scope,
            &dirs,
            root.path(),
            Path::new("fixture"),
            &states,
        );
        add_verified_macro_edges(
            &mut graph,
            &lines,
            &scope,
            &dirs,
            (root.path(), Path::new("fixture")),
            &states,
            VerifiedMacros::from_forms(&[MacroForm::CompileMulti]),
        );
        assert!(graph.overflow);
        assert_eq!(graph.identity_references, MAX_IDENTITIES);
        assert_eq!(graph.macro_outputs.len(), MAX_IDENTITIES);
        assert!(graph.identities.len() <= MAX_IDENTITIES);
    }

    #[test]
    fn compile_assemble_and_link_binary_macro_seeds_are_bounded() {
        let source = "compiled.o: src/compiled.c\nassembled.o: src/assembled.s\nobj/link-input.o: src/link-input.c\ncompiled-archive.a: compiled.o\nassembled-archive.a: assembled.o\n%rule_compile_multi basenames=compiled\n%rule_assemble_multi mmake=assemble-namespace basenames=assembled\n%rule_link_binary file=bin/image.o name=image objs=obj/link-input.o\n#MM compile-owner : compiled-archive.a\n#MM assemble-owner : assembled-archive.a\n#MM link-owner : bin/image.o\n";
        let all_macros = VerifiedMacros::from_forms(&[
            MacroForm::CompileMulti,
            MacroForm::AssembleMulti,
            MacroForm::LinkBinary,
        ]);
        let proofs = [
            ("compiled.o", "compile-owner"),
            ("assembled.o", "assemble-owner"),
            ("obj/link-input.o", "link-owner"),
        ];
        for (output, expected) in proofs {
            assert_eq!(fixture(source, output, all_macros).unwrap().owner, expected);
        }
    }

    #[test]
    fn paired_compile_sidecars_require_exact_full_invocation_and_closed_consumers() {
        let source = "#MM compile-owner : unit-a.o unit-b.o\n#MM depfile-owner : unit-a.d\n%rule_compile_multi basenames=\"unit-a unit-b\"\nunit-a.o unit-b.o unit-a.d unit-b.d : | obj\n";
        let rejected = "unit-a.o unit-b.o unit-a.d unit-b.d";
        let proofs = owners_fixture_with_verified(
            source,
            rejected,
            VerifiedMacros::from_forms(&[MacroForm::CompileMulti]),
        )
        .unwrap();
        assert_eq!(
            proofs
                .iter()
                .map(|proof| proof.owner.as_str())
                .collect::<Vec<_>>(),
            ["compile-owner", "depfile-owner"]
        );
        assert_eq!(proofs[0].chain, ["unit-a.o", "compile-owner"]);
        // The unconsumed unit-b.d is attributed through its paired object's
        // real chain; no synthetic `unit-b.d -> unit-b.o` edge is reported.
        assert!(!proofs
            .iter()
            .any(|proof| proof.chain.first().is_some_and(|id| id == "unit-b.d")));
        assert!(
            owners_fixture_with_verified(source, rejected, VerifiedMacros::default()).is_none()
        );

        let partial = "#MM compile-owner : unit-a.o unit-b.o\n%rule_compile_multi basenames=\"unit-a unit-b\"\nunit-a.o unit-a.d : | obj\n";
        assert!(owners_fixture_with_verified(
            partial,
            "unit-a.o unit-a.d",
            VerifiedMacros::from_forms(&[MacroForm::CompileMulti]),
        )
        .is_none());

        let extra = "#MM compile-owner : unit-a.o unit-b.o extra\n%rule_compile_multi basenames=\"unit-a unit-b\"\nunit-a.o unit-b.o unit-a.d unit-b.d extra : | obj\n";
        assert!(owners_fixture_with_verified(
            extra,
            "unit-a.o unit-b.o unit-a.d unit-b.d extra",
            VerifiedMacros::from_forms(&[MacroForm::CompileMulti]),
        )
        .is_none());

        let ambiguous = "#MM compile-owner : unit-a.o unit-b.o\n%rule_compile_multi basenames=\"unit-a unit-b\"\n%rule_compile_multi basenames=\"unit-a unit-b\"\nunit-a.o unit-b.o unit-a.d unit-b.d : | obj\n";
        assert!(owners_fixture_with_verified(
            ambiguous,
            rejected,
            VerifiedMacros::from_forms(&[MacroForm::CompileMulti]),
        )
        .is_none());

        let extra_recipe = "#MM compile-owner : unit-a.o unit-b.o\n%rule_compile_multi basenames=\"unit-a unit-b\"\nunit-a.o unit-b.o unit-a.d unit-b.d : | obj\n\t@printf alternate-producer\n";
        assert!(owners_fixture_with_verified(
            extra_recipe,
            rejected,
            VerifiedMacros::from_forms(&[MacroForm::CompileMulti]),
        )
        .is_none());

        let competing_pattern = "#MM compile-owner : unit-a.o unit-b.o\n%rule_compile_multi basenames=\"unit-a unit-b\"\nunit-a.o unit-b.o unit-a.d unit-b.d : | obj\n%.d : %.source\n";
        assert!(owners_fixture_with_verified(
            competing_pattern,
            rejected,
            VerifiedMacros::from_forms(&[MacroForm::CompileMulti]),
        )
        .is_none());

        let partial_objects = "#MM compile-owner : unit-a.o\n%rule_compile_multi basenames=\"unit-a unit-b\"\nunit-a.o unit-b.o unit-a.d unit-b.d : | obj\n";
        assert!(owners_fixture_with_verified(
            partial_objects,
            rejected,
            VerifiedMacros::from_forms(&[MacroForm::CompileMulti]),
        )
        .is_none());
    }

    #[test]
    fn paired_compile_sidecars_reject_unknown_or_cyclic_depfile_consumers() {
        let unknown = "#MM compile-owner : unit-a.o unit-b.o\n%rule_compile_multi basenames=\"unit-a unit-b\"\nunit-a.o unit-b.o unit-a.d unit-b.d : | obj\nifeq ($(UNKNOWN),yes)\n#MM conditional-depfile-owner : unit-a.d\nendif\n";
        assert!(owners_fixture_with_verified(
            unknown,
            "unit-a.o unit-b.o unit-a.d unit-b.d",
            VerifiedMacros::from_forms(&[MacroForm::CompileMulti]),
        )
        .is_none());

        let cycle = "#MM compile-owner : unit-a.o unit-b.o\n#MM depfile-owner : unit-a.d\n#MM unit-a.d : depfile-owner\n%rule_compile_multi basenames=\"unit-a unit-b\"\nunit-a.o unit-b.o unit-a.d unit-b.d : | obj\n";
        let (graph, rejected_line, outputs) = source_graph_fixture(
            cycle,
            "unit-a.o unit-b.o unit-a.d unit-b.d",
            VerifiedMacros::from_forms(&[MacroForm::CompileMulti]),
        )
        .unwrap();
        assert!(trace_all_owners(&graph, "unit-a.d", &mut DiagnosticBudget::default()).is_none());
        assert!(attribute_graph_outputs(&graph, rejected_line, &outputs).is_none());
    }

    #[test]
    fn predecessor_trace_preserves_longest_supported_chain_and_sorted_ties() {
        let mut source = String::from("unit.o: source.c\n");
        for index in 0..256 {
            let target = format!("chain-{index:03}");
            let prerequisite = if index == 0 {
                "unit.o".to_owned()
            } else {
                format!("chain-{:03}", index - 1)
            };
            writeln!(source, "{target}: {prerequisite}").unwrap();
        }
        source.push_str("#MM long-owner : chain-255\n");
        let proof = fixture(&source, "unit.o", VerifiedMacros::default()).unwrap();
        assert_eq!(proof.owner, "long-owner");
        assert_eq!(proof.chain.len(), 258);
        assert_eq!(proof.chain.first().map(String::as_str), Some("unit.o"));
        assert_eq!(proof.chain.get(1).map(String::as_str), Some("chain-000"));
        assert_eq!(proof.chain.last().map(String::as_str), Some("long-owner"));

        let tied = "unit.o: source.c\nleft: unit.o\nright: unit.o\n#MM tie-owner : left\n#MM tie-owner : right\n";
        let proof = fixture(tied, "unit.o", VerifiedMacros::default()).unwrap();
        assert_eq!(proof.chain, ["unit.o", "left", "tie-owner"]);
    }

    #[test]
    fn predecessor_trace_bounds_many_roots_over_a_long_chain() {
        let output_names = (0..1_000)
            .map(|index| format!("output-{index:04}"))
            .collect::<Vec<_>>();
        let output_list = output_names.join(" ");
        let mut source = format!("{output_list}: source.c\nhub: {output_list}\n");
        for index in 0..16_000 {
            let target = format!("chain-{index:05}");
            let prerequisite = if index == 0 {
                "hub".to_owned()
            } else {
                format!("chain-{:05}", index - 1)
            };
            writeln!(source, "{target}: {prerequisite}").unwrap();
        }
        source.push_str("#MM stress-owner : chain-15999\n");

        let (graph, rejected_line, outputs) =
            source_graph_fixture(&source, &output_list, VerifiedMacros::default()).unwrap();
        assert_eq!(outputs.len(), 1_000);
        assert!(attribute_graph_outputs(&graph, rejected_line, &outputs).is_none());
    }

    #[test]
    fn returned_path_byte_limit_is_checked_before_materialization() {
        let (graph, _, _) = source_graph_fixture(
            "unit.o: source.c\narchive.a: unit.o\n#MM byte-owner : archive.a\n",
            "unit.o",
            VerifiedMacros::default(),
        )
        .unwrap();
        let mut budget = DiagnosticBudget {
            retained_path_bytes: MAX_DIAGNOSTIC_PATH_BYTES - 1,
            ..DiagnosticBudget::default()
        };
        assert!(trace_all_owners(&graph, "unit.o", &mut budget).is_none());
        assert_eq!(budget.retained_path_bytes, MAX_DIAGNOSTIC_PATH_BYTES - 1);
    }

    #[test]
    fn paired_fallback_does_not_reset_budget_after_ordinary_trace() {
        let source = "#MM compile-owner : unit-a.o unit-b.o\n#MM depfile-owner : unit-a.d\n%rule_compile_multi basenames=\"unit-a unit-b\"\nunit-a.o unit-b.o unit-a.d unit-b.d : | obj\n";
        let (graph, rejected_line, outputs) = source_graph_fixture(
            source,
            "unit-a.o unit-b.o unit-a.d unit-b.d",
            VerifiedMacros::from_forms(&[MacroForm::CompileMulti]),
        )
        .unwrap();

        assert!(trace_all_then_paired(
            &graph,
            &outputs,
            rejected_line,
            &mut DiagnosticBudget::default(),
        )
        .is_some());

        let mut nearly_exhausted = DiagnosticBudget {
            work: MAX_DIAGNOSTIC_WORK - 1,
            ..DiagnosticBudget::default()
        };
        assert!(
            trace_all_then_paired(&graph, &outputs, rejected_line, &mut nearly_exhausted,)
                .is_none()
        );
        assert_eq!(nearly_exhausted.work, MAX_DIAGNOSTIC_WORK);
    }

    #[test]
    fn paired_compile_sidecar_fallback_respects_macro_identity_budget() {
        let basenames = (0..=MAX_IDENTITIES / 2)
            .map(|index| format!("unit-{index}"))
            .collect::<Vec<_>>()
            .join(" ");
        let source = format!(
            "BASENAMES := {basenames}\n#MM compile-owner : probe.o\n%rule_compile_multi basenames=\"$(BASENAMES)\" targetdir=gen\nprobe.o probe.d : | gen\n"
        );
        assert!(owners_fixture_with_verified(
            &source,
            "probe.o probe.d",
            VerifiedMacros::from_forms(&[MacroForm::CompileMulti]),
        )
        .is_none());

        let long_targetdir = "x".repeat(4_000);
        let many_basenames = (0..300)
            .map(|index| format!("u{index}"))
            .collect::<Vec<_>>()
            .join(" ");
        let args = closed_macro_arguments(&format!(
            "basenames=\"{many_basenames}\" targetdir={long_targetdir}"
        ))
        .unwrap();
        let root = tempdir().unwrap();
        let scope = collect_vars_impl("", Some(&crate::parser::TargetContext::default())).0;
        let dirs = DirVars::load(root.path());
        assert!(
            multi_compile_pairs(&args, &scope, &dirs, root.path(), Path::new("fixture"), 0)
                .is_empty()
        );

        let basenames = (0..300)
            .map(|index| format!("unit-{index}"))
            .collect::<Vec<_>>();
        let objects = basenames
            .iter()
            .map(|basename| format!("{basename}.o"))
            .collect::<Vec<_>>();
        let rejected = basenames
            .iter()
            .flat_map(|basename| [format!("{basename}.o"), format!("{basename}.d")])
            .collect::<Vec<_>>()
            .join(" ");
        let source = format!(
            "#MM compile-owner : all-objects\nall-objects: {}\n%rule_compile_multi basenames=\"{}\"\n{} : | obj\n",
            objects.join(" "),
            basenames.join(" "),
            rejected,
        );
        let proofs = owners_fixture_with_verified(
            &source,
            &rejected,
            VerifiedMacros::from_forms(&[MacroForm::CompileMulti]),
        )
        .expect("a finite shared predecessor graph stays below the work/path limits");
        assert_eq!(proofs.len(), 1);
        assert_eq!(proofs[0].owner, "compile-owner");
        assert_eq!(
            proofs[0].chain,
            ["unit-0.o", "all-objects", "compile-owner"]
        );
    }

    #[test]
    fn link_binary_does_not_bypass_unsealed_program_context() {
        // CURDIR is the independently selected declaring directory, not a
        // source-local override. The fixture always declares from `fixture`.
        let source = "GENDIR := generated\nCURDIR := arch/boot\n%build_prog mmake=bootstrap-owner progname=bootstrap files=bootstrap\n%rule_link_binary mmake=bootstrap-owner file=obj/vesa.bin.o name=vesa files=vesa asmfiles=loader\ngenerated/fixture/vesa.o: src/vesa.c\ngenerated/fixture/loader.o: src/loader.S\ngenerated/arch/boot/vesa.o: src/vesa.c\n#MM binary-owner : obj/vesa.bin.o\n";
        let macros = VerifiedMacros::from_forms(&[MacroForm::BuildProg, MacroForm::LinkBinary]);
        for output in ["generated/fixture/vesa.o", "generated/fixture/loader.o"] {
            assert!(fixture(source, output, macros).is_none());
        }
        assert!(fixture(source, "generated/arch/boot/vesa.o", macros).is_none());

        let missing_program = "%rule_link_binary mmake=bootstrap-owner file=obj/vesa.bin.o name=vesa files=vesa\nobj/vesa.o: src/vesa.c\n#MM binary-owner : obj/vesa.bin.o\n";
        assert!(fixture(
            missing_program,
            "obj/vesa.o",
            VerifiedMacros::from_forms(&[MacroForm::LinkBinary])
        )
        .is_none());
    }

    #[test]
    fn duplicate_macro_arguments_cannot_prove_paired_sidecar_ownership() {
        for arguments in [
            "basenames=\"unit-a\" basenames=\"unit-b\"",
            "basenames=\"unit-a\" targetdir= targetdir=obj",
            "basenames=\"unit-a\" usetree=no usetree=yes",
            "basenames=\"unit-a\" srcdir=one srcdir=two",
            "basenames=\"unit-a\" mmake=TMP mmake=other",
        ] {
            let source = format!(
                "#MM compile-owner : unit-a.o\n%rule_compile_multi {arguments}\nunit-a.o unit-a.d : | obj\n"
            );
            assert!(
                owners_fixture_with_verified(
                    &source,
                    "unit-a.o unit-a.d",
                    VerifiedMacros::from_forms(&[MacroForm::CompileMulti]),
                )
                .is_none(),
                "{arguments}"
            );
        }
    }

    #[test]
    fn closed_macro_arguments_ignore_quoted_and_nested_fake_keywords() {
        let arguments = closed_macro_arguments(
            r#"cflags="$(if 1, 'basenames=ghost targetdir=evil mmake=fake',)" basenames="$(addsuffix .c,$(FILES))" targetdir="$(if $(USE),obj,targetdir=other)" mmake="$(if 1,real-owner,mmake=fake)""#,
        )
        .unwrap();
        assert_eq!(
            arguments.keys().map(String::as_str).collect::<Vec<_>>(),
            ["basenames", "cflags", "mmake", "targetdir"]
        );
        assert_eq!(
            macro_value(&arguments, "mmake"),
            Some("$(if 1,real-owner,mmake=fake)")
        );
        assert_eq!(
            macro_value(&arguments, "targetdir"),
            Some("$(if $(USE),obj,targetdir=other)")
        );
    }

    #[test]
    fn hash_bound_macro_contracts_reject_unknown_or_missing_required_arguments() {
        assert_eq!(
            macro_value(
                &closed_macro_arguments("basenames='unit-a'").unwrap(),
                "basenames"
            ),
            Some("'unit-a'")
        );
        assert!(closed_macro_arguments("basenames=unit-a unit-b").is_none());
        // Python's GenMF lexer recognizes Unicode whitespace. Do not let
        // the narrower byte lexer consume a second token as part of a value.
        for separator in ['\u{00a0}', '\u{2003}', '\u{2028}'] {
            assert!(closed_macro_arguments(&format!(
                "basenames=unit-a{separator}unit-b targetdir=obj"
            ))
            .is_none());
        }

        let build_prog = test_macro_contract(MacroForm::BuildProg as usize).unwrap();
        assert!(build_prog["mmake"].required);
        assert!(build_prog["progname"].required);
        let compile_multi = test_macro_contract(MacroForm::CompileMulti as usize).unwrap();
        assert!(compile_multi["basenames"].required);
        let link_binary = test_macro_contract(MacroForm::LinkBinary as usize).unwrap();
        assert!(link_binary["file"].required);
        assert!(link_binary["name"].required);

        let compile = VerifiedMacros::from_forms(&[MacroForm::CompileMulti]);
        for arguments in [
            "basenames=unit-a targetdir=obj unexpected=ignored",
            "basenames=unit-a # ignored-comment",
            "targetdir=obj",
            "basenames=unit-a unit-b targetdir=obj",
            "basenames='unit-a' targetdir=obj",
        ] {
            let source = format!(
                "#MM compile-owner : obj/unit-a.o\n%rule_compile_multi {arguments}\nobj/unit-a.o obj/unit-a.d : | obj\n"
            );
            assert!(
                owners_fixture_with_verified(&source, "obj/unit-a.o obj/unit-a.d", compile)
                    .is_none(),
                "{arguments}"
            );
        }

        // progname is required by GenMF even if `files` would otherwise
        // provide an output stem to this bounded projection.
        let missing_progname = "obj/unit.o: src/unit.c\n#MM build-owner : obj/unit.o\n%build_prog mmake=build-owner files=src/unit objdir=obj\n";
        assert!(fixture(
            missing_progname,
            "obj/unit.o",
            VerifiedMacros::from_forms(&[MacroForm::BuildProg]),
        )
        .is_none());
    }

    fn synthetic_macro_definitions(template: &str) -> NativeMacroDefinitions {
        let (definitions, includes) = parse_native_macro_file(template).unwrap();
        assert!(includes.is_empty());
        let mut result = NativeMacroDefinitions::new();
        for (name, definition) in definitions {
            result.entry(name).or_default().push(definition);
        }
        result
    }

    fn synthetic_macro_hashes(definitions: &NativeMacroDefinitions) -> BTreeMap<String, String> {
        definitions
            .iter()
            .filter(|(_, matches)| matches.len() == 1)
            .map(|(name, matches)| (name.clone(), matches[0].sha256.clone()))
            .collect()
    }

    #[test]
    fn macro_closure_detects_direct_and_transitive_helper_mutations() {
        let original = "%define root value=\n%helper value=yes\n%end\n%define helper value=\nhelper-output: item\n%end\n%define leaf value=\nleaf-output: item\n%end\n";
        let trusted = synthetic_macro_definitions(original);
        let hashes = synthetic_macro_hashes(&trusted);
        assert!(verified_macro_closure_with_hashes(
            "root",
            &trusted,
            |name| { hashes.get(name).cloned() }
        ));

        let direct_mutation = original.replace("helper-output: item", "changed-output: item");
        let direct_definitions = synthetic_macro_definitions(&direct_mutation);
        assert_eq!(
            trusted["root"][0].sha256, direct_definitions["root"][0].sha256,
            "outer definition remains byte-identical"
        );
        assert!(!verified_macro_closure_with_hashes(
            "root",
            &direct_definitions,
            |name| hashes.get(name).cloned()
        ));

        let transitive = "%define root value=\n%helper value=yes\n%end\n%define helper value=\n%leaf value=yes\n%end\n%define leaf value=\nleaf-output: item\n%end\n";
        let transitive_trusted = synthetic_macro_definitions(transitive);
        let transitive_hashes = synthetic_macro_hashes(&transitive_trusted);
        assert!(verified_macro_closure_with_hashes(
            "root",
            &transitive_trusted,
            |name| transitive_hashes.get(name).cloned()
        ));
        let leaf_mutation = transitive.replace("leaf-output: item", "changed-leaf: item");
        let leaf_definitions = synthetic_macro_definitions(&leaf_mutation);
        assert_eq!(
            transitive_trusted["root"][0].sha256, leaf_definitions["root"][0].sha256,
            "transitive mutation leaves the outer definition unchanged"
        );
        assert!(!verified_macro_closure_with_hashes(
            "root",
            &leaf_definitions,
            |name| transitive_hashes.get(name).cloned()
        ));
    }

    #[test]
    fn macro_closure_fails_closed_on_missing_duplicate_cycle_and_budget() {
        let missing = synthetic_macro_definitions("%define root value=\n%compile_q\n%end\n");
        let missing_hashes = synthetic_macro_hashes(&missing);
        assert!(!verified_macro_closure_with_hashes(
            "root",
            &missing,
            |name| missing_hashes.get(name).cloned()
        ));

        let duplicate = synthetic_macro_definitions(
            "%define root value=\n%helper\n%end\n%define helper value=\nfirst: input\n%end\n%define helper value=\nsecond: input\n%end\n",
        );
        let duplicate_hashes = synthetic_macro_hashes(&duplicate);
        let tab_duplicate = synthetic_macro_definitions(
            "%define root value=\n%helper\n%end\n%define helper value=\nfirst: input\n%end\n%define\thelper value=\nsecond: input\n%end\n",
        );
        assert_eq!(tab_duplicate["helper"].len(), 2);
        let tab_duplicate_hashes = synthetic_macro_hashes(&tab_duplicate);
        assert!(!verified_macro_closure_with_hashes(
            "root",
            &tab_duplicate,
            |name| tab_duplicate_hashes.get(name).cloned()
        ));
        assert!(!verified_macro_closure_with_hashes(
            "root",
            &duplicate,
            |name| duplicate_hashes.get(name).cloned()
        ));

        let cycle = synthetic_macro_definitions(
            "%define root value=\n%helper\n%end\n%define helper value=\n%root\n%end\n",
        );
        let cycle_hashes = synthetic_macro_hashes(&cycle);
        assert!(!verified_macro_closure_with_hashes(
            "root",
            &cycle,
            |name| { cycle_hashes.get(name).cloned() }
        ));

        let mut deep = String::new();
        for index in 0..=MAX_TEMPLATE_CLOSURE_DEPTH + 1 {
            let next = if index == MAX_TEMPLATE_CLOSURE_DEPTH + 1 {
                String::new()
            } else {
                format!("%node{}\n", index + 1)
            };
            writeln!(deep, "%define node{index} value=\n{next}%end").unwrap();
        }
        let deep_definitions = synthetic_macro_definitions(&deep);
        let deep_hashes = synthetic_macro_hashes(&deep_definitions);
        assert!(!verified_macro_closure_with_hashes(
            "node0",
            &deep_definitions,
            |name| deep_hashes.get(name).cloned()
        ));

        let repeated_callers =
            std::iter::repeat_n("%helper\n", MAX_TEMPLATE_CLOSURE_WORK).collect::<String>();
        let over_budget = format!(
            "%define root value=\n{repeated_callers}%end\n%define helper value=\nhelper-output: input\n%end\n"
        );
        let over_budget_definitions = synthetic_macro_definitions(&over_budget);
        let over_budget_hashes = synthetic_macro_hashes(&over_budget_definitions);
        assert!(!verified_macro_closure_with_hashes(
            "root",
            &over_budget_definitions,
            |name| over_budget_hashes.get(name).cloned()
        ));
    }

    #[test]
    fn verified_macro_arguments_accept_ordinary_double_quoted_flag_lists() {
        let source = "#MM compile-owner : obj/unit-a.o obj/unit-b.o\n%rule_compile_multi basenames=\"unit-a unit-b\" targetdir=obj cflags=\"-O2 -g\" cppflags=\"-DVALUE=2\"\nobj/unit-a.o obj/unit-b.o obj/unit-a.d obj/unit-b.d : | obj\n";
        let proofs = owners_fixture_with_verified(
            source,
            "obj/unit-a.o obj/unit-b.o obj/unit-a.d obj/unit-b.d",
            VerifiedMacros::from_forms(&[MacroForm::CompileMulti]),
        )
        .unwrap();
        assert_eq!(proofs[0].owner, "compile-owner");
    }

    #[test]
    fn default_and_explicit_macro_flags_cannot_hide_eval_side_effects() {
        let hidden_default = "define HIDDEN\n$(eval #MM hidden-owner : obj/unit.o)\nendef\nCPPFLAGS := $(HIDDEN)\n%rule_compile_multi basenames=unit targetdir=obj\n#MM compile-owner : obj/unit.o\nobj/unit.o obj/unit.d : | obj\n";
        let root = tempdir().unwrap();
        let isolated = with_clean_macro_configuration(
            "define HIDDEN\n$(eval #MM hidden-owner : obj/unit.o)\nendef\nCPPFLAGS := $(HIDDEN)\n",
        );
        let joined = crate::parser::join_continuations(&isolated);
        let (scope, _) = collect_vars_impl(&joined, Some(&crate::parser::TargetContext::default()));
        let dirs = DirVars::load(root.path());
        let contract = test_macro_contract(MacroForm::CompileMulti as usize).unwrap();
        assert!(effective_macro_arguments(
            "basenames=unit targetdir=obj",
            &contract,
            &scope,
            &dirs,
            root.path(),
            Path::new("fixture"),
            0,
        )
        .is_none());
        assert!(owners_fixture_with_verified(
            hidden_default,
            "obj/unit.o obj/unit.d",
            VerifiedMacros::from_forms(&[MacroForm::CompileMulti]),
        )
        .is_none());

        let hidden_explicit = "define HIDDEN\n$(eval #MM hidden-owner : obj/unit.o)\nendef\n%rule_compile_multi basenames=unit targetdir=obj cflags=$(HIDDEN)\n#MM compile-owner : obj/unit.o\nobj/unit.o obj/unit.d : | obj\n";
        assert!(owners_fixture_with_verified(
            hidden_explicit,
            "obj/unit.o obj/unit.d",
            VerifiedMacros::from_forms(&[MacroForm::CompileMulti]),
        )
        .is_none());
    }

    #[test]
    fn unmodeled_active_macro_vetoes_owner_attribution_even_with_literal_arguments() {
        let source =
            "object.o: source.c\n#MM object-owner : object.o\n%unverified_macro output=object.o\n";
        assert!(fixture(source, "object.o", VerifiedMacros::default()).is_none());

        let inactive =
            "ifeq (0,1)\n%unverified_macro output=object.o\nendif\nobject.o: source.c\n#MM object-owner : object.o\n";
        assert_eq!(
            fixture(inactive, "object.o", VerifiedMacros::default())
                .unwrap()
                .owner,
            "object-owner"
        );

        let inert =
            "define HIDDEN\n%unverified_macro output=object.o\nendef\nobject.o: source.c\n#MM object-owner : object.o\n";
        assert_eq!(
            fixture(inert, "object.o", VerifiedMacros::default())
                .unwrap()
                .owner,
            "object-owner"
        );
    }

    #[test]
    fn inline_genmf_macros_veto_but_make_comments_do_not() {
        let chain = "object.o: source.c\n#MM object-owner : object.o\n";
        let inline_unmodeled = format!("{chain}prefix %rule_compile output=object.o\n");
        assert!(fixture(&inline_unmodeled, "object.o", VerifiedMacros::default(),).is_none());

        let inline_modeled = format!("{chain}prefix %rule_compile_multi basenames=unit\n");
        assert!(fixture(
            &inline_modeled,
            "object.o",
            VerifiedMacros::from_forms(&[MacroForm::CompileMulti]),
        )
        .is_none());

        let comment = format!("{chain}# this is a Make comment %rule_compile output=object.o\n");
        assert_eq!(
            fixture(&comment, "object.o", VerifiedMacros::default())
                .unwrap()
                .owner,
            "object-owner"
        );
    }

    #[test]
    fn surviving_make_include_directives_veto_source_ownership() {
        let literal = "object.o: source.c\n#MM object-owner : object.o\ninclude fragment.mk\n";
        assert!(fixture(literal, "object.o", VerifiedMacros::default()).is_none());

        let variable = "FRAGMENT := fragment.mk\nobject.o: source.c\n#MM object-owner : object.o\ninclude $(FRAGMENT)\n";
        assert!(fixture(variable, "object.o", VerifiedMacros::default()).is_none());

        let comment = "object.o: source.c\n#MM object-owner : object.o\n# include fragment.mk\n";
        assert_eq!(
            fixture(comment, "object.o", VerifiedMacros::default())
                .unwrap()
                .owner,
            "object-owner"
        );
    }

    #[test]
    fn malformed_closed_macro_argument_lists_are_unattributable() {
        for arguments in [
            "basenames=\"unit-a\" targetdir",
            "basenames=\"unit-a\" targetdir=\"unterminated",
            "basenames=$(if 1,unit-a,targetdir=evil",
            "basenames=\"unit-a\" basenames=\"unit-b\"",
            "basenames=unit-a\" targetdir=evil",
            "basenames=$(if 1,unit-a,${BROKEN))",
        ] {
            assert!(closed_macro_arguments(arguments).is_none(), "{arguments}");
        }
    }

    #[test]
    fn quoted_and_nested_fake_macro_fields_cannot_select_outputs_or_owners() {
        let compile = VerifiedMacros::from_forms(&[MacroForm::CompileMulti]);
        let fake_basenames = "#MM fake-owner : obj/ghost.o\n%rule_compile_multi cflags=\"$(if 1, basenames=ghost ,)\" basenames=\"unit-a\" targetdir=obj\nobj/ghost.o obj/ghost.d : | obj\n";
        assert!(
            owners_fixture_with_verified(fake_basenames, "obj/ghost.o obj/ghost.d", compile,)
                .is_none()
        );

        let fake_targetdir = "#MM fake-owner : evil/unit-a.o\n%rule_compile_multi cflags=\"$(if 1, targetdir=evil ,)\" basenames=\"unit-a\" targetdir=obj\nevil/unit-a.o evil/unit-a.d : | obj\n";
        assert!(owners_fixture_with_verified(
            fake_targetdir,
            "evil/unit-a.o evil/unit-a.d",
            compile,
        )
        .is_none());

        let fake_mmake = "obj/unit.o: src/unit.c\n%build_prog cflags=\"$(if 1, mmake=fake-owner ,)\" mmake=real-owner progname=app files=unit objdir=obj\n";
        let arguments = closed_macro_arguments(
            "cflags=\"$(if 1, mmake=fake-owner ,)\" mmake=real-owner progname=app files=unit objdir=obj",
        ).unwrap();
        assert_eq!(macro_value(&arguments, "mmake"), Some("real-owner"));
        assert!(fixture(
            fake_mmake,
            "obj/unit.o",
            VerifiedMacros::from_forms(&[MacroForm::BuildProg])
        )
        .is_none());

        let fake_binary = "obj/unit.o: src/unit.c\n#MM real-owner : bin/real.o\n%rule_link_binary ldflags=\"$(if 1, file=bin/fake.o ,)\" file=bin/real.o name=real objs=obj/unit.o mmake=link-namespace\n";
        assert_eq!(
            fixture(
                fake_binary,
                "obj/unit.o",
                VerifiedMacros::from_forms(&[MacroForm::LinkBinary]),
            )
            .unwrap()
            .owner,
            "real-owner"
        );
    }

    #[test]
    fn finite_multi_target_order_only_rule_returns_each_complete_owner() {
        let source = "one.o two.o: | order-only\none-archive.a: one.o\ntwo-archive.a: two.o\n#MM owner-one : one-archive.a\n#MM owner-two : two-archive.a\n";
        let proofs = owners_fixture(source, "one.o two.o").unwrap();
        assert_eq!(
            proofs
                .iter()
                .map(|proof| proof.owner.as_str())
                .collect::<Vec<_>>(),
            ["owner-one", "owner-two"]
        );
        let partial =
            "one.o two.o: | order-only\none-archive.a: one.o\n#MM owner-one : one-archive.a\n";
        assert!(owners_fixture(partial, "one.o two.o").is_none());
    }

    #[test]
    fn double_colon_rules_add_exact_prerequisite_edges() {
        let source = "shared.o:: src/first.c\nshared.o:: src/second.c\narchive.a: shared.o\n#MM archive-owner : archive.a\n";
        let proof = fixture(source, "shared.o", VerifiedMacros::default()).unwrap();
        assert_eq!(proof.owner, "archive-owner");

        let unsupported =
            "shared.o::: src/first.c\narchive.a: shared.o\n#MM archive-owner : archive.a\n";
        assert!(fixture(unsupported, "shared.o", VerifiedMacros::default()).is_none());
    }

    #[test]
    fn one_stem_pattern_rules_match_only_finite_source_identities() {
        let source = "OBJDIR := obj\nEXEDIR := bin\nTOOL := build-tool\nEXES := bin/app\nbuild-tool.a: obj/tool.o\nbin/app: bin/app.o\n$(EXEDIR)/% : $(OBJDIR)/%.o $(TOOL).a | $(EXEDIR)\n#MM app-owner : $(EXES)\n";
        let proof = fixture(source, "build-tool.a", VerifiedMacros::default()).unwrap();
        assert_eq!(proof.owner, "app-owner");
        assert!(proof.chain.contains(&"bin/app".to_owned()));

        let no_finite_target = "OBJDIR := obj\nEXEDIR := bin\nTOOL := build-tool\nbuild-tool.a: obj/tool.o\n$(EXEDIR)/% : $(OBJDIR)/%.o $(TOOL).a | $(EXEDIR)\n";
        assert!(fixture(no_finite_target, "build-tool.a", VerifiedMacros::default()).is_none());
    }

    #[test]
    fn unsupported_multi_stem_pattern_vetoes_full_snapshot() {
        let source = "object.o: input.c\narchive.a: object.o\n#MM owner : archive.a\nbin/%/sub% : object.o\n";
        assert!(fixture(source, "object.o", VerifiedMacros::default()).is_none());
    }

    #[test]
    fn one_stem_pattern_closure_has_a_hard_work_budget() {
        let mut graph = SourceGraph::default();
        graph
            .make_identities
            .extend((0..300).map(|index| format!("file-{index}")));
        graph.patterns.extend((0..300).map(|index| PatternRule {
            target: format!("target-{index}/%"),
            prerequisites: vec!["input-%.o".to_owned()],
        }));
        instantiate_pattern_edges(&mut graph);
        assert!(graph.uncertain);
        assert!(graph.edge_count <= MAX_IDENTITIES);
    }

    #[test]
    fn unowned_consumer_branch_and_cycles_refuse_partial_attribution() {
        let unowned_branch = "object.o: input.c\narchive.a: object.o\n#MM owner-a : archive.a\norphan-target: object.o\n";
        assert!(fixture(unowned_branch, "object.o", VerifiedMacros::default()).is_none());

        let cycle = "object.o: input.c\nfirst: object.o\nobject.o: first\narchive.a: object.o\n#MM owner-a : archive.a\n";
        assert!(fixture(cycle, "object.o", VerifiedMacros::default()).is_none());
    }

    #[test]
    fn distinct_owners_for_one_output_are_ambiguous() {
        let source = "obj/shared.o: src/shared.c\n%build_prog mmake=first progname=one files=src/shared objdir=obj\n%build_prog mmake=second progname=two files=src/shared objdir=obj\n";
        assert!(fixture(
            source,
            "obj/shared.o",
            VerifiedMacros::from_forms(&[MacroForm::BuildProg]),
        )
        .is_none());
    }

    #[test]
    fn disabled_tiff_style_meta_bridge_does_not_own_neighboring_rule() {
        let source = "pkgconfig/libtiff.pc: libtiff.pc.in\n\t@$(ECHO) generated >$@\n##MM workbench-libs-tiff-pkgconfig : pkgconfig/libtiff.pc\n#MM workbench-libs-tiff : linklibs-tiff\n";
        assert!(fixture(source, "pkgconfig/libtiff.pc", VerifiedMacros::default(),).is_none());
    }

    #[test]
    fn unresolved_or_conditional_edges_never_bridge_owners() {
        let unresolved = "obj/unit.o: src/unit.c\n#MM owner : $(UNKNOWN_PREREQUISITES)\n";
        assert!(fixture(unresolved, "obj/unit.o", VerifiedMacros::default()).is_none());

        let unresolved_target =
            "$(UNKNOWN_OUTPUT).o: src/unit.c\n#MM owner : $(UNKNOWN_OUTPUT).o\n";
        assert!(fixture(
            unresolved_target,
            "$(UNKNOWN_OUTPUT).o",
            VerifiedMacros::default(),
        )
        .is_none());

        let joined = crate::parser::join_continuations(
            "obj/unit.o: src/unit.c\nifeq ($(UNKNOWN),yes)\n#MM owner : obj/unit.o\nendif\n",
        );
        let root = tempdir().unwrap();
        let (scope, states) = collect_vars_impl(&joined, None);
        let dirs = DirVars::load(root.path());
        let output_line = 1;
        assert!(attribute_with_verified_macros(
            &joined,
            &scope,
            &dirs,
            (root.path(), Path::new("fixture")),
            Some(&states),
            ("obj/unit.o", output_line),
            VerifiedMacros::default(),
        )
        .is_none());
    }

    #[test]
    fn known_owner_is_not_unique_when_unknown_conditional_consumer_may_exist() {
        let source = "obj/unit.o: src/unit.c\narchive.a: obj/unit.o\n#MM owner-a : archive.a\nifeq ($(UNKNOWN),yes)\n#MM owner-b : obj/unit.o\nendif\n";
        assert!(fixture(source, "obj/unit.o", VerifiedMacros::default()).is_none());
    }

    #[test]
    fn unknown_conditional_rule_cannot_hide_a_second_consumer() {
        let source = "obj/unit.o: src/unit.c\narchive.a: obj/unit.o\n#MM owner-a : archive.a\nifeq ($(UNKNOWN),yes)\nconditional-archive.a: obj/unit.o\nendif\n#MM owner-b : conditional-archive.a\n";
        assert!(fixture(source, "obj/unit.o", VerifiedMacros::default()).is_none());
    }

    #[test]
    fn unknown_conditional_macro_cannot_hide_a_second_consumer() {
        let source = "obj/unit.o: src/unit.c\narchive.a: obj/unit.o\n#MM owner-a : archive.a\nifeq ($(UNKNOWN),yes)\n%rule_link_binary mmake=owner-b file=bin/conditional.o objs=obj/unit.o\nendif\n#MM owner-b : bin/conditional.o\n";
        assert!(fixture(
            source,
            "obj/unit.o",
            VerifiedMacros::from_forms(&[MacroForm::LinkBinary]),
        )
        .is_none());
    }

    #[test]
    fn unresolved_active_second_consumer_invalidates_known_owner() {
        let source = "obj/unit.o: src/unit.c\narchive.a: obj/unit.o\n#MM owner-a : archive.a\nconditional-archive.a: $(UNKNOWN_OBJECTS)\n#MM owner-b : conditional-archive.a\n";
        assert!(fixture(source, "obj/unit.o", VerifiedMacros::default()).is_none());
    }

    #[test]
    fn unresolved_active_target_invalidates_known_owner() {
        let source = "obj/unit.o: src/unit.c\narchive.a: obj/unit.o\n#MM owner-a : archive.a\n$(UNKNOWN_ARCHIVES): obj/unit.o\n";
        assert!(fixture(source, "obj/unit.o", VerifiedMacros::default()).is_none());
    }

    #[test]
    fn native_macro_verification_fails_closed_on_changed_definition() {
        let changed =
            "%define rule_compile_multi mmake=TMP\n%(mmake)_MC_TARGETS := changed\n%end\n";
        assert!(!verified_macro(
            changed,
            "rule_compile_multi",
            COMPILE_MULTI_SHA256
        ));
    }
}
