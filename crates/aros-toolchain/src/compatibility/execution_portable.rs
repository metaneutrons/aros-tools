//! Portable retained compatibility bytes, separate from execution authentication.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use aros_common::{open_regular_file_nofollow, sha256_bytes, Sha256Digest};
use serde::{Deserialize, Serialize};

use super::retained::ExpectedRetainedEvidence;
use super::{
    readback_native_compatibility_receipt, standalone_triples, NativeCompatibilityCommandLogs,
    NativeCompatibilityExpectedPackage, NativeCompatibilityExpectedPortSource,
    NativeCompatibilityReceiptReadback, NativeCompatibilityReceiptReadbackRequest,
    NativeCompatibilityRequest,
};
use crate::compatibility::{
    verify_standalone_outputs_with_compilers, CompatibilityHelperReport,
    CompatibilityHostToolReport, CompatibilityPhase, StandaloneOutputReport,
    StandaloneOutputRequest, StandaloneTargetArtifacts,
};
use crate::profiles::{Profile, Profiles};
use crate::recipe::GitObjectId;
use crate::ContractError;

/// Protected evidence manifest, not a public toolchain release asset.
pub const PORTABLE_NATIVE_COMPATIBILITY_MANIFEST: &str = "compatibility-measurement.json";
const SCHEMA: &str = "aros-toolchain-compatibility-measurement-v2";
const SDK_SCHEMA: &str = "aros-toolchain-compatibility-measurement-v3";
const MAX_ELF_BYTES: usize = super::super::standalone::MAX_STANDALONE_ARTIFACT_BYTES as usize;

/// Independently retained expectations for one compatibility lane.
///
/// These values must come from selected, verified package/profile/source/runtime
/// inputs and separately authenticated owning-job observations. Do not populate
/// them by copying the receipt, export manifest or package environment claims.
/// This declaration does not itself authenticate those selections.
#[derive(Debug, Clone)]
pub struct NativeCompatibilityReceiptExpectations<'a> {
    /// Measured package identity from independent package verification.
    pub package: NativeCompatibilityExpectedPackage<'a>,
    /// Selected source-owned profile matrix.
    pub profiles: &'a Profiles,
    /// Selected lane profile, not a report-selected profile.
    pub profile: &'a Profile,
    /// Source-owned GNU consumer preset; None for LLVM.
    pub gnu_source_preset: Option<&'a str>,
    /// CMake build requirement derived from the selected source contract.
    pub cmake_build_required: bool,
    /// Exact SDK requirement and probe/compiler/path identities measured from
    /// the selected source contract before any compatibility phase executes.
    pub native_sdk: Option<&'a crate::compatibility::NativeSdkLinkSelection>,
    /// Independently measured engine-free SDK consumer source tree.
    pub sdk_consumer_source_tree_sha256: &'a Sha256Digest,
    /// Selected tools engine API version.
    pub engine_api_version: u32,
    /// Selected tools engine content digest.
    pub engine_sha256: &'a Sha256Digest,
    /// Measured required helper identities from the selected tools runtime.
    pub helpers: &'a BTreeMap<String, CompatibilityHelperReport>,
    /// Measured host tool identities from the owning job's selected closure.
    pub host_tools: &'a BTreeMap<String, CompatibilityHostToolReport>,
    /// Independently retained SDK environment identity.
    pub sdk_environment_sha256: &'a Sha256Digest,
    /// Independently retained standalone environment identity.
    pub standalone_environment_sha256: &'a Sha256Digest,
    /// Profile-bound pristine upstream commit.
    pub upstream_source_commit: &'a GitObjectId,
    /// Independently measured pristine upstream tree identity.
    pub upstream_source_tree: &'a GitObjectId,
    /// Selected locked ports closure in stable relative-path order.
    pub ports_sources: &'a [NativeCompatibilityExpectedPortSource],
}

impl NativeCompatibilityReceiptExpectations<'_> {
    pub(super) fn readback_documents(
        &self,
        receipt: &[u8],
        reports: &BTreeMap<CompatibilityPhase, Vec<u8>>,
        logs: &BTreeMap<CompatibilityPhase, Vec<NativeCompatibilityCommandLogs>>,
        standalone: &StandaloneOutputReport,
        native_sdk: Option<&crate::compatibility::NativeSdkPortableProof>,
    ) -> Result<NativeCompatibilityReceiptReadback, ContractError> {
        readback_native_compatibility_receipt(&NativeCompatibilityReceiptReadbackRequest {
            receipt_bytes: receipt,
            phase_report_bytes: reports,
            command_logs: logs,
            package: self.package.clone(),
            profiles: self.profiles,
            profile: self.profile,
            gnu_source_preset: self.gnu_source_preset,
            cmake_build_required: self.cmake_build_required,
            native_sdk_required: self.native_sdk.is_some(),
            native_sdk,
            sdk_consumer_source_tree_sha256: self.sdk_consumer_source_tree_sha256,
            engine_api_version: self.engine_api_version,
            engine_sha256: self.engine_sha256,
            helpers: self.helpers,
            host_tools: self.host_tools,
            sdk_environment_sha256: self.sdk_environment_sha256,
            standalone_environment_sha256: self.standalone_environment_sha256,
            upstream_source_commit: self.upstream_source_commit,
            upstream_source_tree: self.upstream_source_tree,
            ports_sources: self.ports_sources,
            standalone,
        })
    }
}

/// Downloaded closed evidence directory and externally selected raw manifest.
#[derive(Debug, Clone)]
pub struct PortableNativeCompatibilityRequest<'a> {
    /// Absolute real directory of only the export's flat regular files.
    pub directory: PathBuf,
    /// Exact manifest digest selected independently of downloaded metadata.
    pub manifest_sha256: Sha256Digest,
    /// Independent lane expectations; original runner roots are not required.
    pub expected: NativeCompatibilityReceiptExpectations<'a>,
}

/// Complete owning-host export constructed only after retained read-back.
///
/// Upload each returned file unchanged as protected evidence, not as a public
/// release asset. Binary ELF and log bytes remain separate regular files: they
/// are not encoded as huge JSON arrays or reopened through foreign host paths.
/// Success verifies local consistency, not authenticated execution or release
/// qualification. The caller exclusively owns quiescent inputs.
#[derive(Debug)]
pub struct PortableNativeCompatibilityExport {
    files: BTreeMap<String, Vec<u8>>,
    manifest_sha256: Sha256Digest,
    readback: NativeCompatibilityReceiptReadback,
}

impl PortableNativeCompatibilityExport {
    /// Closed file set, including its manifest, in canonical filename order.
    pub fn files(&self) -> impl Iterator<Item = (&str, &[u8])> {
        self.files
            .iter()
            .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
    }

    /// Raw digest to retain outside the uploaded export's metadata.
    #[must_use]
    pub const fn manifest_sha256(&self) -> &Sha256Digest {
        &self.manifest_sha256
    }

    /// Aggregate receipt identity measured from the original exact bytes.
    #[must_use]
    pub const fn receipt_sha256(&self) -> &Sha256Digest {
        &self.readback.receipt_sha256
    }
}

/// Complete rootless byte/ELF read-back; not release admission or provenance.
#[derive(Debug)]
pub struct PortableNativeCompatibilityReadback {
    manifest_sha256: Sha256Digest,
    readback: NativeCompatibilityReceiptReadback,
    standalone: StandaloneOutputReport,
    native_sdk: Option<crate::compatibility::NativeSdkPortableProof>,
}

impl PortableNativeCompatibilityReadback {
    /// Exact selected manifest digest, joined to every measured member.
    #[must_use]
    pub const fn manifest_sha256(&self) -> &Sha256Digest {
        &self.manifest_sha256
    }

    /// Six-phase receipt/log consistency result.
    #[must_use]
    pub const fn receipt(&self) -> &NativeCompatibilityReceiptReadback {
        &self.readback
    }

    /// Actual downloaded C/C++ ELF identities, not receipt-only claims.
    #[must_use]
    pub const fn standalone(&self) -> &StandaloneOutputReport {
        &self.standalone
    }

    /// Complete ordinary SDK proof required by a selected consumer-v2 source.
    /// Its presence is not execution authentication or publication authority.
    #[must_use]
    pub const fn native_sdk(&self) -> Option<&crate::compatibility::NativeSdkPortableProof> {
        self.native_sdk.as_ref()
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MeasurementDocument {
    schema: String,
    files: BTreeMap<String, MemberIdentity>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MemberIdentity {
    sha256: Sha256Digest,
    size: u64,
}

/// Export complete retained compiler-family compatibility evidence.
///
/// Independently revalidates local inputs and all six phases, snapshots their
/// exact reports/logs and actual standalone ELF files, then repeats the join
/// and acquisition before returning an opaque closed file set. Original roots
/// must still exist here. No output is written and no compatibility phase is
/// executed. Local validation can run bounded offline Git/Python probes and
/// use temporary sealed host-generator inputs, as the retained reader does.
///
/// The caller must exclusively own all roots. Repeated observations are not
/// an atomic snapshot, a concurrent-writer lock or proof of real execution.
/// This boundary neither authenticates runtime/jobs nor qualifies a release.
///
/// # Errors
/// Returns AX0703 for legacy/incomplete evidence, changed inputs/bytes, unsafe
/// aliases, wrong ELF contracts or receipt/report/log inconsistencies.
pub fn export_retained_native_compatibility(
    request: &NativeCompatibilityRequest,
    profiles: &Profiles,
) -> Result<PortableNativeCompatibilityExport, ContractError> {
    if !super::package_binding_required(request) {
        return Err(error(
            "portable compatibility requires a compiler-family-v2 package",
        ));
    }
    let prepared = ExpectedRetainedEvidence::prepare(request, profiles)?;
    export_with_expectations(request, profiles, &prepared)
}

pub(super) fn export_with_expectations(
    request: &NativeCompatibilityRequest,
    profiles: &Profiles,
    prepared: &ExpectedRetainedEvidence,
) -> Result<PortableNativeCompatibilityExport, ContractError> {
    let local = prepared.readback(request, profiles)?;
    let expected = prepared.portable_expectations(request, profiles)?;
    let reports = super::super::checked_directory(&request.reports_root, "retained reports")?;
    let outputs = super::super::checked_directory(
        &request.standalone_output_root,
        "retained standalone output root",
    )?;
    let shape = Shape::new(&reports, &outputs, &expected)?;
    let files = acquire_members(&shape, None, BTreeSet::new())?;
    let standalone = verify_standalone_outputs_with_compilers(&shape.standalone, &shape.compilers)?;
    verify_snapshot_elf_identities(&files, &shape, &standalone)?;
    let native_sdk = readback_sdk_snapshot(&files, &expected)?;
    let joined = readback_snapshot(&files, &shape, &expected, &standalone, native_sdk.as_ref())?;
    if local != joined || prepared.readback(request, profiles)? != joined {
        return Err(error(
            "retained compatibility changed during portable export",
        ));
    }
    verify_members_unchanged(&shape, &files, BTreeSet::new())?;
    let document = MeasurementDocument {
        schema: measurement_schema(&expected).into(),
        files: files
            .iter()
            .map(|(name, bytes)| {
                (
                    name.clone(),
                    MemberIdentity {
                        sha256: sha256_bytes(bytes),
                        size: bytes.len() as u64,
                    },
                )
            })
            .collect(),
    };
    let manifest = manifest_bytes(&document)?;
    let manifest_sha256 = sha256_bytes(&manifest);
    let mut files = files;
    files.insert(PORTABLE_NATIVE_COMPATIBILITY_MANIFEST.into(), manifest);
    Ok(PortableNativeCompatibilityExport {
        files,
        manifest_sha256,
        readback: joined,
    })
}

/// Read a downloaded compatibility export without reopening original host roots.
///
/// The exact externally selected manifest digest is checked before parsing.
/// The closed inventory and every bounded member are measured without following
/// links; file aliases are rejected. Downloaded standalone objects are parsed
/// against independent compiler contracts, then the existing six-phase reader
/// joins exact reports/logs to independent source/runtime/package expectations.
/// A second complete acquisition and ELF check is required before returning.
///
/// This read-only boundary does not execute a compiler, authenticate jobs or
/// attestations, prove relocation execution, verify package archive bytes, or
/// admit publication. A release collector must separately join `expected.package`
/// to its actual verified indexed package and authenticate evidence origin.
/// Callers exclusively own quiescent files; no atomic snapshot is promised.
///
/// # Errors
/// Returns AX0703 for malformed, missing, oversized, aliased, changed or
/// inconsistent files or independently selected expectations.
pub fn readback_portable_native_compatibility(
    request: &PortableNativeCompatibilityRequest<'_>,
) -> Result<PortableNativeCompatibilityReadback, ContractError> {
    validate_root(&request.directory)?;
    let shape = Shape::new(&request.directory, &request.directory, &request.expected)?;
    let expected_inventory: BTreeSet<_> = shape
        .members
        .keys()
        .cloned()
        .chain(std::iter::once(
            PORTABLE_NATIVE_COMPATIBILITY_MANIFEST.into(),
        ))
        .collect();
    require_inventory(&request.directory, &expected_inventory)?;
    let mut identities = BTreeSet::new();
    let manifest_path = request
        .directory
        .join(PORTABLE_NATIVE_COMPATIBILITY_MANIFEST);
    let manifest = read_member(
        &manifest_path,
        crate::canonical::MAX_DOCUMENT_BYTES,
        &mut identities,
    )?;
    if sha256_bytes(&manifest) != request.manifest_sha256 {
        return Err(error(
            "portable compatibility manifest differs from its selected raw digest",
        ));
    }
    super::receipt_readback::reject_duplicate_json_keys(&manifest)?;
    let document: MeasurementDocument = serde_json::from_slice(&manifest)
        .map_err(|_| error("portable compatibility manifest is not closed JSON"))?;
    if document.schema != measurement_schema(&request.expected)
        || !document.files.keys().eq(shape.members.keys())
        || manifest_bytes(&document)? != manifest
    {
        return Err(error(
            "portable compatibility manifest differs from its canonical closed inventory",
        ));
    }
    let files = acquire_members(&shape, Some(&document), identities)?;
    let standalone = verify_standalone_outputs_with_compilers(&shape.standalone, &shape.compilers)?;
    verify_snapshot_elf_identities(&files, &shape, &standalone)?;
    let native_sdk = readback_sdk_snapshot(&files, &request.expected)?;
    let joined = readback_snapshot(
        &files,
        &shape,
        &request.expected,
        &standalone,
        native_sdk.as_ref(),
    )?;
    validate_root(&request.directory)?;
    require_inventory(&request.directory, &expected_inventory)?;
    let mut repeated_identities = BTreeSet::new();
    if read_member(
        &manifest_path,
        crate::canonical::MAX_DOCUMENT_BYTES,
        &mut repeated_identities,
    )? != manifest
    {
        return Err(error(
            "portable compatibility evidence changed during read-back",
        ));
    }
    verify_members_unchanged(&shape, &files, repeated_identities)?;
    if verify_standalone_outputs_with_compilers(&shape.standalone, &shape.compilers)? != standalone
    {
        return Err(error(
            "portable compatibility ELF outputs changed during read-back",
        ));
    }
    require_inventory(&request.directory, &expected_inventory)?;
    Ok(PortableNativeCompatibilityReadback {
        manifest_sha256: request.manifest_sha256.clone(),
        readback: joined,
        standalone,
        native_sdk,
    })
}

struct Shape {
    members: BTreeMap<String, (PathBuf, usize)>,
    phases: BTreeMap<CompatibilityPhase, super::super::ProbeReportPaths>,
    standalone: StandaloneOutputRequest,
    compilers: BTreeMap<String, aros_common::ArosCompilerIdentity>,
}

impl Shape {
    fn new(
        reports: &Path,
        outputs: &Path,
        expected: &NativeCompatibilityReceiptExpectations<'_>,
    ) -> Result<Self, ContractError> {
        let targets: BTreeMap<_, _> = standalone_triples(expected.profile)?
            .into_iter()
            .map(|triple| {
                let cpu = triple
                    .split_once('-')
                    .map_or(triple.as_str(), |(cpu, _)| cpu);
                let artifacts = StandaloneTargetArtifacts {
                    c: outputs.join(format!("c-{cpu}.o")),
                    cxx: outputs.join(format!("cxx-{cpu}.o")),
                };
                (triple, artifacts)
            })
            .collect();
        let compilers = targets
            .keys()
            .map(|triple| (triple.clone(), expected.package.compiler.clone()))
            .collect();
        let mut shape = Self {
            members: BTreeMap::new(),
            phases: BTreeMap::new(),
            standalone: StandaloneOutputRequest {
                output_root: outputs.into(),
                targets,
            },
            compilers,
        };
        shape.insert(
            reports.join(super::COMPATIBILITY_RECEIPT_FILE),
            crate::canonical::MAX_DOCUMENT_BYTES,
        )?;
        for phase in super::super::REQUIRED_PROBE_PHASES {
            let count = match phase {
                CompatibilityPhase::CmakeConsumer => 1 + usize::from(expected.cmake_build_required),
                CompatibilityPhase::StandaloneC | CompatibilityPhase::StandaloneCxx => {
                    shape.standalone.targets.len()
                }
                _ => 1,
            };
            let paths = super::super::ProbeReportPaths::new(reports, phase, count);
            shape.insert(paths.report.clone(), crate::canonical::MAX_DOCUMENT_BYTES)?;
            for command in &paths.commands {
                shape.insert(command.stdout.clone(), super::super::MAX_RENDERED_LOG_BYTES)?;
                shape.insert(command.stderr.clone(), super::super::MAX_RENDERED_LOG_BYTES)?;
            }
            shape.phases.insert(phase, paths);
        }
        let objects: Vec<_> = shape
            .standalone
            .targets
            .values()
            .flat_map(|target| [target.c.clone(), target.cxx.clone()])
            .collect();
        for object in objects {
            shape.insert(object, MAX_ELF_BYTES)?;
        }
        if expected.native_sdk.is_some() {
            for name in crate::compatibility::native_sdk_portable::expected_member_names() {
                let path = if reports == outputs {
                    reports.join(&name)
                } else {
                    sdk_member_path(&reports.join("native-sdk-links"), &name)?
                };
                let limit = sdk_member_limit(&name)?;
                if shape.members.insert(name, (path, limit)).is_some() {
                    return Err(error("portable SDK and compatibility filenames collide"));
                }
            }
        }
        Ok(shape)
    }

    fn insert(&mut self, path: PathBuf, limit: usize) -> Result<(), ContractError> {
        if self
            .members
            .insert(filename(&path)?, (path, limit))
            .is_some()
        {
            return Err(error("portable compatibility output filenames collide"));
        }
        Ok(())
    }
}

fn acquire_members(
    shape: &Shape,
    document: Option<&MeasurementDocument>,
    mut identities: BTreeSet<(u64, u64)>,
) -> Result<BTreeMap<String, Vec<u8>>, ContractError> {
    shape
        .members
        .iter()
        .map(|(name, (path, limit))| {
            let bytes = read_member(path, *limit, &mut identities)?;
            if let Some(document) = document {
                let identity = document
                    .files
                    .get(name)
                    .ok_or_else(|| error("portable compatibility member is absent"))?;
                if bytes.len() as u64 != identity.size || sha256_bytes(&bytes) != identity.sha256 {
                    return Err(error(
                        "portable compatibility member differs from its selected manifest",
                    ));
                }
            }
            Ok((name.clone(), bytes))
        })
        .collect()
}

// Compare one bounded member at a time instead of retaining a second complete
// ELF closure. The initial snapshot and at most one additional ELF buffer live
// concurrently; a PC profile's four 128 MiB limits are not doubled wholesale.
fn verify_members_unchanged(
    shape: &Shape,
    files: &BTreeMap<String, Vec<u8>>,
    mut identities: BTreeSet<(u64, u64)>,
) -> Result<(), ContractError> {
    for (name, (path, limit)) in &shape.members {
        let selected = files
            .get(name)
            .ok_or_else(|| error("portable compatibility snapshot is incomplete"))?;
        if read_member(path, *limit, &mut identities)? != *selected {
            return Err(error(
                "portable compatibility evidence bytes changed during read-back",
            ));
        }
    }
    Ok(())
}

fn readback_snapshot(
    files: &BTreeMap<String, Vec<u8>>,
    shape: &Shape,
    expected: &NativeCompatibilityReceiptExpectations<'_>,
    standalone: &StandaloneOutputReport,
    native_sdk: Option<&crate::compatibility::NativeSdkPortableProof>,
) -> Result<NativeCompatibilityReceiptReadback, ContractError> {
    let mut reports = BTreeMap::new();
    let mut logs = BTreeMap::new();
    for (phase, paths) in &shape.phases {
        reports.insert(*phase, member(files, &paths.report)?.to_vec());
        logs.insert(
            *phase,
            paths
                .commands
                .iter()
                .map(|command| {
                    Ok(NativeCompatibilityCommandLogs {
                        stdout: member(files, &command.stdout)?.to_vec(),
                        stderr: member(files, &command.stderr)?.to_vec(),
                    })
                })
                .collect::<Result<Vec<_>, ContractError>>()?,
        );
    }
    expected.readback_documents(
        files
            .get(super::COMPATIBILITY_RECEIPT_FILE)
            .ok_or_else(|| error("portable compatibility receipt is absent"))?,
        &reports,
        &logs,
        standalone,
        native_sdk,
    )
}

const fn measurement_schema(expected: &NativeCompatibilityReceiptExpectations<'_>) -> &'static str {
    if expected.native_sdk.is_some() {
        SDK_SCHEMA
    } else {
        SCHEMA
    }
}

fn readback_sdk_snapshot(
    files: &BTreeMap<String, Vec<u8>>,
    expected: &NativeCompatibilityReceiptExpectations<'_>,
) -> Result<Option<crate::compatibility::NativeSdkPortableProof>, ContractError> {
    let Some(selection) = expected.native_sdk else {
        return Ok(None);
    };
    let sdk_files = files
        .iter()
        .filter(|(name, _)| name.starts_with("sdk-"))
        .map(|(name, bytes)| (name.clone(), bytes.as_slice()))
        .collect();
    crate::compatibility::native_sdk_portable::readback(
        &sdk_files,
        selection,
        &crate::compatibility::native_sdk_portable::NativeSdkPortableExpectations {
            source_tree_sha256: expected.sdk_consumer_source_tree_sha256,
            engine_api_version: expected.engine_api_version,
            engine_sha256: expected.engine_sha256,
            helpers: expected.helpers,
            compiler: expected.package.compiler,
            source_profile: expected
                .gnu_source_preset
                .ok_or_else(|| error("portable SDK requirement lost its source preset"))?,
        },
    )
    .map(Some)
}

pub(super) fn read_sdk_members(root: &Path) -> Result<BTreeMap<String, Vec<u8>>, ContractError> {
    let mut aliases = BTreeSet::new();
    crate::compatibility::native_sdk_portable::expected_member_names()
        .into_iter()
        .map(|name| {
            let bytes = read_member(
                &sdk_member_path(root, &name)?,
                sdk_member_limit(&name)?,
                &mut aliases,
            )?;
            Ok((name, bytes))
        })
        .collect()
}

fn sdk_member_path(root: &Path, name: &str) -> Result<PathBuf, ContractError> {
    let name = name
        .strip_prefix("sdk-")
        .ok_or_else(|| error("SDK evidence filename lacks its namespace"))?;
    if name.starts_with("standalone-") {
        Ok(root.join("reports").join(name))
    } else {
        Ok(root.join(name))
    }
}

fn sdk_member_limit(name: &str) -> Result<usize, ContractError> {
    if name == "sdk-native-sdk-inventory.json" {
        return Ok(16 * 1024 * 1024);
    }
    match Path::new(name)
        .extension()
        .and_then(std::ffi::OsStr::to_str)
    {
        Some("elf") => Ok(MAX_ELF_BYTES),
        Some("map") => Ok(16 * 1024 * 1024),
        Some("log") => Ok(super::super::MAX_RENDERED_LOG_BYTES),
        Some("json") => Ok(crate::canonical::MAX_DOCUMENT_BYTES),
        _ => Err(error("SDK evidence filename has no closed byte bound")),
    }
}

fn verify_snapshot_elf_identities(
    files: &BTreeMap<String, Vec<u8>>,
    shape: &Shape,
    report: &StandaloneOutputReport,
) -> Result<(), ContractError> {
    for (triple, artifacts) in &shape.standalone.targets {
        let measured = report
            .targets
            .get(triple)
            .ok_or_else(|| error("portable standalone target is absent"))?;
        for (path, identity) in [(&artifacts.c, &measured.c), (&artifacts.cxx, &measured.cxx)] {
            let bytes = member(files, path)?;
            if sha256_bytes(bytes) != identity.sha256 || bytes.len() as u64 != identity.size {
                return Err(error(
                    "portable standalone snapshot differs from parsed ELF bytes",
                ));
            }
        }
    }
    Ok(())
}

fn member<'a>(
    files: &'a BTreeMap<String, Vec<u8>>,
    path: &Path,
) -> Result<&'a [u8], ContractError> {
    files
        .get(&filename(path)?)
        .map(Vec::as_slice)
        .ok_or_else(|| error("portable compatibility snapshot is incomplete"))
}

fn filename(path: &Path) -> Result<String, ContractError> {
    path.file_name()
        .and_then(|value| value.to_str())
        .map(str::to_owned)
        .ok_or_else(|| error("portable compatibility member has no UTF-8 filename"))
}

fn manifest_bytes(document: &MeasurementDocument) -> Result<Vec<u8>, ContractError> {
    let value = serde_json::to_value(document)
        .map_err(|_| error("cannot encode portable compatibility manifest"))?;
    crate::canonical::bytes(&value)
        .map_err(|_| error("portable compatibility manifest exceeds its canonical document bound"))
}

fn validate_root(root: &Path) -> Result<(), ContractError> {
    crate::filesystem::open_directory(root).map_err(|_| {
        error("portable compatibility root must be an absolute real directory without links")
    })?;
    if root
        .canonicalize()
        .map_err(|_| error("cannot canonicalize portable compatibility root"))?
        != root
    {
        return Err(error("portable compatibility root must be canonical"));
    }
    Ok(())
}

fn require_inventory(root: &Path, expected: &BTreeSet<String>) -> Result<(), ContractError> {
    let mut found = BTreeSet::new();
    for entry in fs::read_dir(root)
        .map_err(|_| error("cannot enumerate portable compatibility inventory"))?
    {
        let entry = entry.map_err(|_| error("cannot inspect portable compatibility member"))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| error("portable compatibility member name is not UTF-8"))?;
        if found.len() >= expected.len() || !expected.contains(&name) || !found.insert(name) {
            return Err(error(
                "portable compatibility directory differs from its exact inventory",
            ));
        }
    }
    if &found != expected {
        return Err(error(
            "portable compatibility directory differs from its exact inventory",
        ));
    }
    Ok(())
}

fn read_member(
    path: &Path,
    limit: usize,
    identities: &mut BTreeSet<(u64, u64)>,
) -> Result<Vec<u8>, ContractError> {
    let mut file = open_regular_file_nofollow(path)
        .map_err(|_| error("cannot safely open portable compatibility member"))?;
    let before = file
        .metadata()
        .map_err(|_| error("cannot inspect portable compatibility member"))?;
    if !before.is_file()
        || before.nlink() != 1
        || before.len() > limit as u64
        || !identities.insert((before.dev(), before.ino()))
    {
        return Err(error(
            "portable compatibility member is oversized, non-regular or physically aliased",
        ));
    }
    let mut bytes = Vec::with_capacity(before.len() as usize);
    (&mut file)
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| error("cannot read portable compatibility member"))?;
    let after = file
        .metadata()
        .map_err(|_| error("cannot remeasure portable compatibility member"))?;
    if before.len() != after.len()
        || bytes.len() as u64 != before.len()
        || before.mtime() != after.mtime()
        || before.mtime_nsec() != after.mtime_nsec()
        || before.ctime() != after.ctime()
        || before.ctime_nsec() != after.ctime_nsec()
        || after.nlink() != 1
    {
        return Err(error("portable compatibility member changed while read"));
    }
    Ok(bytes)
}

fn error(message: &str) -> ContractError {
    ContractError::compatibility(message)
}
