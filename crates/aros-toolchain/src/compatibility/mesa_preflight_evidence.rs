//! Rootless consistency read-back for a bounded Mesa preflight evidence subset.
//!
//! The selected files can establish that configuration, receipt, input,
//! compiler assignments, the bounded genmodule Make projection, retained
//! stage logs, and selected native Make packets agree. They do not establish
//! that configure or any other child process actually ran.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use aros_common::mesa_configuration::{MesaConfigurationIdentity, ValidatedMesaConfiguration};
use aros_common::mesa_configuration_input::{
    MesaConfigurationInputDocument, MesaGenmoduleBinding, INPUT_FILE, MESA_GENMODULE_MAKE_FILE,
    MESA_GENMODULE_SELECTION_FILE,
};
use aros_common::mesa_genmodule_collection::{
    mesa_genmodule_output_relative_path, MesaGenmoduleCollectionBinding,
};
use aros_common::mesa_genmodule_inventory::{
    MesaGenmoduleSelectionBinding, ValidatedMesaGenmoduleSelection,
    MESA_GENMODULE_SELECTION_MAX_BYTES,
};
use aros_common::mesa_genmodule_make::MesaGenmoduleMakefile;
use aros_common::mesa_native_make::{MesaNativeMakeBinding, MesaNativePreparationStageIdentity};
use aros_common::{sha256_bytes, ArosCompilerIdentity, Sha256Digest};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ContractError;

const RECEIPT_MEMBER: &str = "mesa-preflight-receipt.json";
const CONFIGURATION_MEMBER: &str = "mesa-preflight-configuration.json";
const INPUT_MEMBER: &str = "mesa-preflight-input.json";
const COMPILER_CFG_MEMBER: &str = "mesa-preflight-compiler.cfg";
const GENMODULE_MAKE_MEMBER: &str = "mesa-preflight-genmodule.make";
const GENMODULE_SELECTION_MEMBER: &str = "mesa-preflight-genmodule-selection.json";
const GENMODULE_MODULE: &str = "mesa3dgl26-0";

// The data parser independently retains the legacy v1 32-KiB limit. V2 may
// additionally contain the bounded names/origins inventory.
const MAX_CONFIGURATION_BYTES: usize = 512 * 1024;
const MAX_CONTROL_BYTES: usize = 1024 * 1024;
const MAX_STAGE_BYTES: usize = 256 * 1024;
const MAX_COMPILER_CFG_BYTES: usize = 1024 * 1024;
const MAX_GENMODULE_MAKE_BYTES: usize = 128 * 1024;
const MAX_GENMODULE_SELECTION_BYTES: usize = MESA_GENMODULE_SELECTION_MAX_BYTES;
const MAX_NATIVE_CONTEXT_BYTES: usize = 64 * 1024;
const MAX_NATIVE_CONTROL_BYTES: usize = 64 * 1024;
const MAX_NATIVE_PACKET_BYTES: usize = 4 * 1024 * 1024;

const LEGACY_STAGES: &[&str] = &[
    "configure",
    "feature-inputs",
    "features",
    "configured-values",
];
const GENMODULE_STAGES: &[&str] = &["genmodule-build", "genmodule-make"];

fn stage_names(receipt: &Receipt) -> Result<Vec<String>, ContractError> {
    if receipt.genmodule.is_some() && receipt.genmodule_collection.is_some() {
        return Err(error(
            "Mesa preflight receipt cannot mix legacy and collection genmodule bindings",
        ));
    }
    if let Some(binding) = &receipt.native_make {
        if !(1..=aros_common::mesa_native_make::MAX_NATIVE_CALLS).contains(&binding.calls.len()) {
            return Err(error(
                "Mesa native Make call count is outside the supported range",
            ));
        }
        let collection = receipt
            .genmodule_collection
            .as_ref()
            .ok_or_else(|| error("Mesa native Make binding requires a genmodule collection"))?;
        if receipt.genmodule.is_some()
            || collection.outputs.len()
                > aros_common::mesa_genmodule_inventory::MESA_GENMODULE_INVENTORY_MAX_ROWS
        {
            return Err(error(
                "Mesa native Make collection has an invalid output set",
            ));
        }
        let producers: Vec<_> = collection
            .outputs
            .iter()
            .map(|output| output.producer.clone())
            .collect();
        return aros_common::mesa_native_make::native_preflight_stage_names(&producers)
            .map_err(|_| error("Mesa native preflight stage sequence is invalid"));
    }
    let mut names: Vec<String> = LEGACY_STAGES
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
    if receipt.genmodule.is_some() {
        names.extend(GENMODULE_STAGES.iter().map(|name| (*name).to_owned()));
    }
    if let Some(collection) = &receipt.genmodule_collection {
        if collection.outputs.len()
            > aros_common::mesa_genmodule_inventory::MESA_GENMODULE_INVENTORY_MAX_ROWS
        {
            return Err(error("Mesa genmodule collection exceeds its output limit"));
        }
        let mut previous_owner: Option<&str> = None;
        for output in &collection.outputs {
            let owner = output.producer.owner.as_str();
            if !safe_owner(owner) || previous_owner.is_some_and(|previous| previous >= owner) {
                return Err(error(
                    "Mesa genmodule collection owners are unsafe, duplicated, or unsorted",
                ));
            }
            previous_owner = Some(owner);
        }
        names.push("genmodule-inventory".to_owned());
        names.push("genmodule-build".to_owned());
        for output in &collection.outputs {
            if has_initial_scopes(receipt) {
                names.push(
                    aros_common::mesa_genmodule_collection::mesa_genmodule_scope_stage(
                        &output.producer.owner,
                    )
                    .map_err(|_| error("unsafe initial scope owner"))?,
                );
            }
            names.push(format!("genmodule-make-{}", output.producer.owner));
        }
    }
    Ok(names)
}

fn has_initial_scopes(receipt: &Receipt) -> bool {
    matches!(
        receipt.schema.as_str(),
        "aros-mesa-preflight-v7" | "aros-mesa-preflight-v8"
    )
}

/// Independently selected inputs for consistency-only Mesa preflight read-back.
///
/// These values must come from the selected source, profile and measured LLVM
/// driver selection, not from the evidence files. Paths are checked lexically;
/// this type performs no filesystem access.
#[derive(Serialize)]
pub(super) struct MesaPreflightEvidenceExpectations<'a> {
    pub(super) profile_name: &'a str,
    pub(super) target_triple: &'a str,
    pub(super) profile_document_sha256: &'a Sha256Digest,
    pub(super) compiler: &'a ArosCompilerIdentity,
    pub(super) compiler_tree_sha256: &'a Sha256Digest,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) compiler_root: Option<&'a Path>,
    pub(super) source_tree_sha256: &'a Sha256Digest,
    pub(super) cpu: &'a str,
    pub(super) platform: &'a str,
    pub(super) clang_path: &'a Path,
    pub(super) clangxx_path: &'a Path,
    pub(super) output_root: &'a Path,
    /// Independently selected Source configuration/inventory binding. This
    /// is distinct from `profile_name`, which names the compiler profile.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) genmodule_selection: Option<MesaGenmoduleSelectionBinding>,
    /// Digest measured by the caller, never adopted from a receipt's stages.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) genmodule_helper_sha256: Option<Sha256Digest>,
}

/// Local consistency result for the fixed Mesa preflight member set.
///
/// This is intentionally distinct from `CompletedMesaPreflight`, whose private
/// fields represent the owner-side producer result and full-tree snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MesaPreflightDataProof {
    expectations_sha256: Sha256Digest,
    receipt_sha256: Sha256Digest,
    configuration_sha256: Sha256Digest,
    input_sha256: Sha256Digest,
    configuration_identity: MesaConfigurationIdentity,
}

impl MesaPreflightDataProof {
    pub(super) fn matches_expectations(
        &self,
        expected: &MesaPreflightEvidenceExpectations<'_>,
    ) -> Result<bool, ContractError> {
        Ok(self.expectations_sha256 == expectations_digest(expected)?)
    }
    #[must_use]
    pub const fn receipt_sha256(&self) -> &Sha256Digest {
        &self.receipt_sha256
    }

    #[must_use]
    pub const fn configuration_sha256(&self) -> &Sha256Digest {
        &self.configuration_sha256
    }

    #[must_use]
    pub const fn input_sha256(&self) -> &Sha256Digest {
        &self.input_sha256
    }

    #[must_use]
    pub const fn configuration_identity(&self) -> &MesaConfigurationIdentity {
        &self.configuration_identity
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    schema: String,
    qualification: String,
    identity: MesaConfigurationIdentity,
    configuration_sha256: Sha256Digest,
    compiler_cfg_sha256: Sha256Digest,
    #[serde(
        default,
        deserialize_with = "present_compiler_programs",
        skip_serializing_if = "Option::is_none"
    )]
    compiler_programs: Option<aros_common::captured_programs::ProgramSetSelection>,
    make_values: BTreeMap<String, String>,
    configured_paths: ConfiguredPaths,
    stages: Vec<Stage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    genmodule: Option<MesaGenmoduleBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    genmodule_collection: Option<MesaGenmoduleCollectionBinding>,
    #[serde(
        default,
        deserialize_with = "present_native_binding",
        skip_serializing_if = "Option::is_none"
    )]
    native_make: Option<MesaNativeMakeBinding>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfiguredPaths {
    #[serde(rename = "AROS_BUILD_HOST")]
    aros_build_host: String,
    #[serde(rename = "AROS_DEVELOPER")]
    aros_developer: String,
    #[serde(rename = "AROS_TARGET_ARCH")]
    aros_target_arch: String,
    #[serde(rename = "AROS_TARGET_CPU")]
    aros_target_cpu: String,
    #[serde(rename = "DEBUG_CFLAGS")]
    debug_cflags: String,
    #[serde(rename = "GENDIR")]
    gendir: String,
    #[serde(rename = "OPTIMIZATION_CFLAGS")]
    optimization_cflags: String,
    #[serde(rename = "PORTSDIR")]
    portsdir: String,
    #[serde(rename = "TARGET_CC")]
    target_cc: String,
    #[serde(rename = "TARGET_CFLAGS")]
    target_cflags: String,
    #[serde(rename = "TARGET_CPP")]
    target_cpp: String,
    #[serde(rename = "TARGET_CXX")]
    target_cxx: String,
    #[serde(rename = "TARGET_CXXFLAGS")]
    target_cxxflags: String,
    #[serde(rename = "TARGET_SYSROOT")]
    target_sysroot: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stage {
    program: Sha256Digest,
    invocation: Sha256Digest,
    environment: Sha256Digest,
    stdout: Sha256Digest,
    stderr: Sha256Digest,
    #[serde(
        default,
        deserialize_with = "present_exit_status",
        skip_serializing_if = "Option::is_none"
    )]
    exit_status: Option<i32>,
}

/// Owner-side source paths and bounds for the fixed legacy, genmodule, or
/// complete-source native evidence member set.
///
/// `compiler.cfg` is selected from the receipt's GENDIR, after that path is
/// checked to remain lexically below the selected output root. The returned
/// paths are only for the local exporter; portable read-back never opens them.
pub(super) fn expected_members(
    output_root: &Path,
    receipt_bytes: &[u8],
) -> Result<BTreeMap<String, (PathBuf, usize)>, ContractError> {
    validate_absolute_lexical_path(output_root, "Mesa preflight output root")?;
    if receipt_bytes.len() > MAX_CONTROL_BYTES {
        return Err(error("Mesa preflight receipt exceeds its byte bound"));
    }
    let receipt: Receipt = parse_canonical(receipt_bytes, "Mesa preflight receipt")?;
    validate_receipt_header(&receipt)?;
    let stages = stage_names(&receipt)?;
    if receipt.stages.len() != stages.len() {
        return Err(error(
            "Mesa preflight receipt has an incomplete stage sequence",
        ));
    }
    let gendir = absolute_lexical_path(&receipt.configured_paths.gendir, "Mesa GENDIR")?;
    validate_gendir(output_root, &gendir)?;

    let mut members = BTreeMap::from([
        (
            RECEIPT_MEMBER.to_owned(),
            (output_root.join("receipt.json"), MAX_CONTROL_BYTES),
        ),
        (
            CONFIGURATION_MEMBER.to_owned(),
            (
                output_root.join("mesa-configuration.json"),
                MAX_CONFIGURATION_BYTES,
            ),
        ),
        (
            INPUT_MEMBER.to_owned(),
            (output_root.join(INPUT_FILE), MAX_CONTROL_BYTES),
        ),
        (
            COMPILER_CFG_MEMBER.to_owned(),
            (
                gendir.join("config").join("compiler.cfg"),
                MAX_COMPILER_CFG_BYTES,
            ),
        ),
    ]);
    for stage in &stages {
        for stream in ["stdout", "stderr"] {
            let name = format!("mesa-preflight-{stage}.{stream}");
            members.insert(
                name,
                (
                    output_root.join(format!("{stage}.{stream}")),
                    MAX_STAGE_BYTES,
                ),
            );
        }
    }
    if receipt.genmodule.is_some() {
        members.insert(
            GENMODULE_MAKE_MEMBER.to_owned(),
            (
                output_root.join(MESA_GENMODULE_MAKE_FILE),
                MAX_GENMODULE_MAKE_BYTES,
            ),
        );
    }
    if let Some(collection) = &receipt.genmodule_collection {
        members.insert(
            GENMODULE_SELECTION_MEMBER.to_owned(),
            (
                output_root.join(MESA_GENMODULE_SELECTION_FILE),
                MAX_GENMODULE_SELECTION_BYTES,
            ),
        );
        for output in &collection.outputs {
            let owner = &output.producer.owner;
            members.insert(
                genmodule_output_member(owner)?,
                (
                    output_root.join(
                        mesa_genmodule_output_relative_path(owner)
                            .map_err(|_| error("unsafe Mesa genmodule output owner"))?,
                    ),
                    MAX_GENMODULE_MAKE_BYTES,
                ),
            );
        }
    }
    if let Some(binding) = &receipt.native_make {
        members.insert(
            format!(
                "mesa-preflight-{}",
                aros_common::mesa_native_make::NATIVE_CONTEXT_FILE
            ),
            (
                output_root.join(aros_common::mesa_native_make::NATIVE_CONTEXT_FILE),
                MAX_NATIVE_CONTEXT_BYTES,
            ),
        );
        members.insert(
            format!(
                "mesa-preflight-{}",
                aros_common::mesa_native_make::NATIVE_CONTROL_FILE
            ),
            (
                output_root.join(aros_common::mesa_native_make::NATIVE_CONTROL_FILE),
                MAX_NATIVE_CONTROL_BYTES,
            ),
        );
        for slot in 0..binding.calls.len() {
            let file = aros_common::mesa_native_make::native_packet_file(slot)
                .map_err(|_| error("Mesa native Make call slot is invalid"))?;
            members.insert(
                format!("mesa-preflight-{file}"),
                (output_root.join(file), MAX_NATIVE_PACKET_BYTES),
            );
        }
    }
    Ok(members)
}

/// Validate the fixed member set without filesystem access or process claims.
pub(super) fn readback(
    files: &BTreeMap<String, Vec<u8>>,
    expected: &MesaPreflightEvidenceExpectations<'_>,
) -> Result<MesaPreflightDataProof, ContractError> {
    validate_expectations(expected)?;

    let receipt_bytes = files
        .get(RECEIPT_MEMBER)
        .ok_or_else(|| error("Mesa preflight portable receipt is absent"))?;
    let member_paths = expected_members(expected.output_root, receipt_bytes)?;
    if files.len() != member_paths.len() || files.keys().ne(member_paths.keys()) {
        return Err(error(
            "Mesa preflight portable evidence differs from its exact member set",
        ));
    }
    for (name, bytes) in files {
        let (_, limit) = member_paths
            .get(name)
            .ok_or_else(|| error("Mesa preflight portable evidence has an unexpected member"))?;
        if bytes.len() > *limit {
            return Err(error(
                "Mesa preflight portable member exceeds its byte bound",
            ));
        }
    }

    let receipt: Receipt = parse_canonical(receipt_bytes, "Mesa preflight receipt")?;
    validate_receipt_header(&receipt)?;
    let stages = stage_names(&receipt)?;
    if receipt.stages.len() != stages.len() {
        return Err(error(
            "Mesa preflight receipt has an incomplete stage sequence",
        ));
    }
    if let Some(binding) = &receipt.native_make {
        validate_native_stages(&receipt, binding)?;
    }
    for (index, stage_name) in stages.iter().enumerate() {
        let stage = &receipt.stages[index];
        let stdout_name = format!("mesa-preflight-{stage_name}.stdout");
        let stderr_name = format!("mesa-preflight-{stage_name}.stderr");
        let stdout = member(files, &stdout_name)?;
        let stderr = member(files, &stderr_name)?;
        if sha256_bytes(stdout) != stage.stdout || sha256_bytes(stderr) != stage.stderr {
            return Err(error(
                "Mesa preflight portable stage logs differ from their receipt digests",
            ));
        }
    }

    let profile_digest = expected.profile_document_sha256;
    let invocation_bytes = crate::canonical::bytes(&serde_json::json!({
        "stages": &receipt.stages,
        "profile": profile_digest,
    }))
    .map_err(|_| error("cannot encode Mesa preflight invocation identity"))?;
    let invocation_sha256 = sha256_bytes(&invocation_bytes);
    let identity = MesaConfigurationIdentity {
        target_profile: expected.profile_name.to_owned(),
        target_triple: expected.target_triple.to_owned(),
        compiler: expected.compiler.clone(),
        compiler_tree_sha256: expected.compiler_tree_sha256.clone(),
        source_inputs_sha256: expected.source_tree_sha256.clone(),
        configure_invocation_sha256: invocation_sha256,
    };
    if receipt.identity != identity {
        return Err(error(
            "Mesa preflight receipt identity differs from independently selected inputs",
        ));
    }

    let configuration_bytes = member(files, CONFIGURATION_MEMBER)?;
    let configuration_sha256 = sha256_bytes(configuration_bytes);
    if receipt.configuration_sha256 != configuration_sha256 {
        return Err(error(
            "Mesa preflight configuration differs from its receipt digest",
        ));
    }
    let configuration_value: Value = serde_json::from_slice(configuration_bytes)
        .map_err(|_| error("Mesa preflight configuration is not closed JSON"))?;
    require_canonical(
        &configuration_value,
        configuration_bytes,
        "Mesa configuration",
    )?;
    let configuration = ValidatedMesaConfiguration::parse_bound(configuration_bytes, &identity)
        .map_err(|_| error("Mesa preflight configuration differs from its selected identity"))?;
    if receipt.native_make.is_some() && configuration.initial_values().is_none() {
        return Err(error(
            "Mesa native Make binding requires measured initial values",
        ));
    }
    if let Some(namespace) = configuration.initial_namespace() {
        let observed = aros_common::make_namespace::ValidatedMakeNamespace::from_make_stdout(
            member(files, "mesa-preflight-configured-values.stdout")?,
        )
        .map_err(|_| error("Mesa namespace observation is malformed or missing"))?;
        if &observed != namespace {
            return Err(error(
                "Mesa namespace differs from the retained configured-values stage",
            ));
        }
        if let Some(values) = configuration.initial_values() {
            let retained =
                aros_common::make_initial_values::ValidatedMakeInitialValues::from_make_stdout(
                    member(files, "mesa-preflight-configured-values.stdout")?,
                    namespace,
                )
                .map_err(|_| error("Mesa initial values observation is malformed or missing"))?;
            if &retained != values {
                return Err(error(
                    "Mesa initial values differ from the retained configured-values stage",
                ));
            }
        }
    }
    let recipe = configuration.recipe_configuration();
    let coordinates = &recipe.make_coordinates;
    if Path::new(&coordinates.build_root) != expected.output_root.join("configure")
        || coordinates.ports_root != receipt.configured_paths.portsdir
        || coordinates.generated_root != receipt.configured_paths.gendir
    {
        return Err(error(
            "Mesa Make coordinates differ from the selected configure owner paths",
        ));
    }
    if recipe.target_cc != selected_path_text(expected.clang_path, "selected clang path")?
        || recipe.target_cxx != selected_path_text(expected.clangxx_path, "selected clang++ path")?
        || (recipe.target_sysroot.join(" ") != receipt.configured_paths.target_sysroot
            && recipe.target_sysroot
                != [
                    "--sysroot".to_owned(),
                    receipt.configured_paths.aros_developer.clone(),
                ]
            && recipe.target_sysroot
                != [format!(
                    "--sysroot={}",
                    receipt.configured_paths.aros_developer
                )])
    {
        return Err(error(
            "Mesa recipe drivers/sysroot differ from independently selected configured roles",
        ));
    }
    let make_values = configuration.make_variables();
    if let Some(selected) = &receipt.compiler_programs {
        if expected.compiler_root
            != Some(
                selected
                    .roots
                    .first()
                    .ok_or_else(|| error("compiler program root is absent"))?
                    .root
                    .as_path(),
            )
        {
            return Err(error(
                "compiler program root differs from independently selected root",
            ));
        }
        aros_common::mesa_configuration_input::validate_compiler_program_selection(
            selected,
            &configuration,
        )
        .map_err(|_| error("Mesa receipt compiler program selection differs from configuration"))?;
    }
    if receipt.make_values != make_values {
        return Err(error(
            "Mesa preflight receipt make_values differ from the validated configuration",
        ));
    }

    let input_bytes = member(files, INPUT_MEMBER)?;
    let input: MesaConfigurationInputDocument =
        parse_canonical(input_bytes, "Mesa configuration input")?;
    let input_sha256 = sha256_bytes(input_bytes);
    let valid_version = matches!(
        (
            input.version,
            input.local_source.is_some(),
            input.genmodule.is_some(),
            input.genmodule_collection.is_some()
        ),
        // Local snapshots deliberately exclude one generated subtree. They
        // remain CLI data, never a complete-tree portable native proof.
        (1, false, false, false) | (3, false, true, false) | (5 | 7 | 9, false, false, true)
    );
    let expected_schema = match input.version {
        1 => "aros-mesa-preflight-v1",
        2 => "aros-mesa-preflight-v2",
        3 => "aros-mesa-preflight-v3",
        4 => "aros-mesa-preflight-v4",
        5 => "aros-mesa-preflight-v5",
        6 => "aros-mesa-preflight-v6",
        7 => "aros-mesa-preflight-v7",
        8 => "aros-mesa-preflight-v8",
        9 => "aros-mesa-preflight-v9",
        _ => "",
    };
    let local_source_consistent = input.local_source.as_ref().is_none_or(|source| {
        source.schema == "aros-local-source-v1"
            && source.content_sha256 == identity.source_inputs_sha256
    });
    if !valid_version
        || !local_source_consistent
        || receipt.schema != expected_schema
        || input.genmodule != receipt.genmodule
        || input.genmodule_collection != receipt.genmodule_collection
        || input.identity != identity
        || input.configuration_sha256 != configuration_sha256
        || input.receipt_sha256 != sha256_bytes(receipt_bytes)
    {
        return Err(error(
            "Mesa preflight input does not bind its selected configuration and receipt",
        ));
    }
    if let Some(binding) = &receipt.genmodule {
        validate_genmodule_binding(binding, &receipt.stages, files)?;
    }
    match (
        &receipt.genmodule_collection,
        &expected.genmodule_selection,
        &expected.genmodule_helper_sha256,
    ) {
        (Some(collection), Some(selection), Some(helper))
            if collection.selection == *selection && collection.helper_sha256 == *helper =>
        {
            validate_genmodule_collection(collection, &receipt, files, selection, &configuration)?;
        }
        (Some(_), _, _) => {
            return Err(error(
                "Mesa genmodule collection lacks its independently selected inventory binding",
            ));
        }
        (None, Some(_), _) | (None, _, Some(_)) => {
            return Err(error(
                "selected Mesa genmodule inventory is absent from portable evidence",
            ));
        }
        (None, None, None) => {}
    }

    if let Some(binding) = &receipt.native_make {
        let selection_binding = expected
            .genmodule_selection
            .as_ref()
            .ok_or_else(|| error("Mesa native Make binding lacks a selected inventory"))?;
        let selection_bytes = member(files, GENMODULE_SELECTION_MEMBER)?;
        let selection =
            ValidatedMesaGenmoduleSelection::parse_bound(selection_bytes, selection_binding)
                .map_err(|_| error("Mesa native Make inventory is not caller-bound"))?;
        aros_common::mesa_native_make::load_native_make(
            binding,
            &identity,
            &receipt.stages[3].stdout,
            coordinates,
            selection.producers(),
            |name, _limit| {
                files
                    .get(&format!("mesa-preflight-{name}"))
                    .cloned()
                    .ok_or_else(|| aros_common::ArosError::Configuration {
                        file: name.to_owned(),
                        message: "selected portable native Make member is absent".to_owned(),
                    })
            },
        )
        .map_err(|_| error("Mesa native Make packet data is inconsistent"))?;
    }

    validate_configured_paths(&receipt.configured_paths, expected)?;
    let compiler_cfg = member(files, COMPILER_CFG_MEMBER)?;
    if sha256_bytes(compiler_cfg) != receipt.compiler_cfg_sha256 {
        return Err(error(
            "Mesa compiler.cfg bytes differ from the preflight receipt digest",
        ));
    }
    let compiler_cfg =
        std::str::from_utf8(compiler_cfg).map_err(|_| error("Mesa compiler.cfg is not UTF-8"))?;
    super::mesa_preflight::validate_compiler_assignments(compiler_cfg, &make_values)?;

    Ok(MesaPreflightDataProof {
        expectations_sha256: expectations_digest(expected)?,
        receipt_sha256: sha256_bytes(receipt_bytes),
        configuration_sha256,
        input_sha256,
        configuration_identity: identity,
    })
}

fn expectations_digest(
    expected: &MesaPreflightEvidenceExpectations<'_>,
) -> Result<Sha256Digest, ContractError> {
    let value = serde_json::to_value(expected)
        .map_err(|_| error("cannot encode Mesa evidence selection"))?;
    crate::canonical::bytes(&value)
        .map(|bytes| sha256_bytes(&bytes))
        .map_err(|_| error("cannot encode Mesa evidence selection"))
}

fn validate_expectations(
    expected: &MesaPreflightEvidenceExpectations<'_>,
) -> Result<(), ContractError> {
    validate_identifier(expected.profile_name, "Mesa profile name")?;
    validate_identifier(expected.target_triple, "Mesa target triple")?;
    validate_identifier(expected.cpu, "Mesa target CPU")?;
    validate_identifier(expected.platform, "Mesa target platform")?;
    validate_absolute_lexical_path(expected.output_root, "Mesa preflight output root")?;
    if let Some(root) = expected.compiler_root {
        validate_absolute_lexical_path(root, "selected compiler root")?;
    }
    validate_absolute_lexical_path(expected.clang_path, "selected clang path")?;
    validate_absolute_lexical_path(expected.clangxx_path, "selected clang++ path")?;
    expected
        .compiler
        .validate_for_target(expected.target_triple)
        .map_err(|_| error("selected Mesa compiler identity is invalid"))?;
    if !matches!(expected.compiler, ArosCompilerIdentity::Llvm { .. }) {
        return Err(error(
            "Mesa preflight consistency requires an LLVM compiler",
        ));
    }
    Ok(())
}

fn validate_receipt_header(receipt: &Receipt) -> Result<(), ContractError> {
    let (schema_has_genmodule, schema_has_collection, schema_has_native_make, schema_is_supported) =
        match receipt.schema.as_str() {
            "aros-mesa-preflight-v1" | "aros-mesa-preflight-v2" => (false, false, false, true),
            "aros-mesa-preflight-v3" | "aros-mesa-preflight-v4" => (true, false, false, true),
            "aros-mesa-preflight-v5"
            | "aros-mesa-preflight-v6"
            | "aros-mesa-preflight-v7"
            | "aros-mesa-preflight-v8" => (false, true, false, true),
            "aros-mesa-preflight-v9" => (false, true, true, true),
            _ => (false, false, false, false),
        };
    if !schema_is_supported
        || schema_has_genmodule != receipt.genmodule.is_some()
        || schema_has_collection != receipt.genmodule_collection.is_some()
        || schema_has_native_make != receipt.native_make.is_some()
        || (!schema_has_native_make
            && receipt
                .stages
                .iter()
                .any(|stage| stage.exit_status.is_some()))
        || receipt.qualification != "local-configure-only"
    {
        return Err(error(
            "Mesa preflight receipt schema or qualification is invalid",
        ));
    }
    Ok(())
}

fn present_native_binding<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<MesaNativeMakeBinding>, D::Error> {
    MesaNativeMakeBinding::deserialize(deserializer).map(Some)
}

fn present_compiler_programs<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<aros_common::captured_programs::ProgramSetSelection>, D::Error> {
    aros_common::captured_programs::ProgramSetSelection::deserialize(deserializer).map(Some)
}

fn present_exit_status<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<i32>, D::Error> {
    i32::deserialize(deserializer).map(Some)
}

fn validate_native_stages(
    receipt: &Receipt,
    binding: &MesaNativeMakeBinding,
) -> Result<(), ContractError> {
    if receipt.schema != "aros-mesa-preflight-v9"
        || receipt.stages.len() < 7
        || receipt
            .stages
            .iter()
            .enumerate()
            .any(|(index, stage)| !(4..7).contains(&index) && stage.exit_status.is_some())
    {
        return Err(error("Mesa native receipt has an invalid status layout"));
    }
    let preparation = receipt.stages[4..7]
        .iter()
        .map(|stage| {
            Ok(MesaNativePreparationStageIdentity {
                program: stage.program.clone(),
                invocation: stage.invocation.clone(),
                environment: stage.environment.clone(),
                exit_status: stage
                    .exit_status
                    .ok_or_else(|| error("Mesa native preparation stage lacks exit status"))?,
            })
        })
        .collect::<Result<Vec<_>, ContractError>>()?;
    aros_common::mesa_native_make::validate_native_preparation_stages(
        binding,
        &receipt.stages[3].program,
        &receipt.stages[3].environment,
        &preparation,
    )
    .map_err(|_| error("Mesa native preparation stage identity is invalid"))
}

fn validate_genmodule_binding(
    binding: &MesaGenmoduleBinding,
    stages: &[Stage],
    files: &BTreeMap<String, Vec<u8>>,
) -> Result<(), ContractError> {
    if binding.module != GENMODULE_MODULE
        || stages.len() != LEGACY_STAGES.len() + GENMODULE_STAGES.len()
        || binding.producer_sha256 != stages[LEGACY_STAGES.len() + 1].program
    {
        return Err(error(
            "Mesa genmodule metadata differs from its selected module or producer stage",
        ));
    }
    let makefile = member(files, GENMODULE_MAKE_MEMBER)?;
    if sha256_bytes(makefile) != binding.makefile_sha256 {
        return Err(error(
            "Mesa genmodule Make bytes differ from their receipt digest",
        ));
    }
    let makefile = std::str::from_utf8(makefile)
        .map_err(|_| error("Mesa genmodule Make bytes are not UTF-8"))?;
    MesaGenmoduleMakefile::parse(GENMODULE_MODULE, makefile)
        .map_err(|_| error("Mesa genmodule Make bytes are not a bounded literal fragment"))?;
    // Base and override configuration digests are carried consistently in
    // the input document and receipt. Rootless read-back has no selected
    // source files to authenticate them against.
    Ok(())
}

fn validate_genmodule_collection(
    binding: &MesaGenmoduleCollectionBinding,
    receipt: &Receipt,
    files: &BTreeMap<String, Vec<u8>>,
    expected_selection: &MesaGenmoduleSelectionBinding,
    configuration: &ValidatedMesaConfiguration,
) -> Result<(), ContractError> {
    let stages = &receipt.stages;
    let scoped = has_initial_scopes(receipt);
    let native = receipt.native_make.is_some();
    if scoped && configuration.initial_values().is_none() {
        return Err(error("owner scopes require version-3 configuration values"));
    }
    if native && configuration.initial_values().is_none() {
        return Err(error(
            "Mesa native Make binding requires measured initial values",
        ));
    }
    if &binding.selection != expected_selection {
        return Err(error(
            "Mesa genmodule collection differs from its independently selected inventory binding",
        ));
    }
    if binding.outputs.len()
        > aros_common::mesa_genmodule_inventory::MESA_GENMODULE_INVENTORY_MAX_ROWS
        || stages.len()
            != if native {
                9 + binding.outputs.len()
            } else {
                LEGACY_STAGES.len() + 2 + binding.outputs.len() * if scoped { 2 } else { 1 }
            }
    {
        return Err(error(
            "Mesa genmodule collection has an incomplete output/stage sequence",
        ));
    }

    let selection_bytes = member(files, GENMODULE_SELECTION_MEMBER)?;
    if sha256_bytes(selection_bytes) != binding.inventory_sha256 {
        return Err(error(
            "Mesa genmodule selection bytes differ from their collection digest",
        ));
    }
    let selection =
        ValidatedMesaGenmoduleSelection::parse_bound(selection_bytes, expected_selection)
            .map_err(|_| error("Mesa genmodule selection is not a caller-bound inventory"))?;
    if selection.producers().len() != binding.outputs.len() {
        return Err(error(
            "Mesa genmodule output entries do not exactly cover selected producer owners",
        ));
    }

    let inventory_stage = &stages[if native { 7 } else { LEGACY_STAGES.len() }];
    if inventory_stage.program != binding.helper_sha256 {
        return Err(error(
            "Mesa genmodule inventory stage differs from its selected helper digest",
        ));
    }
    if native {
        let configured = &stages[3];
        let build = &stages[8];
        if build.program != configured.program || build.environment != configured.environment {
            return Err(error(
                "Mesa native generator build differs from configured program/environment",
            ));
        }
    }

    let mut previous_owner: Option<&str> = None;
    for (index, output) in binding.outputs.iter().enumerate() {
        let owner = output.producer.owner.as_str();
        if !safe_owner(owner)
            || previous_owner.is_some_and(|previous| previous >= owner)
            || selection.producers().get(index) != Some(&output.producer)
        {
            return Err(error(
                "Mesa genmodule output producer tuples are not the exact sorted inventory",
            ));
        }
        previous_owner = Some(owner);

        let offset = if native {
            9 + index
        } else {
            LEGACY_STAGES.len() + 2 + index * if scoped { 2 } else { 1 }
        };
        if scoped {
            let stage = &stages[offset];
            let configured = &stages[3];
            if stage.program != configured.program || stage.environment != configured.environment {
                return Err(error(
                    "owner scope Make program/environment differs from configuration",
                ));
            }
            let context = aros_common::mesa_genmodule_collection::mesa_genmodule_scope_context(
                &output.producer,
                configuration.identity(),
                &configured.stdout,
                &configuration.recipe_configuration().make_coordinates,
            )
            .map_err(|_| error("invalid selected owner scope context"))?;
            let name = aros_common::mesa_genmodule_collection::mesa_genmodule_scope_stage(owner)
                .map_err(|_| error("unsafe owner scope stage"))?;
            aros_common::make_scope_snapshot::ValidatedMakeScopeSnapshot::from_make_stdout(
                member(files, &format!("mesa-preflight-{name}.stdout"))?,
                &context,
            )
            .map_err(|_| error("owner scope differs from its selected context"))?;
        }
        let stage = &stages[offset + usize::from(scoped)];
        if stage.program != binding.producer_sha256 {
            return Err(error(
                "Mesa genmodule owner stage differs from the selected producer digest",
            ));
        }

        let makefile = member(files, &genmodule_output_member(owner)?)?;
        if sha256_bytes(makefile) != output.makefile_sha256 {
            return Err(error(
                "Mesa genmodule output bytes differ from their collection digest",
            ));
        }
        let makefile = std::str::from_utf8(makefile)
            .map_err(|_| error("Mesa genmodule output bytes are not UTF-8"))?;
        MesaGenmoduleMakefile::parse(&output.producer.module, makefile)
            .map_err(|_| error("Mesa genmodule output is not a bounded literal fragment"))?;

        if output.override_configuration_sha256.is_some()
            != output.producer.override_configuration.is_some()
        {
            return Err(error(
                "Mesa genmodule override configuration presence differs from its digest binding",
            ));
        }
    }
    Ok(())
}

fn genmodule_output_member(owner: &str) -> Result<String, ContractError> {
    if !safe_owner(owner) {
        return Err(error(
            "Mesa genmodule owner is not a safe member identifier",
        ));
    }
    Ok(format!("mesa-preflight-genmodule-{owner}.make"))
}

fn safe_owner(owner: &str) -> bool {
    !owner.is_empty()
        && owner != "."
        && owner != ".."
        && owner
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-.+".contains(&byte))
}

fn validate_configured_paths(
    paths: &ConfiguredPaths,
    expected: &MesaPreflightEvidenceExpectations<'_>,
) -> Result<(), ContractError> {
    let gendir = absolute_lexical_path(&paths.gendir, "Mesa GENDIR")?;
    validate_gendir(expected.output_root, &gendir)?;
    if paths.aros_target_cpu != expected.cpu || paths.aros_target_arch != expected.platform {
        return Err(error(
            "configured Mesa CPU or platform differs from selected profile",
        ));
    }
    let clang = selected_path_text(expected.clang_path, "selected clang path")?;
    let clangxx = selected_path_text(expected.clangxx_path, "selected clang++ path")?;
    if paths.target_cc != clang
        || paths.target_cpp != format!("{clang} -E")
        || paths.target_cxx != clangxx
    {
        return Err(error(
            "configured Mesa driver paths differ from selected LLVM roles",
        ));
    }
    if paths.aros_build_host.is_empty()
        || paths.aros_build_host.chars().any(char::is_control)
        || paths.target_cflags.chars().any(char::is_control)
        || paths.target_cxxflags.chars().any(char::is_control)
        || paths.debug_cflags.chars().any(char::is_control)
        || paths.optimization_cflags.chars().any(char::is_control)
    {
        return Err(error("configured Mesa feature paths contain invalid text"));
    }
    let developer = absolute_lexical_path(&paths.aros_developer, "Mesa developer path")?;
    let ports = absolute_lexical_path(&paths.portsdir, "Mesa ports path")?;
    let configure_root = expected.output_root.join("configure");
    if developer == configure_root
        || ports == configure_root
        || !developer.starts_with(&configure_root)
        || !ports.starts_with(&configure_root)
    {
        return Err(error(
            "configured Mesa developer or ports path escaped its selected configure root",
        ));
    }
    if !paths.target_sysroot.is_empty() {
        let sysroot = paths
            .target_sysroot
            .strip_prefix("--sysroot=")
            .filter(|value| !value.is_empty())
            .ok_or_else(|| error("configured Mesa sysroot syntax is unsupported"))?;
        absolute_lexical_path(sysroot, "Mesa sysroot")?;
    }
    Ok(())
}

fn validate_gendir(output_root: &Path, gendir: &Path) -> Result<(), ContractError> {
    validate_absolute_lexical_path(output_root, "Mesa preflight output root")?;
    validate_absolute_lexical_path(gendir, "Mesa GENDIR")?;
    let configure_root = output_root.join("configure");
    let relative = gendir
        .strip_prefix(&configure_root)
        .map_err(|_| error("Mesa GENDIR is outside its selected configure root"))?;
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(error("Mesa GENDIR has an unsafe generated path"));
    }
    Ok(())
}

fn parse_canonical<T: for<'de> Deserialize<'de> + Serialize>(
    bytes: &[u8],
    label: &str,
) -> Result<T, ContractError> {
    let parsed: T =
        serde_json::from_slice(bytes).map_err(|_| error(&format!("{label} is not closed JSON")))?;
    let value =
        serde_json::to_value(&parsed).map_err(|_| error(&format!("cannot re-encode {label}")))?;
    require_canonical(&value, bytes, label)?;
    Ok(parsed)
}

fn require_canonical(value: &Value, original: &[u8], label: &str) -> Result<(), ContractError> {
    let encoded = crate::canonical::bytes(value)
        .map_err(|_| error(&format!("{label} exceeds canonical JSON bounds")))?;
    if encoded != original {
        return Err(error(&format!("{label} is not canonical JSON")));
    }
    Ok(())
}

fn member<'a>(files: &'a BTreeMap<String, Vec<u8>>, name: &str) -> Result<&'a [u8], ContractError> {
    files
        .get(name)
        .map(Vec::as_slice)
        .ok_or_else(|| error("Mesa preflight portable evidence is incomplete"))
}

fn selected_path_text(path: &Path, label: &str) -> Result<String, ContractError> {
    validate_absolute_lexical_path(path, label)?;
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| error(&format!("{label} is not UTF-8")))
}

fn validate_identifier(value: &str, label: &str) -> Result<(), ContractError> {
    if value.is_empty()
        || value.len() > 256
        || value.chars().any(char::is_control)
        || value.chars().any(char::is_whitespace)
        || value.bytes().any(|byte| b"$#\\*?[]".contains(&byte))
    {
        return Err(error(&format!("{label} is invalid")));
    }
    Ok(())
}

fn validate_absolute_lexical_path(path: &Path, label: &str) -> Result<(), ContractError> {
    let text = path.to_str().unwrap_or_default();
    if !path.is_absolute()
        || path.as_os_str().is_empty()
        || text.is_empty()
        || text.contains("//")
        || text.chars().any(char::is_control)
        || text.chars().any(char::is_whitespace)
        || text.bytes().any(|byte| b"$#\\*?[]".contains(&byte))
        || path.file_name().is_none()
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
        || text
            .split('/')
            .skip(1)
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
    {
        return Err(error(&format!("{label} is not an absolute lexical path")));
    }
    Ok(())
}

fn absolute_lexical_path(value: &str, label: &str) -> Result<PathBuf, ContractError> {
    let path = PathBuf::from(value);
    validate_absolute_lexical_path(&path, label)?;
    Ok(path)
}

fn error(message: &str) -> ContractError {
    ContractError::compatibility(message)
}

#[cfg(test)]
#[path = "mesa_preflight_evidence_tests.rs"]
mod tests;
