//! Bounded read-back verification for one native toolchain package set.
//!
//! This module has no extraction or installation side effects. It reads the
//! completed package in place, validates every outer member, and recomputes
//! the embedded payload inventory directly from the bounded tar stream.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use aros_common::{
    finish_sha256, payload_casefold_path_key, sha256_file, toolchain_inventory_sha256,
    ArosToolchainManifest, ArosToolchainManifestEntry, Sha256Digest, AROS_TOOLCHAIN_MANIFEST_FILE,
};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use xz2::read::XzDecoder;

use crate::package::{canonical_asset_name, spdx_bytes, validate_link_target};
use crate::profiles::Profile;
use crate::source_lock::SourceLock;
use crate::{ContractError, Recipe};

const ARCHIVE_ROOT: &str = "toolchain";
const MAX_ARCHIVE_BYTES: u64 = 32 * 1024 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: u64 = 500_000;
const MAX_EXPANDED_ARCHIVE_BYTES: u64 = 32 * 1024 * 1024 * 1024;
const MAX_XZ_DECODER_MEMORY: u64 = 512 * 1024 * 1024;
const MAX_METADATA_BYTES: u64 = 16 * 1024 * 1024;
const SCAN_BUFFER_BYTES: usize = 128 * 1024;

/// Closed expected identity for one package-set read-back verification.
#[derive(Debug, Clone)]
pub struct PackageVerificationRequest {
    /// Directory containing exactly one archive, manifest, checksum and SBOM.
    pub package_dir: PathBuf,
    /// Immutable release identifier expected in the manifest.
    pub release_id: String,
    /// Explicit four-host v1 selector.
    pub host: String,
    /// Validated source recipe.
    pub recipe: Recipe,
    /// Validated exact source closure.
    pub source_lock: SourceLock,
    /// Exact selected profile.
    pub profile: Profile,
    /// Expected build-environment receipt material.
    pub build_environment: Map<String, Value>,
    /// Absolute build roots forbidden in archive regular-file contents.
    pub forbidden_prefixes: Vec<PathBuf>,
}

/// Measured read-back result for one verified package set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedPackage {
    /// Parsed external manifest, proven byte-identical to its embedded copy.
    pub manifest: ArosToolchainManifest,
    /// Measured archive SHA-256.
    pub archive_sha256: Sha256Digest,
    /// Measured archive byte length.
    pub archive_size: u64,
}

/// Verify one complete native package set without extracting or installing it.
///
/// # Errors
///
/// Returns AX0602 for unsafe outer assets, malformed or resource-exhausting
/// archives, identity mismatches, noncanonical headers, inventory divergence,
/// or an inconsistent manifest, checksum, or SPDX document.
pub fn verify(request: &PackageVerificationRequest) -> Result<VerifiedPackage, ContractError> {
    validate_request(request)?;
    let asset = canonical_asset_name(
        request.source_lock.version(),
        &request.host,
        request.profile.name(),
    )?;
    let archive = request.package_dir.join(&asset);
    let manifest_path = request.package_dir.join(format!("{asset}.manifest.json"));
    let checksum_path = request.package_dir.join(format!("{asset}.sha256"));
    let sbom_path = request.package_dir.join(format!("{asset}.spdx.json"));
    let expected_members = [
        asset.clone(),
        format!("{asset}.manifest.json"),
        format!("{asset}.sha256"),
        format!("{asset}.spdx.json"),
    ];
    require_exact_outer_members(&request.package_dir, expected_members.iter())?;

    let (archive_size, archive_sha256) = measure_archive(&archive)?;
    let expected_checksum = format!("{archive_sha256}  {asset}\n");
    if read_metadata(&checksum_path, "checksum sidecar")? != expected_checksum.as_bytes() {
        return Err(ContractError::verification(
            "package checksum sidecar does not match the measured archive",
        ));
    }
    let manifest_bytes = read_metadata(&manifest_path, "external manifest")?;
    let manifest: ArosToolchainManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|_| ContractError::verification("external package manifest is not valid JSON"))?;
    manifest.validate().map_err(|_| {
        ContractError::verification("external package manifest violates the v1 contract")
    })?;
    verify_manifest_identity(&manifest, request)?;
    let expected_sbom = spdx_bytes(&request.source_lock, &manifest)?;
    if read_metadata(&sbom_path, "SPDX SBOM")? != expected_sbom {
        return Err(ContractError::verification(
            "package SPDX SBOM does not match the closed source and manifest inputs",
        ));
    }

    let archive_result = verify_archive_tree(&archive, &manifest, &request.forbidden_prefixes)?;
    if archive_result.embedded_manifest != manifest_bytes {
        return Err(ContractError::verification(
            "embedded and external package manifests are not byte-identical",
        ));
    }
    let inventory_matches = archive_result.entries == manifest.files;
    let digest_matches = archive_result.tree_sha256 == manifest.tree_sha256;
    if !inventory_matches || !digest_matches {
        return Err(ContractError::verification(
            "package archive payload inventory does not match its manifest",
        ));
    }
    Ok(VerifiedPackage {
        manifest,
        archive_sha256,
        archive_size,
    })
}

fn validate_request(request: &PackageVerificationRequest) -> Result<(), ContractError> {
    if !request.package_dir.is_absolute() {
        return Err(ContractError::verification(
            "package verification directory must be absolute",
        ));
    }
    let metadata = fs::symlink_metadata(&request.package_dir).map_err(|_| {
        ContractError::verification("package verification directory is inaccessible")
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(ContractError::verification(
            "package verification directory must be a real directory",
        ));
    }
    request.source_lock.verify_recipe_patches(&request.recipe)?;
    for prefix in &request.forbidden_prefixes {
        if !prefix.is_absolute() {
            return Err(ContractError::verification(
                "every forbidden verification prefix must be absolute",
            ));
        }
    }
    Ok(())
}

fn require_exact_outer_members<'a>(
    directory: &Path,
    expected: impl IntoIterator<Item = &'a String>,
) -> Result<(), ContractError> {
    let expected = expected.into_iter().cloned().collect::<BTreeSet<_>>();
    let mut actual = BTreeSet::new();
    for entry in fs::read_dir(directory)
        .map_err(|_| ContractError::verification("cannot enumerate package directory"))?
    {
        let entry = entry
            .map_err(|_| ContractError::verification("cannot read package directory member"))?;
        let file_name = entry
            .file_name()
            .into_string()
            .map_err(|_| ContractError::verification("package directory member is not UTF-8"))?;
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|_| ContractError::verification("cannot inspect package directory member"))?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(ContractError::verification(
                "package directory contains a non-regular outer member",
            ));
        }
        actual.insert(file_name);
    }
    if actual != expected {
        return Err(ContractError::verification(
            "package directory does not contain exactly the required package members",
        ));
    }
    Ok(())
}

fn measure_archive(path: &Path) -> Result<(u64, Sha256Digest), ContractError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| ContractError::verification("cannot inspect package archive"))?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > MAX_ARCHIVE_BYTES
    {
        return Err(ContractError::verification(
            "package archive is not a regular file within the configured resource limit",
        ));
    }
    let result = sha256_file(path)
        .map_err(|_| ContractError::verification("cannot measure package archive"))?;
    if result.size != metadata.len() {
        return Err(ContractError::verification(
            "package archive changed while it was measured",
        ));
    }
    Ok((result.size, result.digest))
}

fn read_metadata(path: &Path, kind: &str) -> Result<Vec<u8>, ContractError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| ContractError::verification(format!("cannot inspect {kind}")))?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > MAX_METADATA_BYTES
    {
        return Err(ContractError::verification(format!(
            "{kind} is not a regular file within the configured resource limit"
        )));
    }
    let content =
        fs::read(path).map_err(|_| ContractError::verification(format!("cannot read {kind}")))?;
    if u64::try_from(content.len()).ok() != Some(metadata.len()) {
        return Err(ContractError::verification(format!(
            "{kind} changed while it was read"
        )));
    }
    Ok(content)
}

fn verify_manifest_identity(
    manifest: &ArosToolchainManifest,
    request: &PackageVerificationRequest,
) -> Result<(), ContractError> {
    let expected = (
        &request.release_id,
        &request.host,
        request.profile.name(),
        request.profile.target_triple(),
        request.source_lock.version(),
        request.recipe.sha256().as_str(),
        request.recipe.source_lock_sha256().as_str(),
        request.recipe.profiles_sha256().as_str(),
        request.recipe.source().0.as_str(),
        request.recipe.producer().0.as_str(),
        request.recipe.tools().0.as_str(),
        request.recipe.source_date_epoch(),
    );
    let actual = (
        &manifest.release_id,
        &manifest.host,
        manifest.target_profile.as_str(),
        manifest.target_triple.as_str(),
        manifest.llvm_version.as_deref().unwrap_or_default(),
        manifest.recipe_sha256.as_str(),
        manifest.source_lock_sha256.as_str(),
        manifest.profiles_sha256.as_str(),
        manifest.source_commit.as_str(),
        manifest.producer_commit.as_str(),
        manifest.tools_commit.as_str(),
        manifest.source_date_epoch,
    );
    if actual != expected
        || manifest.capabilities != request.profile.capabilities()
        || manifest.build_environment != request.build_environment
    {
        return Err(ContractError::verification(
            "package manifest identity is not bound to the selected producer inputs",
        ));
    }
    Ok(())
}

struct ArchiveTree {
    embedded_manifest: Vec<u8>,
    entries: Vec<ArosToolchainManifestEntry>,
    tree_sha256: String,
}

fn verify_archive_tree(
    path: &Path,
    manifest: &ArosToolchainManifest,
    forbidden_prefixes: &[PathBuf],
) -> Result<ArchiveTree, ContractError> {
    let input =
        File::open(path).map_err(|_| ContractError::verification("cannot open package archive"))?;
    let stream = xz2::stream::Stream::new_stream_decoder(MAX_XZ_DECODER_MEMORY, 0)
        .map_err(|_| ContractError::verification("cannot initialize bounded package XZ decoder"))?;
    let decoder = XzDecoder::new_stream(input, stream);
    let mut archive = tar::Archive::new(decoder);
    let mut entries = Vec::new();
    let mut seen = BTreeSet::new();
    let mut previous = None;
    let mut embedded_manifest = None;
    let mut budget = ArchiveBudget::default();
    for (index, entry) in archive
        .entries()
        .map_err(|_| ContractError::verification("cannot read package tar stream"))?
        .enumerate()
    {
        let mut entry = entry.map_err(|_| {
            ContractError::verification("package tar stream contains an invalid entry")
        })?;
        let header = entry.header().clone();
        let entry_type = header.entry_type();
        let size = header
            .size()
            .map_err(|_| ContractError::verification("package tar entry has an invalid size"))?;
        budget.account(entry_type.as_byte() == b'0', size)?;
        let path = entry
            .path()
            .map_err(|_| ContractError::verification("package tar entry has an invalid path"))?;
        let (relative, is_root) = package_relative_path(&path)?;
        if index == 0 {
            if !is_root || entry_type.as_byte() != b'5' {
                return Err(ContractError::verification(
                    "package tar must begin with the toolchain root directory",
                ));
            }
            verify_header(&header, manifest.source_date_epoch, Some(0o755), 0)?;
            continue;
        }
        if is_root {
            return Err(ContractError::verification(
                "package tar contains a duplicate toolchain root directory",
            ));
        }
        let relative = relative.ok_or_else(|| {
            ContractError::verification("package tar entry has no relative payload path")
        })?;
        let relative_text = relative
            .to_str()
            .ok_or_else(|| ContractError::verification("package tar entry path is not UTF-8"))?
            .replace('\\', "/");
        if previous
            .as_ref()
            .is_some_and(|previous: &String| previous >= &relative_text)
        {
            return Err(ContractError::verification(
                "package tar members are not in strict lexical order",
            ));
        }
        previous = Some(relative_text.clone());
        let collision = payload_casefold_path_key(&relative)
            .map_err(|_| ContractError::verification("package tar entry path is not portable"))?;
        if !seen.insert(collision) {
            return Err(ContractError::verification(
                "package tar contains a duplicate or case-folding path collision",
            ));
        }
        match entry_type.as_byte() {
            b'5' => {
                verify_header(&header, manifest.source_date_epoch, Some(0o755), 0)?;
                entries.push(directory_entry(relative_text));
            }
            b'2' => {
                verify_header(&header, manifest.source_date_epoch, Some(0o777), 0)?;
                let target = entry
                    .link_name()
                    .map_err(|_| {
                        ContractError::verification("package tar symlink has an invalid target")
                    })?
                    .ok_or_else(|| {
                        ContractError::verification("package tar symlink has no target")
                    })?;
                validate_link_target(&relative, &target)?;
                let target = target.to_str().ok_or_else(|| {
                    ContractError::verification("package tar symlink target is not UTF-8")
                })?;
                entries.push(ArosToolchainManifestEntry {
                    path: relative_text,
                    mode: "0777".into(),
                    kind: "symlink".into(),
                    sha256: None,
                    size: None,
                    target: Some(target.into()),
                });
            }
            b'0' => {
                verify_header(&header, manifest.source_date_epoch, None, size)?;
                if relative == Path::new(AROS_TOOLCHAIN_MANIFEST_FILE) {
                    if embedded_manifest.is_some() || size > MAX_METADATA_BYTES {
                        return Err(ContractError::verification(
                            "package tar embedded manifest is missing, duplicated, or too large",
                        ));
                    }
                    let bytes = read_exact_entry(&mut entry, size)?;
                    embedded_manifest = Some(bytes);
                } else {
                    let mode = header.mode().map_err(|_| {
                        ContractError::verification("package tar entry has an invalid mode")
                    })?;
                    let mode = if mode & 0o111 == 0 { "0644" } else { "0755" };
                    let sha256 = hash_entry(&mut entry, size, forbidden_prefixes)?;
                    entries.push(ArosToolchainManifestEntry {
                        path: relative_text,
                        mode: mode.into(),
                        kind: "file".into(),
                        sha256: Some(sha256.to_string()),
                        size: Some(size),
                        target: None,
                    });
                }
            }
            _ => {
                return Err(ContractError::verification(
                    "package tar contains an unsupported entry type",
                ));
            }
        }
    }
    let embedded_manifest = embedded_manifest.ok_or_else(|| {
        ContractError::verification("package tar does not contain an embedded manifest")
    })?;
    let tree_sha256 = toolchain_inventory_sha256(&entries)
        .map_err(|_| ContractError::verification("cannot serialize package tar inventory"))?;
    Ok(ArchiveTree {
        embedded_manifest,
        entries,
        tree_sha256,
    })
}

fn package_relative_path(path: &Path) -> Result<(Option<PathBuf>, bool), ContractError> {
    let components = path.components().collect::<Vec<_>>();
    if components.is_empty()
        || components
            .iter()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(ContractError::verification(
            "package tar entry path is not a safe relative path",
        ));
    }
    let Component::Normal(root) = components[0] else {
        return Err(ContractError::verification(
            "package tar entry path is not a safe relative path",
        ));
    };
    if root != ARCHIVE_ROOT {
        return Err(ContractError::verification(
            "package tar entry is outside the toolchain root",
        ));
    }
    if components.len() == 1 {
        return Ok((None, true));
    }
    let mut relative = PathBuf::new();
    for component in components.into_iter().skip(1) {
        let Component::Normal(component) = component else {
            return Err(ContractError::verification(
                "package tar entry path is not a safe relative path",
            ));
        };
        relative.push(component);
    }
    Ok((Some(relative), false))
}

fn verify_header(
    header: &tar::Header,
    epoch: u64,
    expected_mode: Option<u32>,
    expected_size: u64,
) -> Result<(), ContractError> {
    let mode = header
        .mode()
        .map_err(|_| ContractError::verification("package tar entry has an invalid mode"))?;
    let uid = header
        .uid()
        .map_err(|_| ContractError::verification("package tar entry has an invalid owner"))?;
    let gid = header
        .gid()
        .map_err(|_| ContractError::verification("package tar entry has an invalid group"))?;
    let mtime = header
        .mtime()
        .map_err(|_| ContractError::verification("package tar entry has an invalid timestamp"))?;
    let size = header
        .size()
        .map_err(|_| ContractError::verification("package tar entry has an invalid size"))?;
    if expected_mode.is_some_and(|expected| mode != expected)
        || (!expected_mode.is_some_and(|_| true) && !matches!(mode, 0o644 | 0o755))
        || uid != 0
        || gid != 0
        || mtime != epoch
        || size != expected_size
    {
        return Err(ContractError::verification(
            "package tar header does not use canonical v1 metadata",
        ));
    }
    Ok(())
}

fn directory_entry(path: String) -> ArosToolchainManifestEntry {
    ArosToolchainManifestEntry {
        path,
        mode: "0755".into(),
        kind: "directory".into(),
        sha256: None,
        size: None,
        target: None,
    }
}

fn read_exact_entry<R: Read>(entry: &mut R, expected_size: u64) -> Result<Vec<u8>, ContractError> {
    let capacity = usize::try_from(expected_size).map_err(|_| {
        ContractError::verification("package metadata size exceeds addressable memory")
    })?;
    let mut output = Vec::with_capacity(capacity);
    entry
        .read_to_end(&mut output)
        .map_err(|_| ContractError::verification("cannot read package metadata entry"))?;
    if u64::try_from(output.len()).ok() != Some(expected_size) {
        return Err(ContractError::verification(
            "package metadata entry is truncated or changed while it was read",
        ));
    }
    Ok(output)
}

fn hash_entry<R: Read>(
    entry: &mut R,
    expected_size: u64,
    forbidden_prefixes: &[PathBuf],
) -> Result<Sha256Digest, ContractError> {
    let needles = forbidden_prefixes
        .iter()
        .filter_map(|prefix| prefix.to_str())
        .filter(|prefix| !prefix.is_empty())
        .map(str::as_bytes)
        .collect::<Vec<_>>();
    let max_overlap = needles.iter().map(|needle| needle.len()).max().unwrap_or(1) - 1;
    let mut buffer = vec![0_u8; SCAN_BUFFER_BYTES].into_boxed_slice();
    let mut overlap = Vec::new();
    let mut read_total = 0_u64;
    let mut digest = Sha256::new();
    loop {
        let read = entry
            .read(&mut buffer)
            .map_err(|_| ContractError::verification("cannot read package regular-file entry"))?;
        if read == 0 {
            break;
        }
        read_total = read_total
            .checked_add(u64::try_from(read).map_err(io::Error::other).map_err(|_| {
                ContractError::verification("package regular-file byte count overflowed")
            })?)
            .ok_or_else(|| {
                ContractError::verification("package regular-file byte count overflowed")
            })?;
        if read_total > expected_size {
            return Err(ContractError::verification(
                "package regular-file entry exceeds its declared size",
            ));
        }
        digest.update(&buffer[..read]);
        if !needles.is_empty() {
            let mut scan = Vec::with_capacity(overlap.len() + read);
            scan.extend_from_slice(&overlap);
            scan.extend_from_slice(&buffer[..read]);
            if needles
                .iter()
                .any(|needle| scan.windows(needle.len()).any(|window| window == *needle))
            {
                return Err(ContractError::verification(
                    "package regular-file entry contains a forbidden build prefix",
                ));
            }
            let keep = max_overlap.min(scan.len());
            overlap = scan[scan.len() - keep..].to_vec();
        }
    }
    if read_total != expected_size {
        return Err(ContractError::verification(
            "package regular-file entry is truncated",
        ));
    }
    Ok(finish_sha256(digest))
}

#[derive(Default)]
struct ArchiveBudget {
    entries: u64,
    expanded_bytes: u64,
}

impl ArchiveBudget {
    fn account(&mut self, regular_file: bool, size: u64) -> Result<(), ContractError> {
        self.entries = self
            .entries
            .checked_add(1)
            .ok_or_else(|| ContractError::verification("package tar entry count overflowed"))?;
        if self.entries > MAX_ARCHIVE_ENTRIES {
            return Err(ContractError::verification(
                "package tar exceeds the entry-count resource limit",
            ));
        }
        if regular_file {
            self.expanded_bytes = self.expanded_bytes.checked_add(size).ok_or_else(|| {
                ContractError::verification("package tar expanded size overflowed")
            })?;
            if self.expanded_bytes > MAX_EXPANDED_ARCHIVE_BYTES {
                return Err(ContractError::verification(
                    "package tar exceeds the expanded-size resource limit",
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_paths_outside_the_single_toolchain_root() {
        assert!(package_relative_path(Path::new("other/payload")).is_err());
        assert!(package_relative_path(Path::new("toolchain/../escape")).is_err());
        assert_eq!(
            package_relative_path(Path::new("toolchain/bin/clang"))
                .unwrap()
                .0,
            Some(PathBuf::from("bin/clang"))
        );
    }

    #[test]
    fn archive_budget_fails_closed_before_expansion() {
        let mut entries = ArchiveBudget {
            entries: MAX_ARCHIVE_ENTRIES,
            expanded_bytes: 0,
        };
        assert!(entries.account(false, 0).is_err());
        let mut bytes = ArchiveBudget {
            entries: 0,
            expanded_bytes: MAX_EXPANDED_ARCHIVE_BYTES,
        };
        assert!(bytes.account(true, 1).is_err());
    }

    #[test]
    fn archive_reader_rejects_a_nonroot_first_member() {
        use std::io::Write as _;

        let temporary = tempfile::tempdir().unwrap();
        let archive_path = temporary.path().join("malformed.tar.xz");
        let output = File::create(&archive_path).unwrap();
        let encoder = xz2::write::XzEncoder::new(output, 6);
        let mut builder = tar::Builder::new(encoder);
        let mut header = tar::Header::new_ustar();
        header.set_path("other/file").unwrap();
        header.set_size(1);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append(&header, &b"x"[..]).unwrap();
        let encoder = builder.into_inner().unwrap();
        encoder.finish().unwrap().flush().unwrap();
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../scripts/fixtures/toolchain-producer/package-v1.json"
        ))
        .unwrap();
        let manifest: ArosToolchainManifest =
            serde_json::from_value(fixture["manifest"].clone()).unwrap();
        assert!(verify_archive_tree(&archive_path, &manifest, &[]).is_err());
    }
}
