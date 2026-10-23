//! Source-owned native MetaMake invocation evidence, never a producer factory.
//!
//! A policy explicitly selects a closed native context. It is not a claim that
//! configure was executed or that generated classic global files were read.

use crate::genmf_projection::{expand_bytes, Limits as GenmfLimits};
use crate::metamake_owner_graph::{Limits, MetaMakeOwnerGraph, Selection};
use crate::metamake_project::{
    is_ignored_directory_path, CapturedProjectDiscovery, ConfiguredProject, InventoryEntry,
    ProjectGlobalPathContext, ProjectGlobals,
};
use crate::parser::TargetContext;
use aros_common::native_build_contract::LoadedNativeBuildContract;
use aros_common::native_build_contract::NATIVE_RESERVED_MAKE_VARIABLES;
use aros_common::Diagnostic;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
#[cfg(test)]
use std::fs;
use std::path::{Path, PathBuf};

#[path = "native_arch_endpoint_effects.rs"]
mod arch_endpoint_effects;
#[path = "native_arch_lane_aliases.rs"]
mod arch_lane_aliases;
pub use arch_lane_aliases::VerifiedArchLaneAlias;
#[path = "native_assembly_headers.rs"]
mod assembly_headers;
#[path = "native_meta_semantics.rs"]
mod meta_semantics;
pub use meta_semantics::{NativeMetaSemanticsEvidence, NativeMetaSemanticsSelection};

mod child_context;
pub use child_context::MatchedOwnerChildCoordinates;
mod child_source;
mod metadata_selection;
#[cfg(test)]
mod metadata_selector_tests;
pub use child_source::{
    BoundChildRecipeBatch, CapturedChildSourceRequest, DeclaredChildJoinSelection,
    DeclaredChildOutputCandidate, DeclaredChildOutputLinks, DeclaredChildRecipe,
    DeclaredChildRecipeProjection, DeclaredIncludedMakefile, DeclaredIncludedRecipes,
    MatchedChildDeclaredRules, MatchedChildRecipePlan, MatchedChildSource, MatchedChildSourceFold,
    Mesa26OriginalRecipeBinding, NativeGenmoduleIncludePlan, RetainedChildSourceBatchLimits, RetainedChildSourceBatchRequest,
    RetainedChildSourceEffectBatch, RetainedChildSourceFoldLimits, SelectedChildRecipe,
};
pub use metadata_selection::{
    SourceMetadataHookOmission, SourceMetadataSelection, SourceMetadataSelectorOmission,
};
mod runtime_configuration;
pub use runtime_configuration::RuntimeConfigurationCheck;

#[cfg(test)]
#[path = "native_template_hook_tests.rs"]
mod template_hook_tests;

#[cfg(test)]
#[path = "native_meta_edge_origin_tests.rs"]
mod meta_edge_origin_tests;

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
    /// Closed source-owned permission for template-generated extension slots.
    /// This never applies to handwritten edges or concrete source owners.
    #[serde(default)]
    architecture_hook_families: Vec<String>,
    /// Mesa 26 source producer selections. The expressions remain bounded
    /// Make data; this policy layer never interprets or executes them.
    #[serde(default)]
    mesa26_original_producers: Vec<Mesa26OriginalProducerSelection>,
}

/// One Mesa 26 source-native producer selection, not an executable formula.
///
/// `producer`, `cooutput`, and `consumer` are bounded Make expressions for a
/// later Source-bound adapter to evaluate in the original target frame.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Mesa26OriginalProducerSelection {
    pub(crate) source_input: String,
    pub(crate) goal: String,
    pub(crate) producer: String,
    pub(crate) cooutput: String,
    pub(crate) consumer: String,
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
///
/// Loaded identities are immutable outside this module. In particular a caller
/// cannot disable source revalidation by clearing the original digest set:
///
/// ```compile_fail
/// use aros_transpiler::native_owner_projection::NativeOwnerProjection;
/// fn discard_binding(projection: &mut NativeOwnerProjection) {
///     projection.snapshots.clear();
/// }
/// ```
#[derive(Debug)]
pub struct NativeOwnerProjection {
    /// Configuration captured from the validated native source contract.
    /// Architecture effects replay with this exact context, not an arbitrary
    /// caller-provided or ambient configuration.
    architecture_context: TargetContext,
    /// Project globals the policy binds (target.cfg/host.cfg snapshots and
    /// environment), as MetaMake substitutes them into #MM names.
    metamake_globals: BTreeMap<String, String>,
    runtime_configuration: Option<RuntimeConfiguration>,
    graph: MetaMakeOwnerGraph,
    /// Expansion origins keyed by generated owner and directive-start line.
    /// A direct Make fragment intentionally has no template origin.
    expansion_origins: BTreeMap<(String, usize), crate::genmf_projection::ExpandedLineProvenance>,
    /// Raw byte digests of the deterministic Source/GenMF programs used for
    /// this projection. These are not prepared-tree CAS or producer receipts.
    expanded_program_sha256: BTreeMap<String, aros_common::Sha256Digest>,
    architecture_hook_families: BTreeSet<String>,
    /// Optional Mesa 26 recipe intentions, still unbound Make data.
    mesa26_original_producers: Vec<Mesa26OriginalProducerSelection>,
    /// Canonical generated MetaMake owner -> actual source input.
    owners: BTreeMap<String, String>,
    /// Complete discovered inputs at load time, excluding project-ignored directories.
    discovered_inputs: BTreeSet<String>,
    /// Logical discovery input -> original regular in-tree referent. This map
    /// is retained separately from content digests and rechecked before use.
    discovery_original_inputs: BTreeMap<String, String>,
    /// Project discovery policy applied to both the original and current walks.
    ignored_directories: BTreeSet<String>,
    /// Exact root-relative directories excluded from both walks (the calling
    /// engine's in-tree build directory); never matched by basename.
    excluded_paths: BTreeSet<String>,
    /// Exact original bytes; checked against the main parser and recaptured.
    snapshots: BTreeMap<String, String>,
    /// Source contract inputs available to configuration include replay.
    sealed_configuration_inputs: BTreeMap<String, String>,
    /// Explicit Source-contract selector edges; never inferred from names.
    optional_meta_dependencies:
        Vec<aros_common::native_build_contract::NativeOptionalMetaDependency>,
    policy_sha256: String,
    expanded_bytes: usize,
}

#[derive(Debug)]
struct RuntimeConfiguration {
    project: ConfiguredProject,
    absent: BTreeSet<String>,
}

#[derive(Debug, Serialize)]
pub struct OwnerSelectionEvidence {
    pub qualification: &'static str,
    /// True only after both invocation and required native input provenance
    /// have been verified. False never permits capability exclusions.
    pub capability_scope_proven: bool,
    pub policy_sha256: String,
    pub selected_source_files: BTreeSet<String>,
    /// Reached metadata goals keyed by their generated Makefile owner. Unlike
    /// the source-file union, this preserves which child goal is requested.
    /// It is not invocation, preparation completion or native-order proof.
    pub selected_owner_targets: BTreeMap<String, BTreeSet<String>>,
    /// Additional source owners named by the native dependency closure. This
    /// conservative union also protects implicit native link dependencies
    /// which the classic root metadata does not explicitly traverse.
    pub additional_native_source_files: BTreeSet<String>,
    /// Actual parser inputs of required native declarations, independently
    /// indexed from the source invocation metadata.
    pub native_parser_source_files: BTreeSet<String>,
    /// Available native endpoints without a bound parser input. Any such
    /// uncertainty prevents capability exclusions for this invocation.
    pub unbound_native_endpoints: BTreeSet<String>,
    pub reached_targets: BTreeSet<String>,
    pub missing_endpoints: BTreeSet<String>,
    pub snapshot_count: usize,
    pub expanded_bytes: usize,
}

/// An unsupported rule in a source recipe that this closed MetaMake invocation
/// never calls. This is scope evidence, not support for that rule's capability.
#[derive(Debug, Serialize)]
pub struct UninvokedCapabilityFailure {
    pub diagnostic: Diagnostic,
    pub invoking_recipes: BTreeSet<String>,
}

/// Required native endpoints and their independently captured parser origins.
#[derive(Clone, Copy)]
pub struct NativeInvocationSelection<'a> {
    pub roots: &'a [String],
    pub endpoints: &'a BTreeSet<String>,
    pub unavailable_endpoints: &'a BTreeSet<String>,
    pub parser_origins: &'a BTreeMap<String, BTreeSet<String>>,
}

/// Shared sealed source invocation facts, independent of boot/media contracts.
#[derive(Clone, Copy)]
pub struct NativeInvocationPolicy<'a> {
    pub profile: &'a str,
    pub projection: Option<&'a str>,
    pub inputs: &'a [aros_common::native_build_contract::NativeBuildInput],
    pub optional_meta_dependencies:
        &'a [aros_common::native_build_contract::NativeOptionalMetaDependency],
}

impl NativeOwnerProjection {
    /// Original regular Source referent for a logical discovered input.
    /// Policy/template inputs not discovered as Makefiles return None.
    #[must_use]
    pub fn original_input(&self, logical: &str) -> Option<&str> {
        self.discovery_original_inputs
            .get(logical)
            .map(String::as_str)
    }

    /// Complete original logical-input/referent relation, not content authority.
    #[must_use]
    pub const fn original_inputs(&self) -> &BTreeMap<String, String> {
        &self.discovery_original_inputs
    }

    /// Join independently bound runtime project/global data with this policy.
    ///
    /// This compares metadata discovery/substitution inputs, not commands or
    /// generated output bytes. The caller must bind the context's complete
    /// initial environment and ordered global-file bytes to actual execution.
    /// A configure-time host command placeholder may differ; its executable
    /// and invocation identity require the separate supervising producer.
    ///
    /// # Errors
    /// Refuses source drift, missing policy, different project metadata and
    /// every required global value/absence mismatch. Does not run a process.
    pub fn match_runtime_configuration(
        &self,
        source_root: &Path,
        project: &ConfiguredProject,
        globals: &ProjectGlobalPathContext,
    ) -> Result<(), String> {
        self.with_runtime_configuration_checks(source_root, |check| check.compare(project, globals))
    }

    /// Check a batch of runtime configurations under one Source transaction.
    ///
    /// The source projection is verified before the callback and after it has
    /// returned, including when the callback returns an error. The scoped
    /// check borrows only this projection's captured configuration; it cannot
    /// be constructed independently or retained beyond the callback.
    ///
    /// # Errors
    /// Refuses Source drift before or during the callback, missing original
    /// configuration, or any callback comparison error.
    pub fn with_runtime_configuration_checks<'projection, T>(
        &'projection self,
        source_root: &Path,
        operation: impl for<'check> FnOnce(
            &'check RuntimeConfigurationCheck<'projection>,
        ) -> Result<T, String>,
    ) -> Result<T, String> {
        self.verify(source_root)
            .map_err(|_| "runtime owner Source revalidation failed")?;
        let original = self
            .runtime_configuration
            .as_ref()
            .ok_or("runtime owner projection lacks original configuration")?;
        let check = RuntimeConfigurationCheck::new(
            &original.project,
            &self.metamake_globals,
            &original.absent,
        );
        let result = operation(&check);
        self.verify(source_root)
            .map_err(|_| "runtime owner Source changed during comparison".to_owned())?;
        result
    }

    /// Read-only original source digests used by every revalidation.
    #[must_use]
    pub const fn snapshots(&self) -> &BTreeMap<String, String> {
        &self.snapshots
    }

    /// Original policy identity, not a caller-editable evidence label.
    #[must_use]
    pub fn policy_sha256(&self) -> &str {
        &self.policy_sha256
    }

    #[must_use]
    pub const fn expanded_bytes(&self) -> usize {
        self.expanded_bytes
    }

    /// Exact text-expansion evidence for a declaration in this captured
    /// corpus. This is provenance only, never permission to omit an edge or
    /// to manufacture a producer. Direct source fragments have an empty macro
    /// stack; inputs not expanded by GenMF have no expansion record.
    #[must_use]
    pub fn expansion_origin(
        &self,
        declaration: &crate::metamake_owner_graph::TargetDeclarationProvenance,
    ) -> Option<&crate::genmf_projection::ExpandedLineProvenance> {
        if !self.graph.declarations().contains(declaration) {
            return None;
        }
        self.expansion_origins
            .get(&(declaration.file.clone(), declaration.expanded_line))
    }

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
        excluded_paths: &BTreeSet<String>,
    ) -> Result<Option<Self>, String> {
        Self::load_invocation(
            root,
            NativeInvocationPolicy {
                profile: &native.contract.profile,
                projection: native.contract.metamake_projection.as_deref(),
                inputs: &native.contract.inputs,
                optional_meta_dependencies: &native.contract.optional_meta_dependencies,
            },
            files,
            host,
            context,
            excluded_paths,
        )
    }

    /// Validate the same closed source-owner proof for a build or consumer.
    ///
    /// # Errors
    /// Rejects incomplete discovery, unsealed policy, selector/configuration
    /// ambiguity, changed inputs or resource excess. Does not prove a build.
    pub fn load_invocation(
        root: &Path,
        invocation: NativeInvocationPolicy<'_>,
        files: &[PathBuf],
        host: &str,
        context: &TargetContext,
        excluded_paths: &BTreeSet<String>,
    ) -> Result<Option<Self>, String> {
        let Some(path) = invocation.projection else {
            return Ok(None);
        };
        if invocation.optional_meta_dependencies.len() > 64
            || invocation.optional_meta_dependencies.iter().any(|edge| {
                edge.recipe.len() > 4096
                    || edge.target.len() > 4096
                    || edge.dependency.len() > 4096
                    || !invocation
                        .inputs
                        .iter()
                        .any(|input| input.path == edge.recipe)
            })
        {
            return Err("Source optional metadata exceeds bounds or names unsealed recipes".into());
        }
        let sealed = invocation
            .inputs
            .iter()
            .map(|input| (input.path.clone(), input.sha256.to_string()))
            .collect();
        let mut snapshots = BTreeMap::new();
        let (bytes, policy) = load_policy(root, path, invocation.profile, &sealed, &mut snapshots)?;
        validate_policy_identity(&policy, invocation.profile, files.len())?;
        load_evidence_sources(root, &policy, &sealed, &mut snapshots)?;
        let project = load_project(root, &policy, &sealed, &mut snapshots)?;
        let (globals, absent) = load_globals(&policy, &project, host, context)?;
        let discovery_original_inputs =
            verify_complete_discovery(root, files, &project.ignored_directories, excluded_paths)?;
        let discovered: BTreeSet<_> = discovery_original_inputs.keys().cloned().collect();
        let template = load_template(root, &policy, &sealed, &mut snapshots)?;
        let owners = build_owner_map(discovered.clone());
        let (expanded, expanded_bytes, expansion_origins, expanded_program_sha256) = expand_owners(
            root,
            &owners,
            &discovery_original_inputs,
            &template,
            &sealed,
            &mut snapshots,
        )?;
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
        let mut architecture_context = context.clone();
        architecture_context
            .native_configuration_inputs
            .clone_from(&sealed);
        let projection = Self {
            optional_meta_dependencies: invocation.optional_meta_dependencies.to_vec(),
            architecture_context,
            metamake_globals: globals.values,
            runtime_configuration: Some(RuntimeConfiguration {
                project: project.clone(),
                absent,
            }),
            sealed_configuration_inputs: sealed,
            graph,
            expansion_origins,
            expanded_program_sha256,
            architecture_hook_families: policy.architecture_hook_families.into_iter().collect(),
            mesa26_original_producers: policy.mesa26_original_producers,
            owners,
            discovered_inputs: discovered,
            discovery_original_inputs,
            ignored_directories: project.ignored_directories,
            excluded_paths: excluded_paths.clone(),
            snapshots,
            policy_sha256: aros_common::sha256_bytes(&bytes).to_string(),
            expanded_bytes,
        };
        projection.verify(root)?;
        Ok(Some(projection))
    }

    /// Bind the configured scan counterpart of an already excluded directory.
    ///
    /// The caller independently selects this context before execution; no
    /// observed configuration supplies the path. This only adjusts expected
    /// runtime metadata for a namespace already omitted by the original Source
    /// selection. It neither excludes a new input nor authenticates configure,
    /// a local identity, or execution. Native admission must still require the
    /// exact configured directive and revalidate the original Source owner.
    ///
    /// # Errors
    /// Refuses unselected, malformed, basename-only paths or absent Source
    /// runtime policy. No new omission or partial context is returned.
    pub fn with_configured_excluded_directory(mut self, path: &str) -> Result<Self, String> {
        crate::metamake_project::relative_path(path)?;
        if !path.contains('/') || !self.excluded_paths.contains(path) {
            return Err("configured Source ignore is not an exact original exclusion".into());
        }
        let runtime = self
            .runtime_configuration
            .as_mut()
            .ok_or("configured Source ignore requires an original runtime policy")?;
        runtime.project.ignored_directories.insert(path.to_owned());
        self.ignored_directories.insert(path.to_owned());
        Ok(self)
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
            selected_owner_targets,
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
            capability_scope_proven: false,
            policy_sha256: self.policy_sha256.clone(),
            selected_source_files,
            selected_owner_targets,
            additional_native_source_files: BTreeSet::new(),
            native_parser_source_files: BTreeSet::new(),
            unbound_native_endpoints: BTreeSet::new(),
            reached_targets,
            missing_endpoints,
            snapshot_count: self.snapshots.len(),
            expanded_bytes: self.expanded_bytes,
        })
    }

    /// Effective source inputs for the native parser. As in source-owned
    /// generation, `.src` supersedes its generated sibling; direct MetaMake
    /// fragments with no `.src` remain inputs. Discovery still binds both.
    pub fn effective_inputs(&self) -> impl Iterator<Item = &str> {
        self.owners.values().map(String::as_str)
    }

    /// Separate unowned parser failures from recipes outside the exact source
    /// invocation. Keep complete parsing and global inventories: metadata does
    /// not establish that a native producer exists or that a rule is supported.
    ///
    /// Only original parser-input identities are used, never a diagnostic's
    /// displayed path. A failure with any selected, unknown, or global origin
    /// remains fatal. Named failures retain the ordinary target-closure check.
    ///
    /// # Errors
    /// Refuses changed discovery/snapshots or invalid root selection. There is
    /// no default invocation when roots or a source policy are unavailable.
    pub fn scope_capability_failures(
        &self,
        root: &Path,
        selection: NativeInvocationSelection<'_>,
        failures: &mut Vec<Diagnostic>,
        origins: &BTreeMap<Diagnostic, BTreeSet<String>>,
        global_failures: &BTreeSet<Diagnostic>,
    ) -> Result<(OwnerSelectionEvidence, Vec<UninvokedCapabilityFailure>), String> {
        let NativeInvocationSelection {
            roots,
            endpoints: native_endpoints,
            unavailable_endpoints,
            parser_origins,
        } = selection;
        if roots.is_empty() || native_endpoints.is_empty() {
            return Err("capability scope requires resolved native roots".into());
        }
        self.verify(root)?;
        let mut evidence = self.select(roots)?;
        let native_roots: Vec<_> = native_endpoints.iter().cloned().collect();
        let native_sources = self.select(&native_roots)?.selected_source_files;
        evidence.additional_native_source_files = native_sources
            .difference(&evidence.selected_source_files)
            .cloned()
            .collect();
        let inputs: BTreeSet<_> = self.effective_inputs().map(str::to_owned).collect();
        for endpoint in native_endpoints {
            match parser_origins.get(endpoint) {
                Some(origins) if !origins.is_empty() && origins.is_subset(&inputs) => {
                    evidence
                        .native_parser_source_files
                        .extend(origins.iter().cloned());
                }
                _ if !unavailable_endpoints.contains(endpoint) => {
                    evidence.unbound_native_endpoints.insert(endpoint.clone());
                }
                _ => {}
            }
        }
        // A missing selected metadata root or an available native producer
        // without exact input provenance cannot establish a closed scope.
        // Preserve all failures instead of inferring an unrelated recipe.
        if roots
            .iter()
            .any(|root| evidence.missing_endpoints.contains(root))
            || !evidence.unbound_native_endpoints.is_empty()
        {
            return Ok((evidence, Vec::new()));
        }
        evidence.capability_scope_proven = true;
        let selected_sources = evidence
            .selected_source_files
            .union(&native_sources)
            .cloned()
            .chain(evidence.native_parser_source_files.iter().cloned())
            .collect();
        let excluded = partition_uninvoked_failures(
            failures,
            origins,
            global_failures,
            &inputs,
            &selected_sources,
        );
        Ok((evidence, excluded))
    }

    /// Recheck complete discovery and every captured policy, recipe and
    /// imported template before use.
    ///
    /// # Errors
    /// Returns an error if discovery changed or any captured input is
    /// unreadable or changed.
    pub fn verify(&self, root: &Path) -> Result<(), String> {
        verify_captured_discovery(
            root,
            &self.discovered_inputs,
            &self.discovery_original_inputs,
            &self.ignored_directories,
            &self.excluded_paths,
        )?;
        verify_snapshot_digests(root, &self.snapshots, &self.discovery_original_inputs)
    }
}

fn partition_uninvoked_failures(
    failures: &mut Vec<Diagnostic>,
    origins: &BTreeMap<Diagnostic, BTreeSet<String>>,
    global_failures: &BTreeSet<Diagnostic>,
    inputs: &BTreeSet<String>,
    selected: &BTreeSet<String>,
) -> Vec<UninvokedCapabilityFailure> {
    let mut excluded = Vec::new();
    failures.retain(|diagnostic| {
        if diagnostic.code != aros_common::DiagnosticCode::CapabilityDrift
            || diagnostic.stage != aros_common::DiagnosticStage::CapabilityValidation
        {
            return true;
        }
        let named = diagnostic
            .context
            .as_ref()
            .and_then(|context| context.target.as_ref())
            .is_some();
        let Some(recipes) = origins.get(diagnostic) else {
            return true;
        };
        if named
            || global_failures.contains(diagnostic)
            || recipes.is_empty()
            || recipes
                .iter()
                .any(|recipe| !inputs.contains(recipe) || selected.contains(recipe))
        {
            return true;
        }
        excluded.push(UninvokedCapabilityFailure {
            diagnostic: diagnostic.clone(),
            invoking_recipes: recipes.clone(),
        });
        false
    });
    excluded
}

fn load_policy(
    root: &Path,
    path: &str,
    profile: &str,
    sealed: &BTreeMap<String, String>,
    snapshots: &mut BTreeMap<String, String>,
) -> Result<(Vec<u8>, Policy), String> {
    let bytes = read_snapshot(root, path, 64 * 1024, snapshots)?;
    require_seal(path, &bytes, sealed)?;
    let policy: Policy = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    if policy.profile != profile {
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
    validate_architecture_hook_families(&policy.architecture_hook_families)?;
    validate_mesa26_original_producers(&policy.mesa26_original_producers)
}

fn validate_mesa26_original_producers(
    selections: &[Mesa26OriginalProducerSelection],
) -> Result<(), String> {
    if selections.len() > 16 {
        return Err("Mesa 26 original producer selection limit exceeded".into());
    }

    let mut tuples = BTreeSet::new();
    for selection in selections {
        if selection.source_input.len() > 8192
            || selection.source_input.chars().any(char::is_control)
            || crate::metamake_project::relative_path(&selection.source_input).is_err()
            || std::path::Path::new(&selection.source_input).extension()
                != Some(std::ffi::OsStr::new("src"))
        {
            return Err("Mesa 26 original producer has an unsafe Source .src path".into());
        }
        if selection.goal.is_empty()
            || selection.goal.len() > 256
            || selection
                .goal
                .chars()
                .any(|character| character.is_control() || character.is_whitespace())
        {
            return Err("Mesa 26 original producer goal is empty or outside bounds".into());
        }
        for expression in [
            &selection.producer,
            &selection.cooutput,
            &selection.consumer,
        ] {
            if expression.is_empty()
                || expression.len() > 8192
                || expression.chars().any(char::is_control)
            {
                return Err(
                    "Mesa 26 original producer expression is empty or outside bounds".into(),
                );
            }
        }

        if !tuples.insert((
            selection.source_input.as_str(),
            selection.goal.as_str(),
            selection.producer.as_str(),
            selection.cooutput.as_str(),
            selection.consumer.as_str(),
        )) {
            return Err("duplicate Mesa 26 original producer selection".into());
        }
    }
    Ok(())
}

fn validate_architecture_hook_families(names: &[String]) -> Result<(), String> {
    let families: BTreeSet<_> = names.iter().collect();
    if families.len() != names.len()
        || families.iter().any(|family| {
            !matches!(
                family.as_str(),
                "module" | "linklib" | "set-archincludes" | "quick"
            )
        })
    {
        return Err("native architecture hooks require unique closed template families".into());
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
    const FORWARDED_WHEN_SELECTED: &[&str] = &[
        "TARGET_LLVM_VER",
        "TARGET_LLVM_RUNTIMES_STYLE",
        "TARGET_RUST",
        "TARGET_RUST_VER",
    ];
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
            // The frontend forwards these from cache variables that default to
            // empty, so an empty value means it did not select one, not that
            // it selected the empty string. The sealed policy then supplies it.
            if expected.is_empty() && FORWARDED_WHEN_SELECTED.contains(name) {
                continue;
            }
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
    excluded_paths: &BTreeSet<String>,
) -> Result<BTreeMap<String, String>, String> {
    if files.len() > 20_000 {
        return Err("native MetaMake discovery exceeds file budget".into());
    }
    let mut supplied = BTreeSet::new();
    for file in files {
        let name = meta_relative_name(root, file)?;
        if !path_has_ignored_directory(Path::new(&name), ignored_directories)
            && !path_is_excluded(Path::new(&name), excluded_paths)
            && !supplied.insert(name.to_owned())
        {
            return Err(format!("duplicate discovered MetaMake input: {name}"));
        }
    }
    let expected = discover_meta_inputs(root, ignored_directories, excluded_paths)?;
    compare_discovered_inputs(&expected.keys().cloned().collect(), &supplied)?;
    Ok(expected)
}

fn verify_captured_discovery(
    root: &Path,
    discovered_inputs: &BTreeSet<String>,
    originals: &BTreeMap<String, String>,
    ignored_directories: &BTreeSet<String>,
    excluded_paths: &BTreeSet<String>,
) -> Result<(), String> {
    let current = discover_meta_inputs(root, ignored_directories, excluded_paths)?;
    compare_discovered_inputs(discovered_inputs, &current.keys().cloned().collect())?;
    if &current != originals {
        return Err("MetaMake original Source input referents changed".into());
    }
    Ok(())
}

fn compare_discovered_inputs(
    expected: &BTreeSet<String>,
    current: &BTreeSet<String>,
) -> Result<(), String> {
    if expected != current {
        let missing: Vec<_> = expected.difference(current).take(5).cloned().collect();
        let extra: Vec<_> = current.difference(expected).take(5).cloned().collect();
        return Err(format!(
            "MetaMake input discovery changed; omitted={missing:?}, unexpected={extra:?}"
        ));
    }
    Ok(())
}

#[cfg(test)]
fn discover_meta_files(
    root: &Path,
    ignored_directories: &BTreeSet<String>,
    excluded_paths: &BTreeSet<String>,
) -> Result<BTreeSet<String>, String> {
    Ok(
        discover_meta_inputs(root, ignored_directories, excluded_paths)?
            .into_keys()
            .collect(),
    )
}

/// Discover logical native MetaMake inputs and their original in-tree referents.
///
/// This is metadata discovery only, not a content snapshot or Source-policy
/// authorization. Invocation loading independently repeats and verifies it.
///
/// # Errors
/// Refuses incomplete/unsupported inventories, unsafe aliases and budget excess.
pub fn discover_meta_inputs(
    root: &Path,
    ignored_directories: &BTreeSet<String>,
    excluded_paths: &BTreeSet<String>,
) -> Result<BTreeMap<String, String>, String> {
    let mut project =
        ConfiguredProject::parse("[discovery]\ndefaultmakefilename mmakefile\n", "discovery")?;
    project.ignored_directories.clone_from(ignored_directories);
    let discovery = CapturedProjectDiscovery::capture(root, &project, excluded_paths)?;
    if discovery.inputs().len() > 20_000 {
        return Err("native MetaMake discovery exceeds file budget".into());
    }
    Ok(discovery.into_inputs())
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

type ExpansionOrigins = BTreeMap<(String, usize), crate::genmf_projection::ExpandedLineProvenance>;
type ExpandedOwnerPrograms = (
    BTreeMap<String, String>,
    usize,
    ExpansionOrigins,
    BTreeMap<String, aros_common::Sha256Digest>,
);

fn expand_owners(
    root: &Path,
    owners: &BTreeMap<String, String>,
    original_inputs: &BTreeMap<String, String>,
    template: &Path,
    sealed: &BTreeMap<String, String>,
    snapshots: &mut BTreeMap<String, String>,
) -> Result<ExpandedOwnerPrograms, String> {
    let mut expanded = BTreeMap::new();
    let mut expanded_bytes = 0usize;
    let mut expansion_origins = ExpansionOrigins::new();
    let mut program_digests = BTreeMap::new();
    for (owner, source) in owners {
        let original = original_inputs
            .get(source)
            .ok_or("MetaMake owner lacks its original regular input")?;
        let bytes = read_snapshot(root, original, 1024 * 1024, snapshots)?;
        // Logical labels remain parser/child coordinates; discovery binds them
        // to the independently remeasured original regular input.
        retain_snapshot(snapshots, source, &bytes)?;
        let (text, program_digest) = if Path::new(source)
            .file_name()
            .is_some_and(|name| name == "mmakefile.src")
        {
            let expansion = expand_bytes(
                &bytes,
                &root.join(source),
                template,
                GenmfLimits {
                    metadata_provenance_only: true,
                    ..GenmfLimits::default()
                },
            )
            .map_err(|error| error.to_string())?;
            let program_digest = aros_common::sha256_bytes(
                &expansion
                    .native_bytes()
                    .map_err(|error| error.to_string())?,
            );
            // Keep only complete directive starts. Provenance of generated
            // commands never serves as a metadata admission shortcut.
            for origin in expansion.provenance {
                let fragment = &expansion.text[origin.output_start_byte..origin.output_end_byte];
                if fragment.starts_with("#MM") {
                    let key = (owner.clone(), origin.output_line);
                    if expansion_origins.insert(key, origin).is_some() {
                        return Err("ambiguous GenMF metadata line provenance".into());
                    }
                }
            }
            retain_template_snapshots(root, expansion.template_snapshots, sealed, snapshots)?;
            (expansion.text, program_digest)
        } else {
            let program_digest = aros_common::sha256_bytes(&bytes);
            (
                String::from_utf8(bytes.clone())
                    .unwrap_or_else(|_| bytes.into_iter().map(char::from).collect()),
                program_digest,
            )
        };
        expanded_bytes = expanded_bytes
            .checked_add(text.len())
            .filter(|total| *total <= MAX_TOTAL_BYTES)
            .ok_or("native MetaMake expanded corpus exceeds 128 MiB")?;
        expanded.insert(owner.clone(), text);
        program_digests.insert(owner.clone(), program_digest);
    }
    Ok((expanded, expanded_bytes, expansion_origins, program_digests))
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
    original_inputs: &BTreeMap<String, String>,
) -> Result<(), String> {
    for (path, digest) in snapshots {
        let original = original_inputs.get(path).unwrap_or(path);
        let mut recaptured = BTreeMap::new();
        let bytes = read_snapshot(root, original, 8 * 1024 * 1024, &mut recaptured)?;
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
    path.to_str().is_some_and(|path| {
        is_ignored_directory_path(ignored_directories, path, InventoryEntry::RegularFile)
    })
}

/// Whether `path` is, or lies below, one of the exact excluded directories.
fn path_is_excluded(path: &Path, excluded_paths: &BTreeSet<String>) -> bool {
    excluded_paths
        .iter()
        .any(|excluded| path.starts_with(Path::new(excluded)))
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

    #[test]
    fn architecture_families_are_closed_unique_and_opt_in() {
        assert!(validate_architecture_hook_families(&[]).is_ok());
        assert!(validate_architecture_hook_families(&[
            "module".into(),
            "linklib".into(),
            "set-archincludes".into(),
            "quick".into()
        ])
        .is_ok());
        for names in [
            vec!["module".into(), "module".into()],
            vec!["unknown".into()],
            vec!["kernel-exec-riscv".into()],
        ] {
            assert!(validate_architecture_hook_families(&names).is_err());
        }
    }

    fn valid_mesa26_original_producer() -> Mesa26OriginalProducerSelection {
        Mesa26OriginalProducerSelection {
            source_input: "arch/arm-native/gallium/mmakefile.src".into(),
            goal: "linklibs-gallium_vc4-gca".into(),
            producer: "$(GCA_GENDIR)/.gca.stamp".into(),
            cooutput: "$(GCA_GENDIR)/gca_table.c $(GCA_GENDIR)/gca_bind.c".into(),
            consumer: "$(top_libdir)/libgallium_vc4_gca.a".into(),
        }
    }

    #[test]
    fn mesa26_original_producer_policy_is_bounded_and_uses_full_tuple_identity() {
        assert!(validate_mesa26_original_producers(&[]).is_ok());
        assert!(validate_mesa26_original_producers(&[valid_mesa26_original_producer()]).is_ok());

        let duplicate = valid_mesa26_original_producer();
        assert!(validate_mesa26_original_producers(&[duplicate.clone(), duplicate]).is_err());

        let mut distinct = valid_mesa26_original_producer();
        distinct.consumer = "$(top_libdir)/libgallium_v3d_gca.a".into();
        assert!(
            validate_mesa26_original_producers(&[valid_mesa26_original_producer(), distinct,])
                .is_ok()
        );

        assert!(
            validate_mesa26_original_producers(&vec![valid_mesa26_original_producer(); 17])
                .is_err()
        );
    }

    #[test]
    fn mesa26_original_producer_policy_rejects_unsafe_source_goals_and_expressions() {
        for source_input in [
            "",
            "/absolute/path.src",
            "../outside/path.src",
            "arch\\path.src",
            "arch/path.make",
        ] {
            let mut selection = valid_mesa26_original_producer();
            selection.source_input = source_input.into();
            assert!(validate_mesa26_original_producers(&[selection]).is_err());
        }

        for goal in [String::new(), "has whitespace".into(), "line\nbreak".into()] {
            let mut selection = valid_mesa26_original_producer();
            selection.goal = goal;
            assert!(validate_mesa26_original_producers(&[selection]).is_err());
        }
        let mut oversized_goal = valid_mesa26_original_producer();
        oversized_goal.goal = "g".repeat(257);
        assert!(validate_mesa26_original_producers(&[oversized_goal]).is_err());

        for field in ["producer", "cooutput", "consumer"] {
            let mut selection = valid_mesa26_original_producer();
            match field {
                "producer" => selection.producer.clear(),
                "cooutput" => selection.cooutput = "bad\nexpression".into(),
                "consumer" => selection.consumer = "x".repeat(8193),
                _ => unreachable!(),
            }
            assert!(validate_mesa26_original_producers(&[selection]).is_err());
        }
    }

    #[test]
    fn mesa26_original_producer_policy_field_defaults_when_absent() {
        let policy: Policy = serde_json::from_value(serde_json::json!({
            "schema_version": 1,
            "kind": "native-metamake-policy-v1",
            "profile": "fixture",
            "project": "fixture",
            "configuration_source": "config/make.cfg",
            "template": "config/make.tmpl",
            "substitutions": [],
            "global_snapshots": [],
            "environment": [],
            "host_environment": [],
            "declared_absent": [],
            "evidence_sources": [],
            "closed_environment": true,
            "out_of_source": true
        }))
        .unwrap();
        assert!(policy.mesa26_original_producers.is_empty());
    }

    fn unowned_failure() -> Diagnostic {
        Diagnostic::error(
            aros_common::DiagnosticCode::CapabilityDrift,
            aros_common::DiagnosticStage::CapabilityValidation,
            "unsupported source rule",
        )
        .with_location(aros_common::SourceLocation::new("foreign/display-path.src"))
    }

    #[test]
    fn capability_scope_uses_all_parser_origins_not_the_displayed_location() {
        let diagnostic = unowned_failure();
        let inputs = BTreeSet::from([
            "selected/mmakefile.src".into(),
            "other/mmakefile.src".into(),
        ]);
        let selected = BTreeSet::from(["selected/mmakefile.src".into()]);
        let mut failures = vec![diagnostic.clone()];
        let origins = BTreeMap::from([(
            diagnostic.clone(),
            BTreeSet::from(["other/mmakefile.src".into()]),
        )]);
        let excluded = partition_uninvoked_failures(
            &mut failures,
            &origins,
            &BTreeSet::new(),
            &inputs,
            &selected,
        );
        assert!(failures.is_empty());
        assert_eq!(excluded.len(), 1);
        assert_eq!(excluded[0].invoking_recipes, origins[&diagnostic]);

        let origins = BTreeMap::from([(diagnostic.clone(), inputs.clone())]);
        let mut failures = vec![diagnostic.clone()];
        assert!(partition_uninvoked_failures(
            &mut failures,
            &origins,
            &BTreeSet::new(),
            &inputs,
            &selected
        )
        .is_empty());
        assert_eq!(failures, [diagnostic]);
    }

    #[test]
    fn capability_scope_keeps_unknown_empty_and_global_origins_fatal() {
        let diagnostic = unowned_failure();
        let inputs = BTreeSet::from(["other/mmakefile.src".into()]);
        let unknown = BTreeSet::from(["unknown/mmakefile.src".into()]);
        for recipes in [BTreeSet::new(), unknown] {
            let mut failures = vec![diagnostic.clone()];
            let origins = BTreeMap::from([(diagnostic.clone(), recipes)]);
            assert!(partition_uninvoked_failures(
                &mut failures,
                &origins,
                &BTreeSet::new(),
                &inputs,
                &BTreeSet::new()
            )
            .is_empty());
            assert_eq!(failures.as_slice(), std::slice::from_ref(&diagnostic));
        }
        let mut failures = vec![diagnostic.clone()];
        assert!(partition_uninvoked_failures(
            &mut failures,
            &BTreeMap::new(),
            &BTreeSet::new(),
            &inputs,
            &BTreeSet::new()
        )
        .is_empty());
        assert_eq!(failures.as_slice(), std::slice::from_ref(&diagnostic));
        let origins = BTreeMap::from([(diagnostic.clone(), inputs.clone())]);
        let mut failures = vec![diagnostic.clone()];
        assert!(partition_uninvoked_failures(
            &mut failures,
            &origins,
            &BTreeSet::from([diagnostic.clone()]),
            &inputs,
            &BTreeSet::new()
        )
        .is_empty());
        assert_eq!(failures, [diagnostic]);
    }

    #[test]
    fn named_capability_failures_retain_target_closure_validation() {
        let diagnostic = unowned_failure().with_context(aros_common::DiagnosticContext {
            target: Some("named-owner".into()),
            ..Default::default()
        });
        let inputs = BTreeSet::from(["other/mmakefile.src".into()]);
        let origins = BTreeMap::from([(diagnostic.clone(), inputs.clone())]);
        let mut failures = vec![diagnostic.clone()];
        assert!(partition_uninvoked_failures(
            &mut failures,
            &origins,
            &BTreeSet::new(),
            &inputs,
            &BTreeSet::new()
        )
        .is_empty());
        assert_eq!(failures, [diagnostic]);
    }

    #[test]
    fn source_and_graph_errors_are_not_capability_scope_exemptions() {
        let mut diagnostic = unowned_failure();
        diagnostic.code = aros_common::DiagnosticCode::GraphValidation;
        diagnostic.stage = aros_common::DiagnosticStage::GraphValidation;
        let inputs = BTreeSet::from(["other/mmakefile.src".into()]);
        let origins = BTreeMap::from([(diagnostic.clone(), inputs.clone())]);
        let mut failures = vec![diagnostic.clone()];
        assert!(partition_uninvoked_failures(
            &mut failures,
            &origins,
            &BTreeSet::new(),
            &inputs,
            &BTreeSet::new()
        )
        .is_empty());
        assert_eq!(failures, [diagnostic]);
    }

    #[test]
    fn capability_scope_requires_nonempty_roots_before_mutating_failures() {
        let root = tempfile::tempdir().unwrap();
        let projection = projection_for_discovery(root.path(), BTreeSet::new());
        let diagnostic = unowned_failure();
        let mut failures = vec![diagnostic.clone()];
        let error = projection
            .scope_capability_failures(
                root.path(),
                NativeInvocationSelection {
                    roots: &[],
                    endpoints: &BTreeSet::new(),
                    unavailable_endpoints: &BTreeSet::new(),
                    parser_origins: &BTreeMap::new(),
                },
                &mut failures,
                &BTreeMap::new(),
                &BTreeSet::new(),
            )
            .unwrap_err();
        assert!(error.contains("resolved native roots"));
        assert_eq!(failures, [diagnostic]);
    }

    #[test]
    fn unbound_native_producer_prevents_all_exclusions() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("extra")).unwrap();
        fs::write(root.path().join("mmakefile.src"), "#MM root\n").unwrap();
        fs::write(root.path().join("extra/mmakefile.src"), "#MM extra\n").unwrap();
        let mut projection = projection_for_discovery(root.path(), BTreeSet::new());
        projection.graph = MetaMakeOwnerGraph::parse_with_declared_absence(
            &BTreeMap::from([
                ("mmakefile".into(), "#MM root\n".into()),
                ("extra/mmakefile".into(), "#MM extra\n".into()),
            ]),
            &BTreeMap::new(),
            &BTreeSet::new(),
            Limits::default(),
        )
        .unwrap();
        let diagnostic = unowned_failure();
        let origins = BTreeMap::from([(
            diagnostic.clone(),
            BTreeSet::from(["extra/mmakefile.src".into()]),
        )]);
        let endpoints = BTreeSet::from(["root".into(), "unmapped-provider".into()]);
        let parser_origins =
            BTreeMap::from([("root".into(), BTreeSet::from(["mmakefile.src".into()]))]);
        let mut failures = vec![diagnostic.clone()];
        let (evidence, excluded) = projection
            .scope_capability_failures(
                root.path(),
                NativeInvocationSelection {
                    roots: &["root".into()],
                    endpoints: &endpoints,
                    unavailable_endpoints: &BTreeSet::new(),
                    parser_origins: &parser_origins,
                },
                &mut failures,
                &origins,
                &BTreeSet::new(),
            )
            .unwrap();
        assert_eq!(
            evidence.unbound_native_endpoints,
            BTreeSet::from(["unmapped-provider".into()])
        );
        assert!(!evidence.capability_scope_proven);
        assert!(excluded.is_empty());
        assert_eq!(failures, [diagnostic]);
    }

    #[test]
    fn absent_metadata_root_prevents_exclusions_even_with_native_provenance() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("mmakefile.src"), "#MM root\n").unwrap();
        let projection = projection_for_discovery(root.path(), BTreeSet::new());
        let diagnostic = unowned_failure();
        let origins =
            BTreeMap::from([(diagnostic.clone(), BTreeSet::from(["mmakefile.src".into()]))]);
        let endpoints = BTreeSet::from(["absent-root".into()]);
        let parser_origins = BTreeMap::from([(
            "absent-root".into(),
            BTreeSet::from(["mmakefile.src".into()]),
        )]);
        let mut failures = vec![diagnostic.clone()];
        let (_, excluded) = projection
            .scope_capability_failures(
                root.path(),
                NativeInvocationSelection {
                    roots: &["absent-root".into()],
                    endpoints: &endpoints,
                    unavailable_endpoints: &BTreeSet::new(),
                    parser_origins: &parser_origins,
                },
                &mut failures,
                &origins,
                &BTreeSet::new(),
            )
            .unwrap();
        assert!(excluded.is_empty());
        assert_eq!(failures, [diagnostic]);
    }

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
    fn source_owner_selection_serializes_child_goals_without_execution_claim() {
        let root = tempfile::tempdir().unwrap();
        for directory in ["one", "two"] {
            fs::create_dir(root.path().join(directory)).unwrap();
            fs::write(
                root.path().join(directory).join("mmakefile.src"),
                "#MM root\n",
            )
            .unwrap();
        }
        let projection = projection_for_discovery(root.path(), BTreeSet::new());
        let evidence = projection.select(&["root".into()]).unwrap();
        let json = serde_json::to_value(&evidence).unwrap();
        assert_eq!(
            json["selected_owner_targets"],
            serde_json::json!({
                "one/mmakefile": ["root"], "two/mmakefile": ["root"]
            })
        );
        assert_eq!(json["capability_scope_proven"], false);
        assert_eq!(
            json["qualification"],
            "source-owner-metadata-not-producer-proof"
        );
        assert_eq!(
            evidence.selected_source_files,
            BTreeSet::from(["one/mmakefile.src".into(), "two/mmakefile.src".into()])
        );
    }

    #[test]
    fn runtime_owner_configuration_binds_project_and_selected_values_not_commands() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("mmakefile.src"), "#MM root\n").unwrap();
        let mut projection = projection_for_discovery(root.path(), BTreeSet::new());
        let project = ConfiguredProject::parse("[probe]\n", "probe").unwrap();
        let context = ProjectGlobalPathContext::new(&BTreeMap::new()).unwrap();
        assert!(projection
            .match_runtime_configuration(root.path(), &project, &context)
            .is_err());
        projection.runtime_configuration = Some(RuntimeConfiguration {
            project: project.clone(),
            absent: BTreeSet::from(["ABSENT".into()]),
        });
        projection.metamake_globals = BTreeMap::from([("VALUE".into(), "selected".into())]);
        let good = ProjectGlobalPathContext::new(&projection.metamake_globals).unwrap();
        assert!(projection
            .match_runtime_configuration(root.path(), &project, &good)
            .is_ok());
        for values in [
            BTreeMap::new(),
            BTreeMap::from([("VALUE".into(), String::new())]),
            BTreeMap::from([
                ("VALUE".into(), "selected".into()),
                ("ABSENT".into(), String::new()),
            ]),
        ] {
            assert!(projection
                .match_runtime_configuration(
                    root.path(),
                    &project,
                    &ProjectGlobalPathContext::new(&values).unwrap()
                )
                .is_err());
        }
        let mutations: [fn(&mut ConfiguredProject); 11] = [
            |value| value.name = "other".into(),
            |value| value.top = Some("foreign".into()),
            |value| value.default_makefile = "foreign".into(),
            |value| value.default_target = "foreign".into(),
            |value| {
                value.ignored_directories.insert("hidden".into());
            },
            |value| value.added_makefiles.push("extra".into()),
            |value| value.global_variable_files.push("foreign".into()),
            |value| {
                value.generator_inputs.insert("foreign".into());
            },
            |value| value.generator_command = Some("foreign".into()),
            |value| value.global_generator_command = Some("foreign".into()),
            |value| {
                value.variables.insert("foreign".into(), "value".into());
            },
        ];
        for mutate in mutations {
            let mut altered = project.clone();
            mutate(&mut altered);
            assert!(projection
                .match_runtime_configuration(root.path(), &altered, &good)
                .is_err());
        }
        // Executable/command admission is intentionally a different gate.
        let mut host_command = project.clone();
        host_command.make_command = Some("configured-host-command".into());
        assert!(projection
            .match_runtime_configuration(root.path(), &host_command, &good)
            .is_ok());
        fs::write(
            root.path().join("mmakefile"),
            "unexpected discovered file\n",
        )
        .unwrap();
        assert!(projection
            .match_runtime_configuration(root.path(), &project, &good)
            .is_err());
    }

    fn runtime_configuration_fixture(
        root: &Path,
    ) -> (
        NativeOwnerProjection,
        ConfiguredProject,
        ProjectGlobalPathContext,
    ) {
        fs::write(root.join("mmakefile.src"), "#MM root\n").unwrap();
        let mut projection = projection_for_discovery(root, BTreeSet::new());
        let project = ConfiguredProject::parse("[probe]\n", "probe").unwrap();
        projection.runtime_configuration = Some(RuntimeConfiguration {
            project: project.clone(),
            absent: BTreeSet::from(["ABSENT".into()]),
        });
        projection.metamake_globals = BTreeMap::from([("VALUE".into(), "selected".into())]);
        let globals = ProjectGlobalPathContext::new(&projection.metamake_globals).unwrap();
        (projection, project, globals)
    }

    #[test]
    fn runtime_configuration_batch_compares_multiple_values_under_one_scope() {
        let root = tempfile::tempdir().unwrap();
        let (projection, project, globals) = runtime_configuration_fixture(root.path());
        let comparisons = projection
            .with_runtime_configuration_checks(root.path(), |check| {
                check.compare(&project, &globals)?;
                check.compare(&project, &globals)?;
                Ok(2)
            })
            .unwrap();
        assert_eq!(comparisons, 2);
    }

    #[test]
    fn runtime_configuration_batch_late_mismatch_returns_no_partial_result() {
        let root = tempfile::tempdir().unwrap();
        let (projection, project, globals) = runtime_configuration_fixture(root.path());
        let mut altered = project.clone();
        altered.default_target = "foreign".into();
        let result = projection.with_runtime_configuration_checks(root.path(), |check| {
            check.compare(&project, &globals)?;
            let mut partial = vec!["first comparison passed"];
            check.compare(&altered, &globals)?;
            partial.push("late comparison passed");
            Ok(partial)
        });
        assert!(result.is_err());
    }

    #[test]
    fn runtime_configuration_batch_detects_source_mutation_after_callback_error() {
        let root = tempfile::tempdir().unwrap();
        let (mut projection, project, globals) = runtime_configuration_fixture(root.path());
        projection.snapshots.insert(
            "mmakefile.src".into(),
            aros_common::sha256_bytes(b"#MM root\n").to_string(),
        );
        let result = projection.with_runtime_configuration_checks(root.path(), |check| {
            check.compare(&project, &globals)?;
            fs::write(root.path().join("mmakefile.src"), "#MM changed\n").unwrap();
            Err::<(), String>("callback failure".into())
        });
        assert_eq!(
            result.unwrap_err(),
            "runtime owner Source changed during comparison"
        );
    }

    #[test]
    fn runtime_configuration_batch_rejects_preexisting_source_drift_before_callback() {
        let root = tempfile::tempdir().unwrap();
        let (projection, _project, _globals) = runtime_configuration_fixture(root.path());
        fs::write(
            root.path().join("mmakefile"),
            "unexpected discovered file\n",
        )
        .unwrap();
        let callback_calls = std::cell::Cell::new(0);
        let result = projection.with_runtime_configuration_checks(root.path(), |_check| {
            callback_calls.set(callback_calls.get() + 1);
            Ok(())
        });
        assert!(result.is_err());
        assert_eq!(callback_calls.get(), 0);
    }

    #[test]
    fn complete_child_coordinates_require_exact_source_selected_pair_coverage() {
        use aros_common::make_native_invocation::NativeMakeInvocation;
        let root = tempfile::tempdir().unwrap();
        for directory in ["one", "two"] {
            fs::create_dir(root.path().join(directory)).unwrap();
            fs::write(
                root.path().join(directory).join("mmakefile.src"),
                "#MM root\n",
            )
            .unwrap();
        }
        let mut projection = projection_for_discovery(root.path(), BTreeSet::new());
        for directory in ["one", "two"] {
            projection.snapshots.insert(
                format!("{directory}/mmakefile.src"),
                aros_common::sha256_bytes(b"#MM root\n").to_string(),
            );
        }
        projection.graph = MetaMakeOwnerGraph::parse_expanded_files(
            &BTreeMap::from([
                (
                    "one/mmakefile".into(),
                    "#MM- top : first second\n#MM first\n#MM second\n#MM- empty\n#MM- broken : first absent\n".into(),
                ),
                ("two/mmakefile".into(), "#MM first\n".into()),
            ]),
            &BTreeMap::new(),
            Limits::default(),
        )
        .unwrap();
        let child = |directory: &str, goals: &[&str]| {
            NativeMakeInvocation::new(
                std::iter::once("--file=mmakefile".to_owned())
                    .chain(goals.iter().map(|goal| (*goal).to_owned())),
                PathBuf::from("/selected/build").join(directory),
                [],
            )
            .unwrap()
        };
        let one = child("one", &["second", "first"]);
        let two = child("two", &["first"]);
        let first = child("one", &["first"]);
        let second = child("one", &["second"]);
        let repeated = child("one", &["second", "first", "second"]);
        let foreign = child("two", &["second"]);
        let roots = ["top".into()];
        let build = Path::new("/selected/build");
        let check = |children: &[(&str, &NativeMakeInvocation)]| {
            projection.match_complete_child_coordinates(root.path(), &roots, build, children)
        };
        let matched = check(&[("two/mmakefile", &two), ("one/mmakefile", &one)]).unwrap();
        assert_eq!(matched.len(), 2);
        assert_eq!(matched[0].generated_owner(), "two/mmakefile");
        assert_eq!(matched[1].goals(), ["second", "first"]);
        let selected_slots = projection
            .select_required_child_coordinates(
                root.path(),
                &roots,
                build,
                &[("two/mmakefile", &two), ("one/mmakefile", &one)],
                &[
                    ("two/mmakefile.src".into(), "first".into()),
                    ("one/mmakefile.src".into(), "second".into()),
                    ("one/mmakefile.src".into(), "first".into()),
                ],
            )
            .unwrap();
        assert_eq!(selected_slots, [0, 1, 1]);
        // Selecting one owner does not discard the other call from full-roster
        // validation, and two goals in one call retain the same original slot.
        assert_eq!(
            projection
                .select_required_child_coordinates(
                    root.path(),
                    &roots,
                    build,
                    &[("two/mmakefile", &two), ("one/mmakefile", &one)],
                    &[("one/mmakefile.src".into(), "second".into())],
                )
                .unwrap(),
            [1]
        );
        for missing in [
            ("missing/mmakefile.src".into(), "first".into()),
            ("one/mmakefile.src".into(), "missing".into()),
            ("one/mmakefile.src".into(), "first-quick".into()),
            ("one/../one/mmakefile.src".into(), "first".into()),
        ] {
            assert!(projection
                .select_required_child_coordinates(
                    root.path(),
                    &roots,
                    build,
                    &[("two/mmakefile", &two), ("one/mmakefile", &one)],
                    &[missing],
                )
                .unwrap_err()
                .contains("was not observed"));
        }
        assert!(projection
            .select_required_child_coordinates(
                root.path(),
                &roots,
                build,
                &[("two/mmakefile", &two), ("one/mmakefile", &one)],
                &[
                    ("one/mmakefile.src".into(), "first".into()),
                    ("one/mmakefile.src".into(), "first".into()),
                ],
            )
            .unwrap_err()
            .contains("repeats"));
        assert!(projection
            .select_required_child_coordinates(
                root.path(),
                &roots,
                build,
                &[("one/mmakefile", &one)],
                &[("one/mmakefile.src".into(), "first".into())],
            )
            .unwrap_err()
            .contains("omits a selected owner target"));
        for (requirements, reason) in [
            (
                vec![("one/mmakefile.src".into(), "first".into()); 4097],
                "count bound",
            ),
            (
                vec![("x".repeat(16 * 1024 * 1024), "first".into())],
                "byte bound",
            ),
            (vec![(String::new(), "first".into())], "is empty"),
        ] {
            assert!(projection
                .select_required_child_coordinates(
                    root.path(),
                    &roots,
                    build,
                    &[("two/mmakefile", &two), ("one/mmakefile", &one)],
                    &requirements,
                )
                .unwrap_err()
                .contains(reason));
        }
        // Grouping and order do not change exact pair coverage. The same goal
        // at a different owner is a distinct required coordinate.
        assert!(check(&[
            ("one/mmakefile", &first),
            ("two/mmakefile", &two),
            ("one/mmakefile", &second),
        ])
        .is_ok());
        for incomplete in [
            vec![],
            vec![("one/mmakefile", &one)],
            vec![("two/mmakefile", &two)],
            vec![("one/mmakefile", &first), ("two/mmakefile", &two)],
        ] {
            assert!(check(&incomplete)
                .err()
                .unwrap()
                .contains("omits a selected owner target"));
        }
        for duplicates in [
            vec![("one/mmakefile", &repeated), ("two/mmakefile", &two)],
            vec![
                ("one/mmakefile", &one),
                ("two/mmakefile", &two),
                ("one/mmakefile", &first),
            ],
        ] {
            assert!(check(&duplicates)
                .err()
                .unwrap()
                .contains("repeats an owner target"));
            // The original membership-only API deliberately keeps duplicates.
            assert!(projection
                .match_captured_children_coordinates(root.path(), &roots, build, &duplicates)
                .is_ok());
        }
        assert!(check(&[("one/mmakefile", &one), ("two/mmakefile", &foreign)]).is_err());
        assert!(projection
            .match_complete_child_coordinates(root.path(), &["empty".into()], build, &[])
            .unwrap()
            .is_empty());
        for unresolved in ["unknown", "broken"] {
            assert!(projection
                .match_complete_child_coordinates(
                    root.path(),
                    &[unresolved.into()],
                    build,
                    &[("one/mmakefile", &first), ("two/mmakefile", &two)],
                )
                .err()
                .unwrap()
                .contains("unresolved endpoints"));
        }
        // Native MetaMake skips absent dependency nodes, but this does not
        // make the dependency graph complete. Keep both claims independent.
        let broken_roots = ["broken".to_owned()];
        let observed = |children: &[(&str, &NativeMakeInvocation)]| {
            projection.match_observed_owner_coordinates(root.path(), &broken_roots, build, children)
        };
        assert_eq!(
            projection
                .select_source_metadata(root.path(), &broken_roots)
                .unwrap()
                .selection()
                .missing_endpoints,
            BTreeSet::from(["absent".to_owned()]),
        );
        assert_eq!(
            observed(&[("one/mmakefile", &first), ("two/mmakefile", &two)])
                .unwrap()
                .len(),
            2
        );
        for incomplete_or_repeated in [
            vec![],
            vec![("one/mmakefile", &first)],
            vec![("two/mmakefile", &two)],
            vec![
                ("one/mmakefile", &first),
                ("one/mmakefile", &first),
                ("two/mmakefile", &two),
            ],
            vec![("one/mmakefile", &one), ("two/mmakefile", &two)],
        ] {
            assert!(observed(&incomplete_or_repeated).is_err());
        }
        for unknown_roots in [
            vec![],
            vec!["unknown".to_owned()],
            vec!["broken".to_owned(), "unknown".to_owned()],
        ] {
            assert!(projection
                .match_observed_owner_coordinates(
                    root.path(),
                    &unknown_roots,
                    build,
                    &[("one/mmakefile", &first), ("two/mmakefile", &two)],
                )
                .err()
                .unwrap()
                .contains("known requested roots"));
        }
        assert_eq!(
            projection
                .select_required_child_coordinates(
                    root.path(),
                    &broken_roots,
                    build,
                    &[("two/mmakefile", &two), ("one/mmakefile", &first)],
                    &[("one/mmakefile.src".to_owned(), "first".to_owned())],
                )
                .unwrap(),
            vec![1]
        );
        assert!(projection
            .select_required_child_coordinates(
                root.path(),
                &broken_roots,
                build,
                &[("two/mmakefile", &two), ("one/mmakefile", &first)],
                &[("one/mmakefile.src".to_owned(), "absent".to_owned())],
            )
            .is_err());
        assert!(projection
            .select_source_metadata(root.path(), &broken_roots)
            .unwrap()
            .selection()
            .missing_endpoints
            .contains("absent"));
        assert!(check(&vec![("one/mmakefile", &one); 4097])
            .err()
            .unwrap()
            .contains("count bound"));
        // Coverage cannot turn drifted source inputs into selection authority.
        fs::write(root.path().join("one/mmakefile.src"), "#MM changed\n").unwrap();
        assert!(check(&[("one/mmakefile", &one), ("two/mmakefile", &two)])
            .err()
            .unwrap()
            .contains("revalidation failed"));
        assert!(projection
            .select_required_child_coordinates(
                root.path(),
                &roots,
                build,
                &[("two/mmakefile", &two), ("one/mmakefile", &one)],
                &[("one/mmakefile.src".into(), "first".into())],
            )
            .unwrap_err()
            .contains("revalidation failed"));
    }

    #[test]
    fn captured_child_coordinates_bind_selected_owner_input_and_ordered_goals() {
        use aros_common::make_native_invocation::NativeMakeInvocation;
        let root = tempfile::tempdir().unwrap();
        for directory in ["one", "two"] {
            fs::create_dir(root.path().join(directory)).unwrap();
            fs::write(
                root.path().join(directory).join("mmakefile.src"),
                "#MM root\n",
            )
            .unwrap();
        }
        let mut projection = projection_for_discovery(root.path(), BTreeSet::new());
        projection.graph = MetaMakeOwnerGraph::parse_expanded_files(
            &BTreeMap::from([
                (
                    "one/mmakefile".into(),
                    "#MM- top : first second\n#MM first\n#MM second\n".into(),
                ),
                ("two/mmakefile".into(), "#MM other\n".into()),
            ]),
            &BTreeMap::new(),
            Limits::default(),
        )
        .unwrap();
        let make_child = |cwd: &str, args: &[&str]| {
            NativeMakeInvocation::new(
                args.iter().map(|arg| (*arg).to_owned()),
                PathBuf::from(cwd),
                [],
            )
            .unwrap()
        };
        let child = make_child(
            "/selected/build/one",
            &["-s", "--file=mmakefile", "second", "first", "second"],
        );
        let matched = projection
            .match_captured_child_coordinates(
                root.path(),
                &["top".into()],
                Path::new("/selected/build"),
                "one/mmakefile",
                &child,
            )
            .unwrap();
        assert_eq!(matched.generated_owner(), "one/mmakefile");
        assert_eq!(matched.source_input(), "one/mmakefile.src");
        assert_eq!(matched.goals(), ["second", "first", "second"]);
        let batch = projection
            .match_captured_children_coordinates(
                root.path(),
                &["top".into()],
                Path::new("/selected/build"),
                &[("one/mmakefile", &child), ("one/mmakefile", &child)],
            )
            .unwrap();
        assert_eq!(batch.len(), 2);
        assert_eq!(batch[0].goals(), batch[1].goals());
        let foreign = make_child("/selected/build/one", &["--file=mmakefile", "other"]);
        assert!(projection
            .match_captured_children_coordinates(
                root.path(),
                &["top".into()],
                Path::new("/selected/build"),
                &[("one/mmakefile", &child), ("one/mmakefile", &foreign)],
            )
            .is_err());
        let excessive = vec![("one/mmakefile", &child); 4097];
        assert!(projection
            .match_captured_children_coordinates(
                root.path(),
                &["top".into()],
                Path::new("/selected/build"),
                &excessive,
            )
            .err()
            .unwrap()
            .contains("count bound"));
        for (cwd, args) in [
            ("/selected/build/two", vec!["--file=mmakefile", "first"]),
            ("/selected/build/one", vec!["--file=foreign", "first"]),
            (
                "/selected/build/one",
                vec!["--file=../one/mmakefile", "first"],
            ),
            ("/selected/build/one", vec!["--file=mmakefile", "other"]),
            ("/selected/build/one", vec!["first"]),
            ("/selected/build/one", vec!["--file=mmakefile"]),
            (
                "/selected/build/one",
                vec!["--file=mmakefile", "--file=extra", "first"],
            ),
        ] {
            assert!(projection
                .match_captured_child_coordinates(
                    root.path(),
                    &["top".into()],
                    Path::new("/selected/build"),
                    "one/mmakefile",
                    &make_child(cwd, &args),
                )
                .is_err());
        }
        assert!(projection
            .match_captured_child_coordinates(
                root.path(),
                &["top".into()],
                Path::new("/selected/build"),
                "two/mmakefile",
                &child,
            )
            .is_err());
        assert!(projection
            .match_captured_child_coordinates(
                root.path(),
                &["top".into()],
                Path::new("/selected/../build"),
                "one/mmakefile",
                &child,
            )
            .is_err());
        fs::write(root.path().join("one/mmakefile"), "new discovered input\n").unwrap();
        assert!(projection
            .match_captured_child_coordinates(
                root.path(),
                &["top".into()],
                Path::new("/selected/build"),
                "one/mmakefile",
                &child,
            )
            .is_err());
    }

    pub(super) fn projection_for_discovery(
        root: &Path,
        ignored_directories: BTreeSet<String>,
    ) -> NativeOwnerProjection {
        let discovery_original_inputs =
            discover_meta_inputs(root, &ignored_directories, &BTreeSet::new()).unwrap();
        let discovered_inputs: BTreeSet<_> = discovery_original_inputs.keys().cloned().collect();
        let owners = build_owner_map(discovered_inputs.clone());
        let metadata = owners
            .keys()
            .map(|owner| (owner.clone(), "#MM root\n".to_owned()))
            .collect();
        let graph = MetaMakeOwnerGraph::parse_with_declared_absence(
            &metadata,
            &BTreeMap::new(),
            &BTreeSet::new(),
            Limits::default(),
        )
        .unwrap();
        NativeOwnerProjection {
            optional_meta_dependencies: Vec::new(),
            architecture_context: TargetContext::default(),
            metamake_globals: BTreeMap::new(),
            runtime_configuration: None,
            sealed_configuration_inputs: BTreeMap::new(),
            graph,
            expansion_origins: BTreeMap::new(),
            expanded_program_sha256: BTreeMap::new(),
            architecture_hook_families: BTreeSet::new(),
            mesa26_original_producers: Vec::new(),
            owners,
            discovery_original_inputs,
            discovered_inputs,
            ignored_directories,
            excluded_paths: BTreeSet::new(),
            snapshots: BTreeMap::new(),
            policy_sha256: String::new(),
            expanded_bytes: 0,
        }
    }

    #[test]
    fn configured_ignore_requires_original_exact_exclusion_and_runtime_policy() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("mmakefile.src"), "#MM root\n").unwrap();
        let selected = || {
            let mut projection = projection_for_discovery(root.path(), BTreeSet::new());
            projection.excluded_paths.insert("build/fixture".into());
            projection.runtime_configuration = Some(RuntimeConfiguration {
                project: ConfiguredProject::parse("[AROS]\n", "AROS").unwrap(),
                absent: BTreeSet::new(),
            });
            projection
        };
        for path in [
            "build",
            "fixture",
            "build/other",
            "build/fixture-copy",
            "../build/fixture",
        ] {
            assert!(
                selected().with_configured_excluded_directory(path).is_err(),
                "{path}"
            );
        }
        let mut missing = selected();
        missing.runtime_configuration = None;
        assert!(missing
            .with_configured_excluded_directory("build/fixture")
            .is_err());
        let original = selected();
        let original_inputs = original.discovery_original_inputs.clone();
        let original_exclusions = original.excluded_paths.clone();
        let bound = original
            .with_configured_excluded_directory("build/fixture")
            .unwrap();
        assert_eq!(bound.discovery_original_inputs, original_inputs);
        assert_eq!(bound.excluded_paths, original_exclusions);
        assert_eq!(
            bound.ignored_directories,
            BTreeSet::from(["build/fixture".into()])
        );
        assert_eq!(
            bound
                .runtime_configuration
                .unwrap()
                .project
                .ignored_directories,
            BTreeSet::from(["build/fixture".into()])
        );
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
    fn assembly_header_proof_replays_sealed_source_and_concrete_ownership() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("hardware/header");
        fs::create_dir_all(&directory).unwrap();
        let recipe = "hardware/header/mmakefile.src";
        let owner = "hardware/header/mmakefile";
        let source = r#"OBJDIR := $(GENDIR)/header
GENINCDIR := $(GENDIR)/include
#MM
provider : $(GENINCDIR)/generated/table.h
#MM aggregate : prep provider
#MM
aggregate:
	@$(NOP)
GREPTOKEN := ".asciz"
$(OBJDIR)/table.s : $(SRCDIR)/$(CURDIR)/table.c | $(OBJDIR)
	@$(ECHO) "Compiling  $<..."
	@$(TARGET_CC) $(TARGET_SYSROOT) $(CFLAGS) $(PRIV_EXEC_INCLUDES) -S $< -o $@
$(GENINCDIR)/generated/table.h : $(OBJDIR)/table.s | $(GENINCDIR)/generated
	@$(ECHO) Generating $@...
	@grep $(GREPTOKEN) $< | cut -d'"' -f2 | sed 's/\$$//g' >$@
"#;
        fs::write(directory.join("mmakefile.src"), source).unwrap();
        fs::write(directory.join("table.c"), "int table;\n").unwrap();
        let context = TargetContext {
            toolchain: Some("gnu".into()),
            make_variables: BTreeMap::from([
                ("TARGET_CC".into(), "$(NATIVE_TARGET_CC)".into()),
                ("TARGET_SYSROOT".into(), String::new()),
                ("CFLAGS".into(), "-O2".into()),
                ("PRIV_EXEC_INCLUDES".into(), String::new()),
            ]),
            ..TargetContext::default()
        };
        let mut projection = projection_for_discovery(root.path(), BTreeSet::new());
        projection.architecture_context = context.clone();
        projection.snapshots.insert(
            recipe.into(),
            aros_common::sha256_bytes(source.as_bytes()).to_string(),
        );
        let metadata = BTreeMap::from([(
            owner.into(),
            "#MM provider\n#MM aggregate : prep provider\n".into(),
        )]);
        projection.graph = MetaMakeOwnerGraph::parse_with_declared_absence(
            &metadata,
            &BTreeMap::new(),
            &BTreeSet::new(),
            Limits::default(),
        )
        .unwrap();
        let mut dirs = crate::dirs::DirVars::load(root.path());
        dirs.bind_native_target_tool_roles();
        let (headers, rejections) = crate::assembly_headers::collect_from_snapshot(
            source,
            &context,
            &dirs,
            root.path(),
            Path::new("hardware/header"),
        );
        assert!(rejections.is_empty(), "{rejections:?}");
        assert_eq!(headers.len(), 1);
        projection
            .verify_assembly_headers(root.path(), &headers)
            .unwrap();
        fs::create_dir(root.path().join("config")).unwrap();
        let configuration = "AROS_DEVELOPER = $(TARGETDIR)/SYS/Developer\n";
        fs::write(root.path().join("config/make.cfg.in"), configuration).unwrap();
        assert!(projection
            .verify_assembly_headers(root.path(), &headers)
            .is_err());
        projection.sealed_configuration_inputs.insert(
            "config/make.cfg.in".into(),
            aros_common::sha256_bytes(configuration.as_bytes()).to_string(),
        );
        projection
            .verify_assembly_headers(root.path(), &headers)
            .unwrap();
        fs::write(
            root.path().join("config/make.cfg.in"),
            "AROS_DEVELOPER = changed\n",
        )
        .unwrap();
        assert!(projection
            .verify_assembly_headers(root.path(), &headers)
            .is_err());
        fs::write(root.path().join("config/make.cfg.in"), configuration).unwrap();
        let mut changed = headers.clone();
        changed[0].arguments.push("-O3".into());
        assert!(projection
            .verify_assembly_headers(root.path(), &changed)
            .is_err());
        let duplicates = vec![headers[0].clone(), headers[0].clone()];
        assert!(projection
            .verify_assembly_headers(root.path(), &duplicates)
            .is_err());
        let mut ambiguous = metadata.clone();
        ambiguous.insert("other/mmakefile".into(), "#MM provider\n".into());
        projection.graph = MetaMakeOwnerGraph::parse_with_declared_absence(
            &ambiguous,
            &BTreeMap::new(),
            &BTreeSet::new(),
            Limits::default(),
        )
        .unwrap();
        assert!(projection
            .verify_assembly_headers(root.path(), &headers)
            .is_err());
        projection.graph = MetaMakeOwnerGraph::parse_with_declared_absence(
            &metadata,
            &BTreeMap::new(),
            &BTreeSet::new(),
            Limits::default(),
        )
        .unwrap();
        fs::write(
            directory.join("mmakefile.src"),
            format!("{source}# changed\n"),
        )
        .unwrap();
        assert!(projection
            .verify_assembly_headers(root.path(), &headers)
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
    fn an_unselected_llvm_or_rust_selector_does_not_contradict_the_sealed_policy() {
        let mut globals = selector_globals();
        globals.insert("TARGET_LLVM_RUNTIMES_STYLE".into(), "separate".into());
        let mut context = selector_context();
        // CMake forwards the cache variable's empty default.
        context.target_llvm_runtimes_style = Some(String::new());
        assert!(validate_context_selectors(&globals, &BTreeSet::new(), &context).is_ok());
        // A selected value still has to match the policy.
        context.target_llvm_runtimes_style = Some("shared".into());
        assert!(
            validate_context_selectors(&globals, &BTreeSet::new(), &context)
                .unwrap_err()
                .contains("TARGET_LLVM_RUNTIMES_STYLE differs")
        );
        // The selectors that define the target keep comparing empty values.
        let mut wrong = selector_globals();
        wrong.insert("FAMILY".into(), "other".into());
        assert!(
            validate_context_selectors(&wrong, &BTreeSet::new(), &selector_context())
                .unwrap_err()
                .contains("FAMILY differs")
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
        let mut projection = projection_for_discovery(root.path(), BTreeSet::new());
        projection.snapshots = expected;
        assert!(projection.verify(root.path()).is_ok());
        fs::write(root.path().join("module/mmakefile.src"), "source-v2").unwrap();
        assert!(projection
            .verify(root.path())
            .unwrap_err()
            .contains("module/mmakefile.src"));
        fs::write(root.path().join("module/mmakefile.src"), "source-v1").unwrap();
        fs::write(root.path().join("config/make.tmpl"), "template-v2").unwrap();
        assert!(projection
            .verify(root.path())
            .unwrap_err()
            .contains("config/make.tmpl"));
    }

    #[test]
    fn verify_rejects_added_recipe_after_projection_load() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("original")).unwrap();
        fs::write(root.path().join("original/mmakefile.src"), "#MM root\n").unwrap();
        let projection = projection_for_discovery(root.path(), BTreeSet::new());

        fs::create_dir_all(root.path().join("added")).unwrap();
        fs::write(root.path().join("added/mmakefile"), "#MM added\n").unwrap();
        let error = projection.verify(root.path()).unwrap_err();
        assert!(error.contains("added/mmakefile"));
        assert!(error.contains("unexpected"));
    }

    #[test]
    fn verify_rejects_removed_recipe_after_projection_load() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("original")).unwrap();
        let recipe = root.path().join("original/mmakefile.src");
        fs::write(&recipe, "#MM root\n").unwrap();
        let projection = projection_for_discovery(root.path(), BTreeSet::new());

        fs::remove_file(recipe).unwrap();
        let error = projection.verify(root.path()).unwrap_err();
        assert!(error.contains("original/mmakefile.src"));
        assert!(error.contains("omitted"));
    }

    #[test]
    fn verify_rejects_generated_recipe_added_beside_source_recipe() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("module")).unwrap();
        fs::write(root.path().join("module/mmakefile.src"), "#MM source\n").unwrap();
        let projection = projection_for_discovery(root.path(), BTreeSet::new());
        assert_eq!(
            projection.owners.get("module/mmakefile"),
            Some(&"module/mmakefile.src".to_owned())
        );

        fs::write(root.path().join("module/mmakefile"), "#MM generated\n").unwrap();
        let error = projection.verify(root.path()).unwrap_err();
        assert!(error.contains("module/mmakefile"));
        assert!(error.contains("unexpected"));
    }

    #[test]
    fn effective_inputs_prefer_source_and_retain_direct_fragments() {
        let root = tempfile::tempdir().unwrap();
        for (path, contents) in [
            ("module/mmakefile.src", "#MM source\n"),
            ("module/mmakefile", "#MM stale-generated\n"),
            ("direct/mmakefile", "#MM direct\n"),
        ] {
            let path = root.path().join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents).unwrap();
        }
        let projection = projection_for_discovery(root.path(), BTreeSet::new());
        assert_eq!(
            projection.effective_inputs().collect::<BTreeSet<_>>(),
            BTreeSet::from(["direct/mmakefile", "module/mmakefile.src"])
        );
        assert_eq!(projection.discovered_inputs.len(), 3);
        assert!(projection.verify(root.path()).is_ok());
    }

    #[test]
    fn verify_accepts_recipes_added_under_project_ignored_directory() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("live")).unwrap();
        fs::write(root.path().join("live/mmakefile.src"), "#MM root\n").unwrap();
        let projection = projection_for_discovery(root.path(), BTreeSet::from(["vendor".into()]));

        fs::create_dir_all(root.path().join("vendor/nested")).unwrap();
        fs::write(
            root.path().join("vendor/nested/mmakefile.src"),
            "#MM ignored\n",
        )
        .unwrap();
        assert!(projection.verify(root.path()).is_ok());
    }

    #[test]
    #[cfg(unix)]
    fn source_owner_aliases_bind_logical_coordinates_and_original_bytes() {
        use std::os::unix::fs::symlink;
        let temporary = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(temporary.path()).unwrap();
        for directory in ["one", "two"] {
            fs::create_dir(root.join(directory)).unwrap();
            fs::write(root.join(directory).join("mmakefile.src"), "#MM root\n").unwrap();
        }
        symlink("one", root.join("alias")).unwrap();
        let template = root.join("template");
        fs::write(&template, "").unwrap();
        let mut projection = projection_for_discovery(&root, BTreeSet::new());
        let (expanded, bytes, origins, _) = expand_owners(
            &root,
            &projection.owners,
            &projection.discovery_original_inputs,
            &template,
            &BTreeMap::from([(
                "template".into(),
                aros_common::sha256_bytes(b"").to_string(),
            )]),
            &mut projection.snapshots,
        )
        .unwrap();
        projection.graph = MetaMakeOwnerGraph::parse_expanded_files(
            &expanded,
            &BTreeMap::new(),
            Limits::default(),
        )
        .unwrap();
        projection.expanded_bytes = bytes;
        projection.expansion_origins = origins;
        assert_eq!(
            projection.original_input("alias/mmakefile.src"),
            Some("one/mmakefile.src")
        );
        assert_eq!(
            projection.snapshots()["alias/mmakefile.src"],
            projection.snapshots()["one/mmakefile.src"]
        );
        projection.verify(&root).unwrap();
        let selected = projection.select(&["root".into()]).unwrap();
        assert_eq!(selected.selected_owner_targets.len(), 3);
        assert!(selected
            .selected_owner_targets
            .contains_key("alias/mmakefile"));
        // Same hashes and logical inputs, different original Source referent.
        fs::remove_file(root.join("alias")).unwrap();
        symlink("two", root.join("alias")).unwrap();
        assert!(projection
            .verify(&root)
            .unwrap_err()
            .contains("referents changed"));
        fs::remove_file(root.join("alias")).unwrap();
        symlink("one", root.join("alias")).unwrap();
        projection.verify(&root).unwrap();
        fs::write(root.join("one/mmakefile.src"), "#MM changed\n").unwrap();
        assert!(projection
            .verify(&root)
            .unwrap_err()
            .contains("input changed"));
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
            verify_complete_discovery(root.path(), &supplied, &ignored, &BTreeSet::new()).unwrap(),
            BTreeMap::from([
                ("build/mmakefile.src".into(), "build/mmakefile.src".into()),
                ("live/mmakefile.src".into(), "live/mmakefile.src".into()),
            ])
        );
        let main_pruned = vec![root.path().join("live/mmakefile.src")];
        let error =
            verify_complete_discovery(root.path(), &main_pruned, &ignored, &BTreeSet::new())
                .unwrap_err();
        assert!(error.contains("build/mmakefile.src"));
        assert!(error.contains("omitted"));
    }

    #[test]
    fn discovery_path_ignoredir_is_exact_and_does_not_ignore_file_basenames() {
        let root = tempfile::tempdir().unwrap();
        for path in [
            "live/mmakefile.src",
            "build/arm-raspi/mmakefile.src",
            "build/arm-raspi/deep/mmakefile.src",
            "build/arm-raspi-copy/mmakefile.src",
            "build/other/mmakefile.src",
            "vendor/arm-raspi/mmakefile.src",
        ] {
            let file = root.path().join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, "#MM root\n").unwrap();
        }
        let ignored = BTreeSet::from(["build/arm-raspi".to_owned()]);
        let files = [
            "live/mmakefile.src",
            "build/arm-raspi/mmakefile.src",
            "build/arm-raspi/deep/mmakefile.src",
            "build/arm-raspi-copy/mmakefile.src",
            "build/other/mmakefile.src",
            "vendor/arm-raspi/mmakefile.src",
        ]
        .map(|path| root.path().join(path));
        let found =
            verify_complete_discovery(root.path(), &files, &ignored, &BTreeSet::new()).unwrap();
        assert_eq!(
            found.keys().map(String::as_str).collect::<Vec<_>>(),
            [
                "build/arm-raspi-copy/mmakefile.src",
                "build/other/mmakefile.src",
                "live/mmakefile.src",
                "vendor/arm-raspi/mmakefile.src",
            ]
        );

        assert!(!path_has_ignored_directory(
            Path::new("build/arm-raspi"),
            &ignored
        ));
        let bare = BTreeSet::from(["arm-raspi".to_owned()]);
        assert!(path_has_ignored_directory(
            Path::new("vendor/arm-raspi/mmakefile.src"),
            &bare
        ));
        assert!(!path_has_ignored_directory(Path::new("arm-raspi"), &bare));
    }

    #[test]
    fn an_excluded_build_directory_is_left_out_by_exact_path_only() {
        let root = tempfile::tempdir().unwrap();
        for path in [
            "live/mmakefile.src",
            // The engine's in-tree build tree and its generated recipe.
            "build/esp32p4-d1001/SDK/include/mmakefile.src",
            // A source directory that merely shares the basename.
            "vendor/build/mmakefile.src",
            // A sibling build tree of another profile is not excluded.
            "build/other/mmakefile.src",
        ] {
            let file = root.path().join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, "#MM root\n").unwrap();
        }
        let excluded = BTreeSet::from(["build/esp32p4-d1001".to_owned()]);
        let found = discover_meta_files(root.path(), &BTreeSet::new(), &excluded).unwrap();
        assert_eq!(
            found,
            BTreeSet::from([
                "build/other/mmakefile.src".into(),
                "live/mmakefile.src".into(),
                "vendor/build/mmakefile.src".into(),
            ])
        );
        // The main walk's list is checked against the same exclusion.
        let supplied: Vec<_> = found.iter().map(|name| root.path().join(name)).collect();
        assert!(
            verify_complete_discovery(root.path(), &supplied, &BTreeSet::new(), &excluded).is_ok()
        );
    }
}
