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

fn deserialize_make_variables<'de, D>(
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

fn deserialize_host_make_variables<'de, D>(
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

fn deserialize_make_include_bindings<'de, D>(
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
    if contract
        .kernel_compiler_role
        .as_deref()
        .is_some_and(|role| role != "target")
    {
        return Err(invalid(
            "kernel_compiler_role admits only \"target\"".to_owned(),
        ));
    }
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
    if !(1..=MAX_NATIVE_BUILD_INPUTS).contains(&contract.inputs.len()) {
        return Err(invalid(format!(
            "inputs must contain between 1 and {MAX_NATIVE_BUILD_INPUTS} entries"
        )));
    }

    if let Some(name) = contract
        .make_variables
        .keys()
        .find(|name| NATIVE_HOST_MAKE_IDENTITIES.contains(&name.as_str()))
    {
        return Err(invalid(format!(
            "host identity {name:?} requires host_make_variables"
        )));
    }
    for host in contract.host_make_variables.keys() {
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
    for (name, value) in contract.make_variables.iter().chain(
        contract
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
    for variables in contract.host_make_variables.values() {
        if variables
            .keys()
            .any(|name| contract.make_variables.contains_key(name))
        {
            return Err(invalid(
                "shared and host Make configuration must not bind the same variable".into(),
            ));
        }
    }

    let mut declared_paths = HashSet::new();
    let mut resolved_paths = HashSet::new();
    let mut inputs = HashSet::new();
    for (index, input) in contract.inputs.iter().enumerate() {
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

    if let Some(path) = &contract.metamake_projection {
        require_inventoried_path("metamake_projection", path, &inputs, &invalid)?;
    }

    validate_optional_meta_dependencies(
        root,
        &contract.optional_meta_dependencies,
        &inputs,
        &invalid,
    )?;
    validate_make_include_bindings(root, &contract.make_include_bindings, &inputs, &invalid)?;
    for path in contract.generated_make_templates.keys() {
        if contract
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
        &contract.generated_make_templates,
        &contract
            .inputs
            .iter()
            .map(|input| (input.path.clone(), input.sha256.clone()))
            .collect(),
    )?;
    crate::native_host_generator::validate_generators(
        &contract.host_file_generators,
        &inputs.iter().copied().collect(),
    )
    .map_err(invalid)?;
    validate_abi(contract, profile, &invalid)?;
    validate_core(&contract.core, &inputs, &invalid)?;
    validate_package(&contract.package, &inputs, &invalid)?;
    validate_media(&contract.media, &inputs, &invalid)?;
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

fn validate_abi(
    contract: &NativeBuildContract,
    profile: &TargetProfile,
    invalid: &impl Fn(String) -> ArosError,
) -> Result<()> {
    let abi = &contract.abi;
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
    let width = profile.arch.pointer_width();
    if width != 32 && contract.core.residency_policy.algorithm == "riscv32-xip-v1" {
        return Err(invalid(
            "riscv32-xip-v1 residency requires a 32-bit target".to_owned(),
        ));
    }
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

fn validate_source_relative_path(value: &str) -> std::result::Result<PathBuf, &'static str> {
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
mod tests {
    use super::*;
    use crate::{arch::Architecture, BootstrapAbiProfile, TranspilerProfile};
    use serde_json::{json, Value};

    const CONTRACT_PATH: &str = "native-build-v1.json";
    const INPUT_FILES: &[(&str, &str)] = &[
        ("configure.in", "synthetic configure source\n"),
        ("board/board.mk", "synthetic board rules\n"),
        ("kernel/makefile.src", "synthetic core recipe\n"),
        ("kernel/mmakefile", "synthetic MetaMake recipe\n"),
        ("kernel/linker.lds", "synthetic linker script\n"),
        ("kernel/check.sh", "synthetic residency check\n"),
        ("package/makefile.src", "synthetic package recipe\n"),
        ("boot/partitions.csv", "synthetic partition table\n"),
        (
            "boot/sdkconfig.defaults",
            "synthetic bootloader configuration\n",
        ),
        ("boot/standalone.diff", "synthetic bootloader patch\n"),
    ];

    struct Fixture {
        directory: tempfile::TempDir,
        profile: TargetProfile,
        document: Value,
    }

    impl Fixture {
        fn root(&self) -> PathBuf {
            self.directory.path().join("source")
        }

        fn write_contract(&self, document: &Value) {
            fs::write(
                self.directory.path().join("source").join(CONTRACT_PATH),
                serde_json::to_vec_pretty(document).unwrap(),
            )
            .unwrap();
        }

        fn load(&self) -> Result<NativeBuildContract> {
            load_native_build_contract(
                &self.directory.path().join("source"),
                Path::new(CONTRACT_PATH),
                &self.profile,
            )
        }
    }

    #[test]
    fn kernel_compiler_role_admits_only_the_target_role() {
        let fixture = new_fixture();
        let mut document = fixture.document.clone();
        document["kernel_compiler_role"] = json!("target");
        fixture.write_contract(&document);
        assert_eq!(
            fixture.load().unwrap().kernel_compiler_role.as_deref(),
            Some("target")
        );
        for role in ["kernel", "host", ""] {
            document["kernel_compiler_role"] = json!(role);
            fixture.write_contract(&document);
            assert!(
                error_text(fixture.load()).contains("kernel_compiler_role"),
                "{role}"
            );
        }
    }

    fn new_fixture() -> Fixture {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("source");
        fs::create_dir_all(&root).unwrap();
        let profile = TargetProfile {
            name: "synthetic-esp32p4-profile".into(),
            arch: Architecture::Riscv32,
            platform: "synthetic-platform".into(),
            bsp: "invented-board".into(),
            features: Vec::new(),
            float_abi: Some("ilp32f".into()),
            bootloader: None,
            transpiler: Some(TranspilerProfile {
                family: String::new(),
                variant: String::new(),
                toolchain: "gnu".into(),
                cpu32: String::new(),
                use_mmu: false,
                mesa_version: None,
            }),
            bootstrap_abi: Some(BootstrapAbiProfile {
                flavour: "standalone".into(),
                platform_smp: false,
            }),
            native_build_contract: None,
            toolchain_profile: None,
        };
        let inputs = INPUT_FILES
            .iter()
            .map(|(path, contents)| {
                let full_path = root.join(path);
                fs::create_dir_all(full_path.parent().unwrap()).unwrap();
                fs::write(&full_path, contents).unwrap();
                json!({
                    "path": path,
                    "sha256": crate::sha256_bytes(contents.as_bytes()).to_string(),
                })
            })
            .collect::<Vec<_>>();
        let document = json!({
            "schema_version": 1,
            "profile": profile.name,
            "board": profile.bsp,
            "source_baseline": "0123456789abcdef0123456789abcdef01234567",
            "qualification": "experimental-unqualified",
            "inputs": inputs,
            "abi": {
                "source_cpu": "riscv",
                "target_triple": "riscv-aros",
                "isa": "rv32imafc_zicsr_zifencei_zaamo_zalrsc",
                "abi": "ilp32f",
                "code_model": "medany",
                "flavour": "standalone",
                "platform_smp": false,
                "use_mmu": false
            },
            "core": {
                "recipe": "kernel/makefile.src",
                "linker_script": "kernel/linker.lds",
                "resources": ["kernel", "task"],
                "libraries": ["exec", "debug"],
                "devices": ["timer", "flashdisk"],
                "link_libraries": ["exec", "arossupport", "autoinit"],
                "compiler_runtime_role": "libgcc",
                "residency_check": "kernel/check.sh",
                "residency_policy": {
                    "algorithm": "riscv32-xip-v1", "section": ".sramtext",
                    "flash_start": 1_073_741_824, "flash_end": 1_140_850_688,
                    "sram_start": 1_341_128_704, "sram_end": 1_341_652_992
                }
            },
            "package": {
                "recipe": "package/makefile.src",
                "format": "aros-pkg-v1",
                "target": "kernel-package-synthetic-riscv",
                "limit_from_board": "SYNTHETIC_BOARD_PACKAGE_LIMIT"
            },
            "media": {
                "chip": "synthetic-chip",
                "board_rules": "board/board.mk",
                "partition_table": "boot/partitions.csv",
                "core_partition": "core_partition",
                "package_partition": "package_partition",
                "development_volume_offset_from_board": "SYNTHETIC_BOARD_VOLUME_OFFSET",
                "bootloader_configuration": "boot/sdkconfig.defaults",
                "bootloader_patch": "boot/standalone.diff",
                "idf_version": "6.0.1"
            }
        });
        let fixture = Fixture {
            directory,
            profile,
            document,
        };
        fixture.write_contract(&fixture.document);
        fixture
    }

    fn make_include_fixture() -> Fixture {
        let mut fixture = new_fixture();
        for (path, contents) in [
            ("config/aros.cfg", "legacy configuration\n"),
            ("config/secondary.cfg", "secondary configuration\n"),
            ("arch/native-config.mk", "NATIVE_PROJECTION := 1\n"),
        ] {
            let full_path = fixture.root().join(path);
            fs::create_dir_all(full_path.parent().unwrap()).unwrap();
            fs::write(&full_path, contents).unwrap();
            fixture.document["inputs"]
                .as_array_mut()
                .unwrap()
                .push(json!({
                    "path": path,
                    "sha256": crate::sha256_bytes(contents.as_bytes()).to_string(),
                }));
        }
        fs::write(fixture.root().join("arch/foreign.mk"), "FOREIGN := 1\n").unwrap();
        fixture.document["make_include_bindings"] =
            json!({"config/aros.cfg": "arch/native-config.mk"});
        fixture.write_contract(&fixture.document);
        fixture
    }

    fn error_text(result: Result<NativeBuildContract>) -> String {
        result.unwrap_err().to_string()
    }

    #[test]
    fn source_input_inventory_keeps_an_explicit_bounded_limit() {
        let mut fixture = new_fixture();
        let initial_count = fixture.document["inputs"].as_array().unwrap().len();
        for index in initial_count..=MAX_NATIVE_BUILD_INPUTS {
            let path = format!("additional-{index}.src");
            let bytes = format!("source input {index}\n");
            fs::write(fixture.root().join(&path), &bytes).unwrap();
            fixture.document["inputs"]
                .as_array_mut()
                .unwrap()
                .push(json!({
                    "path": path, "sha256": crate::sha256_bytes(bytes.as_bytes()),
                }));
            if index == MAX_NATIVE_BUILD_INPUTS - 1 {
                fixture.write_contract(&fixture.document);
                assert_eq!(
                    fixture.load().unwrap().inputs.len(),
                    MAX_NATIVE_BUILD_INPUTS
                );
            }
        }
        fixture.write_contract(&fixture.document);
        assert!(error_text(fixture.load()).contains("between 1 and 128 entries"));
    }

    #[test]
    fn loads_source_facts_for_an_arbitrary_synthetic_board() {
        let fixture = new_fixture();
        let contract = fixture.load().unwrap();

        assert_eq!(contract.profile, "synthetic-esp32p4-profile");
        assert_eq!(contract.board, "invented-board");
        assert_eq!(contract.qualification, NATIVE_BUILD_QUALIFICATION);
        assert_eq!(contract.inputs.len(), INPUT_FILES.len());
        assert_eq!(contract.package.format, NATIVE_PACKAGE_FORMAT);
        assert_eq!(
            contract.core.compiler_runtime_role,
            NATIVE_COMPILER_RUNTIME_ROLE
        );
        assert_eq!(
            contract.media.development_volume_offset_from_board,
            "SYNTHETIC_BOARD_VOLUME_OFFSET"
        );
        let bound = load_bound_native_build_contract(
            &fixture.root(),
            Path::new(CONTRACT_PATH),
            &fixture.profile,
        )
        .unwrap();
        assert_eq!(bound.contract, contract);
        assert_eq!(
            bound.path,
            fixture.root().join(CONTRACT_PATH).canonicalize().unwrap()
        );
        assert_eq!(
            bound.sha256,
            crate::sha256_file(&bound.path).unwrap().digest
        );
    }

    #[test]
    fn native_partition_generation_binds_contract_profile_and_every_input() {
        let fixture = new_fixture();
        let csv = b"storage,0x40,0,0x9000,4K,\n";
        let path = fixture.root().join("boot/partitions.csv");
        fs::write(&path, csv).unwrap();
        let mut document = fixture.document.clone();
        for input in document["inputs"].as_array_mut().unwrap() {
            if input["path"] == "boot/partitions.csv" {
                input["sha256"] = json!(crate::sha256_bytes(csv));
            }
        }
        fixture.write_contract(&document);
        let contract_hash = crate::sha256_file(&fixture.root().join(CONTRACT_PATH))
            .unwrap()
            .digest;
        let generate = |profile: &TargetProfile, expected: &Sha256Digest| {
            crate::esp_partition::encode_bound_native_partition_table(
                &fixture.root(),
                Path::new(CONTRACT_PATH),
                profile,
                expected,
                0x8000,
                1 << 20,
            )
        };
        let generated = generate(&fixture.profile, &contract_hash).unwrap();
        assert_eq!(generated.binding.board, "invented-board");
        assert_eq!(generated.binding.source_contract_sha256, contract_hash);
        assert_eq!(generated.binding.contract_inputs.len(), INPUT_FILES.len());
        assert_eq!(
            generated.binding.partition_source_sha256,
            crate::sha256_bytes(csv)
        );
        assert_eq!(
            generated.binding.artifact_sha256,
            crate::sha256_bytes(&generated.artifact.bytes)
        );
        assert_eq!(generated.binding.artifact_size, 3072);
        assert_eq!(
            generated.binding.geometry_origin,
            "explicit-caller-input-unqualified"
        );
        assert_eq!(
            generated.binding.qualification,
            "experimental-source-consistency-only"
        );
        assert!(
            generate(&fixture.profile, &crate::sha256_bytes(b"another contract"))
                .unwrap_err()
                .contains("contract SHA-256")
        );
        let mut wrong_profile = fixture.profile.clone();
        wrong_profile.bsp = "other-board".into();
        assert!(generate(&wrong_profile, &contract_hash).is_err());
        for (relative, _) in INPUT_FILES {
            let path = fixture.root().join(relative);
            let original = fs::read(&path).unwrap();
            fs::write(&path, b"changed").unwrap();
            assert!(
                generate(&fixture.profile, &contract_hash)
                    .unwrap_err()
                    .contains("SHA-256 differs"),
                "accepted altered {relative}"
            );
            fs::write(path, original).unwrap();
        }
        assert_eq!(
            generate(&fixture.profile, &contract_hash).unwrap(),
            generated
        );
    }

    fn media_fixture() -> (Fixture, Value) {
        let mut fixture = new_fixture();
        let csv =
            b"core_partition,app,ota_0,0x10000,0x20000,\npkg_partition,0x47,3,0x30000,0x20000,\n";
        fs::write(fixture.root().join("boot/partitions.csv"), csv).unwrap();
        for input in fixture.document["inputs"].as_array_mut().unwrap() {
            if input["path"] == "boot/partitions.csv" {
                input["sha256"] = json!(crate::sha256_bytes(csv));
            }
        }
        fixture.document["media"]["geometry_contract"] = json!("boot/geometry.json");
        fixture.document["media"]["package_partition"] = json!("pkg_partition");
        let geometry = json!({
            "schema_version": 1, "format": "esp-unsigned-flash-v1",
            "profile": fixture.profile.name, "board": fixture.profile.bsp,
            "chip": "synthetic-chip", "chip_id": 21,
            "revision_min": 50, "revision_max": 60, "idf_version": "6.0.1",
            "flash_bytes": 1 << 20, "erase_sector_bytes": 0x1000,
            "bootloader_offset": 0x2000, "partition_table_offset": 0x8000,
            "core_partition_kind": 0, "core_partition_subtype": 0x10,
            "package_partition_kind": 0x47, "package_partition_subtype": 3,
            "package_limit_bytes": 0x10000,
            "development_volume_offset": 0x40000,
            "development_volume_size_bytes": 0x10000
        });
        fixture.document["inputs"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "path": "boot/geometry.json", "sha256": crate::sha256_bytes(b"placeholder")
            }));
        write_media_geometry(&fixture, &geometry);
        (fixture, geometry)
    }

    fn write_media_geometry(fixture: &Fixture, geometry: &Value) {
        let bytes = serde_json::to_vec_pretty(geometry).unwrap();
        fs::write(fixture.root().join("boot/geometry.json"), &bytes).unwrap();
        let mut document = fixture.document.clone();
        for input in document["inputs"].as_array_mut().unwrap() {
            if input["path"] == "boot/geometry.json" {
                input["sha256"] = json!(crate::sha256_bytes(&bytes));
            }
        }
        fixture.write_contract(&document);
    }

    fn resolve_media(
        fixture: &Fixture,
    ) -> std::result::Result<crate::native_media::BoundNativeMedia, String> {
        let expected = crate::sha256_file(&fixture.root().join(CONTRACT_PATH))
            .unwrap()
            .digest;
        crate::native_media::resolve_bound_native_media(
            &fixture.root(),
            Path::new(CONTRACT_PATH),
            &fixture.profile,
            &expected,
        )
    }

    #[test]
    fn native_media_geometry_uses_only_source_numbers_and_revalidates_every_input() {
        let (fixture, _) = media_fixture();
        let resolved = resolve_media(&fixture).unwrap();
        let loaded = load_bound_native_build_contract(
            &fixture.root(),
            Path::new(CONTRACT_PATH),
            &fixture.profile,
        )
        .unwrap();
        assert_eq!(
            resolved.binding.layout.source_contract_sha256,
            loaded.sha256
        );
        assert_eq!(resolved.binding.geometry.chip_id, 21);
        assert_eq!(resolved.binding.geometry.flash_bytes, 1 << 20);
        assert_eq!(resolved.binding.geometry.revision_min, 50);
        assert_eq!(resolved.binding.geometry.package_partition_kind, 0x47);
        assert_eq!(resolved.binding.geometry.package_partition_subtype, 3);
        assert_eq!(
            resolved.binding.partition_binding.geometry_origin,
            "source-owned-media-geometry"
        );
        assert_eq!(resolved.binding.layout.slots.len(), 5);
        let expected = [
            ("bootloader", 0x2000, 0x8000, false),
            ("partition-table", 0x8000, 0x9000, false),
            ("core", 0x10000, 0x30000, true),
            ("bsp", 0x30000, 0x40000, true),
            ("developer", 0x40000, 0x50000, true),
        ];
        for (slot, (role, start, end, native)) in resolved.binding.layout.slots.iter().zip(expected)
        {
            assert_eq!(slot.role, role);
            assert_eq!(slot.range, crate::flash_plan::FlashRange { start, end });
            assert_eq!(slot.native_build, native);
        }
        for input in loaded.contract.inputs {
            let path = fixture.root().join(&input.path);
            let original = fs::read(&path).unwrap();
            fs::write(&path, b"changed").unwrap();
            assert!(
                resolve_media(&fixture)
                    .unwrap_err()
                    .contains("SHA-256 differs"),
                "accepted altered {}",
                input.path
            );
            fs::write(path, original).unwrap();
        }
        assert_eq!(resolve_media(&fixture).unwrap(), resolved);
        assert!(crate::native_media::resolve_bound_native_media(
            &fixture.root(),
            Path::new(CONTRACT_PATH),
            &fixture.profile,
            &crate::sha256_bytes(b"different contract"),
        )
        .unwrap_err()
        .contains("contract SHA-256 differs"));
    }

    #[test]
    fn native_media_geometry_refuses_semantic_mutations_even_with_matching_input_hashes() {
        let (fixture, geometry) = media_fixture();
        for (field, bad_value) in [
            ("schema_version", json!(2)),
            ("format", json!("run-command")),
            ("profile", json!("other-profile")),
            ("board", json!("other-board")),
            ("chip", json!("other-chip")),
            ("idf_version", json!("0.0.0")),
            ("flash_bytes", json!(0)),
            ("flash_bytes", json!(3 << 20)),
            ("flash_bytes", json!(1_u64 << 32)),
            ("erase_sector_bytes", json!(0x800)),
            ("revision_max", json!(49)),
            ("bootloader_offset", json!(0x8000)),
            ("bootloader_offset", json!(0x2001)),
            ("partition_table_offset", json!(0x8001)),
            ("partition_table_offset", json!(0x10000)),
            ("core_partition_kind", json!(1)),
            ("core_partition_subtype", json!(0x11)),
            ("package_partition_kind", json!(0x40)),
            ("package_partition_kind", json!(0xff)),
            ("package_partition_subtype", json!(0)),
            ("package_partition_subtype", json!(256)),
            ("package_limit_bytes", json!(0)),
            ("package_limit_bytes", json!(0x10001)),
            ("package_limit_bytes", json!(u64::MAX)),
            ("development_volume_offset", json!(0x30000)),
            ("development_volume_size_bytes", json!(0)),
            ("development_volume_size_bytes", json!(0x10001)),
            ("development_volume_size_bytes", json!(u64::MAX)),
            ("command", json!("false")),
        ] {
            let mut altered = geometry.clone();
            altered[field] = bad_value;
            write_media_geometry(&fixture, &altered);
            assert!(
                resolve_media(&fixture).is_err(),
                "accepted {field}: {altered}"
            );
        }
        write_media_geometry(&fixture, &geometry);
        assert!(resolve_media(&fixture).is_ok());
    }

    #[test]
    fn native_media_requires_explicit_inventoried_geometry_and_matching_partition_roles() {
        let fixture = new_fixture();
        assert!(fixture.load().is_ok());
        assert!(resolve_media(&fixture)
            .unwrap_err()
            .contains("no explicit media geometry"));

        let (fixture, _) = media_fixture();
        let original_document: Value =
            serde_json::from_slice(&fs::read(fixture.root().join(CONTRACT_PATH)).unwrap()).unwrap();
        for wrong_path in ["boot/missing.json", "../geometry.json"] {
            let mut document = original_document.clone();
            document["media"]["geometry_contract"] = json!(wrong_path);
            fixture.write_contract(&document);
            assert!(resolve_media(&fixture).is_err());
        }
        for invalid_type in [Value::Null, json!(true), json!([]), json!({})] {
            let mut document = original_document.clone();
            document["media"]["geometry_contract"] = invalid_type;
            fixture.write_contract(&document);
            assert!(fixture.load().is_err());
        }
        for (field, value) in [
            ("core_partition", "missing"),
            ("core_partition", "pkg_partition"),
            ("package_partition", "core_partition"),
        ] {
            let mut document = original_document.clone();
            document["media"][field] = json!(value);
            fixture.write_contract(&document);
            assert!(resolve_media(&fixture).is_err(), "accepted {field}={value}");
        }
        for flags in ["encrypted", "readonly", "encrypted:readonly"] {
            let csv = format!(
                "core_partition,app,ota_0,0x10000,0x20000,{flags}\npkg_partition,0x47,3,0x30000,0x20000,\n"
            );
            fs::write(fixture.root().join("boot/partitions.csv"), &csv).unwrap();
            let mut document = original_document.clone();
            for input in document["inputs"].as_array_mut().unwrap() {
                if input["path"] == "boot/partitions.csv" {
                    input["sha256"] = json!(crate::sha256_bytes(csv.as_bytes()));
                }
            }
            fixture.write_contract(&document);
            assert!(resolve_media(&fixture)
                .unwrap_err()
                .contains("unsupported flags"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn native_media_geometry_refuses_a_symlink_even_when_it_matches_the_inventory() {
        let (fixture, _) = media_fixture();
        let path = fixture.root().join("boot/geometry.json");
        fs::rename(&path, fixture.root().join("boot/original.json")).unwrap();
        std::os::unix::fs::symlink("original.json", &path).unwrap();
        assert!(resolve_media(&fixture)
            .unwrap_err()
            .contains("cannot open media geometry"));
    }

    #[test]
    fn native_media_refuses_reinventoried_partition_kind_and_subtype_drift() {
        let (fixture, geometry) = media_fixture();
        let original_csv = fs::read(fixture.root().join("boot/partitions.csv")).unwrap();
        let original_document: Value =
            serde_json::from_slice(&fs::read(fixture.root().join(CONTRACT_PATH)).unwrap()).unwrap();
        for (core_kind, core_subtype, package_kind, package_subtype) in [
            ("data", "0x10", "0x47", "3"),
            ("app", "ota_1", "0x47", "3"),
            ("app", "ota_0", "0x48", "3"),
            ("app", "ota_0", "0x47", "4"),
        ] {
            let csv = format!(
                "core_partition,{core_kind},{core_subtype},0x10000,0x20000,\npkg_partition,{package_kind},{package_subtype},0x30000,0x20000,\n"
            );
            fs::write(fixture.root().join("boot/partitions.csv"), &csv).unwrap();
            let mut document = original_document.clone();
            for input in document["inputs"].as_array_mut().unwrap() {
                if input["path"] == "boot/partitions.csv" {
                    input["sha256"] = json!(crate::sha256_bytes(csv.as_bytes()));
                }
            }
            fixture.write_contract(&document);
            assert!(fixture.load().is_ok());
            assert!(resolve_media(&fixture)
                .unwrap_err()
                .contains("partition formats differ"));
        }
        fs::write(fixture.root().join("boot/partitions.csv"), &original_csv).unwrap();
        // A field omitted or explicitly null is not a board-specific default.
        for field in [
            "core_partition_kind",
            "core_partition_subtype",
            "package_partition_kind",
            "package_partition_subtype",
        ] {
            let mut absent = geometry.clone();
            absent.as_object_mut().unwrap().remove(field);
            write_media_geometry(&fixture, &absent);
            assert!(resolve_media(&fixture)
                .unwrap_err()
                .contains("invalid media geometry"));
            absent[field] = Value::Null;
            write_media_geometry(&fixture, &absent);
            assert!(resolve_media(&fixture)
                .unwrap_err()
                .contains("invalid media geometry"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn native_partition_generation_refuses_an_in_tree_csv_symlink() {
        use std::os::unix::fs::symlink;
        let fixture = new_fixture();
        let csv = b"storage,0x40,0,0x9000,4K,\n";
        let path = fixture.root().join("boot/partitions.csv");
        fs::write(&path, csv).unwrap();
        let mut document = fixture.document.clone();
        for input in document["inputs"].as_array_mut().unwrap() {
            if input["path"] == "boot/partitions.csv" {
                input["sha256"] = json!(crate::sha256_bytes(csv));
            }
        }
        fixture.write_contract(&document);
        let contract_hash = crate::sha256_file(&fixture.root().join(CONTRACT_PATH))
            .unwrap()
            .digest;
        let original = fixture.root().join("boot/original.csv");
        fs::rename(&path, &original).unwrap();
        symlink("original.csv", &path).unwrap();
        assert!(crate::esp_partition::encode_bound_native_partition_table(
            &fixture.root(),
            Path::new(CONTRACT_PATH),
            &fixture.profile,
            &contract_hash,
            0x8000,
            1 << 20,
        )
        .is_err());
        assert_eq!(fs::read(original).unwrap(), csv);
    }

    #[test]
    fn residency_policy_rejects_unknown_empty_overlapping_and_non_rv32_ranges() {
        for (field, value) in [
            ("algorithm", json!("run-source-script")),
            ("section", json!(".text")),
            ("flash_end", json!(1_073_741_824)),
            ("flash_end", json!(4_294_967_297_u64)),
            ("sram_start", json!(1_073_741_824)),
            ("flash_start", json!(-1)),
        ] {
            let fixture = new_fixture();
            let mut document = fixture.document.clone();
            document["core"]["residency_policy"][field] = value;
            fixture.write_contract(&document);
            assert!(fixture.load().is_err(), "{field}");
        }
        let fixture = new_fixture();
        let mut document = fixture.document.clone();
        document["core"]["residency_policy"]["command"] = json!("evil");
        fixture.write_contract(&document);
        assert!(error_text(fixture.load()).contains("unknown field"));
    }

    #[test]
    fn binds_exact_native_selectors_to_the_verified_compiler_not_just_width() {
        let fixture = new_fixture();
        let contract = fixture.load().unwrap();
        let compiler: crate::ArosCompilerIdentity = serde_json::from_value(json!({
            "family": "gnu", "gcc_version": "16.2.0", "binutils_version": "2.47",
            "target": {
                "schema": "aros-riscv-target-v1", "isa": contract.abi.isa,
                "abi": "ilp32f", "code_model": "medany",
                "architecture": "rv32i2p1_m2p0_a2p1_f2p2_c2p0_zicsr2p0_zifencei2p0_zaamo1p0_zalrsc1p0",
                "unaligned_access": false, "atomic_abi": 0, "x3_reg_usage": 0
            }
        })).unwrap();
        validate_native_build_compiler(&contract, &compiler, "riscv-aros").unwrap();
        for field in ["isa", "abi", "code_model"] {
            let mut changed = serde_json::to_value(&contract).unwrap();
            changed["abi"][field] = match field {
                "isa" => json!("rv32imafc"),
                "abi" => json!("ilp32"),
                _ => json!("medlow"),
            };
            let changed = serde_json::from_value(changed).unwrap();
            assert!(validate_native_build_compiler(&changed, &compiler, "riscv-aros").is_err());
        }
        let llvm = crate::ArosCompilerIdentity::Llvm {
            version: "11.0.1".into(),
        };
        assert!(validate_native_build_compiler(&contract, &llvm, "riscv-aros").is_err());
        assert!(validate_native_build_compiler(&contract, &compiler, "riscv64-aros").is_err());
    }

    #[test]
    fn rejects_modified_or_missing_inventoried_inputs() {
        let fixture = new_fixture();
        fs::write(
            fixture.root().join("kernel/linker.lds"),
            "mutated linker script\n",
        )
        .unwrap();
        assert!(error_text(fixture.load()).contains("SHA-256 differs"));

        let fixture = new_fixture();
        fs::remove_file(fixture.root().join("kernel/linker.lds")).unwrap();
        assert!(error_text(fixture.load()).contains("kernel/linker.lds"));
    }

    #[test]
    fn rejects_absolute_parent_and_command_like_input_paths() {
        for path in ["/outside", "../outside", "kernel/evil;make", "C:/outside"] {
            let fixture = new_fixture();
            let mut document = fixture.document.clone();
            document["inputs"][0]["path"] = json!(path);
            fixture.write_contract(&document);
            let message = error_text(fixture.load());
            assert!(message.contains("unsafe"), "{path}: {message}");
        }
    }

    #[test]
    fn rejects_contracts_larger_than_one_mebibyte_before_parsing() {
        let fixture = new_fixture();
        fs::write(
            fixture.root().join(CONTRACT_PATH),
            vec![b' '; usize::try_from(MAX_NATIVE_BUILD_CONTRACT_BYTES).unwrap() + 1],
        )
        .unwrap();

        assert!(error_text(fixture.load()).contains("1048576-byte size limit"));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_input_symlink_that_escapes_source_root() {
        use std::os::unix::fs::symlink;

        let fixture = new_fixture();
        let outside = fixture.directory.path().join("outside.txt");
        fs::write(&outside, "outside source\n").unwrap();
        symlink(&outside, fixture.root().join("escape.txt")).unwrap();
        let mut document = fixture.document.clone();
        document["inputs"][0]["path"] = json!("escape.txt");
        fixture.write_contract(&document);

        let message = error_text(fixture.load());
        assert!(message.contains("escapes canonical scan root"));
    }

    #[test]
    fn rejects_duplicate_input_hash_entries_and_unknown_fields() {
        let fixture = new_fixture();
        let mut duplicate = fixture.document.clone();
        let first_input = duplicate["inputs"][0].clone();
        duplicate["inputs"]
            .as_array_mut()
            .unwrap()
            .push(first_input);
        fixture.write_contract(&duplicate);
        assert!(error_text(fixture.load()).contains("duplicates another declared path"));

        for (location, field) in [
            ("root", "unexpected"),
            ("abi", "command"),
            ("core", "command"),
            ("package", "command"),
            ("media", "command"),
            ("input", "command"),
        ] {
            let fixture = new_fixture();
            let mut document = fixture.document.clone();
            if location == "root" {
                document[field] = json!("echo unsafe");
            } else if location == "input" {
                document["inputs"][0][field] = json!("echo unsafe");
            } else {
                document[location][field] = json!("echo unsafe");
            }
            fixture.write_contract(&document);
            assert!(error_text(fixture.load()).contains("unknown field"));
        }
    }

    #[test]
    fn rejects_incomplete_or_mismatched_profile_abi() {
        let fixture = new_fixture();
        let mut profile = fixture.profile.clone();
        profile.transpiler = None;
        assert!(error_text(load_native_build_contract(
            &fixture.directory.path().join("source"),
            Path::new(CONTRACT_PATH),
            &profile,
        ))
        .contains("no complete transpiler"));

        let mut profile = fixture.profile.clone();
        profile.bootstrap_abi = None;
        assert!(error_text(load_native_build_contract(
            &fixture.directory.path().join("source"),
            Path::new(CONTRACT_PATH),
            &profile,
        ))
        .contains("no complete bootstrap ABI"));

        let mutations: [fn(&mut Value); 10] = [
            |document: &mut Value| document["profile"] = json!("other-profile"),
            |document: &mut Value| document["board"] = json!("other-board"),
            |document: &mut Value| document["abi"]["source_cpu"] = json!("riscv32"),
            |document: &mut Value| document["abi"]["target_triple"] = json!("riscv64-aros"),
            |document: &mut Value| document["abi"]["isa"] = json!("rv64gc"),
            |document: &mut Value| document["abi"]["flavour"] = json!("native"),
            |document: &mut Value| document["abi"]["platform_smp"] = json!(true),
            |document: &mut Value| document["abi"]["use_mmu"] = json!(true),
            |document: &mut Value| document["abi"]["abi"] = json!("ilp32d"),
            |document: &mut Value| document["abi"]["code_model"] = json!("unknown"),
        ];
        for mutation in mutations {
            let fixture = new_fixture();
            let mut document = fixture.document.clone();
            mutation(&mut document);
            fixture.write_contract(&document);
            assert!(matches!(
                fixture.load(),
                Err(ArosError::Configuration { .. })
            ));
        }

        let fixture = new_fixture();
        let mut profile = fixture.profile.clone();
        profile.transpiler.as_mut().unwrap().toolchain = "llvm".into();
        let message = error_text(load_native_build_contract(
            &fixture.directory.path().join("source"),
            Path::new(CONTRACT_PATH),
            &profile,
        ));
        assert!(message.contains("requires the GNU transpiler"));
    }

    #[test]
    fn rejects_referenced_source_paths_outside_the_measured_inventory() {
        let fixture = new_fixture();
        fs::write(
            fixture.root().join("kernel/uninventoried.lds"),
            "unlocked\n",
        )
        .unwrap();
        let mut document = fixture.document.clone();
        document["core"]["linker_script"] = json!("kernel/uninventoried.lds");
        fixture.write_contract(&document);

        let message = error_text(fixture.load());
        assert!(message.contains("core.linker_script"));
        assert!(message.contains("not declared in inputs"));
    }

    #[test]
    fn rejects_unsafe_or_duplicate_core_code_tokens() {
        for values in [json!(["kernel", "kernel"]), json!(["kernel;make"])] {
            let fixture = new_fixture();
            let mut document = fixture.document.clone();
            document["core"]["resources"] = values;
            fixture.write_contract(&document);
            assert!(error_text(fixture.load()).contains("core.resources"));
        }
    }

    #[test]
    fn source_make_configuration_preserves_known_empty_and_literal_values() {
        let fixture = new_fixture();
        let mut document = fixture.document.clone();
        document["make_variables"] = json!({"FEATURE_MODE": "", "SDK_STYLE": "native-v1"});
        fixture.write_contract(&document);
        let contract = fixture.load().unwrap();
        assert_eq!(contract.make_variables.get("FEATURE_MODE").unwrap(), "");
        assert_eq!(
            contract.make_variables.get("SDK_STYLE").unwrap(),
            "native-v1"
        );
        assert!(!contract.make_variables.contains_key("UNDECLARED"));
    }

    #[test]
    fn source_host_configuration_is_selected_without_guessing_missing_hosts() {
        let fixture = new_fixture();
        let mut document = fixture.document.clone();
        document["make_variables"] = json!({"FEATURE_MODE": ""});
        document["host_make_variables"] = json!({
            "linux-x86_64": {"HOST_STYLE": "elf"},
            "macos-aarch64": {"HOST_STYLE": "macho"}
        });
        fixture.write_contract(&document);
        let contract = fixture.load().unwrap();
        for (host, expected) in [("linux-x86_64", "elf"), ("macos-aarch64", "macho")] {
            let variables = contract.make_variables_for_host(host).unwrap();
            assert_eq!(variables["HOST_STYLE"], expected);
            assert_eq!(variables["FEATURE_MODE"], "");
            assert!(!variables.contains_key("UNDECLARED"));
        }
        assert!(contract.make_variables_for_host("linux-aarch64").is_err());
        assert!(contract.make_variables_for_host("").is_err());
        assert!(!fixture
            .load()
            .unwrap()
            .make_variables
            .contains_key("HOST_STYLE"));
    }

    #[test]
    fn source_host_configuration_rejects_unsafe_values_and_ambiguous_defaults() {
        for table in [
            json!({"macos-aarch64": {"HOST_STYLE": "$(eval injected: owner)"}}),
            json!({"macos-aarch64": {"CPU": "injected"}}),
            json!({"macos-aarch64": {"HOST_STYLE": "two words"}}),
            json!({"macos-aarch64": {"SHARED": "collision"}}),
            json!({"bad/host": {"HOST_STYLE": "elf"}}),
        ] {
            let fixture = new_fixture();
            let mut document = fixture.document.clone();
            document["make_variables"] = json!({"SHARED": "known"});
            document["host_make_variables"] = table;
            fixture.write_contract(&document);
            assert!(fixture.load().is_err(), "{document}");
        }
        let fixture = new_fixture();
        assert!(fixture.load().unwrap().make_variables_for_host("").is_ok());
    }

    #[test]
    fn source_host_identity_cannot_be_a_shared_default() {
        for name in NATIVE_HOST_MAKE_IDENTITIES {
            let fixture = new_fixture();
            let mut document = fixture.document.clone();
            document["make_variables"] = json!({name.to_string(): "linux"});
            fixture.write_contract(&document);
            assert!(fixture.load().is_err(), "{name}");

            let mut unvalidated =
                serde_json::from_value::<NativeBuildContract>(document.clone()).unwrap();
            assert!(unvalidated
                .make_variables_for_host("macos-aarch64")
                .is_err());
            unvalidated.make_variables.clear();
            unvalidated.host_make_variables.insert(
                "macos-aarch64".into(),
                BTreeMap::from([(name.to_string(), "darwin".into())]),
            );
            assert_eq!(
                unvalidated
                    .make_variables_for_host("macos-aarch64")
                    .unwrap()[*name],
                "darwin"
            );

            document["make_variables"] = json!({});
            document["host_make_variables"] =
                json!({"macos-aarch64": {name.to_string(): "darwin"}});
            fixture.write_contract(&document);
            assert_eq!(
                fixture
                    .load()
                    .unwrap()
                    .make_variables_for_host("macos-aarch64")
                    .unwrap()[*name],
                "darwin"
            );
        }
    }

    #[test]
    fn source_host_configuration_rejects_duplicate_and_unbounded_maps() {
        for text in [
            r#"{"macos-aarch64":{"FLAG":"0"},"macos-aarch64":{"FLAG":"1"}}"#,
            r#"{"macos-aarch64":{"FLAG":"0","FLAG":"1"}}"#,
        ] {
            assert!(
                deserialize_host_make_variables(&mut serde_json::Deserializer::from_str(text))
                    .is_err()
            );
        }
        let hosts = (0..17)
            .map(|index| (format!("host-{index}"), json!({})))
            .collect::<BTreeMap<_, _>>();
        let text = serde_json::to_string(&hosts).unwrap();
        assert!(
            deserialize_host_make_variables(&mut serde_json::Deserializer::from_str(&text))
                .is_err()
        );
        let variables = (0..=MAX_MAKE_VARIABLES)
            .map(|index| (format!("FLAG_{index}"), "0"))
            .collect::<BTreeMap<_, _>>();
        let text = serde_json::to_string(&json!({"macos-aarch64": variables})).unwrap();
        assert!(
            deserialize_host_make_variables(&mut serde_json::Deserializer::from_str(&text))
                .is_err()
        );
    }

    #[test]
    fn source_make_configuration_rejects_syntax_shadowing_and_unbounded_data() {
        for value in [
            json!({"CPU": "riscv"}),
            json!({"AROS_TOOLCHAIN": "gnu"}),
            json!({"lowercase": "1"}),
            json!({"1BAD": "1"}),
            json!({"FEATURE": "$(shell touch bad)"}),
            json!({"FEATURE": "a;b"}),
            json!({"FEATURE": "a b"}),
            json!({"FEATURE": "a\nb"}),
            json!({"FEATURE": 0}),
            json!({"FEATURE": "a".repeat(129)}),
        ] {
            let fixture = new_fixture();
            let mut document = fixture.document.clone();
            document["make_variables"] = value;
            fixture.write_contract(&document);
            assert!(fixture.load().is_err(), "{document}");
        }
        let duplicate = r#"{"FLAG":"0","FLAG":"1"}"#;
        let mut deserializer = serde_json::Deserializer::from_str(duplicate);
        assert!(deserialize_make_variables(&mut deserializer)
            .unwrap_err()
            .to_string()
            .contains("duplicate Make variable"));
        let too_many: BTreeMap<_, _> = (0..=MAX_MAKE_VARIABLES)
            .map(|index| (format!("FLAG_{index}"), "0"))
            .collect();
        let text = serde_json::to_string(&too_many).unwrap();
        assert!(
            deserialize_make_variables(&mut serde_json::Deserializer::from_str(&text)).is_err()
        );
    }

    #[test]
    fn source_make_include_binding_maps_inventoried_source_to_inventoried_projection() {
        let fixture = make_include_fixture();
        let contract = fixture.load().unwrap();
        assert_eq!(
            contract
                .make_include_bindings
                .get("config/aros.cfg")
                .unwrap(),
            "arch/native-config.mk"
        );

        let mut empty = fixture.document.clone();
        empty["make_include_bindings"] = json!({});
        fixture.write_contract(&empty);
        assert!(fixture.load().unwrap().make_include_bindings.is_empty());

        let mut original = fixture.document.clone();
        original["make_include_bindings"] =
            json!({"arch/native-config.mk": "arch/native-config.mk"});
        fixture.write_contract(&original);
        assert_eq!(
            fixture.load().unwrap().make_include_bindings["arch/native-config.mk"],
            "arch/native-config.mk"
        );
    }

    #[test]
    fn generated_make_templates_require_sealed_sources_and_no_competing_binding() {
        let mut fixture = make_include_fixture();
        for (path, contents) in [
            (
                "configure.ac",
                "AC_CONFIG_FILES([gen/include.cfg:config/include.cfg.in])\n",
            ),
            (
                "config/include.cfg.in",
                "%common\nEXECSMP=\"@ENABLE_EXECSMP@\"\n",
            ),
        ] {
            fs::write(fixture.root().join(path), contents).unwrap();
            fixture.document["inputs"]
                .as_array_mut()
                .unwrap()
                .push(json!({
                    "path": path, "sha256": crate::sha256_bytes(contents.as_bytes()),
                }));
        }
        fixture.document["generated_make_templates"] = json!({
            "gen/include.cfg": {
                "template": "config/include.cfg.in",
                "configure_source": "configure.ac",
                "substitutions": {"@ENABLE_EXECSMP@": ""}
            }
        });
        fixture.write_contract(&fixture.document);
        assert!(!fixture.root().join("gen/include.cfg").exists());
        assert_eq!(fixture.load().unwrap().generated_make_templates.len(), 1);
        let valid = fixture.document.clone();
        fixture.document["make_include_bindings"]["GEN/include.cfg"] =
            json!("arch/native-config.mk");
        fixture.write_contract(&fixture.document);
        assert!(fixture.load().is_err());
        fixture.document = valid;
        fixture.document["inputs"]
            .as_array_mut()
            .unwrap()
            .retain(|input| input["path"] != "configure.ac");
        fixture.write_contract(&fixture.document);
        assert!(fixture.load().is_err());
    }

    #[test]
    fn source_make_include_bindings_reject_uninventoried_paths_and_unsafe_mappings() {
        let fixture = make_include_fixture();

        for (path, is_replacement) in [("config/aros.cfg", false), ("arch/native-config.mk", true)]
        {
            let mut document = fixture.document.clone();
            document["inputs"]
                .as_array_mut()
                .unwrap()
                .retain(|input| input["path"] != path);
            fixture.write_contract(&document);
            let error = error_text(fixture.load());
            let expected = if is_replacement {
                "make_include_bindings replacement"
            } else {
                "make_include_bindings key"
            };
            assert!(error.contains(expected), "{path}: {error}");
            assert!(error.contains("not declared in inputs"), "{path}: {error}");
        }

        for (replacement, expected) in [
            ("../outside.mk", "unsafe"),
            ("arch/*.mk", "unsafe"),
            ("arch/foreign.mk", "not declared in inputs"),
            ("kernel/linker.lds", "must name a .mk file"),
        ] {
            let mut document = fixture.document.clone();
            document["make_include_bindings"] = json!({"config/aros.cfg": replacement});
            fixture.write_contract(&document);
            let error = error_text(fixture.load());
            assert!(error.contains(expected), "{replacement}: {error}");
        }

        let mut duplicate_replacement = fixture.document.clone();
        duplicate_replacement["make_include_bindings"] = json!({
            "config/aros.cfg": "arch/native-config.mk",
            "config/secondary.cfg": "arch/native-config.mk"
        });
        fixture.write_contract(&duplicate_replacement);
        assert!(error_text(fixture.load()).contains("duplicate replacement"));

        let mut null_bindings = fixture.document.clone();
        null_bindings["make_include_bindings"] = Value::Null;
        fixture.write_contract(&null_bindings);
        assert!(fixture.load().is_err());

        let mut null_replacement = fixture.document.clone();
        null_replacement["make_include_bindings"] = json!({"config/aros.cfg": Value::Null});
        fixture.write_contract(&null_replacement);
        assert!(fixture.load().is_err());

        let duplicate = r#"{"config/aros.cfg":"arch/first.mk","config/aros.cfg":"arch/second.mk"}"#;
        let mut deserializer = serde_json::Deserializer::from_str(duplicate);
        assert!(deserialize_make_include_bindings(&mut deserializer)
            .unwrap_err()
            .to_string()
            .contains("duplicate Make include binding"));

        let too_many: BTreeMap<_, _> = (0..=MAX_NATIVE_MAKE_INCLUDE_BINDINGS)
            .map(|index| {
                (
                    format!("config/include-{index}.cfg"),
                    format!("arch/projection-{index}.mk"),
                )
            })
            .collect();
        let text = serde_json::to_string(&too_many).unwrap();
        assert!(
            deserialize_make_include_bindings(&mut serde_json::Deserializer::from_str(&text))
                .is_err()
        );
    }

    #[test]
    fn source_optional_meta_dependencies_load_only_selector_edges_on_recipes() {
        let fixture = new_fixture();
        assert!(fixture
            .load()
            .unwrap()
            .optional_meta_dependencies
            .is_empty());

        let mut document = fixture.document.clone();
        document["optional_meta_dependencies"] = json!([
            {
                "recipe": "kernel/makefile.src",
                "target": "optional-meta+native",
                "dependency": "lib${AROS_TARGET_CPU}-${AROS_TARGET_PLATFORM}"
            },
            {
                "recipe": "kernel/mmakefile",
                "target": "optional-meta-family",
                "dependency": "lib${AROS_TARGET_LEGACY_PLATFORM}_${AROS_TARGET_FAMILY}_${AROS_TARGET_VARIANT}_${AROS_TARGET_CPU32}"
            }
        ]);
        fixture.write_contract(&document);

        let loaded = fixture.load().unwrap();
        assert_eq!(loaded.optional_meta_dependencies.len(), 2);
        assert_eq!(
            loaded.optional_meta_dependencies[0],
            NativeOptionalMetaDependency {
                recipe: "kernel/makefile.src".into(),
                target: "optional-meta+native".into(),
                dependency: "lib${AROS_TARGET_CPU}-${AROS_TARGET_PLATFORM}".into(),
                absence: NativeMetaAbsence::Selector,
            }
        );
    }

    #[test]
    fn source_optional_meta_dependencies_reject_unsafe_or_unbounded_edges() {
        for dependency in [
            "liboptional",
            "${AROS_TARGET_VENDOR}",
            "${AROS_TARGET_CPU:foo}",
            "${AROS_TARGET_CPU}/include",
            "$(shell touch file)",
            "${AROS_TARGET_CPU}${OTHER}",
            "${AROS_TARGET_CPU",
            "${AROS_TARGET_CPU}?",
            "$${AROS_TARGET_CPU}",
            "${AROS_TARGET_CPU}|${AROS_TARGET_PLATFORM}",
            "${AROS_TARGET_CPU}\\other",
        ] {
            let fixture = new_fixture();
            let mut document = fixture.document.clone();
            document["optional_meta_dependencies"] = json!([{
                "recipe": "kernel/makefile.src",
                "target": "optional-meta",
                "dependency": dependency
            }]);
            fixture.write_contract(&document);
            let error = error_text(fixture.load());
            assert!(error.contains("dependency"), "{dependency}: {error}");
        }

        for target in ["", "target/name", "target name", "target${AROS_TARGET_CPU}"] {
            let fixture = new_fixture();
            let mut document = fixture.document.clone();
            document["optional_meta_dependencies"] = json!([{
                "recipe": "kernel/makefile.src",
                "target": target,
                "dependency": "lib${AROS_TARGET_CPU}"
            }]);
            fixture.write_contract(&document);
            let error = error_text(fixture.load());
            assert!(error.contains("target"), "{target}: {error}");
        }

        for recipe in [
            "../outside.src",
            "kernel/not-inventoried.src",
            "kernel/linker.lds",
        ] {
            let fixture = new_fixture();
            let mut document = fixture.document.clone();
            document["optional_meta_dependencies"] = json!([{
                "recipe": recipe,
                "target": "optional-meta",
                "dependency": "lib${AROS_TARGET_CPU}"
            }]);
            fixture.write_contract(&document);
            let error = error_text(fixture.load());
            assert!(error.contains("recipe"), "{recipe}: {error}");
        }

        let fixture = new_fixture();
        let valid_edge = json!({
            "recipe": "kernel/makefile.src",
            "target": "optional-meta",
            "dependency": "lib${AROS_TARGET_CPU}"
        });
        let mut duplicate = fixture.document.clone();
        duplicate["optional_meta_dependencies"] = json!([valid_edge.clone(), valid_edge]);
        fixture.write_contract(&duplicate);
        assert!(error_text(fixture.load()).contains("duplicates"));

        let mut too_many = fixture.document.clone();
        too_many["optional_meta_dependencies"] = Value::Array(
            (0..=64)
                .map(|index| {
                    json!({
                        "recipe": "kernel/makefile.src",
                        "target": format!("optional-meta-{index}"),
                        "dependency": "lib${AROS_TARGET_CPU}"
                    })
                })
                .collect(),
        );
        fixture.write_contract(&too_many);
        assert!(error_text(fixture.load()).contains("exceeds 64 entries"));

        let mut switches = fixture.document.clone();
        switches["make_variables"] = Value::Object(
            (0..=MAX_MAKE_VARIABLES)
                .map(|index| (format!("SWITCH_{index}"), Value::String(String::new())))
                .collect(),
        );
        fixture.write_contract(&switches);
        assert!(error_text(fixture.load()).contains("make_variables exceeds 256 entries"));

        let mut null_edges = fixture.document.clone();
        null_edges["optional_meta_dependencies"] = Value::Null;
        fixture.write_contract(&null_edges);
        assert!(fixture.load().is_err());

        assert!(
            serde_json::from_value::<NativeOptionalMetaDependency>(json!({
                "recipe": "kernel/makefile.src",
                "target": "optional-meta",
                "dependency": "lib${AROS_TARGET_CPU}",
                "ignored": true
            }))
            .is_err()
        );
    }

    #[test]
    fn source_optional_disabled_owner_is_explicit_and_literal() {
        let fixture = new_fixture();
        let mut document = fixture.document.clone();
        document["optional_meta_dependencies"] = json!([{
            "recipe": "kernel/makefile.src", "target": "aggregate",
            "dependency": "disabled-pkgconfig", "absence": "disabled-owner"
        }]);
        fixture.write_contract(&document);
        assert_eq!(
            fixture.load().unwrap().optional_meta_dependencies[0].absence,
            NativeMetaAbsence::DisabledOwner
        );
        for invalid in ["${AROS_TARGET_CPU}", "foo/bar", "", "$(shell false)"] {
            document["optional_meta_dependencies"][0]["dependency"] = json!(invalid);
            fixture.write_contract(&document);
            assert!(fixture.load().is_err());
        }
        document["optional_meta_dependencies"][0]["dependency"] = json!("disabled-pkgconfig");
        for invalid in [json!("ignore-missing"), json!(null), json!(false)] {
            document["optional_meta_dependencies"][0]["absence"] = invalid;
            fixture.write_contract(&document);
            assert!(fixture.load().is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn source_optional_meta_dependencies_reject_symlinked_recipes() {
        use std::os::unix::fs::symlink;

        let fixture = new_fixture();
        fs::write(
            fixture.root().join("kernel/uninventoried.src"),
            "synthetic MetaMake recipe\n",
        )
        .unwrap();
        fs::remove_file(fixture.root().join("kernel/mmakefile")).unwrap();
        symlink("uninventoried.src", fixture.root().join("kernel/mmakefile")).unwrap();
        let mut document = fixture.document.clone();
        for input in document["inputs"].as_array_mut().unwrap() {
            if input["path"] == "kernel/mmakefile" {
                input["sha256"] =
                    json!(crate::sha256_bytes(b"synthetic MetaMake recipe\n").to_string());
            }
        }
        document["optional_meta_dependencies"] = json!([{
            "recipe": "kernel/mmakefile",
            "target": "optional-meta",
            "dependency": "lib${AROS_TARGET_CPU}"
        }]);
        fixture.write_contract(&document);

        let error = error_text(fixture.load());
        assert!(error.contains("non-symlink source file"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn source_make_include_bindings_reject_symlinked_original_or_replacement() {
        use std::os::unix::fs::symlink;

        for mapped_path in ["config/aros.cfg", "arch/native-config.mk"] {
            let fixture = make_include_fixture();
            let path = fixture.root().join(mapped_path);
            let target = fixture.root().join("symlink-target.txt");
            fs::rename(&path, &target).unwrap();
            symlink("../symlink-target.txt", &path).unwrap();
            fixture.write_contract(&fixture.document);

            let error = error_text(fixture.load());
            assert!(
                error.contains("non-symlink source file"),
                "{mapped_path}: {error}"
            );
        }
    }
}
