//! Reviewed, portable media layouts. A local board name is not a media identity.
//!
//! This module validates the first two existing SD layouts. The registry can
//! grow independently of target presets and local board configuration; it does
//! not execute profile content or claim that a board has passed boot testing.

use crate::{sha256_bytes, ArosError, Result, Sha256Digest};
use serde::Deserialize;
use std::collections::BTreeSet;

const FORMAT_VERSION: u32 = 1;
const TREE_FORMAT_VERSION: u32 = 2;
const BUILT_IN_PROFILES: [(&str, &str); 3] = [
    (
        "rpi4-uboot-usb-ecm.toml",
        include_str!("../../../profiles/media/rpi4-uboot-usb-ecm.toml"),
    ),
    (
        "milk-v-titan-uefi.toml",
        include_str!("../../../profiles/media/milk-v-titan-uefi.toml"),
    ),
    (
        "pc-bios-iso.toml",
        include_str!("../../../profiles/media/pc-bios-iso.toml"),
    ),
];

/// A file role and its exact destination on the boot filesystem.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequiredMediaFile {
    pub role: String,
    pub destination: String,
    /// External bytes must resolve through an exact lock pin, not a build receipt.
    #[serde(default)]
    pub external_input: Option<ExternalMediaInput>,
}

/// Identity of one file in a separately reviewed external-input lock.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalMediaInput {
    pub lock_id: String,
    pub file_id: String,
}

/// Exact raw-byte lock identity required by a media profile.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaExternalLockPin {
    pub id: String,
    pub sha256: String,
}

/// Closed, format-specific media geometry. This never selects a host device.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum MediaLayout {
    MbrFat32 {
        start_lba: u64,
        size_bytes: u64,
        label: String,
    },
    Iso9660ElTorito {
        volume_id: String,
        boot_image_role: String,
        catalog_path: String,
        max_size_bytes: u64,
    },
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
    pub layout: MediaLayout,
    #[serde(default)]
    pub external_locks: Vec<MediaExternalLockPin>,
    pub required_files: Vec<RequiredMediaFile>,
    #[serde(default)]
    pub required_trees: Vec<RequiredMediaTree>,
}

/// One complete build-directory subtree placed under a profile destination.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequiredMediaTree {
    pub role: String,
    /// Empty means the filesystem root; no host path is selected here.
    pub destination: String,
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
    if !matches!(profile.format_version, FORMAT_VERSION | TREE_FORMAT_VERSION) {
        return Err(invalid(source, "unsupported format_version"));
    }
    if (profile.format_version == FORMAT_VERSION && !profile.required_trees.is_empty())
        || (profile.format_version == TREE_FORMAT_VERSION && profile.required_trees.is_empty())
    {
        return Err(invalid(
            source,
            "profile version and tree inventory disagree",
        ));
    }
    for (label, value) in [
        ("id", profile.id.as_str()),
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
    if !valid_target_preset(&profile.target_preset) {
        return Err(invalid(
            source,
            "target_preset must be a portable target name",
        ));
    }
    if profile.label.trim().is_empty() || profile.label.chars().any(char::is_control) {
        return Err(invalid(source, "label must be printable and nonempty"));
    }
    match (&profile.layout, profile.medium.as_str()) {
        (
            MediaLayout::MbrFat32 {
                start_lba,
                size_bytes,
                label,
            },
            "mbr-fat32",
        ) => {
            if *start_lba < 2048
                || !start_lba.is_multiple_of(2048)
                || *size_bytes < 64 * 1024 * 1024
                || !size_bytes.is_multiple_of(512)
                || u32::try_from(*start_lba).is_err()
                || u32::try_from(size_bytes / 512).is_err()
                || start_lba
                    .checked_add(size_bytes / 512)
                    .is_none_or(|end| end > u64::from(u32::MAX) + 1)
                || start_lba
                    .checked_mul(512)
                    .and_then(|start| start.checked_add(*size_bytes))
                    .is_none()
                || label.is_empty()
                || label.len() > 11
                || !label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            {
                return Err(invalid(
                    source,
                    "invalid MBR/FAT32 partition geometry or label",
                ));
            }
        }
        (
            MediaLayout::Iso9660ElTorito {
                volume_id,
                boot_image_role,
                catalog_path,
                max_size_bytes,
            },
            "iso9660-el-torito",
        ) => {
            if *max_size_bytes < 1024 * 1024
                || !max_size_bytes.is_multiple_of(2048)
                || volume_id.is_empty()
                || volume_id.len() > 32
                || !volume_id
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
                || !valid_slug(boot_image_role)
                || !valid_destination(catalog_path)
                || !profile
                    .required_files
                    .iter()
                    .any(|file| file.role == *boot_image_role)
            {
                return Err(invalid(source, "invalid ISO volume ID or El Torito role"));
            }
        }
        _ => return Err(invalid(source, "medium and layout kind disagree")),
    }
    if profile.required_files.is_empty() && profile.required_trees.is_empty() {
        return Err(invalid(source, "media profile must declare files or trees"));
    }
    let mut roles = BTreeSet::new();
    let mut destinations = BTreeSet::new();
    let mut referenced_locks = BTreeSet::new();
    let mut referenced_files = BTreeSet::new();
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
        if let Some(external) = &file.external_input {
            if !valid_slug(&external.lock_id) || !valid_slug(&external.file_id) {
                return Err(invalid(
                    source,
                    "external lock and file IDs must be lowercase slugs",
                ));
            }
            referenced_locks.insert(external.lock_id.as_str());
            if !referenced_files.insert((&external.lock_id, &external.file_id)) {
                return Err(invalid(
                    source,
                    "external input is mapped to multiple roles",
                ));
            }
        }
    }
    for tree in &profile.required_trees {
        if !valid_slug(&tree.role) || !roles.insert(tree.role.clone()) {
            return Err(invalid(
                source,
                "required tree role is invalid or duplicated",
            ));
        }
        if !tree.destination.is_empty() && !valid_destination(&tree.destination) {
            return Err(invalid(source, "required tree destination is unsafe"));
        }
        if !tree.destination.is_empty()
            && destinations
                .iter()
                .any(|path| path == &tree.destination.to_ascii_lowercase())
        {
            return Err(invalid(
                source,
                "required tree destination conflicts with a file",
            ));
        }
    }
    for destination in &destinations {
        let mut parent = destination.as_str();
        while let Some((prefix, _)) = parent.rsplit_once('/') {
            if destinations.contains(prefix) {
                return Err(invalid(
                    source,
                    &format!("file destination '{prefix}' is a parent of another file"),
                ));
            }
            parent = prefix;
        }
    }
    if let MediaLayout::Iso9660ElTorito { catalog_path, .. } = &profile.layout {
        if destinations
            .iter()
            .any(|path| path.starts_with(&format!("{}/", catalog_path.to_ascii_lowercase())))
        {
            return Err(invalid(
                source,
                "ISO input is nested beneath the generated catalog",
            ));
        }
        let mut path = catalog_path.to_ascii_lowercase();
        loop {
            if destinations.contains(&path) {
                return Err(invalid(
                    source,
                    "ISO profile conflicts with its generated boot catalog",
                ));
            }
            let Some((parent, _)) = path.rsplit_once('/') else {
                break;
            };
            path = parent.to_string();
        }
    }
    let mut pinned_locks = BTreeSet::new();
    for pin in &profile.external_locks {
        if !valid_slug(&pin.id) || !pinned_locks.insert(pin.id.as_str()) {
            return Err(invalid(source, "external lock ID is invalid or duplicated"));
        }
        Sha256Digest::parse(&pin.sha256)
            .map_err(|_| invalid(source, "external lock SHA-256 is malformed"))?;
    }
    if pinned_locks != referenced_locks {
        return Err(invalid(
            source,
            "external locks and referenced file roles differ",
        ));
    }
    Ok(())
}

pub(crate) fn valid_slug(value: &str) -> bool {
    !value.is_empty()
        && value.starts_with(|character: char| {
            character.is_ascii_lowercase() || character.is_ascii_digit()
        })
        && value.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
}

pub(crate) fn valid_target_preset(value: &str) -> bool {
    !value.is_empty()
        && value.starts_with(|character: char| {
            character.is_ascii_lowercase() || character.is_ascii_digit()
        })
        && value.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || character == '-'
                || character == '_'
        })
}

pub(crate) fn valid_destination(value: &str) -> bool {
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
        built_in_media_profiles, parse_media_profile, select_media_profile, MediaLayout,
        BUILT_IN_PROFILES,
    };

    #[test]
    fn built_in_registry_has_distinct_model_bound_layouts() {
        let profiles = built_in_media_profiles().expect("valid built-in profiles");
        assert_eq!(profiles.len(), 3);
        assert!(profiles.iter().any(|entry| entry.profile.model == "rpi4"));
        assert!(profiles
            .iter()
            .any(|entry| entry.profile.model == "milk-v-titan"));
        assert!(profiles.iter().any(|entry| {
            entry.profile.id == "pc-bios-iso"
                && entry.profile.format_version == 2
                && entry.profile.required_trees.len() == 1
        }));
        assert_ne!(profiles[0].sha256, profiles[1].sha256);
        for profile in profiles
            .iter()
            .filter(|entry| entry.profile.medium == "mbr-fat32")
        {
            assert!(matches!(
                profile.profile.layout,
                MediaLayout::MbrFat32 {
                    start_lba: 2048,
                    size_bytes: 67_108_864,
                    ..
                }
            ));
        }
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

    #[test]
    fn schema_fixtures_accept_pc_and_distinct_pi_targets_without_registering_them() {
        for (name, text, target, model) in [
            (
                "pc-bios-iso.schema-fixture.toml",
                include_str!("../tests/fixtures/media/pc-bios-iso.schema-fixture.toml"),
                "pc-x86_64",
                "pc",
            ),
            (
                "rpi3-native-sd.schema-fixture.toml",
                include_str!("../tests/fixtures/media/rpi3-native-sd.schema-fixture.toml"),
                "arm-raspi",
                "rpi3",
            ),
            (
                "rpi5-native-sd.schema-fixture.toml",
                include_str!("../tests/fixtures/media/rpi5-native-sd.schema-fixture.toml"),
                "rpi-aarch64",
                "rpi5",
            ),
        ] {
            let selected = parse_media_profile(name, text).expect("valid schema fixture");
            assert_eq!(selected.profile.target_preset, target);
            assert_eq!(selected.profile.model, model);
            assert!(select_media_profile(
                std::slice::from_ref(&selected),
                Some(&selected.profile.id),
                target,
                Some(model),
                Some(&selected.profile.transport),
            )
            .is_ok());
            assert!(
                select_media_profile(&[selected], None, "wrong-target", Some(model), None,)
                    .is_err()
            );
        }
    }

    #[test]
    fn layout_schema_rejects_mismatched_formats_bad_geometry_and_unknown_fields() {
        let pi = BUILT_IN_PROFILES[0].1;
        for invalid in [
            pi.replace("start_lba = 2048", "start_lba = 1"),
            pi.replace("start_lba = 2048", "start_lba = 4294967296"),
            pi.replace("size_bytes = 67108864", "size_bytes = 1024"),
            pi.replace("size_bytes = 67108864", "size_bytes = 2199023255552"),
            pi.replace("label = \"AROSBOOT\"", "label = \"TOO-LONG-LABEL\""),
            pi.replace("kind = \"mbr-fat32\"", "kind = \"iso9660-el-torito\""),
            pi.replace("destination = \"config.txt\"", "destination = \"EFI\"")
                .replace(
                    "destination = \"start4.elf\"",
                    "destination = \"EFI/BOOT/start4.elf\"",
                ),
            pi.replace(
                "kind = \"mbr-fat32\"",
                "kind = \"mbr-fat32\"\ncommand = \"sh\"",
            ),
        ] {
            assert!(parse_media_profile("bad SD layout", &invalid).is_err());
        }
        let iso = include_str!("../tests/fixtures/media/pc-bios-iso.schema-fixture.toml");
        for invalid in [
            iso.replace("volume_id = \"AROSLIVE\"", "volume_id = \"bad volume\""),
            iso.replace("max_size_bytes = 536870912", "max_size_bytes = 123"),
            iso.replace(
                "catalog_path = \"boot/grub/boot.catalog\"",
                "catalog_path = \"../escape\"",
            ),
            iso.replace(
                "boot_image_role = \"grub-boot-image\"",
                "boot_image_role = \"missing-role\"",
            ),
            iso.replace("kind = \"iso9660-el-torito\"", "kind = \"mbr-fat32\""),
        ] {
            assert!(parse_media_profile("bad ISO layout", &invalid).is_err());
        }
    }
}
