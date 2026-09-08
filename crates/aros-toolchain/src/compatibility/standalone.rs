//! Standalone collector-output verification for the M5 compatibility harness.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use aros_common::{
    elf::{self, AROS_ABI_VERSION, OS_ABI_AROS},
    open_regular_file_nofollow, sha256_bytes, Sha256Digest,
};

use super::checked_directory;
use crate::ContractError;

const MAX_STANDALONE_TARGETS: usize = 2;
const MAX_STANDALONE_ARTIFACT_BYTES: u64 = 128 * 1024 * 1024;
pub(super) const C_COLLECTOR_SYMBOL: &str = "__TOOLCHAIN_LIST__$";
pub(super) const CXX_COLLECTOR_SYMBOL: &str = "__INIT_ARRAY_LIST__$";

/// Explicit standalone C/C++ outputs for one selected target triple.
#[derive(Debug, Clone)]
pub struct StandaloneTargetArtifacts {
    /// C object linked through the prefix-owned collector.
    pub c: PathBuf,
    /// C++ object linked through the prefix-owned collector.
    pub cxx: PathBuf,
}

/// Closed standalone-output check for one or two declared target triples.
///
/// The PC profile supplies both `x86_64-unknown-aros` and its required
/// `i386-unknown-aros` companion. Other current profiles supply their one
/// configured target triple. Every output must be a distinct direct child of
/// `output_root`; the verifier accepts no glob, link, or host search path.
#[derive(Debug, Clone)]
pub struct StandaloneOutputRequest {
    /// Existing real directory containing only this probe's direct outputs.
    pub output_root: PathBuf,
    /// Exact target triples and their C/C++ output paths.
    pub targets: BTreeMap<String, StandaloneTargetArtifacts>,
}

/// Measured identity of one standalone AROS ELF output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandaloneArtifactIdentity {
    /// SHA-256 of the exact no-follow-read output bytes.
    pub sha256: Sha256Digest,
    /// Exact output byte length.
    pub size: u64,
    /// Parsed ELF class, derived from the output rather than the host.
    pub class: elf::Class,
}

/// Measured C and C++ collector outputs for one target triple.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandaloneTargetReport {
    /// C output identity after AROS and collector-symbol validation.
    pub c: StandaloneArtifactIdentity,
    /// C++ output identity after AROS and collector-symbol validation.
    pub cxx: StandaloneArtifactIdentity,
}

/// Measured standalone collector evidence keyed by target triple.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandaloneOutputReport {
    /// Closed, nonempty set of target-triple output reports.
    pub targets: BTreeMap<String, StandaloneTargetReport>,
}

/// Verify the standalone C/C++ collector output contract without executing a process.
///
/// Every output is opened through a no-follow descriptor, read once within a
/// fixed bound, and parsed with the shared ELF reader. The object must carry
/// AROS' `EI_OSABI`/`EI_ABIVERSION`, have the class required by its target
/// triple, and contain the language-specific collector symbol. This has no
/// network, source, cache, tag, release or publication authority.
///
/// # Errors
///
/// Returns AX0703 for unsafe output paths, duplicate or unsupported target
/// triples, changed files, malformed/non-AROS ELF objects, or missing
/// collector symbols.
pub fn verify_standalone_outputs(
    request: &StandaloneOutputRequest,
) -> Result<StandaloneOutputReport, ContractError> {
    let output_root = checked_directory(&request.output_root, "standalone output root")?;
    if request.targets.is_empty() || request.targets.len() > MAX_STANDALONE_TARGETS {
        return Err(ContractError::compatibility(
            "standalone output contract must contain one or two target triples",
        ));
    }
    let mut paths = BTreeSet::new();
    let mut targets = BTreeMap::new();
    for (triple, artifacts) in &request.targets {
        let class = standalone_target_class(triple)?;
        let c = verify_standalone_artifact(
            &output_root,
            &artifacts.c,
            class,
            C_COLLECTOR_SYMBOL,
            &mut paths,
        )?;
        let cxx = verify_standalone_artifact(
            &output_root,
            &artifacts.cxx,
            class,
            CXX_COLLECTOR_SYMBOL,
            &mut paths,
        )?;
        targets.insert(triple.clone(), StandaloneTargetReport { c, cxx });
    }
    Ok(StandaloneOutputReport { targets })
}

fn standalone_target_class(triple: &str) -> Result<elf::Class, ContractError> {
    if !crate::profiles::identifier(triple) {
        return Err(ContractError::compatibility(
            "standalone output contract contains an unsafe target triple",
        ));
    }
    match triple.split_once('-').map_or(triple, |(cpu, _)| cpu) {
        "x86_64" | "aarch64" => Ok(elf::Class::Elf64),
        "i386" | "arm" => Ok(elf::Class::Elf32),
        _ => Err(ContractError::compatibility(
            "standalone output contract contains an unsupported target triple",
        )),
    }
}

fn verify_standalone_artifact(
    output_root: &Path,
    path: &Path,
    expected_class: elf::Class,
    required_symbol: &str,
    paths: &mut BTreeSet<PathBuf>,
) -> Result<StandaloneArtifactIdentity, ContractError> {
    let path = checked_direct_child(output_root, path, "standalone output")?;
    if !paths.insert(path.clone()) {
        return Err(ContractError::compatibility(
            "standalone output contract reuses one artifact path",
        ));
    }
    let mut file = open_regular_file_nofollow(&path).map_err(|_| {
        ContractError::compatibility(
            "cannot safely open a standalone output without following links",
        )
    })?;
    let before = file
        .metadata()
        .map_err(|_| ContractError::compatibility("cannot inspect the opened standalone output"))?;
    if !before.is_file() || before.len() == 0 || before.len() > MAX_STANDALONE_ARTIFACT_BYTES {
        return Err(ContractError::compatibility(
            "standalone output is empty, non-regular, or exceeds the configured size limit",
        ));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(before.len()).map_err(|_| {
        ContractError::compatibility("standalone output length exceeds addressable memory")
    })?);
    file.read_to_end(&mut bytes)
        .map_err(|_| ContractError::compatibility("cannot read the complete standalone output"))?;
    let after = file.metadata().map_err(|_| {
        ContractError::compatibility("cannot remeasure the opened standalone output")
    })?;
    if after.len() != before.len() || bytes.len() as u64 != before.len() {
        return Err(ContractError::compatibility(
            "standalone output changed while it was read",
        ));
    }
    let object = elf::read(&bytes).map_err(|_| {
        ContractError::compatibility(
            "standalone output is not a supported little-endian ELF object",
        )
    })?;
    if object.class != expected_class
        || object.os_abi != OS_ABI_AROS
        || object.abi_version != AROS_ABI_VERSION
    {
        return Err(ContractError::compatibility(
            "standalone output does not carry the selected target's AROS ELF identity",
        ));
    }
    if !object
        .symbols
        .iter()
        .any(|symbol| symbol.name == required_symbol)
    {
        return Err(ContractError::compatibility(
            "standalone output lacks its required collector symbol",
        ));
    }
    Ok(StandaloneArtifactIdentity {
        sha256: sha256_bytes(&bytes),
        size: before.len(),
        class: object.class,
    })
}

fn checked_direct_child(root: &Path, path: &Path, label: &str) -> Result<PathBuf, ContractError> {
    if !path.is_absolute() {
        return Err(ContractError::compatibility(format!(
            "{label} must be an absolute direct child of the standalone output root"
        )));
    }
    let parent = path.parent().ok_or_else(|| {
        ContractError::compatibility(format!("{label} has no standalone output-root parent"))
    })?;
    let canonical_parent = parent.canonicalize().map_err(|_| {
        ContractError::compatibility(format!(
            "{label} parent cannot be canonicalized below the standalone output root"
        ))
    })?;
    if canonical_parent != root || path.file_name().is_none() {
        return Err(ContractError::compatibility(format!(
            "{label} is not a direct child of the standalone output root"
        )));
    }
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| ContractError::compatibility(format!("cannot inspect {label}")))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(ContractError::compatibility(format!(
            "{label} is not a regular standalone output"
        )));
    }
    Ok(path.to_owned())
}
