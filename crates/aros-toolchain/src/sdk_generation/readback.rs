//! Downloaded SDK generation/package joins, without execution authentication.

use aros_common::{sha256_bytes, Sha256Digest};

use super::{error, record_bytes, GenerationRecord, GnuGenerationRecordV1, LlvmGenerationRecordV1};
use crate::compatibility::{
    readback_portable_native_compatibility, PortableNativeCompatibilityRequest,
};
use crate::release_inputs::ReleaseInputs;
use crate::sdk_package::{self, SdkVerificationRequest, VerifiedSdkPackage};
use crate::source_lock::CompilerFamily;
use crate::{canonical, ContractError};

/// Independent selections for one downloaded SDK generation closure.
///
/// The input collection selects the SDK identity, source preset and compiler
/// references. Package/compatibility expectations and raw digests must be bound
/// separately to their original owning jobs before release qualification. This
/// request is not an authenticated execution assertion or a complete matrix.
#[derive(Debug)]
pub struct SdkGenerationReadbackRequest<'a> {
    pub inputs: &'a ReleaseInputs,
    pub sdk_id: &'a str,
    pub compiler_group_id: &'a str,
    /// Downloaded closed generation record; no recorded path is reopened.
    pub record_bytes: &'a [u8],
    /// Raw digest retained outside the downloaded generation record.
    pub record_sha256: &'a Sha256Digest,
    /// Independently selected SDK archive/tree expectations.
    pub package: &'a SdkVerificationRequest,
    /// Complete six-phase portable evidence with its independent expectations.
    pub compatibility: &'a PortableNativeCompatibilityRequest<'a>,
}

/// Opaque byte-consistency join of a generation record, links and actual SDK.
///
/// Does not authorize release admission, prove execution or authenticate a job.
/// Other compiler references/hosts still need their complete qualification.
#[derive(Debug)]
pub struct SdkGenerationReadback {
    record_sha256: Sha256Digest,
    package: VerifiedSdkPackage,
    compatibility_manifest_sha256: Sha256Digest,
    compatibility_receipt_sha256: Sha256Digest,
    sdk_link_receipt_sha256: Sha256Digest,
}

impl SdkGenerationReadback {
    #[must_use]
    pub const fn record_sha256(&self) -> &Sha256Digest {
        &self.record_sha256
    }

    /// Manifest and archive measurements from fresh complete SDK verification.
    #[must_use]
    pub const fn package(&self) -> &VerifiedSdkPackage {
        &self.package
    }

    #[must_use]
    pub const fn compatibility_manifest_sha256(&self) -> &Sha256Digest {
        &self.compatibility_manifest_sha256
    }

    #[must_use]
    pub const fn compatibility_receipt_sha256(&self) -> &Sha256Digest {
        &self.compatibility_receipt_sha256
    }

    #[must_use]
    pub const fn sdk_link_receipt_sha256(&self) -> &Sha256Digest {
        &self.sdk_link_receipt_sha256
    }
}

/// Reobserve one source-selected SDK generation from downloaded regular files.
///
/// The record schema is selected from the independent compiler group. A closed
/// canonical record must match the selected identity/preset/environment, actual
/// SDK package inventory and the complete portable four-link proof. Both file
/// closures are observed again; no original runner path or source is opened.
/// Callers exclusively own quiescent files. Repeated observations are not an
/// atomic snapshot, original-job authentication or release qualification.
///
/// # Errors
/// Rejects absent selections, substituted identities/groups/profiles, changed
/// record/package/evidence bytes and incomplete links. Writes nothing and never
/// executes a compiler, signs, publishes or weakens SDK qualification guards.
pub fn readback_generation(
    request: &SdkGenerationReadbackRequest<'_>,
) -> Result<SdkGenerationReadback, ContractError> {
    let sdk = request
        .inputs
        .sdks()
        .iter()
        .find(|sdk| sdk.id() == request.sdk_id)
        .ok_or_else(|| error("SDK generation readback has no independently selected product"))?;
    let group = request
        .inputs
        .groups()
        .iter()
        .find(|group| group.id() == request.compiler_group_id)
        .ok_or_else(|| {
            error("SDK generation readback has no independently selected compiler group")
        })?;
    let expected = &request.compatibility.expected;
    let profile = group.profiles().select(expected.profile.name())?;
    let compiler = crate::package_identity::compiler_identity_for_format(
        group.source_lock(),
        profile,
        crate::package::PackageFormat::CompilerFamilyV2,
    )?;
    let manifest = &expected.package.manifest;
    let manifest_compiler = expected
        .package
        .manifest
        .compiler_identity()
        .map_err(|_| error("SDK generation requires a valid family-v2 compiler manifest"))?;
    if !sdk.compiler_profiles().iter().any(|reference| {
        reference.group_id() == group.id() && reference.profile() == profile.name()
    }) || expected.profile.document_sha256() != profile.document_sha256()
        || expected.profiles.select(profile.name())?.document_sha256() != profile.document_sha256()
        || expected.upstream_source_commit != group.profiles().upstream_commit()
        || expected.upstream_source_tree != group.recipe().source().1
        || expected.package.source_commit != group.recipe().source().0
        || expected.package.manifest.schema != 2
        || manifest_compiler != compiler
        || expected.package.compiler != &compiler
        || manifest.recipe_sha256 != group.recipe().sha256().as_str()
        || manifest.source_lock_sha256 != group.source_lock().sha256().as_str()
        || manifest.profiles_sha256 != profile.document_sha256().as_str()
        || manifest.source_commit != group.recipe().source().0.as_str()
        || manifest.producer_commit != group.recipe().producer().0.as_str()
        || manifest.producer_commit != request.inputs.producer_commit().as_str()
        || manifest.tools_commit != group.recipe().tools().0.as_str()
        || manifest.tools_commit != request.inputs.tools_commit().as_str()
        || manifest.source_date_epoch != group.recipe().source_date_epoch()
        || !request
            .inputs
            .hosts()
            .iter()
            .any(|host| host == expected.package.host)
    {
        return Err(error(
            "SDK generation compiler selection differs from its independent release inputs",
        ));
    }
    if request.package.identity != *sdk.identity()
        || expected.profile.target_triple() != sdk.identity().target.target_triple
        || *expected.sdk_consumer_source_tree_sha256 != sdk.identity().source.content_sha256
    {
        return Err(error(
            "SDK generation source or package differs from its independently selected identity",
        ));
    }
    let family = group.source_lock().family();
    match family {
        CompilerFamily::Gnu => {
            let selected = expected
                .native_sdk
                .ok_or_else(|| error("SDK generation readback requires ordinary GNU SDK links"))?;
            if expected.llvm_sdk.is_some()
                || expected.package.compiler.family() != "gnu"
                || expected.gnu_source_preset != Some(sdk.source_preset())
                || selected.source_profile() != sdk.source_preset()
                || selected.source_tree_sha256() != &sdk.identity().source.content_sha256
                || selected.contract_sha256() != &sdk.identity().source.contract_sha256
            {
                return Err(error(
                    "SDK generation GNU source/compiler selection differs from release inputs",
                ));
            }
        }
        CompilerFamily::Llvm => {
            let selected = expected
                .llvm_sdk
                .ok_or_else(|| error("SDK generation readback requires ordinary LLVM SDK links"))?;
            if expected.native_sdk.is_some()
                || expected.gnu_source_preset.is_some()
                || expected.package.compiler.family() != "llvm"
                || selected.identity() != sdk.identity()
                || selected.source_profile() != sdk.source_preset()
                || selected.toolchain_profile() != profile.name()
            {
                return Err(error(
                    "SDK generation LLVM source/compiler selection differs from release inputs",
                ));
            }
        }
    }
    let record = parse_record(request.record_bytes, request.record_sha256, family)?;
    let (source_profile, environment, compatibility_receipt, link_receipt, entries) = match &record
    {
        GenerationRecord::GnuV1(record) => (
            &record.source_profile,
            &record.environment_sha256,
            &record.compatibility_receipt_sha256,
            &record.sdk_link_receipt_sha256,
            record.sdk_entries,
        ),
        GenerationRecord::LlvmV1(record) => (
            &record.source_profile,
            &record.environment_sha256,
            &record.compatibility_receipt_sha256,
            &record.sdk_link_receipt_sha256,
            record.sdk_entries,
        ),
    };
    if record.identity() != sdk.identity()
        || source_profile != sdk.source_preset()
        || environment != expected.sdk_environment_sha256
    {
        return Err(error(
            "SDK generation record differs from its independent source/environment selection",
        ));
    }
    let package = sdk_package::verify(request.package)?;
    if record.sdk_inventory_sha256() != &package.manifest.tree_sha256
        || entries != package.manifest.files.len()
    {
        return Err(error(
            "SDK generation record differs from the actual measured SDK package inventory",
        ));
    }
    let portable = readback_portable_native_compatibility(request.compatibility)?;
    let repeated_portable = readback_portable_native_compatibility(request.compatibility)?;
    for proof in [&portable, &repeated_portable] {
        if proof.receipt().receipt_sha256 != *compatibility_receipt {
            return Err(error(
                "SDK generation record differs from its complete compatibility receipt",
            ));
        }
        let (receipt, inventory, count) = match &record {
            GenerationRecord::GnuV1(_) => {
                let sdk = proof.native_sdk().ok_or_else(|| {
                    error("SDK generation readback lacks complete GNU SDK evidence")
                })?;
                (sdk.receipt_sha256(), sdk.inventory_sha256(), sdk.entries())
            }
            GenerationRecord::LlvmV1(record) => {
                let sdk = proof.llvm_sdk().ok_or_else(|| {
                    error("SDK generation readback lacks complete LLVM SDK evidence")
                })?;
                if sdk.identity() != &record.identity
                    || sdk.binding_sha256() != &record.binding_sha256
                {
                    return Err(error(
                        "SDK generation record differs from its LLVM SDK binding",
                    ));
                }
                (sdk.receipt_sha256(), sdk.inventory_sha256(), sdk.entries())
            }
        };
        if receipt != link_receipt || inventory != record.sdk_inventory_sha256() || count != entries
        {
            return Err(error(
                "SDK generation record differs from its complete SDK link inventory",
            ));
        }
    }
    if portable.manifest_sha256() != repeated_portable.manifest_sha256()
        || portable.receipt() != repeated_portable.receipt()
        || sdk_package::verify(request.package)? != package
    {
        return Err(error(
            "SDK generation package or compatibility bytes changed during readback",
        ));
    }
    Ok(SdkGenerationReadback {
        record_sha256: request.record_sha256.clone(),
        package,
        compatibility_manifest_sha256: portable.manifest_sha256().clone(),
        compatibility_receipt_sha256: compatibility_receipt.clone(),
        sdk_link_receipt_sha256: link_receipt.clone(),
    })
}

fn parse_record(
    bytes: &[u8],
    digest: &Sha256Digest,
    family: CompilerFamily,
) -> Result<GenerationRecord, ContractError> {
    if bytes.is_empty() || bytes.len() > canonical::MAX_DOCUMENT_BYTES {
        return Err(error("SDK generation record exceeds its metadata limit"));
    }
    if sha256_bytes(bytes) != *digest {
        return Err(error(
            "SDK generation record differs from its selected raw digest",
        ));
    }
    let record = match family {
        CompilerFamily::Gnu => {
            let record: GnuGenerationRecordV1 = serde_json::from_slice(bytes)
                .map_err(|_| error("SDK generation record is not closed GNU JSON"))?;
            if record.schema != "aros-sdk-generation-record-v1"
                || record.assurance != "local-generation-binding-not-release-admission"
            {
                return Err(error(
                    "SDK generation record schema or assurance differs from selected GNU policy",
                ));
            }
            GenerationRecord::GnuV1(record)
        }
        CompilerFamily::Llvm => {
            let record: LlvmGenerationRecordV1 = serde_json::from_slice(bytes)
                .map_err(|_| error("SDK generation record is not closed LLVM JSON"))?;
            if record.schema != "aros-llvm-sdk-generation-record-v1"
                || record.assurance != "local-generation-binding-not-release-admission"
            {
                return Err(error(
                    "SDK generation record schema or assurance differs from selected LLVM policy",
                ));
            }
            GenerationRecord::LlvmV1(record)
        }
    };
    if record_bytes(&record)? != bytes {
        return Err(error("SDK generation record is not canonical JSON"));
    }
    Ok(record)
}
