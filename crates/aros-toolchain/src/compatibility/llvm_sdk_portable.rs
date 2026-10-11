//! Rootless, bounded read-back for ordinary LLVM SDK application links.
//!
//! This checks the exact selected source/compiler inputs, command identities,
//! portable inventories, retained logs/maps and generic AROS ELF structure.
//! It cannot authenticate a remote job, prove that SDK payload bytes match an
//! inventory, establish SDK generation, or qualify a release.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use aros_common::sdk_manifest::SdkIdentity;
use aros_common::{
    measure_regular_file_digest_bounded, measure_tree_content_cas_bounded, sha256_bytes,
    toolchain_inventory_sha256, toolchain_tree_inventory_excluding, ArosCompilerIdentity,
    ArosToolchainManifestEntry, CancellationToken, Sha256Digest, TreeTraversalLimits,
};
use serde::{Deserialize, Serialize};

use super::native_sdk_portable::{
    resolve_inventory_path, validate_inventory, validate_inventory_with_limit, BoundedInventory,
};
use super::{
    command_identity, environment, llvm_sdk_application, llvm_sdk_generated_root,
    llvm_sdk_link_paths, llvm_sdk_links, native_sdk_relocation, CompatibilityHelperReport,
    CompatibilityPhase, CompatibilityProbeReport, LlvmSdkDriverBinding,
    LlvmSdkGeneratedRootRequest,
};
use crate::{canonical, ContractError};

const SELECTION_SCHEMA: &str = "aros-llvm-sdk-link-selection-v1";
const BINDING_FILE: &str = "aros-native-consumer-binding.json";
const MAX_DOCUMENT_BYTES: usize = canonical::MAX_DOCUMENT_BYTES;
const MAX_INVENTORY_BYTES: usize = 16 * 1024 * 1024;
const MAX_ELF_BYTES: usize = 128 * 1024 * 1024;
const MAX_MAP_BYTES: usize = 16 * 1024 * 1024;
const MAX_PROBE_SOURCE_BYTES: u64 = 1_024;
const MAX_INVENTORY_ENTRIES: usize = 100_000;
const MAX_COMPILER_FILE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_SELECTION_BYTES: usize = canonical::MAX_DOCUMENT_BYTES;
const TREE_LIMITS: TreeTraversalLimits = TreeTraversalLimits {
    max_entries: MAX_INVENTORY_ENTRIES,
    max_regular_file_bytes: MAX_COMPILER_FILE_BYTES,
};

const BINDING_MEMBER: &str = "llvm-sdk-aros-native-consumer-binding.json";
const COMPILER_INVENTORY_MEMBER: &str = "llvm-sdk-compiler-inventory.json";
const RECEIPT_MEMBER: &str = "llvm-sdk-links.receipt.json";
const INVENTORY_MEMBER: &str = "llvm-sdk-sdk-inventory.json";

/// Independently measured source/compiler selection for portable LLVM SDK links.
///
/// The serialized record is closed, but it is not an authority token. The
/// process-local compiler inventory bytes are deliberately omitted from serde:
/// their digest, size, tree digest and count are selected before execution,
/// while an exported portable proof carries the separately checked bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LlvmSdkLinkSelection {
    schema: String,
    identity: SdkIdentity,
    binding: llvm_sdk_generated_root::ConsumerBinding,
    binding_sha256: Sha256Digest,
    source_root: String,
    cmake_build_root: String,
    compiler_root: String,
    proof_root: String,
    profile: String,
    toolchain_profile: String,
    contract_path_relative: String,
    contract_sha256: Sha256Digest,
    source_tree_sha256: Sha256Digest,
    input_paths: Vec<String>,
    probes: BTreeMap<String, SelectedProbe>,
    compiler: ArosCompilerIdentity,
    compiler_tree_sha256: Sha256Digest,
    compiler_inventory_sha256: Sha256Digest,
    compiler_inventory_file_sha256: Sha256Digest,
    compiler_inventory_size: u64,
    compiler_entries: usize,
    driver_roles: BTreeMap<String, SelectedDriverRole>,
    #[serde(skip)]
    compiler_inventory_bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectedProbe {
    path: String,
    sha256: Sha256Digest,
    size: u64,
    libraries: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectedDriverRole {
    path: String,
    sha256: Sha256Digest,
    size: u64,
}

/// Independent runtime expectations for portable LLVM SDK read-back.
pub(super) struct LlvmSdkPortableExpectations<'a> {
    pub(super) engine_api_version: u32,
    pub(super) engine_sha256: &'a Sha256Digest,
    pub(super) helpers: &'a BTreeMap<String, CompatibilityHelperReport>,
}

/// Opaque local-consistency result for the portable LLVM SDK link evidence.
///
/// This does not prove SDK bytes from an inventory or authenticate execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlvmSdkPortableProof {
    identity: SdkIdentity,
    binding_sha256: Sha256Digest,
    receipt_sha256: Sha256Digest,
    inventory_sha256: Sha256Digest,
    entries: usize,
}

impl LlvmSdkPortableProof {
    #[must_use]
    pub const fn identity(&self) -> &SdkIdentity {
        &self.identity
    }

    #[must_use]
    pub const fn binding_sha256(&self) -> &Sha256Digest {
        &self.binding_sha256
    }

    #[must_use]
    pub const fn receipt_sha256(&self) -> &Sha256Digest {
        &self.receipt_sha256
    }

    #[must_use]
    pub const fn inventory_sha256(&self) -> &Sha256Digest {
        &self.inventory_sha256
    }

    #[must_use]
    pub const fn entries(&self) -> usize {
        self.entries
    }
}

impl LlvmSdkLinkSelection {
    /// Exact source profile selected before generation, not the compiler profile.
    #[must_use]
    pub fn source_profile(&self) -> &str {
        &self.profile
    }

    /// Selected complete source SDK identity.
    #[must_use]
    pub const fn identity(&self) -> &SdkIdentity {
        &self.identity
    }

    /// Independently measured compiler identity.
    #[must_use]
    pub const fn compiler(&self) -> &ArosCompilerIdentity {
        &self.compiler
    }

    /// Source profile's target/toolchain selector, for independent package binding.
    #[must_use]
    pub fn toolchain_profile(&self) -> &str {
        &self.toolchain_profile
    }

    /// The binding path whose bytes the owning build/export phase must retain.
    #[must_use]
    pub fn binding_path(&self) -> PathBuf {
        Path::new(&self.cmake_build_root).join(BINDING_FILE)
    }

    /// Compiler inventory measured before the owning compatibility build.
    #[must_use]
    pub fn compiler_inventory_bytes(&self) -> &[u8] {
        &self.compiler_inventory_bytes
    }

    /// Rerun structural checks without touching any recorded path.
    pub(super) fn validate(&self) -> Result<(), ContractError> {
        if self.schema != SELECTION_SCHEMA
            || self.identity.target != self.binding.target
            || self.binding.schema != "aros-sdk-consumer-validation-v1"
            || self.binding.qualification != "source-binding-not-graph-or-build-proof"
            || self.binding.profile != self.profile
            || !valid_portable_token(&self.toolchain_profile)
            || self.binding.source_dir != self.source_root
            || self.binding.contract_sha256 != self.contract_sha256
            || self.contract_sha256 != self.identity.source.contract_sha256
            || self.binding.input_paths != self.input_paths
            || self.source_tree_sha256 != self.identity.source.content_sha256
            || self.input_paths.is_empty()
            || self.input_paths.len() > 100_000
            || !valid_portable_token(&self.profile)
        {
            return Err(error(
                "LLVM SDK link selection identity or binding is invalid",
            ));
        }
        self.identity
            .validate()
            .map_err(|_| error("LLVM SDK link selection identity is invalid"))?;
        llvm_sdk_application::validate_target(&self.identity.target)?;
        validate_absolute_lexical(&self.source_root)?;
        validate_absolute_lexical(&self.cmake_build_root)?;
        validate_absolute_lexical(&self.compiler_root)?;
        validate_absolute_lexical(&self.proof_root)?;
        validate_source_relative(&self.contract_path_relative)?;
        validate_include_relative(&self.binding.sdk_include_relative)?;
        let expected_contract = joined_path(&self.source_root, &self.contract_path_relative)?;
        if self.binding.contract_path != expected_contract {
            return Err(error(
                "LLVM SDK link binding contract path differs from selection",
            ));
        }
        if !roots_disjoint(&[
            Path::new(&self.source_root),
            Path::new(&self.cmake_build_root),
            Path::new(&self.compiler_root),
            Path::new(&self.proof_root),
        ]) {
            return Err(error("LLVM SDK selected roots overlap"));
        }
        if !self
            .input_paths
            .iter()
            .all(|path| validate_source_relative(path).is_ok())
            || self.input_paths.iter().collect::<BTreeSet<_>>().len() != self.input_paths.len()
        {
            return Err(error(
                "LLVM SDK link selection inputs are unsafe or duplicated",
            ));
        }
        if self
            .probes
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
            != BTreeSet::from(["c", "cxx"])
            || self
                .driver_roles
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>()
                != BTreeSet::from(["c", "cxx"])
        {
            return Err(error("LLVM SDK link selection omits a C or C++ role"));
        }
        for (language, extension) in [("c", "c"), ("cxx", "cpp")] {
            let probe = self
                .probes
                .get(language)
                .ok_or_else(|| error("LLVM SDK selected probe is absent"))?;
            validate_source_relative(&probe.path)?;
            if Path::new(&probe.path)
                .extension()
                .and_then(|part| part.to_str())
                != Some(extension)
                || probe.size == 0
                || probe.size > MAX_PROBE_SOURCE_BYTES
                || !self.input_paths.contains(&probe.path)
            {
                return Err(error("LLVM SDK selected probe is invalid"));
            }
            validate_libraries(&probe.libraries)?;
            let role = self
                .driver_roles
                .get(language)
                .ok_or_else(|| error("LLVM SDK selected compiler role is absent"))?;
            validate_source_relative(&role.path)?;
            let expected_role = if language == "c" {
                "bin/clang"
            } else {
                "bin/clang++"
            };
            if role.path != expected_role || role.size == 0 {
                return Err(error("LLVM SDK compiler role differs from its fixed path"));
            }
        }
        if self.compiler_entries == 0
            || self.compiler_entries > MAX_INVENTORY_ENTRIES
            || self.compiler_inventory_size == 0
            || self.compiler_inventory_size > MAX_INVENTORY_BYTES as u64
            || !valid_digest(&self.binding_sha256)
            || !valid_digest(&self.source_tree_sha256)
            || !valid_digest(&self.compiler_tree_sha256)
            || !valid_digest(&self.compiler_inventory_sha256)
            || !valid_digest(&self.compiler_inventory_file_sha256)
        {
            return Err(error(
                "LLVM SDK link selection inventory identity is invalid",
            ));
        }
        let canonical_binding = canonical::bytes(
            &serde_json::to_value(&self.binding)
                .map_err(|_| error("cannot encode selected LLVM SDK binding"))?,
        )?;
        if sha256_bytes(&canonical_binding) != self.binding_sha256 {
            return Err(error("selected LLVM SDK binding digest is inconsistent"));
        }
        if !self.compiler_inventory_bytes.is_empty()
            && (self.compiler_inventory_bytes.len() as u64 != self.compiler_inventory_size
                || sha256_bytes(&self.compiler_inventory_bytes)
                    != self.compiler_inventory_file_sha256)
        {
            return Err(error("selected LLVM compiler inventory bytes changed"));
        }
        let value = serde_json::to_value(self)
            .map_err(|_| error("cannot encode LLVM SDK link selection"))?;
        if canonical::bytes(&value)?.len() > MAX_SELECTION_BYTES {
            return Err(error("LLVM SDK link selection exceeds its document bound"));
        }
        Ok(())
    }
}

/// Measure source-selected probes and the exact LLVM driver/compiler inventory.
///
/// The SDK output and proof directories may not exist yet. No CMake or
/// application build occurs here; this is a pre-execution selection boundary.
pub(super) fn select(
    request: &LlvmSdkGeneratedRootRequest,
    drivers: &LlvmSdkDriverBinding,
    proof_root: &Path,
    cancellation: &CancellationToken,
) -> Result<LlvmSdkLinkSelection, ContractError> {
    check_cancelled(cancellation)?;
    if request.timeout.is_zero() {
        return Err(error(
            "LLVM SDK link selection requires a positive deadline",
        ));
    }
    if drivers.target() != &request.source.identity().target {
        return Err(error(
            "LLVM SDK drivers differ from the selected source target",
        ));
    }
    request.source.revalidate()?;
    drivers.revalidate(cancellation)?;
    let binding = llvm_sdk_generated_root::selected_binding(request, cancellation)?;
    let source_root = request.source.materialized_source_root();
    let profile = request.source.source_profile();
    if profile
        .transpiler
        .as_ref()
        .is_none_or(|context| context.toolchain != "llvm")
    {
        return Err(error(
            "LLVM SDK selection requires an explicit LLVM source profile",
        ));
    }
    let contract_relative = profile
        .sdk_consumer_contract
        .as_deref()
        .ok_or_else(|| error("LLVM SDK source does not declare a consumer contract"))?;
    validate_source_relative(contract_relative)?;
    let loaded = aros_common::sdk_consumer_contract::load_bound_sdk_consumer_contract(
        source_root,
        Path::new(contract_relative),
        profile,
    )
    .map_err(|_| error("LLVM SDK source consumer contract changed"))?;
    if loaded.sha256 != request.source.identity().source.contract_sha256
        || loaded.contract.target != request.source.identity().target
        || loaded.contract.profile != binding.profile
        || binding.input_paths
            != loaded
                .contract
                .inputs
                .iter()
                .map(|input| input.path.clone())
                .collect::<Vec<_>>()
    {
        return Err(error(
            "LLVM SDK source contract and selected binding differ",
        ));
    }
    let source_root = source_root
        .canonicalize()
        .map_err(|_| error("LLVM SDK source root cannot be resolved"))?;
    let source_root_text = absolute_lexical_text(&source_root)?;
    let build_root =
        canonical_future_path(&request.cmake_build_root, "LLVM SDK CMake build", false)?;
    let build_root_text = absolute_lexical_text(&build_root)?;
    let proof_root = canonical_future_path(proof_root, "LLVM SDK portable proof root", true)?;
    let proof_root_text = absolute_lexical_text(&proof_root)?;
    let compiler_root = drivers
        .clang_path()
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| error("LLVM SDK compiler root is absent"))?
        .to_owned();
    let compiler_root = compiler_root
        .canonicalize()
        .map_err(|_| error("LLVM SDK compiler root cannot be resolved"))?;
    if drivers.clang_path() != compiler_root.join("bin/clang")
        || drivers.clangxx_path() != compiler_root.join("bin/clang++")
    {
        return Err(error(
            "LLVM SDK compiler roles differ from their selected root",
        ));
    }
    let compiler_root_text = absolute_lexical_text(&compiler_root)?;
    if !roots_disjoint(&[
        &source_root,
        Path::new(&build_root_text),
        &compiler_root,
        &proof_root,
    ]) {
        return Err(error(
            "LLVM SDK source, build, compiler and proof roots overlap",
        ));
    }
    let preparation = super::validate_preparation(&request.preparation)?;
    for protected in [
        preparation.source.as_path(),
        preparation.engine.as_path(),
        preparation.helpers.as_path(),
    ] {
        if overlaps(&proof_root, protected)
            || overlaps(&PathBuf::from(&build_root_text), protected)
            || overlaps(&compiler_root, protected)
        {
            return Err(error(
                "LLVM SDK proof/build/compiler root overlaps preparation inputs",
            ));
        }
    }

    let probes = BTreeMap::from([
        (
            "c".to_owned(),
            select_probe(
                &source_root,
                &loaded.contract.native_sdk_link_probes.c,
                &loaded.contract.inputs,
            )?,
        ),
        (
            "cxx".to_owned(),
            select_probe(
                &source_root,
                &loaded.contract.native_sdk_link_probes.cxx,
                &loaded.contract.inputs,
            )?,
        ),
    ]);
    let source_tree_sha256 = request.source.identity().source.content_sha256.clone();

    let compiler_before = measure_tree_content_cas_bounded(&compiler_root, TREE_LIMITS)
        .map_err(|_| error("cannot measure bounded LLVM compiler tree"))?;
    let measured_compiler_sha256 = compiler_before.payload_digest_excluding(None);
    if measured_compiler_sha256 != *drivers.compiler_tree_sha256() {
        return Err(error(
            "LLVM compiler tree differs from its measured driver binding",
        ));
    }
    let (inventory_text, inventory) = toolchain_tree_inventory_excluding(&compiler_root, &[])
        .map_err(|_| error("cannot inventory the bounded LLVM compiler tree"))?;
    let inventory_index = validate_inventory_with_limit(&inventory, MAX_COMPILER_FILE_BYTES)?;
    let _ = inventory_index;
    let compiler_inventory_bytes = native_sdk_relocation::encode_inventory(&inventory)?;
    if compiler_inventory_bytes.is_empty() || compiler_inventory_bytes.len() > MAX_INVENTORY_BYTES {
        return Err(error(
            "LLVM compiler inventory exceeds its encoded byte bound",
        ));
    }
    let compiler_inventory_sha256 = Sha256Digest::parse(&inventory_text)
        .map_err(|_| error("LLVM compiler inventory digest is invalid"))?;
    let compiler_inventory_file_sha256 = sha256_bytes(&compiler_inventory_bytes);
    let compiler_inventory_size = u64::try_from(compiler_inventory_bytes.len())
        .map_err(|_| error("LLVM compiler inventory length overflowed"))?;
    if measure_tree_content_cas_bounded(&compiler_root, TREE_LIMITS)
        .map_err(|_| error("cannot remeasure bounded LLVM compiler tree"))?
        != compiler_before
    {
        return Err(error(
            "LLVM compiler tree changed during inventory selection",
        ));
    }
    let driver_roles = BTreeMap::from([
        (
            "c".to_owned(),
            selected_role(drivers.clang_identity(), &compiler_root)?,
        ),
        (
            "cxx".to_owned(),
            selected_role(drivers.clangxx_identity(), &compiler_root)?,
        ),
    ]);
    let binding_sha256 = sha256_bytes(&canonical::bytes(
        &serde_json::to_value(&binding)
            .map_err(|_| error("cannot encode source-derived LLVM SDK binding"))?,
    )?);
    let selection = LlvmSdkLinkSelection {
        schema: SELECTION_SCHEMA.to_owned(),
        identity: request.source.identity().clone(),
        binding,
        binding_sha256,
        source_root: source_root_text,
        cmake_build_root: build_root_text,
        compiler_root: compiler_root_text,
        proof_root: proof_root_text,
        profile: loaded.contract.profile,
        toolchain_profile: profile.toolchain_profile().to_owned(),
        contract_path_relative: contract_relative.to_owned(),
        contract_sha256: loaded.sha256,
        source_tree_sha256,
        input_paths: loaded
            .contract
            .inputs
            .iter()
            .map(|input| input.path.clone())
            .collect(),
        probes,
        compiler: drivers.compiler().clone(),
        compiler_tree_sha256: measured_compiler_sha256,
        compiler_inventory_sha256,
        compiler_inventory_file_sha256,
        compiler_inventory_size,
        compiler_entries: inventory.len(),
        driver_roles,
        compiler_inventory_bytes,
    };
    selection.validate()?;
    request.source.revalidate()?;
    drivers.revalidate(cancellation)?;
    check_cancelled(cancellation)?;
    Ok(selection)
}

/// Exact exported flat member set with their internal retained paths/bounds.
pub(super) fn expected_members() -> BTreeMap<String, (String, usize)> {
    let mut members = BTreeMap::from([
        (
            BINDING_MEMBER.to_owned(),
            (BINDING_FILE.to_owned(), MAX_DOCUMENT_BYTES),
        ),
        (
            COMPILER_INVENTORY_MEMBER.to_owned(),
            ("compiler-inventory.json".to_owned(), MAX_INVENTORY_BYTES),
        ),
        (
            RECEIPT_MEMBER.to_owned(),
            (
                "receipt/llvm-sdk-links.receipt.json".to_owned(),
                MAX_DOCUMENT_BYTES,
            ),
        ),
        (
            INVENTORY_MEMBER.to_owned(),
            ("sdk-inventory.json".to_owned(), MAX_INVENTORY_BYTES),
        ),
    ]);
    for label in ["original", "relocated"] {
        for language in ["c", "cxx"] {
            let stem = format!("{label}-{language}");
            members.insert(
                format!("llvm-sdk-{stem}.elf"),
                (format!("links/{stem}.elf"), MAX_ELF_BYTES),
            );
            members.insert(
                format!("llvm-sdk-{stem}.map"),
                (format!("links/{stem}.map"), MAX_MAP_BYTES),
            );
        }
    }
    for language in ["c", "cxx"] {
        let phase = format!("standalone-{language}");
        members.insert(
            format!("llvm-sdk-{phase}.report.json"),
            (
                format!("links/reports/{phase}.report.json"),
                MAX_DOCUMENT_BYTES,
            ),
        );
        for index in [1, 2] {
            for stream in ["stdout", "stderr"] {
                members.insert(
                    format!("llvm-sdk-{phase}.{index}.{stream}.log"),
                    (
                        format!("links/reports/{phase}.{index}.{stream}.log"),
                        super::MAX_RENDERED_LOG_BYTES,
                    ),
                );
            }
        }
    }
    members
}

/// Reconstruct and check a portable flat proof without reopening recorded roots.
pub(super) fn readback<B: AsRef<[u8]>>(
    files: &BTreeMap<String, B>,
    selected: &LlvmSdkLinkSelection,
    expected: &LlvmSdkPortableExpectations<'_>,
) -> Result<LlvmSdkPortableProof, ContractError> {
    selected.validate()?;
    let members = expected_members();
    if files.len() != members.len() || files.keys().ne(members.keys()) {
        return Err(error(
            "portable LLVM SDK proof differs from its exact file set",
        ));
    }
    for (name, bytes) in files {
        let (_, limit) = members
            .get(name)
            .ok_or_else(|| error("portable LLVM SDK proof has an unexpected member"))?;
        if !name.starts_with("llvm-sdk-") || bytes.as_ref().len() > *limit {
            return Err(error(
                "portable LLVM SDK member exceeds its type-specific bound",
            ));
        }
    }

    let binding_bytes = member(files, BINDING_MEMBER)?;
    let binding: llvm_sdk_generated_root::ConsumerBinding =
        serde_json::from_slice(binding_bytes)
            .map_err(|_| error("portable LLVM SDK source binding is not closed JSON"))?;
    let binding_canonical = canonical::bytes(
        &serde_json::to_value(&binding)
            .map_err(|_| error("cannot encode portable LLVM SDK source binding"))?,
    )?;
    if binding_canonical != binding_bytes || binding != selected.binding {
        return Err(error(
            "portable LLVM SDK source binding differs from selection",
        ));
    }
    let binding_sha256 = sha256_bytes(binding_bytes);
    if binding_sha256 != selected.binding_sha256 {
        return Err(error(
            "portable LLVM SDK source binding digest differs from selection",
        ));
    }

    let compiler_inventory_bytes = member(files, COMPILER_INVENTORY_MEMBER)?;
    let compiler_inventory: BoundedInventory = serde_json::from_slice(compiler_inventory_bytes)
        .map_err(|_| error("portable LLVM compiler inventory is not bounded closed JSON"))?;
    let compiler_inventory = compiler_inventory.0;
    let compiler_inventory_canonical =
        native_sdk_relocation::encode_inventory(&compiler_inventory)?;
    if compiler_inventory_canonical != compiler_inventory_bytes
        || compiler_inventory.len() != selected.compiler_entries
        || compiler_inventory_bytes.len() as u64 != selected.compiler_inventory_size
        || sha256_bytes(compiler_inventory_bytes) != selected.compiler_inventory_file_sha256
        || toolchain_inventory_sha256(&compiler_inventory)
            .map_err(|_| error("cannot hash portable LLVM compiler inventory"))?
            != selected.compiler_inventory_sha256.as_str()
    {
        return Err(error(
            "portable LLVM compiler inventory differs from preselection",
        ));
    }
    if !selected.compiler_inventory_bytes.is_empty()
        && selected.compiler_inventory_bytes != compiler_inventory_bytes
    {
        return Err(error(
            "portable compiler inventory differs from selected local bytes",
        ));
    }
    let compiler_index =
        validate_inventory_with_limit(&compiler_inventory, MAX_COMPILER_FILE_BYTES)?;
    validate_selected_compiler_roles(selected, &compiler_index)?;

    let sdk_inventory_bytes = member(files, INVENTORY_MEMBER)?;
    let sdk_inventory: BoundedInventory = serde_json::from_slice(sdk_inventory_bytes)
        .map_err(|_| error("portable LLVM SDK inventory is not bounded closed JSON"))?;
    let sdk_inventory = sdk_inventory.0;
    if native_sdk_relocation::encode_inventory(&sdk_inventory)? != sdk_inventory_bytes {
        return Err(error("portable LLVM SDK inventory is not canonical"));
    }
    let sdk_index = validate_inventory(&sdk_inventory)?;
    if sdk_index
        .get("include")
        .is_none_or(|entry| entry.kind != "directory")
    {
        return Err(error(
            "portable LLVM SDK inventory omits its include directory",
        ));
    }
    let inventory_sha256 = Sha256Digest::parse(
        &toolchain_inventory_sha256(&sdk_inventory)
            .map_err(|_| error("cannot hash portable LLVM SDK inventory"))?,
    )
    .map_err(|_| error("portable LLVM SDK inventory digest is invalid"))?;

    let receipt_bytes = member(files, RECEIPT_MEMBER)?;
    let receipt: llvm_sdk_links::LinkRecord = serde_json::from_slice(receipt_bytes)
        .map_err(|_| error("portable LLVM SDK receipt is not closed JSON"))?;
    if llvm_sdk_links::encode(&receipt)? != receipt_bytes {
        return Err(error("portable LLVM SDK receipt is not canonical"));
    }
    let receipt_sha256 = sha256_bytes(receipt_bytes);
    let expected_internal = internal_member_map(files)?;
    if receipt.schema != "aros-llvm-sdk-link-receipt-v1"
        || receipt.assurance != "local-links-not-generation-authentication-or-release-admission"
        || receipt.identity != selected.identity
        || receipt.compiler != selected.compiler
        || receipt.compiler_tree_sha256 != selected.compiler_tree_sha256
        || receipt.sdk_inventory_sha256 != inventory_sha256
        || receipt.sdk_entries != sdk_inventory.len()
        || receipt.members != expected_internal
    {
        return Err(error(
            "portable LLVM SDK receipt differs from selected identities",
        ));
    }

    let original_sdk = sdk_root_from_binding(selected)?;
    let relocated_sdk = Path::new(&selected.proof_root).join("relocated-sdk");
    let output = Path::new(&selected.proof_root).join("links");
    let temporary = Path::new(&selected.proof_root).join("tmp");
    let environment = llvm_sdk_links::environment(&temporary)?;
    let resolved_environment = environment::resolve(&environment)?;
    let environment_sha256 = environment::identity(&resolved_environment)?;
    let helpers = expected.helpers;
    if expected.engine_api_version == 0
        || helpers.is_empty()
        || helpers.values().any(|helper| helper.size == 0)
    {
        return Err(error(
            "portable LLVM SDK runtime expectations are incomplete",
        ));
    }

    for (language, phase, phase_stem) in [
        ("c", CompatibilityPhase::StandaloneC, "standalone-c"),
        ("cxx", CompatibilityPhase::StandaloneCxx, "standalone-cxx"),
    ] {
        let report_name = format!("llvm-sdk-{phase_stem}.report.json");
        let report_bytes = member(files, &report_name)?;
        let report = CompatibilityProbeReport::parse(report_bytes)?;
        let report_canonical = canonical::bytes(
            &serde_json::to_value(&report)
                .map_err(|_| error("cannot encode portable LLVM SDK phase report"))?,
        )?;
        if report_canonical != report_bytes
            || report.phase != phase
            || report.commands.len() != 2
            || report.engine_api_version != expected.engine_api_version
            || report.engine_sha256 != *expected.engine_sha256
            || report.source_tree_sha256 != selected.source_tree_sha256
            || report.helpers != *helpers
            || !report.host_tools.is_empty()
            || report.environment_sha256 != environment_sha256
        {
            return Err(error(
                "portable LLVM SDK phase report differs from selected process inputs",
            ));
        }
        let probe = selected
            .probes
            .get(language)
            .ok_or_else(|| error("portable LLVM SDK selected source probe is absent"))?;
        let fixture = Path::new(&selected.source_root).join(&probe.path);
        let driver = selected
            .driver_roles
            .get(language)
            .ok_or_else(|| error("portable LLVM SDK compiler role is absent"))?;
        let program = Path::new(&selected.compiler_root).join(&driver.path);
        let selected_probe = aros_common::native_consumer_contract::NativeSdkLinkProbe {
            source: probe.path.clone(),
            libraries: probe.libraries.clone(),
        };
        for (index, (label, sdk)) in [
            ("original", original_sdk.as_path()),
            ("relocated", relocated_sdk.as_path()),
        ]
        .into_iter()
        .enumerate()
        {
            let command = llvm_sdk_application::application_command(
                &program,
                &fixture,
                &selected_probe,
                &selected.identity.target,
                sdk,
                &output,
                (language, label),
            )?;
            let evidence = &report.commands[index];
            if evidence.program_sha256 != driver.sha256
                || evidence.command_sha256
                    != command_identity(&command.program, &command.arguments)?
            {
                return Err(error(
                    "LLVM SDK retained invocation differs from its selected command",
                ));
            }
            let phase_name = format!("llvm-sdk-{phase_stem}.{}.stdout.log", index + 1);
            let stdout = member(files, &phase_name)?;
            let stderr_name = format!("llvm-sdk-{phase_stem}.{}.stderr.log", index + 1);
            let stderr = member(files, &stderr_name)?;
            if stdout.is_empty()
                || sha256_bytes(stdout) != evidence.stdout_sha256
                || sha256_bytes(stderr) != evidence.stderr_sha256
                || has_truncation_marker(stdout)
                || has_truncation_marker(stderr)
            {
                return Err(error(
                    "portable LLVM SDK process logs are empty, truncated or mismatched",
                ));
            }
            let stem = format!("{label}-{language}");
            let elf_bytes = member(files, &format!("llvm-sdk-{stem}.elf"))?;
            llvm_sdk_application::verify_application_elf(elf_bytes, &selected.identity.target)?;
            if label == "original" {
                let relocated_bytes = member(files, &format!("llvm-sdk-relocated-{language}.elf"))?;
                if elf_bytes != relocated_bytes {
                    return Err(error("LLVM SDK application ELF differs after relocation"));
                }
            }
            let map_bytes = member(files, &format!("llvm-sdk-{stem}.map"))?;
            validate_trace_and_map(
                stdout,
                map_bytes,
                &TraceSelection {
                    sdk,
                    original_sdk: &original_sdk,
                    compiler_root: &selected.compiler_root,
                    temporary: &temporary,
                    fixture: &fixture,
                    libraries: &probe.libraries,
                    relocated: label == "relocated",
                    sdk_index: &sdk_index,
                    compiler_index: &compiler_index,
                },
            )?;
        }
    }
    Ok(LlvmSdkPortableProof {
        identity: selected.identity.clone(),
        binding_sha256,
        receipt_sha256,
        inventory_sha256,
        entries: sdk_inventory.len(),
    })
}

struct TraceSelection<'a> {
    sdk: &'a Path,
    original_sdk: &'a Path,
    compiler_root: &'a str,
    temporary: &'a Path,
    fixture: &'a Path,
    libraries: &'a [String],
    relocated: bool,
    sdk_index: &'a BTreeMap<String, &'a ArosToolchainManifestEntry>,
    compiler_index: &'a BTreeMap<String, &'a ArosToolchainManifestEntry>,
}

fn validate_trace_and_map(
    trace_bytes: &[u8],
    map_bytes: &[u8],
    selection: &TraceSelection<'_>,
) -> Result<(), ContractError> {
    let TraceSelection {
        sdk,
        original_sdk,
        compiler_root,
        temporary,
        fixture,
        libraries,
        relocated,
        sdk_index,
        compiler_index,
    } = *selection;
    if trace_bytes.is_empty() || map_bytes.is_empty() {
        return Err(error("portable LLVM SDK link trace or map is empty"));
    }
    let trace = std::str::from_utf8(trace_bytes)
        .map_err(|_| error("portable LLVM SDK link trace is not UTF-8"))?;
    let map = std::str::from_utf8(map_bytes)
        .map_err(|_| error("portable LLVM SDK link map is not UTF-8"))?;
    if trace.contains(" bytes omitted by aros]") || map.contains(" bytes omitted by aros]") {
        return Err(error("portable LLVM SDK trace or map is truncated"));
    }
    let original_text = absolute_lexical_text(original_sdk)?;
    if relocated && (trace.contains(&original_text) || map.contains(&original_text)) {
        return Err(error(
            "relocated LLVM SDK trace or map retains its original SDK root",
        ));
    }
    let temporary_text = absolute_lexical_text(temporary)?;
    let fixture_stem = fixture
        .file_stem()
        .and_then(|part| part.to_str())
        .filter(|stem| !stem.is_empty() && !stem.chars().any(char::is_control))
        .ok_or_else(|| error("LLVM SDK fixture has no portable file stem"))?;
    let temp_prefix = format!("{fixture_stem}-");
    let selected_archives = llvm_sdk_link_paths::selected_archive_names(libraries)?;
    let mut linked_inputs = 0usize;
    let mut saw_sdk_archive = false;
    let mut sdk_archives = BTreeSet::new();
    for line in trace.lines().filter(|line| !line.is_empty()) {
        let input = llvm_sdk_link_paths::parse_trace_input(line)?;
        let raw_path = input.path;
        llvm_sdk_link_paths::validate_absolute_raw_path(raw_path)?;
        let Some(relative) = path_below(raw_path, &temporary_text) else {
            let Some(relative_sdk) = path_below(raw_path, &absolute_lexical_text(sdk)?) else {
                if path_below(raw_path, compiler_root).is_some() {
                    validate_indexed_file(raw_path, compiler_root, compiler_index, "compiler")?;
                    if let Some(name) = Path::new(raw_path)
                        .file_name()
                        .and_then(|part| part.to_str())
                    {
                        if selected_archives.contains(name) {
                            return Err(error(
                                "source-declared LLVM SDK library resolved from the compiler tree",
                            ));
                        }
                    }
                    linked_inputs = linked_inputs.saturating_add(1);
                    continue;
                }
                return Err(error(
                    "LLVM SDK trace contains a foreign SDK/compiler input",
                ));
            };
            let resolved = resolve_inventory_path(relative_sdk, sdk_index)?;
            let entry = sdk_index
                .get(&resolved)
                .copied()
                .ok_or_else(|| error("LLVM SDK trace input is absent from its inventory"))?;
            if entry.kind != "file" {
                return Err(error(
                    "LLVM SDK trace input does not resolve to an SDK regular file",
                ));
            }
            if Path::new(relative_sdk)
                .extension()
                .is_some_and(|part| part == "a")
            {
                saw_sdk_archive = true;
                if let Some(name) = Path::new(relative_sdk)
                    .file_name()
                    .and_then(|part| part.to_str())
                {
                    if selected_archives.contains(name) {
                        sdk_archives.insert(name.to_owned());
                    }
                }
            }
            linked_inputs = linked_inputs.saturating_add(1);
            continue;
        };
        validate_driver_temporary(relative, &temp_prefix)?;
        linked_inputs = linked_inputs.saturating_add(1);
    }
    if linked_inputs == 0 || !saw_sdk_archive || sdk_archives != selected_archives {
        return Err(error(
            "LLVM SDK link trace omits SDK archives or source-declared libraries",
        ));
    }
    Ok(())
}

fn validate_selected_compiler_roles(
    selection: &LlvmSdkLinkSelection,
    index: &BTreeMap<String, &ArosToolchainManifestEntry>,
) -> Result<(), ContractError> {
    for role in selection.driver_roles.values() {
        let resolved = resolve_inventory_path(&role.path, index)?;
        let entry = index
            .get(&resolved)
            .copied()
            .ok_or_else(|| error("selected compiler role is absent from its inventory"))?;
        if entry.kind != "file"
            || entry.sha256.as_deref() != Some(role.sha256.as_str())
            || entry.size != Some(role.size)
        {
            return Err(error("selected compiler role differs from its inventory"));
        }
    }
    Ok(())
}

fn validate_indexed_file(
    raw: &str,
    root: &str,
    index: &BTreeMap<String, &ArosToolchainManifestEntry>,
    label: &str,
) -> Result<(), ContractError> {
    let relative = path_below(raw, root)
        .ok_or_else(|| error(format!("trace escaped selected {label} root")))?;
    let resolved = resolve_inventory_path(relative, index)?;
    let entry = index
        .get(&resolved)
        .copied()
        .ok_or_else(|| error(format!("trace input is absent from the {label} inventory")))?;
    if entry.kind != "file" {
        return Err(error(format!("trace input is not a regular {label} file")));
    }
    Ok(())
}

fn internal_member_map<B: AsRef<[u8]>>(
    files: &BTreeMap<String, B>,
) -> Result<BTreeMap<String, llvm_sdk_links::Member>, ContractError> {
    let mut internal = BTreeMap::new();
    for (flat_name, (relative, _)) in expected_members() {
        if flat_name == BINDING_MEMBER
            || flat_name == COMPILER_INVENTORY_MEMBER
            || flat_name == RECEIPT_MEMBER
        {
            continue;
        }
        let bytes = member(files, &flat_name)?;
        if internal
            .insert(
                relative,
                llvm_sdk_links::Member {
                    sha256: sha256_bytes(bytes),
                    size: u64::try_from(bytes.len())
                        .map_err(|_| error("portable LLVM SDK member length overflowed"))?,
                },
            )
            .is_some()
        {
            return Err(error("portable LLVM SDK internal member paths collide"));
        }
    }
    Ok(internal)
}

fn select_probe(
    source_root: &Path,
    probe: &aros_common::native_consumer_contract::NativeSdkLinkProbe,
    inputs: &[aros_common::native_build_contract::NativeBuildInput],
) -> Result<SelectedProbe, ContractError> {
    validate_source_relative(&probe.source)?;
    validate_libraries(&probe.libraries)?;
    let declared = inputs
        .iter()
        .find(|input| input.path == probe.source)
        .ok_or_else(|| error("LLVM SDK source probe is absent from sealed inputs"))?;
    let path = source_root.join(&probe.source);
    let (_, measured) = measure_regular_file_digest_bounded(&path, MAX_PROBE_SOURCE_BYTES)
        .map_err(|_| error("cannot measure bounded LLVM SDK source probe"))?;
    if measured.digest != declared.sha256 || measured.size == 0 {
        return Err(error("LLVM SDK source probe differs from its sealed input"));
    }
    Ok(SelectedProbe {
        path: probe.source.clone(),
        sha256: measured.digest,
        size: measured.size,
        libraries: probe.libraries.clone(),
    })
}

fn selected_role(
    role: &super::LlvmSdkDriverRoleIdentity,
    root: &Path,
) -> Result<SelectedDriverRole, ContractError> {
    let relative = role
        .invocation_path()
        .strip_prefix(root)
        .map_err(|_| error("LLVM SDK compiler role escapes its selected root"))?;
    let relative = relative
        .to_str()
        .ok_or_else(|| error("LLVM SDK compiler role path is not UTF-8"))?;
    validate_source_relative(relative)?;
    if !role.resolved_executable().starts_with(root) {
        return Err(error(
            "LLVM SDK compiler role resolves outside its selected root",
        ));
    }
    Ok(SelectedDriverRole {
        path: relative.to_owned(),
        sha256: role.file_sha256().clone(),
        size: role.file_size(),
    })
}

fn sdk_root_from_binding(selection: &LlvmSdkLinkSelection) -> Result<PathBuf, ContractError> {
    validate_include_relative(&selection.binding.sdk_include_relative)?;
    let include = Path::new(&selection.binding.sdk_include_relative);
    let relative_root = include
        .parent()
        .ok_or_else(|| error("LLVM SDK include binding has no SDK root"))?;
    if relative_root.as_os_str().is_empty() {
        return Err(error(
            "LLVM SDK include binding selects the CMake root itself",
        ));
    }
    Ok(Path::new(&selection.cmake_build_root).join(relative_root))
}

fn canonical_future_path(
    path: &Path,
    label: &str,
    reject_leaf_symlink: bool,
) -> Result<PathBuf, ContractError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
        || path.to_str().is_none()
    {
        return Err(error(format!(
            "{label} must be absolute and non-traversing"
        )));
    }
    let mut ancestor = path;
    let mut missing = Vec::new();
    loop {
        match std::fs::symlink_metadata(ancestor) {
            Ok(metadata) if metadata.is_dir() || metadata.file_type().is_symlink() => break,
            Ok(_) => return Err(error(format!("{label} path component is not a directory"))),
            Err(failure) if failure.kind() == std::io::ErrorKind::NotFound => {
                let component = ancestor
                    .file_name()
                    .ok_or_else(|| error(format!("{label} has no resolvable ancestor")))?;
                missing.push(component.to_owned());
                ancestor = ancestor
                    .parent()
                    .ok_or_else(|| error(format!("{label} has no parent")))?;
            }
            Err(_) => return Err(error(format!("{label} cannot be safely inspected"))),
        }
    }
    let mut canonical = ancestor.canonicalize().map_err(|_| {
        error(format!(
            "{label} nearest existing ancestor cannot be resolved"
        ))
    })?;
    for component in missing.into_iter().rev() {
        canonical.push(component);
    }
    if reject_leaf_symlink
        && std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        return Err(error(format!("{label} leaf must not be a symbolic link")));
    }
    if !canonical.is_absolute() || canonical.to_str().is_none() {
        return Err(error(format!("{label} canonical spelling is not portable")));
    }
    Ok(canonical)
}

fn absolute_lexical_text(path: &Path) -> Result<String, ContractError> {
    if !path.is_absolute()
        || path.to_str().is_none()
        || path.as_os_str().is_empty()
        || path.to_string_lossy().contains("//")
        || path
            .components()
            .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
    {
        return Err(error(
            "selected LLVM SDK path is not an absolute lexical path",
        ));
    }
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| error("selected LLVM SDK path is not UTF-8"))
}

fn validate_absolute_lexical(path: &str) -> Result<(), ContractError> {
    if path.is_empty()
        || path.len() > 32 * 1024
        || path.contains("//")
        || path.chars().any(char::is_control)
        || !Path::new(path).is_absolute()
        || Path::new(path).file_name().is_none()
        || Path::new(path)
            .components()
            .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
        || path
            .split('/')
            .skip(1)
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
    {
        return Err(error("portable LLVM SDK root path is unsafe"));
    }
    Ok(())
}

fn validate_source_relative(path: &str) -> Result<(), ContractError> {
    let candidate = Path::new(path);
    if path.is_empty()
        || path.len() > 32 * 1024
        || path.starts_with('/')
        || path.contains('\\')
        || path.chars().any(char::is_control)
        || candidate.is_absolute()
        || candidate
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        || path
            .split('/')
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
    {
        return Err(error("portable LLVM SDK relative path is unsafe"));
    }
    Ok(())
}

fn validate_include_relative(path: &str) -> Result<(), ContractError> {
    validate_source_relative(path)?;
    if Path::new(path).file_name().and_then(|part| part.to_str()) != Some("include") {
        return Err(error(
            "source-selected LLVM SDK include path must end in include",
        ));
    }
    Ok(())
}

fn validate_libraries(libraries: &[String]) -> Result<(), ContractError> {
    let names = llvm_sdk_link_paths::selected_archive_names(libraries)?;
    if libraries.len() > 64 || names.len() != libraries.len() {
        return Err(error(
            "source-declared LLVM SDK libraries are duplicated or excessive",
        ));
    }
    Ok(())
}

fn joined_path(root: &str, relative: &str) -> Result<String, ContractError> {
    validate_absolute_lexical(root)?;
    validate_source_relative(relative)?;
    let joined = Path::new(root).join(relative);
    joined
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| error("joined LLVM SDK source path is not UTF-8"))
}

fn path_below<'a>(path: &'a str, root: &str) -> Option<&'a str> {
    let suffix = path.strip_prefix(root)?;
    if suffix.is_empty() {
        None
    } else {
        suffix.strip_prefix('/')
    }
}

fn validate_driver_temporary(relative: &str, expected_prefix: &str) -> Result<(), ContractError> {
    let path = Path::new(relative);
    let name = path
        .file_name()
        .and_then(|part| part.to_str())
        .ok_or_else(|| error("LLVM driver temporary name is not portable"))?;
    let suffix = name
        .strip_prefix(expected_prefix)
        .and_then(|tail| tail.strip_suffix(".o"));
    if path.components().count() != 1
        || !suffix.is_some_and(|suffix| {
            suffix.len() == 6 && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
    {
        return Err(error(
            "LLVM driver temporary is not the direct fixture-derived object",
        ));
    }
    Ok(())
}

fn valid_portable_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn valid_digest(digest: &Sha256Digest) -> bool {
    Sha256Digest::parse(digest.as_str()).is_ok() && digest.as_str().bytes().any(|byte| byte != b'0')
}

fn roots_disjoint(roots: &[&Path]) -> bool {
    for (index, left) in roots.iter().enumerate() {
        for right in roots.iter().skip(index + 1) {
            if overlaps(left, right) {
                return false;
            }
        }
    }
    true
}

fn overlaps(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

fn member<'a, B: AsRef<[u8]>>(
    files: &'a BTreeMap<String, B>,
    name: &str,
) -> Result<&'a [u8], ContractError> {
    files
        .get(name)
        .map(AsRef::as_ref)
        .ok_or_else(|| error("portable LLVM SDK proof omits a selected member"))
}

fn has_truncation_marker(bytes: &[u8]) -> bool {
    bytes
        .windows(b" bytes omitted by aros]".len())
        .any(|window| window == b" bytes omitted by aros]")
}

fn check_cancelled(cancellation: &CancellationToken) -> Result<(), ContractError> {
    if cancellation.is_cancelled() {
        Err(error("LLVM SDK source/compiler selection was cancelled"))
    } else {
        Ok(())
    }
}

fn error(message: impl Into<String>) -> ContractError {
    ContractError::compatibility(message)
}
