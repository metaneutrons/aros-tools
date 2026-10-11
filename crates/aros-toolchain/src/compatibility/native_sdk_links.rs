//! Ordinary GNU application links against a source-selected native SDK.
//!
//! This is deliberately separate from the historical freestanding collector
//! probes. Neither a successful parse nor a minimal link qualifies an SDK.
//! This boundary runs the source-sealed C and C++ applications with normal
//! driver defaults twice, once after independently relocating the complete
//! SDK, and retains exact output, map, inventory and process evidence.
//! It does not authenticate a runtime/package or authorize a release.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use aros_common::native_build_contract::NativeBuildAbi;
use aros_common::native_consumer_contract::{
    load_bound_native_consumer_contract, NativeSdkLinkProbe,
};
use aros_common::toolchain_layout::ToolchainToolLayout;
use aros_common::{
    elf, measure_tree_content_cas_bounded, sha256_bytes, ArosCompilerIdentity, CancellationToken,
    Sha256Digest, TargetProfile, TreeTraversalLimits,
};
use serde::{Deserialize, Serialize};

use super::native_sdk_relocation::{encode_inventory, inventory as sdk_inventory, RelocatedSdk};
use super::{
    checked_directory, read_regular_bounded, run_probe, write_new_regular, CompatibilityCommand,
    CompatibilityEnvironment, CompatibilityPhase, CompatibilityPreparation,
    CompatibilityProbeReport, CompatibilityProbeRequest, ProbeReportPaths,
};
use crate::{canonical, ContractError};

const BINDING_FILE: &str = "aros-native-consumer-binding.json";
const RECEIPT_FILE: &str = "native-sdk-links.receipt.json";
const INVENTORY_FILE: &str = "native-sdk-inventory.json";
const MAX_INVENTORY_BYTES: usize = 16 * 1024 * 1024;
const MAX_MAP_BYTES: usize = 16 * 1024 * 1024;
const MAX_ELF_BYTES: usize = 128 * 1024 * 1024;
const PACKAGE_LIMITS: TreeTraversalLimits = TreeTraversalLimits {
    max_entries: 100_000,
    max_regular_file_bytes: 4 * 1024 * 1024 * 1024,
};

/// Exact inputs to an ordinary native SDK application-link proof.
///
/// The owning compatibility executor must independently authenticate the
/// preparation, source and package. This request grants no publication rights.
#[derive(Debug, Clone)]
pub struct NativeSdkLinkRequest {
    /// Exact measured source, embedded engine and helper selection.
    pub preparation: CompatibilityPreparation,
    /// Selected source-owned profile, not the compiler's producer profile.
    pub source_profile: TargetProfile,
    /// Completed native CMake consumer build, including its current binding.
    pub cmake_build_root: PathBuf,
    /// Independently verified installed GNU compiler payload.
    pub compiler_root: PathBuf,
    /// Source/recipe-bound compiler identity from the verified manifest.
    pub compiler: ArosCompilerIdentity,
    /// Fresh, absent, disjoint proof directory; never adopted or overwritten.
    pub output_root: PathBuf,
    /// Positive per-language deadline for both original and relocated links.
    pub timeout: Duration,
}

/// Exact persisted identity of a completed four-link native SDK proof.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeSdkLinkReport {
    /// No-clobber aggregate receipt below the proof root.
    pub receipt: PathBuf,
    /// SHA-256 of the closed canonical aggregate receipt.
    pub receipt_sha256: Sha256Digest,
    /// Complete SDK inventory digest, including directories, modes and links.
    pub sdk_inventory_sha256: Sha256Digest,
    /// Number of entries in the complete original and relocated inventories.
    pub sdk_entries: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ConsumerBinding {
    pub(super) schema: String,
    pub(super) qualification: String,
    pub(super) source_dir: String,
    pub(super) contract_path: String,
    pub(super) contract_sha256: Sha256Digest,
    pub(super) profile: String,
    pub(super) abi: NativeBuildAbi,
    pub(super) exec_smp: bool,
    pub(super) input_paths: Vec<String>,
    pub(super) sdk_include_relative: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FileIdentity {
    pub(super) sha256: Sha256Digest,
    pub(super) size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Receipt {
    pub(super) schema: String,
    pub(super) qualification: String,
    pub(super) source_tree_sha256: Sha256Digest,
    pub(super) contract_sha256: Sha256Digest,
    pub(super) binding_sha256: Sha256Digest,
    pub(super) profile: String,
    pub(super) compiler: ArosCompilerIdentity,
    pub(super) tools_layout_sha256: Sha256Digest,
    pub(super) compiler_tree_sha256: Sha256Digest,
    pub(super) sdk_inventory_sha256: Sha256Digest,
    pub(super) sdk_entries: usize,
    pub(super) inventory: FileIdentity,
    pub(super) fixtures: BTreeMap<String, FileIdentity>,
    pub(super) outputs: BTreeMap<String, FileIdentity>,
    pub(super) maps: BTreeMap<String, FileIdentity>,
    pub(super) reports: BTreeMap<String, Sha256Digest>,
}

/// Execute normal C/C++ application links against original and relocated SDKs.
///
/// All additional libraries are bounded bare identifiers from the sealed
/// source contract; shell fragments and driver-mode overrides are impossible.
/// There is no `-c`, `-r`, `-nostdlib`, `-nostartfiles` or `-nodefaultlibs`.
/// Every output must satisfy the exact GNU RISC-V/AROS ABI contract. The two
/// outputs of each language must be byte-identical. SDK and compiler trees,
/// source inputs, maps and retained logs are rechecked before a receipt exists.
///
/// # Errors
/// Returns AX0703 for a missing v2 contract, stale CMake binding, unsafe roots,
/// incomplete SDK, failed link, ABI mismatch, tree mutation or path leakage.
/// Failed outputs remain for diagnosis; the operation never retries or deletes.
pub fn execute_native_sdk_links(
    request: &NativeSdkLinkRequest,
    cancellation: &CancellationToken,
) -> Result<NativeSdkLinkReport, ContractError> {
    if request.timeout.is_zero() {
        return Err(error("native SDK links require a positive deadline"));
    }
    super::validate_preparation(&request.preparation)?;
    let source = &request.preparation.source_root;
    let relative = request
        .source_profile
        .native_consumer_contract
        .as_deref()
        .ok_or_else(|| error("native SDK links require a source consumer contract"))?;
    let loaded =
        load_bound_native_consumer_contract(source, Path::new(relative), &request.source_profile)
            .map_err(|failure| error(format!("native SDK source binding: {failure}")))?;
    let probes = loaded
        .contract
        .require_native_sdk_link_probes()
        .map_err(|failure| error(format!("native SDK source probes: {failure}")))?;
    aros_common::native_consumer_contract::validate_native_consumer_compiler(
        &loaded.contract,
        &request.compiler,
        &loaded.contract.abi.target_triple,
    )
    .map_err(|failure| error(format!("native SDK compiler binding: {failure}")))?;
    let build = checked_directory(&request.cmake_build_root, "native SDK CMake build")?;
    let compiler_root = checked_directory(&request.compiler_root, "native SDK compiler")?;
    let binding_bytes = read_regular_bounded(
        &build.join(BINDING_FILE),
        "native SDK CMake binding",
        canonical::MAX_DOCUMENT_BYTES,
    )?;
    revalidate_binding_with_helper(request, &loaded, &binding_bytes, cancellation)?;
    let sdk_root = resolve_binding(&binding_bytes, &build, source, &loaded)?;
    let layout = ToolchainToolLayout::load(&compiler_root).map_err(error)?;
    layout
        .validate_binding(&request.compiler, &loaded.contract.abi.target_triple)
        .map_err(error)?;
    let tools = layout
        .resolve_tools(&compiler_root)
        .map_err(error)?
        .into_iter()
        .collect::<BTreeMap<_, _>>();
    let compiler_before = measure_tree_content_cas_bounded(&compiler_root, PACKAGE_LIMITS)
        .map_err(|_| error("cannot measure native SDK compiler input tree"))?;
    let helper_root = request
        .preparation
        .helpers
        .values()
        .next()
        .and_then(|helper| helper.path.parent())
        .ok_or_else(|| error("native SDK helper root is absent"))?;
    let output = create_output_root(
        &request.output_root,
        &[
            source,
            &build,
            &compiler_root,
            &request.preparation.engine_root,
            helper_root,
        ],
    )?;
    write_new_regular(
        &output.join(BINDING_FILE),
        &binding_bytes,
        "exact native SDK CMake source binding",
    )?;
    let relocated = RelocatedSdk::prepare(&sdk_root, &output.join("relocated-sdk"))?;
    // Inventories are a separate bounded payload, not the small producer
    // metadata document (whose canonical encoder intentionally caps 1 MiB).
    let inventory_bytes = encode_inventory(&relocated.inventory)?;
    write_new_regular(
        &output.join(INVENTORY_FILE),
        &inventory_bytes,
        "full native SDK inventory",
    )?;
    let temporary = output.join("tmp");
    fs::create_dir(&temporary)
        .map_err(|_| error("cannot create private native SDK driver temporary root"))?;
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o700))
        .map_err(|_| error("cannot restrict native SDK driver temporary root"))?;
    let reports_root = output.join("reports");
    fs::create_dir(&reports_root)
        .map_err(|_| error("cannot create native SDK link report root"))?;
    let environment = sdk_environment(&temporary)?;
    let mut fixtures = BTreeMap::new();
    let mut outputs = BTreeMap::new();
    let mut maps = BTreeMap::new();
    let mut reports = BTreeMap::new();
    for (language, probe, phase) in [
        ("c", &probes.c, CompatibilityPhase::StandaloneC),
        ("cxx", &probes.cxx, CompatibilityPhase::StandaloneCxx),
    ] {
        let fixture = source.join(&probe.source);
        fixtures.insert(language.into(), file_identity(&fixture, 16 * 1024 * 1024)?);
        let program = tools
            .get(language)
            .ok_or_else(|| error("native SDK compiler role is missing"))?;
        let commands = [
            (&relocated.original, "original"),
            (&relocated.relocated, "relocated"),
        ]
        .into_iter()
        .map(|(sdk, label)| {
            application_command(
                program,
                &fixture,
                probe,
                &request.compiler,
                sdk,
                &output,
                (language, label),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
        let report = run_probe(
            &CompatibilityProbeRequest {
                phase,
                commands,
                environment: environment.clone(),
                current_dir: output.clone(),
                reports_root: reports_root.clone(),
                timeout: request.timeout,
                preparation: request.preparation.clone(),
            },
            cancellation,
        )?;
        let paths = ProbeReportPaths::new(&reports_root, phase, 2);
        let report_bytes = read_regular_bounded(
            &paths.report,
            "native SDK link phase report",
            canonical::MAX_DOCUMENT_BYTES,
        )?;
        if CompatibilityProbeReport::parse(&report_bytes)? != report {
            return Err(error("native SDK phase report changed after execution"));
        }
        reports.insert(language.into(), sha256_bytes(&report_bytes));
        let mut original_bytes = None;
        for (index, (sdk, label)) in [
            (&relocated.original, "original"),
            (&relocated.relocated, "relocated"),
        ]
        .into_iter()
        .enumerate()
        {
            let name = format!("{label}-{language}");
            let elf_bytes = read_regular_bounded(
                &output.join(format!("{name}.elf")),
                "native SDK application ELF",
                MAX_ELF_BYTES,
            )?;
            verify_application_elf(&elf_bytes, &request.compiler)?;
            if let Some(before) = &original_bytes {
                if before != &elf_bytes {
                    return Err(error(
                        "ordinary native SDK application differs after SDK relocation",
                    ));
                }
            } else {
                original_bytes = Some(elf_bytes.clone());
            }
            outputs.insert(name.clone(), identity(&elf_bytes));
            let map_bytes = read_regular_bounded(
                &output.join(format!("{name}.map")),
                "native SDK application link map",
                MAX_MAP_BYTES,
            )?;
            let stdout = read_regular_bounded(
                &paths.commands[index].stdout,
                "native SDK retained stdout",
                super::MAX_RENDERED_LOG_BYTES,
            )?;
            let stderr = read_regular_bounded(
                &paths.commands[index].stderr,
                "native SDK retained stderr",
                super::MAX_RENDERED_LOG_BYTES,
            )?;
            if sha256_bytes(&stdout) != report.commands[index].stdout_sha256
                || sha256_bytes(&stderr) != report.commands[index].stderr_sha256
            {
                return Err(error(
                    "native SDK retained logs differ from their phase report",
                ));
            }
            validate_link_paths(
                &stdout,
                &map_bytes,
                sdk,
                &compiler_root,
                &temporary,
                &relocated.original,
                (label == "relocated", &probe.libraries),
            )?;
            maps.insert(name, identity(&map_bytes));
        }
    }
    relocated.revalidate()?;
    if measure_tree_content_cas_bounded(&compiler_root, PACKAGE_LIMITS)
        .map_err(|_| error("cannot remeasure native SDK compiler input tree"))?
        != compiler_before
    {
        return Err(error(
            "native SDK compiler tree changed during application links",
        ));
    }
    super::validate_preparation(&request.preparation)?;
    let reloaded =
        load_bound_native_consumer_contract(source, Path::new(relative), &request.source_profile)
            .map_err(|_| error("native SDK source inputs changed during application links"))?;
    if reloaded != loaded
        || read_regular_bounded(
            &build.join(BINDING_FILE),
            "revalidated native SDK CMake binding",
            canonical::MAX_DOCUMENT_BYTES,
        )? != binding_bytes
    {
        return Err(error(
            "native SDK source or CMake binding changed during application links",
        ));
    }
    let receipt = Receipt {
        schema: "aros-native-sdk-link-receipt-v1".into(),
        qualification: "local-links-not-release-admission".into(),
        source_tree_sha256: request.preparation.source_tree_sha256.clone(),
        contract_sha256: loaded.sha256,
        binding_sha256: sha256_bytes(&binding_bytes),
        profile: loaded.contract.profile,
        compiler: request.compiler.clone(),
        tools_layout_sha256: layout.sha256().clone(),
        compiler_tree_sha256: compiler_before.payload_digest_excluding(None),
        sdk_inventory_sha256: relocated.inventory_sha256.clone(),
        sdk_entries: relocated.inventory.len(),
        inventory: identity(&inventory_bytes),
        fixtures,
        outputs,
        maps,
        reports,
    };
    let bytes = canonical::bytes(
        &serde_json::to_value(&receipt).map_err(|_| error("cannot encode native SDK receipt"))?,
    )?;
    let path = output.join(RECEIPT_FILE);
    let report = NativeSdkLinkReport {
        receipt: path.clone(),
        receipt_sha256: sha256_bytes(&bytes),
        sdk_inventory_sha256: relocated.inventory_sha256,
        sdk_entries: relocated.inventory.len(),
    };
    // A later language driver must not be able to change earlier retained
    // evidence and still leave a success receipt behind. Validate all proof
    // members against the proposed receipt before its no-clobber write.
    readback_native_sdk_links_bytes(request, &report, &bytes, false)?;
    write_new_regular(&path, &bytes, "native SDK application-link receipt")?;
    if read_regular_bounded(
        &path,
        "revalidated native SDK receipt",
        canonical::MAX_DOCUMENT_BYTES,
    )? != bytes
    {
        return Err(error(
            "native SDK application-link receipt changed after publication",
        ));
    }
    readback_native_sdk_links(request, &report)?;
    Ok(report)
}

/// Reread the complete retained SDK proof against independently selected inputs.
///
/// No process, build or network operation occurs here. Every output, map,
/// report and log is reopened through the bounded no-follow reader; both full
/// SDK inventories and the compiler tree are remeasured. The caller must
/// authenticate the expected receipt digest and package/source selection
/// outside this function. Parsing a receipt never authenticates its claims.
///
/// # Errors
/// Returns AX0703 for missing, changed, reordered, extra or mixed evidence,
/// a compiler/source binding mismatch, or stale SDK paths in retained links.
pub fn readback_native_sdk_links(
    request: &NativeSdkLinkRequest,
    expected: &NativeSdkLinkReport,
) -> Result<(), ContractError> {
    let bytes = read_regular_bounded(
        &expected.receipt,
        "retained native SDK receipt",
        canonical::MAX_DOCUMENT_BYTES,
    )?;
    readback_native_sdk_links_bytes(request, expected, &bytes, true)
}

fn readback_native_sdk_links_bytes(
    request: &NativeSdkLinkRequest,
    expected: &NativeSdkLinkReport,
    bytes: &[u8],
    persisted: bool,
) -> Result<(), ContractError> {
    super::validate_preparation(&request.preparation)?;
    let source = &request.preparation.source_root;
    let relative = request
        .source_profile
        .native_consumer_contract
        .as_deref()
        .ok_or_else(|| error("native SDK readback requires a source consumer selection"))?;
    let loaded =
        load_bound_native_consumer_contract(source, Path::new(relative), &request.source_profile)
            .map_err(|_| error("native SDK readback source contract differs"))?;
    let probes = loaded
        .contract
        .require_native_sdk_link_probes()
        .map_err(|_| error("native SDK readback requires the v2 source probes"))?;
    let output = checked_directory(&request.output_root, "retained native SDK proof")?;
    if expected.receipt != output.join(RECEIPT_FILE) {
        return Err(error("native SDK expected receipt path differs"));
    }
    if sha256_bytes(bytes) != expected.receipt_sha256 {
        return Err(error("native SDK receipt differs from its selected digest"));
    }
    let receipt: Receipt = serde_json::from_slice(bytes)
        .map_err(|_| error("native SDK receipt is not closed JSON"))?;
    if canonical::bytes(
        &serde_json::to_value(&receipt).map_err(|_| error("cannot encode retained SDK receipt"))?,
    )? != bytes
    {
        return Err(error("native SDK receipt is not canonical JSON"));
    }
    let build = checked_directory(&request.cmake_build_root, "retained native SDK CMake build")?;
    let binding = read_regular_bounded(
        &build.join(BINDING_FILE),
        "retained native SDK source binding",
        canonical::MAX_DOCUMENT_BYTES,
    )?;
    if read_regular_bounded(
        &output.join(BINDING_FILE),
        "retained native SDK source binding copy",
        canonical::MAX_DOCUMENT_BYTES,
    )? != binding
    {
        return Err(error(
            "native SDK proof binding differs from its CMake binding",
        ));
    }
    let sdk = resolve_binding(&binding, &build, source, &loaded)?;
    let relocated = checked_directory(
        &output.join("relocated-sdk"),
        "retained relocated native SDK",
    )?;
    let compiler_root = checked_directory(&request.compiler_root, "retained native SDK compiler")?;
    let layout = ToolchainToolLayout::load(&compiler_root).map_err(error)?;
    layout
        .validate_binding(&request.compiler, &loaded.contract.abi.target_triple)
        .map_err(error)?;
    let tools = layout
        .resolve_tools(&compiler_root)
        .map_err(error)?
        .into_iter()
        .collect::<BTreeMap<_, _>>();
    let compiler_tree = measure_tree_content_cas_bounded(&compiler_root, PACKAGE_LIMITS)
        .map_err(|_| error("cannot remeasure retained native SDK compiler"))?;
    if receipt.schema != "aros-native-sdk-link-receipt-v1"
        || receipt.qualification != "local-links-not-release-admission"
        || receipt.source_tree_sha256 != request.preparation.source_tree_sha256
        || receipt.contract_sha256 != loaded.sha256
        || receipt.binding_sha256 != sha256_bytes(&binding)
        || receipt.profile != loaded.contract.profile
        || receipt.compiler != request.compiler
        || receipt.tools_layout_sha256 != *layout.sha256()
        || receipt.compiler_tree_sha256 != compiler_tree.payload_digest_excluding(None)
        || receipt.sdk_inventory_sha256 != expected.sdk_inventory_sha256
        || receipt.sdk_entries != expected.sdk_entries
    {
        return Err(error(
            "native SDK receipt differs from selected source, compiler or SDK identities",
        ));
    }
    let inventory_bytes = read_regular_bounded(
        &output.join(INVENTORY_FILE),
        "retained full native SDK inventory",
        MAX_INVENTORY_BYTES,
    )?;
    if identity(&inventory_bytes) != receipt.inventory {
        return Err(error("native SDK inventory bytes differ from receipt"));
    }
    let inventory: Vec<aros_common::ArosToolchainManifestEntry> =
        serde_json::from_slice(&inventory_bytes)
            .map_err(|_| error("native SDK inventory is not closed JSON"))?;
    let encoded_inventory = encode_inventory(&inventory)?;
    if encoded_inventory != inventory_bytes || inventory.len() != expected.sdk_entries {
        return Err(error("native SDK inventory encoding or count differs"));
    }
    require_proof_members(&output, persisted)?;
    let mut sdk_snapshots = Vec::new();
    for root in [&sdk, &relocated] {
        sdk_snapshots.push(
            measure_tree_content_cas_bounded(root, PACKAGE_LIMITS)
                .map_err(|_| error("native SDK retained tree exceeds its resource bound"))?,
        );
        let (digest, observed) = sdk_inventory(root, "retained native SDK")?;
        if digest != expected.sdk_inventory_sha256 || observed != inventory {
            return Err(error(
                "retained SDK tree differs from its complete original inventory",
            ));
        }
    }
    let temporary = output.join("tmp");
    let environment = super::environment::resolve(&sdk_environment(&temporary)?)?;
    let environment_sha256 = super::environment::identity(&environment)?;
    let helpers = request
        .preparation
        .helpers
        .iter()
        .map(|(name, helper)| {
            (
                name.clone(),
                super::CompatibilityHelperReport {
                    sha256: helper.sha256.clone(),
                    size: helper.size,
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut fixtures = BTreeMap::new();
    let mut outputs = BTreeMap::new();
    let mut maps = BTreeMap::new();
    let mut reports = BTreeMap::new();
    for (language, probe, phase) in [
        ("c", &probes.c, CompatibilityPhase::StandaloneC),
        ("cxx", &probes.cxx, CompatibilityPhase::StandaloneCxx),
    ] {
        let fixture = source.join(&probe.source);
        fixtures.insert(language.into(), file_identity(&fixture, 16 * 1024 * 1024)?);
        let program = tools
            .get(language)
            .ok_or_else(|| error("retained SDK compiler role is missing"))?;
        let paths = ProbeReportPaths::new(&output.join("reports"), phase, 2);
        let report_bytes = read_regular_bounded(
            &paths.report,
            "retained native SDK phase report",
            canonical::MAX_DOCUMENT_BYTES,
        )?;
        let report = CompatibilityProbeReport::parse(&report_bytes)?;
        if report.commands.len() != 2
            || report.phase != phase
            || report.engine_api_version != request.preparation.engine_api_version
            || report.engine_sha256 != request.preparation.engine_sha256
            || report.source_tree_sha256 != request.preparation.source_tree_sha256
            || report.helpers != helpers
            || !report.host_tools.is_empty()
            || report.environment_sha256 != environment_sha256
            || canonical::bytes(
                &serde_json::to_value(&report)
                    .map_err(|_| error("cannot encode retained SDK phase"))?,
            )? != report_bytes
        {
            return Err(error(
                "native SDK phase differs from exact execution inputs",
            ));
        }
        reports.insert(language.into(), sha256_bytes(&report_bytes));
        let mut first = None;
        for (index, (root, label)) in [(&sdk, "original"), (&relocated, "relocated")]
            .into_iter()
            .enumerate()
        {
            let command = application_command(
                program,
                &fixture,
                probe,
                &request.compiler,
                root,
                &output,
                (language, label),
            )?;
            let executable = super::checked_executable(program)?;
            let (program_sha256, _) = super::measure_executable(&executable)?;
            if report.commands[index].program_sha256 != program_sha256
                || report.commands[index].command_sha256
                    != super::command_identity(&command.program, &command.arguments)?
            {
                return Err(error("native SDK phase command differs from ordinary source-declared driver invocation"));
            }
            let name = format!("{label}-{language}");
            let elf_bytes = read_regular_bounded(
                &output.join(format!("{name}.elf")),
                "retained SDK application ELF",
                MAX_ELF_BYTES,
            )?;
            verify_application_elf(&elf_bytes, &request.compiler)?;
            if first.as_ref().is_some_and(|before| before != &elf_bytes) {
                return Err(error(
                    "retained application bytes differ after SDK relocation",
                ));
            }
            first = Some(elf_bytes.clone());
            outputs.insert(name.clone(), identity(&elf_bytes));
            let map = read_regular_bounded(
                &output.join(format!("{name}.map")),
                "retained SDK link map",
                MAX_MAP_BYTES,
            )?;
            let stdout = read_regular_bounded(
                &paths.commands[index].stdout,
                "retained SDK stdout",
                super::MAX_RENDERED_LOG_BYTES,
            )?;
            let stderr = read_regular_bounded(
                &paths.commands[index].stderr,
                "retained SDK stderr",
                super::MAX_RENDERED_LOG_BYTES,
            )?;
            if sha256_bytes(&stdout) != report.commands[index].stdout_sha256
                || sha256_bytes(&stderr) != report.commands[index].stderr_sha256
            {
                return Err(error("native SDK retained logs differ"));
            }
            validate_link_paths(
                &stdout,
                &map,
                root,
                &compiler_root,
                &temporary,
                &sdk,
                (index == 1, &probe.libraries),
            )?;
            maps.insert(name, identity(&map));
        }
    }
    if fixtures != receipt.fixtures
        || outputs != receipt.outputs
        || maps != receipt.maps
        || reports != receipt.reports
    {
        return Err(error(
            "native SDK receipt does not cover the exact four ordinary links",
        ));
    }
    if persisted
        && read_regular_bounded(
            &expected.receipt,
            "rechecked native SDK receipt",
            canonical::MAX_DOCUMENT_BYTES,
        )? != bytes
    {
        return Err(error("native SDK receipt changed during readback"));
    }
    for (root, before) in [&sdk, &relocated].into_iter().zip(sdk_snapshots) {
        if measure_tree_content_cas_bounded(root, PACKAGE_LIMITS)
            .map_err(|_| error("cannot recheck retained native SDK tree"))?
            != before
        {
            return Err(error("native SDK tree changed during retained readback"));
        }
    }
    if measure_tree_content_cas_bounded(&compiler_root, PACKAGE_LIMITS)
        .map_err(|_| error("cannot recheck retained native SDK compiler"))?
        != compiler_tree
    {
        return Err(error(
            "native SDK compiler changed during retained readback",
        ));
    }
    super::validate_preparation(&request.preparation)?;
    require_proof_members(&output, persisted)?;
    Ok(())
}

fn require_proof_members(root: &Path, persisted: bool) -> Result<(), ContractError> {
    let mut names = std::collections::BTreeSet::from([
        BINDING_FILE.into(),
        INVENTORY_FILE.into(),
        "relocated-sdk".into(),
        "tmp".into(),
        "reports".into(),
    ]);
    if persisted {
        names.insert(RECEIPT_FILE.into());
        add_publication_lock(root, RECEIPT_FILE, &mut names)?;
    }
    add_publication_lock(root, INVENTORY_FILE, &mut names)?;
    add_publication_lock(root, BINDING_FILE, &mut names)?;
    for label in ["original", "relocated"] {
        for language in ["c", "cxx"] {
            for suffix in ["elf", "map"] {
                names.insert(format!("{label}-{language}.{suffix}"));
            }
        }
    }
    require_children(root, &names)?;
    let mut reports = std::collections::BTreeSet::new();
    for phase in [
        CompatibilityPhase::StandaloneC,
        CompatibilityPhase::StandaloneCxx,
    ] {
        reports.insert(format!("{}.report.json", phase.file_stem()));
        for index in 1..=2 {
            for stream in ["stdout", "stderr"] {
                reports.insert(format!("{}.{index}.{stream}.log", phase.file_stem()));
            }
        }
    }
    let report_names = reports.clone();
    for name in &report_names {
        add_publication_lock(&root.join("reports"), name, &mut reports)?;
    }
    require_children(&root.join("reports"), &reports)?;
    let temporary = root.join("tmp");
    validate_driver_temporary_root(&temporary)?;
    require_children(&temporary, &std::collections::BTreeSet::new())
}

fn add_publication_lock(
    root: &Path,
    name: &str,
    members: &mut std::collections::BTreeSet<String>,
) -> Result<(), ContractError> {
    let journal = aros_common::publication_journal_path(&root.join(name), "file")
        .map_err(|_| error("cannot derive SDK proof publication journal"))?;
    let lock = aros_common::publication_journal_lock_path(&journal)
        .map_err(|_| error("cannot derive SDK proof publication lock"))?;
    read_regular_bounded(&lock, "empty SDK proof publication lock", 0)?;
    let name = lock
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| error("SDK proof publication lock has no portable name"))?;
    members.insert(name.to_owned());
    Ok(())
}

fn require_children(
    root: &Path,
    expected: &std::collections::BTreeSet<String>,
) -> Result<(), ContractError> {
    let root = checked_directory(root, "native SDK retained member root")?;
    let entries =
        fs::read_dir(root).map_err(|_| error("cannot inspect retained native SDK members"))?;
    let mut observed = std::collections::BTreeSet::new();
    for entry in entries {
        let entry = entry.map_err(|_| error("cannot inspect native SDK member"))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| error("native SDK member name is not UTF-8"))?;
        if !expected.contains(&name) || !observed.insert(name) {
            return Err(error(
                "native SDK proof has an extra or duplicate retained member",
            ));
        }
    }
    if observed != *expected {
        return Err(error("native SDK proof omits a retained member"));
    }
    Ok(())
}

fn sdk_environment(temporary: &Path) -> Result<CompatibilityEnvironment, ContractError> {
    Ok(CompatibilityEnvironment::Poisoned {
        variables: BTreeMap::from([
            ("PATH".into(), "/nonexistent".into()),
            ("LC_ALL".into(), "C".into()),
            ("LANG".into(), "C".into()),
            ("TMPDIR".into(), text_path(temporary)?),
        ]),
    })
}

fn resolve_binding(
    bytes: &[u8],
    build: &Path,
    source: &Path,
    loaded: &aros_common::native_consumer_contract::LoadedNativeConsumerContract,
) -> Result<PathBuf, ContractError> {
    let binding: ConsumerBinding = serde_json::from_slice(bytes)
        .map_err(|_| error("native SDK binding is not closed JSON"))?;
    let expected_inputs = loaded
        .contract
        .inputs
        .iter()
        .map(|input| input.path.clone())
        .collect::<Vec<_>>();
    let expected_smp = loaded
        .contract
        .generated_make_templates
        .values()
        .filter_map(|item| item.substitutions.get("@ENABLE_EXECSMP@"))
        .any(|value| value == "#define __AROSEXEC_SMP__");
    if binding.schema != "aros-native-consumer-validation-v1"
        || binding.qualification != "source-binding-not-graph-or-build-proof"
        || Path::new(&binding.source_dir) != source
        || Path::new(&binding.contract_path) != loaded.path
        || binding.contract_sha256 != loaded.sha256
        || binding.profile != loaded.contract.profile
        || binding.abi != loaded.contract.abi
        || binding.exec_smp != expected_smp
        || binding.input_paths != expected_inputs
    {
        return Err(error(
            "native SDK CMake binding differs from its exact source selection",
        ));
    }
    if binding.sdk_include_relative.is_empty()
        || binding.sdk_include_relative.len() > 4096
        || binding.sdk_include_relative.split('/').any(|part| {
            part.is_empty()
                || matches!(part, "." | "..")
                || !part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_.+-".contains(&byte))
        })
    {
        return Err(error("native SDK binding contains an unsafe include path"));
    }
    let relative = Path::new(&binding.sdk_include_relative);
    if relative.file_name().and_then(|name| name.to_str()) != Some("include") {
        return Err(error(
            "native SDK include root must end in the source-proven include directory",
        ));
    }
    let parent = relative
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| error("native SDK root is absent"))?;
    let root = checked_directory(&build.join(parent), "source-declared native SDK")?;
    if !root.starts_with(build)
        || root == build
        || checked_directory(&root.join("include"), "native SDK public headers")?
            != root.join("include")
    {
        return Err(error(
            "native SDK root or headers escape the selected CMake build",
        ));
    }
    Ok(root)
}

fn revalidate_binding_with_helper(
    request: &NativeSdkLinkRequest,
    loaded: &aros_common::native_consumer_contract::LoadedNativeConsumerContract,
    binding_bytes: &[u8],
    cancellation: &CancellationToken,
) -> Result<(), ContractError> {
    let current = source_binding(request, loaded, cancellation)?;
    let persisted: ConsumerBinding = serde_json::from_slice(binding_bytes)
        .map_err(|_| error("persisted native SDK validator response is not closed JSON"))?;
    if current != persisted {
        return Err(error(
            "native SDK CMake binding differs from fresh source-owned SDK root validation",
        ));
    }
    Ok(())
}

pub(super) fn source_binding(
    request: &NativeSdkLinkRequest,
    loaded: &aros_common::native_consumer_contract::LoadedNativeConsumerContract,
    cancellation: &CancellationToken,
) -> Result<ConsumerBinding, ContractError> {
    let context = request
        .source_profile
        .transpiler
        .as_ref()
        .ok_or_else(|| error("native SDK profile lacks explicit MetaMake selectors"))?;
    let helper = request
        .preparation
        .helpers
        .get("aros-transpiler")
        .ok_or_else(|| error("native SDK source validator is missing"))?;
    let mut command = std::process::Command::new(&helper.path);
    command
        .env_clear()
        .env("PATH", "/nonexistent")
        .env("LC_ALL", "C")
        .current_dir(&request.preparation.source_root)
        .args([
            "--validate-native-consumer-only".into(),
            "--native-consumer-profile".into(),
            request.source_profile.name.clone(),
            "--native-consumer-contract-sha256".into(),
            loaded.sha256.to_string(),
            "--source-dir".into(),
            text_path(&request.preparation.source_root)?,
            "--cpu".into(),
            loaded.contract.abi.source_cpu.clone(),
            "--platform".into(),
            request.source_profile.platform.clone(),
            "--family".into(),
            context.family.clone(),
            "--variant".into(),
            context.variant.clone(),
            "--toolchain".into(),
            context.toolchain.clone(),
            "--cpu32".into(),
            context.cpu32.clone(),
            "--use-mmu".into(),
            if context.use_mmu { "1" } else { "0" }.into(),
            "--float-abi".into(),
            loaded.contract.abi.abi.clone(),
            "--mesa-version".into(),
            context.mesa_version.clone().unwrap_or_default(),
        ]);
    let result = aros_common::run_output_with_input_and_control(
        &mut command,
        &[],
        canonical::MAX_DOCUMENT_BYTES,
        request.timeout.min(Duration::from_secs(60)),
        cancellation,
    )
    .map_err(|_| error("cannot supervise the native SDK source validator"))?;
    if !result.status.success() || result.timed_out || result.cancelled {
        return Err(error(
            "native SDK source validator did not complete successfully",
        ));
    }
    let exact = result
        .stdout
        .exact_bytes()
        .ok_or_else(|| error("native SDK source validation response was truncated"))?;
    let current: ConsumerBinding = serde_json::from_slice(exact)
        .map_err(|_| error("current native SDK validator response is not closed JSON"))?;
    Ok(current)
}

pub(super) fn application_command(
    program: &Path,
    fixture: &Path,
    probe: &NativeSdkLinkProbe,
    compiler: &ArosCompilerIdentity,
    sdk: &Path,
    output: &Path,
    selection: (&str, &str),
) -> Result<CompatibilityCommand, ContractError> {
    let (language, label) = selection;
    let ArosCompilerIdentity::Gnu { target, .. } = compiler else {
        return Err(error("ordinary native SDK links require a GNU compiler"));
    };
    let name = format!("{label}-{language}");
    let mut arguments = vec![
        format!("--sysroot={}", text_path(sdk)?),
        format!("-march={}", target.isa()),
        format!("-mabi={}", target.abi()),
        format!("-mcmodel={}", target.code_model()),
        "-mstrict-align".into(),
        "-O2".into(),
        "-fno-common".into(),
        "-g0".into(),
    ];
    if language == "cxx" {
        arguments.push("-std=c++17".into());
    }
    arguments.extend([
        text_path(fixture)?,
        format!(
            "-Wl,-Map={},--cref,--trace",
            text_path(&output.join(format!("{name}.map")))?
        ),
    ]);
    arguments.extend(probe.libraries.iter().map(|library| format!("-l{library}")));
    arguments.extend(["-o".into(), text_path(&output.join(format!("{name}.elf")))?]);
    Ok(CompatibilityCommand {
        program: program.into(),
        arguments,
    })
}

pub(super) fn verify_application_elf(
    bytes: &[u8],
    compiler: &ArosCompilerIdentity,
) -> Result<(), ContractError> {
    let ArosCompilerIdentity::Gnu { target, .. } = compiler else {
        return Err(error("native SDK ELF lacks its GNU ABI contract"));
    };
    target
        .verify(bytes, elf::riscv::ArtifactRole::ArosRelocatable)
        .map_err(|failure| error(format!("native SDK application ABI: {failure}")))
}

fn validate_link_paths(
    trace: &[u8],
    map: &[u8],
    sdk: &Path,
    compiler: &Path,
    temporary: &Path,
    original_sdk: &Path,
    selection: (bool, &[String]),
) -> Result<(), ContractError> {
    let (relocated, libraries) = selection;
    let trace =
        std::str::from_utf8(trace).map_err(|_| error("native SDK linker trace is not UTF-8"))?;
    let map = std::str::from_utf8(map).map_err(|_| error("native SDK linker map is not UTF-8"))?;
    if trace.contains(" bytes omitted by aros]") {
        return Err(error(
            "native SDK linker trace is truncated and cannot prove its input closure",
        ));
    }
    if relocated
        && (trace.contains(&text_path(original_sdk)?) || map.contains(&text_path(original_sdk)?))
    {
        return Err(error(
            "relocated native SDK link retained its original SDK path",
        ));
    }
    let mut sdk_archive = false;
    let mut sdk_libraries = std::collections::BTreeSet::new();
    let mut linked_inputs = 0;
    for line in trace.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if !line.starts_with('/') {
            return Err(error(
                "native SDK linker trace contains a non-absolute or unrecognized input",
            ));
        }
        let path = Path::new(line.split_once('(').map_or(line, |(prefix, _)| prefix));
        // GCC removes its own compilation temporary before returning. Admit
        // only that direct-child naming contract, not arbitrary descendants
        // or unresolved symlink inputs hidden behind TMPDIR containment.
        if path.starts_with(temporary) {
            validate_driver_temporary(path, temporary)?;
            linked_inputs += 1;
            continue;
        }
        let resolved = path
            .canonicalize()
            .map_err(|_| error("native SDK linker trace names an unresolved input"))?;
        if !resolved.starts_with(sdk)
            && !resolved.starts_with(compiler)
            && !resolved.starts_with(temporary)
        {
            return Err(error("native SDK linker trace selected an input outside the SDK, compiler or private temporary root"));
        }
        sdk_archive |= resolved.starts_with(sdk)
            && resolved
                .extension()
                .is_some_and(|extension| extension == "a");
        if resolved.starts_with(sdk) {
            if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
                sdk_libraries.insert(name.to_owned());
            }
        }
        linked_inputs += 1;
    }
    if !sdk_archive || linked_inputs == 0 || map.is_empty() {
        return Err(error(
            "ordinary native SDK link lacks a traced SDK archive or link map",
        ));
    }
    for library in libraries {
        if !sdk_libraries.contains(&format!("lib{library}.a")) {
            return Err(error(
                "source-declared native SDK library did not resolve from the selected SDK",
            ));
        }
    }
    Ok(())
}

fn validate_driver_temporary(path: &Path, temporary: &Path) -> Result<(), ContractError> {
    validate_driver_temporary_root(temporary)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| error("native SDK driver temporary name is not UTF-8"))?;
    let token = name
        .strip_prefix("cc")
        .and_then(|name| name.strip_suffix(".o"));
    if path.parent() != Some(temporary)
        || !token.is_some_and(|token| {
            !token.is_empty() && token.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
    {
        return Err(error(
            "native SDK trace contains an unrecognized driver temporary input",
        ));
    }
    match fs::symlink_metadata(path) {
        Err(failure) if failure.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            let resolved = path
                .canonicalize()
                .map_err(|_| error("cannot resolve SDK driver temporary"))?;
            if resolved.parent() != Some(temporary) {
                return Err(error(
                    "SDK driver temporary resolves outside its private root",
                ));
            }
            Ok(())
        }
        _ => Err(error(
            "SDK driver temporary is an unsafe or unresolved input",
        )),
    }
}

fn validate_driver_temporary_root(temporary: &Path) -> Result<(), ContractError> {
    let metadata = fs::symlink_metadata(temporary)
        .map_err(|_| error("cannot inspect the private SDK driver temporary root"))?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o777 != 0o700
    {
        return Err(error("native SDK driver temporary root is not private"));
    }
    Ok(())
}

fn create_output_root(path: &Path, inputs: &[&Path]) -> Result<PathBuf, ContractError> {
    if !path.is_absolute() {
        return Err(error("native SDK output root must be absolute"));
    }
    let parent = checked_directory(
        path.parent()
            .ok_or_else(|| error("native SDK output root lacks a parent"))?,
        "native SDK output parent",
    )?;
    let leaf = path
        .file_name()
        .ok_or_else(|| error("native SDK output root lacks a leaf"))?;
    let output = parent.join(leaf);
    if inputs
        .iter()
        .any(|input| output.starts_with(input) || input.starts_with(&output))
    {
        return Err(error(
            "native SDK proof output must be disjoint from every source, build, compiler, engine and helper input",
        ));
    }
    fs::create_dir(&output)
        .map_err(|_| error("native SDK proof root already exists or cannot be created"))?;
    checked_directory(&output, "fresh native SDK proof root")
}

fn file_identity(path: &Path, limit: usize) -> Result<FileIdentity, ContractError> {
    Ok(identity(&read_regular_bounded(
        path,
        "native SDK sealed fixture",
        limit,
    )?))
}
fn identity(bytes: &[u8]) -> FileIdentity {
    FileIdentity {
        sha256: sha256_bytes(bytes),
        size: bytes.len() as u64,
    }
}
fn text_path(path: &Path) -> Result<String, ContractError> {
    let value = path
        .to_str()
        .ok_or_else(|| error("native SDK path is not UTF-8"))?;
    if value.contains(',') || value.chars().any(char::is_control) {
        return Err(error(
            "native SDK driver path contains linker separators or control characters",
        ));
    }
    Ok(value.into())
}
fn error(message: impl Into<String>) -> ContractError {
    ContractError::compatibility(message)
}

#[cfg(test)]
#[path = "native_sdk_links_tests.rs"]
mod integration_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn compiler() -> ArosCompilerIdentity {
        serde_json::from_value(serde_json::json!({
            "family": "gnu", "gcc_version": "16.2.0", "binutils_version": "2.47",
            "target": {
                "schema": "aros-riscv-target-v1", "isa": "rv32imafc_zicsr_zifencei_zaamo_zalrsc",
                "abi": "ilp32f", "code_model": "medany",
                "architecture": "rv32i2p1_m2p0_a2p1_f2p2_c2p0_zicsr2p0_zifencei2p0_zaamo1p0_zalrsc1p0",
                "unaligned_access": false, "atomic_abi": 0, "x3_reg_usage": 0
            }
        })).unwrap()
    }

    #[test]
    fn ordinary_commands_retain_startup_and_standard_library_driver_defaults() {
        let probe = NativeSdkLinkProbe {
            source: "application.cpp".into(),
            libraries: vec!["client-shared".into()],
        };
        let command = application_command(
            Path::new("/prefix/cxx"),
            Path::new("/source/application.cpp"),
            &probe,
            &compiler(),
            Path::new("/selected/sdk"),
            Path::new("/proof"),
            ("cxx", "original"),
        )
        .unwrap();
        for forbidden in [
            "-c",
            "-r",
            "-nostdlib",
            "-nostartfiles",
            "-nodefaultlibs",
            "-ffreestanding",
        ] {
            assert!(
                !command
                    .arguments
                    .iter()
                    .any(|argument| argument == forbidden),
                "{forbidden}"
            );
        }
        assert!(command
            .arguments
            .contains(&"--sysroot=/selected/sdk".into()));
        assert!(command.arguments.contains(&"-mabi=ilp32f".into()));
        assert!(command.arguments.contains(&"-lclient-shared".into()));
        assert!(command.arguments.contains(&"-std=c++17".into()));
    }

    #[test]
    fn exact_application_elf_requires_aros_and_source_abi() {
        let compiler = compiler();
        let ArosCompilerIdentity::Gnu { target, .. } = &compiler else {
            unreachable!()
        };
        let good = super::super::fixture_riscv_elf(elf::Class::Elf32, "application", target, false);
        verify_application_elf(&good, &compiler).unwrap();
        let wrong_float =
            super::super::fixture_riscv_elf(elf::Class::Elf32, "application", target, true);
        assert!(verify_application_elf(&wrong_float, &compiler).is_err());
        let mut wrong_os = good;
        wrong_os[7] = 0;
        assert!(verify_application_elf(&wrong_os, &compiler).is_err());
    }

    #[test]
    fn link_inputs_reject_external_fallback_symlinks_stale_roots_and_truncation() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let sdk = root.join("sdk");
        let compiler = root.join("compiler");
        let tmp = root.join("tmp");
        let original = root.join("original-sdk");
        for path in [&sdk, &compiler, &tmp, &original] {
            fs::create_dir(path).unwrap();
        }
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(sdk.join("client.a"), b"fixture archive").unwrap();
        fs::write(compiler.join("runtime.a"), b"fixture runtime").unwrap();
        fs::write(original.join("old.a"), b"old SDK").unwrap();
        let good = format!(
            "{}\n{}\n{}\n",
            sdk.join("client.a").display(),
            compiler.join("runtime.a").display(),
            tmp.join("ccAb123.o").display()
        );
        validate_link_paths(
            good.as_bytes(),
            b"valid map",
            &sdk,
            &compiler,
            &tmp,
            &original,
            (true, &[]),
        )
        .unwrap();
        let external = format!("{good}{}\n", original.join("old.a").display());
        assert!(validate_link_paths(
            external.as_bytes(),
            b"valid map",
            &sdk,
            &compiler,
            &tmp,
            &original,
            (false, &[])
        )
        .unwrap_err()
        .to_string()
        .contains("outside"));
        let stale_map = format!("LOAD {}", original.join("old.a").display());
        assert!(validate_link_paths(
            good.as_bytes(),
            stale_map.as_bytes(),
            &sdk,
            &compiler,
            &tmp,
            &original,
            (true, &[])
        )
        .unwrap_err()
        .to_string()
        .contains("original SDK"));
        let truncated = format!("{good}[32 bytes omitted by aros]\n");
        assert!(validate_link_paths(
            truncated.as_bytes(),
            b"valid map",
            &sdk,
            &compiler,
            &tmp,
            &original,
            (true, &[])
        )
        .unwrap_err()
        .to_string()
        .contains("truncated"));
        std::os::unix::fs::symlink(original.join("old.a"), sdk.join("escape.a")).unwrap();
        let escaped = format!("{good}{}\n", sdk.join("escape.a").display());
        assert!(validate_link_paths(
            escaped.as_bytes(),
            b"valid map",
            &sdk,
            &compiler,
            &tmp,
            &original,
            (false, &[])
        )
        .is_err());
        assert!(validate_link_paths(
            compiler.join("runtime.a").to_str().unwrap().as_bytes(),
            b"map",
            &sdk,
            &compiler,
            &tmp,
            &original,
            (false, &[])
        )
        .unwrap_err()
        .to_string()
        .contains("lacks"));
    }

    #[test]
    fn fresh_proof_roots_reject_input_overlap_and_existing_data_without_overwrite() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let input = root.join("input");
        fs::create_dir(&input).unwrap();
        assert!(create_output_root(&input.join("nested"), &[&input]).is_err());
        assert!(!input.join("nested").exists());
        fs::write(root.join("existing"), b"preserve me").unwrap();
        assert!(create_output_root(&root.join("existing"), &[&input]).is_err());
        assert_eq!(fs::read(root.join("existing")).unwrap(), b"preserve me");
        let created = create_output_root(&root.join("proof"), &[&input]).unwrap();
        assert!(created.is_dir());
    }

    #[test]
    fn ambiguous_linker_paths_are_rejected() {
        for path in ["/proof/with,comma", "/proof/with\nnewline"] {
            assert!(text_path(Path::new(path)).is_err());
        }
    }

    #[test]
    fn driver_temporary_inputs_are_private_direct_children_not_symlink_fallbacks() {
        let root = tempfile::tempdir().unwrap();
        let temporary = root.path().join("tmp");
        fs::create_dir(&temporary).unwrap();
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o700)).unwrap();
        validate_driver_temporary(&temporary.join("ccAb123.o"), &temporary).unwrap();
        let outside = root.path().join("foreign.o");
        fs::write(&outside, b"foreign").unwrap();
        let linked = temporary.join("ccOther.o");
        std::os::unix::fs::symlink(&outside, &linked).unwrap();
        assert!(validate_driver_temporary(&linked, &temporary).is_err());
        assert!(validate_driver_temporary(&temporary.join("nested/ccAb.o"), &temporary).is_err());
        assert!(validate_driver_temporary(&temporary.join("unknown.o"), &temporary).is_err());
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(validate_driver_temporary(&temporary.join("ccAb123.o"), &temporary).is_err());
    }
}
