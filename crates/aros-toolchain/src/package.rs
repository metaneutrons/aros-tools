//! Deterministic native packaging for one completed local toolchain candidate.
//!
//! This module creates a local package directory only.  It has no release
//! credentials, network access, tag mutation, or publication capability.

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

use aros_common::{
    measure_regular_file, payload_casefold_path_key, publish_prepared_tree_noclobber, sha256_file,
    toolchain_tree_inventory, ArosToolchainManifest, Sha256Digest, AROS_TOOLCHAIN_MANIFEST_FILE,
    AROS_TOOLCHAIN_MANIFEST_SCHEMA,
};
use serde_json::{json, Map, Value};
use tempfile::TempDir;
use xz2::write::XzEncoder;

use crate::profiles::Profile;
use crate::source_lock::{SourceLock, SourcePurpose};
use crate::{ContractError, Recipe};

const ARCHIVE_ROOT: &str = "toolchain";
const SPDX_SCHEMA: &str = "SPDX-2.3";
const MAX_PREFIX_FINDINGS: usize = 20;
const SCAN_BUFFER_BYTES: usize = 1024 * 1024;
const SUPPORTED_HOSTS: &[&str] = &[
    "linux-x86_64",
    "linux-aarch64",
    "macos-x86_64",
    "macos-aarch64",
];

/// Explicit inputs to one local deterministic package operation.
///
/// `output_dir` is the final directory of one complete package set. It must
/// not exist; a successful call publishes that directory atomically.
#[derive(Debug, Clone)]
pub struct PackageRequest {
    /// Completed local candidate prefix. It is read only and never mutated.
    pub candidate_root: PathBuf,
    /// Absent final output directory for the complete package set.
    pub output_dir: PathBuf,
    /// Immutable release identifier recorded in the manifest.
    pub release_id: String,
    /// Explicit four-host v1 selector.
    pub host: String,
    /// Validated source recipe.
    pub recipe: Recipe,
    /// Validated source closure that defines the LLVM version and SPDX inputs.
    pub source_lock: SourceLock,
    /// Exact profile selected from the validated profiles document.
    pub profile: Profile,
    /// Build-environment receipt material included in the manifest.
    pub build_environment: Map<String, Value>,
    /// Absolute build roots that must not appear in any packaged regular file.
    pub forbidden_prefixes: Vec<PathBuf>,
}

/// Measured members of one local package set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageOutput {
    /// Atomic package-set directory.
    pub output_dir: PathBuf,
    /// Deterministic `.tar.xz` asset.
    pub archive: PathBuf,
    /// External copy of the embedded manifest.
    pub manifest: PathBuf,
    /// SHA-256 sidecar for the archive.
    pub checksum: PathBuf,
    /// SPDX 2.3 SBOM for the package inputs.
    pub sbom: PathBuf,
    /// Archive hash measured after durable write.
    pub archive_sha256: Sha256Digest,
    /// Archive byte length measured after durable write.
    pub archive_size: u64,
}

/// Package one local candidate into a deterministic, self-describing set.
///
/// The candidate is copied into a private sibling staging tree, normalized
/// there, and scanned before any package output is made visible. The final
/// package directory appears atomically only after its archive, manifest,
/// sidecar and SBOM are all written and synchronized.
///
/// # Errors
///
/// Returns a producer package diagnostic for invalid inputs, unsafe candidate
/// entries, prefix leakage, serialization/write failures, or output conflicts.
pub fn package(request: &PackageRequest) -> Result<PackageOutput, ContractError> {
    validate_request(request)?;
    let parent = request
        .output_dir
        .parent()
        .ok_or_else(|| ContractError::package("package output directory has no parent"))?;
    fs::create_dir_all(parent)
        .map_err(|_| ContractError::package("cannot create package output parent"))?;
    let candidate_stage = temporary(parent, ".aros-toolchain-candidate-")?;
    let package_stage = temporary(parent, ".aros-toolchain-package-")?;
    let staged_root = candidate_stage.path().join(ARCHIVE_ROOT);
    copy_candidate(&request.candidate_root, &staged_root)?;
    remove_embedded_manifest(&staged_root)?;
    scan_prefixes(&staged_root, &request.forbidden_prefixes)?;

    let (tree_sha256, files) = toolchain_tree_inventory(&staged_root).map_err(|error| {
        ContractError::package(format!("cannot inventory staged candidate: {error}"))
    })?;
    let manifest = manifest(request, tree_sha256, files)?;
    let manifest_bytes = pretty_json(&manifest)?;
    write_new(
        &staged_root.join(AROS_TOOLCHAIN_MANIFEST_FILE),
        &manifest_bytes,
    )?;

    let asset = canonical_asset_name(
        request.source_lock.version(),
        &request.host,
        request.profile.name(),
    )?;
    let archive = package_stage.path().join(&asset);
    write_archive(&archive, &staged_root, request.recipe.source_date_epoch())?;
    let measured = sha256_file(&archive)
        .map_err(|_| ContractError::package("cannot measure completed package archive"))?;
    let external_manifest = package_stage.path().join(format!("{asset}.manifest.json"));
    let checksum = package_stage.path().join(format!("{asset}.sha256"));
    let sbom = package_stage.path().join(format!("{asset}.spdx.json"));
    write_new(&external_manifest, &manifest_bytes)?;
    write_new(
        &checksum,
        format!("{}  {asset}\n", measured.digest).as_bytes(),
    )?;
    write_new(&sbom, &spdx_bytes(&request.source_lock, &manifest)?)?;
    sync_tree(package_stage.path())?;

    publish_prepared_tree_noclobber(package_stage.path(), &request.output_dir).map_err(
        |error| ContractError::package(format!("cannot atomically publish package set: {error}")),
    )?;
    let output = PackageOutput {
        output_dir: request.output_dir.clone(),
        archive: request.output_dir.join(&asset),
        manifest: request.output_dir.join(format!("{asset}.manifest.json")),
        checksum: request.output_dir.join(format!("{asset}.sha256")),
        sbom: request.output_dir.join(format!("{asset}.spdx.json")),
        archive_sha256: measured.digest,
        archive_size: measured.size,
    };
    Ok(output)
}

/// Derive the v1 asset name from closed version, host and profile selectors.
///
/// # Errors
///
/// Returns a package diagnostic unless the selectors are part of the v1
/// four-host/three-profile release contract.
pub fn canonical_asset_name(
    llvm_version: &str,
    host: &str,
    target_profile: &str,
) -> Result<String, ContractError> {
    let version_ok = llvm_version.split('.').count() == 3
        && llvm_version
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()));
    if !version_ok || !SUPPORTED_HOSTS.contains(&host) {
        return Err(ContractError::package(
            "package identity has an unsupported LLVM version or host selector",
        ));
    }
    let profile_ok = matches!(target_profile, "pc-x86_64" | "arm-raspi" | "rpi-aarch64");
    if !profile_ok {
        return Err(ContractError::package(
            "package identity has an unsupported target profile selector",
        ));
    }
    Ok(format!(
        "aros-toolchain-v1-llvm{llvm_version}-{host}-{target_profile}.tar.xz"
    ))
}

fn validate_request(request: &PackageRequest) -> Result<(), ContractError> {
    if !request.candidate_root.is_absolute() || !request.output_dir.is_absolute() {
        return Err(ContractError::package(
            "candidate and package output roots must be absolute paths",
        ));
    }
    let metadata = fs::symlink_metadata(&request.candidate_root)
        .map_err(|_| ContractError::package("candidate root is not accessible"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(ContractError::package(
            "candidate root must be a real directory, not a symbolic link",
        ));
    }
    if request.output_dir.exists() {
        return Err(ContractError::package(
            "package output directory already exists and will not be replaced",
        ));
    }
    if !safe_segment(&request.release_id) {
        return Err(ContractError::package(
            "release identifier must be one safe nonempty segment",
        ));
    }
    let _ = canonical_asset_name(
        request.source_lock.version(),
        &request.host,
        request.profile.name(),
    )?;
    let build_environment = Value::Object(request.build_environment.clone());
    crate::canonical::bytes(&build_environment)?;
    for prefix in &request.forbidden_prefixes {
        if !prefix.is_absolute() {
            return Err(ContractError::package(
                "every forbidden package prefix must be absolute",
            ));
        }
    }
    Ok(())
}

fn temporary(parent: &Path, prefix: &str) -> Result<TempDir, ContractError> {
    tempfile::Builder::new()
        .prefix(prefix)
        .tempdir_in(parent)
        .map_err(|_| ContractError::package("cannot reserve package staging directory"))
}

fn copy_candidate(source: &Path, destination: &Path) -> Result<(), ContractError> {
    fs::create_dir(destination)
        .map_err(|_| ContractError::package("cannot create private candidate staging root"))?;
    copy_directory(source, destination, Path::new(""), &mut BTreeSet::new())
}

fn copy_directory(
    source_root: &Path,
    destination_root: &Path,
    relative: &Path,
    portable_paths: &mut BTreeSet<String>,
) -> Result<(), ContractError> {
    let source_directory = source_root.join(relative);
    let mut children = fs::read_dir(&source_directory)
        .map_err(|_| ContractError::package("cannot read candidate directory"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| ContractError::package("cannot enumerate candidate directory"))?;
    children.sort_by_key(fs::DirEntry::file_name);
    for child in children {
        let name = child.file_name();
        let name = name
            .to_str()
            .ok_or_else(|| ContractError::package("candidate path is not UTF-8"))?;
        if name == ".DS_Store" || name.starts_with(".installflag-") {
            continue;
        }
        let child_relative = relative.join(name);
        let folded = payload_casefold_path_key(&child_relative)
            .map_err(|_| ContractError::package("candidate path is not portable"))?;
        if !portable_paths.insert(folded) {
            return Err(ContractError::package(
                "candidate contains a case-folding path collision",
            ));
        }
        let source = child.path();
        let destination = destination_root.join(&child_relative);
        let metadata = fs::symlink_metadata(&source)
            .map_err(|_| ContractError::package("cannot inspect candidate entry"))?;
        if metadata.file_type().is_symlink() {
            copy_symlink(&source, &destination, &child_relative)?;
        } else if metadata.is_dir() {
            fs::create_dir(&destination)
                .map_err(|_| ContractError::package("cannot stage candidate directory"))?;
            set_mode(&destination, 0o755)?;
            copy_directory(
                source_root,
                destination_root,
                &child_relative,
                portable_paths,
            )?;
        } else if metadata.is_file() {
            copy_regular(&source, &destination, &metadata.permissions())?;
        } else {
            return Err(ContractError::package(
                "candidate contains an unsupported filesystem entry",
            ));
        }
    }
    Ok(())
}

fn copy_regular(
    source: &Path,
    destination: &Path,
    permissions: &fs::Permissions,
) -> Result<(), ContractError> {
    let Some((_, content)) = measure_regular_file(source)
        .map_err(|_| ContractError::package("cannot read candidate regular file safely"))?
    else {
        return Err(ContractError::package(
            "candidate regular file changed while it was staged",
        ));
    };
    let mut destination_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|_| ContractError::package("cannot stage candidate regular file"))?;
    destination_file
        .write_all(&content)
        .and_then(|()| destination_file.sync_all())
        .map_err(|_| ContractError::package("cannot write candidate regular file"))?;
    set_mode(
        destination,
        if is_executable(permissions) {
            0o755
        } else {
            0o644
        },
    )
}

#[cfg(unix)]
fn copy_symlink(source: &Path, destination: &Path, relative: &Path) -> Result<(), ContractError> {
    use std::os::unix::fs::symlink;

    let target = fs::read_link(source)
        .map_err(|_| ContractError::package("cannot read candidate symbolic link"))?;
    validate_link_target(relative, &target)?;
    symlink(target, destination)
        .map_err(|_| ContractError::package("cannot stage candidate symbolic link"))
}

#[cfg(not(unix))]
fn copy_symlink(
    _source: &Path,
    _destination: &Path,
    _relative: &Path,
) -> Result<(), ContractError> {
    Err(ContractError::package(
        "native toolchain package staging requires Unix symbolic-link support",
    ))
}

pub(crate) fn validate_link_target(relative: &Path, target: &Path) -> Result<(), ContractError> {
    if target.is_absolute() || target.to_str().is_none() {
        return Err(ContractError::package(
            "candidate symbolic link has an absolute or non-UTF-8 target",
        ));
    }
    let mut depth = relative
        .parent()
        .map_or(0, |parent| parent.components().count());
    for component in target.components() {
        match component {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir if depth > 0 => depth -= 1,
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(ContractError::package(
                    "candidate symbolic link escapes the toolchain root",
                ));
            }
        }
    }
    Ok(())
}

fn remove_embedded_manifest(root: &Path) -> Result<(), ContractError> {
    let path = root.join(AROS_TOOLCHAIN_MANIFEST_FILE);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            fs::remove_file(path).map_err(|_| {
                ContractError::package("cannot remove staged prior toolchain manifest")
            })
        }
        Ok(_) => Err(ContractError::package(
            "candidate manifest path is not a replaceable regular file",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(ContractError::package(
            "cannot inspect staged manifest path",
        )),
    }
}

fn scan_prefixes(root: &Path, prefixes: &[PathBuf]) -> Result<(), ContractError> {
    let needles: Vec<_> = prefixes
        .iter()
        .filter_map(|path| path.to_str().map(|value| (path, value.as_bytes())))
        .filter(|(_, bytes)| !bytes.is_empty())
        .collect();
    let mut findings = Vec::new();
    scan_directory(root, Path::new(""), &needles, &mut findings)?;
    if findings.is_empty() {
        Ok(())
    } else {
        Err(ContractError::package(format!(
            "staged candidate contains forbidden build prefixes: {}",
            findings.join(", ")
        )))
    }
}

fn scan_directory(
    root: &Path,
    relative: &Path,
    needles: &[(&PathBuf, &[u8])],
    findings: &mut Vec<String>,
) -> Result<(), ContractError> {
    let mut entries = fs::read_dir(root.join(relative))
        .map_err(|_| ContractError::package("cannot scan candidate directory"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| ContractError::package("cannot enumerate candidate scan directory"))?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        if findings.len() >= MAX_PREFIX_FINDINGS {
            return Ok(());
        }
        let path = entry.path();
        let child_relative = relative.join(entry.file_name());
        let metadata = fs::symlink_metadata(&path)
            .map_err(|_| ContractError::package("cannot inspect candidate scan entry"))?;
        if metadata.is_dir() {
            scan_directory(root, &child_relative, needles, findings)?;
        } else if metadata.is_file() && !metadata.file_type().is_symlink() {
            scan_file(&path, &child_relative, needles, findings)?;
        }
    }
    Ok(())
}

fn scan_file(
    path: &Path,
    relative: &Path,
    needles: &[(&PathBuf, &[u8])],
    findings: &mut Vec<String>,
) -> Result<(), ContractError> {
    let mut file = File::open(path)
        .map_err(|_| ContractError::package("cannot open candidate file for prefix scan"))?;
    let max_overlap = needles
        .iter()
        .map(|(_, needle)| needle.len())
        .max()
        .unwrap_or(1)
        - 1;
    let mut overlap = Vec::new();
    let mut buffer = vec![0_u8; SCAN_BUFFER_BYTES];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| ContractError::package("cannot scan candidate file"))?;
        if read == 0 {
            return Ok(());
        }
        let mut content = Vec::with_capacity(overlap.len() + read);
        content.extend_from_slice(&overlap);
        content.extend_from_slice(&buffer[..read]);
        for (_, needle) in needles {
            if content
                .windows(needle.len())
                .any(|window| window == *needle)
            {
                findings.push(relative.display().to_string());
                return Ok(());
            }
        }
        let keep = max_overlap.min(content.len());
        overlap = content[content.len() - keep..].to_vec();
    }
}

fn manifest(
    request: &PackageRequest,
    tree_sha256: String,
    files: Vec<aros_common::ArosToolchainManifestEntry>,
) -> Result<ArosToolchainManifest, ContractError> {
    let manifest = ArosToolchainManifest {
        schema: AROS_TOOLCHAIN_MANIFEST_SCHEMA,
        release_id: request.release_id.clone(),
        host: request.host.clone(),
        target_profile: request.profile.name().to_owned(),
        target_triple: request.profile.target_triple().to_owned(),
        tree_sha256,
        llvm_version: Some(request.source_lock.version().to_owned()),
        recipe_sha256: request.recipe.sha256().to_string(),
        source_lock_sha256: request.recipe.source_lock_sha256().to_string(),
        profiles_sha256: request.recipe.profiles_sha256().to_string(),
        source_commit: request.recipe.source().0.as_str().to_owned(),
        producer_commit: request.recipe.producer().0.as_str().to_owned(),
        tools_commit: request.recipe.tools().0.as_str().to_owned(),
        source_date_epoch: request.recipe.source_date_epoch(),
        capabilities: request.profile.capabilities().to_vec(),
        build_environment: request.build_environment.clone(),
        files,
    };
    manifest.validate().map_err(|error| {
        ContractError::package(format!("generated manifest is invalid: {error}"))
    })?;
    Ok(manifest)
}

pub(crate) fn pretty_json<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, ContractError> {
    // The legacy producer uses json.dumps(sort_keys=True, indent=2) with its
    // default ensure_ascii=True.  `serde_json` already represents maps in
    // lexicographic order here; escaping non-ASCII code points after rendering
    // gives the same portable document encoding without invoking Python.
    let rendered = serde_json::to_string_pretty(&serde_json::to_value(value).map_err(|_| {
        ContractError::package("cannot normalize package document for serialization")
    })?)
    .map_err(|_| ContractError::package("cannot serialize package document"))?;
    let mut bytes = Vec::with_capacity(rendered.len() + 1);
    for character in rendered.chars() {
        if character.is_ascii() {
            bytes.push(character as u8);
        } else {
            let mut units = [0_u16; 2];
            for unit in character.encode_utf16(&mut units).iter() {
                bytes.extend_from_slice(format!("\\u{unit:04x}").as_bytes());
            }
        }
    }
    bytes.push(b'\n');
    Ok(bytes)
}

/// Counts uncompressed tar bytes while forwarding them to the deterministic
/// XZ stream. Python's `tarfile` pads a completed archive to its 20-block
/// record boundary; retaining that rule is necessary for legacy byte parity.
struct CountingWriter<W> {
    inner: W,
    written: u64,
}

impl<W> CountingWriter<W> {
    const fn new(inner: W) -> Self {
        Self { inner, written: 0 }
    }

    const fn written(&self) -> u64 {
        self.written
    }

    fn into_inner(self) -> W {
        self.inner
    }
}

impl<W: Write> Write for CountingWriter<W> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(buffer)?;
        self.written = self
            .written
            .checked_add(u64::try_from(written).map_err(io::Error::other)?)
            .ok_or_else(|| io::Error::other("tar output byte count overflow"))?;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn write_archive(path: &Path, root: &Path, epoch: u64) -> Result<(), ContractError> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| ContractError::package("cannot create package archive"))?;
    let mut archive = CountingWriter::new(XzEncoder::new(file, 9));
    append_entry(&mut archive, root, Path::new(""), epoch)?;
    let mut paths = Vec::new();
    collect_paths(root, Path::new(""), &mut paths)?;
    for relative in paths {
        append_entry(&mut archive, root, &relative, epoch)?;
    }
    archive
        .write_all(&[0_u8; 1024])
        .map_err(|_| ContractError::package("cannot terminate package tar stream"))?;
    let remainder = archive.written() % 10_240;
    if remainder != 0 {
        let padding = usize::try_from(10_240 - remainder)
            .map_err(|_| ContractError::package("invalid final tar record padding"))?;
        archive
            .write_all(&[0_u8; 10_240][..padding])
            .map_err(|_| ContractError::package("cannot align package tar record"))?;
    }
    let file = archive
        .into_inner()
        .finish()
        .map_err(|_| ContractError::package("cannot finish package XZ stream"))?;
    file.sync_all()
        .map_err(|_| ContractError::package("cannot synchronize package archive"))
}

fn collect_paths(
    root: &Path,
    relative: &Path,
    paths: &mut Vec<PathBuf>,
) -> Result<(), ContractError> {
    let mut entries = fs::read_dir(root.join(relative))
        .map_err(|_| ContractError::package("cannot enumerate staged package tree"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| ContractError::package("cannot collect staged package tree"))?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let child = relative.join(entry.file_name());
        paths.push(child.clone());
        if fs::symlink_metadata(root.join(&child))
            .map_err(|_| ContractError::package("cannot inspect staged package entry"))?
            .is_dir()
        {
            collect_paths(root, &child, paths)?;
        }
    }
    Ok(())
}

fn append_entry<W: Write>(
    archive: &mut W,
    root: &Path,
    relative: &Path,
    epoch: u64,
) -> Result<(), ContractError> {
    let owned_source = root.join(relative);
    let source = if relative.as_os_str().is_empty() {
        root
    } else {
        owned_source.as_path()
    };
    let metadata = fs::symlink_metadata(source)
        .map_err(|_| ContractError::package("cannot inspect staged archive entry"))?;
    let mut name = if relative.as_os_str().is_empty() {
        format!("{ARCHIVE_ROOT}/")
    } else {
        format!("{ARCHIVE_ROOT}/{}", relative.to_string_lossy())
    };
    let mode = if metadata.file_type().is_symlink() {
        0o777
    } else if metadata.is_dir() {
        0o755
    } else if metadata.is_file() {
        aros_common::normalized_toolchain_file_mode(&metadata)
    } else {
        return Err(ContractError::package(
            "staged package contains an unsupported archive entry",
        ));
    };
    if metadata.is_dir() && !name.ends_with('/') {
        name.push('/');
    }
    if metadata.file_type().is_symlink() {
        let target = fs::read_link(source)
            .map_err(|_| ContractError::package("cannot read staged archive link"))?;
        let target = target
            .to_str()
            .ok_or_else(|| ContractError::package("staged archive link is not UTF-8"))?;
        write_pax_entry(archive, &name, mode, epoch, b'2', target, None)?;
    } else if metadata.is_dir() {
        write_pax_entry(archive, &name, mode, epoch, b'5', "", None)?;
    } else {
        let mut file = File::open(source)
            .map_err(|_| ContractError::package("cannot open staged archive file"))?;
        write_pax_entry(
            archive,
            &name,
            mode,
            epoch,
            b'0',
            "",
            Some((&mut file, metadata.len())),
        )?;
    }
    Ok(())
}

/// Write a PAX-format member with the same field policy as Python's legacy
/// `tarfile.PAX_FORMAT`: paths and link targets which are non-ASCII or exceed
/// their ustar field are recorded in a preceding local extended header.  The
/// abbreviated ustar header remains valid and reproducible; the PAX record is
/// authoritative for extraction.
fn write_pax_entry<W: Write>(
    archive: &mut W,
    name: &str,
    mode: u32,
    epoch: u64,
    kind: u8,
    target: &str,
    payload: Option<(&mut File, u64)>,
) -> Result<(), ContractError> {
    let size = payload.as_ref().map_or(0, |(_, size)| *size);
    let mut extensions = Vec::new();
    if !name.is_ascii() || name.len() > 100 {
        extensions.push(("path", name.as_bytes()));
    }
    if kind == b'2' && (!target.is_ascii() || target.len() > 100) {
        extensions.push(("linkpath", target.as_bytes()));
    }
    if !extensions.is_empty() {
        let mut records = Vec::new();
        for (key, value) in extensions {
            append_pax_record(&mut records, key, value)?;
        }
        write_tar_header(
            archive,
            "././@PaxHeader",
            0,
            0,
            0,
            records.len() as u64,
            0,
            b'x',
            "",
        )?;
        write_tar_payload(archive, &records)?;
    }
    write_tar_header(archive, name, mode, 0, 0, size, epoch, kind, target)?;
    if let Some((file, _)) = payload {
        copy_tar_payload(archive, file, size)?;
    }
    Ok(())
}

fn append_pax_record(output: &mut Vec<u8>, key: &str, value: &[u8]) -> Result<(), ContractError> {
    let base = key
        .len()
        .checked_add(value.len())
        .and_then(|length| length.checked_add(3))
        .ok_or_else(|| ContractError::package("PAX record is too large"))?;
    let mut length = base + 1;
    loop {
        let next = base
            .checked_add(decimal_digits(length))
            .ok_or_else(|| ContractError::package("PAX record is too large"))?;
        if next == length {
            break;
        }
        length = next;
    }
    output.extend_from_slice(length.to_string().as_bytes());
    output.push(b' ');
    output.extend_from_slice(key.as_bytes());
    output.push(b'=');
    output.extend_from_slice(value);
    output.push(b'\n');
    Ok(())
}

const fn decimal_digits(mut value: usize) -> usize {
    let mut digits = 1;
    while value >= 10 {
        value /= 10;
        digits += 1;
    }
    digits
}

#[allow(clippy::too_many_arguments)]
fn write_tar_header<W: Write>(
    output: &mut W,
    name: &str,
    mode: u32,
    uid: u64,
    gid: u64,
    size: u64,
    mtime: u64,
    kind: u8,
    target: &str,
) -> Result<(), ContractError> {
    let mut header = [0_u8; 512];
    write_tar_string(&mut header[0..100], name);
    write_tar_octal(&mut header[100..108], u64::from(mode))?;
    write_tar_octal(&mut header[108..116], uid)?;
    write_tar_octal(&mut header[116..124], gid)?;
    write_tar_octal(&mut header[124..136], size)?;
    write_tar_octal(&mut header[136..148], mtime)?;
    header[148..156].fill(b' ');
    header[156] = kind;
    write_tar_string(&mut header[157..257], target);
    header[257..265].copy_from_slice(b"ustar\x0000");
    // uname, gname, device numbers and prefix stay zero-filled. This matches
    // the legacy TarInfo fields supplied by the producer.
    let checksum: u64 = header.iter().map(|byte| u64::from(*byte)).sum();
    let checksum_text = format!("{checksum:06o}\0");
    header[148..155].copy_from_slice(checksum_text.as_bytes());
    output
        .write_all(&header)
        .map_err(|_| ContractError::package("cannot write package tar header"))
}

fn write_tar_string(field: &mut [u8], value: &str) {
    for (offset, character) in value.chars().enumerate() {
        let encoded = if character.is_ascii() {
            [character as u8, 0, 0, 0]
        } else {
            [b'?', 0, 0, 0]
        };
        if offset >= field.len() {
            break;
        }
        field[offset] = encoded[0];
    }
}

fn write_tar_octal(field: &mut [u8], value: u64) -> Result<(), ContractError> {
    let digits = field
        .len()
        .checked_sub(1)
        .ok_or_else(|| ContractError::package("invalid internal tar numeric field width"))?;
    if value >= 8_u64.saturating_pow(u32::try_from(digits).unwrap_or(u32::MAX)) {
        return Err(ContractError::package(
            "package tar metadata cannot be represented in the v1 PAX header",
        ));
    }
    let encoded = format!("{value:0digits$o}\0");
    field.copy_from_slice(encoded.as_bytes());
    Ok(())
}

fn write_tar_payload<W: Write>(output: &mut W, payload: &[u8]) -> Result<(), ContractError> {
    output
        .write_all(payload)
        .map_err(|_| ContractError::package("cannot write package tar payload"))?;
    write_tar_padding(output, payload.len() as u64)
}

fn copy_tar_payload<W: Write>(
    output: &mut W,
    input: &mut File,
    size: u64,
) -> Result<(), ContractError> {
    let copied = io::copy(input, output)
        .map_err(|_| ContractError::package("cannot write staged package tar payload"))?;
    if copied != size {
        return Err(ContractError::package(
            "staged package file changed while it was archived",
        ));
    }
    write_tar_padding(output, size)
}

fn write_tar_padding<W: Write>(output: &mut W, size: u64) -> Result<(), ContractError> {
    let remainder = size % 512;
    if remainder != 0 {
        let padding = usize::try_from(512 - remainder)
            .map_err(|_| ContractError::package("invalid tar padding length"))?;
        output
            .write_all(&[0_u8; 512][..padding])
            .map_err(|_| ContractError::package("cannot write package tar padding"))?;
    }
    Ok(())
}

pub(crate) fn spdx_bytes(
    source_lock: &SourceLock,
    manifest: &ArosToolchainManifest,
) -> Result<Vec<u8>, ContractError> {
    let mut packages = vec![json!({
        "SPDXID": "SPDXRef-Package-AROSToolchain",
        "name": format!("AROS {} toolchain", manifest.target_profile),
        "versionInfo": manifest.release_id,
        "downloadLocation": "NOASSERTION",
        "filesAnalyzed": false,
        "checksums": [{"algorithm": "SHA256", "checksumValue": manifest.tree_sha256}],
        "licenseConcluded": "NOASSERTION", "licenseDeclared": "NOASSERTION", "copyrightText": "NOASSERTION"
    })];
    let mut relationships = Vec::new();
    for (index, source) in source_lock.source_components().enumerate() {
        let identifier = format!("SPDXRef-Source-{}", index + 1);
        let payload = source.payload();
        packages.push(json!({
            "SPDXID": identifier, "name": source.component(), "versionInfo": source.version(),
            "downloadLocation": payload.url(), "filesAnalyzed": false,
            "checksums": [{"algorithm": "SHA256", "checksumValue": payload.sha256().as_str()}],
            "licenseConcluded": "NOASSERTION", "licenseDeclared": "NOASSERTION", "copyrightText": "NOASSERTION"
        }));
        let relationship = match source.purpose() {
            SourcePurpose::ToolchainComponent => "GENERATED_FROM",
            SourcePurpose::TargetBuildDependency => "BUILD_DEPENDENCY_OF",
        };
        let (left, right) = if relationship == "GENERATED_FROM" {
            ("SPDXRef-Package-AROSToolchain", identifier.as_str())
        } else {
            (identifier.as_str(), "SPDXRef-Package-AROSToolchain")
        };
        relationships.push(json!({"spdxElementId": left, "relationshipType": relationship, "relatedSpdxElement": right}));
    }
    for (index, package) in source_lock.host_python_packages().iter().enumerate() {
        let identifier = format!("SPDXRef-HostPython-{}", index + 1);
        let payload = package.payload();
        packages.push(json!({
            "SPDXID": identifier, "name": package.name(), "versionInfo": package.version(),
            "downloadLocation": payload.url(), "filesAnalyzed": false,
            "checksums": [{"algorithm": "SHA256", "checksumValue": payload.sha256().as_str()}],
            "licenseConcluded": "NOASSERTION", "licenseDeclared": "NOASSERTION", "copyrightText": "NOASSERTION"
        }));
        relationships.push(json!({"spdxElementId": identifier, "relationshipType": "BUILD_DEPENDENCY_OF", "relatedSpdxElement": "SPDXRef-Package-AROSToolchain"}));
    }
    pretty_json(&json!({
        "spdxVersion": SPDX_SCHEMA, "dataLicense": "CC0-1.0", "SPDXID": "SPDXRef-DOCUMENT",
        "name": format!("AROS-toolchain-{}", &manifest.tree_sha256[..16]),
        "documentNamespace": format!("https://github.com/metaneutrons/aros-toolchains/toolchain/{}", manifest.tree_sha256),
        "creationInfo": {"created": rfc3339(manifest.source_date_epoch)?, "creators": ["Tool: aros-toolchain-producer-2"]},
        "documentDescribes": ["SPDXRef-Package-AROSToolchain"], "packages": packages, "relationships": relationships
    }))
}

fn rfc3339(seconds: u64) -> Result<String, ContractError> {
    let seconds = i64::try_from(seconds).map_err(|_| {
        ContractError::package("source date epoch is outside the supported timestamp range")
    })?;
    let days = seconds.div_euclid(86_400);
    let time = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    if !(0..=9_999).contains(&year) {
        return Err(ContractError::package(
            "source date epoch is outside the SPDX timestamp range",
        ));
    }
    Ok(format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        time / 3_600,
        (time % 3_600) / 60,
        time % 60
    ))
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let doe = days - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let day_of_year = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    (year + i64::from(month <= 2), month, day)
}

fn write_new(path: &Path, content: &[u8]) -> Result<(), ContractError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| ContractError::package("cannot create package output member"))?;
    file.write_all(content)
        .and_then(|()| file.sync_all())
        .map_err(|_| ContractError::package("cannot write package output member"))?;
    set_mode(path, 0o644)
}

#[cfg(unix)]
fn is_executable(permissions: &fs::Permissions) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    permissions.mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(permissions: &fs::Permissions) -> bool {
    !permissions.readonly()
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<(), ContractError> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|_| ContractError::package("cannot normalize staged candidate mode"))
}

#[cfg(not(unix))]
fn set_mode(path: &Path, _mode: u32) -> Result<(), ContractError> {
    let mut permissions = fs::metadata(path)
        .map_err(|_| ContractError::package("cannot inspect staged candidate mode"))?
        .permissions();
    permissions.set_readonly(false);
    fs::set_permissions(path, permissions)
        .map_err(|_| ContractError::package("cannot normalize staged candidate mode"))
}

fn sync_tree(root: &Path) -> Result<(), ContractError> {
    for entry in fs::read_dir(root)
        .map_err(|_| ContractError::package("cannot synchronize package staging"))?
    {
        let path = entry
            .map_err(|_| ContractError::package("cannot synchronize package staging"))?
            .path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|_| ContractError::package("cannot synchronize package staging"))?;
        if metadata.is_file() && !metadata.file_type().is_symlink() {
            File::open(path)
                .and_then(|file| file.sync_all())
                .map_err(|_| ContractError::package("cannot synchronize package member"))?;
        }
    }
    File::open(root)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| ContractError::package("cannot synchronize package staging directory"))
}

fn safe_segment(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    use aros_common::sha256_bytes;
    use serde_json::json;

    use crate::profiles::Profiles;

    fn signed_recipe() -> Recipe {
        let mut value = json!({
            "schema": "aros-toolchain-recipe-v2",
            "source_commit": "1".repeat(40), "source_tree": "2".repeat(40),
            "producer_commit": "3".repeat(40), "producer_tree": "4".repeat(40),
            "tools_commit": "5".repeat(40), "tools_tree": "6".repeat(40),
            "source_date_epoch": 946_684_800_u64,
            "source_lock_sha256": "a".repeat(64), "profiles_sha256": "b".repeat(64),
            "patches": []
        });
        value["recipe_sha256"] = json!(sha256_bytes(&crate::canonical::bytes(&value).unwrap()));
        Recipe::parse(&serde_json::to_vec(&value).unwrap()).unwrap()
    }

    fn source_lock() -> SourceLock {
        SourceLock::parse(
            &serde_json::to_vec(&json!({
                "schema": "aros-toolchain-source-lock-v2", "family": "llvm", "version": "11.0.0",
                "sources": [{
                    "component": "llvm", "version": "11.0.0", "purpose": "toolchain-component",
                    "filename": "llvm.tar.xz", "url": "https://example.invalid/llvm.tar.xz",
                    "sha256": "c".repeat(64), "size": 1
                }],
                "host_python_packages": [{
                    "name": "mako", "version": "1.3.10", "filename": "mako.tar.gz",
                    "url": "https://example.invalid/mako.tar.gz", "sha256": "d".repeat(64), "size": 1,
                    "source_root": "mako", "python_path": "."
                }]
            }))
            .unwrap(),
        )
        .unwrap()
    }

    fn profile() -> Profile {
        Profiles::parse(
            &serde_json::to_vec(&json!({
                "schema": "aros-toolchain-profiles-v1", "upstream_commit": "4".repeat(40),
                "profiles": [{
                    "name": "pc-x86_64", "configure_target": "pc-x86_64",
                    "upstream_output_target": "pc-x86_64", "target_triple": "x86_64-unknown-aros",
                    "cpu": "x86_64", "platform": "pc", "float_abi": "",
                    "capabilities": ["c"]
                }]
            }))
            .unwrap(),
        )
        .unwrap()
        .select("pc-x86_64")
        .unwrap()
        .clone()
    }

    #[cfg(unix)]
    fn write_fixture_candidate(root: &Path) {
        use std::os::unix::fs::{symlink, PermissionsExt as _};

        fs::create_dir_all(root.join("share/Größe")).unwrap();
        fs::set_permissions(root.join("share"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(root.join("share/Größe"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(
            root.join("share/Größe/marker-ä.txt"),
            b"AROS tree fixture\n",
        )
        .unwrap();
        fs::set_permissions(
            root.join("share/Größe/marker-ä.txt"),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        symlink("Größe/marker-ä.txt", root.join("share/vector-link")).unwrap();
    }

    #[test]
    fn pretty_json_matches_the_legacy_ascii_manifest_vector() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../scripts/fixtures/toolchain-producer/package-v1.json"
        ))
        .unwrap();
        let bytes = pretty_json(&fixture["manifest"]).unwrap();
        assert_eq!(sha256_bytes(&bytes).as_str(), fixture["manifest_sha256"]);
        assert!(String::from_utf8(bytes)
            .unwrap()
            .contains("Gr\\u00f6\\u00dfe"));
    }

    #[cfg(unix)]
    #[test]
    fn tar_xz_matches_the_legacy_utf8_pax_known_answer() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../scripts/fixtures/toolchain-producer/package-v1.json"
        ))
        .unwrap();
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("toolchain");
        fs::create_dir(&root).unwrap();
        write_fixture_candidate(&root);
        write_new(
            &root.join(AROS_TOOLCHAIN_MANIFEST_FILE),
            &pretty_json(&fixture["manifest"]).unwrap(),
        )
        .unwrap();
        let archive = temporary.path().join("fixture.tar.xz");
        write_archive(&archive, &root, 946_684_800).unwrap();
        let measured = sha256_file(&archive).unwrap();
        assert_eq!(measured.size, fixture["archive_size"].as_u64().unwrap());
        assert_eq!(measured.digest.as_str(), fixture["archive_sha256"]);
    }

    #[test]
    fn selects_every_v1_asset_name_and_rejects_unclosed_selectors() {
        for host in SUPPORTED_HOSTS {
            for profile in ["pc-x86_64", "arm-raspi", "rpi-aarch64"] {
                assert_eq!(
                    canonical_asset_name("11.0.0", host, profile).unwrap(),
                    format!("aros-toolchain-v1-llvm11.0.0-{host}-{profile}.tar.xz")
                );
            }
        }
        assert!(canonical_asset_name("11.0", "linux-x86_64", "pc-x86_64").is_err());
        assert!(canonical_asset_name("11.0.0", "freebsd-x86_64", "pc-x86_64").is_err());
        assert!(canonical_asset_name("11.0.0", "linux-x86_64", "unknown").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn package_is_deterministic_nonmutating_and_removes_legacy_noise() {
        let temporary = tempfile::tempdir().unwrap();
        let candidate = temporary.path().join("candidate");
        fs::create_dir(&candidate).unwrap();
        write_fixture_candidate(&candidate);
        fs::write(candidate.join(".DS_Store"), b"host noise").unwrap();
        fs::write(candidate.join(".installflag-codesign"), b"host noise").unwrap();
        let request = |output_dir| PackageRequest {
            candidate_root: candidate.clone(),
            output_dir,
            release_id: "fixture-release".into(),
            host: "linux-x86_64".into(),
            recipe: signed_recipe(),
            source_lock: source_lock(),
            profile: profile(),
            build_environment: Map::new(),
            forbidden_prefixes: vec![],
        };
        let first = package(&request(temporary.path().join("first"))).unwrap();
        let second = package(&request(temporary.path().join("second"))).unwrap();
        for (left, right) in [
            (&first.archive, &second.archive),
            (&first.manifest, &second.manifest),
            (&first.checksum, &second.checksum),
            (&first.sbom, &second.sbom),
        ] {
            assert_eq!(fs::read(left).unwrap(), fs::read(right).unwrap());
        }
        assert!(!candidate.join(AROS_TOOLCHAIN_MANIFEST_FILE).exists());
        assert!(candidate.join(".DS_Store").exists());
        assert!(candidate.join(".installflag-codesign").exists());
        let manifest: ArosToolchainManifest =
            serde_json::from_slice(&fs::read(first.manifest).unwrap()).unwrap();
        manifest.validate().unwrap();
        assert!(manifest
            .files
            .iter()
            .all(|entry| !entry.path.starts_with('.')));
        let verification = crate::package_verify::PackageVerificationRequest {
            package_dir: first.output_dir.clone(),
            release_id: "fixture-release".into(),
            host: "linux-x86_64".into(),
            recipe: signed_recipe(),
            source_lock: source_lock(),
            profile: profile(),
            build_environment: Map::new(),
            forbidden_prefixes: vec![],
        };
        let verified = crate::package_verify::verify(&verification).unwrap();
        assert_eq!(verified.manifest, manifest);
        let mut wrong_release = verification.clone();
        wrong_release.release_id = "other-release".into();
        assert!(crate::package_verify::verify(&wrong_release).is_err());
        let checksum = fs::read(&first.checksum).unwrap();
        fs::write(&first.checksum, b"wrong checksum\n").unwrap();
        assert!(crate::package_verify::verify(&verification).is_err());
        fs::write(&first.checksum, checksum).unwrap();
        let sbom = fs::read(&first.sbom).unwrap();
        fs::write(&first.sbom, b"{}\n").unwrap();
        assert!(crate::package_verify::verify(&verification).is_err());
        fs::write(&first.sbom, sbom).unwrap();
        fs::write(first.output_dir.join("unexpected"), b"extra outer asset").unwrap();
        assert!(crate::package_verify::verify(&verification).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn package_rejects_cross_chunk_prefixes_case_collisions_and_existing_outputs() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("candidate");
        fs::create_dir(&root).unwrap();
        fs::write(
            root.join("prefix"),
            [vec![b'x'; SCAN_BUFFER_BYTES - 5], b"/work/build".to_vec()].concat(),
        )
        .unwrap();
        assert!(scan_prefixes(&root, &[PathBuf::from("/work/build")]).is_err());

        assert_eq!(
            payload_casefold_path_key(Path::new("Foo")).unwrap(),
            payload_casefold_path_key(Path::new("foo")).unwrap()
        );
        fs::write(root.join("Foo"), b"one").unwrap();
        let request = PackageRequest {
            candidate_root: root,
            output_dir: temporary.path().join("already-there"),
            release_id: "fixture-release".into(),
            host: "linux-x86_64".into(),
            recipe: signed_recipe(),
            source_lock: source_lock(),
            profile: profile(),
            build_environment: Map::new(),
            forbidden_prefixes: vec![],
        };
        package(&request).unwrap();
        assert!(package(&request).is_err());
    }
}
