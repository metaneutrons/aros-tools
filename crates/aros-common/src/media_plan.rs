//! Read-only resolution of a reviewed media profile into exact image inputs.
//!
//! This is deliberately not an image builder. A backend must remeasure each
//! source when copying it and independently read back the completed image.

use crate::media_input_lock::{
    bind_profile_media_input_locks, verify_media_locked_input, MediaInputLockError,
};
use crate::media_profile::{MediaLayout, ResolvedMediaProfile};
use crate::media_receipt::{
    parse_media_build_receipt, verify_media_build_receipt, MediaBuildIdentity, MediaReceiptError,
    MediaReceiptOrigin,
};
use crate::{sha256_bytes, Sha256Digest};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Local path for one exact externally locked file. No URL is fetched here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaExternalFile {
    pub lock_id: String,
    pub file_id: String,
    pub path: PathBuf,
}

/// Origin of one planned image file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaPlanFileOrigin {
    Build,
    External { lock_id: String, file_id: String },
}

/// One exact file placement for a future format backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaPlanFile {
    pub role: String,
    pub destination: String,
    pub sha256: Sha256Digest,
    pub size_bytes: u64,
    pub source_path: PathBuf,
    pub origin: MediaPlanFileOrigin,
}

/// Closed input plan. Its source paths are a point-in-time observation only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaImagePlan {
    pub profile_id: String,
    pub profile_sha256: Sha256Digest,
    pub receipt_sha256: Sha256Digest,
    pub receipt_origin: MediaReceiptOrigin,
    pub build_identity: Option<MediaBuildIdentity>,
    pub target_preset: String,
    pub model: String,
    pub transport: String,
    pub medium: String,
    pub layout: MediaLayout,
    pub external_lock_sha256: BTreeMap<String, Sha256Digest>,
    pub files: Vec<MediaPlanFile>,
    pub total_payload_bytes: u64,
    /// Exact raw-image extent for MBR media; ISO size is backend-dependent.
    pub raw_image_size_bytes: Option<u64>,
}

/// A profile, receipt or external-input set was not an exact image plan.
#[derive(Debug, thiserror::Error)]
pub enum MediaPlanError {
    #[error("invalid media composition plan: {0}")]
    Invalid(String),
    #[error(transparent)]
    Receipt(#[from] MediaReceiptError),
    #[error(transparent)]
    External(#[from] MediaInputLockError),
}

/// Verify all declared local inputs and resolve their exact destinations.
///
/// No output path is created or modified. The caller must pass raw lock bytes
/// so each lock is rebound to the digest pinned in the reviewed profile.
/// The returned paths must be revalidated by the eventual composer.
///
/// # Errors
///
/// Rejects a changed receipt/file, missing or extra role, changed lock,
/// missing or extra external file, unsafe input, or overflowing image extent.
pub fn plan_media_image(
    profile: &ResolvedMediaProfile,
    build_root: &Path,
    receipt_bytes: &[u8],
    lock_documents: &[(&str, &[u8])],
    external_files: &[MediaExternalFile],
) -> Result<MediaImagePlan, MediaPlanError> {
    let receipt = parse_media_build_receipt(receipt_bytes)?;
    verify_media_build_receipt(build_root, &receipt, &profile.profile)?;
    let locks = bind_profile_media_input_locks(profile, lock_documents)?;

    let expected_build_roles: BTreeSet<_> = profile
        .profile
        .required_files
        .iter()
        .filter(|file| {
            receipt.origin == MediaReceiptOrigin::LegacyV1 || file.external_input.is_none()
        })
        .map(|file| file.role.as_str())
        .collect();
    let actual_build_roles: BTreeSet<_> = receipt
        .files
        .iter()
        .map(|file| file.role.as_str())
        .collect();
    if actual_build_roles != expected_build_roles {
        return Err(invalid("build receipt has missing or unselected roles"));
    }

    let expected_external: BTreeSet<_> = profile
        .profile
        .required_files
        .iter()
        .filter_map(|file| file.external_input.as_ref())
        .map(|external| (external.lock_id.as_str(), external.file_id.as_str()))
        .collect();
    let mut supplied_external = BTreeMap::new();
    for file in external_files {
        if supplied_external
            .insert((file.lock_id.as_str(), file.file_id.as_str()), &file.path)
            .is_some()
        {
            return Err(invalid("duplicate external file path"));
        }
    }
    if supplied_external.keys().copied().collect::<BTreeSet<_>>() != expected_external {
        return Err(invalid("external file paths differ from selected roles"));
    }

    let receipt_files: BTreeMap<_, _> = receipt
        .files
        .iter()
        .map(|file| (file.role.as_str(), file))
        .collect();
    let mut files = Vec::with_capacity(profile.profile.required_files.len());
    let mut total_payload_bytes = 0_u64;
    for required in &profile.profile.required_files {
        let (sha256, size_bytes, source_path, origin) = if let Some(external) =
            &required.external_input
        {
            let lock = locks
                .iter()
                .find(|lock| lock.lock.id == external.lock_id)
                .ok_or_else(|| invalid("selected external lock is missing"))?;
            let input = lock
                .lock
                .files
                .iter()
                .find(|file| file.id == external.file_id)
                .ok_or_else(|| invalid("selected locked file is missing"))?;
            let source_path = supplied_external
                .get(&(external.lock_id.as_str(), external.file_id.as_str()))
                .ok_or_else(|| invalid("selected external file path is missing"))?;
            verify_media_locked_input(source_path, input)?;
            if let Some(legacy_file) = receipt_files.get(required.role.as_str()) {
                let legacy_digest = Sha256Digest::parse(&legacy_file.sha256)
                    .map_err(|_| invalid("legacy file digest is malformed"))?;
                let locked_digest = Sha256Digest::parse(&input.sha256)
                    .map_err(|_| invalid("locked file digest is malformed"))?;
                if legacy_digest != locked_digest || legacy_file.size_bytes != input.size_bytes {
                    return Err(invalid("legacy receipt differs from the external lock"));
                }
            }
            (
                Sha256Digest::parse(&input.sha256)
                    .map_err(|_| invalid("locked file digest is malformed"))?,
                input.size_bytes,
                (*source_path).clone(),
                MediaPlanFileOrigin::External {
                    lock_id: external.lock_id.clone(),
                    file_id: external.file_id.clone(),
                },
            )
        } else {
            let input = receipt_files
                .get(required.role.as_str())
                .ok_or_else(|| invalid("required build role is missing"))?;
            (
                Sha256Digest::parse(&input.sha256)
                    .map_err(|_| invalid("build file digest is malformed"))?,
                input.size_bytes,
                build_root.join(&input.path),
                MediaPlanFileOrigin::Build,
            )
        };
        total_payload_bytes = total_payload_bytes
            .checked_add(size_bytes)
            .ok_or_else(|| invalid("total payload size overflows u64"))?;
        files.push(MediaPlanFile {
            role: required.role.clone(),
            destination: required.destination.clone(),
            sha256,
            size_bytes,
            source_path,
            origin,
        });
    }
    files.sort_by(|left, right| left.destination.cmp(&right.destination));
    let raw_image_size_bytes = match &profile.profile.layout {
        MediaLayout::MbrFat32 {
            start_lba,
            size_bytes,
            ..
        } => {
            if total_payload_bytes > *size_bytes {
                return Err(invalid("payload bytes exceed the FAT32 partition extent"));
            }
            Some(
                start_lba
                    .checked_mul(512)
                    .and_then(|start| start.checked_add(*size_bytes))
                    .ok_or_else(|| invalid("raw image size overflows u64"))?,
            )
        }
        MediaLayout::Iso9660ElTorito { .. } => None,
    };

    Ok(MediaImagePlan {
        profile_id: profile.profile.id.clone(),
        profile_sha256: profile.sha256.clone(),
        receipt_sha256: sha256_bytes(receipt_bytes),
        receipt_origin: receipt.origin,
        build_identity: receipt.build_identity,
        target_preset: profile.profile.target_preset.clone(),
        model: profile.profile.model.clone(),
        transport: profile.profile.transport.clone(),
        medium: profile.profile.medium.clone(),
        layout: profile.profile.layout.clone(),
        external_lock_sha256: locks
            .into_iter()
            .map(|lock| (lock.lock.id, lock.sha256))
            .collect(),
        files,
        total_payload_bytes,
        raw_image_size_bytes,
    })
}

fn invalid(message: &str) -> MediaPlanError {
    MediaPlanError::Invalid(message.to_string())
}

#[cfg(test)]
mod tests {
    use super::{plan_media_image, MediaExternalFile, MediaPlanFileOrigin};
    use crate::media_profile::{built_in_media_profiles, parse_media_profile};
    use crate::media_receipt::{MediaBuildFile, MediaBuildReceipt, MediaReceiptOrigin};
    use crate::sha256_bytes;
    use std::fs;

    fn receipt(
        origin: MediaReceiptOrigin,
        target: &str,
        model: &str,
        transport: &str,
        roles: &[(&str, &str, &[u8])],
    ) -> Vec<u8> {
        let files = roles
            .iter()
            .map(|(role, path, bytes)| MediaBuildFile {
                role: (*role).to_string(),
                path: (*path).to_string(),
                sha256: sha256_bytes(bytes).to_string(),
                size_bytes: bytes.len() as u64,
            })
            .collect();
        let receipt = MediaBuildReceipt::new(
            origin,
            target.to_string(),
            model.to_string(),
            transport.to_string(),
            files,
        )
        .unwrap();
        serde_json::to_vec(&receipt).unwrap()
    }

    #[test]
    fn plans_registered_sd_files_without_creating_an_image() {
        let root = tempfile::tempdir().unwrap();
        let profile = built_in_media_profiles().unwrap().remove(0);
        let mut roles = Vec::new();
        for required in &profile.profile.required_files {
            let filename = format!("{}.bin", required.role);
            fs::write(root.path().join(&filename), required.role.as_bytes()).unwrap();
            roles.push((
                required.role.clone(),
                filename,
                required.role.as_bytes().to_vec(),
            ));
        }
        let role_refs: Vec<_> = roles
            .iter()
            .map(|(role, path, bytes)| (role.as_str(), path.as_str(), bytes.as_slice()))
            .collect();
        let bytes = receipt(
            MediaReceiptOrigin::Cmake,
            &profile.profile.target_preset,
            &profile.profile.model,
            &profile.profile.transport,
            &role_refs,
        );
        let plan = plan_media_image(&profile, root.path(), &bytes, &[], &[]).unwrap();
        assert_eq!(plan.files.len(), profile.profile.required_files.len());
        assert_eq!(plan.profile_sha256, profile.sha256);
        assert_eq!(plan.receipt_sha256, sha256_bytes(&bytes));
        assert_eq!(plan.raw_image_size_bytes, Some(68_157_440));
        assert!(plan
            .files
            .iter()
            .all(|file| file.origin == MediaPlanFileOrigin::Build));
        assert!(!root.path().join("image.img").exists());
        let legacy_bytes = receipt(
            MediaReceiptOrigin::LegacyV1,
            &profile.profile.target_preset,
            &profile.profile.model,
            &profile.profile.transport,
            &role_refs,
        );
        let legacy_plan = plan_media_image(&profile, root.path(), &legacy_bytes, &[], &[]).unwrap();
        assert_eq!(legacy_plan.receipt_origin, MediaReceiptOrigin::LegacyV1);
        assert_eq!(legacy_plan.files.len(), plan.files.len());

        fs::write(root.path().join("config.bin"), b"changed").unwrap();
        assert!(plan_media_image(&profile, root.path(), &bytes, &[], &[]).is_err());
        fs::write(root.path().join("config.bin"), b"config").unwrap();
        let mut extra = role_refs.clone();
        extra.push(("unselected", "extra.bin", b"extra"));
        fs::write(root.path().join("extra.bin"), b"extra").unwrap();
        let extra_receipt = receipt(
            MediaReceiptOrigin::Cmake,
            &profile.profile.target_preset,
            &profile.profile.model,
            &profile.profile.transport,
            &extra,
        );
        assert!(plan_media_image(&profile, root.path(), &extra_receipt, &[], &[]).is_err());
    }

    #[test]
    fn binds_external_file_to_the_exact_profile_lock_before_planning() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("bootstrap"), b"built").unwrap();
        fs::write(root.path().join("firmware.bin"), b"locked").unwrap();
        let lock = format!(
            "format_version = 1\nkind = \"aros-media-external-inputs\"\nid = \"firmware\"\n\n[[files]]\nid = \"blob\"\norigin_url = \"https://example.org/firmware.bin\"\norigin_revision = \"v1\"\nsha256 = \"{}\"\nsize_bytes = 6\nlicense_id = \"MIT\"\n",
            sha256_bytes(b"locked")
        );
        let profile = format!(
            "format_version = 1\nid = \"test-media\"\ntarget_preset = \"pc-x86_64\"\nmodel = \"pc\"\ntransport = \"bios\"\nmedium = \"iso9660-el-torito\"\nboot_protocol = \"grub-bios\"\nlabel = \"Test media\"\n\n[layout]\nkind = \"iso9660-el-torito\"\nvolume_id = \"AROSTEST\"\nboot_image_role = \"bootstrap\"\n\n[[external_locks]]\nid = \"firmware\"\nsha256 = \"{}\"\n\n[[required_files]]\nrole = \"bootstrap\"\ndestination = \"boot/bootstrap\"\n\n[[required_files]]\nrole = \"firmware\"\ndestination = \"boot/firmware.bin\"\nexternal_input = {{ lock_id = \"firmware\", file_id = \"blob\" }}\n",
            sha256_bytes(lock.as_bytes())
        );
        let profile = parse_media_profile("test", &profile).unwrap();
        let receipt = receipt(
            MediaReceiptOrigin::Cmake,
            "pc-x86_64",
            "pc",
            "bios",
            &[("bootstrap", "bootstrap", b"built")],
        );
        let external = [MediaExternalFile {
            lock_id: "firmware".into(),
            file_id: "blob".into(),
            path: root.path().join("firmware.bin"),
        }];
        let locks = [("firmware", lock.as_bytes())];
        let plan = plan_media_image(&profile, root.path(), &receipt, &locks, &external).unwrap();
        assert_eq!(plan.files.len(), 2);
        assert_eq!(plan.raw_image_size_bytes, None);
        assert!(matches!(
            plan.files[1].origin,
            MediaPlanFileOrigin::External { .. }
        ));
        assert!(plan_media_image(&profile, root.path(), &receipt, &[], &external).is_err());
        assert!(plan_media_image(&profile, root.path(), &receipt, &locks, &[]).is_err());
        fs::write(&external[0].path, b"tamper").unwrap();
        assert!(plan_media_image(&profile, root.path(), &receipt, &locks, &external).is_err());
    }
}
