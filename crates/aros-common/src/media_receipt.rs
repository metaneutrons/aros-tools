//! Closed contract for source-dependent media inputs.
//!
//! A CMake target can report built files; a verified legacy v1 boot bundle can
//! only report its measured inputs. These origins must not be conflated. This
//! receipt is an input to a future composer, not proof that a medium boots or
//! that legacy bytes came from a specific build. The composer must separately
//! bind source, toolchain, reviewed profile and locked external inputs.

use crate::media_profile::{valid_target_preset, MediaProfile};
use crate::{casefold_path_key, open_regular_file_nofollow, sha256_reader, Sha256Digest};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const FORMAT_VERSION: u32 = 1;
const KIND: &str = "aros-media-inputs";
const MAX_RECEIPT_BYTES: usize = 1024 * 1024;
const MAX_FILES: usize = 1024;

/// One source-dependent file claimed by the declared receipt origin.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MediaBuildFile {
    pub role: String,
    pub path: String,
    pub sha256: String,
    pub size_bytes: u64,
}

/// Origin of measured files; legacy inputs do not assert CMake provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum MediaReceiptOrigin {
    #[serde(rename = "cmake")]
    Cmake,
    #[serde(rename = "legacy-v1")]
    LegacyV1,
}

/// Versioned media input inventory, intentionally excluding final media identity.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MediaBuildReceipt {
    pub format_version: u32,
    pub kind: String,
    pub origin: MediaReceiptOrigin,
    /// For legacy inputs this is the selected profile's expected target, not
    /// independent evidence of the build that produced those inputs.
    pub target_preset: String,
    pub model: String,
    pub transport: String,
    pub files: Vec<MediaBuildFile>,
}

impl MediaBuildReceipt {
    /// Construct a checked v1 receipt from an existing verified producer.
    ///
    /// # Errors
    ///
    /// Returns an error when identity, file paths, roles or digests violate
    /// the closed receipt contract.
    pub fn new(
        origin: MediaReceiptOrigin,
        target_preset: String,
        model: String,
        transport: String,
        files: Vec<MediaBuildFile>,
    ) -> Result<Self, MediaReceiptError> {
        let receipt = Self {
            format_version: FORMAT_VERSION,
            kind: KIND.to_string(),
            origin,
            target_preset,
            model,
            transport,
            files,
        };
        validate_receipt(&receipt)?;
        Ok(receipt)
    }
}

/// Fail-closed receipt parsing and measurement error.
#[derive(Debug, thiserror::Error)]
pub enum MediaReceiptError {
    #[error("invalid media build receipt: {0}")]
    Invalid(String),
    #[error("cannot read a declared media build file: {0}")]
    Io(#[from] std::io::Error),
}

/// Parse a bounded, closed JSON receipt without touching the build tree.
///
/// # Errors
///
/// Returns an error for an oversized document, unknown field, unsupported
/// version, unsafe path, duplicate role/path or malformed digest.
pub fn parse_media_build_receipt(bytes: &[u8]) -> Result<MediaBuildReceipt, MediaReceiptError> {
    if bytes.len() > MAX_RECEIPT_BYTES {
        return Err(invalid("receipt exceeds its one-megabyte limit"));
    }
    let receipt: MediaBuildReceipt = serde_json::from_slice(bytes)
        .map_err(|error| invalid(&format!("JSON schema error: {error}")))?;
    validate_receipt(&receipt)?;
    Ok(receipt)
}

/// Require that a typed receipt is compatible with the selected reviewed
/// profile, then remeasure every declared regular build file through a
/// no-follow descriptor.
///
/// # Errors
///
/// Returns an error for an incompatible profile, missing required role,
/// unsafe file, changed size or changed SHA-256. A successful result is a
/// point-in-time observation; composition must revalidate before publication.
pub fn verify_media_build_receipt(
    root: &Path,
    receipt: &MediaBuildReceipt,
    profile: &MediaProfile,
) -> Result<(), MediaReceiptError> {
    validate_receipt(receipt)?;
    if receipt.target_preset != profile.target_preset
        || receipt.model != profile.model
        || receipt.transport != profile.transport
    {
        return Err(invalid("receipt does not match the selected media profile"));
    }
    let actual_roles: BTreeSet<&str> = receipt
        .files
        .iter()
        .map(|file| file.role.as_str())
        .collect();
    for required in &profile.required_files {
        if receipt.origin == MediaReceiptOrigin::Cmake && required.external_input.is_some() {
            if actual_roles.contains(required.role.as_str()) {
                return Err(invalid(&format!(
                    "CMake receipt must not claim external role '{}'",
                    required.role
                )));
            }
            continue;
        }
        if !actual_roles.contains(required.role.as_str()) {
            return Err(invalid(&format!(
                "missing required build role '{}'",
                required.role
            )));
        }
    }

    let root = root.canonicalize()?;
    if !root.is_dir() {
        return Err(invalid("media build root is not a directory"));
    }
    for declared in &receipt.files {
        let path = root.join(&declared.path);
        let mut file = open_regular_file_nofollow(&path)?;
        let actual = sha256_reader(&mut file)?;
        let expected = Sha256Digest::parse(&declared.sha256)
            .map_err(|_| invalid("file.sha256 is not a SHA-256 digest"))?;
        if actual.size != declared.size_bytes || actual.digest != expected {
            return Err(invalid(&format!(
                "measured size or SHA-256 differs for role '{}'",
                declared.role
            )));
        }
    }
    Ok(())
}

fn validate_receipt(receipt: &MediaBuildReceipt) -> Result<(), MediaReceiptError> {
    let encoded_size = serde_json::to_vec(receipt)
        .map_err(|_| invalid("receipt cannot be serialized"))?
        .len();
    if encoded_size > MAX_RECEIPT_BYTES {
        return Err(invalid("receipt exceeds its one-megabyte limit"));
    }
    if receipt.format_version != FORMAT_VERSION || receipt.kind != KIND {
        return Err(invalid("unsupported format_version or kind"));
    }
    if !valid_target_preset(&receipt.target_preset) {
        return Err(invalid("target_preset must be a portable target name"));
    }
    for (label, value) in [
        ("model", receipt.model.as_str()),
        ("transport", receipt.transport.as_str()),
    ] {
        if !valid_slug(value) {
            return Err(invalid(&format!("{label} must be a lowercase slug")));
        }
    }
    if receipt.files.is_empty() || receipt.files.len() > MAX_FILES {
        return Err(invalid("files must contain between one and 1024 entries"));
    }
    let mut roles = BTreeSet::new();
    let mut paths = BTreeSet::new();
    for file in &receipt.files {
        if !valid_slug(&file.role) || !roles.insert(&file.role) {
            return Err(invalid("file role is invalid or duplicated"));
        }
        if file.path.is_empty()
            || file.path.contains('\\')
            || file
                .path
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return Err(invalid("file path is not a safe portable relative path"));
        }
        let path = PathBuf::from(&file.path);
        let folded = casefold_path_key(&path)
            .map_err(|_| invalid("file path is not portable across supported hosts"))?;
        if !paths.insert(folded) {
            return Err(invalid("file path is duplicated after case folding"));
        }
        Sha256Digest::parse(&file.sha256)
            .map_err(|_| invalid("file.sha256 is not a SHA-256 digest"))?;
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

fn invalid(message: &str) -> MediaReceiptError {
    MediaReceiptError::Invalid(message.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        parse_media_build_receipt, verify_media_build_receipt, MediaBuildFile, MediaBuildReceipt,
        MediaReceiptOrigin, KIND,
    };
    use crate::media_profile::{built_in_media_profiles, parse_media_profile};
    use crate::sha256_bytes;
    use std::fs;

    fn fixture() -> (tempfile::TempDir, Vec<u8>) {
        let root = tempfile::tempdir().expect("build root");
        fs::write(root.path().join("loader.efi"), b"loader bytes").expect("build file");
        let digest = sha256_bytes(b"loader bytes");
        let receipt = format!(
            r#"{{"format_version":1,"kind":"{KIND}","origin":"cmake","target_preset":"opensbi-riscv64","model":"milk-v-titan","transport":"uefi-esp","files":[{{"role":"uefi-loader","path":"loader.efi","sha256":"{digest}","size_bytes":12}}]}}"#
        );
        (root, receipt.into_bytes())
    }

    #[test]
    fn parser_rejects_unknown_fields_unsafe_paths_and_duplicate_roles() {
        let (_, bytes) = fixture();
        let text = String::from_utf8(bytes).expect("fixture UTF-8");
        assert!(parse_media_build_receipt(
            text.replace("\"files\":", "\"command\":\"sh\",\"files\":")
                .as_bytes()
        )
        .is_err());
        assert!(parse_media_build_receipt(
            text.replace("\"origin\":\"cmake\"", "\"origin\":\"unknown\"")
                .as_bytes()
        )
        .is_err());
        assert!(
            parse_media_build_receipt(text.replace("opensbi-riscv64", "pc-x86_64").as_bytes())
                .is_ok()
        );
        assert!(parse_media_build_receipt(
            text.replace("opensbi-riscv64", "-pc-x86_64").as_bytes()
        )
        .is_err());
        assert!(
            parse_media_build_receipt(text.replace("loader.efi", "../loader.efi").as_bytes())
                .is_err()
        );
        assert!(parse_media_build_receipt(
            text.replace("\"role\":\"uefi-loader\"", "\"role\":\"bad/role\"")
                .as_bytes()
        )
        .is_err());
        assert!(parse_media_build_receipt(
            text.replace("\"role\":\"uefi-loader\"", "\"role\":\"-bad\"")
                .as_bytes()
        )
        .is_err());
        let mut duplicate: serde_json::Value = serde_json::from_str(&text).expect("fixture JSON");
        let first = duplicate["files"][0].clone();
        duplicate["files"]
            .as_array_mut()
            .expect("files array")
            .push(first);
        assert!(parse_media_build_receipt(&serde_json::to_vec(&duplicate).expect("JSON")).is_err());

        duplicate["files"][1]["role"] = serde_json::json!("second-role");
        duplicate["files"][1]["path"] = serde_json::json!("LOADER.EFI");
        assert!(parse_media_build_receipt(&serde_json::to_vec(&duplicate).expect("JSON")).is_err());
        assert!(parse_media_build_receipt(&vec![b'a'; 1024 * 1024 + 1]).is_err());
        assert!(MediaBuildReceipt::new(
            MediaReceiptOrigin::Cmake,
            "opensbi-riscv64".to_string(),
            "milk-v-titan".to_string(),
            "uefi-esp".to_string(),
            vec![MediaBuildFile {
                role: "uefi-loader".to_string(),
                path: "a".repeat(1024 * 1024),
                sha256: sha256_bytes(b"loader bytes").to_string(),
                size_bytes: 12,
            }],
        )
        .is_err());
    }

    #[test]
    fn verification_rejects_missing_required_role_and_modified_file() {
        let (root, bytes) = fixture();
        let receipt = parse_media_build_receipt(&bytes).expect("valid receipt");
        let profile = built_in_media_profiles()
            .expect("profiles")
            .into_iter()
            .find(|entry| entry.profile.model == "milk-v-titan")
            .expect("Titan profile");
        assert!(verify_media_build_receipt(root.path(), &receipt, &profile.profile).is_err());

        let mut one_role = profile.profile;
        one_role
            .required_files
            .retain(|file| file.role == "uefi-loader");
        verify_media_build_receipt(root.path(), &receipt, &one_role).expect("measured file");
        fs::write(root.path().join("loader.efi"), b"changed bytes").expect("tamper");
        assert!(verify_media_build_receipt(root.path(), &receipt, &one_role).is_err());
    }

    #[test]
    fn cmake_receipt_only_claims_built_roles_and_legacy_inventory_stays_compatible() {
        let profile = format!(
            r#"format_version = 1
id = "test-native-sd"
target_preset = "rpi-aarch64"
model = "rpi5"
transport = "native-sd"
medium = "mbr-fat32"
boot_protocol = "pi-firmware"
label = "Test only"

[[external_locks]]
id = "firmware-test"
sha256 = "{}"

[[required_files]]
role = "kernel-image"
destination = "kernel8.img"

[[required_files]]
role = "firmware-start"
destination = "start4.elf"
external_input = {{ lock_id = "firmware-test", file_id = "firmware-start" }}
"#,
            "0".repeat(64)
        );
        let profile = parse_media_profile("test", &profile).expect("profile");
        let root = tempfile::tempdir().expect("root");
        fs::write(root.path().join("kernel8.img"), b"kernel").expect("kernel");
        fs::write(root.path().join("start4.elf"), b"firmware").expect("firmware");
        let built = MediaBuildFile {
            role: "kernel-image".to_string(),
            path: "kernel8.img".to_string(),
            sha256: sha256_bytes(b"kernel").to_string(),
            size_bytes: 6,
        };
        let external = MediaBuildFile {
            role: "firmware-start".to_string(),
            path: "start4.elf".to_string(),
            sha256: sha256_bytes(b"firmware").to_string(),
            size_bytes: 8,
        };
        let cmake = MediaBuildReceipt::new(
            MediaReceiptOrigin::Cmake,
            "rpi-aarch64".to_string(),
            "rpi5".to_string(),
            "native-sd".to_string(),
            vec![built.clone()],
        )
        .expect("CMake receipt");
        verify_media_build_receipt(root.path(), &cmake, &profile.profile)
            .expect("CMake only claims built files");

        let mut false_claim = cmake;
        false_claim.files.push(external.clone());
        assert!(verify_media_build_receipt(root.path(), &false_claim, &profile.profile).is_err());

        let legacy = MediaBuildReceipt::new(
            MediaReceiptOrigin::LegacyV1,
            "rpi-aarch64".to_string(),
            "rpi5".to_string(),
            "native-sd".to_string(),
            vec![built, external],
        )
        .expect("legacy inventory");
        verify_media_build_receipt(root.path(), &legacy, &profile.profile)
            .expect("legacy input measurement remains supported");
    }

    #[cfg(unix)]
    #[test]
    fn verification_rejects_a_symlinked_file_or_parent() {
        use std::os::unix::fs::symlink;

        let (root, bytes) = fixture();
        let receipt = parse_media_build_receipt(&bytes).expect("valid receipt");
        let mut profile = built_in_media_profiles()
            .expect("profiles")
            .into_iter()
            .find(|entry| entry.profile.model == "milk-v-titan")
            .expect("Titan profile")
            .profile;
        profile
            .required_files
            .retain(|file| file.role == "uefi-loader");

        fs::rename(root.path().join("loader.efi"), root.path().join("real.efi"))
            .expect("move real file");
        symlink("real.efi", root.path().join("loader.efi")).expect("create symlink");
        assert!(verify_media_build_receipt(root.path(), &receipt, &profile).is_err());

        let parent = root.path().join("artifact");
        symlink(root.path(), &parent).expect("create parent symlink");
        let mut parent_receipt = receipt;
        parent_receipt.files[0].path = "artifact/real.efi".to_string();
        assert!(verify_media_build_receipt(root.path(), &parent_receipt, &profile).is_err());
    }
}
