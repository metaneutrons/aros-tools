//! Bounded, data-only handoff for an already-produced Mesa configure result.
//!
//! This reader checks content bindings, paths, and local filesystem stability.
//! It does not authenticate a producer or establish that configure ran.

use crate::mesa_configuration::{MesaConfigurationIdentity, ValidatedMesaConfiguration};
pub use crate::mesa_genmodule_collection::MESA_GENMODULE_SELECTION_FILE;
use crate::mesa_genmodule_collection::{
    load_genmodule_collection, LoadedMesaGenmoduleCollection, MesaGenmoduleCollectionBinding,
};
use crate::mesa_genmodule_make::MesaGenmoduleMakefile;
use crate::publication::{measure_regular_file_bounded, measure_tree_content_cas_bounded};
use crate::{sha256_bytes, ArosError, Result, Sha256Digest, TreeContentCas, TreeTraversalLimits};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::path::{Component, Path, PathBuf};

/// Exact filename for the Mesa transpiler input document.
pub const INPUT_FILE: &str = "mesa-transpiler-input.json";
/// Exact raw Source genmodule Make projection handoff filename.
pub const MESA_GENMODULE_MAKE_FILE: &str = "mesa-genmodule.make";
/// Exact source-relative base configuration bound by the Mesa genmodule handoff.
pub const MESA_GENMODULE_BASE_CONFIG: &str = "workbench/libs/gl/gl.conf";
/// Exact source-relative Mesa override configuration bound by the handoff.
pub const MESA_GENMODULE_OVERRIDE_CONFIG: &str = "workbench/libs/mesa/mesa3dgl.conf";

const CONFIGURATION_FILE: &str = "mesa-configuration.json";
const RECEIPT_FILE: &str = "receipt.json";
const MAX_CONTROL_FILE_BYTES: u64 = 1024 * 1024;
const MAX_NATIVE_STAGE_BYTES: u64 = 256 * 1024;
const MAX_INPUT_ROOT_ENTRIES: usize = 100_000;
const MAX_INPUT_ROOT_FILE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const RECEIPT_SCHEMA: &str = "aros-mesa-preflight-v1";
const RECEIPT_QUALIFICATION: &str = "local-configure-only";

/// Strict binding document handed to the Mesa transpiler subprocess.
///
/// Its digests bind the exact configuration and preflight receipt bytes. They
/// do not authenticate who wrote those files or establish that configure ran.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MesaConfigurationInputDocument {
    pub version: u32,
    pub identity: MesaConfigurationIdentity,
    pub configuration_sha256: Sha256Digest,
    pub receipt_sha256: Sha256Digest,
    /// Versions 2, 4, 6, 8 and 10: exact local source selection, never arbitrary exclusions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_source: Option<crate::local_source::LocalSourceIdentity>,
    /// Versions 3 and 4 only: exact bindings for the original genmodule
    /// Make projection and its two selected source configurations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genmodule: Option<MesaGenmoduleBinding>,
    /// Versions 5 through 10: a bounded multi-producer inventory/output handoff.
    /// Versions 7 and 8 additionally require one context-bound initial scope
    /// observation per selected producer in the exact receipt stage sequence.
    /// Versions 9 and 10 instead require the receipt's separately bound native
    /// call collection. That is data, not an ordinary preparation assertion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genmodule_collection: Option<MesaGenmoduleCollectionBinding>,
}

/// Data-only digests binding the Mesa genmodule Make projection to its inputs.
///
/// These fields do not authenticate the producer or prove that the original
/// genmodule executable ran.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MesaGenmoduleBinding {
    pub module: String,
    pub makefile_sha256: Sha256Digest,
    pub base_configuration_sha256: Sha256Digest,
    pub override_configuration_sha256: Sha256Digest,
    pub producer_sha256: Sha256Digest,
}

/// Validated data-only Mesa input retained with a whole-root filesystem snapshot.
///
/// A successful load means the files were locally consistent and unchanged
/// during loading. It is not historical producer proof, configure execution
/// proof, or producer authentication.
#[derive(Debug, Clone)]
pub struct LoadedMesaConfigurationInput {
    compiler_programs: Option<crate::captured_programs::CapturedProgramSet>,
    configuration: ValidatedMesaConfiguration,
    input_sha256: Sha256Digest,
    receipt_sha256: Sha256Digest,
    root: PathBuf,
    root_snapshot: TreeContentCas,
    local_source: Option<crate::local_source::LocalSourceIdentity>,
    genmodule_makefile: Option<MesaGenmoduleMakefile>,
    genmodule_binding: Option<MesaGenmoduleBinding>,
    genmodule_collection: Option<LoadedMesaGenmoduleCollection>,
    initial_scopes: BTreeMap<String, crate::make_scope_snapshot::ValidatedMakeScopeSnapshot>,
    native_make: Option<crate::mesa_native_make::LoadedMesaNativeMake>,
}

impl LoadedMesaConfigurationInput {
    /// Optional independently transported compiler-tree/driver data.
    /// Legacy receipts have none; absence cannot authorize a native graph path.
    /// These are filesystem-consistent identities, not package authentication.
    #[must_use]
    pub const fn compiler_programs(&self) -> Option<&crate::captured_programs::CapturedProgramSet> {
        self.compiler_programs.as_ref()
    }
    /// Local source selection for versions 2, 4, and 6. A consumer must verify it
    /// against the actual source before using its generated-subtree exclusion.
    #[must_use]
    pub const fn local_source(&self) -> Option<&crate::local_source::LocalSourceIdentity> {
        self.local_source.as_ref()
    }

    /// Parsed original genmodule Make data for input schema versions 3 and 4.
    #[must_use]
    pub const fn genmodule_makefile(&self) -> Option<&MesaGenmoduleMakefile> {
        self.genmodule_makefile.as_ref()
    }

    /// Exact genmodule Make/source digest binding for input schema versions 3 and 4.
    #[must_use]
    pub const fn genmodule_binding(&self) -> Option<&MesaGenmoduleBinding> {
        self.genmodule_binding.as_ref()
    }

    /// Validated multi-producer collection for input schema versions 5 and 6.
    #[must_use]
    pub const fn genmodule_collection(&self) -> Option<&LoadedMesaGenmoduleCollection> {
        self.genmodule_collection.as_ref()
    }

    /// Original-owner scope data, not a complete native child startup packet.
    /// These records do not replace independently bound argv/environment,
    /// observation phase or producer evidence. Legacy inputs provide no scopes.
    #[must_use]
    pub const fn initial_scopes(
        &self,
    ) -> &BTreeMap<String, crate::make_scope_snapshot::ValidatedMakeScopeSnapshot> {
        &self.initial_scopes
    }

    /// Native packets with independently transported owner/call associations.
    /// These are not legacy root-prefix scopes or execution authentication.
    #[must_use]
    pub const fn native_make(&self) -> Option<&crate::mesa_native_make::LoadedMesaNativeMake> {
        self.native_make.as_ref()
    }

    /// Validated configure data bound to the input document's identity.
    #[must_use]
    pub const fn configuration(&self) -> &ValidatedMesaConfiguration {
        &self.configuration
    }

    /// SHA-256 of the exact input document bytes read during loading.
    #[must_use]
    pub const fn input_sha256(&self) -> &Sha256Digest {
        &self.input_sha256
    }

    /// SHA-256 of the exact receipt bytes bound by the input document.
    #[must_use]
    pub const fn receipt_sha256(&self) -> &Sha256Digest {
        &self.receipt_sha256
    }

    /// Canonical directory containing the input, configuration, and receipt.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Check that the complete input root still matches its load-time snapshot.
    ///
    /// This is a local no-drift check. It does not establish when or by whom
    /// the files were produced.
    ///
    /// # Errors
    /// Rejects unsafe, unmeasurable or changed input trees.
    pub fn revalidate(&self) -> Result<()> {
        if let Some(programs) = &self.compiler_programs {
            programs.verify().map_err(|_| {
                configuration_error(RECEIPT_FILE, "compiler program roots changed after loading")
            })?;
        }
        let current = snapshot_input_root(&self.root)?;
        if current != self.root_snapshot {
            return Err(configuration_error(
                INPUT_FILE,
                "the complete Mesa input root changed after it was loaded",
            ));
        }
        Ok(())
    }
}

/// Load a bounded Mesa subprocess handoff from its canonical input path.
///
/// The input file must be the exact `INPUT_FILE` leaf below a canonical
/// absolute directory. The input document's SHA-256, configuration digest,
/// receipt digest, identity, Make projection or genmodule collection, and
/// measured `compiler.cfg` are checked. The complete input root is snapshotted
/// before and after loading.
///
/// This establishes consistency at load time and permits later drift checks;
/// it does not authenticate the producer or prove configure was executed.
///
/// # Errors
/// Rejects unsafe/oversized paths and files, inconsistent bindings, unsupported
/// documents, incomplete Make values, and filesystem changes during loading.
pub fn load_mesa_configuration_input(
    path: &Path,
    expected_sha256: &Sha256Digest,
) -> Result<LoadedMesaConfigurationInput> {
    let (root, canonical_input) = validate_input_path(path)?;
    let before = snapshot_input_root(&root)?;

    let input_bytes = read_regular_bounded(&canonical_input, INPUT_FILE)?;
    let input_sha256 = sha256_bytes(&input_bytes);
    if &input_sha256 != expected_sha256 {
        return Err(configuration_error(
            INPUT_FILE,
            "exact input document SHA-256 differs from the caller-provided expected digest",
        ));
    }
    let document: MesaConfigurationInputDocument = serde_json::from_slice(&input_bytes)
        .map_err(|error| configuration_error(INPUT_FILE, format!("invalid JSON: {error}")))?;
    if !matches!(
        (
            document.version,
            document.local_source.is_some(),
            document.genmodule.is_some(),
            document.genmodule_collection.is_some()
        ),
        (1, false, false, false)
            | (2, true, false, false)
            | (3, false, true, false)
            | (4, true, true, false)
            | (5 | 7 | 9, false, false, true)
            | (6 | 8 | 10, true, false, true)
    ) {
        return Err(configuration_error(
            INPUT_FILE,
            format!(
                "unsupported Mesa configuration input version {}",
                document.version
            ),
        ));
    }
    if document
        .local_source
        .as_ref()
        .is_some_and(|local| local.content_sha256 != document.identity.source_inputs_sha256)
    {
        return Err(configuration_error(
            INPUT_FILE,
            "local source digest differs from the configured identity",
        ));
    }

    let configuration_path = root.join(CONFIGURATION_FILE);
    let configuration_bytes = read_regular_bounded(&configuration_path, CONFIGURATION_FILE)?;
    let configuration_sha256 = sha256_bytes(&configuration_bytes);
    if configuration_sha256 != document.configuration_sha256 {
        return Err(configuration_error(
            CONFIGURATION_FILE,
            "exact configuration SHA-256 differs from the input document",
        ));
    }

    let receipt_path = root.join(RECEIPT_FILE);
    let receipt_bytes = read_regular_bounded(&receipt_path, RECEIPT_FILE)?;
    let receipt_sha256 = sha256_bytes(&receipt_bytes);
    if receipt_sha256 != document.receipt_sha256 {
        return Err(configuration_error(
            RECEIPT_FILE,
            "exact receipt SHA-256 differs from the input document",
        ));
    }

    let receipt: MesaPreflightReceipt = serde_json::from_slice(&receipt_bytes)
        .map_err(|_| configuration_error(RECEIPT_FILE, "invalid preflight receipt JSON"))?;
    validate_receipt(&receipt, &document)?;

    let configuration =
        ValidatedMesaConfiguration::parse_bound(&configuration_bytes, &document.identity)?;
    if let Some(selected) = &receipt.compiler_programs {
        validate_compiler_program_selection(selected, &configuration)?;
    }
    if let Some(namespace) = configuration.initial_namespace() {
        let expected_stages = if let Some(collection) = &document.genmodule_collection {
            if matches!(document.version, 9 | 10) {
                9 + collection.outputs.len()
            } else {
                6 + collection.outputs.len()
                    * if matches!(document.version, 7 | 8) {
                        2
                    } else {
                        1
                    }
            }
        } else if document.genmodule.is_some() {
            6
        } else {
            4
        };
        if receipt.stages.len() != expected_stages {
            return Err(configuration_error(
                RECEIPT_FILE,
                "namespace requires the exact configured stage sequence",
            ));
        }
        // The fixed four configure stages put the final Make observation at
        // index 3. This is retained-byte consistency, not execution authority.
        let stage = receipt.stages.get(3).ok_or_else(|| {
            configuration_error(
                RECEIPT_FILE,
                "namespace requires the configured-values stage digest",
            )
        })?;
        let stdout = read_regular_bounded(
            &root.join("configured-values.stdout"),
            "configured-values.stdout",
        )?;
        if sha256_bytes(&stdout) != stage.stdout {
            return Err(configuration_error(
                RECEIPT_FILE,
                "namespace Make stdout differs from its stage digest",
            ));
        }
        let observed = crate::make_namespace::ValidatedMakeNamespace::from_make_stdout(&stdout)?;
        if &observed != namespace {
            return Err(configuration_error(
                RECEIPT_FILE,
                "configuration namespace differs from retained Make stdout",
            ));
        }
        if let Some(values) = configuration.initial_values() {
            let retained =
                crate::make_initial_values::ValidatedMakeInitialValues::from_make_stdout(
                    &stdout, namespace,
                )?;
            if &retained != values {
                return Err(configuration_error(
                    RECEIPT_FILE,
                    "configuration initial values differ from retained Make stdout",
                ));
            }
        }
    }
    let make_values = configuration.make_variables();
    if receipt.make_values != make_values {
        return Err(configuration_error(
            RECEIPT_FILE,
            "receipt make_values differ from the complete validated configuration projection",
        ));
    }

    let gendir = validate_gendir(&root, &receipt.configured_paths.gendir)?;
    let coordinates = &configuration.recipe_configuration().make_coordinates;
    if Path::new(&coordinates.build_root) != root.join("configure")
        || Path::new(&coordinates.generated_root) != gendir
        || coordinates.ports_root != receipt.configured_paths.portsdir
    {
        return Err(configuration_error(
            RECEIPT_FILE,
            "measured Mesa coordinates differ from the selected configure owner paths",
        ));
    }
    let compiler_cfg_path = gendir.join("config").join("compiler.cfg");
    let compiler_cfg_bytes = read_regular_bounded(&compiler_cfg_path, "compiler.cfg")?;
    if sha256_bytes(&compiler_cfg_bytes) != receipt.compiler_cfg_sha256 {
        return Err(configuration_error(
            "compiler.cfg",
            "exact compiler.cfg SHA-256 differs from the preflight receipt",
        ));
    }

    let genmodule_makefile = match document.genmodule.as_ref() {
        Some(binding) => Some(load_genmodule_makefile(&root, coordinates, binding)?),
        None => None,
    };
    let genmodule_collection = match document.genmodule_collection.as_ref() {
        Some(binding) => Some(load_genmodule_collection(
            &root,
            Path::new(&coordinates.source_root),
            binding,
        )?),
        None => None,
    };
    let mut initial_scopes = BTreeMap::new();
    if matches!(document.version, 7 | 8) {
        if configuration.initial_values().is_none() {
            return Err(configuration_error(
                RECEIPT_FILE,
                "owner scopes require version-3 configuration values",
            ));
        }
        let collection = genmodule_collection.as_ref().ok_or_else(|| {
            configuration_error(
                RECEIPT_FILE,
                "owner scopes require their selected collection",
            )
        })?;
        let configured = &receipt.stages[3];
        for (index, producer) in collection.selection().producers().iter().enumerate() {
            let stage = &receipt.stages[6 + 2 * index];
            if stage.program.is_none()
                || stage.environment.is_none()
                || stage.program != configured.program
                || stage.environment != configured.environment
            {
                return Err(configuration_error(
                    RECEIPT_FILE,
                    "owner scope Make program/environment differs from configuration",
                ));
            }
            let context = crate::mesa_genmodule_collection::mesa_genmodule_scope_context(
                producer,
                &document.identity,
                &configured.stdout,
                coordinates,
            )?;
            let name =
                crate::mesa_genmodule_collection::mesa_genmodule_scope_stage(&producer.owner)
                    .map_err(|_| configuration_error(RECEIPT_FILE, "unsafe owner scope stage"))?;
            let stdout =
                read_regular_bounded(&root.join(format!("{name}.stdout")), "owner scope stdout")?;
            if sha256_bytes(&stdout) != stage.stdout {
                return Err(configuration_error(
                    RECEIPT_FILE,
                    "owner scope stdout differs from its stage digest",
                ));
            }
            let scope = crate::make_scope_snapshot::ValidatedMakeScopeSnapshot::from_make_stdout(
                &stdout, &context,
            )?;
            initial_scopes.insert(producer.owner.clone(), scope);
        }
    }

    let native_make = if let Some(binding) = &receipt.native_make {
        let collection = genmodule_collection.as_ref().ok_or_else(|| {
            configuration_error(
                RECEIPT_FILE,
                "native calls require their selected collection",
            )
        })?;
        if configuration.initial_values().is_none() {
            return Err(configuration_error(
                RECEIPT_FILE,
                "native calls require measured initial values",
            ));
        }
        validate_native_stages(&receipt, binding)?;
        let stage_names = crate::mesa_native_make::native_preflight_stage_names(
            collection.selection().producers(),
        )?;
        if receipt.stages.len() != stage_names.len() {
            return Err(configuration_error(
                RECEIPT_FILE,
                "native stage sequence is incomplete",
            ));
        }
        for (name, stage) in stage_names.iter().zip(&receipt.stages) {
            let stderr = stage.stderr.as_ref().ok_or_else(|| {
                configuration_error(RECEIPT_FILE, "native stage lacks stderr identity")
            })?;
            for (suffix, expected) in [("stdout", &stage.stdout), ("stderr", stderr)] {
                let bytes = read_native_file(
                    &root.join(format!("{name}.{suffix}")),
                    MAX_NATIVE_STAGE_BYTES,
                )?;
                if sha256_bytes(&bytes) != *expected {
                    return Err(configuration_error(
                        RECEIPT_FILE,
                        "native stage log differs from its binding",
                    ));
                }
            }
        }
        let selected = document
            .genmodule_collection
            .as_ref()
            .ok_or_else(|| configuration_error(RECEIPT_FILE, "native selection is absent"))?;
        if receipt.stages[7].program.as_ref() != Some(&selected.helper_sha256)
            || receipt.stages[8].program != receipt.stages[3].program
            || receipt.stages[8].environment != receipt.stages[3].environment
            || receipt.stages[9..]
                .iter()
                .any(|stage| stage.program.as_ref() != Some(&selected.producer_sha256))
        {
            return Err(configuration_error(
                RECEIPT_FILE,
                "native generator stage identity differs",
            ));
        }
        let data = crate::mesa_native_make::load_native_make(
            binding,
            &document.identity,
            &receipt.stages[3].stdout,
            coordinates,
            collection.selection().producers(),
            |name, limit| read_native_file(&root.join(name), limit),
        )?;
        let prepared = crate::measure_tree_content_cas(&root.join("configure"))?;
        if prepared.payload_digest_excluding(None) != binding.prepared_tree_sha256 {
            return Err(configuration_error(
                RECEIPT_FILE,
                "native prepared input tree differs",
            ));
        }
        Some(data)
    } else {
        None
    };

    let (compiler_programs, after) = if let Some(selected) = &receipt.compiler_programs {
        let (programs, after) = crate::captured_programs::CapturedProgramSet::capture_with(
            &selected.roots,
            &selected.programs,
            |_| {
                let after = snapshot_input_root(&root)
                    .map_err(|_| "could not revalidate complete input root".to_owned())?;
                if before != after {
                    return Err("complete input root changed during loading".to_owned());
                }
                Ok(after)
            },
        )
        .map_err(|_| {
            configuration_error(
                RECEIPT_FILE,
                "compiler program or input roots differ from selected receipt data",
            )
        })?;
        if after != snapshot_input_root(&root)? {
            return Err(configuration_error(
                INPUT_FILE,
                "the complete Mesa input root changed during compiler revalidation",
            ));
        }
        (Some(programs), after)
    } else {
        let after = snapshot_input_root(&root)?;
        if before != after {
            return Err(configuration_error(
                INPUT_FILE,
                "the complete Mesa input root changed while it was being loaded",
            ));
        }
        (None, after)
    };

    Ok(LoadedMesaConfigurationInput {
        compiler_programs,
        configuration,
        input_sha256,
        receipt_sha256,
        root,
        root_snapshot: after,
        local_source: document.local_source,
        genmodule_makefile,
        genmodule_binding: document.genmodule,
        genmodule_collection,
        initial_scopes,
        native_make,
    })
}

#[derive(Debug, Deserialize)]
struct MesaPreflightReceipt {
    schema: String,
    qualification: String,
    identity: MesaConfigurationIdentity,
    configuration_sha256: Sha256Digest,
    compiler_cfg_sha256: Sha256Digest,
    #[serde(default, deserialize_with = "present_compiler_programs")]
    compiler_programs: Option<crate::captured_programs::ProgramSetSelection>,
    configured_paths: ConfiguredPaths,
    make_values: BTreeMap<String, String>,
    #[serde(default)]
    stages: Vec<MesaPreflightStage>,
    #[serde(default)]
    genmodule: Option<MesaGenmoduleBinding>,
    #[serde(default)]
    genmodule_collection: Option<MesaGenmoduleCollectionBinding>,
    #[serde(default, deserialize_with = "present_native_binding")]
    native_make: Option<crate::mesa_native_make::MesaNativeMakeBinding>,
}

fn present_compiler_programs<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<crate::captured_programs::ProgramSetSelection>, D::Error> {
    <crate::captured_programs::ProgramSetSelection as serde::Deserialize>::deserialize(deserializer)
        .map(Some)
}

/// Check optional compiler-role transport against the configured identity.
///
/// This reads no files. Physical consumers must capture/revalidate the program
/// set; portable evidence can establish only retained data agreement here.
/// The root path is transported explicitly, never inferred from driver names.
/// # Errors
/// Refuses malformed selections, unrelated roots/digests, absent C/C++ roles or
/// invocation spellings that differ from measured Source configuration.
pub fn validate_compiler_program_selection(
    selected: &crate::captured_programs::ProgramSetSelection,
    configuration: &ValidatedMesaConfiguration,
) -> Result<()> {
    selected
        .validate()
        .map_err(|_| configuration_error(RECEIPT_FILE, "compiler program selection is invalid"))?;
    if selected.programs.len() != 2 {
        return Err(configuration_error(
            RECEIPT_FILE,
            "compiler program selection must contain exactly the configured C and C++ roles",
        ));
    }
    if selected.roots.len() != 1
        || selected.roots[0].content_sha256 != configuration.identity().compiler_tree_sha256
    {
        return Err(configuration_error(
            RECEIPT_FILE,
            "compiler program root differs from configured tree identity",
        ));
    }
    let recipe = configuration.recipe_configuration();
    for (role, invocation) in [
        ("target-c", &recipe.target_cc),
        ("target-cxx", &recipe.target_cxx),
    ] {
        let program = selected
            .programs
            .iter()
            .find(|program| program.role == role)
            .ok_or_else(|| {
                configuration_error(RECEIPT_FILE, "configured compiler program role is absent")
            })?;
        if program.root_index != 0
            || program.invocation_path.as_os_str() != std::ffi::OsStr::new(invocation)
        {
            return Err(configuration_error(
                RECEIPT_FILE,
                "compiler program invocation differs from configured role",
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct MesaPreflightStage {
    stdout: Sha256Digest,
    #[serde(default)]
    program: Option<Sha256Digest>,
    #[serde(default)]
    environment: Option<Sha256Digest>,
    #[serde(default)]
    invocation: Option<Sha256Digest>,
    #[serde(default)]
    stderr: Option<Sha256Digest>,
    #[serde(default, deserialize_with = "present_exit_status")]
    exit_status: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct ConfiguredPaths {
    #[serde(rename = "GENDIR")]
    gendir: String,
    #[serde(rename = "PORTSDIR")]
    portsdir: String,
}

fn validate_receipt(
    receipt: &MesaPreflightReceipt,
    document: &MesaConfigurationInputDocument,
) -> Result<()> {
    let expected_schema = match document.version {
        1 => RECEIPT_SCHEMA,
        2 => "aros-mesa-preflight-v2",
        3 => "aros-mesa-preflight-v3",
        4 => "aros-mesa-preflight-v4",
        5 => "aros-mesa-preflight-v5",
        6 => "aros-mesa-preflight-v6",
        7 => "aros-mesa-preflight-v7",
        8 => "aros-mesa-preflight-v8",
        9 => "aros-mesa-preflight-v9",
        10 => "aros-mesa-preflight-v10",
        _ => unreachable!("document version is validated before receipt parsing"),
    };
    if receipt.schema != expected_schema {
        return Err(configuration_error(
            RECEIPT_FILE,
            format!("unsupported receipt schema {:?}", receipt.schema),
        ));
    }
    if receipt.qualification != RECEIPT_QUALIFICATION {
        return Err(configuration_error(
            RECEIPT_FILE,
            format!(
                "unsupported receipt qualification {:?}",
                receipt.qualification
            ),
        ));
    }
    if receipt.identity != document.identity {
        return Err(configuration_error(
            RECEIPT_FILE,
            "receipt identity differs from the input document",
        ));
    }
    if receipt.configuration_sha256 != document.configuration_sha256 {
        return Err(configuration_error(
            RECEIPT_FILE,
            "receipt configuration SHA-256 differs from the input document",
        ));
    }
    if receipt.genmodule.as_ref() != document.genmodule.as_ref() {
        return Err(configuration_error(
            RECEIPT_FILE,
            "receipt genmodule binding differs from the input document",
        ));
    }
    if receipt.genmodule_collection.as_ref() != document.genmodule_collection.as_ref() {
        return Err(configuration_error(
            RECEIPT_FILE,
            "receipt genmodule collection binding differs from the input document",
        ));
    }
    if matches!(document.version, 9 | 10) != receipt.native_make.is_some()
        || (!matches!(document.version, 9 | 10)
            && receipt
                .stages
                .iter()
                .any(|stage| stage.exit_status.is_some()))
    {
        return Err(configuration_error(
            RECEIPT_FILE,
            "native binding/status does not match its receipt version",
        ));
    }
    Ok(())
}

fn present_native_binding<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<crate::mesa_native_make::MesaNativeMakeBinding>, D::Error> {
    <crate::mesa_native_make::MesaNativeMakeBinding as Deserialize>::deserialize(deserializer)
        .map(Some)
}

fn present_exit_status<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<i32>, D::Error> {
    <i32 as Deserialize>::deserialize(deserializer).map(Some)
}

fn validate_native_stages(
    receipt: &MesaPreflightReceipt,
    binding: &crate::mesa_native_make::MesaNativeMakeBinding,
) -> Result<()> {
    if receipt.stages.len() < 9
        || receipt.stages.iter().any(|stage| {
            stage.program.is_none()
                || stage.environment.is_none()
                || stage.invocation.is_none()
                || stage.stderr.is_none()
        })
    {
        return Err(configuration_error(
            RECEIPT_FILE,
            "native receipt requires complete stage identities",
        ));
    }
    if receipt
        .stages
        .iter()
        .enumerate()
        .any(|(index, stage)| !(4..7).contains(&index) && stage.exit_status.is_some())
    {
        return Err(configuration_error(
            RECEIPT_FILE,
            "native receipt has a status outside its native stages",
        ));
    }
    let stages = receipt.stages[4..7]
        .iter()
        .map(|stage| {
            Ok(
                crate::mesa_native_make::MesaNativePreparationStageIdentity {
                    program: stage.program.clone().unwrap(),
                    invocation: stage.invocation.clone().unwrap(),
                    environment: stage.environment.clone().unwrap(),
                    exit_status: stage.exit_status.ok_or_else(|| {
                        configuration_error(RECEIPT_FILE, "native stage lacks explicit exit status")
                    })?,
                },
            )
        })
        .collect::<Result<Vec<_>>>()?;
    crate::mesa_native_make::validate_native_preparation_stages(
        binding,
        receipt.stages[3].program.as_ref().unwrap(),
        receipt.stages[3].environment.as_ref().unwrap(),
        &stages,
    )
}

fn read_native_file(path: &Path, limit: u64) -> Result<Vec<u8>> {
    measure_regular_file_bounded(path, limit)
        .map_err(|_| configuration_error(RECEIPT_FILE, "native handoff member cannot be measured"))?
        .map(|(_, bytes)| bytes)
        .ok_or_else(|| configuration_error(RECEIPT_FILE, "native handoff member is absent"))
}

fn load_genmodule_makefile(
    input_root: &Path,
    coordinates: &crate::mesa_configuration::MesaMakeCoordinates,
    binding: &MesaGenmoduleBinding,
) -> Result<MesaGenmoduleMakefile> {
    if binding.module != "mesa3dgl26-0" {
        return Err(configuration_error(
            MESA_GENMODULE_MAKE_FILE,
            format!(
                "unsupported Mesa genmodule module {:?}; expected mesa3dgl26-0",
                binding.module
            ),
        ));
    }

    let makefile_path = input_root.join(MESA_GENMODULE_MAKE_FILE);
    let makefile_bytes = read_regular_bounded(&makefile_path, MESA_GENMODULE_MAKE_FILE)?;
    if sha256_bytes(&makefile_bytes) != binding.makefile_sha256 {
        return Err(configuration_error(
            MESA_GENMODULE_MAKE_FILE,
            "exact genmodule Make SHA-256 differs from the input binding",
        ));
    }
    let makefile_text = std::str::from_utf8(&makefile_bytes).map_err(|error| {
        configuration_error(
            MESA_GENMODULE_MAKE_FILE,
            format!("genmodule Make output is not UTF-8: {error}"),
        )
    })?;
    let parsed = MesaGenmoduleMakefile::parse(&binding.module, makefile_text).map_err(|error| {
        configuration_error(
            MESA_GENMODULE_MAKE_FILE,
            format!("invalid literal genmodule Make projection: {error}"),
        )
    })?;

    let source_root = Path::new(&coordinates.source_root);
    for (relative, expected, context) in [
        (
            MESA_GENMODULE_BASE_CONFIG,
            &binding.base_configuration_sha256,
            MESA_GENMODULE_BASE_CONFIG,
        ),
        (
            MESA_GENMODULE_OVERRIDE_CONFIG,
            &binding.override_configuration_sha256,
            MESA_GENMODULE_OVERRIDE_CONFIG,
        ),
    ] {
        let path = source_root.join(relative);
        let bytes = read_regular_bounded(&path, context)?;
        if &sha256_bytes(&bytes) != expected {
            return Err(configuration_error(
                context,
                "exact source configuration SHA-256 differs from the genmodule binding",
            ));
        }
    }
    Ok(parsed)
}

fn validate_input_path(path: &Path) -> Result<(PathBuf, PathBuf)> {
    if !path.is_absolute() {
        return Err(configuration_error(
            INPUT_FILE,
            "input path must be absolute",
        ));
    }
    if path.file_name() != Some(OsStr::new(INPUT_FILE)) {
        return Err(configuration_error(
            INPUT_FILE,
            format!("input path must end in the exact filename {INPUT_FILE}"),
        ));
    }
    if !is_normalized_absolute(path) {
        return Err(configuration_error(
            INPUT_FILE,
            "input path must be normalized and contain no dot or parent components",
        ));
    }
    let canonical = path.canonicalize().map_err(|error| {
        configuration_error(
            INPUT_FILE,
            format!("cannot canonicalize input path: {error}"),
        )
    })?;
    if canonical.as_os_str() != path.as_os_str() {
        return Err(configuration_error(
            INPUT_FILE,
            "input path is not its canonical absolute spelling",
        ));
    }
    let root = path
        .parent()
        .ok_or_else(|| configuration_error(INPUT_FILE, "input path has no containing directory"))?;
    crate::validate_existing_directory_prefix_nofollow(root)
        .map_err(|error| configuration_error(INPUT_FILE, format!("unsafe input root: {error}")))?;
    let metadata = fs::metadata(root).map_err(|error| {
        configuration_error(INPUT_FILE, format!("cannot inspect input root: {error}"))
    })?;
    if !metadata.is_dir() {
        return Err(configuration_error(
            INPUT_FILE,
            "input root is not a directory",
        ));
    }
    Ok((root.to_path_buf(), canonical))
}

fn validate_gendir(root: &Path, raw_gendir: &str) -> Result<PathBuf> {
    let gendir = Path::new(raw_gendir);
    if !gendir.is_absolute() || !is_normalized_absolute(gendir) {
        return Err(configuration_error(
            RECEIPT_FILE,
            "configured_paths.GENDIR must be an absolute normalized path",
        ));
    }
    crate::validate_existing_directory_prefix_nofollow(gendir).map_err(|error| {
        configuration_error(
            RECEIPT_FILE,
            format!("configured_paths.GENDIR is unsafe: {error}"),
        )
    })?;
    let configure_root = root.join("configure");
    crate::validate_existing_directory_prefix_nofollow(&configure_root).map_err(|error| {
        configuration_error(
            RECEIPT_FILE,
            format!("unsafe input configure root: {error}"),
        )
    })?;
    let canonical_configure = configure_root.canonicalize().map_err(|error| {
        configuration_error(
            RECEIPT_FILE,
            format!("cannot resolve input configure root: {error}"),
        )
    })?;
    if canonical_configure.as_os_str() != configure_root.as_os_str() {
        return Err(configuration_error(
            RECEIPT_FILE,
            "input configure root is not canonical",
        ));
    }
    let configure_metadata = fs::metadata(&configure_root).map_err(|error| {
        configuration_error(
            RECEIPT_FILE,
            format!("cannot inspect input configure root: {error}"),
        )
    })?;
    if !configure_metadata.is_dir() {
        return Err(configuration_error(
            RECEIPT_FILE,
            "input configure root is not a directory",
        ));
    }
    let canonical_gendir = gendir.canonicalize().map_err(|error| {
        configuration_error(
            RECEIPT_FILE,
            format!("cannot resolve configured_paths.GENDIR: {error}"),
        )
    })?;
    if canonical_gendir.as_os_str() != gendir.as_os_str()
        || canonical_gendir == canonical_configure
        || !canonical_gendir.starts_with(&canonical_configure)
    {
        return Err(configuration_error(
            RECEIPT_FILE,
            "configured_paths.GENDIR must be canonical and strictly below the input configure root",
        ));
    }
    let metadata = fs::metadata(&canonical_gendir).map_err(|error| {
        configuration_error(
            RECEIPT_FILE,
            format!("cannot inspect configured GENDIR: {error}"),
        )
    })?;
    if !metadata.is_dir() {
        return Err(configuration_error(
            RECEIPT_FILE,
            "configured_paths.GENDIR is not a directory",
        ));
    }
    Ok(canonical_gendir)
}

fn is_normalized_absolute(path: &Path) -> bool {
    path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::RootDir | Component::Normal(_)))
}

fn read_regular_bounded(path: &Path, context: &str) -> Result<Vec<u8>> {
    measure_regular_file_bounded(path, MAX_CONTROL_FILE_BYTES)
        .map_err(|error| configuration_error(context, format!("cannot safely read file: {error}")))?
        .map(|(_identity, bytes)| bytes)
        .ok_or_else(|| configuration_error(context, "required file is missing"))
}

fn snapshot_input_root(root: &Path) -> Result<TreeContentCas> {
    let limits = TreeTraversalLimits::new(MAX_INPUT_ROOT_ENTRIES, MAX_INPUT_ROOT_FILE_BYTES)
        .map_err(|error| {
            configuration_error(INPUT_FILE, format!("invalid input root limits: {error}"))
        })?;
    measure_tree_content_cas_bounded(root, limits).map_err(|error| {
        configuration_error(
            INPUT_FILE,
            format!("cannot snapshot complete input root: {error}"),
        )
    })
}

fn configuration_error(file: &str, message: impl Into<String>) -> ArosError {
    ArosError::Configuration {
        file: file.into(),
        message: message.into(),
    }
}

#[cfg(test)]
#[path = "mesa_configuration_input_tests.rs"]
mod tests;
