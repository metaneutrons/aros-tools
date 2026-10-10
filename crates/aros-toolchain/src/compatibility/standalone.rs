//! Standalone collector-output verification for the M5 compatibility harness.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use aros_common::{
    elf::{self, AROS_ABI_VERSION, OS_ABI_AROS},
    open_regular_file_nofollow, sha256_bytes, ArosCompilerIdentity, Sha256Digest,
};

use super::checked_directory;
use crate::ContractError;

const MAX_STANDALONE_TARGETS: usize = 2;
pub(super) const MAX_STANDALONE_ARTIFACT_BYTES: u64 = 128 * 1024 * 1024;
// `collect-aros` defines the list bounds as ordinary ELF symbols. The `$`
// commonly shown after them is a shell-regex end anchor, not part of either
// upstream symbol name.
pub(super) const C_COLLECTOR_SYMBOL: &str = "__TOOLCHAIN_LIST__";
pub(super) const CXX_COLLECTOR_SYMBOL: &str = "__INIT_ARRAY_LIST__";

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
/// triple, and define the language-specific collector symbol in an existing
/// section or as an absolute symbol. This has no
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
    verify_outputs(request, None)
}

/// Verify standalone outputs against explicitly supplied compiler identities.
///
/// The identity map must cover exactly every requested target. Every explicit
/// target requires its matching ELF machine and relocatable link type. GNU RISC-V
/// outputs additionally pass the complete source-bound target contract:
/// machine, class, relocatable link type, floating-point ABI, ISA attributes,
/// stack/atomic/register conventions and AROS ABI marking. Compiler versions
/// and target triples are validated before reading outputs. RISC-V is never
/// admitted by the legacy verifier without this explicit contract.
///
/// Callers must obtain these identities from independently verified package
/// manifests and bind them to selected recipe/profile inputs. This operation
/// verifies output bytes only; it does not authenticate those declarations,
/// execute a compiler, qualify relocation or authorize publication.
///
/// # Errors
///
/// Returns AX0703 for an incomplete or invalid compiler map, unsafe outputs,
/// target-contract mismatches or missing language-specific collector symbols.
pub fn verify_standalone_outputs_with_compilers(
    request: &StandaloneOutputRequest,
    compilers: &BTreeMap<String, ArosCompilerIdentity>,
) -> Result<StandaloneOutputReport, ContractError> {
    if request.targets.len() > MAX_STANDALONE_TARGETS
        || compilers.len() != request.targets.len()
        || !compilers.keys().eq(request.targets.keys())
    {
        return Err(ContractError::compatibility(
            "standalone compiler identities must cover exactly the requested targets",
        ));
    }
    for (triple, compiler) in compilers {
        compiler.validate_for_target(triple).map_err(|_| {
            ContractError::compatibility(
                "standalone compiler identity differs from its target contract",
            )
        })?;
    }
    verify_outputs(request, Some(compilers))
}

fn verify_outputs(
    request: &StandaloneOutputRequest,
    compilers: Option<&BTreeMap<String, ArosCompilerIdentity>>,
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
        let compiler = compilers.and_then(|values| values.get(triple));
        let riscv = match compiler {
            Some(ArosCompilerIdentity::Gnu { target, .. }) => Some(target),
            _ => None,
        };
        let class = standalone_target_class(triple, riscv)?;
        let machine = if compilers.is_some() {
            Some(
                match triple
                    .split_once('-')
                    .map_or(triple.as_str(), |(cpu, _)| cpu)
                {
                    "x86_64" => 62,
                    "i386" => 3,
                    "arm" => 40,
                    "aarch64" => 183,
                    "riscv" | "riscv64" => elf::riscv::MACHINE,
                    _ => unreachable!("standalone_target_class rejects unsupported CPUs"),
                },
            )
        } else {
            None
        };
        let c = verify_standalone_artifact(
            &output_root,
            &artifacts.c,
            (class, machine),
            C_COLLECTOR_SYMBOL,
            riscv,
            &mut paths,
        )?;
        let cxx = verify_standalone_artifact(
            &output_root,
            &artifacts.cxx,
            (class, machine),
            CXX_COLLECTOR_SYMBOL,
            riscv,
            &mut paths,
        )?;
        targets.insert(triple.clone(), StandaloneTargetReport { c, cxx });
    }
    Ok(StandaloneOutputReport { targets })
}

fn standalone_target_class(
    triple: &str,
    riscv: Option<&elf::riscv::TargetContract>,
) -> Result<elf::Class, ContractError> {
    if !crate::profiles::identifier(triple) {
        return Err(ContractError::compatibility(
            "standalone output contract contains an unsafe target triple",
        ));
    }
    match triple.split_once('-').map_or(triple, |(cpu, _)| cpu) {
        "x86_64" | "aarch64" => Ok(elf::Class::Elf64),
        "i386" | "arm" => Ok(elf::Class::Elf32),
        "riscv" if riscv.is_some() => Ok(elf::Class::Elf32),
        "riscv64" if riscv.is_some() => Ok(elf::Class::Elf64),
        _ => Err(ContractError::compatibility(
            "standalone output contract contains an unsupported target triple",
        )),
    }
}

fn verify_standalone_artifact(
    output_root: &Path,
    path: &Path,
    expected_identity: (elf::Class, Option<u16>),
    required_symbol: &str,
    riscv: Option<&elf::riscv::TargetContract>,
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
    (&mut file)
        .take(MAX_STANDALONE_ARTIFACT_BYTES + 1)
        .read_to_end(&mut bytes)
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
    if object.class != expected_identity.0
        || object.os_abi != OS_ABI_AROS
        || object.abi_version != AROS_ABI_VERSION
        || expected_identity
            .1
            .is_some_and(|machine| object.machine != machine || object.kind != 1)
    {
        return Err(ContractError::compatibility(
            "standalone output does not carry the selected target's AROS ELF identity",
        ));
    }
    if let Some(target) = riscv {
        target
            .verify(&bytes, elf::riscv::ArtifactRole::ArosRelocatable)
            .map_err(|_| {
                ContractError::compatibility(
                    "standalone output differs from its source-bound RISC-V target contract",
                )
            })?;
    }
    if !object.symbols.iter().any(|symbol| {
        symbol.name == required_symbol
            && match symbol.home {
                elf::Home::Absolute => true,
                elf::Home::Undefined => false,
                elf::Home::Section(index) => {
                    index != 0
                        && index < 0xff00
                        && object.sections.iter().any(|section| section.index == index)
                }
            }
    }) {
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
