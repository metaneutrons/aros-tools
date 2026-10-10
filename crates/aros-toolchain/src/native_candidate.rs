//! Complete finished native-candidate evidence, separate from execution provenance.
//!
//! Collector-only phase outputs remain useful local checkpoints, but cannot
//! bind the complete compiler payload. This boundary measures the finished
//! tree (including links, directories and raw modes) and its six retained
//! phase receipts against independently selected inputs and receipt digests.
//! A self-digest is not authentication or proof that commands executed.

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use aros_common::{
    measure_regular_file_bounded, measure_tree_content_cas_bounded, sha256_bytes,
    ArosCompilerIdentity, Sha256Digest, TreeContentCas, TreeTraversalLimits,
};
use serde::{Deserialize, Serialize};

use crate::package::{PackageOutput, PackageRequest};
use crate::package_verify::VerifiedPackage;
use crate::plan::Identity;
use crate::profiles::Profile;
use crate::recipe::GitObjectId;
use crate::source_lock::SourceLock;
use crate::{canonical, ContractError, Recipe};

const SCHEMA: &str = "aros-toolchain-finished-candidate-v2";
const RECEIPT_FILE: &str = "finished-candidate.json";
mod build_result;
pub use build_result::{readback_finished_build_result, FinishedBuildResultRequest};
mod comparison;
pub use comparison::{compare_finished_candidate_packages, FinishedCandidateComparison};
mod portable;
pub use portable::{
    export_finished_package_measurement, readback_finished_package_measurement,
    FinishedPackageMeasurement, PortableFinishedPackageReadback, PortableFinishedPackageRequest,
};
mod release_readback;
pub use release_readback::{
    readback_release_builds_v2, ReleaseBuildLaneReadbackV2, ReleaseBuildLaneRequestV2,
    ReleaseBuildReadbackRequestV2, ReleaseBuildReadbackV2, ReleaseBuildSideRequestV2,
};
mod release_compatibility;
pub use release_compatibility::{
    readback_release_compatibility_v2, ReleaseCompatibilityLaneReadbackV2,
    ReleaseCompatibilityReadbackRequestV2, ReleaseCompatibilityReadbackV2,
};
#[cfg(test)]
mod comparison_tests;
#[cfg(test)]
mod release_compatibility_tests;
#[cfg(test)]
mod release_readback_tests;

#[cfg(test)]
mod comparison_guard_tests;
#[cfg(test)]
mod portable_tests;
#[cfg(test)]
mod tests;
pub(crate) const PHASES: [&str; 6] = [
    "preflight",
    "environment",
    "configure",
    "compiler",
    "collector",
    "publish",
];

/// Independently selected inputs and original roots for one finished candidate.
///
/// Receipt digests must come from separately retained build evidence, not be
/// inferred from whichever receipts happen to exist in the selected directory.
/// The original host roots must remain exclusively owned while being checked.
#[derive(Debug)]
pub struct FinishedCandidateRequest<'a> {
    /// Original native work root, not its `native-lifecycle` subdirectory.
    pub work_dir: &'a Path,
    /// Original output root containing the finished `toolchain` directory.
    pub output_dir: &'a Path,
    /// Validated recipe selected independently of retained receipts.
    pub recipe: &'a Recipe,
    /// Exact recipe-bound compiler-family lock.
    pub source_lock: &'a SourceLock,
    /// Exact recipe-bound selected profile.
    pub profile: &'a Profile,
    /// Expected native identity, including independently selected executor observation.
    pub identity: &'a Identity,
    /// Expected self-digests, in preflight/environment/configure/compiler/collector/publish order.
    pub phase_receipt_digests: &'a [Sha256Digest; 6],
    /// Independently expected self-digest of `finished-candidate.json`.
    pub candidate_receipt_digest: &'a Sha256Digest,
}

/// Opaque read-back proof of a complete local candidate and retained chain.
///
/// It is not execution provenance, a portable recovery record, a compatibility
/// result, or release admission. The raw tree digest is deliberately distinct
/// from the normalized package-manifest tree digest.
#[derive(Debug)]
pub struct FinishedCandidateReadback {
    record: FinishedRecord,
    snapshot: TreeContentCas,
    receipts: Vec<(PathBuf, Vec<u8>)>,
    work_dir: PathBuf,
}

impl FinishedCandidateReadback {
    pub(crate) const fn snapshot(&self) -> &TreeContentCas {
        &self.snapshot
    }

    /// Self-digest of the independently selected finished-tree receipt.
    #[must_use]
    pub const fn receipt_sha256(&self) -> &Sha256Digest {
        &self.record.receipt_sha256
    }

    /// Complete raw tree digest, including link targets and original modes.
    #[must_use]
    pub const fn payload_sha256(&self) -> &Sha256Digest {
        &self.record.payload_sha256
    }

    /// Number of all non-root filesystem entries, not only regular collectors.
    #[must_use]
    pub const fn entry_count(&self) -> u64 {
        self.record.entry_count
    }

    /// Total measured regular-file bytes in the complete finished tree.
    #[must_use]
    pub const fn regular_file_bytes(&self) -> u64 {
        self.record.regular_file_bytes
    }

    pub(crate) fn require_package_request(
        &self,
        request: &PackageRequest,
    ) -> Result<(), ContractError> {
        let compiler = crate::package_identity::compiler_identity_for_format(
            &request.source_lock,
            &request.profile,
            crate::package::PackageFormat::CompilerFamilyV2,
        )?;
        if request.candidate_root != self.record.candidate_root
            || request.recipe.sha256() != &self.record.identity.recipe_sha256
            || request.recipe.source().0 != &self.record.identity.source_commit
            || request.recipe.producer().0 != &self.record.identity.producer_commit
            || request.recipe.tools().0 != &self.record.identity.tools_commit
            || request.source_lock.sha256() != &self.record.source_lock_sha256
            || request.profile.document_sha256() != &self.record.profiles_sha256
            || request.profile.name() != self.record.identity.target_profile
            || request.host != self.record.identity.host
            || request.profile.target_triple() != self.record.target_triple
            || compiler != self.record.compiler
        {
            return Err(ContractError::state(
                "finished candidate does not bind the selected package inputs",
            ));
        }
        self.revalidate()
    }

    pub(crate) fn revalidate(&self) -> Result<(), ContractError> {
        require_inventory(self.receipts[0].0.parent().expect("receipt parent"))?;
        for (path, expected) in &self.receipts {
            if read_document(path)? != *expected {
                return Err(ContractError::state(
                    "finished candidate receipt bytes changed after read-back",
                ));
            }
        }
        if measure_payload(&self.record.candidate_root)? != self.snapshot {
            return Err(ContractError::state(
                "finished candidate payload changed after read-back",
            ));
        }
        Ok(())
    }
}

/// A complete V2 package joined to the exact locally measured candidate.
///
/// The package's four members were read back before becoming visible. This
/// does not establish independent A/B builds or authenticated execution.
#[derive(Debug)]
pub struct FinishedCandidatePackage {
    output: PackageOutput,
    verified: VerifiedPackage,
    candidate_receipt_sha256: Sha256Digest,
    candidate_payload_sha256: Sha256Digest,
    request: PackageRequest,
}

impl FinishedCandidatePackage {
    /// Atomically published package members.
    #[must_use]
    pub const fn output(&self) -> &PackageOutput {
        &self.output
    }
    /// Complete bounded package read-back, including its normalized payload inventory.
    #[must_use]
    pub const fn verified(&self) -> &VerifiedPackage {
        &self.verified
    }
    /// Selected finished-candidate receipt joined to this package operation.
    #[must_use]
    pub const fn candidate_receipt_sha256(&self) -> &Sha256Digest {
        &self.candidate_receipt_sha256
    }
    /// Raw candidate digest; not the normalized package tree digest.
    #[must_use]
    pub const fn candidate_payload_sha256(&self) -> &Sha256Digest {
        &self.candidate_payload_sha256
    }
}

/// Package exactly one previously checked finished candidate in family-V2 format.
///
/// Source acquisition uses a bounded no-follow snapshot copy. Only the private
/// copy undergoes the existing package normalization/marker-removal rules.
/// Candidate and receipts are revalidated before the atomic package commit.
/// No existing candidate, receipt or package is overwritten.
///
/// # Errors
/// Rejects mismatched inputs, changed candidate/chain, unsafe staging, or any
/// failure of the complete package read-back. It has no release authority.
pub fn package_finished_candidate(
    request: &PackageRequest,
    candidate: &FinishedCandidateReadback,
) -> Result<FinishedCandidatePackage, ContractError> {
    let (output, verified) = crate::package::package_with_candidate_proof(request, candidate)?;
    Ok(FinishedCandidatePackage {
        output,
        verified,
        candidate_receipt_sha256: candidate.receipt_sha256().clone(),
        candidate_payload_sha256: candidate.payload_sha256().clone(),
        request: request.clone(),
    })
}

/// Read a complete finished payload and its ordered local lifecycle chain.
///
/// Bounded no-follow acquisition rejects duplicate/unknown fields, altered
/// identities, missing/extra receipts, broken predecessors, collector-only
/// substitutes and any complete-tree change. Compiler checkpoint outputs are
/// not remeasured after intentional collector cleanup; the finished-tree
/// record binds the actual resulting tree instead. No command is executed.
///
/// # Errors
/// Returns a state diagnostic on any inconsistent or unsafe retained input.
pub fn readback_finished_candidate(
    request: &FinishedCandidateRequest<'_>,
) -> Result<FinishedCandidateReadback, ContractError> {
    let expected = expected_record(request)?;
    let directory = request.work_dir.join("native-lifecycle/receipts");
    require_inventory(&directory)?;
    let mut receipts = read_phase_chain(
        &directory,
        request.identity,
        request.phase_receipt_digests,
        request.work_dir,
        request.output_dir,
        request.profile,
    )?;
    let path = directory.join(RECEIPT_FILE);
    let bytes = read_document(&path)?;
    let record: FinishedRecord = serde_json::from_slice(&bytes).map_err(|_| {
        ContractError::state("finished candidate receipt violates its closed schema")
    })?;
    verify_self_digest(&record, &record.receipt_sha256, &bytes)?;
    if record.receipt_sha256 != *request.candidate_receipt_digest
        || record.schema != SCHEMA
        || record.identity != expected.identity
        || record.compiler != expected.compiler
        || record.target_triple != expected.target_triple
        || record.source_lock_sha256 != expected.source_lock_sha256
        || record.profiles_sha256 != expected.profiles_sha256
        || record.candidate_root != expected.candidate_root
        || record.phase_receipt_digests != expected.phase_receipt_digests
    {
        return Err(ContractError::state(
            "finished candidate receipt does not match independently selected inputs and chain",
        ));
    }
    let snapshot = measure_payload(&record.candidate_root)?;
    if record.payload_sha256 != snapshot.payload_digest_excluding(None)
        || record.entry_count != snapshot.entry_count() as u64
        || Some(record.regular_file_bytes) != snapshot.regular_file_bytes()
        || record.entry_count == 0
    {
        return Err(ContractError::state(
            "finished candidate receipt does not match the complete payload",
        ));
    }
    receipts.push((path, bytes));
    let proof = FinishedCandidateReadback {
        record,
        snapshot,
        receipts,
        work_dir: request.work_dir.to_path_buf(),
    };
    proof.revalidate()?;
    Ok(proof)
}

pub(crate) fn payload_limits() -> TreeTraversalLimits {
    TreeTraversalLimits {
        // The archive adds its root and generated manifest. Reserve the full
        // verifier metadata budget even when the raw tree has no old manifest.
        max_entries: usize::try_from(crate::package_verify::MAX_ARCHIVE_ENTRIES - 2)
            .expect("archive entry budget fits supported hosts"),
        max_regular_file_bytes: crate::package_verify::MAX_EXPANDED_ARCHIVE_BYTES
            - crate::package_verify::MAX_METADATA_BYTES,
    }
}

fn measure_payload(root: &Path) -> Result<TreeContentCas, ContractError> {
    validate_root(root)?;
    measure_tree_content_cas_bounded(root, payload_limits()).map_err(|_| {
        ContractError::state("cannot safely measure the complete bounded candidate payload")
    })
}

// Writer arguments reuse the public expectation contract; the placeholder
// candidate digest is ignored until the record has been durably created.
pub(crate) fn persist_finished_candidate(
    request: &FinishedCandidateRequest<'_>,
) -> Result<Sha256Digest, ContractError> {
    let mut record = expected_record(request)?;
    let directory = request.work_dir.join("native-lifecycle/receipts");
    read_phase_chain(
        &directory,
        request.identity,
        request.phase_receipt_digests,
        request.work_dir,
        request.output_dir,
        request.profile,
    )?;
    let snapshot = measure_payload(&record.candidate_root)?;
    record.payload_sha256 = snapshot.payload_digest_excluding(None);
    record.entry_count = snapshot.entry_count() as u64;
    record.regular_file_bytes = snapshot
        .regular_file_bytes()
        .ok_or_else(|| ContractError::state("candidate byte total is not representable"))?;
    if record.entry_count == 0 {
        return Err(ContractError::state("finished candidate payload is empty"));
    }
    let mut unsigned = serde_json::to_value(&record)
        .map_err(|_| ContractError::state("cannot encode finished candidate receipt"))?;
    unsigned
        .as_object_mut()
        .expect("record object")
        .remove("receipt_sha256");
    record.receipt_sha256 = sha256_bytes(&canonical::bytes(&unsigned)?);
    let bytes = canonical::bytes(
        &serde_json::to_value(&record)
            .map_err(|_| ContractError::state("cannot encode finished candidate receipt"))?,
    )?;
    let path = directory.join(RECEIPT_FILE);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|_| {
            ContractError::state(
                "cannot create fresh finished candidate receipt; published payload is retained",
            )
        })?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| {
            ContractError::state(
                "cannot durably write finished candidate receipt; published payload is retained",
            )
        })?;
    crate::filesystem::open_directory(&directory)
        .and_then(|file| file.sync_all())
        .map_err(|_| ContractError::state("cannot sync finished candidate receipt directory"))?;
    readback_finished_candidate(&FinishedCandidateRequest {
        candidate_receipt_digest: &record.receipt_sha256,
        ..*request
    })?;
    Ok(record.receipt_sha256)
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct NativeIdentity {
    recipe_sha256: Sha256Digest,
    source_commit: GitObjectId,
    producer_commit: GitObjectId,
    tools_commit: GitObjectId,
    host: String,
    target_profile: String,
    executor: NativeExecutor,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct NativeExecutor {
    contract_id: Option<String>,
    contract_sha256: Option<Sha256Digest>,
    tools_commit: Option<GitObjectId>,
    binary_sha256: Sha256Digest,
    origin_evidence_sha256: Option<Sha256Digest>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FinishedRecord {
    schema: String,
    identity: NativeIdentity,
    compiler: ArosCompilerIdentity,
    target_triple: String,
    source_lock_sha256: Sha256Digest,
    profiles_sha256: Sha256Digest,
    candidate_root: PathBuf,
    phase_receipt_digests: [Sha256Digest; 6],
    payload_sha256: Sha256Digest,
    entry_count: u64,
    regular_file_bytes: u64,
    receipt_sha256: Sha256Digest,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PhaseReceipt {
    schema: String,
    identity: NativeIdentity,
    phase: String,
    input_sha256: Sha256Digest,
    output_root: PathBuf,
    outputs: Vec<PhaseOutput>,
    previous_receipt_sha256: Option<Sha256Digest>,
    receipt_sha256: Sha256Digest,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct PhaseOutput {
    path: String,
    kind: String,
    sha256: Sha256Digest,
    size: u64,
}

fn expected_record(
    request: &FinishedCandidateRequest<'_>,
) -> Result<FinishedRecord, ContractError> {
    validate_root(request.work_dir)?;
    validate_root(request.output_dir)?;
    if request.work_dir.starts_with(request.output_dir)
        || request.output_dir.starts_with(request.work_dir)
    {
        return Err(ContractError::state(
            "finished candidate work and output roots overlap",
        ));
    }
    crate::package_identity::require_recipe_binding(
        request.recipe,
        request.source_lock,
        request.profile,
        crate::package::PackageFormat::CompilerFamilyV2,
    )?;
    let identity = owned_identity(request.identity)?;
    if identity.recipe_sha256 != *request.recipe.sha256()
        || &identity.source_commit != request.recipe.source().0
        || &identity.producer_commit != request.recipe.producer().0
        || &identity.tools_commit != request.recipe.tools().0
        || identity.target_profile != request.profile.name()
        || identity.executor.contract_id.as_deref() != Some("aros-toolchain-producer-v1")
        || identity.executor.contract_sha256.is_none()
        || identity.executor.tools_commit.as_ref() != Some(&identity.tools_commit)
    {
        return Err(ContractError::state(
            "finished candidate identity differs from selected recipe/profile/native executor",
        ));
    }
    // Local evidence may retain an Intel macOS build; family-V2 packaging
    // independently rejects that host for a new release.
    if !matches!(
        identity.host.as_str(),
        "linux-x86_64" | "linux-aarch64" | "macos-aarch64" | "macos-x86_64"
    ) {
        return Err(ContractError::state(
            "finished candidate has an unsupported native host",
        ));
    }
    Ok(FinishedRecord {
        schema: SCHEMA.into(),
        identity,
        compiler: crate::package_identity::compiler_identity_for_format(
            request.source_lock,
            request.profile,
            crate::package::PackageFormat::CompilerFamilyV2,
        )?,
        target_triple: request.profile.target_triple().into(),
        source_lock_sha256: request.source_lock.sha256().clone(),
        profiles_sha256: request.profile.document_sha256().clone(),
        candidate_root: request.output_dir.join("toolchain"),
        phase_receipt_digests: request.phase_receipt_digests.clone(),
        payload_sha256: sha256_bytes(b""),
        entry_count: 0,
        regular_file_bytes: 0,
        receipt_sha256: sha256_bytes(b""),
    })
}

fn owned_identity(identity: &Identity) -> Result<NativeIdentity, ContractError> {
    serde_json::from_value(
        serde_json::to_value(identity)
            .map_err(|_| ContractError::state("cannot encode expected native identity"))?,
    )
    .map_err(|_| ContractError::state("expected native identity violates its schema"))
}

fn read_phase_chain(
    directory: &Path,
    identity: &Identity,
    expected: &[Sha256Digest; 6],
    work: &Path,
    output: &Path,
    profile: &Profile,
) -> Result<Vec<(PathBuf, Vec<u8>)>, ContractError> {
    let identity = owned_identity(identity)?;
    let mut retained = Vec::new();
    for (index, phase) in PHASES.iter().enumerate() {
        let path = directory.join(format!("{phase}.json"));
        let bytes = read_document(&path)?;
        let receipt =
            validate_phase_receipt(&bytes, &identity, expected, index, work, output, profile)?;
        if index == 5 {
            for item in &receipt.outputs {
                let relative = item.path.strip_prefix("toolchain/").ok_or_else(|| {
                    ContractError::state(
                        "published collector path is outside its selected candidate",
                    )
                })?;
                let (_, content) = measure_regular_file_bounded(
                    &output.join("toolchain").join(relative),
                    canonical::MAX_DOCUMENT_BYTES as u64 * 128,
                )
                .map_err(|_| ContractError::state("cannot safely read published collector"))?
                .ok_or_else(|| {
                    ContractError::state("published collector changed while measured")
                })?;
                if content.len() as u64 != item.size || sha256_bytes(&content) != item.sha256 {
                    return Err(ContractError::state(
                        "published collector differs from its phase receipt",
                    ));
                }
            }
        }
        retained.push((path, bytes));
    }
    Ok(retained)
}

// Shared pure receipt validation for local measurement and portable byte read-back.
// Only the local caller additionally measures the original collector files.
fn validate_phase_receipt(
    bytes: &[u8],
    identity: &NativeIdentity,
    expected: &[Sha256Digest; 6],
    index: usize,
    work: &Path,
    output: &Path,
    profile: &Profile,
) -> Result<PhaseReceipt, ContractError> {
    let phase = PHASES[index];
    let receipt: PhaseReceipt = serde_json::from_slice(bytes)
        .map_err(|_| ContractError::state("native phase receipt violates its closed schema"))?;
    verify_self_digest(&receipt, &receipt.receipt_sha256, bytes)?;
    let expected_root = if index < 3 {
        work.join("native-lifecycle")
    } else if index == 5 {
        output.join("toolchain")
    } else {
        output.join(".aros-native-toolchain-stage")
    };
    if receipt.schema != "aros-toolchain-receipt-v1"
        || receipt.phase != phase
        || &receipt.identity != identity
        || receipt.output_root != expected_root
        || receipt.receipt_sha256 != expected[index]
        || receipt.previous_receipt_sha256.as_ref()
            != index.checked_sub(1).map(|previous| &expected[previous])
    {
        return Err(ContractError::state("native phase receipt differs from independently selected identity, phase, root or predecessor"));
    }
    let mut paths = BTreeSet::new();
    for item in &receipt.outputs {
        if item.kind != "file"
            || !safe_relative(&item.path)
            || !paths.insert(&item.path)
            || item.size > payload_limits().max_regular_file_bytes
        {
            return Err(ContractError::state(
                "native phase output inventory is unsafe or duplicated",
            ));
        }
    }
    if index < 2 && !receipt.outputs.is_empty() {
        return Err(ContractError::state(
            "native input-only phase has unexpected outputs",
        ));
    }
    if index == 5 {
        let expected_collectors = crate::native_family::collector_outputs(profile)
            .into_iter()
            .map(|path| format!("toolchain/{path}"))
            .collect::<BTreeSet<_>>();
        if paths
            .iter()
            .map(|path| (*path).clone())
            .collect::<BTreeSet<_>>()
            != expected_collectors
        {
            return Err(ContractError::state(
                "published phase does not bind the complete selected collector inventory",
            ));
        }
        if receipt.outputs.is_empty() {
            return Err(ContractError::state(
                "published phase omits its collector inventory",
            ));
        }
    }
    Ok(receipt)
}

fn verify_self_digest<T: Serialize>(
    record: &T,
    expected: &Sha256Digest,
    bytes: &[u8],
) -> Result<(), ContractError> {
    let mut value = serde_json::to_value(record)
        .map_err(|_| ContractError::state("cannot encode retained candidate evidence"))?;
    if canonical::bytes(&value)? != bytes {
        return Err(ContractError::state(
            "retained candidate evidence is not canonical closed JSON",
        ));
    }
    value
        .as_object_mut()
        .expect("receipt object")
        .remove("receipt_sha256");
    if sha256_bytes(&canonical::bytes(&value)?) != *expected {
        return Err(ContractError::state(
            "retained candidate evidence self-digest differs",
        ));
    }
    Ok(())
}

fn require_inventory(directory: &Path) -> Result<(), ContractError> {
    validate_root(directory)?;
    crate::filesystem::open_directory(directory)
        .map_err(|_| ContractError::state("candidate receipts directory is unsafe"))?;
    let expected = PHASES
        .iter()
        .map(|phase| format!("{phase}.json"))
        .chain(std::iter::once(RECEIPT_FILE.to_owned()))
        .collect::<BTreeSet<_>>();
    let mut actual = BTreeSet::new();
    for entry in fs::read_dir(directory)
        .map_err(|_| ContractError::state("cannot enumerate candidate receipts"))?
    {
        let entry =
            entry.map_err(|_| ContractError::state("cannot enumerate candidate receipt"))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| ContractError::state("candidate receipt name is not UTF-8"))?;
        if !expected.contains(&name) || !actual.insert(name) {
            return Err(ContractError::state(
                "candidate receipt inventory contains unexpected entries",
            ));
        }
    }
    if actual != expected {
        return Err(ContractError::state(
            "candidate receipt inventory is incomplete",
        ));
    }
    Ok(())
}

fn read_document(path: &Path) -> Result<Vec<u8>, ContractError> {
    let (_, bytes) = measure_regular_file_bounded(path, canonical::MAX_DOCUMENT_BYTES as u64)
        .map_err(|_| ContractError::state("candidate receipt is unsafe or exceeds its byte bound"))?
        .ok_or_else(|| ContractError::state("candidate receipt changed while read"))?;
    Ok(bytes)
}

fn validate_root(path: &Path) -> Result<(), ContractError> {
    if !path.is_absolute()
        || path.to_str().is_none()
        || path
            .components()
            .any(|component| !matches!(component, Component::RootDir | Component::Normal(_)))
    {
        return Err(ContractError::state(
            "candidate evidence root must be absolute canonical UTF-8",
        ));
    }
    Ok(())
}

fn safe_relative(value: &str) -> bool {
    !value.is_empty()
        && !value.contains('\\')
        && !value.contains('\0')
        && value
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}
