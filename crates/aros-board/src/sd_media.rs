//! MBR/FAT32 media-plan validation and deterministic artifact metadata.

use super::{
    image_geometry, validate_fat_label, validate_relative_path, validate_role, PartitionLayout,
    RawImage, VerifiedBootFile, MEDIA_RAW_IMAGE_FILENAME, MIB, SECTOR_BYTES,
};
use aros_common::media_plan::{MediaImagePlan, MediaPlanFileOrigin};
use aros_common::media_profile::MediaLayout;
use aros_common::media_receipt::MediaReceiptOrigin;
use miette::Result;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub(super) fn validate_media_plan_for_fat32(
    plan: &MediaImagePlan,
) -> Result<(PartitionLayout, Vec<VerifiedBootFile>)> {
    let MediaLayout::MbrFat32 {
        start_lba,
        size_bytes,
        label,
    } = &plan.layout
    else {
        miette::bail!("The selected media plan is not an MBR/FAT32 layout.");
    };
    if plan.medium != "mbr-fat32"
        || *start_lba < 2048
        || !start_lba.is_multiple_of(2048)
        || *size_bytes < 64 * MIB
        || !size_bytes.is_multiple_of(SECTOR_BYTES)
    {
        miette::bail!("The selected media plan has invalid MBR/FAT32 geometry.");
    }
    validate_fat_label(label)?;
    let partition = PartitionLayout {
        scheme: "mbr".into(),
        filesystem: "fat32".into(),
        start_lba: *start_lba,
        size_bytes: *size_bytes,
        label: label.clone(),
    };
    let geometry = image_geometry(&partition)?;
    if plan.raw_image_size_bytes != Some(geometry.image_size_bytes) {
        miette::bail!("The media plan's raw-image extent differs from its layout.");
    }
    if plan.files.is_empty() || plan.files.len() > 1024 {
        miette::bail!("The media plan has an invalid file count.");
    }
    let mut roles = BTreeSet::new();
    let mut destinations = BTreeSet::new();
    let mut files = Vec::with_capacity(plan.files.len());
    let mut total_bytes = 0_u64;
    for file in &plan.files {
        validate_role(&file.role)?;
        if !roles.insert(file.role.as_str()) {
            miette::bail!("The media plan has a duplicate file role.");
        }
        let destination = validate_relative_path(&file.destination, "media destination")?;
        let folded = aros_common::casefold_path_key(&destination)
            .map_err(|error| miette::miette!("Unsafe media destination: {error}"))?;
        if !destinations.insert(folded) {
            miette::bail!("The media plan has a duplicate destination.");
        }
        let mut source = aros_common::open_regular_file_nofollow(&file.source_path)
            .map_err(|error| miette::miette!("Unsafe media source for '{}': {error}", file.role))?;
        let measured = aros_common::sha256_reader(&mut source).map_err(|error| {
            miette::miette!("Cannot measure media source for '{}': {error}", file.role)
        })?;
        if measured.digest != file.sha256 || measured.size != file.size_bytes {
            miette::bail!("Media source for '{}' changed after planning.", file.role);
        }
        total_bytes = total_bytes
            .checked_add(measured.size)
            .ok_or_else(|| miette::miette!("Media payload byte count overflows u64."))?;
        files.push(VerifiedBootFile {
            role: file.role.clone(),
            source: file.source_path.clone(),
            destination,
            sha256: file.sha256.to_string(),
            size_bytes: file.size_bytes,
        });
    }
    for destination in &destinations {
        let mut parent = destination.as_str();
        while let Some((prefix, _)) = parent.rsplit_once('/') {
            if destinations.contains(prefix) {
                miette::bail!("A media file destination is a parent of another file.");
            }
            parent = prefix;
        }
    }
    if total_bytes != plan.total_payload_bytes || total_bytes > partition.size_bytes {
        miette::bail!("Media plan payload size differs from the verified inputs or partition.");
    }
    files.sort_by(|left, right| left.destination.cmp(&right.destination));
    Ok((partition, files))
}

pub(super) fn stable_media_volume_id(plan: &MediaImagePlan) -> u32 {
    let mut hasher = Sha256::new();
    hasher.update(b"aros-media-volume-id-v1\n");
    hasher.update(plan.profile_sha256.as_str());
    hasher.update(b"\n");
    hasher.update(plan.receipt_sha256.as_str());
    hasher.update(b"\n");
    for (id, sha256) in &plan.external_lock_sha256 {
        hasher.update(id);
        hasher.update(b"=");
        hasher.update(sha256.as_str());
        hasher.update(b"\n");
    }
    let digest = hasher.finalize();
    u32::from_le_bytes([digest[0], digest[1], digest[2], digest[3]])
}

pub(super) fn render_media_manifest(
    plan: &MediaImagePlan,
    partition: &PartitionLayout,
    image: &RawImage,
) -> Result<Vec<u8>> {
    let mut sorted_files: Vec<_> = plan.files.iter().collect();
    sorted_files.sort_by(|left, right| left.destination.cmp(&right.destination));
    let files: Vec<_> = sorted_files
        .into_iter()
        .map(|file| {
            let origin = match &file.origin {
                MediaPlanFileOrigin::Build => serde_json::json!({"kind":"build"}),
                MediaPlanFileOrigin::External { lock_id, file_id } => {
                    serde_json::json!({"kind":"external","lock_id":lock_id,"file_id":file_id})
                }
            };
            serde_json::json!({
                "role": file.role,
                "destination": file.destination,
                "sha256": file.sha256,
                "size_bytes": file.size_bytes,
                "origin": origin
            })
        })
        .collect();
    let document = serde_json::json!({
        "format_version": 1,
        "kind": "aros-media-image",
        "profile_id": plan.profile_id,
        "profile_sha256": plan.profile_sha256,
        "receipt_sha256": plan.receipt_sha256,
        "receipt_origin": match plan.receipt_origin {
            MediaReceiptOrigin::Cmake => "cmake",
            MediaReceiptOrigin::LegacyV1 => "legacy-v1",
        },
        "target_preset": plan.target_preset,
        "model": plan.model,
        "transport": plan.transport,
        "medium": plan.medium,
        "partition": {
            "scheme": partition.scheme,
            "filesystem": partition.filesystem,
            "start_lba": partition.start_lba,
            "size_bytes": partition.size_bytes,
            "label": partition.label,
        },
        "external_lock_sha256": plan.external_lock_sha256,
        "files": files,
        "image": {
            "filename": MEDIA_RAW_IMAGE_FILENAME,
            "sha256": image.sha256,
            "size_bytes": image.size_bytes,
        },
    });
    let mut encoded = serde_json::to_vec_pretty(&document)
        .map_err(|error| miette::miette!("Cannot serialize media manifest: {error}"))?;
    encoded.push(b'\n');
    Ok(encoded)
}
