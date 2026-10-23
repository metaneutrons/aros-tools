//! Explicit source-owned selection for the ordinary CLI Mesa preflight.
//!
//! This does not manufacture a producer-owned Profile or authenticate execution.

use aros_common::mesa_genmodule_inventory::{
    MesaGenmoduleInventoryScope, MesaGenmoduleSelectionBinding,
};
use aros_common::sdk_consumer_contract::{
    load_bound_sdk_consumer_contract, LoadedSdkConsumerContract,
};
use aros_common::sdk_manifest::{SdkAbiValue, SdkTarget};
use aros_common::{measure_regular_file_bounded, sha256_bytes, Sha256Digest, TargetProfile};
use std::path::{Path, PathBuf};

use crate::source_lock::HostPythonLock;
use crate::{profiles::Profile, ContractError};

/// Source profile and its sealed SDK ABI/configure declaration.
#[derive(Debug, Clone)]
pub struct SourceMesaPreflightProfile {
    source_root: PathBuf,
    profile: TargetProfile,
    contract: LoadedSdkConsumerContract,
    document_sha256: Sha256Digest,
    cpu: String,
    configure_target: String,
    python_lock: Option<HostPythonLock>,
}

impl SourceMesaPreflightProfile {
    pub(super) fn native_owner_projection(
        &self,
        files: &[PathBuf],
        host: &str,
        excluded: &std::collections::BTreeSet<String>,
        local_source: Option<&aros_common::local_source::LocalSourceIdentity>,
    ) -> Result<aros_transpiler::native_owner_projection::NativeOwnerProjection, ContractError>
    {
        self.revalidate()?;
        let context = self.native_target_context(host)?;
        let contract = &self.contract.contract;
        let projection =
            aros_transpiler::native_owner_projection::NativeOwnerProjection::load_invocation(
                &self.source_root,
                aros_transpiler::native_owner_projection::NativeInvocationPolicy {
                    profile: &self.profile.name,
                    projection: Some(&contract.metamake_projection),
                    inputs: &contract.inputs,
                    optional_meta_dependencies: &contract.optional_meta_dependencies,
                },
                files,
                host,
                &context,
                excluded,
            )
            .map_err(|_| invalid("cannot bind original Source owner metadata"))?
            .ok_or_else(|| invalid("Source has no owner metadata policy"))?;
        match local_source {
            None => Ok(projection),
            Some(local) => projection
                .with_configured_excluded_directory(&local.generated_subtree)
                .map_err(|_| invalid("cannot bind original local Source scan exclusion")),
        }
    }

    fn native_target_context(
        &self,
        host: &str,
    ) -> Result<aros_transpiler::TargetContext, ContractError> {
        let selected = self
            .profile
            .transpiler
            .as_ref()
            .ok_or_else(|| invalid("source profile lacks its MetaMake selectors"))?;
        let contract = &self.contract.contract;
        let generated_make_templates =
            aros_common::native_make_template::resolve_generated_make_templates(
                &self.source_root,
                &contract.generated_make_templates,
                &contract
                    .inputs
                    .iter()
                    .map(|input| (input.path.clone(), input.sha256.clone()))
                    .collect(),
            )
            .map_err(|_| invalid("cannot bind sealed generated Make templates"))?;
        Ok(aros_transpiler::TargetContext {
            cpu: Some(self.cpu.clone()),
            platform: Some(self.profile.platform.clone()),
            family: Some(selected.family.clone()),
            variant: Some(selected.variant.clone()),
            toolchain: Some(selected.toolchain.clone()),
            cpu32: Some(selected.cpu32.clone()),
            use_mmu: Some(if selected.use_mmu { "1" } else { "0" }.into()),
            float_abi: self.profile.float_abi.clone(),
            mesa_version: selected.mesa_version.clone(),
            // Preserve the same closed Source configuration used by the
            // ordinary transpiler CLI. Selectors alone cannot replay effects
            // whose source includes a contract-bound configuration fragment.
            make_include_bindings: contract.make_include_bindings.clone(),
            generated_make_templates,
            make_variables: contract
                .make_variables_for_host(host)
                .map_err(|_| invalid("cannot bind selected host Make variables"))?,
            host_file_generators: contract.host_file_generators.clone(),
            native_kernel_sources_in_target_role: contract.kernel_compiler_role.as_deref()
                == Some("target"),
            native_configuration_inputs: contract
                .inputs
                .iter()
                .map(|input| (input.path.clone(), input.sha256.to_string()))
                .collect(),
            ..Default::default()
        })
    }

    /// Load the exact checkout declaration; no built-in fallback or board alias.
    ///
    /// # Errors
    /// Rejects absent/changed profiles, non-Mesa selection, missing configure
    /// target, invalid sealed SDK inputs or mismatched target ABI.
    pub fn load(root: &Path, selected: &TargetProfile) -> Result<Self, ContractError> {
        let source_root = root
            .canonicalize()
            .map_err(|_| invalid("cannot resolve selected source root"))?;
        let root = source_root.as_path();
        let path = root.join("aros-targets.toml");
        let read = || {
            measure_regular_file_bounded(&path, 1024 * 1024)
                .map_err(|_| invalid("cannot measure source profile document"))?
                .map(|(_, bytes)| bytes)
                .ok_or_else(|| invalid("source-owned profile document is absent"))
        };
        let bytes = read()?;
        let text =
            std::str::from_utf8(&bytes).map_err(|_| invalid("source profiles are not UTF-8"))?;
        let profiles = TargetProfile::parse_config(text, "Mesa source profile")
            .map_err(|_| invalid("invalid source-owned target profiles"))?;
        let profile = profiles
            .targets
            .into_iter()
            .find(|profile| profile.name == selected.name)
            .ok_or_else(|| invalid("selected source target is absent"))?;
        if &profile != selected
            || !profile.transpiler.as_ref().is_some_and(|selection| {
                selection.toolchain == "llvm" && selection.mesa_version.as_deref() == Some("26.0.0")
            })
        {
            return Err(invalid(
                "source target differs or does not select LLVM Mesa 26",
            ));
        }
        let relative = profile
            .sdk_consumer_contract
            .as_deref()
            .ok_or_else(|| invalid("Mesa preflight requires a source-owned SDK ABI contract"))?;
        let contract = load_bound_sdk_consumer_contract(root, Path::new(relative), &profile)
            .map_err(|_| invalid("Mesa source SDK contract or sealed inputs are invalid"))?;
        let configure_target = contract
            .contract
            .mesa_configure_target
            .clone()
            .ok_or_else(|| invalid("Mesa source contract has no explicit configure target"))?;
        let Some(SdkAbiValue::Text(cpu)) = contract.contract.target.abi.get("source_cpu") else {
            return Err(invalid("Mesa SDK target lacks its source CPU"));
        };
        let cpu = cpu.clone();
        let python_lock = contract
            .contract
            .mesa_host_python_lock
            .as_ref()
            .map(|relative| {
                let (_, bytes) = measure_regular_file_bounded(&root.join(relative), 1024 * 1024)
                    .map_err(|_| invalid("cannot measure selected host Python lock"))?
                    .ok_or_else(|| invalid("selected host Python lock is absent"))?;
                let expected = contract
                    .contract
                    .inputs
                    .iter()
                    .find(|input| &input.path == relative)
                    .ok_or_else(|| invalid("host Python lock is not sealed"))?;
                if sha256_bytes(&bytes) != expected.sha256 {
                    return Err(invalid("host Python lock differs from its sealed bytes"));
                }
                HostPythonLock::parse(&bytes)
            })
            .transpose()?;
        if read()? != bytes {
            return Err(invalid("source profile document changed during selection"));
        }
        let projection = crate::canonical::bytes(&serde_json::json!({
            "schema":"aros-mesa-source-selection-v1",
            "profiles_sha256":sha256_bytes(&bytes),
            "source_preset":profile.name,
            "sdk_contract_sha256":contract.sha256,
        }))?;
        Ok(Self {
            source_root,
            profile,
            contract,
            document_sha256: sha256_bytes(&projection),
            cpu,
            configure_target,
            python_lock,
        })
    }

    /// Complete SDK target selected by the sealed source contract.
    #[must_use]
    pub const fn target(&self) -> &SdkTarget {
        &self.contract.contract.target
    }

    /// Canonical source root whose profile and sealed contract were loaded.
    #[must_use]
    pub fn source_root(&self) -> &Path {
        &self.source_root
    }

    /// Recheck the exact source preset, profile bytes and sealed SDK inputs.
    ///
    /// # Errors
    /// Rejects source-profile or contract drift after selection.
    pub fn revalidate(&self) -> Result<(), ContractError> {
        let current = Self::load(&self.source_root, &self.profile)?;
        if current.document_sha256 != self.document_sha256 {
            return Err(invalid(
                "source profile or configuration contract changed after selection",
            ));
        }
        Ok(())
    }

    /// Source-declared configure argument, independent of board naming.
    #[must_use]
    pub fn configure_target(&self) -> &str {
        &self.configure_target
    }

    /// Build the exact data binding for a selected genmodule inventory scope.
    ///
    /// The profile name is the retained source preset, not its compiler-profile
    /// alias. The SDK contract digest is the one loaded and validated for that
    /// source preset.
    #[must_use]
    pub fn genmodule_inventory_binding(
        &self,
        scope: MesaGenmoduleInventoryScope,
    ) -> MesaGenmoduleSelectionBinding {
        MesaGenmoduleSelectionBinding {
            profile: self.profile.name.clone(),
            kind: "sdk-consumer".into(),
            scope,
            contract_sha256: self.contract.sha256.clone(),
        }
    }

    /// Return explicit transpiler selectors for the requested inventory scope.
    ///
    /// No source, output, ports, or build paths are included. This describes a
    /// bounded inventory request; it does not authenticate execution.
    ///
    /// # Errors
    /// Rejects an internally incomplete source profile rather than omitting a
    /// selector or substituting an ambient/default value.
    pub fn genmodule_inventory_arguments(
        &self,
        scope: MesaGenmoduleInventoryScope,
    ) -> Result<Vec<String>, ContractError> {
        let context = self
            .profile
            .transpiler
            .as_ref()
            .ok_or_else(|| invalid("selected source profile lost its MetaMake selectors"))?;
        let mesa_version = context
            .mesa_version
            .as_deref()
            .ok_or_else(|| invalid("selected source profile lost its Mesa version selector"))?;
        let (profile_flag, digest_flag) = match scope {
            MesaGenmoduleInventoryScope::ContractRoots => (
                "--native-consumer-profile",
                "--native-consumer-contract-sha256",
            ),
            MesaGenmoduleInventoryScope::FullTarget => (
                "--mesa-genmodule-full-inventory-profile",
                "--mesa-genmodule-full-contract-sha256",
            ),
        };

        Ok(vec![
            "--mesa-genmodule-inventory-only".into(),
            profile_flag.into(),
            self.profile.name.clone(),
            digest_flag.into(),
            self.contract.sha256.to_string(),
            "--cpu".into(),
            self.cpu.clone(),
            "--platform".into(),
            self.profile.platform.clone(),
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
            self.profile.float_abi.clone().unwrap_or_default(),
            "--mesa-version".into(),
            mesa_version.into(),
        ])
    }

    /// Explicit host-module inputs, when declared. Runtime execution must
    /// reject absence instead of accepting ambient packages.
    #[must_use]
    pub const fn python_lock(&self) -> Option<&HostPythonLock> {
        self.python_lock.as_ref()
    }
}

/// Two validated selection domains; source data never becomes a producer Profile.
#[derive(Debug, Clone, Copy)]
pub enum MesaPreflightProfile<'a> {
    Producer(&'a Profile),
    Source(&'a SourceMesaPreflightProfile),
}

impl<'a> From<&'a Profile> for MesaPreflightProfile<'a> {
    fn from(profile: &'a Profile) -> Self {
        Self::Producer(profile)
    }
}

impl<'a> From<&'a SourceMesaPreflightProfile> for MesaPreflightProfile<'a> {
    fn from(profile: &'a SourceMesaPreflightProfile) -> Self {
        Self::Source(profile)
    }
}

impl MesaPreflightProfile<'_> {
    pub(super) fn name(&self) -> &str {
        match self {
            Self::Producer(p) => p.name(),
            Self::Source(p) => p.profile.toolchain_profile(),
        }
    }
    pub(super) fn target_triple(&self) -> &str {
        match self {
            Self::Producer(p) => p.target_triple(),
            Self::Source(p) => &p.target().target_triple,
        }
    }
    pub(super) fn configure_target(&self) -> &str {
        match self {
            Self::Producer(p) => p.configure_target(),
            Self::Source(p) => p.configure_target(),
        }
    }
    pub(super) const fn document_sha256(&self) -> &Sha256Digest {
        match self {
            Self::Producer(p) => p.document_sha256(),
            Self::Source(p) => &p.document_sha256,
        }
    }
    pub(super) fn cpu(&self) -> &str {
        match self {
            Self::Producer(p) => p.cpu(),
            Self::Source(p) => &p.cpu,
        }
    }
    pub(super) fn platform(&self) -> &str {
        match self {
            Self::Producer(p) => p.platform(),
            Self::Source(p) => &p.profile.platform,
        }
    }
    pub(super) fn float_abi(&self) -> &str {
        match self {
            Self::Producer(p) => p.float_abi(),
            Self::Source(p) => p.profile.float_abi.as_deref().unwrap_or(""),
        }
    }
    pub(super) const fn source_target(&self) -> Option<&SdkTarget> {
        match self {
            Self::Producer(_) => None,
            Self::Source(p) => Some(p.target()),
        }
    }
    pub(super) fn source_root(&self) -> Option<&Path> {
        match self {
            Self::Producer(_) => None,
            Self::Source(profile) => Some(&profile.source_root),
        }
    }
}

fn invalid(message: &str) -> ContractError {
    ContractError::invalid(format!("Mesa source selection: {message}"))
}

#[cfg(test)]
mod owner_policy_tests {
    use super::*;
    use std::collections::BTreeSet;

    /// Data-only diagnosis; never reconstructs a prior live collection owner.
    #[test]
    #[ignore = "requires explicit Source join case; read-only policy diagnosis"]
    fn actual_source_owner_policy_diagnostic() {
        let path = std::env::var_os("AROS_TEST_MESA_PREFLIGHT_CASE").unwrap();
        let case: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let root = Path::new(case["source_root"].as_str().unwrap());
        let selection = &case["native_collection"]["source_join"];
        let targets = TargetProfile::load_config(&root.join("aros-targets.toml")).unwrap();
        let target = targets
            .targets
            .iter()
            .find(|target| target.name == selection["source_profile"].as_str().unwrap())
            .unwrap();
        let profile = SourceMesaPreflightProfile::load(root, target).unwrap();
        let tree = aros_common::measure_tree_content_cas(root).unwrap();
        let files = tree
            .entry_kinds()
            .filter_map(|(name, _)| {
                let path = Path::new(std::str::from_utf8(name).unwrap());
                if path
                    .file_name()
                    .is_some_and(|name| name == "mmakefile" || name == "mmakefile.src")
                {
                    Some(root.join(path))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        let contract = &profile.contract.contract;
        let projection =
            aros_transpiler::native_owner_projection::NativeOwnerProjection::load_invocation(
                root,
                aros_transpiler::native_owner_projection::NativeInvocationPolicy {
                    profile: &profile.profile.name,
                    projection: Some(&contract.metamake_projection),
                    inputs: &contract.inputs,
                    optional_meta_dependencies: &contract.optional_meta_dependencies,
                },
                &files,
                selection["host"].as_str().unwrap(),
                &profile
                    .native_target_context(selection["host"].as_str().unwrap())
                    .unwrap(),
                &BTreeSet::new(),
            )
            .expect("read-only original Source owner policy load")
            .unwrap();
        let goals = case["native_launch_query"]["goals"]
            .as_array()
            .unwrap()
            .iter()
            .map(|goal| goal.as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        let bound = projection.select_source_metadata(root, &goals).unwrap();
        eprintln!(
            "current Source metadata selection: {}",
            serde_json::to_string(&bound).unwrap()
        );
        let selected = bound.selection();
        assert!(
            selected.missing_endpoints.is_empty(),
            "unresolved metadata endpoints: {:?}",
            selected.missing_endpoints
        );
        assert!(
            !selected.selected_owner_targets.is_empty(),
            "no selected Source owners"
        );
    }
}
