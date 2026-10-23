//! Portable, bounded selection of inputs for a Mesa preflight.
//!
//! This record binds independently selected source, profile and compiler
//! values. It is data for consistency read-back, not producer authority.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use aros_common::mesa_genmodule_inventory::{
    MesaGenmoduleInventoryScope, MesaGenmoduleSelectionBinding,
};
use aros_common::sdk_manifest::{SdkAbiValue, SdkIdentity};
use aros_common::{ArosCompilerIdentity, Sha256Digest};
use serde::{Deserialize, Serialize};

use super::super::mesa_preflight_evidence::MesaPreflightDataProof;
use super::NativeCompatibilityRequest;
use crate::compatibility::LlvmSdkDriverBinding;
use crate::profiles::Profile;
use crate::ContractError;

const SELECTION_SCHEMA: &str = "aros-mesa-preflight-selection-v1";
const COLLECTION_SELECTION_SCHEMA: &str = "aros-mesa-preflight-selection-v2";

/// Independently selected Mesa preflight inputs for bounded read-back.
///
/// The private fields prevent ordinary callers from assembling a selection;
/// deserialized records must still pass [`Self::validate`] and
/// [`Self::validate_binding`]. This record does not prove that a preflight ran.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MesaPreflightSelection {
    schema: String,
    sdk_identity: SdkIdentity,
    profile_name: String,
    target_triple: String,
    profile_document_sha256: Sha256Digest,
    compiler: ArosCompilerIdentity,
    compiler_tree_sha256: Sha256Digest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    compiler_root: Option<PathBuf>,
    source_tree_sha256: Sha256Digest,
    cpu: String,
    platform: String,
    configure_target: String,
    float_abi: String,
    clang_path: PathBuf,
    clangxx_path: PathBuf,
    output_root: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    genmodule_selection: Option<MesaGenmoduleSelectionBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    genmodule_helper_sha256: Option<Sha256Digest>,
}

impl MesaPreflightSelection {
    /// Validate the self-contained, data-only selection without filesystem access.
    ///
    /// # Errors
    /// Rejects unsupported schemas, inconsistent SDK/compiler identities,
    /// malformed target selectors or unsafe lexical paths.
    pub fn validate(&self) -> Result<(), ContractError> {
        let legacy = self.schema == SELECTION_SCHEMA
            && self.genmodule_selection.is_none()
            && self.genmodule_helper_sha256.is_none();
        let collection = self.schema == COLLECTION_SELECTION_SCHEMA
            && self.genmodule_selection.is_some()
            && self.genmodule_helper_sha256.is_some();
        if !legacy && !collection {
            return Err(error("Mesa preflight selection schema is invalid"));
        }
        if let Some(binding) = &self.genmodule_selection {
            validate_identifier(&binding.profile, "Mesa Source preset")?;
            if binding.kind != "sdk-consumer"
                || binding.scope != MesaGenmoduleInventoryScope::ContractRoots
                || is_zero_digest(&binding.contract_sha256)
                || binding.contract_sha256 != self.sdk_identity.source.contract_sha256
                || self
                    .genmodule_helper_sha256
                    .as_ref()
                    .is_none_or(is_zero_digest)
            {
                return Err(error("Mesa native collection selection is invalid"));
            }
        }
        self.sdk_identity
            .validate()
            .map_err(|_| error("Mesa preflight selection SDK identity is invalid"))?;
        if is_zero_digest(&self.profile_document_sha256)
            || is_zero_digest(&self.compiler_tree_sha256)
        {
            return Err(error("Mesa selection contains an empty content identity"));
        }
        validate_identifier(&self.profile_name, "Mesa profile name")?;
        validate_identifier(&self.target_triple, "Mesa target triple")?;
        validate_identifier(&self.cpu, "Mesa target CPU")?;
        validate_identifier(&self.platform, "Mesa target platform")?;
        validate_identifier(&self.configure_target, "Mesa configure target")?;
        if !self.float_abi.is_empty() {
            validate_identifier(&self.float_abi, "Mesa floating-point ABI")?;
        }
        self.compiler
            .validate_for_target(&self.target_triple)
            .map_err(|_| error("Mesa selection compiler identity is invalid"))?;
        if !matches!(self.compiler, ArosCompilerIdentity::Llvm { .. }) {
            return Err(error("Mesa selection requires an LLVM compiler"));
        }
        if self.sdk_identity.target.target_triple != self.target_triple
            || self.sdk_identity.source.content_sha256 != self.source_tree_sha256
            || self.sdk_identity.target.abi.get("source_cpu")
                != Some(&SdkAbiValue::Text(self.cpu.clone()))
        {
            return Err(error(
                "Mesa selection differs from its independently selected SDK target or source",
            ));
        }
        match (
            self.float_abi.as_str(),
            self.sdk_identity.target.abi.get("float_abi"),
        ) {
            ("", None) => {}
            (expected, Some(SdkAbiValue::Text(actual))) if expected == actual => {}
            _ => {
                return Err(error(
                    "Mesa selection floating-point ABI differs from its SDK identity",
                ));
            }
        }

        validate_absolute_lexical_path(&self.clang_path, "selected clang path")?;
        validate_absolute_lexical_path(&self.clangxx_path, "selected clang++ path")?;
        validate_absolute_lexical_path(&self.output_root, "Mesa preflight output root")?;
        if let Some(root) = &self.compiler_root {
            validate_absolute_lexical_path(root, "selected compiler root")?;
            if !self.clang_path.starts_with(root)
                || !self.clangxx_path.starts_with(root)
                || &self.clang_path == root
                || &self.clangxx_path == root
            {
                return Err(error("Mesa drivers are outside the selected compiler root"));
            }
        }
        if self.clang_path.file_name().and_then(|name| name.to_str()) != Some("clang")
            || self.clangxx_path.file_name().and_then(|name| name.to_str()) != Some("clang++")
            || self.clang_path.parent() != self.clangxx_path.parent()
            || self
                .clang_path
                .parent()
                .and_then(Path::file_name)
                .and_then(|name| name.to_str())
                != Some("bin")
            || self.output_root.file_name().and_then(|name| name.to_str()) != Some("mesa-preflight")
        {
            return Err(error("Mesa preflight selection paths are inconsistent"));
        }
        Ok(())
    }

    /// SDK product, target, source and epoch identity selected by the source.
    #[must_use]
    pub const fn identity(&self) -> &SdkIdentity {
        &self.sdk_identity
    }

    /// LLVM compiler identity selected for the preflight.
    #[must_use]
    pub const fn compiler(&self) -> &ArosCompilerIdentity {
        &self.compiler
    }

    /// Complete measured LLVM compiler-tree digest.
    #[must_use]
    pub const fn compiler_tree_sha256(&self) -> &Sha256Digest {
        &self.compiler_tree_sha256
    }

    /// Independently selected engine-free source-tree digest.
    #[must_use]
    pub const fn source_tree_sha256(&self) -> &Sha256Digest {
        &self.source_tree_sha256
    }

    /// Compiler profile name selected from the profiles document.
    #[must_use]
    pub fn profile_name(&self) -> &str {
        &self.profile_name
    }

    /// Exact AROS target triple selected from the compiler profile.
    #[must_use]
    pub fn target_triple(&self) -> &str {
        &self.target_triple
    }

    /// Exact enclosing profiles-document digest.
    #[must_use]
    pub const fn profile_document_sha256(&self) -> &Sha256Digest {
        &self.profile_document_sha256
    }

    /// Canonical output leaf reserved for this preflight.
    #[must_use]
    pub fn output_root(&self) -> &Path {
        &self.output_root
    }

    pub(super) fn validate_genmodule_binding(
        &self,
        expected: Option<(&MesaGenmoduleSelectionBinding, &Sha256Digest)>,
    ) -> Result<(), ContractError> {
        match (
            &self.genmodule_selection,
            &self.genmodule_helper_sha256,
            expected,
        ) {
            (None, None, None) => Ok(()),
            (Some(binding), Some(helper), Some((expected_binding, expected_helper)))
                if binding == expected_binding && helper == expected_helper =>
            {
                Ok(())
            }
            _ => Err(error(
                "Mesa collection differs from independently selected Source/helper inputs",
            )),
        }
    }

    pub(super) fn validate_helper(&self, helper: &Sha256Digest) -> Result<(), ContractError> {
        if self
            .genmodule_helper_sha256
            .as_ref()
            .is_some_and(|selected| selected != helper)
        {
            return Err(error(
                "Mesa collection differs from the measured transpiler helper",
            ));
        }
        Ok(())
    }

    /// Join this record to independently selected profile/compiler/source data.
    ///
    /// This confirms selection consistency only. It is not authority that a
    /// producer ran or that the source or compiler package was authenticated.
    ///
    /// # Errors
    /// Rejects any mismatch with the supplied compiler profile, measured
    /// compiler identity/tree, source-tree digest or SDK identity.
    pub fn validate_binding(
        &self,
        profile: &Profile,
        compiler: &ArosCompilerIdentity,
        compiler_tree: &Sha256Digest,
        compiler_root: &Path,
        source_tree: &Sha256Digest,
        identity: &SdkIdentity,
    ) -> Result<(), ContractError> {
        self.validate()?;
        identity
            .validate()
            .map_err(|_| error("selected Mesa binding SDK identity is invalid"))?;
        compiler
            .validate_for_target(profile.target_triple())
            .map_err(|_| error("selected Mesa binding compiler identity is invalid"))?;
        if profile.family() != crate::source_lock::CompilerFamily::Llvm
            || !matches!(compiler, ArosCompilerIdentity::Llvm { .. })
            || profile.name() != self.profile_name
            || profile.target_triple() != self.target_triple
            || profile.document_sha256() != &self.profile_document_sha256
            || profile.cpu() != self.cpu
            || profile.platform() != self.platform
            || profile.configure_target() != self.configure_target
            || profile.float_abi() != self.float_abi
            || compiler != &self.compiler
            || compiler_tree != &self.compiler_tree_sha256
            || self
                .compiler_root
                .as_deref()
                .is_some_and(|root| root != compiler_root)
            || source_tree != &self.source_tree_sha256
            || identity != &self.sdk_identity
        {
            return Err(error(
                "Mesa preflight selection differs from independently selected bindings",
            ));
        }
        Ok(())
    }

    pub(super) fn expectations(
        &self,
    ) -> super::super::mesa_preflight_evidence::MesaPreflightEvidenceExpectations<'_> {
        super::super::mesa_preflight_evidence::MesaPreflightEvidenceExpectations {
            profile_name: &self.profile_name,
            target_triple: &self.target_triple,
            profile_document_sha256: &self.profile_document_sha256,
            compiler: &self.compiler,
            compiler_tree_sha256: &self.compiler_tree_sha256,
            compiler_root: self.compiler_root.as_deref(),
            source_tree_sha256: &self.source_tree_sha256,
            cpu: &self.cpu,
            platform: &self.platform,
            clang_path: &self.clang_path,
            clangxx_path: &self.clangxx_path,
            output_root: &self.output_root,
            genmodule_selection: self.genmodule_selection.clone(),
            genmodule_helper_sha256: self.genmodule_helper_sha256.clone(),
        }
    }

    /// Read only the fixed bounded evidence members against this selection.
    ///
    /// # Errors
    /// Rejects unsafe selection data, incomplete/extra members or inconsistent
    /// Mesa evidence. It performs no filesystem access and proves no execution.
    pub(super) fn readback(
        &self,
        files: &BTreeMap<String, Vec<u8>>,
    ) -> Result<MesaPreflightDataProof, ContractError> {
        self.validate()?;
        super::super::mesa_preflight_evidence::readback(files, &self.expectations())
    }
}

/// Select the source-owned preflight inputs before any Mesa execution begins.
pub(super) fn select(
    request: &NativeCompatibilityRequest,
    drivers: &LlvmSdkDriverBinding,
) -> Result<Option<MesaPreflightSelection>, ContractError> {
    if !super::mesa::required(request) {
        return Ok(None);
    }
    let source = request
        .llvm_sdk_source
        .as_ref()
        .ok_or_else(|| error("configured Mesa selection lost its verified LLVM SDK source"))?;
    super::llvm_sdk::select(request, drivers.compiler())?
        .ok_or_else(|| error("configured Mesa selection lost its verified source binding"))?;

    let source_profile = source.source_profile();
    let context = source_profile
        .transpiler
        .as_ref()
        .ok_or_else(|| error("configured Mesa source lost its transpiler context"))?;
    if context.toolchain != "llvm"
        || context.mesa_version.as_deref() != Some("26.0.0")
        || source_profile.arch.source_cpu() != request.profile.cpu()
        || source_profile.platform != request.profile.platform()
        || source_profile.float_abi.as_deref().unwrap_or_default() != request.profile.float_abi()
        || drivers.target() != &source.identity().target
        || request.preparation.source_tree_sha256 != source.identity().source.content_sha256
    {
        return Err(error(
            "configured Mesa source context differs from selected LLVM inputs",
        ));
    }

    let report_root = if request
        .reports_root
        .try_exists()
        .map_err(|_| error("cannot inspect compatibility report root"))?
    {
        super::checked_directory(&request.reports_root, "compatibility report root")?
    } else {
        super::checked_absent_root(&request.reports_root, "compatibility report root")?
    };
    let selection = MesaPreflightSelection {
        schema: COLLECTION_SELECTION_SCHEMA.to_owned(),
        sdk_identity: source.identity().clone(),
        profile_name: request.profile.name().to_owned(),
        target_triple: request.profile.target_triple().to_owned(),
        profile_document_sha256: request.profile.document_sha256().clone(),
        compiler: drivers.compiler().clone(),
        compiler_tree_sha256: drivers.compiler_tree_sha256().clone(),
        compiler_root: Some(drivers.compiler_root().to_path_buf()),
        source_tree_sha256: request.preparation.source_tree_sha256.clone(),
        cpu: request.profile.cpu().to_owned(),
        platform: request.profile.platform().to_owned(),
        configure_target: request.profile.configure_target().to_owned(),
        float_abi: request.profile.float_abi().to_owned(),
        clang_path: drivers.clang_path().to_path_buf(),
        clangxx_path: drivers.clangxx_path().to_path_buf(),
        output_root: report_root.join("mesa-preflight"),
        genmodule_selection: Some(
            crate::compatibility::SourceMesaPreflightProfile::load(
                &request.preparation.source_root,
                source_profile,
            )?
            .genmodule_inventory_binding(MesaGenmoduleInventoryScope::ContractRoots),
        ),
        genmodule_helper_sha256: Some(
            request
                .preparation
                .helpers
                .get("aros-transpiler")
                .ok_or_else(|| error("Mesa selection lost its measured inventory helper"))?
                .sha256
                .clone(),
        ),
    };
    selection.validate_binding(
        &request.profile,
        drivers.compiler(),
        drivers.compiler_tree_sha256(),
        drivers.compiler_root(),
        &request.preparation.source_tree_sha256,
        source.identity(),
    )?;
    Ok(Some(selection))
}

fn validate_identifier(value: &str, label: &str) -> Result<(), ContractError> {
    if value.is_empty()
        || value.len() > 256
        || value.chars().any(char::is_control)
        || value.chars().any(char::is_whitespace)
        || value.bytes().any(|byte| b"$#\\*?[]".contains(&byte))
    {
        return Err(error(&format!("{label} is invalid")));
    }
    Ok(())
}

fn validate_absolute_lexical_path(path: &Path, label: &str) -> Result<(), ContractError> {
    let text = path.to_str().unwrap_or_default();
    if !path.is_absolute()
        || path.as_os_str().is_empty()
        || text.is_empty()
        || text.contains("//")
        || text.chars().any(char::is_control)
        || text.chars().any(char::is_whitespace)
        || text.bytes().any(|byte| b"$#\\*?[]".contains(&byte))
        || path.file_name().is_none()
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
        || text
            .split('/')
            .skip(1)
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
    {
        return Err(error(&format!("{label} is not an absolute lexical path")));
    }
    Ok(())
}

fn is_zero_digest(digest: &Sha256Digest) -> bool {
    digest.as_str().bytes().all(|byte| byte == b'0')
}

fn error(message: &str) -> ContractError {
    ContractError::compatibility(message)
}
