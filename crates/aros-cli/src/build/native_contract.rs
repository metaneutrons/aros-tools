//! Validated source contract selection shared by native builders and consumers.

use aros_common::{
    native_build_contract::{
        load_bound_native_build_contract, LoadedNativeBuildContract, NativeBuildInput,
    },
    native_consumer_contract::{load_bound_native_consumer_contract, LoadedNativeConsumerContract},
    native_host_generator::NativeHostFileGenerator,
    Sha256Digest, TargetProfile,
};
use miette::{IntoDiagnostic, Result, WrapErr};
use std::path::Path;

#[derive(Debug, Clone)]
pub(super) enum NativeContractSelection {
    Build(Box<LoadedNativeBuildContract>),
    Consumer(Box<LoadedNativeConsumerContract>),
}

impl NativeContractSelection {
    /// Load exactly one contract named by the measured source profile.
    ///
    /// Both profile bindings are rejected before either file is read so an
    /// ambiguous source declaration cannot reach toolchain resolution.
    pub(super) fn load(root: &Path, profile: &TargetProfile) -> Result<Option<Self>> {
        match (
            profile.native_build_contract.as_deref(),
            profile.native_consumer_contract.as_deref(),
        ) {
            (Some(_), Some(_)) => {
                miette::bail!("source profile selects both a native build and consumer contract");
            }
            (Some(relative), None) => Ok(Some(Self::Build(Box::new(
                load_bound_native_build_contract(root, Path::new(relative), profile)
                    .into_diagnostic()
                    .wrap_err("native source build contract is invalid")?,
            )))),
            (None, Some(relative)) => Ok(Some(Self::Consumer(Box::new(
                load_bound_native_consumer_contract(root, Path::new(relative), profile)
                    .into_diagnostic()
                    .wrap_err("native source consumer contract is invalid")?,
            )))),
            (None, None) => Ok(None),
        }
    }

    pub(super) fn path(&self) -> &Path {
        match self {
            Self::Build(binding) => &binding.path,
            Self::Consumer(binding) => &binding.path,
        }
    }

    pub(super) const fn sha256(&self) -> &Sha256Digest {
        match self {
            Self::Build(binding) => &binding.sha256,
            Self::Consumer(binding) => &binding.sha256,
        }
    }

    pub(super) fn inputs(&self) -> &[NativeBuildInput] {
        match self {
            Self::Build(binding) => &binding.contract.inputs,
            Self::Consumer(binding) => &binding.contract.inputs,
        }
    }

    pub(super) fn host_file_generators(&self) -> &[NativeHostFileGenerator] {
        match self {
            Self::Build(binding) => &binding.contract.host_file_generators,
            Self::Consumer(binding) => &binding.contract.host_file_generators,
        }
    }

    pub(super) fn relative_path<'a>(&self, profile: &'a TargetProfile) -> Option<&'a str> {
        match self {
            Self::Build(_) => profile.native_build_contract.as_deref(),
            Self::Consumer(_) => profile.native_consumer_contract.as_deref(),
        }
    }

    pub(super) const fn is_build(&self) -> bool {
        matches!(self, Self::Build(_))
    }

    pub(super) fn cmake_variables(&self) -> Result<Vec<(String, String)>> {
        let path = self
            .path()
            .to_str()
            .filter(|path| !path.contains([';', '\n', '\r']))
            .ok_or_else(|| miette::miette!("native contract path is unsafe for CMake"))?;
        let digest = self.sha256().to_string();
        let (selected_path, selected_digest, cleared_path, cleared_digest) = if self.is_build() {
            (
                "AROS_NATIVE_BUILD_CONTRACT",
                "AROS_NATIVE_BUILD_CONTRACT_SHA256",
                "AROS_NATIVE_CONSUMER_CONTRACT",
                "AROS_NATIVE_CONSUMER_CONTRACT_SHA256",
            )
        } else {
            (
                "AROS_NATIVE_CONSUMER_CONTRACT",
                "AROS_NATIVE_CONSUMER_CONTRACT_SHA256",
                "AROS_NATIVE_BUILD_CONTRACT",
                "AROS_NATIVE_BUILD_CONTRACT_SHA256",
            )
        };
        Ok(vec![
            (selected_path.into(), path.into()),
            (selected_digest.into(), digest),
            (cleared_path.into(), String::new()),
            (cleared_digest.into(), String::new()),
        ])
    }

    pub(super) fn empty_cmake_variables() -> Vec<(String, String)> {
        vec![
            ("AROS_NATIVE_BUILD_CONTRACT".into(), String::new()),
            ("AROS_NATIVE_BUILD_CONTRACT_SHA256".into(), String::new()),
            ("AROS_NATIVE_CONSUMER_CONTRACT".into(), String::new()),
            ("AROS_NATIVE_CONSUMER_CONTRACT_SHA256".into(), String::new()),
        ]
    }
}
