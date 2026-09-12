//! Bounded read-back verification for one native toolchain package set.
//!
//! Its public verification entry point has no extraction or installation side
//! effects. It reads the completed package in place, validates every outer
//! member, and recomputes the embedded payload inventory directly from the
//! bounded tar stream. The crate-private stream reader is also used by the
//! separate fresh-extraction boundary so both operations retain one parser.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};

#[cfg(unix)]
use std::fs::OpenOptions;
#[cfg(unix)]
use std::os::unix::fs::{symlink, PermissionsExt as _};

use aros_common::{
    finish_sha256, open_regular_file_nofollow, payload_casefold_path_key, sha256_reader,
    toolchain_inventory_sha256, ArosToolchainManifest, ArosToolchainManifestEntry, Sha256Digest,
    AROS_TOOLCHAIN_MANIFEST_FILE,
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

/// The four closed outer members of one toolchain package set.
///
/// This internal representation lets the release-index verifier reuse the
/// same bounded archive read-back logic after it has established the complete
/// release inventory. Public callers should use [`verify`], which
/// additionally requires that its directory contains exactly these members.
#[derive(Debug, Clone)]
pub(crate) struct PackageAssetPaths {
    pub(crate) archive: PathBuf,
    pub(crate) manifest: PathBuf,
    pub(crate) checksum: PathBuf,
    pub(crate) sbom: PathBuf,
}

impl PackageAssetPaths {
    pub(crate) fn for_asset(directory: &Path, asset: &str) -> Self {
        Self {
            archive: directory.join(asset),
            manifest: directory.join(format!("{asset}.manifest.json")),
            checksum: directory.join(format!("{asset}.sha256")),
            sbom: directory.join(format!("{asset}.spdx.json")),
        }
    }
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
    let paths = PackageAssetPaths::for_asset(&request.package_dir, &asset);
    let expected_members = [
        asset.clone(),
        format!("{asset}.manifest.json"),
        format!("{asset}.sha256"),
        format!("{asset}.spdx.json"),
    ];
    require_exact_outer_members(&request.package_dir, expected_members.iter())?;

    verify_members_validated(request, &paths)
}

/// Verify explicit members that are already part of a larger closed inventory.
///
/// The caller must establish the outer inventory contract before calling this
/// function. It retains all no-follow, identity, sidecar, SPDX, bounded-XZ,
/// tar-header, payload-inventory, and embedded-manifest checks of [`verify`].
pub(crate) fn verify_members(
    request: &PackageVerificationRequest,
    paths: &PackageAssetPaths,
) -> Result<VerifiedPackage, ContractError> {
    validate_request(request)?;
    verify_members_validated(request, paths)
}

fn verify_members_validated(
    request: &PackageVerificationRequest,
    paths: &PackageAssetPaths,
) -> Result<VerifiedPackage, ContractError> {
    let (archive_file, archive_size, archive_sha256) = measure_archive(&paths.archive)?;
    let asset = paths
        .archive
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            ContractError::verification("package archive path is not a UTF-8 basename")
        })?;
    let expected_checksum = format!("{archive_sha256}  {asset}\n");
    if read_metadata(&paths.checksum, "checksum sidecar")? != expected_checksum.as_bytes() {
        return Err(ContractError::verification(
            "package checksum sidecar does not match the measured archive",
        ));
    }
    let manifest_bytes = read_metadata(&paths.manifest, "external manifest")?;
    let manifest: ArosToolchainManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|_| ContractError::verification("external package manifest is not valid JSON"))?;
    manifest.validate().map_err(|_| {
        ContractError::verification("external package manifest violates the v1 contract")
    })?;
    verify_manifest_identity(&manifest, request)?;
    let expected_sbom = spdx_bytes(&request.source_lock, &manifest)?;
    if read_metadata(&paths.sbom, "SPDX SBOM")? != expected_sbom {
        return Err(ContractError::verification(
            "package SPDX SBOM does not match the closed source and manifest inputs",
        ));
    }

    let archive_result = verify_archive_tree(archive_file, &manifest, &request.forbidden_prefixes)?;
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

pub(crate) fn measure_archive(path: &Path) -> Result<(File, u64, Sha256Digest), ContractError> {
    let mut file = open_regular_file_nofollow(path)
        .map_err(|_| ContractError::verification("cannot safely open package archive"))?;
    let metadata = file
        .metadata()
        .map_err(|_| ContractError::verification("cannot inspect package archive"))?;
    if metadata.len() > MAX_ARCHIVE_BYTES {
        return Err(ContractError::verification(
            "package archive is not a regular file within the configured resource limit",
        ));
    }
    let result = sha256_reader(&mut file.by_ref().take(MAX_ARCHIVE_BYTES + 1))
        .map_err(|_| ContractError::verification("cannot measure package archive"))?;
    if result.size != metadata.len() {
        return Err(ContractError::verification(
            "package archive changed while it was measured",
        ));
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|_| ContractError::verification("cannot rewind package archive"))?;
    Ok((file, result.size, result.digest))
}

fn read_metadata(path: &Path, kind: &str) -> Result<Vec<u8>, ContractError> {
    let mut file = open_regular_file_nofollow(path)
        .map_err(|_| ContractError::verification(format!("cannot safely open {kind}")))?;
    let metadata = file
        .metadata()
        .map_err(|_| ContractError::verification(format!("cannot inspect {kind}")))?;
    if metadata.len() > MAX_METADATA_BYTES {
        return Err(ContractError::verification(format!(
            "{kind} is not a regular file within the configured resource limit"
        )));
    }
    let capacity = usize::try_from(metadata.len())
        .map_err(|_| ContractError::verification(format!("{kind} exceeds addressable memory")))?;
    let mut content = Vec::with_capacity(capacity);
    file.by_ref()
        .take(MAX_METADATA_BYTES + 1)
        .read_to_end(&mut content)
        .map_err(|_| ContractError::verification(format!("cannot read {kind}")))?;
    if u64::try_from(content.len()).is_ok_and(|size| size > MAX_METADATA_BYTES) {
        return Err(ContractError::verification(format!(
            "{kind} exceeds the configured resource limit while it was read"
        )));
    }
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
    input: File,
    manifest: &ArosToolchainManifest,
    forbidden_prefixes: &[PathBuf],
) -> Result<ArchiveTree, ContractError> {
    scan_archive(
        input,
        manifest,
        forbidden_prefixes,
        &mut VerifyingArchiveConsumer,
    )
}

/// Revalidate and materialize one already measured native package archive.
///
/// The caller creates one fresh, owned extraction root. This operation streams
/// the archive through the same parser as package verification and retains the
/// root on every failure for diagnosis. It never adopts, overwrites or removes
/// existing material.
#[cfg(unix)]
pub(crate) fn extract_verified_archive(
    input: File,
    manifest: &ArosToolchainManifest,
    forbidden_prefixes: &[PathBuf],
    destination: &Path,
) -> Result<(), ContractError> {
    let mut consumer = ExtractingArchiveConsumer::new(destination)?;
    scan_archive(input, manifest, forbidden_prefixes, &mut consumer).map(|_| ())
}

fn scan_archive<C: ArchiveConsumer>(
    input: File,
    manifest: &ArosToolchainManifest,
    forbidden_prefixes: &[PathBuf],
    consumer: &mut C,
) -> Result<ArchiveTree, ContractError> {
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
                consumer.directory(&relative)?;
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
                consumer.symlink(&relative, Path::new(target))?;
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
                    let bytes = consumer.embedded_manifest(&relative, &mut entry, size)?;
                    embedded_manifest = Some(bytes);
                } else {
                    let mode = header.mode().map_err(|_| {
                        ContractError::verification("package tar entry has an invalid mode")
                    })?;
                    let mode = if mode & 0o111 == 0 { "0644" } else { "0755" };
                    let sha256 =
                        consumer.regular(&relative, mode, &mut entry, size, forbidden_prefixes)?;
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
    let mut decoder = archive.into_inner();
    verify_archive_terminal(&mut decoder)?;
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

trait ArchiveConsumer {
    fn directory(&mut self, relative: &Path) -> Result<(), ContractError>;

    fn symlink(&mut self, relative: &Path, target: &Path) -> Result<(), ContractError>;

    fn embedded_manifest(
        &mut self,
        relative: &Path,
        entry: &mut dyn Read,
        size: u64,
    ) -> Result<Vec<u8>, ContractError>;

    fn regular(
        &mut self,
        relative: &Path,
        mode: &str,
        entry: &mut dyn Read,
        size: u64,
        forbidden_prefixes: &[PathBuf],
    ) -> Result<Sha256Digest, ContractError>;
}

struct VerifyingArchiveConsumer;

impl ArchiveConsumer for VerifyingArchiveConsumer {
    fn directory(&mut self, _relative: &Path) -> Result<(), ContractError> {
        Ok(())
    }

    fn symlink(&mut self, _relative: &Path, _target: &Path) -> Result<(), ContractError> {
        Ok(())
    }

    fn embedded_manifest(
        &mut self,
        _relative: &Path,
        entry: &mut dyn Read,
        size: u64,
    ) -> Result<Vec<u8>, ContractError> {
        read_exact_entry(entry, size)
    }

    fn regular(
        &mut self,
        _relative: &Path,
        _mode: &str,
        entry: &mut dyn Read,
        size: u64,
        forbidden_prefixes: &[PathBuf],
    ) -> Result<Sha256Digest, ContractError> {
        hash_entry(entry, size, forbidden_prefixes)
    }
}

#[cfg(unix)]
struct ExtractingArchiveConsumer<'a> {
    root: &'a Path,
    directories: BTreeSet<PathBuf>,
}

#[cfg(unix)]
impl<'a> ExtractingArchiveConsumer<'a> {
    fn new(root: &'a Path) -> Result<Self, ContractError> {
        crate::filesystem::open_directory(root).map_err(|_| {
            ContractError::verification("package extraction root is not a safe real directory")
        })?;
        let mut directories = BTreeSet::new();
        directories.insert(PathBuf::new());
        Ok(Self { root, directories })
    }

    fn require_parent(&self, relative: &Path) -> Result<(), ContractError> {
        let parent = relative.parent().ok_or_else(|| {
            ContractError::verification("package extraction entry has no relative parent")
        })?;
        if !self.directories.contains(parent) {
            return Err(ContractError::verification(
                "package extraction entry has no previously materialized real parent directory",
            ));
        }
        crate::filesystem::open_directory(&self.root.join(parent)).map_err(|_| {
            ContractError::verification(
                "package extraction parent changed or is not a real directory",
            )
        })?;
        Ok(())
    }

    fn create_directory(&mut self, relative: &Path) -> Result<(), ContractError> {
        self.require_parent(relative)?;
        let path = self.root.join(relative);
        fs::create_dir(&path).map_err(|_| {
            ContractError::verification("cannot create a fresh package extraction directory")
        })?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).map_err(|_| {
            ContractError::verification("cannot normalize a package extraction directory mode")
        })?;
        crate::filesystem::open_directory(&path).map_err(|_| {
            ContractError::verification("package extraction directory is not a real directory")
        })?;
        self.directories.insert(relative.to_path_buf());
        Ok(())
    }

    fn write_regular(
        &self,
        relative: &Path,
        mode: u32,
        entry: &mut dyn Read,
        size: u64,
        forbidden_prefixes: &[PathBuf],
    ) -> Result<Sha256Digest, ContractError> {
        self.require_parent(relative)?;
        let path = self.root.join(relative);
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|_| {
                ContractError::verification("cannot create a fresh package extraction file")
            })?;
        let digest = hash_entry_to(entry, size, forbidden_prefixes, &mut output)?;
        output.sync_all().map_err(|_| {
            ContractError::verification("cannot durably write a package extraction file")
        })?;
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).map_err(|_| {
            ContractError::verification("cannot normalize a package extraction file mode")
        })?;
        Ok(digest)
    }

    fn write_manifest(
        &self,
        relative: &Path,
        entry: &mut dyn Read,
        size: u64,
    ) -> Result<Vec<u8>, ContractError> {
        let contents = read_exact_entry(entry, size)?;
        self.require_parent(relative)?;
        let path = self.root.join(relative);
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|_| {
                ContractError::verification("cannot create the extracted package manifest")
            })?;
        std::io::Write::write_all(&mut output, &contents).map_err(|_| {
            ContractError::verification("cannot write the extracted package manifest")
        })?;
        output.sync_all().map_err(|_| {
            ContractError::verification("cannot durably write the extracted package manifest")
        })?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).map_err(|_| {
            ContractError::verification("cannot normalize the extracted package manifest mode")
        })?;
        Ok(contents)
    }
}

#[cfg(unix)]
impl ArchiveConsumer for ExtractingArchiveConsumer<'_> {
    fn directory(&mut self, relative: &Path) -> Result<(), ContractError> {
        self.create_directory(relative)
    }

    fn symlink(&mut self, relative: &Path, target: &Path) -> Result<(), ContractError> {
        self.require_parent(relative)?;
        let path = self.root.join(relative);
        symlink(target, &path).map_err(|_| {
            ContractError::verification("cannot create a fresh package extraction symlink")
        })?;
        let metadata = fs::symlink_metadata(&path).map_err(|_| {
            ContractError::verification("cannot inspect a package extraction symlink")
        })?;
        if !metadata.file_type().is_symlink() {
            return Err(ContractError::verification(
                "package extraction symlink changed while it was created",
            ));
        }
        Ok(())
    }

    fn embedded_manifest(
        &mut self,
        relative: &Path,
        entry: &mut dyn Read,
        size: u64,
    ) -> Result<Vec<u8>, ContractError> {
        self.write_manifest(relative, entry, size)
    }

    fn regular(
        &mut self,
        relative: &Path,
        mode: &str,
        entry: &mut dyn Read,
        size: u64,
        forbidden_prefixes: &[PathBuf],
    ) -> Result<Sha256Digest, ContractError> {
        let mode = match mode {
            "0644" => 0o644,
            "0755" => 0o755,
            _ => {
                return Err(ContractError::verification(
                    "package extraction received a noncanonical regular-file mode",
                ))
            }
        };
        self.write_regular(relative, mode, entry, size, forbidden_prefixes)
    }
}

fn verify_archive_terminal(decoder: &mut XzDecoder<File>) -> Result<(), ContractError> {
    let mut buffer = [0_u8; 8 * 1024];
    let mut trailer_size = 0_u64;
    loop {
        let read = decoder.read(&mut buffer).map_err(|_| {
            ContractError::verification("package XZ stream is truncated or malformed")
        })?;
        if read == 0 {
            return Ok(());
        }
        trailer_size = trailer_size
            .checked_add(u64::try_from(read).map_err(io::Error::other).map_err(|_| {
                ContractError::verification("package tar trailer byte count overflowed")
            })?)
            .ok_or_else(|| {
                ContractError::verification("package tar trailer byte count overflowed")
            })?;
        if trailer_size > 20 * 512 || buffer[..read].iter().any(|byte| *byte != 0) {
            return Err(ContractError::verification(
                "package tar has a noncanonical trailing record",
            ));
        }
    }
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

fn read_exact_entry<R: Read + ?Sized>(
    entry: &mut R,
    expected_size: u64,
) -> Result<Vec<u8>, ContractError> {
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

fn hash_entry<R: Read + ?Sized>(
    entry: &mut R,
    expected_size: u64,
    forbidden_prefixes: &[PathBuf],
) -> Result<Sha256Digest, ContractError> {
    hash_entry_with_output(entry, expected_size, forbidden_prefixes, None)
}

#[cfg(unix)]
fn hash_entry_to<R: Read + ?Sized>(
    entry: &mut R,
    expected_size: u64,
    forbidden_prefixes: &[PathBuf],
    output: &mut File,
) -> Result<Sha256Digest, ContractError> {
    hash_entry_with_output(entry, expected_size, forbidden_prefixes, Some(output))
}

fn hash_entry_with_output<R: Read + ?Sized>(
    entry: &mut R,
    expected_size: u64,
    forbidden_prefixes: &[PathBuf],
    mut output: Option<&mut File>,
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
        if let Some(output) = output.as_deref_mut() {
            std::io::Write::write_all(output, &buffer[..read]).map_err(|_| {
                ContractError::verification("cannot write package regular-file extraction bytes")
            })?;
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

    fn fixture_manifest() -> ArosToolchainManifest {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../scripts/fixtures/toolchain-producer/package-v1.json"
        ))
        .unwrap();
        serde_json::from_value(fixture["manifest"].clone()).unwrap()
    }

    fn canonical_header(
        path: &str,
        entry_type: tar::EntryType,
        size: u64,
        mode: u32,
    ) -> tar::Header {
        let manifest = fixture_manifest();
        let mut header = tar::Header::new_ustar();
        header.set_path(path).unwrap();
        header.set_entry_type(entry_type);
        header.set_size(size);
        header.set_mode(mode);
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(manifest.source_date_epoch);
        header.set_cksum();
        header
    }

    fn write_xz_tar(path: &Path, append: impl FnOnce(&mut xz2::write::XzEncoder<File>)) {
        use std::io::Write as _;

        let output = File::create(path).unwrap();
        let encoder = xz2::write::XzEncoder::new(output, 6);
        let mut builder = tar::Builder::new(encoder);
        builder
            .append(
                &canonical_header("toolchain", tar::EntryType::Directory, 0, 0o755),
                &b""[..],
            )
            .unwrap();
        builder.finish().unwrap();
        let mut encoder = builder.into_inner().unwrap();
        append(&mut encoder);
        encoder.finish().unwrap().flush().unwrap();
    }

    fn write_xz_tar_entries(
        path: &Path,
        append: impl FnOnce(&mut tar::Builder<xz2::write::XzEncoder<File>>),
    ) {
        use std::io::Write as _;

        let output = File::create(path).unwrap();
        let encoder = xz2::write::XzEncoder::new(output, 6);
        let mut builder = tar::Builder::new(encoder);
        builder
            .append(
                &canonical_header("toolchain", tar::EntryType::Directory, 0, 0o755),
                &b""[..],
            )
            .unwrap();
        append(&mut builder);
        builder.finish().unwrap();
        let encoder = builder.into_inner().unwrap();
        encoder.finish().unwrap().flush().unwrap();
    }

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
        let manifest = fixture_manifest();
        assert!(verify_archive_tree(File::open(&archive_path).unwrap(), &manifest, &[]).is_err());
    }

    #[test]
    fn archive_reader_rejects_truncated_xz_and_nonzero_tar_trailers() {
        use std::io::Write as _;

        let temporary = tempfile::tempdir().unwrap();
        let archive = temporary.path().join("trailer.tar.xz");
        write_xz_tar(&archive, |encoder| {
            encoder.write_all(b"untrusted trailing bytes").unwrap();
        });
        let manifest = fixture_manifest();
        assert!(verify_archive_tree(File::open(&archive).unwrap(), &manifest, &[]).is_err());

        let bytes = fs::read(&archive).unwrap();
        fs::write(&archive, &bytes[..bytes.len() - 1]).unwrap();
        assert!(verify_archive_tree(File::open(&archive).unwrap(), &manifest, &[]).is_err());
    }

    #[test]
    fn archive_reader_rejects_symlink_escapes_before_inventory_acceptance() {
        let temporary = tempfile::tempdir().unwrap();
        let archive = temporary.path().join("escape.tar.xz");
        write_xz_tar(&archive, |encoder| {
            let mut builder = tar::Builder::new(encoder);
            let mut header = canonical_header("toolchain/link", tar::EntryType::Symlink, 0, 0o777);
            header.set_link_name("../../escape").unwrap();
            header.set_cksum();
            builder.append(&header, &b""[..]).unwrap();
            builder.finish().unwrap();
        });
        let manifest = fixture_manifest();
        assert!(verify_archive_tree(File::open(&archive).unwrap(), &manifest, &[]).is_err());
    }

    #[test]
    fn archive_reader_rejects_casefold_collisions_and_special_entries() {
        let temporary = tempfile::tempdir().unwrap();
        let collision = temporary.path().join("collision.tar.xz");
        write_xz_tar_entries(&collision, |builder| {
            for path in ["toolchain/Foo", "toolchain/foo"] {
                builder
                    .append(
                        &canonical_header(path, tar::EntryType::Regular, 1, 0o644),
                        &b"x"[..],
                    )
                    .unwrap();
            }
        });
        let manifest = fixture_manifest();
        assert!(verify_archive_tree(File::open(&collision).unwrap(), &manifest, &[]).is_err());

        let special = temporary.path().join("special.tar.xz");
        write_xz_tar_entries(&special, |builder| {
            builder
                .append(
                    &canonical_header("toolchain/fifo", tar::EntryType::Fifo, 0, 0o644),
                    &b""[..],
                )
                .unwrap();
        });
        assert!(verify_archive_tree(File::open(&special).unwrap(), &manifest, &[]).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn archive_extraction_materializes_a_safe_archive_in_a_retained_root() {
        use std::os::unix::fs::PermissionsExt as _;

        let temporary = tempfile::tempdir().unwrap();
        let archive = temporary.path().join("safe.tar.xz");
        write_xz_tar_entries(&archive, |builder| {
            builder
                .append(
                    &canonical_header("toolchain/bin", tar::EntryType::Directory, 0, 0o755),
                    &b""[..],
                )
                .unwrap();
            builder
                .append(
                    &canonical_header("toolchain/bin/clang", tar::EntryType::Regular, 5, 0o755),
                    &b"clang"[..],
                )
                .unwrap();
            builder
                .append(
                    &canonical_header(
                        "toolchain/toolchain-manifest.json",
                        tar::EntryType::Regular,
                        2,
                        0o644,
                    ),
                    &b"{}"[..],
                )
                .unwrap();
        });
        let root = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("retained-root");
        fs::create_dir(&root).unwrap();

        extract_verified_archive(
            File::open(&archive).unwrap(),
            &fixture_manifest(),
            &[],
            &root,
        )
        .unwrap();

        let clang = root.join("bin/clang");
        assert_eq!(fs::read(&clang).unwrap(), b"clang");
        assert_eq!(
            fs::metadata(&clang).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert_eq!(
            fs::read(root.join("toolchain-manifest.json")).unwrap(),
            b"{}"
        );
        assert!(root.is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn archive_extraction_rejects_children_below_a_retained_symlink() {
        let temporary = tempfile::tempdir().unwrap();
        let archive = temporary.path().join("symlink-child.tar.xz");
        write_xz_tar_entries(&archive, |builder| {
            let mut link = canonical_header("toolchain/link", tar::EntryType::Symlink, 0, 0o777);
            link.set_link_name("safe").unwrap();
            link.set_cksum();
            builder.append(&link, &b""[..]).unwrap();
            builder
                .append(
                    &canonical_header("toolchain/link/escape", tar::EntryType::Regular, 1, 0o644),
                    &b"x"[..],
                )
                .unwrap();
        });
        let root = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("retained-root");
        fs::create_dir(&root).unwrap();

        assert!(extract_verified_archive(
            File::open(&archive).unwrap(),
            &fixture_manifest(),
            &[],
            &root,
        )
        .is_err());

        assert!(fs::symlink_metadata(root.join("link"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(!root.join("link/escape").exists());
        assert!(root.is_dir());
    }
}
