//! Independent read-back of a composed MBR/FAT32 artifact.
//!
//! The manifest and checksum file describe content, not authenticated origin.
//! A caller requiring provenance must bind them to a reviewed profile/receipt
//! and an external attestation separately.

use super::{
    image_geometry, portable_path, validate_fat_label, validate_relative_path, validate_role,
    verify_raw_image, ImageFileExpectation, PartitionLayout, ARTIFACT_CHECKSUMS,
    MEDIA_ARTIFACT_MANIFEST, MEDIA_RAW_IMAGE_FILENAME, MIB, SECTOR_BYTES,
};
use aros_common::media_receipt::{
    validate_media_build_identity, MediaBuildIdentity, MediaReceiptOrigin,
};
use aros_common::Sha256Digest;
use aros_common::{casefold_path_key, open_regular_file_nofollow, sha256_bytes, sha256_reader};
use miette::Result;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::Path;

const MAX_MANIFEST_BYTES: u64 = 2 * MIB;
const MAX_CHECKSUM_BYTES: u64 = 512;

/// Exact image facts obtained from the artifact, without its original sources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedMediaArtifact {
    pub profile_id: String,
    pub profile_sha256: Sha256Digest,
    pub receipt_sha256: Sha256Digest,
    pub receipt_origin: MediaReceiptOrigin,
    pub build_identity: Option<MediaBuildIdentity>,
    pub target_preset: String,
    pub external_lock_sha256: BTreeMap<String, Sha256Digest>,
    pub image_sha256: Sha256Digest,
    pub image_size_bytes: u64,
    pub file_count: usize,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    format_version: u32,
    kind: String,
    profile_id: String,
    profile_sha256: Sha256Digest,
    receipt_sha256: Sha256Digest,
    receipt_origin: MediaReceiptOrigin,
    #[serde(default)]
    build_identity: Option<MediaBuildIdentity>,
    target_preset: String,
    model: String,
    transport: String,
    medium: String,
    partition: ManifestPartition,
    external_lock_sha256: BTreeMap<String, Sha256Digest>,
    files: Vec<ManifestFile>,
    image: ManifestImage,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestPartition {
    scheme: String,
    filesystem: String,
    start_lba: u64,
    size_bytes: u64,
    label: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestImage {
    filename: String,
    sha256: Sha256Digest,
    size_bytes: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestFile {
    role: String,
    destination: String,
    sha256: Sha256Digest,
    size_bytes: u64,
    origin: ManifestOrigin,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum ManifestOrigin {
    Build,
    External { lock_id: String, file_id: String },
}

/// Verify a composed artifact without the source tree or its input plan.
///
/// The directory must contain exactly the image, manifest and SHA256SUMS as
/// regular files. The image hash, MBR, FAT32 filesystem and every manifest
/// file are read back. This verifies self-consistency, not build provenance.
///
/// # Errors
///
/// Rejects an unknown schema, unexpected artifact entry, changed checksum,
/// unsafe placement, invalid geometry or an unreadable/altered image file.
pub fn verify_fat32_media_artifact(artifact_dir: &Path) -> Result<VerifiedMediaArtifact> {
    let expected = BTreeSet::from([
        MEDIA_RAW_IMAGE_FILENAME,
        MEDIA_ARTIFACT_MANIFEST,
        ARTIFACT_CHECKSUMS,
    ]);
    let mut seen = BTreeSet::new();
    for entry in fs::read_dir(artifact_dir).map_err(|error| {
        miette::miette!(
            "Cannot read media artifact '{}': {error}",
            artifact_dir.display()
        )
    })? {
        let entry =
            entry.map_err(|error| miette::miette!("Cannot inspect media entry: {error}"))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| miette::miette!("Media artifact contains a non-Unicode filename."))?;
        if !expected.contains(name.as_str()) || !seen.insert(name.clone()) {
            miette::bail!("Media artifact contains unexpected entry '{name}'.");
        }
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|error| miette::miette!("Cannot inspect media entry '{name}': {error}"))?;
        if !metadata.file_type().is_file() {
            miette::bail!("Media artifact entry '{name}' is not a regular file.");
        }
    }
    if seen.len() != expected.len() {
        miette::bail!("Media artifact is missing a required file.");
    }

    let manifest_bytes = read_bounded_regular(
        &artifact_dir.join(MEDIA_ARTIFACT_MANIFEST),
        MAX_MANIFEST_BYTES,
    )?;
    let manifest: Manifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| miette::miette!("Invalid media image manifest: {error}"))?;
    let files = validate_manifest(&manifest)?;
    let checksums =
        read_bounded_regular(&artifact_dir.join(ARTIFACT_CHECKSUMS), MAX_CHECKSUM_BYTES)?;
    let expected_checksums = format!(
        "{}  {}\n{}  {}\n",
        manifest.image.sha256,
        MEDIA_RAW_IMAGE_FILENAME,
        sha256_bytes(&manifest_bytes),
        MEDIA_ARTIFACT_MANIFEST
    );
    if checksums != expected_checksums.as_bytes() {
        miette::bail!("Media artifact SHA256SUMS does not match its manifest and image.");
    }

    let image_path = artifact_dir.join(MEDIA_RAW_IMAGE_FILENAME);
    let measure = measure_regular(&image_path)?;
    if measure.0 != manifest.image.sha256 || measure.1 != manifest.image.size_bytes {
        miette::bail!("Media image size or SHA-256 differs from its manifest.");
    }
    let partition = PartitionLayout {
        scheme: manifest.partition.scheme,
        filesystem: manifest.partition.filesystem,
        start_lba: manifest.partition.start_lba,
        size_bytes: manifest.partition.size_bytes,
        label: manifest.partition.label,
    };
    let geometry = image_geometry(&partition)?;
    if geometry.image_size_bytes != measure.1 {
        miette::bail!("Media image extent differs from its partition geometry.");
    }
    verify_raw_image(&files, &image_path, &geometry)?;
    if measure_regular(&image_path)? != measure {
        miette::bail!("Media image changed during filesystem verification.");
    }
    Ok(VerifiedMediaArtifact {
        profile_id: manifest.profile_id,
        profile_sha256: manifest.profile_sha256,
        receipt_sha256: manifest.receipt_sha256,
        receipt_origin: manifest.receipt_origin,
        build_identity: manifest.build_identity,
        target_preset: manifest.target_preset,
        external_lock_sha256: manifest.external_lock_sha256,
        image_sha256: measure.0,
        image_size_bytes: measure.1,
        file_count: files.len(),
    })
}

fn validate_manifest(manifest: &Manifest) -> Result<Vec<ImageFileExpectation>> {
    if !matches!(manifest.format_version, 1 | 2)
        || manifest.kind != "aros-media-image"
        || manifest.medium != "mbr-fat32"
        || manifest.partition.scheme != "mbr"
        || manifest.partition.filesystem != "fat32"
        || manifest.image.filename != MEDIA_RAW_IMAGE_FILENAME
    {
        miette::bail!("Unsupported media image manifest identity or layout.");
    }
    if manifest.format_version == 1 && manifest.build_identity.is_some() {
        miette::bail!("Historical media manifest cannot claim a build identity.");
    }
    if manifest.receipt_origin == MediaReceiptOrigin::LegacyV1 && manifest.build_identity.is_some()
    {
        miette::bail!("Legacy media manifest cannot claim a build identity.");
    }
    if let Some(identity) = &manifest.build_identity {
        validate_media_build_identity(identity).map_err(|error| {
            miette::miette!("Media manifest build identity is invalid: {error}")
        })?;
    }
    for (value, label) in [
        (&manifest.profile_id, "profile ID"),
        (&manifest.target_preset, "target preset"),
        (&manifest.model, "model"),
        (&manifest.transport, "transport"),
    ] {
        if value.is_empty() || value != value.trim() {
            miette::bail!("Media manifest has an invalid {label}.");
        }
    }
    validate_fat_label(&manifest.partition.label)?;
    if manifest.partition.start_lba < 2048
        || !manifest.partition.start_lba.is_multiple_of(2048)
        || manifest.partition.size_bytes < 64 * MIB
        || !manifest.partition.size_bytes.is_multiple_of(SECTOR_BYTES)
    {
        miette::bail!("Media manifest has invalid MBR/FAT32 geometry.");
    }
    if manifest.files.is_empty() || manifest.files.len() > 1024 {
        miette::bail!("Media manifest has an invalid file count.");
    }
    let mut roles = BTreeSet::new();
    let mut destinations = BTreeSet::new();
    let mut referenced_locks = BTreeSet::new();
    let mut files = Vec::with_capacity(manifest.files.len());
    for file in &manifest.files {
        validate_role(&file.role)?;
        if !roles.insert(file.role.as_str()) {
            miette::bail!("Media manifest has a duplicate role.");
        }
        let destination = validate_relative_path(&file.destination, "media destination")?;
        let folded = casefold_path_key(&destination)
            .map_err(|error| miette::miette!("Unsafe media destination: {error}"))?;
        if !destinations.insert(folded) {
            miette::bail!("Media manifest has a duplicate destination.");
        }
        if let ManifestOrigin::External { lock_id, file_id } = &file.origin {
            validate_role(lock_id)?;
            validate_role(file_id)?;
            referenced_locks.insert(lock_id.as_str());
        }
        files.push(ImageFileExpectation {
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
    for lock_id in manifest.external_lock_sha256.keys() {
        validate_role(lock_id)?;
    }
    if referenced_locks
        != manifest
            .external_lock_sha256
            .keys()
            .map(String::as_str)
            .collect()
    {
        miette::bail!("Media manifest external locks differ from selected files.");
    }
    Ok(files)
}

fn read_bounded_regular(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let file = open_regular_file_nofollow(path).map_err(|error| {
        miette::miette!(
            "Cannot open regular media file '{}': {error}",
            path.display()
        )
    })?;
    if file
        .metadata()
        .map_err(|error| miette::miette!("Cannot stat media file: {error}"))?
        .len()
        > limit
    {
        miette::bail!(
            "Media metadata file '{}' exceeds its size limit.",
            path.display()
        );
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| miette::miette!("Cannot read media file '{}': {error}", path.display()))?;
    if bytes.len() as u64 > limit {
        miette::bail!(
            "Media metadata file '{}' exceeds its size limit.",
            path.display()
        );
    }
    Ok(bytes)
}

fn measure_regular(path: &Path) -> Result<(Sha256Digest, u64)> {
    let mut file = open_regular_file_nofollow(path).map_err(|error| {
        miette::miette!(
            "Cannot open regular media image '{}': {error}",
            path.display()
        )
    })?;
    let measure = sha256_reader(&mut file).map_err(|error| {
        miette::miette!("Cannot hash media image '{}': {error}", path.display())
    })?;
    Ok((measure.digest, measure.size))
}
pub(super) fn verify_fat_inventory<T: fatfs::ReadWriteSeek>(
    root: &fatfs::Dir<'_, T>,
    files: &[ImageFileExpectation],
) -> Result<()> {
    let mut actual_files = BTreeSet::new();
    let mut actual_dirs = BTreeSet::new();
    collect_fat_entries(root, "", 0, &mut actual_files, &mut actual_dirs)?;
    let mut expected_files = BTreeSet::new();
    let mut expected_dirs = BTreeSet::new();
    for file in files {
        let destination = portable_path(&file.destination);
        expected_files.insert(
            aros_common::casefold_path_key(&file.destination)
                .map_err(|error| miette::miette!("Unsafe expected FAT path: {error}"))?,
        );
        let mut parent = destination.as_str();
        while let Some((prefix, _)) = parent.rsplit_once('/') {
            expected_dirs.insert(
                aros_common::casefold_path_key(Path::new(prefix))
                    .map_err(|error| miette::miette!("Unsafe expected FAT directory: {error}"))?,
            );
            parent = prefix;
        }
    }
    if actual_files != expected_files || actual_dirs != expected_dirs {
        miette::bail!("FAT32 image inventory differs from the declared media files.");
    }
    Ok(())
}

fn collect_fat_entries<T: fatfs::ReadWriteSeek>(
    dir: &fatfs::Dir<'_, T>,
    prefix: &str,
    depth: usize,
    files: &mut BTreeSet<String>,
    dirs: &mut BTreeSet<String>,
) -> Result<()> {
    if depth > 32 || files.len() + dirs.len() > 2048 {
        miette::bail!("FAT32 image directory inventory exceeds its limits.");
    }
    for entry in dir.iter() {
        let entry =
            entry.map_err(|error| miette::miette!("Cannot enumerate FAT32 image: {error}"))?;
        let name = entry.file_name();
        if name == "." || name == ".." {
            continue;
        }
        let path = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let relative = validate_relative_path(&path, "FAT32 image entry")?;
        let folded = aros_common::casefold_path_key(&relative)
            .map_err(|error| miette::miette!("Unsafe FAT32 image entry: {error}"))?;
        if entry.is_dir() {
            if !dirs.insert(folded) {
                miette::bail!("FAT32 image contains a duplicate directory entry.");
            }
            collect_fat_entries(&entry.to_dir(), &path, depth + 1, files, dirs)?;
        } else if !files.insert(folded) {
            miette::bail!("FAT32 image contains a duplicate file entry.");
        }
        if files.len() + dirs.len() > 2048 {
            miette::bail!("FAT32 image directory inventory exceeds its limits.");
        }
    }
    Ok(())
}
