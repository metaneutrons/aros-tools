//! Rootless read-back for retained native SDK link evidence.
//!
//! This module checks byte consistency, selected source/compiler identities,
//! ordinary-driver command identities, trace containment and RISC-V/AROS ELF
//! headers. It does not reopen foreign roots, authenticate execution, or prove
//! SDK bytes from an inventory document.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ffi::OsStr;
use std::fmt;
use std::path::{Component, Path, PathBuf};

use aros_common::native_build_contract::NativeBuildAbi;
use aros_common::native_consumer_contract::{
    load_bound_native_consumer_contract, NativeSdkLinkProbe,
};
use aros_common::toolchain_layout::ToolchainToolLayout;
use aros_common::{
    measure_regular_file_digest_bounded, measure_tree_content_cas_bounded, sha256_bytes,
    toolchain_inventory_sha256, ArosCompilerIdentity, ArosToolchainManifestEntry, Sha256Digest,
    TreeTraversalLimits,
};
use serde::de::{self, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};

use super::native_sdk_links::{self, ConsumerBinding, FileIdentity, NativeSdkLinkRequest, Receipt};
use super::{
    CompatibilityEnvironment, CompatibilityHelperReport, CompatibilityPhase,
    CompatibilityProbeReport,
};
use crate::ContractError;

const SELECTION_SCHEMA: &str = "aros-native-sdk-link-selection-v1";
const BINDING_MEMBER: &str = "sdk-aros-native-consumer-binding.json";
const RECEIPT_MEMBER: &str = "sdk-native-sdk-links.receipt.json";
const INVENTORY_MEMBER: &str = "sdk-native-sdk-inventory.json";
const MAX_SOURCE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_INVENTORY_BYTES: usize = 16 * 1024 * 1024;
const MAX_MAP_BYTES: usize = 16 * 1024 * 1024;
const MAX_ELF_BYTES: usize = 128 * 1024 * 1024;
const MAX_DRIVER_BYTES: u64 = 512 * 1024 * 1024;
const MAX_INVENTORY_ENTRIES: usize = 100_000;
const MAX_INVENTORY_FILE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_INVENTORY_DEPTH: usize = 128;
const MAX_INVENTORY_PATH_BYTES: usize = 32 * 1024;
const MAX_SYMLINK_HOPS: usize = 128;
const PACKAGE_LIMITS: TreeTraversalLimits = TreeTraversalLimits {
    max_entries: 100_000,
    max_regular_file_bytes: 4 * 1024 * 1024 * 1024,
};

/// Independently measured, source-selected inputs for a native SDK link proof.
///
/// This closed selection is an input claim for local consistency checks, not
/// authority to publish, a signature, or evidence that a compiler ran.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeSdkLinkSelection {
    schema: String,
    source_root: String,
    cmake_build_root: String,
    compiler_root: String,
    output_root: String,
    source_profile: String,
    source_tree_sha256: Sha256Digest,
    contract_path_relative: String,
    contract_sha256: Sha256Digest,
    consumer_schema: String,
    consumer_profile: String,
    abi: NativeBuildAbi,
    exec_smp: bool,
    input_paths: Vec<String>,
    sdk_include_relative: String,
    binding: ConsumerBinding,
    fixtures: BTreeMap<String, SelectedFixture>,
    libraries: BTreeMap<String, Vec<String>>,
    compiler: ArosCompilerIdentity,
    compiler_tree_sha256: Sha256Digest,
    tools_layout_sha256: Sha256Digest,
    programs: BTreeMap<String, SelectedProgram>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectedFixture {
    path: String,
    sha256: Sha256Digest,
    size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectedProgram {
    path: String,
    sha256: Sha256Digest,
    size: u64,
}

/// Independent runtime/source expectations for rootless SDK-proof read-back.
pub(super) struct NativeSdkPortableExpectations<'a> {
    pub(super) source_tree_sha256: &'a Sha256Digest,
    pub(super) engine_api_version: u32,
    pub(super) engine_sha256: &'a Sha256Digest,
    pub(super) helpers: &'a BTreeMap<String, CompatibilityHelperReport>,
    pub(super) compiler: &'a ArosCompilerIdentity,
    pub(super) source_profile: &'a str,
}

/// Opaque local-consistency result for portable SDK link evidence.
///
/// It neither authenticates the original process nor establishes that the
/// source SDK tree had the bytes asserted by its inventory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeSdkPortableProof {
    receipt_sha256: Sha256Digest,
    inventory_sha256: Sha256Digest,
    entries: usize,
}

impl NativeSdkPortableProof {
    /// Digest of the exact canonical local link receipt bytes.
    #[must_use]
    pub const fn receipt_sha256(&self) -> &Sha256Digest {
        &self.receipt_sha256
    }

    /// Digest of the complete included SDK inventory, not SDK archive bytes.
    #[must_use]
    pub const fn inventory_sha256(&self) -> &Sha256Digest {
        &self.inventory_sha256
    }

    /// Number of entries in the included complete SDK inventory.
    #[must_use]
    pub const fn entries(&self) -> usize {
        self.entries
    }
}

/// Measure source-bound SDK inputs before any compatibility phase executes.
///
/// Existing roots are measured with bounded no-follow CAS; the CMake build and
/// proof roots may still be absent, so their already-selected absolute lexical
/// spellings are retained without opening or canonicalizing them.
pub(super) fn select(
    request: &NativeSdkLinkRequest,
) -> Result<NativeSdkLinkSelection, ContractError> {
    if request.timeout.is_zero() {
        return Err(error("native SDK selection requires a positive deadline"));
    }
    let source_root = super::checked_directory(
        &request.preparation.source_root,
        "native SDK selection source root",
    )?;
    if source_root != request.preparation.source_root {
        return Err(error("native SDK source root selection is not canonical"));
    }
    let compiler_root = super::checked_directory(&request.compiler_root, "native SDK compiler")?;
    if compiler_root != request.compiler_root {
        return Err(error("native SDK compiler root selection is not canonical"));
    }
    let source_root_text = absolute_lexical_text(&source_root)?;
    let cmake_build_root = absolute_lexical_text(&request.cmake_build_root)?;
    let compiler_root_text = absolute_lexical_text(&compiler_root)?;
    let output_root = absolute_lexical_text(&request.output_root)?;
    ensure_disjoint_roots(
        &source_root,
        &PathBuf::from(&cmake_build_root),
        &compiler_root,
        &PathBuf::from(&output_root),
    )?;

    let source_before = measure_tree_content_cas_bounded(&source_root, PACKAGE_LIMITS)
        .map_err(|_| error("cannot measure bounded native SDK source selection"))?;
    let source_tree_sha256 = source_before.payload_digest_excluding(None);
    if source_tree_sha256 != request.preparation.source_tree_sha256 {
        return Err(error(
            "native SDK source selection differs from the prepared source tree",
        ));
    }

    let contract_path_relative = request
        .source_profile
        .native_consumer_contract
        .as_deref()
        .ok_or_else(|| error("native SDK selection requires a source consumer contract"))?;
    validate_source_relative(contract_path_relative)?;
    let loaded = load_bound_native_consumer_contract(
        &source_root,
        Path::new(contract_path_relative),
        &request.source_profile,
    )
    .map_err(|failure| error(format!("native SDK selection source contract: {failure}")))?;
    let probes = loaded
        .contract
        .require_native_sdk_link_probes()
        .map_err(|failure| error(format!("native SDK selection source probes: {failure}")))?;
    aros_common::native_consumer_contract::validate_native_consumer_compiler(
        &loaded.contract,
        &request.compiler,
        &loaded.contract.abi.target_triple,
    )
    .map_err(|failure| error(format!("native SDK selection compiler binding: {failure}")))?;

    let binding = native_sdk_links::source_binding(
        request,
        &loaded,
        &aros_common::CancellationToken::default(),
    )?;
    let expected_smp = loaded
        .contract
        .generated_make_templates
        .values()
        .filter_map(|item| item.substitutions.get("@ENABLE_EXECSMP@"))
        .any(|value| value == "#define __AROSEXEC_SMP__");
    let input_paths = loaded
        .contract
        .inputs
        .iter()
        .map(|input| input.path.clone())
        .collect::<Vec<_>>();
    validate_binding_semantics(
        &binding,
        BindingExpectations {
            source_root: &source_root_text,
            contract_path_relative,
            contract_sha256: &loaded.sha256,
            profile: &loaded.contract.profile,
            abi: &loaded.contract.abi,
            exec_smp: expected_smp,
            input_paths: &input_paths,
        },
    )?;

    let compiler_before = measure_tree_content_cas_bounded(&compiler_root, PACKAGE_LIMITS)
        .map_err(|_| error("cannot measure bounded native SDK compiler selection"))?;
    let layout = ToolchainToolLayout::load(&compiler_root).map_err(error)?;
    layout
        .validate_binding(&request.compiler, &loaded.contract.abi.target_triple)
        .map_err(error)?;
    let tools = layout
        .resolve_tools(&compiler_root)
        .map_err(error)?
        .into_iter()
        .collect::<BTreeMap<_, _>>();
    let mut programs = BTreeMap::new();
    for role in ["c", "cxx"] {
        let path = tools
            .get(role)
            .ok_or_else(|| error("native SDK compiler layout omits a declared language role"))?;
        let relative = path
            .strip_prefix(&compiler_root)
            .map_err(|_| error("native SDK compiler role escapes its selected root"))?;
        let relative = relative
            .to_str()
            .ok_or_else(|| error("native SDK compiler role path is not UTF-8"))?;
        validate_source_relative(relative)?;
        let resolved = path
            .canonicalize()
            .map_err(|_| error("cannot resolve selected native SDK compiler role"))?;
        if !resolved.starts_with(&compiler_root) {
            return Err(error(
                "native SDK compiler role resolves outside its payload",
            ));
        }
        let (_, measured) = measure_regular_file_digest_bounded(&resolved, MAX_DRIVER_BYTES)
            .map_err(|_| error("cannot measure bounded native SDK compiler role"))?;
        if measured.size == 0 {
            return Err(error("native SDK compiler role is empty"));
        }
        programs.insert(
            role.to_owned(),
            SelectedProgram {
                path: relative.to_owned(),
                sha256: measured.digest,
                size: measured.size,
            },
        );
    }

    let fixtures = BTreeMap::from([
        ("c".into(), measure_fixture(&source_root, &probes.c)?),
        ("cxx".into(), measure_fixture(&source_root, &probes.cxx)?),
    ]);
    let libraries = BTreeMap::from([
        ("c".into(), probes.c.libraries.clone()),
        ("cxx".into(), probes.cxx.libraries.clone()),
    ]);
    let selection = NativeSdkLinkSelection {
        schema: SELECTION_SCHEMA.into(),
        source_root: source_root_text,
        cmake_build_root,
        compiler_root: compiler_root_text,
        output_root,
        source_profile: request.source_profile.name.clone(),
        source_tree_sha256,
        contract_path_relative: contract_path_relative.into(),
        contract_sha256: loaded.sha256,
        consumer_schema: loaded.contract.schema.clone(),
        consumer_profile: loaded.contract.profile.clone(),
        abi: loaded.contract.abi.clone(),
        exec_smp: expected_smp,
        input_paths: loaded
            .contract
            .inputs
            .iter()
            .map(|input| input.path.clone())
            .collect(),
        sdk_include_relative: binding.sdk_include_relative.clone(),
        binding,
        fixtures,
        libraries,
        compiler: request.compiler.clone(),
        compiler_tree_sha256: compiler_before.payload_digest_excluding(None),
        tools_layout_sha256: layout.sha256().clone(),
        programs,
    };
    selection.validate()?;

    if measure_tree_content_cas_bounded(&source_root, PACKAGE_LIMITS)
        .map_err(|_| error("cannot remeasure bounded native SDK source selection"))?
        != source_before
        || measure_tree_content_cas_bounded(&compiler_root, PACKAGE_LIMITS)
            .map_err(|_| error("cannot remeasure bounded native SDK compiler selection"))?
            != compiler_before
    {
        return Err(error(
            "native SDK selection inputs changed during measurement",
        ));
    }
    revalidate_prepared_helpers(request)?;
    Ok(selection)
}

impl NativeSdkLinkSelection {
    /// Recheck closed selection claims without opening any recorded root.
    pub(super) fn validate(&self) -> Result<(), ContractError> {
        if self.schema != SELECTION_SCHEMA
            || self.consumer_schema != "aros-native-consumer-contract-v2"
            || !valid_portable_token(&self.source_profile)
            || self.consumer_profile != self.source_profile
            || self.binding.profile != self.consumer_profile
            || self.binding.sdk_include_relative != self.sdk_include_relative
        {
            return Err(error("native SDK selection schema or profile is invalid"));
        }
        for path in [
            &self.source_root,
            &self.cmake_build_root,
            &self.compiler_root,
            &self.output_root,
        ] {
            validate_absolute_lexical(path)?;
        }
        ensure_disjoint_roots(
            Path::new(&self.source_root),
            Path::new(&self.cmake_build_root),
            Path::new(&self.compiler_root),
            Path::new(&self.output_root),
        )?;
        validate_source_relative(&self.contract_path_relative)?;
        validate_source_relative(&self.sdk_include_relative)?;
        if self.binding.source_dir != self.source_root
            || self.binding.schema != "aros-native-consumer-validation-v1"
            || self.binding.qualification != "source-binding-not-graph-or-build-proof"
            || self.binding.contract_path
                != joined_path(&self.source_root, &self.contract_path_relative)?
            || self.binding.contract_sha256 != self.contract_sha256
            || self.binding.abi != self.abi
            || self.binding.exec_smp != self.exec_smp
            || self.binding.input_paths != self.input_paths
        {
            return Err(error(
                "native SDK selection binding differs from its source claims",
            ));
        }
        if self.abi.source_cpu.is_empty()
            || self.abi.target_triple.is_empty()
            || self.abi.isa.is_empty()
            || self.abi.abi.is_empty()
            || self.abi.code_model.is_empty()
            || self.abi.flavour.is_empty()
        {
            return Err(error("native SDK selection ABI is structurally incomplete"));
        }
        if !valid_portable_token(&self.abi.source_cpu)
            || !valid_portable_token(&self.abi.target_triple)
            || !valid_portable_token(&self.abi.isa)
            || !valid_portable_token(&self.abi.abi)
            || !valid_portable_token(&self.abi.code_model)
            || !valid_portable_token(&self.abi.flavour)
            || self.abi.target_triple != format!("{}-aros", self.abi.source_cpu)
        {
            return Err(error(
                "native SDK selection ABI contains non-portable values",
            ));
        }
        validate_binding_include(&self.sdk_include_relative)?;
        if self.input_paths.is_empty() || self.input_paths.len() > 128 {
            return Err(error("native SDK selection input path set is invalid"));
        }
        for path in &self.input_paths {
            validate_source_relative(path)?;
        }
        if self.input_paths.iter().collect::<BTreeSet<_>>().len() != self.input_paths.len() {
            return Err(error("native SDK selection input paths are not unique"));
        }
        if self
            .fixtures
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
            != BTreeSet::from(["c", "cxx"])
            || self
                .libraries
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>()
                != BTreeSet::from(["c", "cxx"])
            || self
                .programs
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>()
                != BTreeSet::from(["c", "cxx"])
        {
            return Err(error(
                "native SDK selection omits a C or C++ language input",
            ));
        }
        for (language, fixture) in &self.fixtures {
            validate_source_relative(&fixture.path)?;
            if fixture.size == 0
                || fixture.size > MAX_SOURCE_BYTES
                || !self.input_paths.contains(&fixture.path)
            {
                return Err(error("native SDK fixture selection has an invalid size"));
            }
            if (language == "c" && Path::new(&fixture.path).extension() != Some(OsStr::new("c")))
                || (language == "cxx"
                    && Path::new(&fixture.path).extension() != Some(OsStr::new("cpp")))
            {
                return Err(error(
                    "native SDK fixture extension differs from its language",
                ));
            }
        }
        for libraries in self.libraries.values() {
            validate_libraries(libraries)?;
        }
        for program in self.programs.values() {
            validate_source_relative(&program.path)?;
            if program.size == 0 || program.size > MAX_DRIVER_BYTES {
                return Err(error(
                    "native SDK compiler-role selection has an invalid size",
                ));
            }
        }
        if !matches!(self.compiler, ArosCompilerIdentity::Gnu { .. }) {
            return Err(error("native SDK selection requires a GNU compiler"));
        }
        self.compiler
            .validate_for_target(&self.abi.target_triple)
            .map_err(|_| error("native SDK compiler identity does not validate its target"))?;
        let ArosCompilerIdentity::Gnu { target, .. } = &self.compiler else {
            return Err(error("native SDK selection requires a GNU compiler"));
        };
        if target.isa() != self.abi.isa
            || target.abi() != self.abi.abi
            || target.code_model() != self.abi.code_model
        {
            return Err(error("native SDK selection compiler and ABI differ"));
        }
        Ok(())
    }
}

fn measure_fixture(
    source_root: &Path,
    probe: &NativeSdkLinkProbe,
) -> Result<SelectedFixture, ContractError> {
    validate_source_relative(&probe.source)?;
    let path = source_root.join(&probe.source);
    let (_, measured) = measure_regular_file_digest_bounded(&path, MAX_SOURCE_BYTES)
        .map_err(|_| error("cannot measure bounded source-sealed SDK fixture"))?;
    if measured.size == 0 {
        return Err(error("source-sealed SDK fixture is empty"));
    }
    Ok(SelectedFixture {
        path: probe.source.clone(),
        sha256: measured.digest,
        size: measured.size,
    })
}

fn revalidate_prepared_helpers(request: &NativeSdkLinkRequest) -> Result<(), ContractError> {
    let expected = super::REQUIRED_HELPERS
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<BTreeSet<_>>();
    if request
        .preparation
        .helpers
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>()
        != expected
    {
        return Err(error("native SDK preparation has an incomplete helper set"));
    }
    for (name, helper) in &request.preparation.helpers {
        let path = super::checked_executable(&helper.path)?;
        let (sha256, size) = super::measure_executable(&path)?;
        if path.file_name().and_then(|part| part.to_str()) != Some(name.as_str())
            || sha256 != helper.sha256
            || size != helper.size
        {
            return Err(error(
                "native SDK preparation helper changed after selection",
            ));
        }
    }
    Ok(())
}

/// Exact flat portable SDK evidence namespace expected by this verifier.
pub(super) fn expected_member_names() -> BTreeSet<String> {
    let mut names = BTreeSet::from([
        BINDING_MEMBER.to_owned(),
        RECEIPT_MEMBER.to_owned(),
        INVENTORY_MEMBER.to_owned(),
    ]);
    for label in ["original", "relocated"] {
        for language in ["c", "cxx"] {
            names.insert(format!("sdk-{label}-{language}.elf"));
            names.insert(format!("sdk-{label}-{language}.map"));
        }
    }
    for language in ["c", "cxx"] {
        names.insert(format!("sdk-standalone-{language}.report.json"));
        for command in [1, 2] {
            for stream in ["stdout", "stderr"] {
                names.insert(format!("sdk-standalone-{language}.{command}.{stream}.log"));
            }
        }
    }
    names
}

/// Verify a flat portable SDK proof without reopening any path in the proof.
///
/// This is a local-consistency result only: the receipt is not authentication,
/// maps/traces do not prove an SDK archive's bytes, and the included inventory
/// is not a source of SDK-tree authority.
pub(super) fn readback<B: AsRef<[u8]>>(
    files: &BTreeMap<String, B>,
    selected: &NativeSdkLinkSelection,
    expected: &NativeSdkPortableExpectations<'_>,
) -> Result<NativeSdkPortableProof, ContractError> {
    selected.validate()?;
    if selected.source_tree_sha256 != *expected.source_tree_sha256
        || selected.compiler != *expected.compiler
        || selected.source_profile != expected.source_profile
    {
        return Err(error(
            "portable native SDK selection differs from independent source/compiler expectations",
        ));
    }
    let names = expected_member_names();
    if files.len() != names.len() || files.keys().ne(names.iter()) {
        return Err(error(
            "portable native SDK proof differs from its exact file set",
        ));
    }
    for (name, bytes) in files {
        if !name.starts_with("sdk-") || bytes.as_ref().len() > member_limit(name)? {
            return Err(error(
                "portable native SDK member exceeds its type-specific bound",
            ));
        }
    }

    let binding_bytes = member(files, BINDING_MEMBER)?;
    let binding: ConsumerBinding = serde_json::from_slice(binding_bytes)
        .map_err(|_| error("portable native SDK source binding is not closed JSON"))?;
    if binding != selected.binding {
        return Err(error(
            "portable native SDK source binding differs from selection",
        ));
    }
    let canonical_binding = crate::canonical::bytes(
        &serde_json::to_value(&binding)
            .map_err(|_| error("cannot encode portable native SDK source binding"))?,
    )?;
    if canonical_binding != binding_bytes {
        return Err(error(
            "portable native SDK source binding is not canonical JSON",
        ));
    }
    let binding_sha256 = sha256_bytes(binding_bytes);

    let inventory_bytes = member(files, INVENTORY_MEMBER)?;
    let inventory: BoundedInventory = serde_json::from_slice(inventory_bytes).map_err(|_| {
        error("portable native SDK inventory is not closed or exceeds its count bound")
    })?;
    let inventory = inventory.0;
    let canonical_inventory = super::native_sdk_relocation::encode_inventory(&inventory)?;
    if canonical_inventory != inventory_bytes {
        return Err(error(
            "portable native SDK inventory encoding is not canonical",
        ));
    }
    let inventory_index = validate_inventory(&inventory)?;
    if inventory_index
        .get("include")
        .is_none_or(|entry| entry.kind != "directory")
    {
        return Err(error(
            "portable native SDK inventory omits its source-selected include directory",
        ));
    }
    let inventory_sha256 = Sha256Digest::parse(
        &toolchain_inventory_sha256(&inventory)
            .map_err(|_| error("cannot hash portable native SDK inventory"))?,
    )
    .map_err(|_| error("portable native SDK inventory digest is invalid"))?;

    let receipt_bytes = member(files, RECEIPT_MEMBER)?;
    let receipt: Receipt = serde_json::from_slice(receipt_bytes)
        .map_err(|_| error("portable native SDK receipt is not closed JSON"))?;
    let canonical_receipt = crate::canonical::bytes(
        &serde_json::to_value(&receipt)
            .map_err(|_| error("cannot encode portable native SDK receipt"))?,
    )?;
    if canonical_receipt != receipt_bytes {
        return Err(error("portable native SDK receipt is not canonical JSON"));
    }
    let receipt_sha256 = sha256_bytes(receipt_bytes);

    let original_sdk = sdk_root_from_binding(selected)?;
    let relocated_sdk = PathBuf::from(&selected.output_root).join("relocated-sdk");
    let temporary = PathBuf::from(&selected.output_root).join("tmp");
    let expected_receipt = FileIdentity {
        sha256: sha256_bytes(inventory_bytes),
        size: u64::try_from(inventory_bytes.len())
            .map_err(|_| error("portable native SDK inventory length overflowed"))?,
    };
    if receipt.schema != "aros-native-sdk-link-receipt-v1"
        || receipt.qualification != "local-links-not-release-admission"
        || receipt.source_tree_sha256 != selected.source_tree_sha256
        || receipt.contract_sha256 != selected.contract_sha256
        || receipt.binding_sha256 != binding_sha256
        || receipt.profile != selected.consumer_profile
        || receipt.compiler != selected.compiler
        || receipt.tools_layout_sha256 != selected.tools_layout_sha256
        || receipt.compiler_tree_sha256 != selected.compiler_tree_sha256
        || receipt.sdk_inventory_sha256 != inventory_sha256
        || receipt.sdk_entries != inventory.len()
        || receipt.inventory != expected_receipt
    {
        return Err(error(
            "portable native SDK receipt differs from the selected identities",
        ));
    }
    validate_receipt_shapes(&receipt, selected)?;

    let environment = sdk_environment(&temporary)?;
    let environment = super::environment::resolve(&environment)?;
    let environment_sha256 = super::environment::identity(&environment)?;
    for (language, phase, phase_name) in [
        ("c", CompatibilityPhase::StandaloneC, "standalone-c"),
        ("cxx", CompatibilityPhase::StandaloneCxx, "standalone-cxx"),
    ] {
        let report_name = format!("sdk-{phase_name}.report.json");
        let report_bytes = member(files, &report_name)?;
        let report = CompatibilityProbeReport::parse(report_bytes)?;
        let canonical_report = crate::canonical::bytes(
            &serde_json::to_value(&report)
                .map_err(|_| error("cannot encode portable native SDK phase report"))?,
        )?;
        if canonical_report != report_bytes
            || report.phase != phase
            || report.commands.len() != 2
            || report.engine_api_version != expected.engine_api_version
            || report.engine_sha256 != *expected.engine_sha256
            || report.source_tree_sha256 != *expected.source_tree_sha256
            || report.helpers != *expected.helpers
            || !report.host_tools.is_empty()
            || report.environment_sha256 != environment_sha256
        {
            return Err(error(
                "portable native SDK phase report differs from selected process inputs",
            ));
        }
        if receipt.reports.get(language) != Some(&sha256_bytes(report_bytes)) {
            return Err(error(
                "portable native SDK receipt omits its exact phase report",
            ));
        }
        let probe = selected_probe(selected, language)?;
        let fixture = selected
            .fixtures
            .get(language)
            .ok_or_else(|| error("portable native SDK fixture selection is absent"))?;
        let fixture_path = PathBuf::from(&selected.source_root).join(&fixture.path);
        let program = selected
            .programs
            .get(language)
            .ok_or_else(|| error("portable native SDK compiler role is absent"))?;
        let program_path = PathBuf::from(&selected.compiler_root).join(&program.path);
        for (index, (label, sdk)) in [
            ("original", original_sdk.as_path()),
            ("relocated", relocated_sdk.as_path()),
        ]
        .into_iter()
        .enumerate()
        {
            let command = native_sdk_links::application_command(
                &program_path,
                &fixture_path,
                &probe,
                &selected.compiler,
                sdk,
                Path::new(&selected.output_root),
                (language, label),
            )?;
            if command.arguments.iter().any(|argument| {
                matches!(
                    argument.as_str(),
                    "-c" | "-r" | "-nostdlib" | "-nostartfiles" | "-nodefaultlibs"
                ) || argument.starts_with("-Wl,-r")
            }) {
                return Err(error(
                    "portable native SDK command is not an ordinary driver link",
                ));
            }
            let command_sha256 = super::command_identity(&command.program, &command.arguments)?;
            let command_report = &report.commands[index];
            if command_report.program_sha256 != program.sha256
                || command_report.command_sha256 != command_sha256
            {
                return Err(error(
                    "portable native SDK command differs from exact source-declared defaults",
                ));
            }
            let output_name = format!("{label}-{language}");
            let elf_bytes = member(files, &format!("sdk-{output_name}.elf"))?;
            native_sdk_links::verify_application_elf(elf_bytes, &selected.compiler)?;
            if receipt.outputs.get(&output_name) != Some(&identity(elf_bytes)) {
                return Err(error(
                    "portable native SDK receipt differs from its application ELF",
                ));
            }
            if label == "original" {
                let relocated_bytes = member(files, &format!("sdk-relocated-{language}.elf"))?;
                if elf_bytes != relocated_bytes {
                    return Err(error(
                        "portable application ELF differs after SDK relocation",
                    ));
                }
            }

            let map_bytes = member(files, &format!("sdk-{output_name}.map"))?;
            if receipt.maps.get(&output_name) != Some(&identity(map_bytes)) {
                return Err(error(
                    "portable native SDK receipt differs from its link map",
                ));
            }
            validate_trace_and_map(
                files,
                &report,
                TraceExpectations {
                    command_index: index,
                    language,
                    map_bytes,
                    sdk,
                    original_sdk: &original_sdk,
                    temporary: &temporary,
                    compiler_root: &selected.compiler_root,
                    inventory: &inventory_index,
                    libraries: &probe.libraries,
                    relocated: label == "relocated",
                },
            )?;
        }
    }

    Ok(NativeSdkPortableProof {
        receipt_sha256,
        inventory_sha256,
        entries: inventory.len(),
    })
}

fn sdk_environment(temporary: &Path) -> Result<CompatibilityEnvironment, ContractError> {
    let temporary = absolute_lexical_text(temporary)?;
    Ok(CompatibilityEnvironment::Poisoned {
        variables: BTreeMap::from([
            ("PATH".into(), "/nonexistent".into()),
            ("LC_ALL".into(), "C".into()),
            ("LANG".into(), "C".into()),
            ("TMPDIR".into(), temporary),
        ]),
    })
}

fn selected_probe(
    selected: &NativeSdkLinkSelection,
    language: &str,
) -> Result<NativeSdkLinkProbe, ContractError> {
    let fixture = selected
        .fixtures
        .get(language)
        .ok_or_else(|| error("portable native SDK fixture selection is absent"))?;
    let libraries = selected
        .libraries
        .get(language)
        .ok_or_else(|| error("portable native SDK library selection is absent"))?;
    Ok(NativeSdkLinkProbe {
        source: fixture.path.clone(),
        libraries: libraries.clone(),
    })
}

fn validate_receipt_shapes(
    receipt: &Receipt,
    selected: &NativeSdkLinkSelection,
) -> Result<(), ContractError> {
    let expected_fixtures = selected
        .fixtures
        .iter()
        .map(|(language, selected)| {
            (
                language.clone(),
                FileIdentity {
                    sha256: selected.sha256.clone(),
                    size: selected.size,
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    let expected_outputs = ["original-c", "relocated-c", "original-cxx", "relocated-cxx"]
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    let expected_reports = BTreeSet::from(["c".to_owned(), "cxx".to_owned()]);
    if receipt.fixtures != expected_fixtures
        || receipt.outputs.keys().cloned().collect::<BTreeSet<_>>() != expected_outputs
        || receipt.maps.keys().cloned().collect::<BTreeSet<_>>() != expected_outputs
        || receipt.reports.keys().cloned().collect::<BTreeSet<_>>() != expected_reports
    {
        return Err(error(
            "portable native SDK receipt has an incomplete proof shape",
        ));
    }
    Ok(())
}

struct BoundedInventory(Vec<ArosToolchainManifestEntry>);

impl<'de> Deserialize<'de> for BoundedInventory {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct InventoryVisitor;

        impl<'de> Visitor<'de> for InventoryVisitor {
            type Value = BoundedInventory;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a bounded array of SDK inventory entries")
            }

            fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let capacity = sequence.size_hint().unwrap_or_default().min(4096);
                let mut entries = Vec::with_capacity(capacity);
                while let Some(entry) = sequence.next_element::<ArosToolchainManifestEntry>()? {
                    if entries.len() >= MAX_INVENTORY_ENTRIES {
                        return Err(de::Error::custom("SDK inventory entry limit exceeded"));
                    }
                    if entry.path.len() > MAX_INVENTORY_PATH_BYTES {
                        return Err(de::Error::custom("SDK inventory path limit exceeded"));
                    }
                    entries.push(entry);
                }
                Ok(BoundedInventory(entries))
            }
        }

        deserializer.deserialize_seq(InventoryVisitor)
    }
}

#[derive(Clone, Copy)]
struct TraceExpectations<'a> {
    command_index: usize,
    language: &'a str,
    map_bytes: &'a [u8],
    sdk: &'a Path,
    original_sdk: &'a Path,
    temporary: &'a Path,
    compiler_root: &'a str,
    inventory: &'a BTreeMap<String, &'a ArosToolchainManifestEntry>,
    libraries: &'a [String],
    relocated: bool,
}

fn validate_trace_and_map<B: AsRef<[u8]>>(
    files: &BTreeMap<String, B>,
    report: &CompatibilityProbeReport,
    expected: TraceExpectations<'_>,
) -> Result<(), ContractError> {
    let TraceExpectations {
        command_index,
        language,
        map_bytes,
        sdk,
        original_sdk,
        temporary,
        compiler_root,
        inventory,
        libraries,
        relocated,
    } = expected;
    if map_bytes.is_empty() {
        return Err(error("portable native SDK link map is empty"));
    }
    let map = std::str::from_utf8(map_bytes)
        .map_err(|_| error("portable native SDK link map is not UTF-8"))?;
    let original_root_text = absolute_lexical_text(original_sdk)?;
    if relocated && map.contains(&original_root_text) {
        return Err(error(
            "relocated native SDK map retains its original SDK root",
        ));
    }

    let stdout_name = format!("sdk-standalone-{language}.{}.stdout.log", command_index + 1);
    let stderr_name = format!("sdk-standalone-{language}.{}.stderr.log", command_index + 1);
    let stdout = member(files, &stdout_name)?;
    let stderr = member(files, &stderr_name)?;
    let command = report
        .commands
        .get(command_index)
        .ok_or_else(|| error("portable native SDK phase command is absent"))?;
    if sha256_bytes(stdout) != command.stdout_sha256
        || sha256_bytes(stderr) != command.stderr_sha256
    {
        return Err(error(
            "portable native SDK retained logs differ from their report",
        ));
    }
    if has_truncation_marker(stdout) || has_truncation_marker(stderr) {
        return Err(error(
            "portable native SDK process log is truncated and cannot prove its inputs",
        ));
    }
    let trace = std::str::from_utf8(stdout)
        .map_err(|_| error("portable native SDK linker trace is not UTF-8"))?;
    let original_root_text = absolute_lexical_text(original_sdk)?;
    if relocated && trace.contains(&original_root_text) {
        return Err(error(
            "relocated native SDK trace retains its original SDK root",
        ));
    }
    let sdk_text = absolute_lexical_text(sdk)?;
    let temporary_text = absolute_lexical_text(temporary)?;
    let mut saw_sdk_archive = false;
    let mut sdk_names = BTreeSet::new();
    let mut lines = 0_usize;
    for line in trace.lines().map(str::trim).filter(|line| !line.is_empty()) {
        lines = lines
            .checked_add(1)
            .ok_or_else(|| error("portable native SDK trace line count overflowed"))?;
        let path_text = normalize_trace_path(trace_input_path(line)?)?;
        if relocated && path_text.contains(&original_root_text) {
            return Err(error(
                "relocated native SDK trace retains its original SDK root",
            ));
        }
        if let Some(relative) = path_below(&path_text, &temporary_text) {
            validate_driver_temp_relative(relative)?;
            continue;
        }
        if let Some(relative) = path_below(&path_text, &sdk_text) {
            validate_inventory_relative(relative)?;
            let resolved = resolve_inventory_path(relative, inventory)?;
            let final_entry = inventory
                .get(&resolved)
                .copied()
                .ok_or_else(|| error("portable SDK trace input is absent from its inventory"))?;
            if final_entry.kind != "file" {
                return Err(error("portable SDK trace input is not a regular SDK file"));
            }
            if Path::new(&resolved).extension() == Some(OsStr::new("a")) {
                saw_sdk_archive = true;
                if let Some(name) = Path::new(relative)
                    .file_name()
                    .and_then(|name| name.to_str())
                {
                    sdk_names.insert(name.to_owned());
                }
            }
            continue;
        }
        if let Some(relative) = path_below(&path_text, compiler_root) {
            validate_source_relative(relative)?;
            continue;
        }
        return Err(error(
            "portable native SDK trace contains an input outside selected SDK/compiler/tmp roots",
        ));
    }
    if lines == 0 || !saw_sdk_archive {
        return Err(error(
            "portable native SDK link lacks a traced SDK archive or input closure",
        ));
    }
    for library in libraries {
        if !sdk_names.contains(&format!("lib{library}.a")) {
            return Err(error(
                "portable source-declared library did not trace from the selected SDK",
            ));
        }
    }
    // Keep the proof's phase identity in the call signature explicit: the
    // selected report is already checked against its C/C++ phase by readback.
    let expected_phase = if language == "c" {
        CompatibilityPhase::StandaloneC
    } else {
        CompatibilityPhase::StandaloneCxx
    };
    if report.phase != expected_phase {
        return Err(error(
            "portable native SDK trace is attached to the wrong phase",
        ));
    }
    Ok(())
}

fn validate_inventory(
    entries: &[ArosToolchainManifestEntry],
) -> Result<BTreeMap<String, &ArosToolchainManifestEntry>, ContractError> {
    if entries.len() > MAX_INVENTORY_ENTRIES {
        return Err(error(
            "portable native SDK inventory exceeds its entry limit",
        ));
    }
    let mut index = BTreeMap::new();
    let mut previous: Option<&str> = None;
    let mut file_bytes = 0_u64;
    for entry in entries {
        validate_inventory_relative(&entry.path)?;
        if previous.is_some_and(|path| path >= entry.path.as_str()) {
            return Err(error(
                "portable native SDK inventory is unsorted or contains duplicate paths",
            ));
        }
        previous = Some(&entry.path);
        let depth = entry.path.split('/').count();
        if depth > MAX_INVENTORY_DEPTH || entry.path.len() > MAX_INVENTORY_PATH_BYTES {
            return Err(error(
                "portable native SDK inventory path exceeds its bound",
            ));
        }
        if index.insert(entry.path.clone(), entry).is_some() {
            return Err(error("portable native SDK inventory has duplicate paths"));
        }
        match entry.kind.as_str() {
            "directory" => {
                if entry.mode != "0755"
                    || entry.sha256.is_some()
                    || entry.size.is_some()
                    || entry.target.is_some()
                {
                    return Err(error("portable SDK inventory directory fields are invalid"));
                }
            }
            "file" => {
                if !matches!(entry.mode.as_str(), "0644" | "0755")
                    || entry.target.is_some()
                    || entry.sha256.is_none()
                    || entry.size.is_none()
                {
                    return Err(error("portable SDK inventory file fields are invalid"));
                }
                Sha256Digest::parse(entry.sha256.as_deref().unwrap_or_default())
                    .map_err(|_| error("portable SDK inventory file digest is invalid"))?;
                file_bytes = file_bytes
                    .checked_add(entry.size.unwrap_or_default())
                    .ok_or_else(|| error("portable SDK inventory file-byte count overflowed"))?;
                if file_bytes > MAX_INVENTORY_FILE_BYTES {
                    return Err(error("portable SDK inventory exceeds its file-byte limit"));
                }
            }
            "symlink" => {
                if entry.mode != "0777"
                    || entry.sha256.is_some()
                    || entry.size.is_some()
                    || entry.target.is_none()
                {
                    return Err(error(
                        "portable SDK inventory symbolic-link fields are invalid",
                    ));
                }
                let target = entry.target.as_deref().unwrap_or_default();
                validate_symlink_target(target)?;
            }
            _ => {
                return Err(error(
                    "portable SDK inventory contains an unknown entry type",
                ))
            }
        }
    }
    for entry in entries {
        if let Some(parent) = Path::new(&entry.path).parent() {
            if !parent.as_os_str().is_empty() {
                let parent = parent
                    .to_str()
                    .ok_or_else(|| error("portable SDK inventory parent is not UTF-8"))?;
                let parent_entry = index
                    .get(parent)
                    .ok_or_else(|| error("portable SDK inventory omits a parent directory"))?;
                if parent_entry.kind != "directory" {
                    return Err(error("portable SDK inventory parent is not a directory"));
                }
            }
        }
    }
    for entry in entries.iter().filter(|entry| entry.kind == "symlink") {
        let parent = Path::new(&entry.path)
            .parent()
            .and_then(Path::to_str)
            .unwrap_or_default();
        let target = entry.target.as_deref().unwrap_or_default();
        let _ = resolve_relative_components(parent, target, &index)?;
    }
    Ok(index)
}

fn resolve_inventory_path(
    relative: &str,
    index: &BTreeMap<String, &ArosToolchainManifestEntry>,
) -> Result<String, ContractError> {
    validate_inventory_relative(relative)?;
    resolve_relative_components("", relative, index)
}

fn resolve_relative_components(
    base: &str,
    target: &str,
    index: &BTreeMap<String, &ArosToolchainManifestEntry>,
) -> Result<String, ContractError> {
    if target.is_empty() || target.starts_with('/') || target.contains('\\') {
        return Err(error("portable SDK symbolic link target is unsafe"));
    }
    let mut resolved = if base.is_empty() {
        Vec::new()
    } else {
        base.split('/').map(str::to_owned).collect()
    };
    let mut pending = target
        .split('/')
        .map(str::to_owned)
        .collect::<VecDeque<_>>();
    let mut hops = 0_usize;
    while let Some(component) = pending.pop_front() {
        match component.as_str() {
            "" => return Err(error("portable SDK link target has an empty component")),
            "." => {}
            ".." => {
                if resolved.pop().is_none() {
                    return Err(error("portable SDK link escapes its inventory root"));
                }
            }
            _ => {
                if component.chars().any(char::is_control) || component.contains(':') {
                    return Err(error("portable SDK link target has an unsafe component"));
                }
                resolved.push(component);
                let candidate = resolved.join("/");
                let entry = index
                    .get(&candidate)
                    .ok_or_else(|| error("portable SDK link target is dangling"))?;
                if entry.kind == "symlink" {
                    hops += 1;
                    if hops > MAX_SYMLINK_HOPS {
                        return Err(error("portable SDK link target is cyclic or too deep"));
                    }
                    resolved.pop();
                    let parent = resolved.join("/");
                    let link_target = entry.target.as_deref().unwrap_or_default();
                    validate_symlink_target(link_target)?;
                    let mut replacement = link_target
                        .split('/')
                        .map(str::to_owned)
                        .collect::<VecDeque<_>>();
                    replacement.append(&mut pending);
                    pending = replacement;
                    if !parent.is_empty() {
                        // The symlink target is relative to the link's parent.
                        // Rebuild only the already-resolved lexical prefix.
                        resolved = parent.split('/').map(str::to_owned).collect();
                    }
                } else if !pending.is_empty() && entry.kind != "directory" {
                    return Err(error(
                        "portable SDK link target traverses a non-directory entry",
                    ));
                }
            }
        }
    }
    let result = resolved.join("/");
    if !result.is_empty() && !index.contains_key(&result) {
        return Err(error("portable SDK link target is dangling"));
    }
    Ok(result)
}

#[derive(Clone, Copy)]
struct BindingExpectations<'a> {
    source_root: &'a str,
    contract_path_relative: &'a str,
    contract_sha256: &'a Sha256Digest,
    profile: &'a str,
    abi: &'a NativeBuildAbi,
    exec_smp: bool,
    input_paths: &'a [String],
}

fn validate_binding_semantics(
    binding: &ConsumerBinding,
    expected: BindingExpectations<'_>,
) -> Result<(), ContractError> {
    let contract_path = joined_path(expected.source_root, expected.contract_path_relative)?;
    if binding.schema != "aros-native-consumer-validation-v1"
        || binding.qualification != "source-binding-not-graph-or-build-proof"
        || binding.source_dir != expected.source_root
        || binding.contract_path != contract_path
        || binding.contract_sha256 != *expected.contract_sha256
        || binding.profile != expected.profile
        || binding.abi != *expected.abi
        || binding.exec_smp != expected.exec_smp
        || binding.input_paths != expected.input_paths
    {
        return Err(error("native SDK binding differs from source selection"));
    }
    validate_binding_include(&binding.sdk_include_relative)
}

fn validate_binding_include(path: &str) -> Result<(), ContractError> {
    if path.is_empty()
        || path.len() > 4096
        || path.split('/').any(|part| {
            part.is_empty()
                || matches!(part, "." | "..")
                || !part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_.+-".contains(&byte))
        })
        || Path::new(path).file_name().and_then(|name| name.to_str()) != Some("include")
    {
        return Err(error("native SDK binding include path is unsafe"));
    }
    let parent = Path::new(path)
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| error("native SDK binding has no SDK parent"))?;
    if !parent.is_relative()
        || parent
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(error("native SDK binding SDK root is not relative"));
    }
    Ok(())
}

fn sdk_root_from_binding(selected: &NativeSdkLinkSelection) -> Result<PathBuf, ContractError> {
    let relative = Path::new(&selected.sdk_include_relative);
    let parent = relative
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| error("native SDK binding omits its root"))?;
    let root = PathBuf::from(&selected.cmake_build_root).join(parent);
    if !root.starts_with(Path::new(&selected.cmake_build_root))
        || root == Path::new(&selected.cmake_build_root)
    {
        return Err(error(
            "native SDK binding root escapes the selected CMake build",
        ));
    }
    Ok(root)
}

fn trace_input_path(line: &str) -> Result<&str, ContractError> {
    if line.contains(" bytes omitted by aros]") {
        return Err(error("portable native SDK trace is truncated"));
    }
    let path = if line.ends_with(')') {
        if let Some(open) = line.rfind('(') {
            let archive = &line[..open];
            let member = &line[open + 1..line.len() - 1];
            if Path::new(archive).extension() == Some(OsStr::new("a")) {
                if member.is_empty()
                    || member
                        .chars()
                        .any(|character| matches!(character, '(' | ')') || character.is_control())
                {
                    return Err(error(
                        "portable native SDK trace has malformed archive syntax",
                    ));
                }
                archive
            } else {
                line
            }
        } else {
            line
        }
    } else {
        line
    };
    if !path.starts_with('/') || path.chars().any(char::is_control) {
        return Err(error("portable native SDK trace path is unsafe"));
    }
    Ok(path)
}

fn normalize_trace_path(path: &str) -> Result<String, ContractError> {
    if !path.starts_with('/') || path.len() > 4096 || path.chars().any(char::is_control) {
        return Err(error(
            "portable native SDK trace path is not a bounded absolute path",
        ));
    }
    let mut components = Vec::new();
    for component in path[1..].split('/') {
        match component {
            "" | "." => {
                return Err(error(
                    "portable native SDK trace path has an empty or current component",
                ));
            }
            ".." => {
                if components.pop().is_none() {
                    return Err(error("portable native SDK trace path escapes its root"));
                }
            }
            component => components.push(component),
        }
    }
    if components.is_empty() {
        return Err(error(
            "portable native SDK trace path names a filesystem root",
        ));
    }
    Ok(format!("/{}", components.join("/")))
}

fn validate_driver_temp_relative(relative: &str) -> Result<(), ContractError> {
    if relative.contains('/') {
        return Err(error("portable compiler temporary is not a direct child"));
    }
    let path = Path::new(relative);
    let token = path
        .file_stem()
        .and_then(OsStr::to_str)
        .and_then(|value| value.strip_prefix("cc"));
    if path.extension() != Some(OsStr::new("o"))
        || !token.is_some_and(|value| {
            !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
    {
        return Err(error(
            "portable compiler temporary has an unrecognized name",
        ));
    }
    Ok(())
}

fn validate_libraries(libraries: &[String]) -> Result<(), ContractError> {
    if libraries.len() > 16 {
        return Err(error("native SDK source declares too many extra libraries"));
    }
    let mut seen = BTreeSet::new();
    for library in libraries {
        if library.is_empty()
            || library.len() > 128
            || !library
                .as_bytes()
                .first()
                .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
            || !library
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_.+-".contains(&byte))
            || !seen.insert(library)
        {
            return Err(error(
                "native SDK source library is not a unique bare identifier",
            ));
        }
    }
    Ok(())
}

fn valid_portable_token(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.+".contains(&byte))
}

fn validate_source_relative(path: &str) -> Result<(), ContractError> {
    if path.is_empty()
        || path.len() > 4096
        || path.starts_with('/')
        || path.contains('\\')
        || path.contains(':')
        || path.contains('\0')
        || path.chars().any(char::is_control)
        || path.split('/').any(|part| {
            part.is_empty()
                || matches!(part, "." | "..")
                || !part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"-_.+".contains(&byte))
        })
    {
        return Err(error("native SDK selection has an unsafe relative path"));
    }
    Ok(())
}

fn validate_inventory_relative(path: &str) -> Result<(), ContractError> {
    if path.is_empty()
        || path.len() > MAX_INVENTORY_PATH_BYTES
        || path.starts_with('/')
        || path.contains('\\')
        || path.contains(':')
        || path.contains('\0')
        || path.chars().any(char::is_control)
        || path
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
    {
        return Err(error("portable SDK inventory has an unsafe relative path"));
    }
    Ok(())
}

fn validate_symlink_target(target: &str) -> Result<(), ContractError> {
    if target.is_empty()
        || target.starts_with('/')
        || target.contains('\\')
        || target.contains('\0')
        || target.chars().any(char::is_control)
    {
        return Err(error("portable SDK symbolic-link target is unsafe"));
    }
    for component in target.split('/') {
        if component.is_empty() || component.contains(':') {
            return Err(error("portable SDK symbolic-link target is non-portable"));
        }
    }
    Ok(())
}

fn absolute_lexical_text(path: &Path) -> Result<String, ContractError> {
    let value = path
        .to_str()
        .ok_or_else(|| error("native SDK selected path is not UTF-8"))?;
    validate_absolute_lexical(value)?;
    Ok(value.to_owned())
}

fn validate_absolute_lexical(value: &str) -> Result<(), ContractError> {
    if !value.starts_with('/')
        || value.len() > 4096
        || value.contains(',')
        || value.contains('\0')
        || value.chars().any(char::is_control)
    {
        return Err(error(
            "native SDK selected root is not a bounded absolute path",
        ));
    }
    if value == "/"
        || value[1..]
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
    {
        return Err(error(
            "native SDK selected root is not a canonical lexical path",
        ));
    }
    Ok(())
}

fn ensure_disjoint_roots(
    source: &Path,
    build: &Path,
    compiler: &Path,
    output: &Path,
) -> Result<(), ContractError> {
    let roots = [source, build, compiler, output];
    for (index, left) in roots.iter().enumerate() {
        for right in roots.iter().skip(index + 1) {
            if left == right || left.starts_with(right) || right.starts_with(left) {
                return Err(error("native SDK selected roots overlap"));
            }
        }
    }
    Ok(())
}

fn joined_path(root: &str, relative: &str) -> Result<String, ContractError> {
    validate_absolute_lexical(root)?;
    validate_source_relative(relative)?;
    Path::new(root)
        .join(relative)
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| error("native SDK selected joined path is not UTF-8"))
}

fn path_below<'a>(path: &'a str, root: &str) -> Option<&'a str> {
    let prefix = format!("{}/", root.trim_end_matches('/'));
    path.strip_prefix(&prefix)
        .filter(|relative| !relative.is_empty())
}

fn identity(bytes: &[u8]) -> FileIdentity {
    FileIdentity {
        sha256: sha256_bytes(bytes),
        size: bytes.len() as u64,
    }
}

fn member_limit(name: &str) -> Result<usize, ContractError> {
    if name == BINDING_MEMBER || name == RECEIPT_MEMBER {
        Ok(crate::canonical::MAX_DOCUMENT_BYTES)
    } else if name == INVENTORY_MEMBER {
        Ok(MAX_INVENTORY_BYTES)
    } else {
        let path = Path::new(name);
        let extension = path.extension();
        if extension == Some(OsStr::new("elf")) {
            Ok(MAX_ELF_BYTES)
        } else if extension == Some(OsStr::new("map")) {
            Ok(MAX_MAP_BYTES)
        } else if extension == Some(OsStr::new("log")) {
            Ok(super::MAX_RENDERED_LOG_BYTES)
        } else if extension == Some(OsStr::new("json"))
            && path
                .file_stem()
                .is_some_and(|stem| Path::new(stem).extension() == Some(OsStr::new("report")))
        {
            Ok(crate::canonical::MAX_DOCUMENT_BYTES)
        } else {
            Err(error("portable native SDK member has an unknown name"))
        }
    }
}

fn member<'a, B: AsRef<[u8]>>(
    files: &'a BTreeMap<String, B>,
    name: &str,
) -> Result<&'a [u8], ContractError> {
    files
        .get(name)
        .map(AsRef::as_ref)
        .ok_or_else(|| error("portable native SDK proof member is absent"))
}

fn has_truncation_marker(bytes: &[u8]) -> bool {
    bytes
        .windows(b" bytes omitted by aros]".len())
        .any(|window| window == b" bytes omitted by aros]")
}

fn error(message: impl Into<String>) -> ContractError {
    ContractError::compatibility(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_entry(path: &str, kind: &str, target: Option<&str>) -> ArosToolchainManifestEntry {
        ArosToolchainManifestEntry {
            path: path.into(),
            mode: match kind {
                "directory" => "0755",
                "symlink" => "0777",
                _ => "0644",
            }
            .into(),
            kind: kind.into(),
            sha256: (kind == "file").then(|| "a".repeat(64)),
            size: (kind == "file").then_some(1),
            target: target.map(str::to_owned),
        }
    }

    #[test]
    fn portable_namespace_is_flat_exact_and_sdk_prefixed() {
        let names = expected_member_names();
        assert_eq!(names.len(), 21);
        assert!(names.iter().all(|name| name.starts_with("sdk-")));
        assert!(names.contains(BINDING_MEMBER));
        assert!(names.contains(RECEIPT_MEMBER));
        assert!(names.contains(INVENTORY_MEMBER));
    }

    #[test]
    fn inventory_paths_and_links_reject_escape_dangling_and_cycles() {
        let valid = vec![
            fixture_entry("bin", "directory", None),
            fixture_entry("bin/tool", "file", None),
            fixture_entry("link", "symlink", Some("bin/tool")),
        ];
        let index = validate_inventory(&valid).unwrap();
        assert_eq!(resolve_inventory_path("link", &index).unwrap(), "bin/tool");

        let escaping = vec![fixture_entry("link", "symlink", Some("../outside"))];
        assert!(validate_inventory(&escaping).is_err());
        let dangling = vec![fixture_entry("link", "symlink", Some("missing"))];
        assert!(validate_inventory(&dangling).is_err());
        let cyclic = vec![
            fixture_entry("a", "symlink", Some("b")),
            fixture_entry("b", "symlink", Some("a")),
        ];
        assert!(validate_inventory(&cyclic).is_err());
    }

    #[test]
    fn lexical_path_checks_never_normalize_foreign_paths() {
        assert!(validate_absolute_lexical("/foreign/root/sdk").is_ok());
        assert!(validate_absolute_lexical("/foreign/root/../sdk").is_err());
        assert!(validate_absolute_lexical("relative/sdk").is_err());
        assert!(validate_inventory_relative("lib/libfoo.a").is_ok());
        assert!(validate_inventory_relative("lib/../outside.a").is_err());
        assert_eq!(path_below("/sdk/lib/foo.a", "/sdk"), Some("lib/foo.a"));
        assert_eq!(path_below("/sdk-old/lib/foo.a", "/sdk"), None);
    }

    #[test]
    fn trace_truncation_and_temporary_aliases_are_rejected() {
        assert!(trace_input_path("/sdk/lib/foo.a").is_ok());
        assert!(trace_input_path("[12 bytes omitted by aros]").is_err());
        assert_eq!(
            trace_input_path("/sdk (build)/lib/libfoo.a(objects/foo.o)").unwrap(),
            "/sdk (build)/lib/libfoo.a"
        );
        assert_eq!(
            trace_input_path("/sdk (build)/lib/foo.o").unwrap(),
            "/sdk (build)/lib/foo.o"
        );
        assert_eq!(
            normalize_trace_path("/sdk/lib/../lib/libfoo.a").unwrap(),
            "/sdk/lib/libfoo.a"
        );
        assert!(normalize_trace_path("/../../outside.a").is_err());
        assert!(validate_driver_temp_relative("ccAb123.o").is_ok());
        assert!(validate_driver_temp_relative("../ccAb123.o").is_err());
        assert!(validate_driver_temp_relative("cc_bad.o").is_err());
    }
}
