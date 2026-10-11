//! Bind ordinary LLVM links to the source-selected SDK in a retained CMake build.
//!
//! A CMake binding is not a build-success proof. This boundary independently
//! revalidates that binding through the measured source validator and retains
//! the selected SDK tree. The generation/release owner must separately establish
//! and bind six-phase generation and authenticated job evidence before admission.

use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use aros_common::sdk_consumer_contract::{
    load_bound_sdk_consumer_contract, LoadedSdkConsumerContract, SDK_CONSUMER_CONTRACT_SCHEMA,
};
use aros_common::sdk_manifest::SdkTarget;
use aros_common::{
    measure_tree_content_cas_bounded, run_output_with_input_and_control, sha256_bytes,
    CancellationToken, Sha256Digest, TreeContentCas, TreeTraversalLimits,
};
use serde::{Deserialize, Serialize};

use super::{
    checked_directory, execute_llvm_sdk_links, llvm_sdk_application, read_regular_bounded,
    readback_llvm_sdk_links, validate_preparation, CompatibilityPreparation, CompletedLlvmSdkLinks,
    LlvmSdkDriverBinding, LlvmSdkLinkRequest,
};
use crate::sdk_source::VerifiedSdkSource;
use crate::{canonical, ContractError};

const BINDING_FILE: &str = "aros-native-consumer-binding.json";
const SDK_LIMITS: TreeTraversalLimits = TreeTraversalLimits {
    max_entries: 100_000,
    max_regular_file_bytes: 2 * 1024 * 1024 * 1024,
};

/// Independent committed source, measured engine/helpers and existing build.
#[derive(Debug, Clone)]
pub struct LlvmSdkGeneratedRootRequest {
    /// Exact clean source and complete scalar SDK identity.
    pub source: VerifiedSdkSource,
    /// Embedded engine and measured helper programs for this source projection.
    pub preparation: CompatibilityPreparation,
    /// Retained consumer build; never an arbitrary caller-selected SDK root.
    pub cmake_build_root: PathBuf,
    /// Positive elapsed budget; no successful result is returned after expiry.
    /// Filesystem I/O and existing bounded Git/source audits are not preempted.
    pub timeout: Duration,
}

/// Opaque source-derived SDK selection, not six-phase generation completion.
#[derive(Debug)]
pub struct LlvmSdkGeneratedRoot {
    request: LlvmSdkGeneratedRootRequest,
    sdk_root: PathBuf,
    binding_bytes: Vec<u8>,
    sdk_snapshot: TreeContentCas,
}

impl LlvmSdkGeneratedRoot {
    /// Canonical SDK root derived from the exact source validator response.
    #[must_use]
    pub fn sdk_root(&self) -> &Path {
        &self.sdk_root
    }

    /// Digest of the exact retained CMake binding, including its source identity.
    #[must_use]
    pub fn binding_sha256(&self) -> Sha256Digest {
        sha256_bytes(&self.binding_bytes)
    }

    /// Read the current binding/source through the measured read-only helper.
    ///
    /// # Errors
    /// Rejects changed binding bytes, helpers, source, SDK content or modes.
    /// No CMake, compiler, build, repair or publication occurs.
    pub fn revalidate(&self, cancellation: &CancellationToken) -> Result<(), ContractError> {
        self.revalidate_with_timeout(self.request.timeout, cancellation)
    }

    fn revalidate_with_timeout(
        &self,
        timeout: Duration,
        cancellation: &CancellationToken,
    ) -> Result<(), ContractError> {
        let mut request = self.request.clone();
        request.timeout = timeout;
        let current = bind_llvm_sdk_generated_root(&request, cancellation)?;
        if current.sdk_root != self.sdk_root
            || current.binding_bytes != self.binding_bytes
            || current.sdk_snapshot != self.sdk_snapshot
        {
            return Err(error("LLVM generated SDK binding or tree changed"));
        }
        Ok(())
    }
}

/// Ordinary link proof that cannot replace its source-selected CMake SDK.
#[derive(Debug)]
pub struct CompletedLlvmSdkBuildLinks {
    generated: LlvmSdkGeneratedRoot,
    links: CompletedLlvmSdkLinks,
}

impl CompletedLlvmSdkBuildLinks {
    /// The exact original/relocated application evidence from this build SDK.
    #[must_use]
    pub const fn links(&self) -> &CompletedLlvmSdkLinks {
        &self.links
    }

    /// Source-derived original SDK, not a separately supplied packaging input.
    #[must_use]
    pub fn sdk_root(&self) -> &Path {
        self.generated.sdk_root()
    }

    /// Exact CMake/source binding retained by the owning execution.
    #[must_use]
    pub fn binding_sha256(&self) -> Sha256Digest {
        self.generated.binding_sha256()
    }

    /// Reaudit both the owning build binding and the complete four-link closure.
    ///
    /// # Errors
    /// Rejects changed source/build/SDK/helper/compiler/evidence; never repairs.
    pub fn revalidate(&self, cancellation: &CancellationToken) -> Result<(), ContractError> {
        let started = Instant::now();
        let timeout = self.generated.request.timeout;
        self.generated
            .revalidate_with_timeout(remaining(started, timeout, cancellation)?, cancellation)?;
        self.links.revalidate(cancellation)?;
        self.generated
            .revalidate_with_timeout(remaining(started, timeout, cancellation)?, cancellation)?;
        remaining(started, timeout, cancellation)?;
        Ok(())
    }
}

/// Resolve the source-owned SDK only after checking retained and fresh bindings.
///
/// The helper independently rederives the include directory from sealed source
/// configuration. A matching target descriptor alone cannot select another SDK.
/// This validates selection and existing bytes, not build success or SDK completeness.
///
/// # Errors
/// Rejects unbound source/preparation, unsafe/overlapping roots, a malformed or
/// stale CMake response, failed/truncated helper output and changed inputs.
/// Never writes a file, adopts another path, or falls back to a conventional root.
pub fn bind_llvm_sdk_generated_root(
    request: &LlvmSdkGeneratedRootRequest,
    cancellation: &CancellationToken,
) -> Result<LlvmSdkGeneratedRoot, ContractError> {
    let started = Instant::now();
    remaining(started, request.timeout, cancellation)?;
    let loaded = bound_source(request)?;
    let build = real_build_root(request)?;
    let path = build.join(BINDING_FILE);
    let bytes = read_regular_bounded(
        &path,
        "LLVM SDK CMake binding",
        canonical::MAX_DOCUMENT_BYTES,
    )?;
    let persisted: ConsumerBinding = serde_json::from_slice(&bytes)
        .map_err(|_| error("LLVM SDK CMake binding is not closed JSON"))?;
    validate_response(&persisted, request, &loaded)?;
    let fresh = current_binding(
        request,
        &loaded,
        remaining(started, request.timeout, cancellation)?,
        cancellation,
    )?;
    validate_response(&fresh, request, &loaded)?;
    if fresh != persisted {
        return Err(error(
            "LLVM SDK CMake binding differs from fresh source validation",
        ));
    }
    let sdk_root = resolve_sdk_root(&build, &fresh.sdk_include_relative)?;
    let sdk_snapshot = measure_sdk(&sdk_root)?;
    // The source validator is a measured tool, not a filesystem snapshot or a
    // writer lock. Reopen every selected binding after it returned.
    if bound_source(request)? != loaded
        || real_build_root(request)? != build
        || read_regular_bounded(
            &path,
            "LLVM SDK CMake binding",
            canonical::MAX_DOCUMENT_BYTES,
        )? != bytes
        || resolve_sdk_root(&build, &fresh.sdk_include_relative)? != sdk_root
        || measure_sdk(&sdk_root)? != sdk_snapshot
    {
        return Err(error("LLVM SDK build selection changed during validation"));
    }
    remaining(started, request.timeout, cancellation)?;
    Ok(LlvmSdkGeneratedRoot {
        request: LlvmSdkGeneratedRootRequest {
            cmake_build_root: build,
            ..request.clone()
        },
        sdk_root,
        binding_bytes: bytes,
        sdk_snapshot,
    })
}

/// Execute four ordinary LLVM links using only the SDK selected by this build.
///
/// Uses one elapsed-time budget across binding, links and final revalidation.
/// Existing bounded Git/source audits and filesystem I/O are not preempted;
/// expiration rejects a successful result rather than guaranteeing return time.
/// Failure retains link diagnostics. It does not assert six-phase generation,
/// authenticate a compiler package/job, export portable evidence or admit a release.
///
/// # Errors
/// Rejects invalid build selection before creating link outputs, failed links,
/// changed inputs or an expired/cancelled budget; never retries or publishes.
pub fn execute_llvm_sdk_build_links(
    request: &LlvmSdkGeneratedRootRequest,
    drivers: &LlvmSdkDriverBinding,
    output_root: &Path,
    cancellation: &CancellationToken,
) -> Result<CompletedLlvmSdkBuildLinks, ContractError> {
    let started = Instant::now();
    if drivers.target() != &request.source.identity().target {
        return Err(error(
            "LLVM SDK build drivers differ from the complete source target",
        ));
    }
    let generated = bind_llvm_sdk_generated_root(request, cancellation)?;
    let output_root = disjoint_output(output_root, &generated.request.cmake_build_root)?;
    let links = execute_llvm_sdk_links(
        &LlvmSdkLinkRequest {
            source: request.source.clone(),
            drivers: drivers.clone(),
            preparation: request.preparation.clone(),
            sdk_root: generated.sdk_root.clone(),
            output_root,
            timeout: remaining(started, request.timeout, cancellation)?,
        },
        cancellation,
    )?;
    generated.revalidate_with_timeout(
        remaining(started, request.timeout, cancellation)?,
        cancellation,
    )?;
    remaining(started, request.timeout, cancellation)?;
    Ok(CompletedLlvmSdkBuildLinks { generated, links })
}

/// Read retained links against the independently selected source/CMake build.
///
/// Both expected digests must be retained by the execution owner, not selected
/// from the evidence being audited. Source and driver-tree audits run, but
/// no SDK build, application link, copy, repair or output write occurs. The
/// single elapsed budget prevents returning success after expiration; existing
/// bounded source audits and filesystem I/O are not preempted.
///
/// # Errors
/// Rejects changed source/build/binding/SDK/evidence, unsafe overlap or an
/// expired/cancelled budget. This does not authenticate a remote CI execution.
pub fn readback_llvm_sdk_build_links(
    request: &LlvmSdkGeneratedRootRequest,
    drivers: &LlvmSdkDriverBinding,
    output_root: &Path,
    expected_binding_sha256: &Sha256Digest,
    expected_receipt_sha256: &Sha256Digest,
    cancellation: &CancellationToken,
) -> Result<CompletedLlvmSdkBuildLinks, ContractError> {
    let started = Instant::now();
    let generated = bind_llvm_sdk_generated_root(request, cancellation)?;
    if generated.binding_sha256() != *expected_binding_sha256 {
        return Err(error(
            "retained LLVM SDK build binding differs from its independent digest",
        ));
    }
    let output_root = disjoint_output(output_root, &generated.request.cmake_build_root)?;
    let links = readback_llvm_sdk_links(
        &LlvmSdkLinkRequest {
            source: request.source.clone(),
            drivers: drivers.clone(),
            preparation: request.preparation.clone(),
            sdk_root: generated.sdk_root.clone(),
            output_root,
            timeout: remaining(started, request.timeout, cancellation)?,
        },
        expected_receipt_sha256,
        cancellation,
    )?;
    generated.revalidate_with_timeout(
        remaining(started, request.timeout, cancellation)?,
        cancellation,
    )?;
    remaining(started, request.timeout, cancellation)?;
    Ok(CompletedLlvmSdkBuildLinks { generated, links })
}

fn disjoint_output(path: &Path, build: &Path) -> Result<PathBuf, ContractError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err(error(
            "LLVM SDK build link output must be an absolute non-traversing leaf",
        ));
    }
    let parent = checked_directory(
        path.parent()
            .ok_or_else(|| error("LLVM SDK build link output parent is absent"))?,
        "LLVM SDK build link output parent",
    )?;
    let output = parent.join(
        path.file_name()
            .ok_or_else(|| error("LLVM SDK build link output leaf is absent"))?,
    );
    if output.starts_with(build) || build.starts_with(&output) {
        return Err(error(
            "LLVM SDK build link output overlaps the selected CMake build",
        ));
    }
    Ok(output)
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
    pub(super) target: SdkTarget,
    pub(super) exec_smp: bool,
    pub(super) input_paths: Vec<String>,
    pub(super) sdk_include_relative: String,
}

/// Select the source-derived binding before CMake creates any output.
/// This probes only the measured read-only source validator. The selected
/// relative SDK path is lexical policy, not proof that SDK bytes exist.
pub(super) fn selected_binding(
    request: &LlvmSdkGeneratedRootRequest,
    cancellation: &CancellationToken,
) -> Result<ConsumerBinding, ContractError> {
    let started = Instant::now();
    remaining(started, request.timeout, cancellation)?;
    let loaded = bound_source(request)?;
    let binding = current_binding(
        request,
        &loaded,
        remaining(started, request.timeout, cancellation)?,
        cancellation,
    )?;
    validate_response(&binding, request, &loaded)?;
    validate_include_relative(&binding.sdk_include_relative)?;
    if bound_source(request)? != loaded {
        return Err(error(
            "LLVM SDK source changed during pre-execution selection",
        ));
    }
    remaining(started, request.timeout, cancellation)?;
    Ok(binding)
}

fn bound_source(
    request: &LlvmSdkGeneratedRootRequest,
) -> Result<LoadedSdkConsumerContract, ContractError> {
    if request.source.consumer_contract_schema() != SDK_CONSUMER_CONTRACT_SCHEMA
        || request.preparation.source_root != request.source.materialized_source_root()
        || request.preparation.source_tree_sha256 != request.source.identity().source.content_sha256
    {
        return Err(error("LLVM SDK build source and preparation differ"));
    }
    request.source.revalidate()?;
    validate_preparation(&request.preparation)?;
    let profile = request.source.source_profile();
    if profile
        .transpiler
        .as_ref()
        .is_none_or(|context| context.toolchain != "llvm")
    {
        return Err(error("LLVM SDK build source must explicitly select LLVM"));
    }
    llvm_sdk_application::validate_target(&request.source.identity().target)?;
    let relative = profile
        .sdk_consumer_contract
        .as_deref()
        .ok_or_else(|| error("LLVM SDK build source contract is absent"))?;
    load_bound_sdk_consumer_contract(
        request.source.materialized_source_root(),
        Path::new(relative),
        profile,
    )
    .map_err(|_| error("LLVM SDK build source contract changed"))
}

fn real_build_root(request: &LlvmSdkGeneratedRootRequest) -> Result<PathBuf, ContractError> {
    if !request.cmake_build_root.is_absolute()
        || request
            .cmake_build_root
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err(error(
            "LLVM SDK CMake build must be an absolute non-traversing root",
        ));
    }
    let build = checked_directory(&request.cmake_build_root, "LLVM SDK CMake build")?;
    let preparation = validate_preparation(&request.preparation)?;
    for protected in [
        request.source.original_source_root(),
        preparation.source.as_path(),
        preparation.engine.as_path(),
        preparation.helpers.as_path(),
    ] {
        if build.starts_with(protected) || protected.starts_with(&build) {
            return Err(error("LLVM SDK CMake build overlaps a protected input"));
        }
    }
    Ok(build)
}

fn validate_response(
    binding: &ConsumerBinding,
    request: &LlvmSdkGeneratedRootRequest,
    loaded: &LoadedSdkConsumerContract,
) -> Result<(), ContractError> {
    let mut exec_smp = None;
    for template in loaded.contract.generated_make_templates.values() {
        if let Some(value) = template.substitutions.get("@ENABLE_EXECSMP@") {
            let selected = match value.as_str() {
                "" => false,
                "#define __AROSEXEC_SMP__" => true,
                _ => {
                    return Err(error(
                        "LLVM SDK source has an invalid Exec SMP substitution",
                    ))
                }
            };
            if exec_smp.is_some_and(|previous| previous != selected) {
                return Err(error("LLVM SDK source templates disagree on Exec SMP"));
            }
            exec_smp = Some(selected);
        }
    }
    let paths: Vec<_> = loaded
        .contract
        .inputs
        .iter()
        .map(|input| input.path.clone())
        .collect();
    if binding.schema != "aros-sdk-consumer-validation-v1"
        || binding.qualification != "source-binding-not-graph-or-build-proof"
        || Path::new(&binding.source_dir) != request.source.materialized_source_root()
        || Path::new(&binding.contract_path) != loaded.path
        || binding.contract_sha256 != loaded.sha256
        || binding.profile != loaded.contract.profile
        || binding.target != request.source.identity().target
        || binding.exec_smp != exec_smp.unwrap_or(false)
        || binding.input_paths != paths
    {
        return Err(error(
            "LLVM SDK CMake binding differs from its exact source selection",
        ));
    }
    Ok(())
}

fn current_binding(
    request: &LlvmSdkGeneratedRootRequest,
    loaded: &LoadedSdkConsumerContract,
    timeout: Duration,
    cancellation: &CancellationToken,
) -> Result<ConsumerBinding, ContractError> {
    let profile = request.source.source_profile();
    let context = profile
        .transpiler
        .as_ref()
        .ok_or_else(|| error("LLVM SDK source lost its explicit selector context"))?;
    let helper = request
        .preparation
        .helpers
        .get("aros-transpiler")
        .ok_or_else(|| error("LLVM SDK source validator is absent"))?;
    let mut command = Command::new(&helper.path);
    command
        .env_clear()
        .env("PATH", "/nonexistent")
        .env("LC_ALL", "C")
        .current_dir(request.source.materialized_source_root())
        .args([
            "--validate-native-consumer-only".into(),
            "--native-consumer-profile".into(),
            profile.name.clone(),
            "--native-consumer-contract-sha256".into(),
            loaded.sha256.to_string(),
            "--source-dir".into(),
            text_path(request.source.materialized_source_root())?,
            "--cpu".into(),
            profile.arch.source_cpu(),
            "--platform".into(),
            profile.platform.clone(),
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
            profile.float_abi.clone().unwrap_or_default(),
            "--mesa-version".into(),
            context.mesa_version.clone().unwrap_or_default(),
        ]);
    let output = run_output_with_input_and_control(
        &mut command,
        &[],
        canonical::MAX_DOCUMENT_BYTES,
        timeout.min(Duration::from_secs(60)),
        cancellation,
    )
    .map_err(|_| error("cannot supervise LLVM SDK source validation"))?;
    if !output.status.success() || output.timed_out || output.cancelled {
        return Err(error(
            "LLVM SDK source validator did not complete successfully",
        ));
    }
    let bytes = output
        .stdout
        .exact_bytes()
        .ok_or_else(|| error("LLVM SDK source validator response was truncated"))?;
    serde_json::from_slice(bytes)
        .map_err(|_| error("LLVM SDK source validator response is not closed JSON"))
}

fn resolve_sdk_root(build: &Path, include: &str) -> Result<PathBuf, ContractError> {
    let parent = validate_include_relative(include)?;
    let root = checked_directory(&build.join(parent), "source-selected LLVM SDK")?;
    if !root.starts_with(build)
        || root == build
        || root != build.join(parent)
        || checked_directory(&root.join("include"), "source-selected LLVM SDK headers")?
            != root.join("include")
    {
        return Err(error("LLVM SDK root or headers escape the selected build"));
    }
    Ok(root)
}

fn validate_include_relative(include: &str) -> Result<&Path, ContractError> {
    if include.is_empty()
        || include.len() > 4096
        || include.split('/').any(|part| {
            part.is_empty()
                || matches!(part, "." | "..")
                || !part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_.+-".contains(&byte))
        })
    {
        return Err(error("LLVM SDK source include path is unsafe"));
    }
    let relative = Path::new(include);
    if relative.file_name().and_then(|name| name.to_str()) != Some("include") {
        return Err(error("LLVM SDK source include path must end in include"));
    }
    let parent = relative
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| error("LLVM SDK source root is absent"))?;
    Ok(parent)
}

fn measure_sdk(root: &Path) -> Result<TreeContentCas, ContractError> {
    measure_tree_content_cas_bounded(root, SDK_LIMITS)
        .map_err(|_| error("cannot measure bounded source-selected LLVM SDK"))
}

fn remaining(
    started: Instant,
    timeout: Duration,
    cancellation: &CancellationToken,
) -> Result<Duration, ContractError> {
    if cancellation.is_cancelled() {
        return Err(error("LLVM SDK build binding was cancelled"));
    }
    timeout
        .checked_sub(started.elapsed())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| error("LLVM SDK build binding deadline expired"))
}

fn text_path(path: &Path) -> Result<String, ContractError> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| error("LLVM SDK source path is not UTF-8"))
}

fn error(message: &str) -> ContractError {
    ContractError::compatibility(message)
}
