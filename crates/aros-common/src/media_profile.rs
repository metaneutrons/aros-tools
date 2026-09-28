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

/// Select a portable profile for one target without inferring a board from a
/// target preset. A missing ID is allowed only when precisely one profile
/// matches the target, model and transport constraints.
///
/// # Errors
///
/// Returns an error for an unknown ID, an incompatible explicit selection,
/// no matching profile, or an ambiguous implicit selection.
pub fn select_media_profile(
    profiles: &[ResolvedMediaProfile],
    requested_id: Option<&str>,
    target_preset: &str,
    model: Option<&str>,
    transport: Option<&str>,
) -> Result<ResolvedMediaProfile> {
    let mut ids = BTreeSet::new();
    if profiles
        .iter()
        .any(|entry| !ids.insert(entry.profile.id.as_str()))
    {
        return Err(invalid(
            "<media-profile-selection>",
            "duplicate profile ID in media registry",
        ));
    }
    if let Some(id) = requested_id {
        return profiles
            .iter()
            .find(|entry| entry.profile.id == id)
            .filter(|entry| {
                entry.profile.target_preset == target_preset
                    && model.is_none_or(|value| entry.profile.model == value)
                    && transport.is_none_or(|value| entry.profile.transport == value)
            })
            .cloned()
            .ok_or_else(|| {
                invalid(
                    "<media-profile-selection>",
                    &format!(
                        "profile '{id}' is unknown or incompatible with target '{target_preset}'"
                    ),
                )
            });
    }
    let matching: Vec<_> = profiles
        .iter()
        .filter(|entry| {
            entry.profile.target_preset == target_preset
                && model.is_none_or(|value| entry.profile.model == value)
                && transport.is_none_or(|value| entry.profile.transport == value)
        })
        .collect();
    match matching.as_slice() {
        [profile] => Ok((*profile).clone()),
        [] => Err(invalid(
            "<media-profile-selection>",
            &format!("no media profile matches target '{target_preset}'"),
        )),
        _ => Err(invalid(
            "<media-profile-selection>",
            &format!(
                "multiple media profiles match target '{target_preset}'; select a profile ID explicitly"
            ),
        )),
    }
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
    use super::{
        built_in_media_profiles, parse_media_profile, select_media_profile, BUILT_IN_PROFILES,
    };

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

    #[test]
    fn selection_requires_a_profile_id_when_one_target_has_multiple_media() {
        let mut profiles = built_in_media_profiles().expect("built-in profiles");
        let mut second = profiles[0].clone();
        second.profile.id = "rpi4-native-sd".to_string();
        second.profile.transport = "native-sd".to_string();
        profiles.push(second);

        let ambiguous = select_media_profile(&profiles, None, "rpi-aarch64", Some("rpi4"), None)
            .expect_err("target and model do not select a medium");
        assert!(ambiguous
            .to_string()
            .contains("select a profile ID explicitly"));

        let selected = select_media_profile(
            &profiles,
            Some("rpi4-native-sd"),
            "rpi-aarch64",
            Some("rpi4"),
            Some("native-sd"),
        )
        .expect("explicitly selected profile");
        assert_eq!(selected.profile.id, "rpi4-native-sd");

        assert!(select_media_profile(
            &profiles,
            Some("rpi4-native-sd"),
            "arm-raspi",
            Some("rpi4"),
            Some("native-sd")
        )
        .is_err());

        let duplicate = profiles[0].clone();
        profiles.push(duplicate);
        assert!(select_media_profile(
            &profiles,
            Some("rpi4-uboot-usb-ecm"),
            "rpi-aarch64",
            Some("rpi4"),
            Some("uboot-usb-ecm")
        )
        .is_err());
    }
}
