//! In-memory read-back for compiler-family native compatibility receipts.
//!
//! This boundary joins exact retained bytes to independently supplied package,
//! profile, engine, source, ports, and standalone-output identities. It does
//! not establish where the bytes came from or authenticate the caller's
//! declarations.

use std::collections::{BTreeMap, BTreeSet};

use aros_common::{sha256_bytes, ArosCompilerIdentity, ArosToolchainManifest, Sha256Digest};
use serde::de::{DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde::Deserializer;

use super::{
    CompatibilityReceiptArtifact, CompatibilityReceiptDocument, CompatibilityReceiptPortsSource,
};
use crate::compatibility::{
    CompatibilityHelperReport, CompatibilityHostToolReport, CompatibilityPhase,
    CompatibilityProbeReport, StandaloneArtifactIdentity, StandaloneOutputReport,
};
use crate::compatibility_ports::{safe_fetch_marker_path, safe_relative_path};
use crate::profiles::{identifier, Profile, Profiles};
use crate::recipe::GitObjectId;
use crate::source_lock::CompilerFamily;
use crate::ContractError;

const MAX_RECEIPT_READBACK_COMMANDS: usize = 9;
const MAX_RECEIPT_READBACK_LOG_BYTES: usize =
    super::super::MAX_RENDERED_LOG_BYTES * MAX_RECEIPT_READBACK_COMMANDS * 2;

/// Actual retained stdout and stderr bytes for one reported command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeCompatibilityCommandLogs {
    /// Exact rendered stdout log bytes read back from the retained log.
    pub stdout: Vec<u8>,
    /// Exact rendered stderr log bytes read back from the retained log.
    pub stderr: Vec<u8>,
}

/// Independently measured package identity expected by the read-back join.
///
/// The manifest must come from the independently verified package; its
/// canonical digest is recomputed here. The package source commit is separate
/// from the SDK consumer source-tree CAS and pristine upstream consumer commit.
#[derive(Debug, Clone)]
pub struct NativeCompatibilityExpectedPackage<'a> {
    /// Exact v2 manifest from the independently verified package.
    pub manifest: &'a ArosToolchainManifest,
    /// SHA-256 measured over the package archive bytes.
    pub archive_sha256: &'a Sha256Digest,
    /// Exact measured archive byte length.
    pub archive_size: u64,
    /// Independently selected compiler identity, including GNU target ABI.
    pub compiler: &'a ArosCompilerIdentity,
    /// Recipe-bound package-build source commit from the manifest inputs.
    pub source_commit: &'a GitObjectId,
    /// Expected package host selector.
    pub host: &'a str,
}

/// One locked ports-input record represented by the aggregate receipt.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeCompatibilityExpectedPortSource {
    /// Stable lock-local source identifier.
    pub id: String,
    /// Portable cache filename.
    pub cache_filename: String,
    /// Safe relative source path.
    pub relative_path: String,
    /// Exact fetch marker path, or an empty string when absent.
    pub fetch_marker: String,
    /// Locked payload SHA-256.
    pub sha256: Sha256Digest,
    /// Locked payload byte length.
    pub size: u64,
}

/// All byte snapshots and independent expectations for one read-back.
#[derive(Debug)]
pub struct NativeCompatibilityReceiptReadbackRequest<'a> {
    /// Exact canonical aggregate receipt bytes.
    pub receipt_bytes: &'a [u8],
    /// Exact phase report bytes keyed by phase; this map must contain all six.
    pub phase_report_bytes: &'a BTreeMap<CompatibilityPhase, Vec<u8>>,
    /// Exact retained stdout/stderr bytes in command order for each phase.
    pub command_logs: &'a BTreeMap<CompatibilityPhase, Vec<NativeCompatibilityCommandLogs>>,
    /// Independently verified package identity.
    pub package: NativeCompatibilityExpectedPackage<'a>,
    /// Validated profiles document selected from the expected source inputs.
    pub profiles: &'a Profiles,
    /// Exact selected profile from the profiles document.
    pub profile: &'a Profile,
    /// Expected source-owned GNU preset; LLVM receipts require None.
    pub gnu_source_preset: Option<&'a str>,
    /// Whether the validated GNU source preset requires a CMake build command.
    /// Derive this from its source-native-consumer contract, not the reports.
    /// LLVM must set this to false.
    pub cmake_build_required: bool,
    /// Measured content-only CAS of the engine-free SDK consumer source tree.
    pub sdk_consumer_source_tree_sha256: &'a Sha256Digest,
    /// Embedded tools-owned CMake engine API version expected by every phase.
    pub engine_api_version: u32,
    /// Embedded tools-owned CMake engine digest expected by every phase.
    pub engine_sha256: &'a Sha256Digest,
    /// Exact required helper identities expected by every phase.
    pub helpers: &'a BTreeMap<String, CompatibilityHelperReport>,
    /// Exact measured host-tool identities expected for CMake and upstream.
    pub host_tools: &'a BTreeMap<String, CompatibilityHostToolReport>,
    /// Independently expected closed environment digest for CMake/upstream.
    pub sdk_environment_sha256: &'a Sha256Digest,
    /// Independently expected poisoned environment digest for standalone phases.
    pub standalone_environment_sha256: &'a Sha256Digest,
    /// Recipe/profile-bound pristine upstream consumer commit.
    pub upstream_source_commit: &'a GitObjectId,
    /// Independently measured content tree identity of pristine upstream.
    pub upstream_source_tree: &'a GitObjectId,
    /// Locked ports payload closure in stable relative-path order.
    pub ports_sources: &'a [NativeCompatibilityExpectedPortSource],
    /// Independently verified standalone C/C++ output identities.
    pub standalone: &'a StandaloneOutputReport,
}

/// Result emitted only after the complete receipt/read-back join succeeds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeCompatibilityReceiptReadback {
    /// SHA-256 measured over the exact aggregate receipt bytes.
    pub receipt_sha256: Sha256Digest,
    /// The exact validated six-phase set.
    pub phases: BTreeSet<CompatibilityPhase>,
}

/// Verify exact in-memory receipt, report, and retained-log bytes against
/// independently supplied compiler-family expectations.
///
/// Only GNU receipt v3 and LLVM family-v2 receipt v4 are admitted. All JSON
/// documents must be canonical, closed metadata; reports and logs are bounded
/// before hashing. The caller must bind the expected manifest, source/profile,
/// engine/helper, ports, upstream, and standalone identities to the relevant
/// independently verified release/index inputs, runtime, and source trees,
/// and must supply byte snapshots read from the retained operation outputs.
///
/// This function performs no filesystem read, compiler execution, signature
/// verification, publication, release admission, or authentication of the
/// executor or supplied declarations. Success means only that these exact
/// in-memory bytes and declarations form a complete, internally consistent
/// read-back.
///
/// # Errors
///
/// Returns AX0703 if any expected identity differs, metadata is malformed or
/// noncanonical, a phase/report/log is missing or reordered, or a byte bound
/// is exceeded.
pub fn readback_native_compatibility_receipt(
    request: &NativeCompatibilityReceiptReadbackRequest<'_>,
) -> Result<NativeCompatibilityReceiptReadback, ContractError> {
    validate_expectations(request)?;
    if request.receipt_bytes.is_empty()
        || request.receipt_bytes.len() > crate::canonical::MAX_DOCUMENT_BYTES
    {
        return Err(readback_error(
            "native compatibility receipt bytes are empty or exceed the document limit",
        ));
    }
    reject_duplicate_json_keys(request.receipt_bytes)?;
    let document: CompatibilityReceiptDocument = serde_json::from_slice(request.receipt_bytes)
        .map_err(|_| {
            readback_error("native compatibility receipt is not a closed JSON document")
        })?;
    document.validate()?;
    require_canonical_json(
        &document,
        request.receipt_bytes,
        "native compatibility receipt",
    )?;

    let expected_schema = match request.package.compiler {
        ArosCompilerIdentity::Gnu { .. } => super::GNU_COMPATIBILITY_RECEIPT_SCHEMA,
        ArosCompilerIdentity::Llvm { .. } => super::LLVM_V2_COMPATIBILITY_RECEIPT_SCHEMA,
    };
    if document.schema != expected_schema || document.package.is_none() {
        return Err(readback_error(
            "native compatibility receipt schema does not match the expected compiler family",
        ));
    }
    validate_receipt_package(&document, request)?;
    validate_receipt_upstream(&document, request)?;
    validate_receipt_ports(&document, request.ports_sources)?;
    validate_receipt_standalone(&document, request)?;
    validate_phase_readbacks(&document, request)?;

    let phases = super::super::REQUIRED_PROBE_PHASES.into_iter().collect();
    Ok(NativeCompatibilityReceiptReadback {
        receipt_sha256: sha256_bytes(request.receipt_bytes),
        phases,
    })
}

fn validate_expectations(
    request: &NativeCompatibilityReceiptReadbackRequest<'_>,
) -> Result<(), ContractError> {
    let manifest = request.package.manifest;
    manifest.validate().map_err(|_| {
        readback_error("expected package manifest is not a valid closed v2 manifest")
    })?;
    if manifest.schema != 2
        || request.package.archive_size == 0
        || request
            .package
            .compiler
            .validate_for_target(&manifest.target_triple)
            .is_err()
    {
        return Err(readback_error(
            "expected package is not a valid measured compiler-family v2 identity",
        ));
    }
    let manifest_compiler = manifest
        .compiler_identity()
        .map_err(|_| readback_error("expected package manifest has no valid compiler identity"))?;
    if &manifest_compiler != request.package.compiler
        || manifest.host != request.package.host
        || manifest.source_commit != request.package.source_commit.as_str()
    {
        return Err(readback_error(
            "expected package compiler, host, or package source commit differs from its manifest",
        ));
    }
    GitObjectId::try_from(manifest.source_commit.clone())
        .map_err(|_| readback_error("expected package source commit is malformed"))?;
    let profiles_selection = request
        .profiles
        .select(request.profile.name())
        .map_err(|_| readback_error("expected profile is absent from its profiles document"))?;
    if request.profile.document_sha256() != profiles_selection.document_sha256()
        || request.profile.name() != profiles_selection.name()
        || request.profile.target_triple() != profiles_selection.target_triple()
        || request.profile.family() != request.profiles.family()
        || manifest.profiles_sha256 != request.profile.document_sha256().as_str()
        || manifest.target_profile != request.profile.name()
        || manifest.target_triple != request.profile.target_triple()
        || family_for_profile(request.profile.family()) != request.package.compiler.family()
    {
        return Err(readback_error(
            "expected package manifest is not bound to the selected profiles document and profile",
        ));
    }
    if request.profiles.upstream_commit() != request.upstream_source_commit {
        return Err(readback_error(
            "expected profile document does not select the expected upstream source commit",
        ));
    }
    if matches!(request.package.compiler, ArosCompilerIdentity::Llvm { .. })
        && request.cmake_build_required
    {
        return Err(readback_error(
            "LLVM compatibility cannot require the GNU source-native-consumer CMake build command",
        ));
    }
    let selected_compiler_matches = match request.package.compiler {
        ArosCompilerIdentity::Gnu { target, .. } => request.profile.target() == Some(target),
        ArosCompilerIdentity::Llvm { .. } => request.profile.target().is_none(),
    };
    if !selected_compiler_matches {
        return Err(readback_error(
            "expected compiler ABI differs from the selected profile target contract",
        ));
    }
    if request.engine_api_version == 0
        || request
            .helpers
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
            != crate::compatibility::REQUIRED_HELPERS
                .iter()
                .copied()
                .collect()
        || request.helpers.values().any(|helper| helper.size == 0)
    {
        return Err(readback_error(
            "expected engine or helper identity set is incomplete",
        ));
    }
    let expected_host_tools =
        crate::compatibility::native_compatibility_host_tools(request.package.host)?
            .into_iter()
            .collect::<BTreeSet<_>>();
    if request
        .host_tools
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>()
        != expected_host_tools
        || request.host_tools.values().any(|tool| tool.size == 0)
    {
        return Err(readback_error(
            "expected host-tool identity set is incomplete for the package host",
        ));
    }
    if request.standalone.targets.is_empty()
        || request.standalone.targets.len() > 2
        || request
            .standalone
            .targets
            .keys()
            .any(|triple| !identifier(triple))
    {
        return Err(readback_error(
            "expected standalone verification result has an invalid target set",
        ));
    }
    let expected_targets = expected_standalone_targets(request.profile, request.package.compiler);
    if request
        .standalone
        .targets
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>()
        != expected_targets
    {
        return Err(readback_error(
            "expected standalone target set differs from the selected compiler profile",
        ));
    }
    for (triple, target) in &request.standalone.targets {
        let expected_class = standalone_class_for_triple(triple).ok_or_else(|| {
            readback_error("expected standalone target has an unsupported ELF class")
        })?;
        if target.c.size == 0
            || target.cxx.size == 0
            || class_name(target.c.class) != expected_class
            || class_name(target.cxx.class) != expected_class
        {
            return Err(readback_error(
                "expected standalone verification result has an invalid ELF identity",
            ));
        }
    }
    validate_expected_ports(request.ports_sources)?;
    Ok(())
}

fn validate_receipt_package(
    document: &CompatibilityReceiptDocument,
    request: &NativeCompatibilityReceiptReadbackRequest<'_>,
) -> Result<(), ContractError> {
    let package = document
        .package
        .as_ref()
        .ok_or_else(|| readback_error("compiler-family receipt is missing its package binding"))?;
    let manifest = request.package.manifest;
    let manifest_value = serde_json::to_value(manifest)
        .map_err(|_| readback_error("cannot encode expected package manifest"))?;
    let manifest_bytes = crate::canonical::bytes(&manifest_value)
        .map_err(|_| readback_error("expected package manifest is not canonicalizable"))?;
    let manifest_sha256 = sha256_bytes(&manifest_bytes);
    let tree_sha256 = Sha256Digest::parse(&manifest.tree_sha256)
        .map_err(|_| readback_error("expected package tree digest is malformed"))?;
    let expected_preset = match request.package.compiler {
        ArosCompilerIdentity::Gnu { .. } => request.gnu_source_preset,
        ArosCompilerIdentity::Llvm { .. } => None,
    };
    if matches!(request.package.compiler, ArosCompilerIdentity::Gnu { .. })
        && expected_preset.is_none_or(|preset| !identifier(preset))
        || matches!(request.package.compiler, ArosCompilerIdentity::Llvm { .. })
            && request.gnu_source_preset.is_some()
    {
        return Err(readback_error(
            "expected GNU source preset is missing or LLVM unexpectedly selected one",
        ));
    }
    if &package.compiler != request.package.compiler
        || package.host != manifest.host
        || package.target_profile != request.profile.name()
        || package.target_triple != request.profile.target_triple()
        || &package.archive_sha256 != request.package.archive_sha256
        || package.archive_size != request.package.archive_size
        || package.manifest_sha256 != manifest_sha256
        || package.tree_sha256 != tree_sha256
        || package.source_tree_sha256 != *request.sdk_consumer_source_tree_sha256
        || package.source_preset.as_deref() != expected_preset
    {
        return Err(readback_error(
            "native compatibility receipt package, profile, preset, or SDK source binding differs from expected inputs",
        ));
    }
    Ok(())
}

fn validate_receipt_upstream(
    document: &CompatibilityReceiptDocument,
    request: &NativeCompatibilityReceiptReadbackRequest<'_>,
) -> Result<(), ContractError> {
    if document.upstream_source_commit != request.upstream_source_commit.as_str()
        || document.upstream_source_tree != request.upstream_source_tree.as_str()
    {
        return Err(readback_error(
            "native compatibility receipt upstream commit or tree differs from expected inputs",
        ));
    }
    Ok(())
}

fn validate_expected_ports(
    expected: &[NativeCompatibilityExpectedPortSource],
) -> Result<(), ContractError> {
    if expected.is_empty() || expected.len() > 128 {
        return Err(readback_error(
            "expected locked ports closure is empty or exceeds its input limit",
        ));
    }
    let mut ids = BTreeSet::new();
    let mut cache_filenames = BTreeSet::new();
    let mut paths = BTreeSet::new();
    let mut markers = BTreeSet::new();
    let mut previous_path = None;
    for source in expected {
        if !identifier(&source.id)
            || source.cache_filename.is_empty()
            || !safe_relative_path(&source.relative_path)
            || (!source.fetch_marker.is_empty() && !safe_fetch_marker_path(&source.fetch_marker))
            || source.size == 0
            || !ids.insert(source.id.as_str())
            || !cache_filenames.insert(source.cache_filename.as_str())
            || !paths.insert(source.relative_path.as_str())
            || (!source.fetch_marker.is_empty() && !markers.insert(source.fetch_marker.as_str()))
            || previous_path.is_some_and(|prior: &str| prior >= source.relative_path.as_str())
        {
            return Err(readback_error(
                "expected locked ports closure has an invalid, duplicate, or unordered record",
            ));
        }
        previous_path = Some(source.relative_path.as_str());
    }
    Ok(())
}

fn validate_receipt_ports(
    document: &CompatibilityReceiptDocument,
    expected: &[NativeCompatibilityExpectedPortSource],
) -> Result<(), ContractError> {
    let projected = expected
        .iter()
        .map(|source| CompatibilityReceiptPortsSource {
            id: source.id.clone(),
            cache_filename: source.cache_filename.clone(),
            relative_path: source.relative_path.clone(),
            fetch_marker: source.fetch_marker.clone(),
            sha256: source.sha256.clone(),
            size: source.size,
        })
        .collect::<Vec<_>>();
    if document.ports_sources != projected {
        return Err(readback_error(
            "native compatibility receipt ports closure differs from the exact locked payload closure",
        ));
    }
    Ok(())
}

fn validate_receipt_standalone(
    document: &CompatibilityReceiptDocument,
    request: &NativeCompatibilityReceiptReadbackRequest<'_>,
) -> Result<(), ContractError> {
    if document.standalone_targets.len() != request.standalone.targets.len() {
        return Err(readback_error(
            "native compatibility receipt standalone target set differs from verified outputs",
        ));
    }
    for (triple, expected) in &request.standalone.targets {
        let actual = document.standalone_targets.get(triple).ok_or_else(|| {
            readback_error("native compatibility receipt omits a verified standalone target")
        })?;
        if !artifact_matches(&actual.c, &expected.c)
            || !artifact_matches(&actual.cxx, &expected.cxx)
        {
            return Err(readback_error(
                "native compatibility receipt standalone ELF output claim differs from verified bytes",
            ));
        }
    }
    Ok(())
}

fn validate_phase_readbacks(
    document: &CompatibilityReceiptDocument,
    request: &NativeCompatibilityReceiptReadbackRequest<'_>,
) -> Result<(), ContractError> {
    let required = super::super::REQUIRED_PROBE_PHASES;
    let expected_phase_set = required.into_iter().collect::<BTreeSet<_>>();
    if request.phase_report_bytes.len() != required.len()
        || request
            .phase_report_bytes
            .keys()
            .copied()
            .collect::<BTreeSet<_>>()
            != expected_phase_set
        || request
            .command_logs
            .keys()
            .copied()
            .collect::<BTreeSet<_>>()
            != expected_phase_set
    {
        return Err(readback_error(
            "native compatibility read-back does not contain exactly six phase reports and log sets",
        ));
    }
    let receipt_phase_digests = document
        .phase_reports
        .iter()
        .map(|entry| (entry.phase, &entry.report_sha256))
        .collect::<BTreeMap<_, _>>();
    let mut total_log_bytes = 0usize;
    let mut parsed_reports = BTreeMap::new();
    for phase in required {
        let bytes = request
            .phase_report_bytes
            .get(&phase)
            .ok_or_else(|| readback_error("native compatibility read-back omits a phase report"))?;
        if bytes.is_empty() || bytes.len() > crate::canonical::MAX_DOCUMENT_BYTES {
            return Err(readback_error(
                "native compatibility phase report is empty or exceeds the document limit",
            ));
        }
        reject_duplicate_json_keys(bytes)?;
        let report = CompatibilityProbeReport::parse(bytes)?;
        require_canonical_json(&report, bytes, "native compatibility phase report")?;
        if report.phase != phase
            || receipt_phase_digests.get(&phase).copied() != Some(&sha256_bytes(bytes))
        {
            return Err(readback_error(
                "native compatibility phase report identity or exact-byte digest differs from the aggregate receipt",
            ));
        }
        if report.engine_api_version != request.engine_api_version
            || &report.engine_sha256 != request.engine_sha256
            || &report.source_tree_sha256 != request.sdk_consumer_source_tree_sha256
            || report.helpers != *request.helpers
        {
            return Err(readback_error(
                "native compatibility phase mixes engine, helper, or SDK source identities",
            ));
        }
        match phase {
            CompatibilityPhase::CmakeConsumer
            | CompatibilityPhase::UpstreamConfigure
            | CompatibilityPhase::UpstreamIncludes
            | CompatibilityPhase::UpstreamLinklibs
                if report.host_tools != *request.host_tools
                    || &report.environment_sha256 != request.sdk_environment_sha256 =>
            {
                return Err(readback_error(
                    "native compatibility SDK phase host-tool or environment identity differs from independent expectation",
                ));
            }
            CompatibilityPhase::StandaloneC | CompatibilityPhase::StandaloneCxx
                if !report.host_tools.is_empty()
                    || &report.environment_sha256 != request.standalone_environment_sha256 =>
            {
                return Err(readback_error(
                    "native compatibility standalone environment identity differs from independent expectation",
                ));
            }
            _ => {}
        }
        let expected_commands = match phase {
            CompatibilityPhase::CmakeConsumer => 1 + usize::from(request.cmake_build_required),
            CompatibilityPhase::UpstreamConfigure
            | CompatibilityPhase::UpstreamIncludes
            | CompatibilityPhase::UpstreamLinklibs => 1,
            CompatibilityPhase::StandaloneC | CompatibilityPhase::StandaloneCxx => {
                request.standalone.targets.len()
            }
        };
        if report.commands.len() != expected_commands {
            return Err(readback_error(
                "native compatibility phase has an unexpected command count",
            ));
        }
        let logs = request.command_logs.get(&phase).ok_or_else(|| {
            readback_error("native compatibility read-back omits retained command logs")
        })?;
        if logs.len() != expected_commands {
            return Err(readback_error(
                "native compatibility retained-log count differs from the reported command count",
            ));
        }
        for (command, log) in report.commands.iter().zip(logs) {
            if log.stdout.len() > super::super::MAX_RENDERED_LOG_BYTES
                || log.stderr.len() > super::super::MAX_RENDERED_LOG_BYTES
            {
                return Err(readback_error(
                    "native compatibility retained command log exceeds its configured bound",
                ));
            }
            total_log_bytes = total_log_bytes
                .checked_add(log.stdout.len())
                .and_then(|total| total.checked_add(log.stderr.len()))
                .ok_or_else(|| readback_error("native compatibility total log size overflowed"))?;
            if sha256_bytes(&log.stdout) != command.stdout_sha256
                || sha256_bytes(&log.stderr) != command.stderr_sha256
            {
                return Err(readback_error(
                    "native compatibility retained command log bytes differ from their report hashes",
                ));
            }
        }
        parsed_reports.insert(phase, report);
    }
    if total_log_bytes > MAX_RECEIPT_READBACK_LOG_BYTES {
        return Err(readback_error(
            "native compatibility total retained-log bytes exceed the aggregate bound",
        ));
    }
    validate_phase_environment_closure(&parsed_reports)?;
    Ok(())
}

fn validate_phase_environment_closure(
    reports: &BTreeMap<CompatibilityPhase, CompatibilityProbeReport>,
) -> Result<(), ContractError> {
    let first = reports
        .get(&CompatibilityPhase::CmakeConsumer)
        .ok_or_else(|| readback_error("native compatibility report set lost its CMake phase"))?;
    for phase in [
        CompatibilityPhase::UpstreamConfigure,
        CompatibilityPhase::UpstreamIncludes,
        CompatibilityPhase::UpstreamLinklibs,
    ] {
        let report = reports.get(&phase).ok_or_else(|| {
            readback_error("native compatibility report set lost an upstream phase")
        })?;
        if report.host_tools != first.host_tools
            || report.environment_sha256 != first.environment_sha256
        {
            return Err(readback_error(
                "native compatibility upstream phases do not share one measured host environment",
            ));
        }
    }
    let standalone_c = reports
        .get(&CompatibilityPhase::StandaloneC)
        .ok_or_else(|| readback_error("native compatibility report set lost standalone C"))?;
    let standalone_cxx = reports
        .get(&CompatibilityPhase::StandaloneCxx)
        .ok_or_else(|| readback_error("native compatibility report set lost standalone C++"))?;
    if !standalone_c.host_tools.is_empty()
        || !standalone_cxx.host_tools.is_empty()
        || standalone_c.environment_sha256 != standalone_cxx.environment_sha256
    {
        return Err(readback_error(
            "native compatibility standalone phases do not share the closed poisoned environment",
        ));
    }
    Ok(())
}

fn expected_standalone_targets<'a>(
    profile: &'a Profile,
    compiler: &ArosCompilerIdentity,
) -> BTreeSet<&'a str> {
    let mut targets = BTreeSet::from([profile.target_triple()]);
    if matches!(compiler, ArosCompilerIdentity::Llvm { .. }) && profile.name() == "pc-x86_64" {
        targets.insert("i386-unknown-aros");
    }
    targets
}

fn standalone_class_for_triple(triple: &str) -> Option<&'static str> {
    match triple.split_once('-').map_or(triple, |(cpu, _)| cpu) {
        "x86_64" | "aarch64" | "riscv64" => Some("elf64"),
        "i386" | "arm" | "riscv" => Some("elf32"),
        _ => None,
    }
}

const fn class_name(class: aros_common::elf::Class) -> &'static str {
    match class {
        aros_common::elf::Class::Elf32 => "elf32",
        aros_common::elf::Class::Elf64 => "elf64",
    }
}

const fn family_for_profile(family: CompilerFamily) -> &'static str {
    match family {
        CompilerFamily::Gnu => "gnu",
        CompilerFamily::Llvm => "llvm",
    }
}

fn artifact_matches(
    actual: &CompatibilityReceiptArtifact,
    expected: &StandaloneArtifactIdentity,
) -> bool {
    actual.sha256 == expected.sha256
        && actual.size == expected.size
        && actual.class == class_name(expected.class)
}

fn require_canonical_json<T: serde::Serialize>(
    parsed: &T,
    original: &[u8],
    label: &str,
) -> Result<(), ContractError> {
    let value = serde_json::to_value(parsed)
        .map_err(|_| readback_error(format!("cannot encode parsed {label}")))?;
    let encoded = crate::canonical::bytes(&value)
        .map_err(|_| readback_error(format!("{label} is not canonical JSON")))?;
    if encoded != original {
        return Err(readback_error(format!(
            "{label} bytes are not canonical JSON"
        )));
    }
    Ok(())
}

pub(super) fn reject_duplicate_json_keys(bytes: &[u8]) -> Result<(), ContractError> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    DuplicateKeyCheck { depth: 0 }
        .deserialize(&mut deserializer)
        .map_err(|_| {
            readback_error("native compatibility metadata is malformed or has duplicate keys")
        })?;
    deserializer
        .end()
        .map_err(|_| readback_error("native compatibility metadata has trailing or malformed JSON"))
}

struct DuplicateKeyCheck {
    depth: usize,
}

impl<'de> DeserializeSeed<'de> for DuplicateKeyCheck {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        if self.depth > 64 {
            return Err(serde::de::Error::custom(
                "JSON nesting exceeds the metadata bound",
            ));
        }
        deserializer.deserialize_any(DuplicateKeyVisitor { depth: self.depth })
    }
}

struct DuplicateKeyVisitor {
    depth: usize,
}

impl<'de> Visitor<'de> for DuplicateKeyVisitor {
    type Value = ();

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a JSON value with unique object keys")
    }

    fn visit_bool<E>(self, _value: bool) -> Result<(), E> {
        Ok(())
    }

    fn visit_i64<E>(self, _value: i64) -> Result<(), E> {
        Ok(())
    }

    fn visit_u64<E>(self, _value: u64) -> Result<(), E> {
        Ok(())
    }

    fn visit_f64<E>(self, _value: f64) -> Result<(), E> {
        Ok(())
    }

    fn visit_str<E>(self, _value: &str) -> Result<(), E> {
        Ok(())
    }

    fn visit_string<E>(self, _value: String) -> Result<(), E> {
        Ok(())
    }

    fn visit_unit<E>(self) -> Result<(), E> {
        Ok(())
    }

    fn visit_none<E>(self) -> Result<(), E> {
        Ok(())
    }

    fn visit_some<D>(self, deserializer: D) -> Result<(), D::Error>
    where
        D: Deserializer<'de>,
    {
        DuplicateKeyCheck {
            depth: self.depth + 1,
        }
        .deserialize(deserializer)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<(), A::Error>
    where
        A: SeqAccess<'de>,
    {
        while sequence
            .next_element_seed(DuplicateKeyCheck {
                depth: self.depth + 1,
            })?
            .is_some()
        {}
        Ok(())
    }

    fn visit_map<A>(self, mut object: A) -> Result<(), A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut keys = BTreeSet::new();
        while let Some(key) = object.next_key::<String>()? {
            if !keys.insert(key) {
                return Err(serde::de::Error::custom("duplicate JSON object key"));
            }
            object.next_value_seed(DuplicateKeyCheck {
                depth: self.depth + 1,
            })?;
        }
        Ok(())
    }
}

fn readback_error(message: impl Into<String>) -> ContractError {
    ContractError::compatibility(message)
}
