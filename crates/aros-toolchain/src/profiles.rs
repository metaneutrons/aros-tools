//! Closed parser for the producer-owned target profile matrix.
//!
//! Profiles remain authoritative in `aros-toolchains`; this module validates
//! and selects their committed bytes without duplicating a profile recipe in
//! the CLI or native lifecycle.

use std::collections::BTreeSet;

use serde::Deserialize;

use crate::recipe::GitObjectId;
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

/// Validated profiles-v1 document.
#[derive(Debug, Clone)]
pub struct Profiles(ProfileDocument);

/// One selected, producer-defined target profile.
#[derive(Debug, Clone)]
pub struct Profile {
    name: String,
    configure_target: String,
    upstream_output_target: String,
    target_triple: String,
    cpu: String,
    platform: String,
    float_abi: String,
    capabilities: Vec<String>,
}

impl Profile {
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
}

impl Profiles {
    /// Parse a bounded closed profiles-v1 document without filesystem access.
    ///
    /// # Errors
    ///
    /// Returns AX0101 for duplicate fields, unknown fields, unsupported schema
    /// or ambiguous profile/capability identifiers.
    pub fn parse(input: &[u8]) -> Result<Self, ContractError> {
        if input.len() > crate::canonical::MAX_DOCUMENT_BYTES {
            return Err(ContractError::invalid("profiles document exceeds 1 MiB"));
        }
        let profiles: ProfileDocument = serde_json::from_slice(input).map_err(|_| {
            ContractError::invalid("invalid or unsupported closed profiles-v1 document")
        })?;
        validate(&profiles)?;
        Ok(Self(profiles))
    }

    /// Profiles matrix's declared upstream compatibility reference.
    #[must_use]
    pub const fn upstream_commit(&self) -> &GitObjectId {
        &self.0.upstream_commit
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

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileDocument {
    schema: String,
    upstream_commit: GitObjectId,
    profiles: Vec<Profile>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileRecord {
    name: String,
    configure_target: String,
    upstream_output_target: String,
    target_triple: String,
    cpu: String,
    platform: String,
    float_abi: String,
    capabilities: Vec<String>,
}

impl<'de> Deserialize<'de> for Profile {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let profile = ProfileRecord::deserialize(deserializer)?;
        Ok(Self {
            name: profile.name,
            configure_target: profile.configure_target,
            upstream_output_target: profile.upstream_output_target,
            target_triple: profile.target_triple,
            cpu: profile.cpu,
            platform: profile.platform,
            float_abi: profile.float_abi,
            capabilities: profile.capabilities,
        })
    }
}

fn validate(profiles: &ProfileDocument) -> Result<(), ContractError> {
    if profiles.schema != "aros-toolchain-profiles-v1"
        || profiles.profiles.is_empty()
        || profiles.profiles.len() > MAX_PROFILES
    {
        return Err(ContractError::invalid(
            "expected profiles-v1 with 1..128 entries",
        ));
    }
    let mut names = BTreeSet::new();
    for profile in &profiles.profiles {
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
            || profile.capabilities.iter().any(|value| {
                !identifier(value) || !SUPPORTED_CAPABILITIES.contains(&value.as_str())
            })
            || profile.capabilities.iter().collect::<BTreeSet<_>>().len()
                != profile.capabilities.len()
        {
            return Err(ContractError::invalid(
                "profiles contain duplicate or invalid identifiers/capabilities",
            ));
        }
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
