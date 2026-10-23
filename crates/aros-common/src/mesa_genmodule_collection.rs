//! Bounded, data-only handoff for multiple selected Mesa genmodule producers.
//!
//! The reader checks the transported inventory, producer tuples, Make
//! projections, and source-configuration bytes for local consistency. It does
//! not authenticate a producer or establish that genmodule execution occurred.

use crate::mesa_genmodule_inventory::{
    MesaGenmoduleProducer, MesaGenmoduleSelectionBinding, ValidatedMesaGenmoduleSelection,
    MESA_GENMODULE_SELECTION_MAX_BYTES,
};
use crate::mesa_genmodule_make::MesaGenmoduleMakefile;
use crate::publication::{measure_regular_file_bounded, FileIdentity};
use crate::{sha256_bytes, ArosError, Result, Sha256Digest};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

/// Exact selected-inventory document filename inside the handoff root.
pub const MESA_GENMODULE_SELECTION_FILE: &str = "mesa-genmodule-selection.json";
/// Directory containing one raw Make projection per selected producer owner.
pub const MESA_GENMODULE_OUTPUTS_DIRECTORY: &str = "genmodule-outputs";
/// Exact filename of each per-owner raw Make projection.
pub const MESA_GENMODULE_OUTPUT_FILE: &str = "module.make";

const MAX_MAKE_BYTES: u64 = 128 * 1024;
const MAX_SOURCE_CONFIGURATION_BYTES: u64 = 1024 * 1024;
const MAX_COLLECTION_READ_BYTES: u64 = 64 * 1024 * 1024;

/// Original MetaMake execution inputs for one selected generated owner.
///
/// The selection and identities must already be caller-bound. MetaMake passes
/// TOP/SRCDIR/CURDIR/TARGET as command-line assignments and runs a generated
/// directory makefile below TOP, not from the top-level configure directory.
/// This constructs a consistency expectation, not execution authority.
///
/// # Errors
/// Rejects unsafe declaration directories, owners and scope inputs.
pub fn mesa_genmodule_scope_context(
    producer: &MesaGenmoduleProducer,
    identity: &crate::mesa_configuration::MesaConfigurationIdentity,
    configuration_stdout_sha256: &Sha256Digest,
    coordinates: &crate::mesa_configuration::MesaMakeCoordinates,
) -> Result<crate::make_scope_snapshot::MakeScopeContext> {
    coordinates.validate()?;
    let relative = Path::new(&producer.declaration_directory);
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(collection_error(
            "Make scope",
            "unsafe owner declaration directory",
        ));
    }
    let context = crate::make_scope_snapshot::MakeScopeContext {
        owner: producer.owner.clone(),
        source_inputs_sha256: identity.source_inputs_sha256.clone(),
        compiler_tree_sha256: identity.compiler_tree_sha256.clone(),
        configuration_stdout_sha256: configuration_stdout_sha256.clone(),
        working_directory: Path::new(&coordinates.build_root)
            .join(relative)
            .to_str()
            .ok_or_else(|| collection_error("Make scope", "non-UTF8 working directory"))?
            .to_owned(),
        inputs: BTreeMap::from([
            ("TOP".into(), coordinates.build_root.clone()),
            ("SRCDIR".into(), coordinates.source_root.clone()),
            ("CURDIR".into(), producer.declaration_directory.clone()),
            ("TARGET".into(), producer.owner.clone()),
        ]),
    };
    context.validate()?;
    Ok(context)
}

/// Deterministic stage name for one independently selected owner scope.
///
/// # Errors
/// Rejects unsafe owner names using the same output-path contract.
pub fn mesa_genmodule_scope_stage(owner: &str) -> std::result::Result<String, String> {
    mesa_genmodule_output_relative_path(owner)?;
    Ok(format!("genmodule-initial-{owner}"))
}

/// Data-only binding for a selected collection of Mesa genmodule outputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MesaGenmoduleCollectionBinding {
    pub selection: MesaGenmoduleSelectionBinding,
    pub inventory_sha256: Sha256Digest,
    pub helper_sha256: Sha256Digest,
    pub producer_sha256: Sha256Digest,
    /// Exactly one sorted entry per selected producer owner.
    pub outputs: Vec<MesaGenmoduleOutputBinding>,
}

/// Digests binding one producer's raw Make output and source configurations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MesaGenmoduleOutputBinding {
    pub producer: MesaGenmoduleProducer,
    pub makefile_sha256: Sha256Digest,
    pub base_configuration_sha256: Sha256Digest,
    #[serde(deserialize_with = "deserialize_present_option")]
    pub override_configuration_sha256: Option<Sha256Digest>,
}

/// Validated collection retained after checking every relevant local read.
///
/// This is consistency data, not producer execution or producer authentication.
#[derive(Debug, Clone)]
pub struct LoadedMesaGenmoduleCollection {
    binding: MesaGenmoduleCollectionBinding,
    selection: ValidatedMesaGenmoduleSelection,
    outputs: BTreeMap<String, MesaGenmoduleMakefile>,
}

impl LoadedMesaGenmoduleCollection {
    /// Caller-bound and row-validated selection inventory.
    #[must_use]
    pub const fn selection(&self) -> &ValidatedMesaGenmoduleSelection {
        &self.selection
    }

    /// Parsed Make output keyed by its graph owner, sorted by owner.
    #[must_use]
    pub const fn outputs(&self) -> &BTreeMap<String, MesaGenmoduleMakefile> {
        &self.outputs
    }

    /// Exact collection binding checked during loading.
    #[must_use]
    pub const fn binding(&self) -> &MesaGenmoduleCollectionBinding {
        &self.binding
    }
}

/// Return the deterministic safe relative path for one producer's raw Make file.
///
/// # Errors
/// Rejects owner identifiers that could escape or alias a path component.
pub fn mesa_genmodule_output_relative_path(owner: &str) -> std::result::Result<PathBuf, String> {
    if owner.is_empty()
        || owner.len() > 255
        || owner == "."
        || owner == ".."
        || !owner
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-.+".contains(&byte))
    {
        return Err(format!("unsafe Mesa genmodule owner `{owner}`"));
    }
    Ok(Path::new(MESA_GENMODULE_OUTPUTS_DIRECTORY)
        .join(owner)
        .join(MESA_GENMODULE_OUTPUT_FILE))
}

/// Load and validate a bounded multi-producer Mesa genmodule collection.
///
/// Inventory records are caller-bound to `binding.selection`; output entries
/// must exactly and uniquely cover those validated producers. Each raw Make
/// output and selected source configuration is read through bounded no-follow
/// regular-file access and checked again before return.
///
/// This establishes local byte consistency only. It does not authenticate the
/// helper or producer, or prove that genmodule ran.
///
/// # Errors
/// Rejects unsafe roots, inconsistent/oversized data, missing or extra owners,
/// changed digests, invalid Make projections, unsafe files, and read-time drift.
pub fn load_genmodule_collection(
    input_root: &Path,
    source_root: &Path,
    binding: &MesaGenmoduleCollectionBinding,
) -> Result<LoadedMesaGenmoduleCollection> {
    let input_root = validate_root(input_root, "Mesa genmodule input root")?;
    let source_root = validate_root(source_root, "Mesa genmodule source root")?;
    let mut reads = BTreeMap::<PathBuf, ObservedFile>::new();

    let inventory_bytes = read_observed(
        &input_root,
        Path::new(MESA_GENMODULE_SELECTION_FILE),
        MESA_GENMODULE_SELECTION_FILE,
        MESA_GENMODULE_SELECTION_MAX_BYTES as u64,
        &mut reads,
    )?;
    if sha256_bytes(&inventory_bytes) != binding.inventory_sha256 {
        return Err(collection_error(
            MESA_GENMODULE_SELECTION_FILE,
            "raw inventory SHA-256 differs from the collection binding",
        ));
    }
    let selection =
        ValidatedMesaGenmoduleSelection::parse_bound(&inventory_bytes, &binding.selection)
            .map_err(|error| collection_error(MESA_GENMODULE_SELECTION_FILE, error))?;

    let selected_producers = selection.producers();
    if binding.outputs.len() != selected_producers.len() {
        return Err(collection_error(
            MESA_GENMODULE_SELECTION_FILE,
            "collection output entries do not exactly cover the selected producer owners",
        ));
    }
    let mut previous_owner: Option<&str> = None;
    let mut parsed_outputs = BTreeMap::new();
    for (output, selected) in binding.outputs.iter().zip(selected_producers) {
        if previous_owner.is_some_and(|owner| owner >= output.producer.owner.as_str()) {
            return Err(collection_error(
                MESA_GENMODULE_SELECTION_FILE,
                "collection output entries are not uniquely sorted by producer owner",
            ));
        }
        previous_owner = Some(&output.producer.owner);
        if &output.producer != selected {
            return Err(collection_error(
                MESA_GENMODULE_SELECTION_FILE,
                format!(
                    "collection producer tuple for `{}` differs from the selected inventory",
                    output.producer.owner
                ),
            ));
        }

        let makefile_relative = mesa_genmodule_output_relative_path(&selected.owner)
            .map_err(|error| collection_error(MESA_GENMODULE_SELECTION_FILE, error))?;
        let makefile_bytes = read_observed(
            &input_root,
            &makefile_relative,
            &makefile_relative.display().to_string(),
            MAX_MAKE_BYTES,
            &mut reads,
        )?;
        if sha256_bytes(&makefile_bytes) != output.makefile_sha256 {
            return Err(collection_error(
                &makefile_relative.display().to_string(),
                "raw genmodule Make SHA-256 differs from the collection binding",
            ));
        }
        let makefile_text = std::str::from_utf8(&makefile_bytes).map_err(|error| {
            collection_error(
                &makefile_relative.display().to_string(),
                format!("raw genmodule Make output is not UTF-8: {error}"),
            )
        })?;
        let parsed =
            MesaGenmoduleMakefile::parse(&selected.module, makefile_text).map_err(|error| {
                collection_error(
                    &makefile_relative.display().to_string(),
                    format!("invalid literal genmodule Make projection: {error}"),
                )
            })?;

        let base_bytes = read_observed(
            &source_root,
            Path::new(&selected.base_configuration),
            &selected.base_configuration,
            MAX_SOURCE_CONFIGURATION_BYTES,
            &mut reads,
        )?;
        if sha256_bytes(&base_bytes) != output.base_configuration_sha256 {
            return Err(collection_error(
                &selected.base_configuration,
                "base configuration SHA-256 differs from the collection binding",
            ));
        }

        match (
            selected.override_configuration.as_deref(),
            output.override_configuration_sha256.as_ref(),
        ) {
            (Some(relative), Some(expected)) => {
                let override_bytes = read_observed(
                    &source_root,
                    Path::new(relative),
                    relative,
                    MAX_SOURCE_CONFIGURATION_BYTES,
                    &mut reads,
                )?;
                if &sha256_bytes(&override_bytes) != expected {
                    return Err(collection_error(
                        relative,
                        "override configuration SHA-256 differs from the collection binding",
                    ));
                }
            }
            (None, None) => {}
            _ => {
                return Err(collection_error(
                    MESA_GENMODULE_SELECTION_FILE,
                    format!(
                        "producer `{}` override configuration presence differs from its digest binding",
                        selected.owner
                    ),
                ));
            }
        }
        parsed_outputs.insert(selected.owner.clone(), parsed);
    }

    recheck_observed_files(&reads)?;
    Ok(LoadedMesaGenmoduleCollection {
        binding: binding.clone(),
        selection,
        outputs: parsed_outputs,
    })
}

#[derive(Debug)]
struct ObservedFile {
    identity: FileIdentity,
    digest: Sha256Digest,
    byte_len: u64,
    limit: u64,
    context: String,
}

fn read_observed(
    root: &Path,
    relative: &Path,
    context: &str,
    limit: u64,
    reads: &mut BTreeMap<PathBuf, ObservedFile>,
) -> Result<Vec<u8>> {
    if !is_normalized_relative(relative) {
        return Err(collection_error(
            context,
            "file path is not normalized and relative",
        ));
    }
    let path = root.join(relative);
    let (identity, bytes) = measure_regular_file_bounded(&path, limit)
        .map_err(|error| collection_error(context, format!("cannot safely read file: {error}")))?
        .ok_or_else(|| collection_error(context, "required file is missing"))?;
    let digest = sha256_bytes(&bytes);
    if let Some(previous) = reads.get(&path) {
        if previous.identity != identity || previous.digest != digest {
            return Err(collection_error(
                context,
                "file changed between repeated collection reads",
            ));
        }
    } else {
        let total_bytes = reads
            .values()
            .try_fold(bytes.len() as u64, |total, previous| {
                total.checked_add(previous.byte_len)
            });
        if total_bytes.is_none_or(|total| total > MAX_COLLECTION_READ_BYTES) {
            return Err(collection_error(
                context,
                format!(
                    "collection reads exceed the {MAX_COLLECTION_READ_BYTES}-byte aggregate limit"
                ),
            ));
        }
        reads.insert(
            path,
            ObservedFile {
                identity,
                digest,
                byte_len: bytes.len() as u64,
                limit,
                context: context.to_owned(),
            },
        );
    }
    Ok(bytes)
}

fn recheck_observed_files(reads: &BTreeMap<PathBuf, ObservedFile>) -> Result<()> {
    for (path, previous) in reads {
        let (identity, bytes) = measure_regular_file_bounded(path, previous.limit)
            .map_err(|error| {
                collection_error(
                    &previous.context,
                    format!("cannot recheck file safely: {error}"),
                )
            })?
            .ok_or_else(|| {
                collection_error(&previous.context, "file disappeared during loading")
            })?;
        if identity != previous.identity || sha256_bytes(&bytes) != previous.digest {
            return Err(collection_error(
                &previous.context,
                "file changed while the genmodule collection was being loaded",
            ));
        }
    }
    Ok(())
}

fn validate_root(path: &Path, context: &str) -> Result<PathBuf> {
    if !path.is_absolute() || !is_normalized_absolute(path) {
        return Err(collection_error(
            context,
            "root must be an absolute normalized path",
        ));
    }
    crate::validate_existing_directory_prefix_nofollow(path)
        .map_err(|error| collection_error(context, format!("unsafe root: {error}")))?;
    let canonical = path
        .canonicalize()
        .map_err(|error| collection_error(context, format!("cannot resolve root: {error}")))?;
    if canonical.as_os_str() != path.as_os_str() {
        return Err(collection_error(context, "root spelling is not canonical"));
    }
    let metadata = fs::metadata(path)
        .map_err(|error| collection_error(context, format!("cannot inspect root: {error}")))?;
    if !metadata.is_dir() {
        return Err(collection_error(context, "root is not a directory"));
    }
    Ok(canonical)
}

fn is_normalized_absolute(path: &Path) -> bool {
    path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::RootDir | Component::Normal(_)))
}

fn is_normalized_relative(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn collection_error(file: &str, message: impl Into<String>) -> ArosError {
    ArosError::Configuration {
        file: file.into(),
        message: message.into(),
    }
}

fn deserialize_present_option<'de, D, T>(
    deserializer: D,
) -> std::result::Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

#[cfg(test)]
#[path = "mesa_genmodule_collection_tests.rs"]
mod tests;
