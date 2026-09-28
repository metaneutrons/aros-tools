//! Deterministic ISO/El Torito composition with independent image read-back.
//! `xorriso` is a fixed backend executable, never a command from profile data.

use super::{
    copy_and_hash, publish_staged_directory_noreplace, resolve_new_output_path,
    validate_relative_path, validate_role, write_new_file, RawImage, StagedMediaImage,
    VerifiedMediaArtifact, ARTIFACT_CHECKSUMS, MEDIA_ARTIFACT_MANIFEST, MEDIA_ISO_IMAGE_FILENAME,
};
use aros_common::media_plan::MediaImagePlan;
use aros_common::media_profile::MediaLayout;
use aros_common::media_receipt::{
    validate_media_build_identity, MediaBuildIdentity, MediaReceiptOrigin,
};
use aros_common::{open_regular_file_nofollow, sha256_bytes, sha256_reader, Sha256Digest};
use miette::Result;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FIXED_DATE: &str = "2020010100000000";
const MAX_DOCUMENT: u64 = 8 * 1024 * 1024;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct IsoManifest {
    format_version: u32,
    kind: String,
    profile_id: String,
    profile_sha256: Sha256Digest,
    receipt_sha256: Sha256Digest,
    receipt_origin: MediaReceiptOrigin,
    build_identity: Option<MediaBuildIdentity>,
    target_preset: String,
    model: String,
    transport: String,
    medium: String,
    backend_version: String,
    volume_id: String,
    boot_image: String,
    catalog_path: String,
    max_size_bytes: u64,
    external_lock_sha256: BTreeMap<String, Sha256Digest>,
    directories: Vec<String>,
    files: Vec<IsoFile>,
    image: IsoImage,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct IsoFile {
    role: String,
    destination: String,
    input_sha256: Option<Sha256Digest>,
    input_size_bytes: Option<u64>,
    sha256: Sha256Digest,
    size_bytes: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct IsoImage {
    filename: String,
    sha256: Sha256Digest,
    size_bytes: u64,
}

/// Compose an ISO in an isolated sibling and publish only after full read-back
/// and the caller's final source/toolchain identity gate.
///
/// # Errors
/// Refuses invalid plans, changed inputs, unsupported tools, unexpected ISO
/// entries, altered El Torito metadata, or an existing destination.
pub fn stage_iso_media_plan_with_gate<F>(
    plan: &MediaImagePlan,
    output_dir: &Path,
    identity_gate: F,
) -> Result<StagedMediaImage>
where
    F: FnOnce() -> Result<()>,
{
    let MediaLayout::Iso9660ElTorito {
        volume_id,
        boot_image_role,
        catalog_path,
        max_size_bytes,
    } = &plan.layout
    else {
        miette::bail!("The selected media plan is not an ISO/El Torito layout.");
    };
    if plan.medium != "iso9660-el-torito"
        || plan.files.is_empty()
        || plan.files.len() > 20_000
        || plan.total_payload_bytes > *max_size_bytes
        || plan.raw_image_size_bytes.is_some()
        || plan.image_capacity_bytes != Some(*max_size_bytes)
        || plan.generated_files != [catalog_path.clone()]
    {
        miette::bail!("Invalid ISO media plan or payload capacity.");
    }
    let destination = resolve_new_output_path(output_dir)?;
    let stage = tempfile::Builder::new()
        .prefix(".aros-media-iso-stage-")
        .tempdir_in(
            destination
                .parent()
                .ok_or_else(|| miette::miette!("No output parent."))?,
        )
        .map_err(|error| miette::miette!("Cannot create isolated ISO stage: {error}"))?;
    let root = stage.path().join("input");
    fs::create_dir(&root)
        .map_err(|error| miette::miette!("Cannot create ISO staging files: {error}"))?;
    let mut roles = BTreeSet::new();
    let mut destinations = BTreeSet::new();
    let mut grafts = String::new();
    let mut planned = Vec::new();
    let mut total = 0_u64;
    let mut boot_image = None;
    for (index, file) in plan.files.iter().enumerate() {
        validate_role(&file.role)?;
        if !roles.insert(file.role.as_str()) {
            miette::bail!("Duplicate ISO file role.");
        }
        let relative = validate_iso_path(&file.destination)?;
        if !destinations.insert(relative.clone()) || file.destination == *catalog_path {
            miette::bail!("Duplicate or reserved ISO destination.");
        }
        let source_name = format!("f{index:05}");
        let target = root.join(&source_name);
        let (digest, size) = copy_and_hash(&file.source_path, &target)?;
        if digest != file.sha256.to_string() || size != file.size_bytes {
            miette::bail!("ISO source '{}' changed during composition.", file.role);
        }
        normalize_file_mode(&target)?;
        writeln!(grafts, "{}=input/{source_name}", file.destination)
            .expect("writing to String cannot fail");
        total = total
            .checked_add(size)
            .ok_or_else(|| miette::miette!("ISO payload size overflow."))?;
        if file.role == *boot_image_role {
            boot_image = Some(file.destination.clone());
        }
        planned.push(IsoFile {
            role: file.role.clone(),
            destination: file.destination.clone(),
            input_sha256: Some(file.sha256.clone()),
            input_size_bytes: Some(size),
            sha256: file.sha256.clone(),
            size_bytes: size,
        });
    }
    if total != plan.total_payload_bytes || total > *max_size_bytes {
        miette::bail!("ISO planned payload size differs from measured inputs.");
    }
    for (index, directory) in plan.directories.iter().enumerate() {
        validate_iso_path(directory)?;
        let has_child_file = plan
            .files
            .iter()
            .any(|file| file.destination.starts_with(&format!("{directory}/")));
        let has_child_directory = plan
            .directories
            .iter()
            .any(|other| other.starts_with(&format!("{directory}/")));
        if !has_child_file && !has_child_directory {
            let source_name = format!("d{index:05}");
            fs::create_dir(root.join(&source_name))
                .map_err(|error| miette::miette!("Cannot stage empty ISO directory: {error}"))?;
            normalize_directory_mode(&root.join(&source_name))?;
            writeln!(grafts, "{directory}=input/{source_name}")
                .expect("writing to String cannot fail");
        }
    }
    normalize_directory_mode(&root)?;
    write_new_file(&stage.path().join("grafts.txt"), grafts.as_bytes())?;
    let boot_image =
        boot_image.ok_or_else(|| miette::miette!("El Torito boot image role is missing."))?;
    let backend_version = xorriso_version()?;
    let image_path = stage.path().join(MEDIA_ISO_IMAGE_FILENAME);
    run_xorriso_in(
        stage.path(),
        &[
            "-as",
            "mkisofs",
            "-graft-points",
            "-path-list",
            "grafts.txt",
            "-R",
            "-iso-level",
            "3",
            "-uid",
            "0",
            "-gid",
            "0",
            "-volid",
            volume_id,
            "-b",
            &boot_image,
            "-c",
            catalog_path,
            "-no-emul-boot",
            "-boot-load-size",
            "4",
            "-boot-info-table",
            &format!("--modification-date={FIXED_DATE}"),
            "--set_all_file_dates",
            FIXED_DATE,
            "-o",
            MEDIA_ISO_IMAGE_FILENAME,
        ],
    )?;
    let (image_sha256, image_size_bytes) = measure(&image_path)?;
    if image_size_bytes > *max_size_bytes || !image_size_bytes.is_multiple_of(2048) {
        miette::bail!("Produced ISO exceeds profile capacity or has invalid block size.");
    }
    // The El Torito catalog is generated, and -boot-info-table patches a
    // fixed field in the boot image. Record their *observed* ISO bytes.
    let mut extracted_paths: Vec<String> = planned
        .iter()
        .map(|file| file.destination.clone())
        .collect();
    extracted_paths.push(catalog_path.clone());
    let extracted = extract_inventory(&image_path, &extracted_paths)?;
    for file in &mut planned {
        let (digest, size) = extracted
            .get(&file.destination)
            .cloned()
            .ok_or_else(|| miette::miette!("ISO read-back omitted a planned file."))?;
        if file.destination != boot_image && (digest != file.sha256 || size != file.size_bytes) {
            miette::bail!("ISO payload differs from input '{}'.", file.role);
        }
        file.sha256 = digest;
        file.size_bytes = size;
    }
    let (catalog_sha256, catalog_size) = extracted
        .get(catalog_path)
        .cloned()
        .ok_or_else(|| miette::miette!("ISO read-back omitted the El Torito catalog."))?;
    planned.push(IsoFile {
        role: "eltorito-catalog".into(),
        destination: catalog_path.clone(),
        input_sha256: None,
        input_size_bytes: None,
        sha256: catalog_sha256,
        size_bytes: catalog_size,
    });
    planned.sort_by(|a, b| a.destination.cmp(&b.destination));
    let manifest = IsoManifest {
        format_version: 1,
        kind: "aros-media-iso-image".into(),
        profile_id: plan.profile_id.clone(),
        profile_sha256: plan.profile_sha256.clone(),
        receipt_sha256: plan.receipt_sha256.clone(),
        receipt_origin: plan.receipt_origin,
        build_identity: plan.build_identity.clone(),
        target_preset: plan.target_preset.clone(),
        model: plan.model.clone(),
        transport: plan.transport.clone(),
        medium: plan.medium.clone(),
        backend_version,
        volume_id: volume_id.clone(),
        boot_image,
        catalog_path: catalog_path.clone(),
        max_size_bytes: *max_size_bytes,
        external_lock_sha256: plan.external_lock_sha256.clone(),
        directories: plan.directories.clone(),
        files: planned,
        image: IsoImage {
            filename: MEDIA_ISO_IMAGE_FILENAME.into(),
            sha256: image_sha256.clone(),
            size_bytes: image_size_bytes,
        },
    };
    let mut bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|error| miette::miette!("Cannot serialize ISO manifest: {error}"))?;
    bytes.push(b'\n');
    write_new_file(&stage.path().join(MEDIA_ARTIFACT_MANIFEST), &bytes)?;
    let checksums = format!(
        "{}  {}\n{}  {}\n",
        image_sha256,
        MEDIA_ISO_IMAGE_FILENAME,
        sha256_bytes(&bytes),
        MEDIA_ARTIFACT_MANIFEST
    );
    write_new_file(&stage.path().join(ARTIFACT_CHECKSUMS), checksums.as_bytes())?;
    fs::remove_dir_all(&root)
        .map_err(|error| miette::miette!("Cannot clear ISO staging inputs: {error}"))?;
    fs::remove_file(stage.path().join("grafts.txt"))
        .map_err(|error| miette::miette!("Cannot clear ISO graft list: {error}"))?;
    verify_iso_media_artifact(stage.path())?;
    identity_gate()?;
    publish_staged_directory_noreplace(stage.path(), &destination)
        .map_err(|error| miette::miette!("Cannot atomically publish ISO artifact: {error}"))?;
    Ok(StagedMediaImage {
        artifact_dir: destination.clone(),
        image: RawImage {
            path: destination.join(MEDIA_ISO_IMAGE_FILENAME),
            sha256: image_sha256.to_string(),
            size_bytes: image_size_bytes,
        },
        manifest_path: destination.join(MEDIA_ARTIFACT_MANIFEST),
    })
}

/// Independently inspect and hash the ISO contents and El Torito metadata.
///
/// # Errors
/// Rejects a changed or extraneous artifact, mismatched image/filesystem,
/// unsupported manifest, missing BIOS boot entry, or a changed embedded file.
pub fn verify_iso_media_artifact(dir: &Path) -> Result<VerifiedMediaArtifact> {
    let expected = BTreeSet::from([
        MEDIA_ISO_IMAGE_FILENAME,
        MEDIA_ARTIFACT_MANIFEST,
        ARTIFACT_CHECKSUMS,
    ]);
    let mut found = BTreeSet::new();
    for entry in
        fs::read_dir(dir).map_err(|error| miette::miette!("Cannot read ISO artifact: {error}"))?
    {
        let entry =
            entry.map_err(|error| miette::miette!("Cannot inspect ISO artifact: {error}"))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| miette::miette!("Non-Unicode ISO artifact entry."))?;
        if !expected.contains(name.as_str())
            || !entry
                .file_type()
                .map_err(|error| miette::miette!("Cannot inspect ISO entry: {error}"))?
                .is_file()
        {
            miette::bail!("Unexpected or unsafe ISO artifact entry '{name}'.");
        }
        found.insert(name);
    }
    if found.len() != expected.len() {
        miette::bail!("Incomplete ISO artifact directory.");
    }
    let bytes = read_bounded(&dir.join(MEDIA_ARTIFACT_MANIFEST), MAX_DOCUMENT)?;
    let manifest: IsoManifest = serde_json::from_slice(&bytes)
        .map_err(|error| miette::miette!("Invalid ISO manifest: {error}"))?;
    if manifest.format_version != 1
        || manifest.kind != "aros-media-iso-image"
        || manifest.medium != "iso9660-el-torito"
        || !manifest
            .backend_version
            .starts_with("xorriso version   :  ")
        || manifest.image.filename != MEDIA_ISO_IMAGE_FILENAME
        || manifest.max_size_bytes < 1024 * 1024
        || !manifest.max_size_bytes.is_multiple_of(2048)
        || manifest.build_identity.is_some()
            && manifest.receipt_origin == MediaReceiptOrigin::LegacyV1
    {
        miette::bail!("Unsupported ISO manifest identity or layout.");
    }
    if let Some(identity) = &manifest.build_identity {
        validate_media_build_identity(identity)
            .map_err(|error| miette::miette!("Invalid ISO build identity: {error}"))?;
    }
    let sum = read_bounded(&dir.join(ARTIFACT_CHECKSUMS), 512)?;
    let expected_sum = format!(
        "{}  {}\n{}  {}\n",
        manifest.image.sha256,
        MEDIA_ISO_IMAGE_FILENAME,
        sha256_bytes(&bytes),
        MEDIA_ARTIFACT_MANIFEST
    );
    if sum != expected_sum.as_bytes() {
        miette::bail!("ISO SHA256SUMS mismatch.");
    }
    let image_path = dir.join(MEDIA_ISO_IMAGE_FILENAME);
    let measured = measure(&image_path)?;
    if measured.0 != manifest.image.sha256
        || measured.1 != manifest.image.size_bytes
        || measured.1 > manifest.max_size_bytes
        || !measured.1.is_multiple_of(2048)
    {
        miette::bail!("ISO image hash, size or capacity mismatch.");
    }
    validate_iso_path(&manifest.boot_image)?;
    validate_iso_path(&manifest.catalog_path)?;
    if manifest.volume_id.is_empty()
        || manifest.volume_id.len() > 32
        || !manifest
            .volume_id
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
    {
        miette::bail!("Invalid ISO volume ID.");
    }
    let report = xorriso_text(&image_path, &["-report_el_torito", "plain"])?;
    if !report.lines().any(|line| {
        line.starts_with("El Torito cat path :")
            && line.ends_with(&format!("/{}", manifest.catalog_path))
    }) || !report.lines().any(|line| {
        line.starts_with("El Torito img path :")
            && line.ends_with(&format!("/{}", manifest.boot_image))
    }) || !report.lines().any(|line| {
        line.starts_with("El Torito boot img :") && line.contains("BIOS") && line.contains(" y ")
    }) {
        miette::bail!("ISO El Torito BIOS boot metadata differs from the manifest.");
    }
    let pvd = xorriso_text(&image_path, &["-pvd_info"])?;
    if !pvd
        .lines()
        .any(|line| line == format!("Volume Id    : {}", manifest.volume_id))
    {
        miette::bail!("ISO volume ID differs from the manifest.");
    }
    let listed = xorriso_text(&image_path, &["-find", "/", "-type", "f"])?;
    let mut image_files = BTreeSet::new();
    for line in listed.lines() {
        let path = line
            .strip_prefix("'/")
            .and_then(|s| s.strip_suffix('\''))
            .ok_or_else(|| miette::miette!("Unrecognized ISO file listing."))?;
        validate_iso_path(path)?;
        if !image_files.insert(path.to_string()) {
            miette::bail!("Duplicate ISO file listing.");
        }
    }
    if manifest.files.is_empty() || manifest.files.len() > 20_001 {
        miette::bail!("Invalid ISO file count.");
    }
    let listed_dirs = xorriso_text(&image_path, &["-find", "/", "-type", "d"])?;
    let mut image_dirs = BTreeSet::new();
    for line in listed_dirs.lines() {
        if line == "'/'" {
            continue;
        }
        let path = line
            .strip_prefix("'/")
            .and_then(|s| s.strip_suffix('\''))
            .ok_or_else(|| miette::miette!("Unrecognized ISO directory listing."))?;
        validate_iso_path(path)?;
        if !image_dirs.insert(path.to_string()) {
            miette::bail!("Duplicate ISO directory listing.");
        }
    }
    let mut expected_dirs = BTreeSet::new();
    for directory in &manifest.directories {
        validate_iso_path(directory)?;
        if !expected_dirs.insert(directory.clone()) {
            miette::bail!("Duplicate ISO manifest directory.");
        }
    }
    if expected_dirs != image_dirs {
        miette::bail!("ISO directory inventory differs from its manifest: expected {expected_dirs:?}, found {image_dirs:?}.");
    }
    let mut expected_files = BTreeSet::new();
    let mut roles = BTreeSet::new();
    let mut catalog_count = 0;
    let all_paths: Vec<String> = manifest
        .files
        .iter()
        .map(|file| file.destination.clone())
        .collect();
    let extracted = extract_inventory(&image_path, &all_paths)?;
    for file in &manifest.files {
        validate_role(&file.role)?;
        validate_iso_path(&file.destination)?;
        if !roles.insert(file.role.as_str()) || !expected_files.insert(file.destination.clone()) {
            miette::bail!("Duplicate ISO role or placement.");
        }
        if file.destination == manifest.catalog_path {
            catalog_count += 1;
            if file.input_sha256.is_some() || file.input_size_bytes.is_some() {
                miette::bail!("Generated ISO catalog must not claim source bytes.");
            }
        } else if file.input_sha256.is_none() || file.input_size_bytes.is_none() {
            miette::bail!("ISO file has no input identity.");
        } else if file.destination != manifest.boot_image
            && (file.input_sha256.as_ref() != Some(&file.sha256)
                || file.input_size_bytes != Some(file.size_bytes))
        {
            miette::bail!("Untransformed ISO file differs from its input identity.");
        }
        if extracted.get(&file.destination) != Some(&(file.sha256.clone(), file.size_bytes)) {
            miette::bail!(
                "Embedded ISO file '{}' differs from the manifest.",
                file.destination
            );
        }
    }
    // xorriso deliberately hides the generated catalog from Rock Ridge
    // traversal, although -extract_single can independently read it.
    expected_files.remove(&manifest.catalog_path);
    if catalog_count != 1
        || !expected_files.contains(&manifest.boot_image)
        || expected_files != image_files
    {
        miette::bail!("ISO inventory differs from its manifest: expected {expected_files:?}, found {image_files:?}.");
    }
    if measure(&image_path)? != measured {
        miette::bail!("ISO changed during verification.");
    }
    Ok(VerifiedMediaArtifact {
        medium: manifest.medium,
        profile_id: manifest.profile_id,
        profile_sha256: manifest.profile_sha256,
        receipt_sha256: manifest.receipt_sha256,
        receipt_origin: manifest.receipt_origin,
        build_identity: manifest.build_identity,
        target_preset: manifest.target_preset,
        external_lock_sha256: manifest.external_lock_sha256,
        image_sha256: measured.0,
        image_size_bytes: measured.1,
        file_count: manifest.files.len(),
    })
}

fn validate_iso_path(raw: &str) -> Result<PathBuf> {
    let path = validate_relative_path(raw, "ISO destination")?;
    if !raw
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"/_+.-".contains(&byte))
    {
        miette::bail!("ISO destination contains unsupported characters.");
    }
    Ok(path)
}

fn normalize_file_mode(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o644))
            .map_err(|error| miette::miette!("Cannot normalize ISO file mode: {error}"))?;
    }
    Ok(())
}

fn normalize_directory_mode(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
            .map_err(|error| miette::miette!("Cannot normalize ISO directory mode: {error}"))?;
    }
    Ok(())
}

fn measure(path: &Path) -> Result<(Sha256Digest, u64)> {
    let mut file = open_regular_file_nofollow(path)
        .map_err(|error| miette::miette!("Cannot read regular ISO file: {error}"))?;
    let hash = sha256_reader(&mut file)
        .map_err(|error| miette::miette!("Cannot hash ISO file: {error}"))?;
    Ok((hash.digest, hash.size))
}

fn read_bounded(path: &Path, max: u64) -> Result<Vec<u8>> {
    let file = open_regular_file_nofollow(path)
        .map_err(|error| miette::miette!("Cannot read regular ISO document: {error}"))?;
    let mut bytes = Vec::new();
    file.take(max + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| miette::miette!("Cannot read ISO document: {error}"))?;
    if bytes.len() as u64 > max {
        miette::bail!("ISO document exceeds size limit.");
    }
    Ok(bytes)
}

fn run_xorriso(args: &[&str]) -> Result<Output> {
    run_xorriso_command(Command::new("xorriso"), args)
}

fn run_xorriso_in(directory: &Path, args: &[&str]) -> Result<Output> {
    let mut command = Command::new("xorriso");
    command.current_dir(directory);
    run_xorriso_command(command, args)
}

fn run_xorriso_command(mut command: Command, args: &[&str]) -> Result<Output> {
    let output = command
        .arg("-no_rc")
        .args(args)
        .output()
        .map_err(|error| miette::miette!("Cannot execute xorriso ISO backend: {error}"))?;
    if !output.status.success() {
        miette::bail!(
            "xorriso ISO backend failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(output)
}

fn xorriso_version() -> Result<String> {
    let output = run_xorriso(&["-version"])?;
    let text = String::from_utf8(output.stdout)
        .map_err(|_| miette::miette!("xorriso version output is not UTF-8."))?;
    text.lines()
        .find(|line| line.starts_with("xorriso version   :  "))
        .map(str::to_string)
        .ok_or_else(|| miette::miette!("Cannot identify xorriso backend version."))
}

fn xorriso_text(image: &Path, args: &[&str]) -> Result<String> {
    let path = image
        .to_str()
        .ok_or_else(|| miette::miette!("Non-Unicode ISO path."))?;
    let mut command = vec!["-indev", path];
    command.extend_from_slice(args);
    let output = run_xorriso(&command)?;
    String::from_utf8(output.stdout).map_err(|_| miette::miette!("xorriso output is not UTF-8."))
}

fn extract_inventory(
    image: &Path,
    paths: &[String],
) -> Result<BTreeMap<String, (Sha256Digest, u64)>> {
    if paths.len() > 20_001 {
        miette::bail!("ISO extraction exceeds its file limit.");
    }
    let temp = tempfile::tempdir()
        .map_err(|error| miette::miette!("Cannot stage ISO read-back: {error}"))?;
    let image = image
        .to_str()
        .ok_or_else(|| miette::miette!("Non-Unicode ISO path."))?;
    let mut measured = BTreeMap::new();
    for (chunk_index, chunk) in paths.chunks(128).enumerate() {
        let mut args = vec![
            "-osirrox".to_string(),
            "on".to_string(),
            "-indev".to_string(),
            image.to_string(),
        ];
        let mut outputs = Vec::with_capacity(chunk.len());
        for (offset, relative) in chunk.iter().enumerate() {
            validate_iso_path(relative)?;
            if measured.contains_key(relative)
                || outputs
                    .iter()
                    .any(|(path, _): &(String, PathBuf)| path == relative)
            {
                miette::bail!("Duplicate ISO read-back path.");
            }
            let output = temp
                .path()
                .join(format!("f{:05}", chunk_index * 128 + offset));
            args.extend([
                "-extract_single".to_string(),
                format!("/{relative}"),
                output
                    .to_str()
                    .ok_or_else(|| miette::miette!("Non-Unicode ISO extraction path."))?
                    .to_string(),
            ]);
            outputs.push((relative.clone(), output));
        }
        let output = Command::new("xorriso")
            .arg("-no_rc")
            .args(&args)
            .output()
            .map_err(|error| miette::miette!("Cannot execute ISO read-back: {error}"))?;
        if !output.status.success() {
            miette::bail!(
                "ISO read-back failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        for (relative, output) in outputs {
            measured.insert(relative, measure(&output)?);
            fs::remove_file(&output)
                .map_err(|error| miette::miette!("Cannot clear ISO extraction: {error}"))?;
        }
    }
    Ok(measured)
}

#[cfg(test)]
mod tests {
    use super::{stage_iso_media_plan_with_gate, verify_iso_media_artifact};
    use aros_common::media_plan::plan_media_image;
    use aros_common::media_profile::built_in_media_profiles;
    use aros_common::media_receipt::{
        MediaBuildFile, MediaBuildIdentity, MediaBuildReceipt, MediaBuildTree,
    };
    use aros_common::media_tree::measure_media_tree;
    use aros_common::sha256_bytes;
    use std::fs;

    #[test]
    fn complete_pc_sys_tree_qualifies_when_explicitly_provided() {
        let Ok(path) = std::env::var("AROS_TEST_PC_SYS_ROOT") else {
            return;
        };
        let sys = std::path::PathBuf::from(path);
        let build_root = sys.parent().expect("SYS has a build root");
        let profile = built_in_media_profiles()
            .unwrap()
            .into_iter()
            .find(|entry| entry.profile.id == "pc-bios-iso")
            .unwrap();
        let tree = measure_media_tree(&sys).unwrap();
        assert!(
            tree.files.len() > 1000,
            "qualification requires a complete PC SYS tree"
        );
        let files = profile
            .profile
            .required_files
            .iter()
            .map(|required| {
                let path = format!("SYS/{}", required.destination);
                let measured = aros_common::sha256_file(&build_root.join(&path)).unwrap();
                MediaBuildFile {
                    role: required.role.clone(),
                    path,
                    sha256: measured.digest.to_string(),
                    size_bytes: measured.size,
                }
            })
            .collect();
        let receipt = MediaBuildReceipt::new_bound_cmake_with_trees(
            "pc-x86_64".into(),
            "pc".into(),
            "bios-iso".into(),
            MediaBuildIdentity {
                source_commit: "1".repeat(40),
                source_tree: "2".repeat(40),
                toolchain_release_id: "local-qualification".into(),
                toolchain_tree_sha256: sha256_bytes(b"tree"),
                toolchain_manifest_sha256: sha256_bytes(b"manifest"),
            },
            files,
            vec![MediaBuildTree {
                role: "sys-tree".into(),
                path: "SYS".into(),
                sha256: tree.sha256.clone(),
                file_count: tree.files.len(),
                directory_count: tree.directories.len(),
            }],
        )
        .unwrap();
        let receipt = serde_json::to_vec(&receipt).unwrap();
        let plan = plan_media_image(&profile, build_root, &receipt, &[], &[]).unwrap();
        assert_eq!(plan.files.len(), tree.files.len());
        let temp = tempfile::tempdir().unwrap();
        let first =
            stage_iso_media_plan_with_gate(&plan, &temp.path().join("first"), || Ok(())).unwrap();
        let second =
            stage_iso_media_plan_with_gate(&plan, &temp.path().join("second"), || Ok(())).unwrap();
        assert_eq!(first.image.sha256(), second.image.sha256());
        let verified = verify_iso_media_artifact(&first.artifact_dir).unwrap();
        assert_eq!(verified.file_count, tree.files.len() + 1);
        println!(
            "complete PC SYS: files={}, directories={}, tree_sha256={}, iso_bytes={}, iso_sha256={}",
            tree.files.len(),
            tree.directories.len(),
            tree.sha256,
            first.image.size_bytes(),
            first.image.sha256()
        );
    }

    #[test]
    fn iso_is_reproducible_and_independently_read_back() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let sys = source.join("SYS");
        fs::create_dir_all(sys.join("Docs/empty")).unwrap();
        fs::write(sys.join("Docs/readme"), b"readme").unwrap();
        let profile = built_in_media_profiles()
            .unwrap()
            .into_iter()
            .find(|entry| entry.profile.id == "pc-bios-iso")
            .unwrap();
        let mut files = Vec::new();
        for file in &profile.profile.required_files {
            let bytes = vec![0x55; 4096];
            let path = format!("SYS/{}", file.destination);
            fs::create_dir_all(source.join(&path).parent().unwrap()).unwrap();
            fs::write(source.join(&path), &bytes).unwrap();
            files.push(MediaBuildFile {
                role: file.role.clone(),
                path,
                sha256: sha256_bytes(&bytes).to_string(),
                size_bytes: bytes.len() as u64,
            });
        }
        let tree = measure_media_tree(&sys).unwrap();
        let receipt = MediaBuildReceipt::new_bound_cmake_with_trees(
            "pc-x86_64".into(),
            "pc".into(),
            "bios-iso".into(),
            MediaBuildIdentity {
                source_commit: "1".repeat(40),
                source_tree: "2".repeat(40),
                toolchain_release_id: "test".into(),
                toolchain_tree_sha256: sha256_bytes(b"tree"),
                toolchain_manifest_sha256: sha256_bytes(b"manifest"),
            },
            files,
            vec![MediaBuildTree {
                role: "sys-tree".into(),
                path: "SYS".into(),
                sha256: tree.sha256,
                file_count: tree.files.len(),
                directory_count: tree.directories.len(),
            }],
        )
        .unwrap();
        let receipt = serde_json::to_vec(&receipt).unwrap();
        let plan = plan_media_image(&profile, &source, &receipt, &[], &[]).unwrap();
        let first =
            stage_iso_media_plan_with_gate(&plan, &temp.path().join("first"), || Ok(())).unwrap();
        let second =
            stage_iso_media_plan_with_gate(&plan, &temp.path().join("second"), || Ok(())).unwrap();
        assert_eq!(first.image.sha256(), second.image.sha256());
        assert_eq!(
            fs::read(&first.manifest_path).unwrap(),
            fs::read(&second.manifest_path).unwrap()
        );
        let verified = verify_iso_media_artifact(&first.artifact_dir).unwrap();
        assert_eq!(verified.medium, "iso9660-el-torito");
        assert_eq!(verified.file_count, 4); // three SYS files plus generated catalog
        assert!(
            stage_iso_media_plan_with_gate(&plan, &temp.path().join("refused"), || {
                Err(miette::miette!("source identity changed"))
            })
            .is_err()
        );
        assert!(!temp.path().join("refused").exists());
        assert!(stage_iso_media_plan_with_gate(&plan, &first.artifact_dir, || Ok(())).is_err());
        let mut over_capacity = plan.clone();
        over_capacity.total_payload_bytes = u64::MAX;
        assert!(stage_iso_media_plan_with_gate(
            &over_capacity,
            &temp.path().join("over-capacity"),
            || Ok(())
        )
        .is_err());
        assert!(!temp.path().join("over-capacity").exists());
        #[cfg(unix)]
        {
            let symlink = source.join("linked-bootstrap");
            std::os::unix::fs::symlink(sys.join("boot/pc/bootstrap"), &symlink).unwrap();
            let mut linked = plan.clone();
            linked
                .files
                .iter_mut()
                .find(|file| file.role == "bootstrap")
                .unwrap()
                .source_path = symlink;
            assert!(stage_iso_media_plan_with_gate(
                &linked,
                &temp.path().join("linked"),
                || Ok(())
            )
            .is_err());
            assert!(!temp.path().join("linked").exists());
            let nested_link = sys.join("Docs/link");
            std::os::unix::fs::symlink(sys.join("Docs/readme"), &nested_link).unwrap();
            assert!(plan_media_image(&profile, &source, &receipt, &[], &[]).is_err());
            fs::remove_file(nested_link).unwrap();
        }
        fs::write(sys.join("Docs/readme"), b"changed").unwrap();
        assert!(plan_media_image(&profile, &source, &receipt, &[], &[]).is_err());
        fs::write(sys.join("Docs/readme"), b"readme").unwrap();
        fs::write(sys.join("boot/pc/bootstrap"), b"changed").unwrap();
        assert!(
            stage_iso_media_plan_with_gate(&plan, &temp.path().join("changed"), || Ok(())).is_err()
        );
        assert!(!temp.path().join("changed").exists());
        assert!(verify_iso_media_artifact(&second.artifact_dir).is_ok());
        let mut image = fs::read(second.image.path()).unwrap();
        image[100] ^= 0xff;
        fs::write(second.image.path(), image).unwrap();
        assert!(verify_iso_media_artifact(&second.artifact_dir).is_err());
        fs::write(first.artifact_dir.join("extra"), b"extra").unwrap();
        assert!(verify_iso_media_artifact(&first.artifact_dir).is_err());
    }
}
