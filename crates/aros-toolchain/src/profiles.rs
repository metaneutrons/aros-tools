//! Closed parser for the producer-owned target profile matrix.
//!
//! Profiles remain authoritative in `aros-toolchains`; this module validates
//! and selects their committed bytes without duplicating a profile recipe in
//! the CLI or native lifecycle.

use std::collections::BTreeSet;

use serde::Deserialize;

use aros_common::elf::riscv::TargetContract;
use aros_common::{sha256_bytes, Sha256Digest};

use crate::recipe::GitObjectId;
use crate::source_lock::CompilerFamily;
use crate::ContractError;

const MAX_PROFILES: usize = 128;
const SUPPORTED_CAPABILITIES: &[&str] = &[
    "c",
    "cxx",
    "objc",
    "compiler-rt",
    "compiler-rt32",
    "libcxx",
    "libcxxabi",
    "libunwind",
    "multilib-collector",
    "standalone-collector",
];
const SUPPORTED_GNU_CAPABILITIES: &[&str] = &[
    "c",
    "cxx",
    "libgcc",
    "libstdcxx",
    "libsupcxx",
    "standalone-collector",
];

/// Validated profiles-v1 or profiles-v2 document.
#[derive(Debug, Clone)]
pub struct Profiles(ProfileDocument);

/// One selected, producer-defined target profile.
#[derive(Debug, Clone)]
pub struct Profile {
    family: CompilerFamily,
    document_sha256: Sha256Digest,
    name: String,
    configure_target: String,
    upstream_output_target: String,
    target_triple: String,
    cpu: String,
    platform: String,
    float_abi: String,
    capabilities: Vec<String>,
    target: Option<TargetContract>,
}

impl Profile {
    /// Digest of the exact enclosing profiles bytes, retained across clones.
    #[must_use]
    pub const fn document_sha256(&self) -> &Sha256Digest {
        &self.document_sha256
    }
    /// Compiler family declared by the enclosing profiles-v2 document.
    #[must_use]
    pub const fn family(&self) -> CompilerFamily {
        self.family
    }

    /// Producer-owned profile name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Exact target argument used by the later upstream configure phase.
    #[must_use]
    pub fn configure_target(&self) -> &str {
        &self.configure_target
    }

    /// Exact upstream output selector used by later verification phases.
    #[must_use]
    pub fn upstream_output_target(&self) -> &str {
        &self.upstream_output_target
    }

    /// AROS target triple.
    #[must_use]
    pub fn target_triple(&self) -> &str {
        &self.target_triple
    }

    /// Target CPU selector.
    #[must_use]
    pub fn cpu(&self) -> &str {
        &self.cpu
    }

    /// Target platform selector.
    #[must_use]
    pub fn platform(&self) -> &str {
        &self.platform
    }

    /// Profile-specific floating-point ABI, or an empty explicit value.
    #[must_use]
    pub fn float_abi(&self) -> &str {
        &self.float_abi
    }

    /// Closed, unique capability declarations in producer order.
    #[must_use]
    pub fn capabilities(&self) -> &[String] {
        &self.capabilities
    }

    /// Embedded RISC-V target contract for a GNU profiles-v2 entry.
    #[must_use]
    pub const fn target(&self) -> Option<&TargetContract> {
        self.target.as_ref()
    }
}

impl Profiles {
    /// Parse a bounded closed profiles-v1 or profiles-v2 document without filesystem access.
    ///
    /// # Errors
    ///
    /// Returns AX0101 for duplicate fields, unknown fields, unsupported schema
    /// or ambiguous profile/capability identifiers.
    pub fn parse(input: &[u8]) -> Result<Self, ContractError> {
        if input.len() > crate::canonical::MAX_DOCUMENT_BYTES {
            return Err(ContractError::invalid("profiles document exceeds 1 MiB"));
        }
        // Keep the legacy v1 deserialization and its failure contract first:
        // v1 remains byte-for-byte closed and always denotes LLVM.
        let digest = sha256_bytes(input);
        let profiles = if let Ok(document) = serde_json::from_slice::<ProfileDocumentV1>(input) {
            parse_v1(document, &digest)?
        } else {
            let document: ProfileDocumentV2 = serde_json::from_slice(input).map_err(|_| {
                ContractError::invalid("invalid or unsupported closed profiles-v1 document")
            })?;
            parse_v2(document, &digest)?
        };
        Ok(Self(profiles))
    }

    /// Profiles matrix's declared upstream compatibility reference.
    #[must_use]
    pub const fn upstream_commit(&self) -> &GitObjectId {
        &self.0.upstream_commit
    }

    /// Compiler family declared by profiles-v2; profiles-v1 is LLVM.
    #[must_use]
    pub const fn family(&self) -> CompilerFamily {
        self.0.family
    }

    /// Select one exact preset. No heuristic aliasing is supported.
    ///
    /// # Errors
    ///
    /// Returns AX0101 when the requested preset is not in the validated
    /// producer-owned matrix.
    pub fn select(&self, name: &str) -> Result<&Profile, ContractError> {
        self.0
            .profiles
            .iter()
            .find(|profile| profile.name == name)
            .ok_or_else(|| {
                ContractError::invalid("preset is not present in the recipe-selected profiles")
            })
    }

    /// All validated profiles in declared order.
    #[must_use]
    pub fn entries(&self) -> &[Profile] {
        &self.0.profiles
    }
}

#[derive(Debug, Clone)]
struct ProfileDocument {
    family: CompilerFamily,
    upstream_commit: GitObjectId,
    profiles: Vec<Profile>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileDocumentV1 {
    schema: String,
    upstream_commit: GitObjectId,
    profiles: Vec<ProfileV1Record>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileDocumentV2 {
    schema: String,
    family: CompilerFamily,
    upstream_commit: GitObjectId,
    profiles: Vec<ProfileV2Record>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileV1Record {
    name: String,
    configure_target: String,
    upstream_output_target: String,
    target_triple: String,
    cpu: String,
    platform: String,
    float_abi: String,
    capabilities: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileV2Record {
    name: String,
    configure_target: String,
    upstream_output_target: String,
    target_triple: String,
    cpu: String,
    platform: String,
    float_abi: String,
    capabilities: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_non_null")]
    target: Option<TargetContract>,
}

fn deserialize_non_null<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

fn parse_v1(
    document: ProfileDocumentV1,
    digest: &Sha256Digest,
) -> Result<ProfileDocument, ContractError> {
    if document.schema != "aros-toolchain-profiles-v1"
        || document.profiles.is_empty()
        || document.profiles.len() > MAX_PROFILES
    {
        return Err(ContractError::invalid(
            "expected profiles-v1 with 1..128 entries",
        ));
    }
    let profiles = document
        .profiles
        .into_iter()
        .map(|profile| Profile {
            family: CompilerFamily::Llvm,
            document_sha256: digest.clone(),
            name: profile.name,
            configure_target: profile.configure_target,
            upstream_output_target: profile.upstream_output_target,
            target_triple: profile.target_triple,
            cpu: profile.cpu,
            platform: profile.platform,
            float_abi: profile.float_abi,
            capabilities: profile.capabilities,
            target: None,
        })
        .collect::<Vec<_>>();
    validate_profiles(&profiles, CompilerFamily::Llvm)?;
    Ok(ProfileDocument {
        family: CompilerFamily::Llvm,
        upstream_commit: document.upstream_commit,
        profiles,
    })
}

fn parse_v2(
    document: ProfileDocumentV2,
    digest: &Sha256Digest,
) -> Result<ProfileDocument, ContractError> {
    if document.schema != "aros-toolchain-profiles-v2"
        || document.profiles.is_empty()
        || document.profiles.len() > MAX_PROFILES
    {
        return Err(ContractError::invalid(
            "expected profiles-v2 with 1..128 entries",
        ));
    }
    let profiles = document
        .profiles
        .into_iter()
        .map(|profile| Profile {
            family: document.family,
            document_sha256: digest.clone(),
            name: profile.name,
            configure_target: profile.configure_target,
            upstream_output_target: profile.upstream_output_target,
            target_triple: profile.target_triple,
            cpu: profile.cpu,
            platform: profile.platform,
            float_abi: profile.float_abi,
            capabilities: profile.capabilities,
            target: profile.target,
        })
        .collect::<Vec<_>>();
    validate_profiles(&profiles, document.family)?;
    Ok(ProfileDocument {
        family: document.family,
        upstream_commit: document.upstream_commit,
        profiles,
    })
}

fn validate_profiles(profiles: &[Profile], family: CompilerFamily) -> Result<(), ContractError> {
    let mut names = BTreeSet::new();
    for profile in profiles {
        let capabilities = match family {
            CompilerFamily::Llvm => SUPPORTED_CAPABILITIES,
            CompilerFamily::Gnu => SUPPORTED_GNU_CAPABILITIES,
        };
        if !names.insert(&profile.name)
            || [
                &profile.name,
                &profile.configure_target,
                &profile.upstream_output_target,
                &profile.target_triple,
                &profile.cpu,
                &profile.platform,
            ]
            .iter()
            .any(|value| !identifier(value))
            || (!profile.float_abi.is_empty() && !identifier(&profile.float_abi))
            || profile.capabilities.is_empty()
            || profile
                .capabilities
                .iter()
                .any(|value| !identifier(value) || !capabilities.contains(&value.as_str()))
            || profile.capabilities.iter().collect::<BTreeSet<_>>().len()
                != profile.capabilities.len()
        {
            return Err(ContractError::invalid(
                "profiles contain duplicate or invalid identifiers/capabilities",
            ));
        }
        match family {
            CompilerFamily::Llvm if profile.target.is_some() => {
                return Err(ContractError::invalid(
                    "LLVM profiles-v2 entries cannot declare a RISC-V target contract",
                ));
            }
            CompilerFamily::Llvm => {}
            CompilerFamily::Gnu => validate_gnu_profile(profile)?,
        }
    }
    Ok(())
}

fn validate_gnu_profile(profile: &Profile) -> Result<(), ContractError> {
    let target = profile.target.as_ref().ok_or_else(|| {
        ContractError::invalid("GNU profiles-v2 entries require a RISC-V target contract")
    })?;
    let cpu_width = match profile.cpu.as_str() {
        "riscv" => "ilp32",
        "riscv64" => "lp64",
        _ => {
            return Err(ContractError::invalid(
                "GNU RISC-V profile cpu must be riscv or riscv64",
            ));
        }
    };
    if !target.abi().starts_with(cpu_width) {
        return Err(ContractError::invalid(
            "GNU profile cpu width differs from its target ABI",
        ));
    }
    if profile.float_abi != target.abi() {
        return Err(ContractError::invalid(
            "GNU profile float_abi must equal its target ABI",
        ));
    }
    let triple_prefix = profile.target_triple.strip_suffix("-aros");
    if triple_prefix.is_none_or(|prefix| {
        !identifier(prefix) || prefix.split('-').next() != Some(profile.cpu.as_str())
    }) {
        return Err(ContractError::invalid(
            "GNU profile target_triple must match its cpu and end in -aros",
        ));
    }

    let has = |capability: &str| profile.capabilities.iter().any(|value| value == capability);
    if !has("c") || !has("libgcc") || !has("standalone-collector") {
        return Err(ContractError::invalid(
            "GNU profiles require c, libgcc and standalone-collector",
        ));
    }
    if has("cxx") != (has("libstdcxx") && has("libsupcxx")) || has("libstdcxx") != has("libsupcxx")
    {
        return Err(ContractError::invalid(
            "GNU C++ capability requires both libstdcxx and libsupcxx",
        ));
    }
    Ok(())
}

pub(crate) fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        && value != "."
        && value != ".."
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::Profiles;

    fn document() -> serde_json::Value {
        json!({
            "schema": "aros-toolchain-profiles-v1",
            "upstream_commit": "a".repeat(40),
            "profiles": [{
                "name": "pc-x86_64", "configure_target": "pc-x86_64",
                "upstream_output_target": "pc-x86_64", "target_triple": "x86_64-unknown-aros",
                "cpu": "x86_64", "platform": "pc", "float_abi": "",
                "capabilities": ["c", "cxx", "standalone-collector"]
            }]
        })
    }

    #[test]
    fn parses_and_selects_one_closed_profile() {
        let profiles = Profiles::parse(&serde_json::to_vec(&document()).unwrap()).unwrap();
        let profile = profiles.select("pc-x86_64").unwrap();
        assert_eq!(profile.target_triple(), "x86_64-unknown-aros");
        assert_eq!(profile.capabilities(), ["c", "cxx", "standalone-collector"]);
        assert_eq!(profiles.upstream_commit().as_str(), "a".repeat(40));
    }

    #[test]
    fn rejects_unknown_or_ambiguous_profile_contracts() {
        let mut duplicate_capability = document();
        duplicate_capability["profiles"][0]["capabilities"] = json!(["c", "c"]);
        assert!(Profiles::parse(&serde_json::to_vec(&duplicate_capability).unwrap()).is_err());

        let mut unknown = document();
        unknown["unexpected"] = json!(true);
        assert!(Profiles::parse(&serde_json::to_vec(&unknown).unwrap()).is_err());

        let mut unsupported_capability = document();
        unsupported_capability["profiles"][0]["capabilities"] = json!(["c", "future-runtime"]);
        assert!(Profiles::parse(&serde_json::to_vec(&unsupported_capability).unwrap()).is_err());
    }
}
