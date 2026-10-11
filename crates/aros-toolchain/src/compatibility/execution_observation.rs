//! Pre-execution compatibility inputs, transported separately from results.

use std::collections::BTreeMap;

use aros_common::{
    measure_regular_file_bounded, sha256_bytes, ArosCompilerIdentity, CancellationToken,
    Sha256Digest,
};
use serde::{Deserialize, Serialize};

use super::portable::export_with_expectations;
use super::receipt_readback::reject_duplicate_json_keys;
use super::retained::ExpectedRetainedEvidence;
use super::{
    execute_native_compatibility, package_binding_required, NativeCompatibilityExpectedPackage,
    NativeCompatibilityExpectedPortSource, NativeCompatibilityReceiptExpectations,
    NativeCompatibilityReport, NativeCompatibilityRequest, PortableNativeCompatibilityExport,
};
use crate::compatibility::{CompatibilityHelperReport, CompatibilityHostToolReport};
use crate::profiles::{Profile, Profiles};
use crate::recipe::GitObjectId;
use crate::ContractError;

const SCHEMA: &str = "aros-toolchain-compatibility-inputs-v1";
const SDK_SCHEMA: &str = "aros-toolchain-compatibility-inputs-v2";
const MAX_STANDALONE_FIXTURE_BYTES: u64 = 1024 * 1024;

/// Bounded content identity only; this makes no claim about a fixture path or
/// filesystem identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StandaloneFixtureIdentity {
    sha256: Sha256Digest,
    size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StandaloneFixtureIdentities {
    c: StandaloneFixtureIdentity,
    cxx: StandaloneFixtureIdentity,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InputDocument {
    schema: String,
    package_manifest_sha256: Sha256Digest,
    archive_sha256: Sha256Digest,
    archive_size: u64,
    compiler: ArosCompilerIdentity,
    package_source_commit: GitObjectId,
    host: String,
    profiles_sha256: Sha256Digest,
    target_profile: String,
    target_triple: String,
    source_preset: Option<String>,
    cmake_build_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    native_sdk: Option<crate::compatibility::NativeSdkLinkSelection>,
    sdk_source_tree_sha256: Sha256Digest,
    engine_api_version: u32,
    engine_sha256: Sha256Digest,
    helpers: BTreeMap<String, CompatibilityHelperReport>,
    host_tools: BTreeMap<String, CompatibilityHostToolReport>,
    standalone_c_fixture: StandaloneFixtureIdentity,
    standalone_cxx_fixture: StandaloneFixtureIdentity,
    sdk_environment_sha256: Sha256Digest,
    standalone_environment_sha256: Sha256Digest,
    upstream_source_commit: GitObjectId,
    upstream_source_tree: GitObjectId,
    ports_sources: Vec<NativeCompatibilityExpectedPortSource>,
}

/// Exact input document and its claims, not authenticated execution evidence.
///
/// The execution factory measures these values before any compatibility phase.
/// Parsing downloaded bytes cannot recreate that observation or authenticate
/// its runtime, source, environment, job or artifact. The protected collector
/// must verify origin and bind this exact document independently of reports.
#[derive(Debug, Clone)]
pub struct NativeCompatibilityInputClaims {
    document: InputDocument,
    bytes: Vec<u8>,
    sha256: Sha256Digest,
}

impl NativeCompatibilityInputClaims {
    /// Parse one externally selected, canonical and closed input document.
    ///
    /// # Errors
    /// Returns AX0703 for changed raw bytes, unknown/duplicate/noncanonical
    /// metadata, oversize documents, or another schema. Parsing grants no trust.
    pub fn parse(bytes: &[u8], selected_sha256: &Sha256Digest) -> Result<Self, ContractError> {
        if bytes.is_empty() || bytes.len() > crate::canonical::MAX_DOCUMENT_BYTES {
            return Err(error(
                "compatibility input observation exceeds its metadata limit",
            ));
        }
        if sha256_bytes(bytes) != *selected_sha256 {
            return Err(error(
                "compatibility input observation differs from its selected raw digest",
            ));
        }
        reject_duplicate_json_keys(bytes)?;
        let document: InputDocument = serde_json::from_slice(bytes)
            .map_err(|_| error("compatibility input observation is not closed JSON"))?;
        if (document.schema != SCHEMA && document.schema != SDK_SCHEMA)
            || (document.schema == SDK_SCHEMA) != document.native_sdk.is_some()
        {
            return Err(error(
                "compatibility input observation has an unsupported schema",
            ));
        }
        if let Some(selection) = &document.native_sdk {
            selection.validate()?;
            if !document.cmake_build_required
                || !matches!(document.compiler, ArosCompilerIdentity::Gnu { .. })
            {
                return Err(error(
                    "SDK input observation requires a GNU native consumer build",
                ));
            }
        }
        if [
            document.standalone_c_fixture.size,
            document.standalone_cxx_fixture.size,
        ]
        .iter()
        .any(|size| !(1..=MAX_STANDALONE_FIXTURE_BYTES).contains(size))
        {
            return Err(error(
                "compatibility input observation has an invalid standalone fixture size",
            ));
        }
        if encode(&document)? != bytes {
            return Err(error(
                "compatibility input observation is not canonical JSON",
            ));
        }
        Ok(Self {
            document,
            bytes: bytes.to_vec(),
            sha256: selected_sha256.clone(),
        })
    }

    /// Exact bytes to upload as protected input evidence, not a release asset.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Raw digest to retain outside the uploaded input document.
    #[must_use]
    pub const fn sha256(&self) -> &Sha256Digest {
        &self.sha256
    }

    /// Rebind downloaded input claims to an independently verified package and
    /// selected profiles. Other measured identities remain unauthenticated
    /// claims until the owning-job/runtime/source evidence has been verified.
    /// Never select this document or its raw digest from a receipt's own fields.
    /// `native_sdk_required` must be selected from the independently qualified
    /// source contract, never inferred from this input document's SDK marker.
    ///
    /// # Errors
    /// Returns AX0703 for package/compiler/source/host/profile substitutions.
    pub fn expectations<'a>(
        &'a self,
        package: NativeCompatibilityExpectedPackage<'a>,
        profiles: &'a Profiles,
        profile: &'a Profile,
        native_sdk_required: bool,
    ) -> Result<NativeCompatibilityReceiptExpectations<'a>, ContractError> {
        let doc = &self.document;
        if doc.native_sdk.is_some() != native_sdk_required {
            return Err(error(
                "SDK requirement differs from independent source policy",
            ));
        }
        let selected = profiles.select(profile.name())?;
        let manifest = crate::canonical::bytes(
            &serde_json::to_value(package.manifest)
                .map_err(|_| error("cannot encode expected compatibility package manifest"))?,
        )?;
        if doc.package_manifest_sha256 != sha256_bytes(&manifest)
            || &doc.archive_sha256 != package.archive_sha256
            || doc.archive_size != package.archive_size
            || &doc.compiler != package.compiler
            || &doc.package_source_commit != package.source_commit
            || doc.host != package.host
            || doc.profiles_sha256 != *profile.document_sha256()
            || doc.profiles_sha256 != *selected.document_sha256()
            || doc.target_profile != profile.name()
            || doc.target_triple != profile.target_triple()
            || &doc.upstream_source_commit != profiles.upstream_commit()
        {
            return Err(error(
                "compatibility input observation differs from independently selected package or profiles",
            ));
        }
        Ok(NativeCompatibilityReceiptExpectations {
            package,
            profiles,
            profile,
            gnu_source_preset: doc.source_preset.as_deref(),
            cmake_build_required: doc.cmake_build_required,
            native_sdk: doc.native_sdk.as_ref(),
            sdk_consumer_source_tree_sha256: &doc.sdk_source_tree_sha256,
            engine_api_version: doc.engine_api_version,
            engine_sha256: &doc.engine_sha256,
            helpers: &doc.helpers,
            host_tools: &doc.host_tools,
            sdk_environment_sha256: &doc.sdk_environment_sha256,
            standalone_environment_sha256: &doc.standalone_environment_sha256,
            upstream_source_commit: &doc.upstream_source_commit,
            upstream_source_tree: &doc.upstream_source_tree,
            ports_sources: &doc.ports_sources,
        })
    }

    fn from_expected(
        expected: &NativeCompatibilityReceiptExpectations<'_>,
        standalone_fixtures: StandaloneFixtureIdentities,
    ) -> Result<Self, ContractError> {
        let manifest = crate::canonical::bytes(
            &serde_json::to_value(expected.package.manifest)
                .map_err(|_| error("cannot encode observed compatibility package manifest"))?,
        )?;
        let document = InputDocument {
            schema: if expected.native_sdk.is_some() {
                SDK_SCHEMA
            } else {
                SCHEMA
            }
            .into(),
            package_manifest_sha256: sha256_bytes(&manifest),
            archive_sha256: expected.package.archive_sha256.clone(),
            archive_size: expected.package.archive_size,
            compiler: expected.package.compiler.clone(),
            package_source_commit: expected.package.source_commit.clone(),
            host: expected.package.host.into(),
            profiles_sha256: expected.profile.document_sha256().clone(),
            target_profile: expected.profile.name().into(),
            target_triple: expected.profile.target_triple().into(),
            source_preset: expected.gnu_source_preset.map(str::to_owned),
            cmake_build_required: expected.cmake_build_required,
            native_sdk: expected.native_sdk.cloned(),
            sdk_source_tree_sha256: expected.sdk_consumer_source_tree_sha256.clone(),
            engine_api_version: expected.engine_api_version,
            engine_sha256: expected.engine_sha256.clone(),
            helpers: expected.helpers.clone(),
            host_tools: expected.host_tools.clone(),
            standalone_c_fixture: standalone_fixtures.c,
            standalone_cxx_fixture: standalone_fixtures.cxx,
            sdk_environment_sha256: expected.sdk_environment_sha256.clone(),
            standalone_environment_sha256: expected.standalone_environment_sha256.clone(),
            upstream_source_commit: expected.upstream_source_commit.clone(),
            upstream_source_tree: expected.upstream_source_tree.clone(),
            ports_sources: expected.ports_sources.to_vec(),
        };
        let bytes = encode(&document)?;
        Self::parse(&bytes, &sha256_bytes(&bytes))
    }
}

/// One locally executed compatibility operation and its separate byte closures.
///
/// No constructor or parser can manufacture this execution result from claims.
/// It does not authenticate external workflow execution or grant release rights.
#[derive(Debug)]
pub struct NativeCompatibilityExecutionExport {
    report: NativeCompatibilityReport,
    inputs: NativeCompatibilityInputClaims,
    evidence: PortableNativeCompatibilityExport,
}

impl NativeCompatibilityExecutionExport {
    /// The six-phase locally executed result.
    #[must_use]
    pub const fn report(&self) -> &NativeCompatibilityReport {
        &self.report
    }
    /// Inputs measured before compatibility execution, separate from results.
    #[must_use]
    pub const fn inputs(&self) -> &NativeCompatibilityInputClaims {
        &self.inputs
    }
    /// Closed reports/logs/ELFs acquired against those same prior expectations.
    #[must_use]
    pub const fn evidence(&self) -> &PortableNativeCompatibilityExport {
        &self.evidence
    }
}

/// Execute one family-v2 compatibility lane and export its byte-bound evidence.
///
/// Captures independently prepared inputs before any of the six phases starts.
/// After execution, retained read-back and portable export reuse those exact
/// pre-execution expectations rather than copying fields out of a report. C
/// and C++ fixture contents are measured as bounded no-follow regular-file
/// snapshots and checked before execution, after execution, and after export.
/// Their claims bind raw content size and SHA-256 only, not paths or filesystem
/// identities. Original roots must remain exclusively owned and quiescent;
/// these checks do not defeat a deliberate mutate-and-restore race. It writes
/// only the normal execution outputs; the caller publishes the returned
/// protected export separately with no-clobber semantics. No signing, external
/// origin authentication, qualification, recovery or release admission occurs.
///
/// # Errors
/// Returns AX0703 for legacy packages, input drift or inconsistent retained
/// evidence, and propagates execution failures. Never exports partial phases.
pub fn execute_native_compatibility_with_export(
    request: &NativeCompatibilityRequest,
    profiles: &Profiles,
    cancellation: &CancellationToken,
) -> Result<NativeCompatibilityExecutionExport, ContractError> {
    if !package_binding_required(request) {
        return Err(error(
            "compatibility execution export requires a compiler-family-v2 package",
        ));
    }
    let standalone_fixtures = measure_standalone_fixtures(request)?;
    let expected = ExpectedRetainedEvidence::prepare(request, profiles)?;
    let inputs = NativeCompatibilityInputClaims::from_expected(
        &expected.portable_expectations(request, profiles)?,
        standalone_fixtures.clone(),
    )?;
    revalidate_standalone_fixtures(request, &standalone_fixtures)?;
    let report = execute_native_compatibility(request, cancellation)?;
    revalidate_standalone_fixtures(request, &standalone_fixtures)?;
    let evidence = export_with_expectations(request, profiles, &expected)?;
    revalidate_standalone_fixtures(request, &standalone_fixtures)?;
    if evidence.receipt_sha256() != &report.receipt.sha256 {
        return Err(error(
            "exported compatibility receipt differs from its execution digest",
        ));
    }
    Ok(NativeCompatibilityExecutionExport {
        report,
        inputs,
        evidence,
    })
}

fn measure_standalone_fixtures(
    request: &NativeCompatibilityRequest,
) -> Result<StandaloneFixtureIdentities, ContractError> {
    Ok(StandaloneFixtureIdentities {
        c: measure_standalone_fixture(&request.standalone_fixtures.c, "C")?,
        cxx: measure_standalone_fixture(&request.standalone_fixtures.cxx, "C++")?,
    })
}

fn measure_standalone_fixture(
    path: &std::path::Path,
    language: &str,
) -> Result<StandaloneFixtureIdentity, ContractError> {
    let (_, bytes) = measure_regular_file_bounded(path, MAX_STANDALONE_FIXTURE_BYTES)
        .map_err(|_| {
            error(&format!(
                "standalone {language} compatibility fixture cannot be safely measured"
            ))
        })?
        .ok_or_else(|| {
            error(&format!(
                "standalone {language} compatibility fixture cannot be safely measured"
            ))
        })?;
    if bytes.is_empty() {
        return Err(error(&format!(
            "standalone {language} compatibility fixture must not be empty"
        )));
    }
    let size = u64::try_from(bytes.len())
        .map_err(|_| error("standalone compatibility fixture size is not representable"))?;
    Ok(StandaloneFixtureIdentity {
        sha256: sha256_bytes(&bytes),
        size,
    })
}

fn revalidate_standalone_fixtures(
    request: &NativeCompatibilityRequest,
    expected: &StandaloneFixtureIdentities,
) -> Result<(), ContractError> {
    let observed = measure_standalone_fixtures(request)
        .map_err(|_| error("standalone compatibility fixture changed after input observation"))?;
    if &observed != expected {
        return Err(error(
            "standalone compatibility fixture changed after input observation",
        ));
    }
    Ok(())
}

fn encode(document: &InputDocument) -> Result<Vec<u8>, ContractError> {
    crate::canonical::bytes(
        &serde_json::to_value(document)
            .map_err(|_| error("cannot encode compatibility input observation"))?,
    )
}

fn error(message: &str) -> ContractError {
    ContractError::compatibility(message)
}

#[cfg(test)]
#[path = "execution_observation_tests.rs"]
mod tests;
