//! Strict loader for checkout-owned native build contracts.

use crate::{ArosError, Result, Sha256Digest, TargetProfile};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

const NATIVE_BUILD_SCHEMA_VERSION: u32 = 1;
const NATIVE_BUILD_QUALIFICATION: &str = "experimental-unqualified";
const NATIVE_PACKAGE_FORMAT: &str = "aros-pkg-v1";
const NATIVE_COMPILER_RUNTIME_ROLE: &str = "libgcc";
const MAX_NATIVE_BUILD_INPUTS: usize = 128;
const MAX_NATIVE_MAKE_INCLUDE_BINDINGS: usize = 16;
const MAX_NATIVE_BUILD_CONTRACT_BYTES: u64 = 1024 * 1024;

/// Source-owned native build facts, bound to a target profile and checked inputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeBuildContract {
    pub schema_version: u32,
    pub profile: String,
    pub board: String,
    /// Revision whose source rules were exported, not the current checkout
    /// identity or a clean-tree provenance claim. The input digests below
    /// bind the actual rules; build receipts must measure the selected tree.
    pub source_baseline: String,
    pub qualification: String,
    /// Explicit source configuration defaults for generic Make evaluation.
    /// Empty is a known value; an omitted name remains unknown. Local source
    /// assignments retain precedence. These are not command-line overrides.
    #[serde(default, deserialize_with = "deserialize_make_variables")]
    pub make_variables: BTreeMap<String, String>,
    /// Literal source configuration selected by the measured native host key.
    /// A declared table must contain the actual host; no default host or
    /// environment-provided Make value is inferred. Local assignments retain
    /// precedence over the selected values.
    #[serde(default, deserialize_with = "deserialize_host_make_variables")]
    pub host_make_variables: BTreeMap<String, BTreeMap<String, String>>,
    /// Explicit source-owned bindings for included Make configuration.
    /// Both endpoints remain inventoried. A self-binding reads the original
    /// .mk fragment; a distinct endpoint selects a source-native projection.
    #[serde(default, deserialize_with = "deserialize_make_include_bindings")]
    pub make_include_bindings: BTreeMap<String, String>,
    /// Generated Make includes reconstructed only from sealed configure-owned
    /// templates and explicit profile substitutions. Generated files are not read.
    #[serde(
        default,
        deserialize_with = "crate::native_make_template::deserialize_bindings"
    )]
    pub generated_make_templates:
        BTreeMap<String, crate::native_make_template::GeneratedMakeTemplateBinding>,
    /// Optional source-owned, closed native MetaMake invocation policy. This is
    /// distinct from GNU Make/ABI configuration and never invokes configure.
    #[serde(default)]
    pub metamake_projection: Option<String>,
    /// Explicit source-owned absent MetaMake edges with a closed absence proof.
    #[serde(default)]
    pub optional_meta_dependencies: Vec<NativeOptionalMetaDependency>,
    /// Explicit source exports for bounded host-C generated files. The
    /// transpiler proves their executable recipe shape before graph admission.
    #[serde(default)]
    pub host_file_generators: Vec<crate::native_host_generator::NativeHostFileGenerator>,
    /// How `%build_archspecific ... compiler=kernel` sources are compiled.
    /// The only admitted value, `target`, is the source's declaration that
    /// its kernel code builds in the target compiler role, as AROS-NX already
    /// builds pc's kernel. Absent, such sources have no native producer.
    #[serde(default)]
    pub kernel_compiler_role: Option<String>,
    pub inputs: Vec<NativeBuildInput>,
    pub abi: NativeBuildAbi,
    pub core: NativeBuildCore,
    pub package: NativeBuildPackage,
    pub media: NativeBuildMedia,
}

/// Shared source-owned invocation facts that can be validated independently
/// of package, media and core build policy.
#[derive(Clone, Copy)]
pub(crate) struct NativeInvocationConfiguration<'a> {
    pub(crate) inputs: &'a [NativeBuildInput],
    pub(crate) make_variables: &'a BTreeMap<String, String>,
    pub(crate) host_make_variables: &'a BTreeMap<String, BTreeMap<String, String>>,
    pub(crate) make_include_bindings: &'a BTreeMap<String, String>,
    pub(crate) generated_make_templates:
        &'a BTreeMap<String, crate::native_make_template::GeneratedMakeTemplateBinding>,
    pub(crate) metamake_projection: Option<&'a str>,
    pub(crate) optional_meta_dependencies: &'a [NativeOptionalMetaDependency],
    pub(crate) host_file_generators: &'a [crate::native_host_generator::NativeHostFileGenerator],
    pub(crate) kernel_compiler_role: Option<&'a str>,
}

/// One source-owned MetaMake optional dependency selected by target identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeOptionalMetaDependency {
    pub recipe: String,
    pub target: String,
    pub dependency: String,
    #[serde(default, skip_serializing_if = "NativeMetaAbsence::is_selector")]
    pub absence: NativeMetaAbsence,
}

/// Why a source-owned metadata edge may be absent. Neither case permits
/// discarding a present, unsupported or rejected producer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NativeMetaAbsence {
    #[default]
    Selector,
    DisabledOwner,
}

impl NativeMetaAbsence {
    // serde's skip_serializing_if predicate receives a reference.
    #[allow(clippy::trivially_copy_pass_by_ref)]
    fn is_selector(&self) -> bool {
        *self == Self::Selector
    }
}

/// Target identity selectors cannot be shadowed by source configuration data.
pub const NATIVE_RESERVED_MAKE_VARIABLES: &[&str] = &[
    "AROS_TARGET_CPU",
    "CPU",
    "AROS_TARGET_ARCH",
    "ARCH",
    "AROS_TARGET_PLATFORM",
    "AROS_TARGET_FAMILY",
    "FAMILY",
    "AROS_TARGET_VARIANT",
    "AROS_TOOLCHAIN",
    "AROS_TARGET_CPU32",
    "USE_MMU",
    "GCC_CONFIG_FLOAT_ABI",
    "OPT_MESAGL",
    "TARGET_LLVM_VER",
    "TARGET_LLVM_RUNTIMES_STYLE",
    "TARGET_RUST",
    "TARGET_RUST_VER",
];

pub(crate) fn deserialize_make_variables<'de, D>(
    deserializer: D,
) -> std::result::Result<BTreeMap<String, String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct Variables;
    impl<'de> serde::de::Visitor<'de> for Variables {
        type Value = BTreeMap<String, String>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("an object of unique literal Make configuration values")
        }

        fn visit_map<M>(self, mut map: M) -> std::result::Result<Self::Value, M::Error>
        where
            M: serde::de::MapAccess<'de>,
        {
            let mut variables = BTreeMap::new();
            while let Some((key, value)) = map.next_entry::<String, String>()? {
                if variables.insert(key.clone(), value).is_some() {
                    return Err(serde::de::Error::custom(format!(
                        "duplicate Make variable {key:?}"
                    )));
                }
                if variables.len() > MAX_MAKE_VARIABLES {
                    return Err(serde::de::Error::custom(format!(
                        "make_variables exceeds {MAX_MAKE_VARIABLES} entries"
                    )));
                }
            }
            Ok(variables)
        }
    }
    deserializer.deserialize_map(Variables)
}

/// A port declares every build switch its recipes test, including the
/// diagnostic ones a production build leaves empty (ESP32-P4: about 120).
const MAX_MAKE_VARIABLES: usize = 256;

pub(crate) fn deserialize_host_make_variables<'de, D>(
    deserializer: D,
) -> std::result::Result<BTreeMap<String, BTreeMap<String, String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(transparent)]
    struct Variables(
        #[serde(deserialize_with = "deserialize_make_variables")] BTreeMap<String, String>,
    );
    struct Hosts;
    impl<'de> serde::de::Visitor<'de> for Hosts {
        type Value = BTreeMap<String, BTreeMap<String, String>>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("at most sixteen unique host configuration maps")
        }

        fn visit_map<M>(self, mut map: M) -> std::result::Result<Self::Value, M::Error>
        where
            M: serde::de::MapAccess<'de>,
        {
            let mut hosts = BTreeMap::new();
            while let Some((host, variables)) = map.next_entry::<String, Variables>()? {
                if hosts.insert(host.clone(), variables.0).is_some() {
                    return Err(serde::de::Error::custom(format!(
                        "duplicate native host {host:?}"
                    )));
                }
                if hosts.len() > 16 {
                    return Err(serde::de::Error::custom(
                        "host_make_variables exceeds 16 hosts",
                    ));
                }
            }
            Ok(hosts)
        }
    }
    deserializer.deserialize_map(Hosts)
}

impl NativeBuildContract {
    /// Resolve source-owned configuration for the actual native build host.
    ///
    /// # Errors
    /// Rejects a missing declared host or an ambiguous shared/host binding.
    pub fn make_variables_for_host(&self, host: &str) -> Result<BTreeMap<String, String>> {
        if let Some(name) = self
            .make_variables
            .keys()
            .find(|name| NATIVE_HOST_MAKE_IDENTITIES.contains(&name.as_str()))
        {
            return Err(configuration_error(
                &self.profile,
                &format!("host identity {name:?} requires host_make_variables"),
            ));
        }
        let mut variables = self.make_variables.clone();
        if self.host_make_variables.is_empty() {
            return Ok(variables);
        }
        let selected = self.host_make_variables.get(host).ok_or_else(|| {
            configuration_error(
                &self.profile,
                &format!("no source Make configuration for native host {host:?}"),
            )
        })?;
        for (name, value) in selected {
            if variables.insert(name.clone(), value.clone()).is_some() {
                return Err(configuration_error(
                    &self.profile,
                    &format!("ambiguous shared and host Make variable {name:?}"),
                ));
            }
        }
        Ok(variables)
    }
}

// Unlike target selectors, these names are valid in the host-selected map.
// They may not be shared across actual hosts or resolved as target aliases.
const NATIVE_HOST_MAKE_IDENTITIES: &[&str] = &["AROS_HOST_ARCH", "AROS_HOST_CPU"];

pub(crate) fn deserialize_make_include_bindings<'de, D>(
    deserializer: D,
) -> std::result::Result<BTreeMap<String, String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct Bindings;
    impl<'de> serde::de::Visitor<'de> for Bindings {
        type Value = BTreeMap<String, String>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("an object of unique source-relative Make include bindings")
        }

        fn visit_map<M>(self, mut map: M) -> std::result::Result<Self::Value, M::Error>
        where
            M: serde::de::MapAccess<'de>,
        {
            let mut bindings = BTreeMap::new();
            while let Some((include, replacement)) = map.next_entry::<String, String>()? {
                if bindings.insert(include.clone(), replacement).is_some() {
                    return Err(serde::de::Error::custom(format!(
                        "duplicate Make include binding {include:?}"
                    )));
                }
                if bindings.len() > MAX_NATIVE_MAKE_INCLUDE_BINDINGS {
                    return Err(serde::de::Error::custom(format!(
                        "make_include_bindings exceeds {MAX_NATIVE_MAKE_INCLUDE_BINDINGS} entries"
                    )));
                }
            }
            Ok(bindings)
        }
    }
    deserializer.deserialize_map(Bindings)
}

/// One source-relative file included in the contract's measured input set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeBuildInput {
    pub path: String,
    pub sha256: Sha256Digest,
}

/// Native target ABI decisions recorded by the source checkout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeBuildAbi {
    pub source_cpu: String,
    pub target_triple: String,
    pub isa: String,
    pub abi: String,
    pub code_model: String,
    pub flavour: String,
    pub platform_smp: bool,
    pub use_mmu: bool,
}

/// Source recipes and linker inputs used to build the kernel core.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeBuildCore {
    pub recipe: String,
    pub linker_script: String,
    pub resources: Vec<String>,
    pub libraries: Vec<String>,
    pub devices: Vec<String>,
    pub link_libraries: Vec<String>,
    pub compiler_runtime_role: String,
    pub residency_check: String,
    pub residency_policy: NativeResidencyPolicy,
}

/// Source-declared geometry for an implemented, non-script residency check.
///
/// The named-reference algorithm is intentionally narrower than a CFG proof.
/// Region bounds are half-open and come from the selected source contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeResidencyPolicy {
    pub algorithm: String,
    pub section: String,
    pub flash_start: u64,
    pub flash_end: u64,
    pub sram_start: u64,
    pub sram_end: u64,
}

impl NativeResidencyPolicy {
    /// Reject unsupported algorithms, ambiguous sections or invalid geometry.
    ///
    /// # Errors
    /// Rejects empty, overlapping or non-RV32 address ranges.
    pub fn validate(&self) -> std::result::Result<(), String> {
        if self.algorithm != "riscv32-xip-v1" || self.section != ".sramtext" {
            return Err("unsupported native residency algorithm or section".into());
        }
        let maximum = 1_u64 << 32;
        if self.flash_start >= self.flash_end
            || self.sram_start >= self.sram_end
            || self.flash_end > maximum
            || self.sram_end > maximum
            || self.flash_start < self.sram_end && self.sram_start < self.flash_end
        {
            return Err(
                "native residency ranges must be nonempty, disjoint RV32 half-open ranges".into(),
            );
        }
        Ok(())
    }
}

/// Source recipe and source-defined package construction facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeBuildPackage {
    pub recipe: String,
    pub format: String,
    pub target: String,
    pub limit_from_board: String,
}

/// Source-defined media layout references and bootloader inputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeBuildMedia {
    pub chip: String,
    pub board_rules: String,
    pub partition_table: String,
    pub core_partition: String,
    pub package_partition: String,
    pub development_volume_offset_from_board: String,
    pub bootloader_configuration: String,
    pub bootloader_patch: String,
    pub idf_version: String,
    /// Optional source-owned numeric media export. It must be inventoried;
    /// consumers without a media adapter do not infer a default geometry.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_geometry_contract"
    )]
    pub geometry_contract: Option<String>,
}

fn deserialize_geometry_contract<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // Omission is the backwards-compatible core-only state. An explicit null
    // is not a path and must agree with the strict CMake boundary.
    String::deserialize(deserializer).map(Some)
}

/// Exact contract bytes validated together with their parsed source facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedNativeBuildContract {
    pub path: PathBuf,
    pub sha256: Sha256Digest,
    pub contract: NativeBuildContract,
}

/// Load and validate a native build contract from a source checkout.
///
/// The contract is source-owned. Its source paths are constrained to the
/// checkout, all declared inputs are hashed, and ABI/profile consistency is
/// checked before any facts are returned to an engine consumer.
///
/// # Errors
///
/// Returns a configuration error when the contract cannot be read, parsed,
/// matched to `profile`, or bound to its declared source inputs.
pub fn load_native_build_contract(
    root: &Path,
    relative: &Path,
    profile: &TargetProfile,
) -> Result<NativeBuildContract> {
    Ok(load_bound_native_build_contract(root, relative, profile)?.contract)
}

/// Load a source contract and measure the very bytes that were validated.
///
/// # Errors
/// Rejects the same unsafe paths, malformed contracts, mismatched profiles
/// and altered inputs as [`load_native_build_contract`].
pub fn load_bound_native_build_contract(
    root: &Path,
    relative: &Path,
    profile: &TargetProfile,
) -> Result<LoadedNativeBuildContract> {
    let label = relative.display().to_string();
    let relative_text = relative
        .to_str()
        .ok_or_else(|| configuration_error(&label, "contract path is not valid UTF-8"))?;
    validate_source_relative_path(relative_text).map_err(|reason| {
        configuration_error(&label, &format!("invalid contract path: {reason}"))
    })?;

    let contract_path = crate::canonical_source_file(root, relative).map_err(|error| {
        configuration_error(
            &label,
            &format!("cannot resolve contract below source root: {error}"),
        )
    })?;
    let mut file = fs::File::open(&contract_path)
        .map_err(|error| configuration_error(&label, &format!("cannot read contract: {error}")))?;
    let initial_length = file
        .metadata()
        .map_err(|error| configuration_error(&label, &format!("cannot inspect contract: {error}")))?
        .len();
    if initial_length > MAX_NATIVE_BUILD_CONTRACT_BYTES {
        return Err(configuration_error(
            &label,
            "contract exceeds the 1048576-byte size limit",
        ));
    }
    let capacity = usize::try_from(initial_length).map_err(|_| {
        configuration_error(&label, "contract size cannot be represented in memory")
    })?;
    let mut bytes = Vec::with_capacity(capacity);
    file.by_ref()
        .take(MAX_NATIVE_BUILD_CONTRACT_BYTES)
        .read_to_end(&mut bytes)
        .map_err(|error| configuration_error(&label, &format!("cannot read contract: {error}")))?;
    let final_length = file
        .metadata()
        .map_err(|error| configuration_error(&label, &format!("cannot inspect contract: {error}")))?
        .len();
    if final_length > MAX_NATIVE_BUILD_CONTRACT_BYTES {
        return Err(configuration_error(
            &label,
            "contract exceeds the 1048576-byte size limit",
        ));
    }
    let sha256 = crate::sha256_bytes(&bytes);
    let content = String::from_utf8(bytes)
        .map_err(|error| configuration_error(&label, &format!("contract is not UTF-8: {error}")))?;
    let contract: NativeBuildContract = serde_json::from_str(&content)
        .map_err(|error| configuration_error(&label, &format!("invalid JSON contract: {error}")))?;

    validate_contract(root, relative, &contract, profile)?;
    Ok(LoadedNativeBuildContract {
        path: contract_path,
        sha256,
        contract,
    })
}

/// Bind source selectors to an independently verified compiler identity.
///
/// Token syntax or matching bit width alone does not establish ISA/ABI
/// compatibility. The caller must verify the payload before passing its
/// identity here; this function does not turn metadata into compiler proof.
///
/// # Errors
/// Rejects a different family, triple, ISA, ABI or code model.
pub fn validate_native_build_compiler(
    contract: &NativeBuildContract,
    compiler: &crate::ArosCompilerIdentity,
    triple: &str,
) -> Result<()> {
    let invalid = |message: &str| configuration_error(&contract.profile, message);
    compiler
        .validate_for_target(triple)
        .map_err(|_| invalid("verified compiler target identity is invalid"))?;
    let crate::ArosCompilerIdentity::Gnu { target, .. } = compiler else {
        return Err(invalid(
            "native source contract requires a verified GNU compiler",
        ));
    };
    if triple != contract.abi.target_triple
        || target.isa() != contract.abi.isa
        || target.abi() != contract.abi.abi
        || target.code_model() != contract.abi.code_model
    {
        return Err(invalid(
            "native source ISA/ABI/code-model/triple differs from the verified compiler target",
        ));
    }
    Ok(())
}

fn validate_contract(
    root: &Path,
    contract_path: &Path,
    contract: &NativeBuildContract,
    profile: &TargetProfile,
) -> Result<()> {
    let label = contract_path.display().to_string();
    let invalid = |message: String| configuration_error(&label, &message);

    if contract.schema_version != NATIVE_BUILD_SCHEMA_VERSION {
        return Err(invalid(format!(
            "schema_version must be {NATIVE_BUILD_SCHEMA_VERSION}"
        )));
    }
    if contract.qualification != NATIVE_BUILD_QUALIFICATION {
        return Err(invalid(format!(
            "qualification must be {NATIVE_BUILD_QUALIFICATION:?}"
        )));
    }
    if !safe_token(&contract.profile) || !safe_token(&contract.board) {
        return Err(invalid(
            "profile and board must be portable code tokens".to_owned(),
        ));
    }
    // Keep the full-contract diagnostic order stable. The shared invocation
    // validator repeats this check for consumers that load only that subset.
    validate_kernel_compiler_role(contract.kernel_compiler_role.as_deref(), &invalid)?;
    if contract.profile != profile.name {
        return Err(invalid(format!(
            "profile {:?} does not match selected target {:?}",
            contract.profile, profile.name
        )));
    }
    if contract.board != profile.bsp {
        return Err(invalid(format!(
            "board {:?} does not match selected target BSP {:?}",
            contract.board, profile.bsp
        )));
    }
    if !is_hex_string(&contract.source_baseline, 40) {
        return Err(invalid(
            "source_baseline must be exactly 40 hexadecimal characters".to_owned(),
        ));
    }
    validate_native_invocation_configuration(
        root,
        NativeInvocationConfiguration {
            inputs: &contract.inputs,
            make_variables: &contract.make_variables,
            host_make_variables: &contract.host_make_variables,
            make_include_bindings: &contract.make_include_bindings,
            generated_make_templates: &contract.generated_make_templates,
            metamake_projection: contract.metamake_projection.as_deref(),
            optional_meta_dependencies: &contract.optional_meta_dependencies,
            host_file_generators: &contract.host_file_generators,
            kernel_compiler_role: contract.kernel_compiler_role.as_deref(),
        },
        &invalid,
    )?;
    let inputs = contract
        .inputs
        .iter()
        .map(|input| input.path.as_str())
        .collect::<HashSet<_>>();

    // The residency-width rule belongs to core build policy, not shared ABI
    // validation. Check the common ABI prefix first to preserve its historical
    // diagnostic priority, then retain the build-specific residency gate.
    validate_native_abi_prefix(&contract.abi, profile, &invalid)?;
    let width = profile.arch.pointer_width();
    if width != 32 && contract.core.residency_policy.algorithm == "riscv32-xip-v1" {
        return Err(invalid(
            "riscv32-xip-v1 residency requires a 32-bit target".to_owned(),
        ));
    }
    validate_native_abi(&contract.abi, profile, &invalid)?;
    validate_core(&contract.core, &inputs, &invalid)?;
    validate_package(&contract.package, &inputs, &invalid)?;
    validate_media(&contract.media, &inputs, &invalid)?;
    Ok(())
}

/// Validate the source inputs and invocation-only configuration shared by
/// native consumers that do not load full build/package/media policy.
pub(crate) fn validate_native_invocation_configuration(
    root: &Path,
    configuration: NativeInvocationConfiguration<'_>,
    invalid: &impl Fn(String) -> ArosError,
) -> Result<()> {
    validate_kernel_compiler_role(configuration.kernel_compiler_role, invalid)?;
    if !(1..=MAX_NATIVE_BUILD_INPUTS).contains(&configuration.inputs.len()) {
        return Err(invalid(format!(
            "inputs must contain between 1 and {MAX_NATIVE_BUILD_INPUTS} entries"
        )));
    }

    if let Some(name) = configuration
        .make_variables
        .keys()
        .find(|name| NATIVE_HOST_MAKE_IDENTITIES.contains(&name.as_str()))
    {
        return Err(invalid(format!(
            "host identity {name:?} requires host_make_variables"
        )));
    }
    for host in configuration.host_make_variables.keys() {
        if host.is_empty()
            || host.len() > 64
            || !host.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"-_".contains(&byte)
            })
        {
            return Err(invalid(format!(
                "host_make_variables host key {host:?} is unsafe"
            )));
        }
    }
    for (name, value) in configuration.make_variables.iter().chain(
        configuration
            .host_make_variables
            .values()
            .flat_map(|variables| variables.iter()),
    ) {
        let valid_name = name.len() <= 64
            && name
                .as_bytes()
                .first()
                .is_some_and(|byte| byte.is_ascii_uppercase() || *byte == b'_')
            && name
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_');
        if !valid_name || NATIVE_RESERVED_MAKE_VARIABLES.contains(&name.as_str()) {
            return Err(invalid(format!(
                "make_variables key {name:?} is unsafe or shadows a target selector"
            )));
        }
        if value.len() > 128
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_.+-".contains(&byte))
        {
            return Err(invalid(format!(
                "make_variables.{name} must be a literal scalar (possibly empty), not Make or shell syntax"
            )));
        }
    }
    for variables in configuration.host_make_variables.values() {
        if variables
            .keys()
            .any(|name| configuration.make_variables.contains_key(name))
        {
            return Err(invalid(
                "shared and host Make configuration must not bind the same variable".into(),
            ));
        }
    }

    let mut declared_paths = HashSet::new();
    let mut resolved_paths = HashSet::new();
    let mut inputs = HashSet::new();
    for (index, input) in configuration.inputs.iter().enumerate() {
        let path = validate_source_relative_path(&input.path).map_err(|reason| {
            invalid(format!(
                "inputs[{index}].path {:?} is unsafe: {reason}",
                input.path
            ))
        })?;
        let path_key = input.path.to_ascii_lowercase();
        if !declared_paths.insert(path_key) {
            return Err(invalid(format!(
                "inputs[{index}].path {:?} duplicates another declared path",
                input.path
            )));
        }
        let resolved = crate::canonical_source_file(root, &path).map_err(|error| {
            invalid(format!(
                "inputs[{index}].path {:?} is not a regular source file below the root: {error}",
                input.path
            ))
        })?;
        if !resolved_paths.insert(resolved.clone()) {
            return Err(invalid(format!(
                "inputs[{index}].path {:?} resolves to a file already declared by another input",
                input.path
            )));
        }
        let measured = crate::sha256_file(&resolved).map_err(|error| {
            invalid(format!(
                "cannot hash inputs[{index}].path {:?}: {error}",
                input.path
            ))
        })?;
        if measured.digest != input.sha256 {
            return Err(invalid(format!(
                "inputs[{index}].path {:?} SHA-256 differs from the declared digest",
                input.path
            )));
        }
        inputs.insert(input.path.as_str());
    }

    if let Some(path) = configuration.metamake_projection {
        require_inventoried_path("metamake_projection", path, &inputs, invalid)?;
    }

    validate_optional_meta_dependencies(
        root,
        configuration.optional_meta_dependencies,
        &inputs,
        invalid,
    )?;
    validate_make_include_bindings(root, configuration.make_include_bindings, &inputs, invalid)?;
    for path in configuration.generated_make_templates.keys() {
        if configuration
            .make_include_bindings
            .keys()
            .any(|key| key.eq_ignore_ascii_case(path))
        {
            return Err(invalid(format!(
                "Make include {path:?} has both source and template bindings"
            )));
        }
    }
    crate::native_make_template::resolve_generated_make_templates(
        root,
        configuration.generated_make_templates,
        &configuration
            .inputs
            .iter()
            .map(|input| (input.path.clone(), input.sha256.clone()))
            .collect(),
    )?;
    crate::native_host_generator::validate_generators(
        configuration.host_file_generators,
        &inputs.iter().copied().collect(),
    )
    .map_err(invalid)?;
    Ok(())
}

fn validate_kernel_compiler_role(
    role: Option<&str>,
    invalid: &impl Fn(String) -> ArosError,
) -> Result<()> {
    if role.is_some_and(|role| role != "target") {
        return Err(invalid(
            "kernel_compiler_role admits only \"target\"".to_owned(),
        ));
    }
    Ok(())
}

fn validate_optional_meta_dependencies(
    root: &Path,
    dependencies: &[NativeOptionalMetaDependency],
    inputs: &HashSet<&str>,
    invalid: &impl Fn(String) -> ArosError,
) -> Result<()> {
    const MAX_OPTIONAL_META_DEPENDENCIES: usize = 64;

    if dependencies.len() > MAX_OPTIONAL_META_DEPENDENCIES {
        return Err(invalid(format!(
            "optional_meta_dependencies exceeds {MAX_OPTIONAL_META_DEPENDENCIES} entries"
        )));
    }

    let mut triples = HashSet::new();
    for (index, edge) in dependencies.iter().enumerate() {
        let prefix = format!("optional_meta_dependencies[{index}]");
        require_inventoried_path(&format!("{prefix}.recipe"), &edge.recipe, inputs, invalid)?;
        let recipe_name = Path::new(&edge.recipe)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let is_src = recipe_name
            .strip_suffix(".src")
            .is_some_and(|stem| !stem.is_empty());
        if !is_src && recipe_name != "mmakefile" {
            return Err(invalid(format!(
                "{prefix}.recipe {:?} must name a .src file or mmakefile",
                edge.recipe
            )));
        }
        canonical_native_build_input(root, Path::new(&edge.recipe)).map_err(|error| {
            invalid(format!(
                "{prefix}.recipe {:?} is not a regular non-symlink source file: {error}",
                edge.recipe
            ))
        })?;

        if !is_safe_meta_make_name(&edge.target) {
            return Err(invalid(format!(
                "{prefix}.target {:?} must be a literal MetaMake target name",
                edge.target
            )));
        }
        let safe_dependency = match edge.absence {
            NativeMetaAbsence::Selector => is_safe_optional_meta_dependency(&edge.dependency),
            NativeMetaAbsence::DisabledOwner => is_safe_meta_make_name(&edge.dependency),
        };
        if !safe_dependency {
            return Err(invalid(format!(
                "{prefix}.dependency {:?} must be a safe selector template or an explicitly disabled literal owner",
                edge.dependency
            )));
        }
        if !triples.insert((
            edge.recipe.as_str(),
            edge.target.as_str(),
            edge.dependency.as_str(),
        )) {
            return Err(invalid(format!(
                "{prefix} duplicates an optional MetaMake dependency triple"
            )));
        }
    }
    Ok(())
}

fn is_safe_meta_make_name(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(is_meta_make_name_byte)
}

const fn is_meta_make_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'+' | b'-')
}

fn is_safe_optional_meta_dependency(value: &str) -> bool {
    const SELECTORS: &[&str] = &[
        "AROS_TARGET_CPU",
        "AROS_TARGET_PLATFORM",
        "AROS_TARGET_LEGACY_PLATFORM",
        "AROS_TARGET_FAMILY",
        "AROS_TARGET_VARIANT",
        "AROS_TARGET_CPU32",
    ];

    let bytes = value.as_bytes();
    let mut index = 0;
    let mut has_selector = false;
    while index < bytes.len() {
        if bytes[index] == b'$' {
            if bytes.get(index + 1) != Some(&b'{') {
                return false;
            }
            let Some(end) = bytes[index + 2..].iter().position(|byte| *byte == b'}') else {
                return false;
            };
            let selector_end = index + 2 + end;
            let Ok(selector) = std::str::from_utf8(&bytes[index + 2..selector_end]) else {
                return false;
            };
            if !SELECTORS.contains(&selector) {
                return false;
            }
            has_selector = true;
            index = selector_end + 1;
        } else if is_meta_make_name_byte(bytes[index]) {
            index += 1;
        } else {
            return false;
        }
    }
    has_selector
}

fn validate_make_include_bindings(
    root: &Path,
    bindings: &BTreeMap<String, String>,
    inputs: &HashSet<&str>,
    invalid: &impl Fn(String) -> ArosError,
) -> Result<()> {
    if bindings.len() > MAX_NATIVE_MAKE_INCLUDE_BINDINGS {
        return Err(invalid(format!(
            "make_include_bindings exceeds {MAX_NATIVE_MAKE_INCLUDE_BINDINGS} entries"
        )));
    }
    let mut replacements = HashSet::new();
    for (include, replacement) in bindings {
        validate_native_make_include_path(include).map_err(|reason| {
            invalid(format!(
                "make_include_bindings key {include:?} is unsafe: {reason}"
            ))
        })?;
        validate_native_make_include_path(replacement).map_err(|reason| {
            invalid(format!(
                "make_include_bindings replacement {replacement:?} is unsafe: {reason}"
            ))
        })?;
        // This contract uses an exact lowercase suffix, matching CMake and
        // the case-sensitive inventoried path rather than host file guessing.
        if replacement.strip_suffix(".mk").is_none() {
            return Err(invalid(format!(
                "make_include_bindings replacement {replacement:?} must name a .mk file"
            )));
        }
        if !replacements.insert(replacement.to_ascii_lowercase()) {
            return Err(invalid(format!(
                "make_include_bindings contains duplicate replacement {replacement:?}"
            )));
        }
        require_inventoried_path("make_include_bindings key", include, inputs, invalid)?;
        require_inventoried_path(
            "make_include_bindings replacement",
            replacement,
            inputs,
            invalid,
        )?;
        for (field, value) in [
            ("key", include.as_str()),
            ("replacement", replacement.as_str()),
        ] {
            canonical_native_build_input(root, Path::new(value)).map_err(|error| {
                invalid(format!(
                    "make_include_bindings {field} {value:?} is not a regular non-symlink source file: {error}"
                ))
            })?;
        }
    }
    Ok(())
}

/// Resolve an inventoried input while rejecting symlinks at every path
/// component. The shared source helper proves containment and regular-file
/// type; this native contract boundary also requires stable non-symlink paths.
fn canonical_native_build_input(root: &Path, relative: &Path) -> std::io::Result<PathBuf> {
    let source_root = root.canonicalize()?;
    let mut candidate = source_root.clone();
    for component in relative.components() {
        candidate.push(component);
        let metadata = fs::symlink_metadata(&candidate)?;
        if metadata.file_type().is_symlink() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("source path '{}' crosses a symlink", candidate.display()),
            ));
        }
    }
    crate::canonical_source_file(&source_root, relative)
}

fn validate_native_make_include_path(value: &str) -> std::result::Result<PathBuf, &'static str> {
    if value.len() > 4096 {
        return Err("must not exceed 4096 bytes");
    }
    validate_source_relative_path(value)
}

pub(crate) fn validate_native_abi(
    abi: &NativeBuildAbi,
    profile: &TargetProfile,
    invalid: &impl Fn(String) -> ArosError,
) -> Result<()> {
    validate_native_abi_prefix(abi, profile, invalid)?;
    let width = profile.arch.pointer_width();
    if !(abi.isa.starts_with(&format!("rv{width}i"))
        || width == 64 && abi.isa.starts_with("rva") && abi.isa.ends_with("u64"))
    {
        return Err(invalid(
            "abi.isa width does not match selected target architecture".to_owned(),
        ));
    }
    if !matches!(abi.code_model.as_str(), "medlow" | "medany") {
        return Err(invalid(
            "abi.code_model must be either \"medlow\" or \"medany\"".to_owned(),
        ));
    }
    if profile.float_abi.as_deref() != Some(abi.abi.as_str()) {
        return Err(invalid(format!(
            "abi.abi {:?} does not match selected target float_abi {:?}",
            abi.abi, profile.float_abi
        )));
    }
    let transpiler = profile.transpiler.as_ref().ok_or_else(|| {
        invalid("selected target profile has no complete transpiler configuration".to_owned())
    })?;
    if transpiler.toolchain != "gnu" {
        return Err(invalid(format!(
            "native contract requires the GNU transpiler, selected profile uses {:?}",
            transpiler.toolchain
        )));
    }
    if abi.use_mmu != transpiler.use_mmu {
        return Err(invalid(format!(
            "abi.use_mmu {} does not match profile transpiler.use_mmu {}",
            abi.use_mmu, transpiler.use_mmu
        )));
    }
    let bootstrap_abi = profile.bootstrap_abi.as_ref().ok_or_else(|| {
        invalid("selected target profile has no complete bootstrap ABI configuration".to_owned())
    })?;
    if !matches!(
        bootstrap_abi.flavour.as_str(),
        "native" | "standalone" | "emulation"
    ) {
        return Err(invalid(
            "selected target profile has an unsupported bootstrap ABI flavour".to_owned(),
        ));
    }
    if abi.flavour != bootstrap_abi.flavour {
        return Err(invalid(format!(
            "abi.flavour {:?} does not match profile bootstrap ABI flavour {:?}",
            abi.flavour, bootstrap_abi.flavour
        )));
    }
    if abi.platform_smp != bootstrap_abi.platform_smp {
        return Err(invalid(format!(
            "abi.platform_smp {} does not match profile bootstrap ABI platform_smp {}",
            abi.platform_smp, bootstrap_abi.platform_smp
        )));
    }
    Ok(())
}

fn validate_native_abi_prefix(
    abi: &NativeBuildAbi,
    profile: &TargetProfile,
    invalid: &impl Fn(String) -> ArosError,
) -> Result<()> {
    for (field, value) in [
        ("abi.source_cpu", abi.source_cpu.as_str()),
        ("abi.target_triple", abi.target_triple.as_str()),
        ("abi.isa", abi.isa.as_str()),
        ("abi.abi", abi.abi.as_str()),
        ("abi.code_model", abi.code_model.as_str()),
        ("abi.flavour", abi.flavour.as_str()),
    ] {
        if !safe_token(value) {
            return Err(invalid(format!("{field} must be a portable code token")));
        }
    }
    if abi.source_cpu != profile.arch.source_cpu() {
        return Err(invalid(format!(
            "abi.source_cpu {:?} does not match architecture source CPU {:?}",
            abi.source_cpu,
            profile.arch.source_cpu()
        )));
    }
    let expected_triple = format!("{}-aros", profile.arch.source_cpu());
    if abi.target_triple != expected_triple {
        return Err(invalid(format!(
            "abi.target_triple {:?} does not match selected target {:?}",
            abi.target_triple, expected_triple
        )));
    }
    Ok(())
}

fn validate_core(
    core: &NativeBuildCore,
    inputs: &HashSet<&str>,
    invalid: &impl Fn(String) -> ArosError,
) -> Result<()> {
    core.residency_policy.validate().map_err(invalid)?;
    for (field, path) in [
        ("core.recipe", core.recipe.as_str()),
        ("core.linker_script", core.linker_script.as_str()),
        ("core.residency_check", core.residency_check.as_str()),
    ] {
        require_inventoried_path(field, path, inputs, invalid)?;
    }
    for (field, values) in [
        ("core.resources", &core.resources),
        ("core.libraries", &core.libraries),
        ("core.devices", &core.devices),
        ("core.link_libraries", &core.link_libraries),
    ] {
        validate_unique_tokens(field, values, invalid)?;
    }
    if core.compiler_runtime_role != NATIVE_COMPILER_RUNTIME_ROLE {
        return Err(invalid(format!(
            "core.compiler_runtime_role must be {NATIVE_COMPILER_RUNTIME_ROLE:?}"
        )));
    }
    Ok(())
}

fn validate_package(
    package: &NativeBuildPackage,
    inputs: &HashSet<&str>,
    invalid: &impl Fn(String) -> ArosError,
) -> Result<()> {
    require_inventoried_path("package.recipe", &package.recipe, inputs, invalid)?;
    if package.format != NATIVE_PACKAGE_FORMAT {
        return Err(invalid(format!(
            "package.format must be {NATIVE_PACKAGE_FORMAT:?}"
        )));
    }
    for (field, value) in [
        ("package.target", package.target.as_str()),
        (
            "package.limit_from_board",
            package.limit_from_board.as_str(),
        ),
    ] {
        if !safe_token(value) {
            return Err(invalid(format!("{field} must be a portable code token")));
        }
    }
    Ok(())
}

fn validate_media(
    media: &NativeBuildMedia,
    inputs: &HashSet<&str>,
    invalid: &impl Fn(String) -> ArosError,
) -> Result<()> {
    if let Some(path) = &media.geometry_contract {
        require_inventoried_path("media.geometry_contract", path, inputs, invalid)?;
    }
    for (field, path) in [
        ("media.board_rules", media.board_rules.as_str()),
        ("media.partition_table", media.partition_table.as_str()),
        (
            "media.bootloader_configuration",
            media.bootloader_configuration.as_str(),
        ),
        ("media.bootloader_patch", media.bootloader_patch.as_str()),
    ] {
        require_inventoried_path(field, path, inputs, invalid)?;
    }
    for (field, value) in [
        ("media.chip", media.chip.as_str()),
        ("media.core_partition", media.core_partition.as_str()),
        ("media.package_partition", media.package_partition.as_str()),
        (
            "media.development_volume_offset_from_board",
            media.development_volume_offset_from_board.as_str(),
        ),
        ("media.idf_version", media.idf_version.as_str()),
    ] {
        if !safe_token(value) {
            return Err(invalid(format!("{field} must be a portable code token")));
        }
    }
    Ok(())
}

fn require_inventoried_path(
    field: &str,
    value: &str,
    inputs: &HashSet<&str>,
    invalid: &impl Fn(String) -> ArosError,
) -> Result<()> {
    validate_source_relative_path(value)
        .map_err(|reason| invalid(format!("{field} has an unsafe path {value:?}: {reason}")))?;
    if !inputs.contains(value) {
        return Err(invalid(format!(
            "{field} path {value:?} is not declared in inputs"
        )));
    }
    Ok(())
}

fn validate_unique_tokens(
    field: &str,
    values: &[String],
    invalid: &impl Fn(String) -> ArosError,
) -> Result<()> {
    let mut seen = HashSet::new();
    for value in values {
        if !safe_token(value) || !seen.insert(value.as_str()) {
            return Err(invalid(format!(
                "{field} contains an unsafe or duplicate token {value:?}"
            )));
        }
    }
    Ok(())
}

pub(crate) fn validate_source_relative_path(
    value: &str,
) -> std::result::Result<PathBuf, &'static str> {
    if value.is_empty() || value.starts_with('/') || value.contains('\\') || value.contains(':') {
        return Err("must be a non-empty portable relative path");
    }
    let mut result = PathBuf::new();
    let mut components = 0;
    for component in Path::new(value).components() {
        match component {
            Component::Normal(segment) => {
                let Some(segment) = segment.to_str() else {
                    return Err("contains a non-UTF-8 path component");
                };
                if segment == "." || segment == ".." || !safe_token(segment) {
                    return Err("contains an unsafe path component");
                }
                components += 1;
                result.push(segment);
            }
            _ => return Err("must not contain absolute, current, or parent components"),
        }
    }
    if components == 0 || result.to_str() != Some(value) {
        return Err("must use canonical slash-separated source-relative syntax");
    }
    Ok(result)
}

fn safe_token(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'+'))
}

fn is_hex_string(value: &str, length: usize) -> bool {
    value.len() == length && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn configuration_error(file: &str, message: &str) -> ArosError {
    ArosError::Configuration {
        file: file.to_owned(),
        message: message.to_owned(),
    }
}

#[cfg(test)]
#[path = "native_build_contract_tests.rs"]
mod tests;
