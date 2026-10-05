//! Source-owned native MetaMake invocation evidence, never a producer factory.
//!
//! A policy explicitly selects a closed native context. It is not a claim that
//! configure was executed or that generated classic global files were read.

use crate::genmf_projection::{expand_files, Limits as GenmfLimits};
use crate::metamake_owner_graph::{Limits, MetaMakeOwnerGraph, Selection};
use crate::metamake_project::{ConfiguredProject, ProjectGlobals};
use crate::parser::TargetContext;
use aros_common::native_build_contract::LoadedNativeBuildContract;
use aros_common::native_build_contract::NATIVE_RESERVED_MAKE_VARIABLES;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

const MAX_TOTAL_BYTES: usize = 128 * 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    schema_version: u32,
    kind: String,
    profile: String,
    project: String,
    configuration_source: String,
    template: String,
    /// Configure substitutions are explicit policy choices, never guessed.
    substitutions: Vec<Binding>,
    /// Ordered projected global snapshots: host precedes target.
    global_snapshots: Vec<GlobalSnapshot>,
    /// Exact exported environment values, with no ambient fallback.
    environment: Vec<Binding>,
    host_environment: Vec<HostEnvironment>,
    declared_absent: Vec<String>,
    evidence_sources: Vec<String>,
    closed_environment: bool,
    out_of_source: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    name: String,
    value: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GlobalSnapshot {
    source: String,
    configured_path: String,
    text: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HostEnvironment {
    host: String,
    bindings: Vec<Binding>,
}

/// Snapshots and complete metadata graph of one explicit native invocation.
#[derive(Debug)]
pub struct NativeOwnerProjection {
    graph: MetaMakeOwnerGraph,
    /// Canonical generated MetaMake owner -> actual source input.
    owners: BTreeMap<String, String>,
    /// Exact original bytes; checked against the main parser and recaptured.
    pub snapshots: BTreeMap<String, String>,
    pub policy_sha256: String,
    pub expanded_bytes: usize,
}

#[derive(Debug, Serialize)]
pub struct OwnerSelectionEvidence {
    pub qualification: &'static str,
    pub policy_sha256: String,
    pub selected_source_files: BTreeSet<String>,
    pub reached_targets: BTreeSet<String>,
    pub missing_endpoints: BTreeSet<String>,
    pub snapshot_count: usize,
    pub expanded_bytes: usize,
}

impl NativeOwnerProjection {
    /// Load sealed policy/config/templates and verify supplied discovery against
    /// a fresh MetaMake walk. Source `.src` regenerates its owner instead of
    /// trusting an old generated mmakefile. No command or ambient variable runs.
    ///
    /// # Errors
    /// Refuses policy ambiguity, unbound globals, changed/nonregular inputs,
    /// unsupported reference syntax and bounded resource excess.
    pub fn load(
        root: &Path,
        native: &LoadedNativeBuildContract,
        files: &[PathBuf],
        host: &str,
        context: &TargetContext,
    ) -> Result<Option<Self>, String> {
        let Some(path) = &native.contract.metamake_projection else {
            return Ok(None);
        };
        let sealed = sealed_inputs(native);
        let mut snapshots = BTreeMap::new();
        let (bytes, policy) = load_policy(root, path, native, &sealed, &mut snapshots)?;
        validate_policy_identity(&policy, &native.contract.profile, files.len())?;
        load_evidence_sources(root, &policy, &sealed, &mut snapshots)?;
        let project = load_project(root, &policy, &sealed, &mut snapshots)?;
        let (globals, absent) = load_globals(&policy, &project, host, context)?;
        let discovered = verify_complete_discovery(root, files, &project.ignored_directories)?;
        let template = load_template(root, &policy, &sealed, &mut snapshots)?;
        let owners = build_owner_map(discovered);
        let (expanded, expanded_bytes) =
            expand_owners(root, &owners, &template, &sealed, &mut snapshots)?;
        let limits = Limits {
            preserve_empty_endpoints: true,
            max_total_bytes: MAX_TOTAL_BYTES,
            ..Limits::default()
        };
        let graph = MetaMakeOwnerGraph::parse_with_declared_absence(
            &expanded,
            &globals.values,
            &absent,
            limits,
        )?;
        let projection = Self {
            graph,
            owners,
            snapshots,
            policy_sha256: aros_common::sha256_bytes(&bytes).to_string(),
            expanded_bytes,
        };
        projection.verify(root)?;
        Ok(Some(projection))
    }

    /// The complete source owner set, including virtual routes; no native
    /// producer or missing endpoint is inferred by this selection.
    ///
    /// # Errors
    /// Returns an error when graph traversal exceeds its resource limits or a
    /// selected owner has no corresponding discovered input.
    pub fn select(&self, roots: &[String]) -> Result<OwnerSelectionEvidence, String> {
        let Selection {
            reached_targets,
            missing_endpoints,
            selected_owner_files,
        } = self.graph.select(roots, Limits::default())?;
        let selected_source_files = selected_owner_files
            .iter()
            .map(|owner| {
                self.owners
                    .get(owner)
                    .cloned()
                    .ok_or_else(|| format!("owner has no bound input: {owner}"))
            })
            .collect::<Result<_, _>>()?;
        Ok(OwnerSelectionEvidence {
            qualification: "source-owner-metadata-not-producer-proof",
            policy_sha256: self.policy_sha256.clone(),
            selected_source_files,
            reached_targets,
            missing_endpoints,
            snapshot_count: self.snapshots.len(),
            expanded_bytes: self.expanded_bytes,
        })
    }

    /// Recheck every captured policy, recipe and imported template before use.
    ///
    /// # Errors
    /// Returns an error if any captured input is unreadable or changed.
    pub fn verify(&self, root: &Path) -> Result<(), String> {
        verify_snapshot_digests(root, &self.snapshots)
    }
}

fn sealed_inputs(native: &LoadedNativeBuildContract) -> BTreeMap<String, String> {
    native
        .contract
        .inputs
        .iter()
        .map(|input| (input.path.clone(), input.sha256.to_string()))
        .collect()
}

fn load_policy(
    root: &Path,
    path: &str,
    native: &LoadedNativeBuildContract,
    sealed: &BTreeMap<String, String>,
    snapshots: &mut BTreeMap<String, String>,
) -> Result<(Vec<u8>, Policy), String> {
    let bytes = read_snapshot(root, path, 64 * 1024, snapshots)?;
    require_seal(path, &bytes, sealed)?;
    let policy: Policy = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    if policy.profile != native.contract.profile {
        return Err("native MetaMake policy profile differs from native build contract".into());
    }
    Ok((bytes, policy))
}

fn validate_policy_identity(
    policy: &Policy,
    profile: &str,
    file_count: usize,
) -> Result<(), String> {
    if policy.schema_version != 1
        || policy.kind != "native-metamake-policy-v1"
        || policy.profile != profile
        || !policy.closed_environment
        || !policy.out_of_source
        || file_count > 20_000
    {
        return Err("native MetaMake policy identity or closed invocation is invalid".into());
    }
    if policy.evidence_sources.is_empty() || policy.evidence_sources.len() > 32 {
        return Err("native MetaMake policy requires bounded sealed source evidence".into());
    }
    let unique: BTreeSet<_> = policy.evidence_sources.iter().collect();
    if unique.len() != policy.evidence_sources.len() {
        return Err("duplicate native MetaMake evidence source".into());
    }
    Ok(())
}

fn load_evidence_sources(
    root: &Path,
    policy: &Policy,
    sealed: &BTreeMap<String, String>,
    snapshots: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    for source in &policy.evidence_sources {
        let bytes = read_snapshot(root, source, 1024 * 1024, snapshots)?;
        require_seal(source, &bytes, sealed)?;
    }
    Ok(())
}

fn load_project(
    root: &Path,
    policy: &Policy,
    sealed: &BTreeMap<String, String>,
    snapshots: &mut BTreeMap<String, String>,
) -> Result<ConfiguredProject, String> {
    let bytes = read_snapshot(root, &policy.configuration_source, 64 * 1024, snapshots)?;
    require_seal(&policy.configuration_source, &bytes, sealed)?;
    let source = String::from_utf8(bytes).map_err(|error| error.to_string())?;
    let config = substitute_configuration(&source, &policy.substitutions)?;
    let project = ConfiguredProject::parse(&config, &policy.project)?;
    if project.default_makefile != "mmakefile" || !project.added_makefiles.is_empty() {
        return Err("native MetaMake project discovery policy is unsupported".into());
    }
    validate_global_snapshot_paths(&project.global_variable_files, &policy.global_snapshots)?;
    Ok(project)
}

fn substitute_configuration(source: &str, substitutions: &[Binding]) -> Result<String, String> {
    let mut config = source.to_owned();
    let mut tokens = BTreeSet::new();
    for binding in substitutions {
        if binding.name.len() > 128
            || !binding.name.starts_with('@')
            || !binding.name.ends_with('@')
            || binding.value.contains(['@', '\n', '\r', '\0'])
            || !tokens.insert(&binding.name)
            || !config.contains(&binding.name)
        {
            return Err("invalid, duplicate or unused MetaMake substitution".into());
        }
        config = config.replace(&binding.name, &binding.value);
    }
    Ok(config)
}

fn validate_global_snapshot_paths(
    configured_paths: &[String],
    snapshots: &[GlobalSnapshot],
) -> Result<(), String> {
    if configured_paths.len() != snapshots.len() || snapshots.len() > 16 {
        return Err(
            "native MetaMake global snapshot count differs from project configuration".into(),
        );
    }
    let mut unique = BTreeSet::new();
    for (index, (configured, snapshot)) in configured_paths.iter().zip(snapshots).enumerate() {
        if configured != &snapshot.configured_path || !unique.insert(configured) {
            return Err(format!(
                "native MetaMake global snapshot {index} does not match its configured path/order"
            ));
        }
    }
    Ok(())
}

fn load_globals(
    policy: &Policy,
    project: &ConfiguredProject,
    host: &str,
    context: &TargetContext,
) -> Result<(ProjectGlobals, BTreeSet<String>), String> {
    let mut initial = project.variables.clone();
    add_bindings(&mut initial, &policy.environment)?;
    let selected_host = selected_host_environment(&policy.host_environment, host)?;
    add_bindings(&mut initial, &selected_host.bindings)?;
    let global_files = projected_global_files(policy)?;
    let globals = ProjectGlobals::parse(&initial, &global_files)?;
    let absent = declared_absent(policy)?;
    validate_context_selectors(&globals.values, &absent, context)?;
    Ok((globals, absent))
}

fn selected_host_environment<'a>(
    hosts: &'a [HostEnvironment],
    host: &str,
) -> Result<&'a HostEnvironment, String> {
    let identities: BTreeSet<_> = hosts.iter().map(|entry| &entry.host).collect();
    if identities.len() != hosts.len() || hosts.len() > 8 {
        return Err("ambiguous native MetaMake host bindings".into());
    }
    hosts
        .iter()
        .find(|entry| entry.host == host)
        .ok_or_else(|| "native MetaMake policy does not declare this host".into())
}

fn projected_global_files(policy: &Policy) -> Result<Vec<(String, String)>, String> {
    policy
        .global_snapshots
        .iter()
        .map(|snapshot| {
            if !policy.evidence_sources.contains(&snapshot.source) {
                return Err("projected globals have no sealed source provenance".to_owned());
            }
            Ok((snapshot.source.clone(), snapshot.text.clone()))
        })
        .collect()
}

fn declared_absent(policy: &Policy) -> Result<BTreeSet<String>, String> {
    let absent: BTreeSet<_> = policy.declared_absent.iter().cloned().collect();
    if absent.len() != policy.declared_absent.len() {
        return Err("duplicate explicitly absent MetaMake variable".into());
    }
    Ok(absent)
}

fn validate_context_selectors(
    globals: &BTreeMap<String, String>,
    absent: &BTreeSet<String>,
    context: &TargetContext,
) -> Result<(), String> {
    const REQUIRED: &[&str] = &[
        "AROS_TARGET_CPU",
        "CPU",
        "AROS_TARGET_ARCH",
        "ARCH",
        "FAMILY",
        "AROS_TARGET_VARIANT",
        "AROS_TOOLCHAIN",
        "AROS_TARGET_CPU32",
        "AROS_TARGET_PLATFORM",
    ];
    for name in REQUIRED {
        let expected = context.value_of(name).ok_or_else(|| {
            format!("native target context lacks required MetaMake selector {name}")
        })?;
        compare_selector(globals, absent, name, &expected, true)?;
    }
    for name in NATIVE_RESERVED_MAKE_VARIABLES {
        if REQUIRED.contains(name) {
            continue;
        }
        if let Some(expected) = context.value_of(name) {
            compare_selector(globals, absent, name, &expected, false)?;
        }
    }
    Ok(())
}

fn compare_selector(
    globals: &BTreeMap<String, String>,
    absent: &BTreeSet<String>,
    name: &str,
    expected: &str,
    required: bool,
) -> Result<(), String> {
    if absent.contains(name) {
        return Err(format!(
            "native MetaMake selector {name} is both context-bound and declared absent"
        ));
    }
    match globals.get(name) {
        Some(actual) if actual == expected => Ok(()),
        Some(_) => Err(format!(
            "native MetaMake selector {name} differs from target context"
        )),
        None if required => Err(format!(
            "native MetaMake policy does not bind required selector {name}"
        )),
        None => Ok(()),
    }
}

fn verify_complete_discovery(
    root: &Path,
    files: &[PathBuf],
    ignored_directories: &BTreeSet<String>,
) -> Result<BTreeSet<String>, String> {
    if files.len() > 20_000 {
        return Err("native MetaMake discovery exceeds file budget".into());
    }
    let mut supplied = BTreeSet::new();
    for file in files {
        let name = meta_relative_name(root, file)?;
        if !path_has_ignored_directory(Path::new(&name), ignored_directories)
            && !supplied.insert(name.to_owned())
        {
            return Err(format!("duplicate discovered MetaMake input: {name}"));
        }
    }
    let expected = discover_meta_files(root, ignored_directories)?;
    if expected != supplied {
        let missing: Vec<_> = expected.difference(&supplied).take(5).cloned().collect();
        let extra: Vec<_> = supplied.difference(&expected).take(5).cloned().collect();
        return Err(format!(
            "MetaMake input discovery is incomplete; omitted={missing:?}, unexpected={extra:?}"
        ));
    }
    Ok(expected)
}

fn discover_meta_files(
    root: &Path,
    ignored_directories: &BTreeSet<String>,
) -> Result<BTreeSet<String>, String> {
    let walker = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            entry.depth() == 0
                || !entry.file_type().is_dir()
                || !entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| ignored_directories.contains(name))
        });
    let mut files = BTreeSet::new();
    for entry in walker {
        let entry = entry.map_err(|error| format!("cannot verify MetaMake discovery: {error}"))?;
        if entry.file_type().is_symlink()
            && !is_ignored_name(entry.file_name(), ignored_directories)
        {
            if fs::metadata(entry.path()).is_ok_and(|metadata| metadata.is_dir()) {
                return Err(format!(
                    "MetaMake discovery cannot follow source directory symlink: {}",
                    entry.path().display()
                ));
            }
        }
        if is_meta_make_name(entry.file_name()) {
            let name = meta_relative_name(root, entry.path())?.to_owned();
            if !files.insert(name.clone()) {
                return Err(format!("duplicate filesystem MetaMake input: {name}"));
            }
            if files.len() > 20_000 {
                return Err("native MetaMake discovery exceeds file budget".into());
            }
        }
    }
    Ok(files)
}

fn build_owner_map(discovered: BTreeSet<String>) -> BTreeMap<String, String> {
    let mut owners = BTreeMap::new();
    for name in discovered {
        let source = name.ends_with("/mmakefile.src") || name == "mmakefile.src";
        let owner = if source {
            name.strip_suffix(".src").unwrap()
        } else {
            &name
        };
        if source || !owners.contains_key(owner) {
            owners.insert(owner.to_owned(), name);
        }
    }
    owners
}

fn load_template(
    root: &Path,
    policy: &Policy,
    sealed: &BTreeMap<String, String>,
    snapshots: &mut BTreeMap<String, String>,
) -> Result<PathBuf, String> {
    let bytes = read_snapshot(
        root,
        &policy.template,
        GenmfLimits::default().max_template_bytes as u64,
        snapshots,
    )?;
    require_seal(&policy.template, &bytes, sealed)?;
    aros_common::canonical_source_file(root, Path::new(&policy.template))
        .map_err(|error| error.to_string())
}

fn expand_owners(
    root: &Path,
    owners: &BTreeMap<String, String>,
    template: &Path,
    sealed: &BTreeMap<String, String>,
    snapshots: &mut BTreeMap<String, String>,
) -> Result<(BTreeMap<String, String>, usize), String> {
    let mut expanded = BTreeMap::new();
    let mut expanded_bytes = 0usize;
    for (owner, source) in owners {
        let bytes = read_snapshot(root, source, 1024 * 1024, snapshots)?;
        let text = if source.ends_with(".src") {
            let expansion = expand_files(&root.join(source), template, GenmfLimits::default())
                .map_err(|error| error.to_string())?;
            retain_template_snapshots(root, expansion.template_snapshots, sealed, snapshots)?;
            expansion.text
        } else {
            String::from_utf8(bytes.clone())
                .unwrap_or_else(|_| bytes.into_iter().map(char::from).collect())
        };
        expanded_bytes = expanded_bytes
            .checked_add(text.len())
            .filter(|total| *total <= MAX_TOTAL_BYTES)
            .ok_or("native MetaMake expanded corpus exceeds 128 MiB")?;
        expanded.insert(owner.clone(), text);
    }
    Ok((expanded, expanded_bytes))
}

fn retain_template_snapshots(
    root: &Path,
    inputs: Vec<crate::genmf_projection::TemplateSnapshot>,
    sealed: &BTreeMap<String, String>,
    snapshots: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    for input in inputs {
        let path = meta_relative_name(root, &input.path)?.to_owned();
        require_seal(&path, &input.bytes, sealed)?;
        retain_snapshot(snapshots, &path, &input.bytes)?;
    }
    Ok(())
}

fn verify_snapshot_digests(
    root: &Path,
    snapshots: &BTreeMap<String, String>,
) -> Result<(), String> {
    for (path, digest) in snapshots {
        let mut recaptured = BTreeMap::new();
        let bytes = read_snapshot(root, path, 8 * 1024 * 1024, &mut recaptured)?;
        if aros_common::sha256_bytes(&bytes).as_str() != digest {
            return Err(format!("MetaMake projection input changed: {path}"));
        }
    }
    Ok(())
}

fn meta_relative_name<'a>(root: &Path, path: &'a Path) -> Result<&'a str, String> {
    path.strip_prefix(root)
        .map_err(|error| error.to_string())?
        .to_str()
        .ok_or_else(|| "non-UTF-8 MetaMake source path".into())
}

fn path_has_ignored_directory(path: &Path, ignored_directories: &BTreeSet<String>) -> bool {
    path.components().any(|component| {
        ignored_directories
            .iter()
            .any(|ignored| component.as_os_str() == std::ffi::OsStr::new(ignored))
    })
}

fn is_ignored_name(name: &std::ffi::OsStr, ignored_directories: &BTreeSet<String>) -> bool {
    name.to_str()
        .is_some_and(|name| ignored_directories.contains(name))
}

fn is_meta_make_name(name: &std::ffi::OsStr) -> bool {
    matches!(name.to_str(), Some("mmakefile.src" | "mmakefile"))
}

fn add_bindings(values: &mut BTreeMap<String, String>, bindings: &[Binding]) -> Result<(), String> {
    if bindings.len() > 128 {
        return Err("MetaMake environment binding budget exceeded".into());
    }
    for binding in bindings {
        if values
            .insert(binding.name.clone(), binding.value.clone())
            .is_some()
        {
            return Err(format!(
                "duplicate MetaMake environment binding: {}",
                binding.name
            ));
        }
    }
    Ok(())
}

fn read_snapshot(
    root: &Path,
    relative: &str,
    limit: u64,
    snapshots: &mut BTreeMap<String, String>,
) -> Result<Vec<u8>, String> {
    let path = aros_common::canonical_source_file(root, Path::new(relative))
        .map_err(|error| error.to_string())?;
    let (_, bytes) = aros_common::measure_regular_file_bounded(&path, limit)
        .map_err(|error| error.to_string())?
        .ok_or("missing MetaMake projection input")?;
    retain_snapshot(snapshots, relative, &bytes)?;
    Ok(bytes)
}

fn retain_snapshot(
    snapshots: &mut BTreeMap<String, String>,
    path: &str,
    bytes: &[u8],
) -> Result<(), String> {
    let digest = aros_common::sha256_bytes(bytes).to_string();
    if snapshots.get(path).is_some_and(|old| old != &digest) {
        return Err(format!(
            "MetaMake projection input changed during expansion: {path}"
        ));
    }
    snapshots.insert(path.to_owned(), digest);
    Ok(())
}

fn require_seal(path: &str, bytes: &[u8], seals: &BTreeMap<String, String>) -> Result<(), String> {
    if seals
        .get(path)
        .is_none_or(|digest| digest != aros_common::sha256_bytes(bytes).as_str())
    {
        return Err(format!(
            "native MetaMake policy input is not sealed: {path}"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selector_globals() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("AROS_TARGET_CPU".into(), "riscv".into()),
            ("CPU".into(), "riscv".into()),
            ("AROS_TARGET_ARCH".into(), "esp32p4".into()),
            ("ARCH".into(), "esp32p4".into()),
            ("AROS_TARGET_PLATFORM".into(), "esp32p4-riscv".into()),
            ("FAMILY".into(), String::new()),
            ("AROS_TARGET_VARIANT".into(), String::new()),
            ("AROS_TOOLCHAIN".into(), "gnu".into()),
            ("AROS_TARGET_CPU32".into(), String::new()),
        ])
    }

    fn selector_context() -> TargetContext {
        TargetContext {
            cpu: Some("riscv".into()),
            platform: Some("esp32p4".into()),
            family: Some(String::new()),
            variant: Some(String::new()),
            toolchain: Some("gnu".into()),
            cpu32: Some(String::new()),
            ..TargetContext::default()
        }
    }

    #[test]
    fn global_snapshots_bind_exact_configured_paths_in_order() {
        let configured = vec!["host.cfg".into(), "target.cfg".into()];
        let snapshots = vec![
            GlobalSnapshot {
                source: "host.cfg.in".into(),
                configured_path: "host.cfg".into(),
                text: String::new(),
            },
            GlobalSnapshot {
                source: "target.cfg.in".into(),
                configured_path: "target.cfg".into(),
                text: String::new(),
            },
        ];
        assert!(validate_global_snapshot_paths(&configured, &snapshots).is_ok());
        assert!(validate_global_snapshot_paths(
            &configured,
            &snapshots.into_iter().rev().collect::<Vec<_>>()
        )
        .is_err());
    }

    #[test]
    fn native_policy_selectors_must_match_profile_context() {
        let globals = selector_globals();
        let context = selector_context();
        assert!(validate_context_selectors(&globals, &BTreeSet::new(), &context).is_ok());

        let mut wrong = globals;
        wrong.insert("ARCH".into(), "riscv".into());
        assert!(
            validate_context_selectors(&wrong, &BTreeSet::new(), &context)
                .unwrap_err()
                .contains("ARCH differs")
        );
    }

    #[test]
    fn context_known_selector_cannot_be_declared_absent() {
        assert!(validate_context_selectors(
            &selector_globals(),
            &BTreeSet::from(["FAMILY".into()]),
            &selector_context(),
        )
        .unwrap_err()
        .contains("both context-bound and declared absent"));
    }

    #[test]
    fn missing_environment_name_is_not_silently_emptied() {
        let files = BTreeMap::from([("fixture/mmakefile".into(), "#MM root-$(UNKNOWN)\n".into())]);
        let error = MetaMakeOwnerGraph::parse_with_declared_absence(
            &files,
            &BTreeMap::new(),
            &BTreeSet::new(),
            Limits::default(),
        )
        .unwrap_err();
        assert!(error.contains("unbound"));
    }

    #[test]
    fn native_projection_keeps_empty_endpoint_as_missing_not_as_a_target() {
        let files = BTreeMap::from([("fixture/mmakefile".into(), "#MM- root : $(EMPTY)\n".into())]);
        let graph = MetaMakeOwnerGraph::parse_with_declared_absence(
            &files,
            &BTreeMap::from([("EMPTY".into(), String::new())]),
            &BTreeSet::new(),
            Limits {
                preserve_empty_endpoints: true,
                ..Limits::default()
            },
        )
        .unwrap();
        let selected = graph.select(&["root".into()], Limits::default()).unwrap();
        assert_eq!(selected.missing_endpoints, BTreeSet::from([String::new()]));
        assert!(!selected.reached_targets.contains(""));
    }

    #[test]
    fn policy_inputs_must_match_seals() {
        let seals = BTreeMap::from([(
            "policy.json".into(),
            aros_common::sha256_bytes(b"sealed policy").to_string(),
        )]);
        assert!(require_seal("policy.json", b"sealed policy", &seals).is_ok());
        assert!(require_seal("policy.json", b"changed policy", &seals).is_err());
    }

    #[test]
    fn source_and_template_snapshot_drift_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("config")).unwrap();
        fs::create_dir_all(root.path().join("module")).unwrap();
        fs::write(root.path().join("config/make.tmpl"), "template-v1").unwrap();
        fs::write(root.path().join("module/mmakefile.src"), "source-v1").unwrap();
        let expected = BTreeMap::from([
            (
                "config/make.tmpl".into(),
                aros_common::sha256_bytes(b"template-v1").to_string(),
            ),
            (
                "module/mmakefile.src".into(),
                aros_common::sha256_bytes(b"source-v1").to_string(),
            ),
        ]);
        assert!(verify_snapshot_digests(root.path(), &expected).is_ok());
        fs::write(root.path().join("module/mmakefile.src"), "source-v2").unwrap();
        assert!(verify_snapshot_digests(root.path(), &expected)
            .unwrap_err()
            .contains("module/mmakefile.src"));
        fs::write(root.path().join("module/mmakefile.src"), "source-v1").unwrap();
        fs::write(root.path().join("config/make.tmpl"), "template-v2").unwrap();
        assert!(verify_snapshot_digests(root.path(), &expected)
            .unwrap_err()
            .contains("config/make.tmpl"));
    }

    #[test]
    fn discovery_accepts_project_ignoredirs_but_refuses_main_pruned_build() {
        let root = tempfile::tempdir().unwrap();
        for directory in ["live", "vendor", "build"] {
            fs::create_dir_all(root.path().join(directory)).unwrap();
            fs::write(
                root.path().join(directory).join("mmakefile.src"),
                "#MM target\n",
            )
            .unwrap();
        }
        let ignored = BTreeSet::from(["vendor".into()]);
        let supplied = vec![
            root.path().join("live/mmakefile.src"),
            root.path().join("vendor/mmakefile.src"),
            root.path().join("build/mmakefile.src"),
        ];
        assert_eq!(
            verify_complete_discovery(root.path(), &supplied, &ignored).unwrap(),
            BTreeSet::from(["build/mmakefile.src".into(), "live/mmakefile.src".into()])
        );
        let main_pruned = vec![root.path().join("live/mmakefile.src")];
        let error = verify_complete_discovery(root.path(), &main_pruned, &ignored).unwrap_err();
        assert!(error.contains("build/mmakefile.src"));
        assert!(error.contains("omitted"));
    }
}
