//! Local consistency verification for one source-bound raw native kernel ELF.
//!
//! This command deliberately does not execute profile scripts or claim a
//! released-toolchain trust chain. It binds the image to the source contract
//! and locally installed toolchain, then runs the declared objdump residency
//! lint over the measured `.sramtext` section.

use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::Duration;

use anyhow::Context;
use aros_common::elf::riscv::ArtifactRole;
use aros_common::elf::{self, SHF_ALLOC, SHT_NOBITS};
use aros_common::toolchain_layout::ToolchainToolLayout;
use aros_common::{
    canonical_source_file, measure_regular_file_bounded, render_diagnostics,
    run_output_with_timeout, sha256_bytes, toolchain_tree_inventory, ArosCompilerIdentity,
    ArosToolchainManifest, Diagnostic, DiagnosticCode, DiagnosticFormat, DiagnosticSet,
    DiagnosticStage, Sha256Digest, TargetProfile, AROS_TOOLCHAIN_MANIFEST_FILE,
};
use clap::{error::ErrorKind, Parser};
use serde::Serialize;

const MAX_SOURCE_CONFIG_BYTES: u64 = 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;
const MAX_ELF_BYTES: u64 = 64 * 1024 * 1024;
const OBJDUMP_CAPTURE_BYTES: usize = 8 * 1024 * 1024;
const OBJDUMP_TIMEOUT: Duration = Duration::from_secs(30);
const SHF_EXECINSTR: u64 = 0x4;

const OBSERVABILITY_POLICY: aros_common::ObservabilityPolicy = aros_common::ObservabilityPolicy {
    log_schema: "aros-verify-log-v1",
    component: "AROS verifier",
    include_invocation: false,
    observability_code: DiagnosticCode::VerifyObservability,
    observability_stage: DiagnosticStage::Observability,
    internal_code: DiagnosticCode::VerifyInternal,
    internal_stage: DiagnosticStage::Internal,
    hint: "select an explicit writable local file or disable verifier logging",
};

#[derive(Parser, Debug)]
#[command(
    name = "aros-verify native-core",
    version,
    about = "Check source-bound native-core ELF residency against its GNU toolchain"
)]
struct NativeCoreArgs {
    /// Explicit source checkout containing aros-targets.toml.
    #[arg(long)]
    source: PathBuf,

    /// Unique profile name from the explicit source checkout.
    #[arg(long)]
    profile: String,

    /// Source-relative native-build contract path.
    #[arg(long)]
    contract: PathBuf,

    /// Expected SHA-256 of the exact source contract bytes.
    #[arg(long)]
    contract_sha256: String,

    /// Installed, closed toolchain prefix.
    #[arg(long)]
    toolchain_prefix: PathBuf,

    /// Raw native-core ELF to inspect.
    #[arg(long)]
    elf: PathBuf,

    /// New JSON report path. Existing paths are never replaced.
    #[arg(long)]
    report: PathBuf,

    /// Diagnostic renderer used for failures.
    #[arg(
        long,
        value_enum,
        default_value_t = DiagnosticFormat::Human,
        env = "AROS_VERIFY_DIAGNOSTIC_FORMAT"
    )]
    diagnostic_format: DiagnosticFormat,
}

#[derive(Debug, Serialize)]
struct NativeCoreReport {
    schema: &'static str,
    result: &'static str,
    evidence_scope: &'static str,
    profile: String,
    source_config: SourceDigest,
    contract: ContractDigest,
    contract_inputs: Vec<aros_common::native_build_contract::NativeBuildInput>,
    toolchain: ToolchainDigest,
    compiler: ArosCompilerIdentity,
    elf: ElfDigest,
    objdump: ToolDigest,
    residency: ResidencyRecord,
}

#[derive(Debug, Serialize)]
struct SourceDigest {
    path: String,
    sha256: Sha256Digest,
}

#[derive(Debug, Serialize)]
struct ContractDigest {
    path: String,
    sha256: Sha256Digest,
}

#[derive(Debug, Serialize)]
struct ToolchainDigest {
    manifest_path: String,
    manifest_sha256: Sha256Digest,
    tree_sha256: String,
    tool_layout_sha256: Sha256Digest,
}

#[derive(Debug, Serialize)]
struct ElfDigest {
    path: String,
    sha256: Sha256Digest,
    size: u64,
    section: String,
    section_address: u64,
    section_size: u64,
}

#[derive(Debug, Serialize)]
struct ToolDigest {
    path: String,
    sha256: Sha256Digest,
}

#[derive(Debug, Serialize)]
struct ResidencyRecord {
    algorithm: String,
    flash_start: u64,
    flash_end: u64,
    sram_start: u64,
    sram_end: u64,
    checked_section: String,
    offending_flash_references: Vec<String>,
}

#[derive(Clone)]
struct FileSnapshot {
    identity: aros_common::FileIdentity,
    bytes: Vec<u8>,
    sha256: Sha256Digest,
}

struct SourceBinding {
    root: PathBuf,
    config_path: PathBuf,
    config: FileSnapshot,
    profile: TargetProfile,
    contract: aros_common::native_build_contract::LoadedNativeBuildContract,
}

struct ToolchainBinding {
    root: PathBuf,
    manifest_path: PathBuf,
    manifest_snapshot: FileSnapshot,
    manifest: ArosToolchainManifest,
    tree_sha256: String,
    inventory: Vec<aros_common::ArosToolchainManifestEntry>,
    layout: ToolchainToolLayout,
    objdump_path: PathBuf,
    objdump_sha256: Sha256Digest,
    compiler: ArosCompilerIdentity,
}

/// Dispatch the subcommand's arguments while retaining the verifier's shared
/// diagnostic renderer and the established top-level command behavior.
pub fn entry(arguments: Vec<OsString>) -> ExitCode {
    let requested_format =
        aros_common::requested_diagnostic_format(&arguments, "AROS_VERIFY_DIAGNOSTIC_FORMAT");
    let mut parser_arguments = Vec::with_capacity(arguments.len().saturating_sub(1));
    parser_arguments.push(OsString::from("aros-verify native-core"));
    parser_arguments.extend(arguments.into_iter().skip(2));

    let args = match NativeCoreArgs::try_parse_from(parser_arguments) {
        Ok(args) => args,
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            return match aros_common::write_stdout(&error.to_string()) {
                Ok(()) => ExitCode::SUCCESS,
                Err(write_error) => {
                    render_error(
                        DiagnosticCode::VerifyObservability,
                        DiagnosticStage::Observability,
                        format!("could not write native-core command help: {write_error}"),
                        None,
                        requested_format,
                    );
                    ExitCode::FAILURE
                }
            };
        }
        Err(error) => {
            render_error(
                DiagnosticCode::VerifyInvocation,
                DiagnosticStage::Invocation,
                error.to_string().trim().to_owned(),
                None,
                requested_format,
            );
            return ExitCode::FAILURE;
        }
    };
    let requested_format = args.diagnostic_format;

    match verify(&args) {
        Ok(report) => match publish_report(&args.report, &report) {
            Ok(()) => {
                let message = format!(
                    "native-core local consistency check passed: {}\n",
                    args.report.display()
                );
                let _ = aros_common::write_stdout(&message);
                ExitCode::SUCCESS
            }
            Err(error) => {
                render_error(
                    DiagnosticCode::VerifyPublication,
                    DiagnosticStage::OutputPublication,
                    format!("cannot publish native-core report: {error:#}"),
                    Some(&args.report),
                    requested_format,
                );
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            render_error(
                DiagnosticCode::VerifyInput,
                DiagnosticStage::ObjectInspection,
                format!("native-core check failed: {error:#}"),
                Some(&args.elf),
                requested_format,
            );
            ExitCode::FAILURE
        }
    }
}

fn render_error(
    code: DiagnosticCode,
    stage: DiagnosticStage,
    message: String,
    path: Option<&Path>,
    format: DiagnosticFormat,
) {
    let diagnostic = Diagnostic::error(code, stage, message);
    let diagnostic = match path {
        Some(path) => {
            diagnostic.with_location(aros_common::SourceLocation::new(path.display().to_string()))
        }
        None => diagnostic,
    };
    render_diagnostics(
        &DiagnosticSet::single(diagnostic),
        format,
        OBSERVABILITY_POLICY,
    );
}

fn verify(args: &NativeCoreArgs) -> anyhow::Result<NativeCoreReport> {
    ensure_new_report_path(&args.report)?;
    let source = load_source_binding(args)?;
    let toolchain = load_toolchain_binding(args, &source)?;

    let elf_snapshot = snapshot_file(&args.elf, MAX_ELF_BYTES)?;
    let elf_path = fs::canonicalize(&args.elf).map_err(|error| {
        anyhow::anyhow!("cannot resolve ELF path '{}': {error}", args.elf.display())
    })?;
    let target = match &toolchain.compiler {
        ArosCompilerIdentity::Gnu { target, .. } => target,
        ArosCompilerIdentity::Llvm { .. } => {
            anyhow::bail!("native-core contract requires a verified GNU compiler identity")
        }
    };
    target
        .verify(&elf_snapshot.bytes, ArtifactRole::NativeCore)
        .context("ELF does not satisfy the verified native-core target contract")?;

    let object = elf::read(&elf_snapshot.bytes).context("cannot parse measured native-core ELF")?;
    let policy = &source.contract.contract.core.residency_policy;
    policy
        .validate()
        .map_err(|reason| anyhow::anyhow!("invalid source-bound residency policy: {reason}"))?;
    let implemented_algorithm = match super::native_residency::ALGORITHM_VERSION {
        super::native_residency::ResidencyAlgorithm::Riscv32XipV1 => "riscv32-xip-v1",
    };
    if policy.algorithm != implemented_algorithm {
        anyhow::bail!("source-bound residency policy is not implemented by this verifier");
    }
    let sections = object
        .sections
        .iter()
        .filter(|section| section.name == policy.section)
        .collect::<Vec<_>>();
    let [section] = sections.as_slice() else {
        anyhow::bail!(
            "ELF must contain exactly one '{}' section, found {}",
            policy.section,
            sections.len()
        );
    };
    if section.size == 0
        || section.flags & SHF_EXECINSTR == 0
        || section.flags & SHF_ALLOC == 0
        || section.kind == SHT_NOBITS
    {
        anyhow::bail!(
            "'{}' must be a nonempty allocated executable content section",
            policy.section
        );
    }
    let section_end = section
        .address
        .checked_add(section.size)
        .ok_or_else(|| anyhow::anyhow!("'{}' address range overflows", policy.section))?;
    if section.address < policy.sram_start || section_end > policy.sram_end {
        anyhow::bail!(
            "'{}' address range {:#x}..{:#x} is outside source-bound SRAM {:#x}..{:#x}",
            policy.section,
            section.address,
            section_end,
            policy.sram_start,
            policy.sram_end
        );
    }

    let flash = super::native_residency::AddressRange::new(policy.flash_start, policy.flash_end)
        .map_err(|error| anyhow::anyhow!("invalid source-bound flash range: {error}"))?;
    let sram = super::native_residency::AddressRange::new(policy.sram_start, policy.sram_end)
        .map_err(|error| anyhow::anyhow!("invalid source-bound SRAM range: {error}"))?;
    let offending = run_residency_check(
        &toolchain.objdump_path,
        &elf_path,
        &policy.section,
        flash,
        sram,
        OBJDUMP_CAPTURE_BYTES,
        OBJDUMP_TIMEOUT,
    )
    .context("declared objdump output failed the source-bound SRAM residency check")?;
    if !offending.is_empty() {
        anyhow::bail!(
            ".sramtext has named references into flash: {}",
            offending.join("; ")
        );
    }

    verify_source_unchanged(args, &source)?;
    verify_toolchain_unchanged(&toolchain)?;
    verify_file_unchanged(&args.elf, &elf_snapshot, MAX_ELF_BYTES)
        .context("native-core ELF changed during verification")?;
    ensure_new_report_path(&args.report)?;

    Ok(NativeCoreReport {
        schema: "aros-verify-native-core-v1",
        result: "passed",
        evidence_scope: "local-payload-consistency-only; not released-toolchain provenance or hardware qualification",
        profile: source.profile.name.clone(),
        source_config: SourceDigest {
            path: source.config_path.display().to_string(),
            sha256: source.config.sha256.clone(),
        },
        contract: ContractDigest {
            path: source.contract.path.display().to_string(),
            sha256: source.contract.sha256.clone(),
        },
        contract_inputs: source.contract.contract.inputs.clone(),
        toolchain: ToolchainDigest {
            manifest_path: toolchain.manifest_path.display().to_string(),
            manifest_sha256: toolchain.manifest_snapshot.sha256.clone(),
            tree_sha256: toolchain.tree_sha256.clone(),
            tool_layout_sha256: toolchain.layout.sha256().clone(),
        },
        compiler: toolchain.compiler.clone(),
        elf: ElfDigest {
            path: elf_path.display().to_string(),
            sha256: elf_snapshot.sha256,
            size: u64::try_from(elf_snapshot.bytes.len()).unwrap_or(u64::MAX),
            section: policy.section.clone(),
            section_address: section.address,
            section_size: section.size,
        },
        objdump: ToolDigest {
            path: toolchain.objdump_path.display().to_string(),
            sha256: toolchain.objdump_sha256.clone(),
        },
        residency: ResidencyRecord {
            algorithm: policy.algorithm.clone(),
            flash_start: policy.flash_start,
            flash_end: policy.flash_end,
            sram_start: policy.sram_start,
            sram_end: policy.sram_end,
            checked_section: policy.section.clone(),
            offending_flash_references: offending,
        },
    })
}

fn load_source_binding(args: &NativeCoreArgs) -> anyhow::Result<SourceBinding> {
    let root = args.source.canonicalize().map_err(|error| {
        anyhow::anyhow!(
            "cannot resolve source root '{}': {error}",
            args.source.display()
        )
    })?;
    if !fs::metadata(&root)
        .map_err(|error| anyhow::anyhow!("cannot inspect source root: {error}"))?
        .is_dir()
    {
        anyhow::bail!("source root is not a directory: {}", root.display());
    }
    let config_path = canonical_source_file(&root, Path::new("aros-targets.toml"))
        .context("explicit source checkout must contain a safe aros-targets.toml")?;
    let config = snapshot_file(&config_path, MAX_SOURCE_CONFIG_BYTES)
        .context("cannot measure explicit source aros-targets.toml")?;
    let loaded = TargetProfile::load_config(&config_path)
        .context("cannot parse explicit source aros-targets.toml")?;
    let mut profiles = loaded
        .targets
        .into_iter()
        .filter(|profile| profile.name == args.profile);
    let profile = profiles
        .next()
        .ok_or_else(|| anyhow::anyhow!("source has no profile named '{}'", args.profile))?;
    if profiles.next().is_some() {
        anyhow::bail!(
            "source contains more than one profile named '{}'",
            args.profile
        );
    }
    let expected_contract = Sha256Digest::parse(&args.contract_sha256)
        .context("--contract-sha256 must be exactly 64 hexadecimal characters")?;
    let contract = aros_common::native_build_contract::load_bound_native_build_contract(
        &root,
        &args.contract,
        &profile,
    )
    .context("cannot load source-bound native-build contract")?;
    if contract.sha256 != expected_contract {
        anyhow::bail!(
            "source contract digest mismatch: expected {}, measured {}",
            expected_contract,
            contract.sha256
        );
    }
    contract
        .contract
        .core
        .residency_policy
        .validate()
        .map_err(|reason| anyhow::anyhow!("invalid native residency policy: {reason}"))?;
    Ok(SourceBinding {
        root,
        config_path,
        config,
        profile,
        contract,
    })
}

fn load_toolchain_binding(
    args: &NativeCoreArgs,
    source: &SourceBinding,
) -> anyhow::Result<ToolchainBinding> {
    let root = args.toolchain_prefix.canonicalize().map_err(|error| {
        anyhow::anyhow!(
            "cannot resolve toolchain prefix '{}': {error}",
            args.toolchain_prefix.display()
        )
    })?;
    if !fs::metadata(&root)
        .map_err(|error| anyhow::anyhow!("cannot inspect toolchain prefix: {error}"))?
        .is_dir()
    {
        anyhow::bail!("toolchain prefix is not a directory: {}", root.display());
    }
    let manifest_path = root.join(AROS_TOOLCHAIN_MANIFEST_FILE);
    let manifest_snapshot = snapshot_file(&manifest_path, MAX_MANIFEST_BYTES)
        .context("cannot measure no-follow toolchain manifest")?;
    let manifest_from_snapshot: ArosToolchainManifest =
        serde_json::from_slice(&manifest_snapshot.bytes)
            .context("toolchain manifest snapshot is invalid JSON")?;
    manifest_from_snapshot
        .validate()
        .map_err(|reason| anyhow::anyhow!("invalid toolchain manifest snapshot: {reason}"))?;
    let manifest =
        ArosToolchainManifest::load(&root).context("cannot load installed toolchain manifest")?;
    if manifest != manifest_from_snapshot {
        anyhow::bail!("toolchain manifest changed between bounded measurement and validated load");
    }
    if !manifest_profile_matches_source(&source.profile, &manifest.target_profile)
        || manifest.target_triple != source.contract.contract.abi.target_triple
    {
        anyhow::bail!("toolchain profile or triple does not match the selected source contract");
    }
    let compiler = manifest.compiler_identity().map_err(|reason| {
        anyhow::anyhow!("manifest has no verified compiler identity: {reason}")
    })?;
    aros_common::native_build_contract::validate_native_build_compiler(
        &source.contract.contract,
        &compiler,
        &manifest.target_triple,
    )
    .context("source contract does not match the installed GNU compiler identity")?;

    let (tree_sha256, inventory) =
        toolchain_tree_inventory(&root).context("cannot inventory complete toolchain prefix")?;
    if tree_sha256 != manifest.tree_sha256 || inventory != manifest.files {
        anyhow::bail!(
            "toolchain manifest digest or file inventory does not match the measured prefix"
        );
    }
    let layout = ToolchainToolLayout::load(&root).map_err(|reason| {
        anyhow::anyhow!("cannot load declared toolchain executables: {reason}")
    })?;
    layout
        .validate_binding(&compiler, &manifest.target_triple)
        .map_err(|reason| anyhow::anyhow!("toolchain executable layout is not bound: {reason}"))?;
    if !layout.has_objdump_role() {
        anyhow::bail!("toolchain executable layout has no verified objdump role");
    }
    let tools = layout
        .resolve_tools(&root)
        .map_err(|reason| anyhow::anyhow!("cannot resolve declared toolchain tools: {reason}"))?;
    let objdump_path = tools
        .into_iter()
        .find_map(|(role, path)| (role == "objdump").then_some(path))
        .ok_or_else(|| anyhow::anyhow!("toolchain layout did not resolve its objdump role"))?;
    let objdump_target = objdump_path
        .canonicalize()
        .context("cannot resolve declared objdump executable target")?;
    let relative = objdump_target
        .strip_prefix(&root)
        .context("declared objdump resolves outside canonical toolchain prefix")?;
    let relative_text = relative
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("objdump path is not valid UTF-8"))?;
    let objdump_entry = inventory
        .iter()
        .find(|entry| entry.path == relative_text)
        .ok_or_else(|| {
            anyhow::anyhow!("declared objdump is absent from measured toolchain inventory")
        })?;
    if objdump_entry.kind != "file" {
        anyhow::bail!("declared objdump target is not a regular payload file");
    }
    let objdump_sha256 = Sha256Digest::parse(
        objdump_entry
            .sha256
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("declared objdump has no measured file digest"))?,
    )
    .context("measured objdump digest is invalid")?;

    Ok(ToolchainBinding {
        root,
        manifest_path,
        manifest_snapshot,
        manifest,
        tree_sha256,
        inventory,
        layout,
        objdump_path,
        objdump_sha256,
        compiler,
    })
}

fn manifest_profile_matches_source(profile: &TargetProfile, manifest_profile: &str) -> bool {
    manifest_profile == profile.toolchain_profile()
}

fn snapshot_file(path: &Path, max_bytes: u64) -> anyhow::Result<FileSnapshot> {
    let (identity, bytes) = measure_regular_file_bounded(path, max_bytes)
        .map_err(|error| anyhow::anyhow!("cannot safely measure '{}': {error}", path.display()))?
        .ok_or_else(|| anyhow::anyhow!("file is absent: {}", path.display()))?;
    let sha256 = sha256_bytes(&bytes);
    Ok(FileSnapshot {
        identity,
        bytes,
        sha256,
    })
}

fn run_residency_check(
    objdump_path: &Path,
    elf_path: &Path,
    section: &str,
    flash: super::native_residency::AddressRange,
    sram: super::native_residency::AddressRange,
    capture_limit: usize,
    timeout: Duration,
) -> anyhow::Result<Vec<String>> {
    let mut command = Command::new(objdump_path);
    command
        .arg("-dr")
        .arg(format!("--section={section}"))
        .arg(elf_path);
    let output = run_output_with_timeout(&mut command, capture_limit, timeout)
        .context("cannot execute the exact objdump declared by the toolchain layout")?;
    if output.timed_out {
        anyhow::bail!("declared objdump exceeded its deadline");
    }
    if !output.status.success() {
        anyhow::bail!("declared objdump exited unsuccessfully: {}", output.status);
    }
    let disassembly_bytes = output
        .stdout
        .exact_bytes()
        .ok_or_else(|| anyhow::anyhow!("objdump stdout exceeded the complete-output limit"))?;
    let stderr_bytes = output
        .stderr
        .exact_bytes()
        .ok_or_else(|| anyhow::anyhow!("objdump stderr exceeded the complete-output limit"))?;
    std::str::from_utf8(stderr_bytes).context("objdump stderr is not complete UTF-8 output")?;
    let disassembly = std::str::from_utf8(disassembly_bytes)
        .context("objdump stdout is not complete UTF-8 output")?;
    super::native_residency::check_sram_residency(disassembly, flash, sram)
        .context("objdump output is malformed or has flash references in SRAM text")
}

fn verify_file_unchanged(
    path: &Path,
    expected: &FileSnapshot,
    max_bytes: u64,
) -> anyhow::Result<()> {
    let current = snapshot_file(path, max_bytes)?;
    if current.identity != expected.identity
        || current.sha256 != expected.sha256
        || current.bytes != expected.bytes
    {
        anyhow::bail!("file changed during verification: {}", path.display());
    }
    Ok(())
}

fn verify_source_unchanged(args: &NativeCoreArgs, source: &SourceBinding) -> anyhow::Result<()> {
    verify_file_unchanged(&source.config_path, &source.config, MAX_SOURCE_CONFIG_BYTES)?;
    let current = aros_common::native_build_contract::load_bound_native_build_contract(
        &source.root,
        &args.contract,
        &source.profile,
    )
    .context("cannot revalidate source-bound contract and its declared inputs")?;
    if current.sha256 != source.contract.sha256
        || current.contract != source.contract.contract
        || current.path != source.contract.path
    {
        anyhow::bail!("source contract or one of its declared inputs changed during verification");
    }
    Ok(())
}

fn verify_toolchain_unchanged(toolchain: &ToolchainBinding) -> anyhow::Result<()> {
    verify_file_unchanged(
        &toolchain.manifest_path,
        &toolchain.manifest_snapshot,
        MAX_MANIFEST_BYTES,
    )?;
    let current_manifest = ArosToolchainManifest::load(&toolchain.root)
        .context("cannot re-read toolchain manifest")?;
    let (tree_sha256, inventory) = toolchain_tree_inventory(&toolchain.root)
        .context("cannot re-inventory complete toolchain prefix")?;
    if current_manifest != toolchain.manifest
        || tree_sha256 != toolchain.tree_sha256
        || inventory != toolchain.inventory
    {
        anyhow::bail!("toolchain manifest or payload changed during verification");
    }
    let layout = ToolchainToolLayout::load(&toolchain.root)
        .map_err(|reason| anyhow::anyhow!("cannot reload toolchain executable layout: {reason}"))?;
    if layout.sha256() != toolchain.layout.sha256() {
        anyhow::bail!("toolchain executable layout changed during verification");
    }
    Ok(())
}

fn ensure_new_report_path(path: &Path) -> anyhow::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => anyhow::bail!(
            "report path already exists; refusing to overwrite: {}",
            path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn publish_report(path: &Path, report: &NativeCoreReport) -> anyhow::Result<()> {
    ensure_new_report_path(path)?;
    let mut bytes =
        serde_json::to_vec_pretty(report).context("cannot encode native-core report")?;
    bytes.push(b'\n');
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            anyhow::anyhow!(
                "cannot create new report path '{}': {error}",
                path.display()
            )
        })?;
    file.write_all(&bytes)
        .context("cannot write complete native-core report")?;
    file.sync_all()
        .context("cannot durably flush native-core report")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn parse(args: &[&str]) -> Result<NativeCoreArgs, clap::Error> {
        NativeCoreArgs::try_parse_from(args.iter().copied())
    }

    #[test]
    fn native_core_help_documents_all_required_bindings() {
        let error = parse(&["aros-verify native-core", "--help"])
            .expect_err("--help must exit through clap help");
        assert_eq!(error.kind(), ErrorKind::DisplayHelp);
        let rendered = error.to_string();
        for option in [
            "--source",
            "--profile",
            "--contract",
            "--contract-sha256",
            "--toolchain-prefix",
            "--elf",
            "--report",
            "--diagnostic-format",
        ] {
            assert!(rendered.contains(option), "help is missing {option}");
        }
    }

    #[test]
    fn native_core_requires_all_explicit_inputs() {
        let error = parse(&["aros-verify native-core"]).expect_err("required options are absent");
        assert_eq!(error.kind(), ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn report_path_existing_file_is_never_replaced() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let report_path = directory.path().join("report.json");
        fs::write(&report_path, b"preserve me").expect("seed report");
        let error = ensure_new_report_path(&report_path).expect_err("existing path is rejected");
        assert!(error.to_string().contains("refusing to overwrite"));
        assert_eq!(
            fs::read(report_path).expect("existing report remains"),
            b"preserve me"
        );
    }

    #[test]
    fn explicit_toolchain_profile_alias_binds_but_board_name_does_not() {
        let profile: TargetProfile = serde_json::from_value(serde_json::json!({
            "name": "esp32p4-d1001",
            "arch": "riscv32",
            "platform": "esp32",
            "bsp": "esp32p4",
            "toolchain_profile": "rv32-p4-local"
        }))
        .expect("deserialize minimal explicit source profile");
        assert!(manifest_profile_matches_source(&profile, "rv32-p4-local"));
        assert!(!manifest_profile_matches_source(&profile, "esp32p4-d1001"));
        assert!(!manifest_profile_matches_source(&profile, "rv32-p4-other"));
    }

    #[cfg(unix)]
    fn fake_objdump(directory: &Path, body: &str) -> PathBuf {
        let path = directory.join("objdump");
        fs::write(&path, format!("#!/bin/sh\n{body}"))
            .expect("write temporary fake declared objdump");
        let mut permissions = fs::metadata(&path)
            .expect("inspect temporary fake objdump")
            .permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&path, permissions).expect("make fake declared objdump executable");
        path
    }

    #[cfg(unix)]
    fn fixture_ranges() -> (
        super::super::native_residency::AddressRange,
        super::super::native_residency::AddressRange,
    ) {
        (
            super::super::native_residency::AddressRange::new(0x4000_0000, 0x4200_0000)
                .expect("valid flash range"),
            super::super::native_residency::AddressRange::new(0x4ff0_0000, 0x5000_0000)
                .expect("valid SRAM range"),
        )
    }

    #[cfg(unix)]
    const VALID_DISASSEMBLY: &str = r"printf '%s\n' \
'/fixture/objdump: file format elf32-littleriscv' \
'' \
'Disassembly of section .sramtext:' \
'' \
'4ff00000 <probe>:' \
'4ff00000:	00000013	nop'
";

    #[cfg(unix)]
    #[test]
    fn invokes_declared_objdump_with_exact_arguments_and_checks_complete_output() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let objdump = fake_objdump(
            directory.path(),
            &format!(
                r#"[ "$1" = "-dr" ] || exit 91
[ "$2" = "--section=.sramtext" ] || exit 92
case "$3" in *.elf) ;; *) exit 93 ;; esac
{VALID_DISASSEMBLY}"#
            ),
        );
        let elf = directory.path().join("core.elf");
        fs::write(&elf, b"not parsed by fake tool").expect("write temp image placeholder");
        let (flash, sram) = fixture_ranges();
        let offending = run_residency_check(
            &objdump,
            &elf,
            ".sramtext",
            flash,
            sram,
            OBJDUMP_CAPTURE_BYTES,
            Duration::from_secs(2),
        )
        .expect("valid complete objdump output");
        assert!(offending.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_malformed_objdump_output_and_nonzero_exit() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let elf = directory.path().join("core.elf");
        fs::write(&elf, b"placeholder").expect("write temp image placeholder");
        let (flash, sram) = fixture_ranges();
        let malformed = fake_objdump(
            directory.path(),
            "printf '%s\\n' 'Disassembly of section .sramtext:' '4ff00000:\\t00000013\\tnop'\n",
        );
        let error = run_residency_check(
            &malformed,
            &elf,
            ".sramtext",
            flash,
            sram,
            OBJDUMP_CAPTURE_BYTES,
            Duration::from_secs(2),
        )
        .expect_err("bannerless objdump output is rejected");
        assert!(format!("{error:#}").contains("malformed"));

        let failing = fake_objdump(directory.path(), "exit 17\n");
        let error = run_residency_check(
            &failing,
            &elf,
            ".sramtext",
            flash,
            sram,
            OBJDUMP_CAPTURE_BYTES,
            Duration::from_secs(2),
        )
        .expect_err("failed objdump process is rejected");
        assert!(error.to_string().contains("exited unsuccessfully"));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_objdump_timeout_and_truncated_capture() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let elf = directory.path().join("core.elf");
        fs::write(&elf, b"placeholder").expect("write temp image placeholder");
        let (flash, sram) = fixture_ranges();
        let slow = fake_objdump(directory.path(), "sleep 2\n");
        let error = run_residency_check(
            &slow,
            &elf,
            ".sramtext",
            flash,
            sram,
            OBJDUMP_CAPTURE_BYTES,
            Duration::from_millis(50),
        )
        .expect_err("deadline is enforced");
        assert!(error.to_string().contains("exceeded its deadline"));

        let verbose = fake_objdump(directory.path(), "head -c 256 /dev/zero\n");
        let error = run_residency_check(
            &verbose,
            &elf,
            ".sramtext",
            flash,
            sram,
            128,
            Duration::from_secs(2),
        )
        .expect_err("truncated process output is rejected");
        assert!(error.to_string().contains("complete-output limit"));
    }
}
