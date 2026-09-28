//! Reviewed, portable media layouts. A local board name is not a media identity.
//!
//! This module validates the first two existing SD layouts. The registry can
//! grow independently of target presets and local board configuration; it does
//! not execute profile content or claim that a board has passed boot testing.

use crate::{sha256_bytes, ArosError, Result, Sha256Digest};
use serde::Deserialize;
use std::collections::BTreeSet;

const FORMAT_VERSION: u32 = 1;
const BUILT_IN_PROFILES: [(&str, &str); 2] = [
    (
        "rpi4-uboot-usb-ecm.toml",
        include_str!("../../../profiles/media/rpi4-uboot-usb-ecm.toml"),
    ),
    (
        "milk-v-titan-uefi.toml",
        include_str!("../../../profiles/media/milk-v-titan-uefi.toml"),
    ),
];

/// A file role and its exact destination on the boot filesystem.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequiredMediaFile {
    pub role: String,
    pub destination: String,
}

/// A portable medium layout, distinct from a target preset or local board.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaProfile {
    pub format_version: u32,
    pub id: String,
    pub target_preset: String,
    pub model: String,
    pub transport: String,
    pub medium: String,
    pub boot_protocol: String,
    pub label: String,
    pub required_files: Vec<RequiredMediaFile>,
}

/// Validated profile bound to the bytes reviewed in this repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedMediaProfile {
    pub profile: MediaProfile,
    pub sha256: Sha256Digest,
}

/// Parse one closed media profile and reject ambiguous or unsafe layouts.
///
/// # Errors
///
/// Returns an error on an unsupported version, unknown field, malformed
/// identity, unsafe path, or duplicate role/destination.
pub fn parse_media_profile(source: &str, text: &str) -> Result<ResolvedMediaProfile> {
    let profile: MediaProfile =
        toml::from_str(text).map_err(|error| invalid(source, &error.to_string()))?;
    validate_profile(source, &profile)?;
    Ok(ResolvedMediaProfile {
        profile,
        sha256: sha256_bytes(text.as_bytes()),
    })
}

/// Load the currently embedded SD profiles without imposing a global mapping
/// between target, model and medium. Future callers select by profile ID.
///
/// # Errors
///
/// Returns an error if an embedded profile is invalid or the registry contains
/// duplicate IDs.
pub fn built_in_media_profiles() -> Result<Vec<ResolvedMediaProfile>> {
    let mut profiles = Vec::with_capacity(BUILT_IN_PROFILES.len());
    let mut ids = BTreeSet::new();
    for (source, text) in BUILT_IN_PROFILES {
        let resolved = parse_media_profile(source, text)?;
        let profile = &resolved.profile;
        if !ids.insert(profile.id.clone()) {
            return Err(invalid(source, "duplicate profile ID in built-in registry"));
        }
        profiles.push(resolved);
    }
    Ok(profiles)
}

fn validate_profile(source: &str, profile: &MediaProfile) -> Result<()> {
    if profile.format_version != FORMAT_VERSION {
        return Err(invalid(source, "unsupported format_version"));
    }
    for (label, value) in [
        ("id", profile.id.as_str()),
        ("target_preset", profile.target_preset.as_str()),
        ("model", profile.model.as_str()),
        ("transport", profile.transport.as_str()),
        ("medium", profile.medium.as_str()),
        ("boot_protocol", profile.boot_protocol.as_str()),
    ] {
        if !valid_slug(value) {
            return Err(invalid(
                source,
                &format!("{label} must be a nonempty lowercase slug"),
            ));
        }
    }
    if profile.label.trim().is_empty() || profile.label.chars().any(char::is_control) {
        return Err(invalid(source, "label must be printable and nonempty"));
    }
    if profile.required_files.is_empty() {
        return Err(invalid(source, "required_files must not be empty"));
    }
    let mut roles = BTreeSet::new();
    let mut destinations = BTreeSet::new();
    for file in &profile.required_files {
        if !valid_slug(&file.role) {
            return Err(invalid(
                source,
                "required_files.role must be a lowercase slug",
            ));
        }
        if !roles.insert(file.role.clone()) {
            return Err(invalid(
                source,
                &format!("duplicate required role '{}'", file.role),
            ));
        }
        if !valid_destination(&file.destination) {
            return Err(invalid(
                source,
                &format!("unsafe destination '{}'", file.destination),
            ));
        }
        let folded = file.destination.to_ascii_lowercase();
        if !destinations.insert(folded) {
            return Err(invalid(
                source,
                &format!("duplicate destination '{}'", file.destination),
            ));
        }
    }
    Ok(())
}

fn valid_slug(value: &str) -> bool {
    !value.is_empty()
        && value.starts_with(|character: char| {
            character.is_ascii_lowercase() || character.is_ascii_digit()
        })
        && value.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
}

fn valid_destination(value: &str) -> bool {
    !value.is_empty()
        && !value.contains('\\')
        && !value.contains(':')
        && value.split('/').all(|part| {
            !part.is_empty() && part != "." && part != ".." && !part.chars().any(char::is_control)
        })
}

fn invalid(file: &str, message: &str) -> ArosError {
    ArosError::MediaProfile {
        file: file.to_string(),
        message: message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{built_in_media_profiles, parse_media_profile, BUILT_IN_PROFILES};

    #[test]
    fn built_in_registry_has_distinct_model_bound_layouts() {
        let profiles = built_in_media_profiles().expect("valid built-in profiles");
        assert_eq!(profiles.len(), 2);
        assert!(profiles.iter().any(|entry| entry.profile.model == "rpi4"));
        assert!(profiles
            .iter()
            .any(|entry| entry.profile.model == "milk-v-titan"));
        assert_ne!(profiles[0].sha256, profiles[1].sha256);
    }

    #[test]
    fn rejects_unknown_fields_and_unsafe_or_ambiguous_destinations() {
        let text = BUILT_IN_PROFILES[0].1;
        let unknown = text.replace("format_version = 1", "format_version = 1\ncommand = \"sh\"");
        assert!(parse_media_profile("fixture", &unknown).is_err());

        let traversal = text.replace(
            "destination = \"config.txt\"",
            "destination = \"../config.txt\"",
        );
        assert!(parse_media_profile("fixture", &traversal).is_err());

        let duplicate = text.replace(
            "destination = \"start4.elf\"",
            "destination = \"CONFIG.TXT\"",
        );
        assert!(parse_media_profile("fixture", &duplicate).is_err());

        let version = text.replace("format_version = 1", "format_version = 2");
        assert!(parse_media_profile("fixture", &version).is_err());
    }
}
