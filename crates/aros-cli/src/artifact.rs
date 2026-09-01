//! Verified download, extraction, inventory, and atomic publication helpers.

use futures_util::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use miette::{bail, IntoDiagnostic, Result, WrapErr};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;
use tempfile::TempDir;
use xz2::read::XzDecoder;

use aros_common::toolchain_manifest::{ArosToolchainManifestEntry, AROS_TOOLCHAIN_MANIFEST_FILE};
use aros_common::{casefold_path_key, parse_credential_free_https_url};

/// Marker published only after a toolchain envelope is complete.
pub const INSTALL_COMPLETE_FILE: &str = ".complete";

/// Absolute denial-of-service ceiling for archives whose contract predates an
/// exact byte size. New release locks should always provide `size`.
const MAX_UNSIZED_ARCHIVE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: u64 = 500_000;
const MAX_EXPANDED_ARCHIVE_BYTES: u64 = 32 * 1024 * 1024 * 1024;
const MAX_XZ_DECODER_MEMORY: u64 = 512 * 1024 * 1024;
const MAX_REDIRECTS: usize = 10;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const TRANSFER_TIMEOUT: Duration = Duration::from_hours(1);

/// Resolve the user-controlled AROS state directory.
///
/// # Errors
///
/// Returns an error when neither an absolute `AROS_HOME` nor an absolute
/// platform home directory is available. State is never silently rooted in
/// the caller's current working directory.
pub fn aros_home() -> Result<PathBuf> {
    let path = if let Some(configured) = std::env::var_os("AROS_HOME") {
        PathBuf::from(configured)
    } else {
        PathBuf::from(std::env::var_os("HOME").ok_or_else(|| {
            miette::miette!(
                "cannot resolve AROS state: HOME is unset; set AROS_HOME to an absolute path"
            )
        })?)
        .join(".aros")
    };
    require_absolute_state_path("AROS_HOME", path)
}

/// Return the content-addressed archive-cache root.
pub fn archive_cache_root() -> Result<PathBuf> {
    match std::env::var_os("AROS_CACHE_DIR") {
        Some(path) => require_absolute_state_path("AROS_CACHE_DIR", PathBuf::from(path)),
        None => Ok(aros_home()?.join("cache")),
    }
}

pub fn require_absolute_state_path(label: &str, path: PathBuf) -> Result<PathBuf> {
    if !path.is_absolute() {
        bail!("{label} must be an absolute path, got '{}'", path.display());
    }
    Ok(path)
}

/// Hash one regular file and return its lowercase SHA-256 digest.
///
/// # Errors
///
/// Returns an error when the file cannot be opened or read completely.
pub fn sha256_file(path: &Path) -> Result<String> {
    aros_common::sha256_file(path)
        .map(|result| result.digest.to_string())
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to hash '{}'", path.display()))
}

/// Verify an archive's exact size, when known, and required SHA-256 digest.
///
/// # Errors
///
/// Returns an error for inaccessible content or any identity mismatch.
pub fn verify_archive(
    path: &Path,
    expected_sha256: &str,
    expected_size: Option<u64>,
) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to inspect archive '{}'", path.display()))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        bail!("archive '{}' is not a regular file", path.display());
    }
    if let Some(expected) = expected_size {
        if metadata.len() != expected {
            bail!(
                "archive '{}' has size {}, expected {}",
                path.display(),
                metadata.len(),
                expected
            );
        }
    }
    let actual = sha256_file(path)?;
    if !actual.eq_ignore_ascii_case(expected_sha256) {
        bail!(
            "SHA256 mismatch for '{}': expected {}, got {}",
            path.display(),
            expected_sha256,
            actual
        );
    }
    Ok(())
}

/// Resolve an archive from the verified local cache or an authenticated URL.
///
/// Downloads are streamed into a temporary file, verified, and published to
/// the content-addressed cache only after all checks pass.
///
/// # Errors
///
/// Returns an error for offline cache misses, transport failures, invalid
/// responses, or archive identity mismatches.
pub async fn obtain_archive(
    url: &str,
    expected_sha256: &str,
    expected_size: Option<u64>,
    offline: bool,
    force_download: bool,
) -> Result<PathBuf> {
    let expected_sha256 = require_sha256(Some(expected_sha256), "archive")?;
    if expected_size == Some(0) {
        bail!("archive has an invalid declared size of zero bytes");
    }
    let download_url = validate_download_url(url)?;
    let cache_dir = archive_cache_root()?.join("downloads").join("sha256");
    fs::create_dir_all(&cache_dir)
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to create cache '{}'", cache_dir.display()))?;
    let cache_path = cache_dir.join(format!("{expected_sha256}.tar.xz"));

    if cache_path.exists() && !force_download {
        verify_archive(&cache_path, &expected_sha256, expected_size)?;
        return Ok(cache_path);
    }
    if offline {
        bail!(
            "offline mode: verified archive {} is not available in {}",
            expected_sha256,
            cache_dir.display()
        );
    }

    let client = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(TRANSFER_TIMEOUT)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= MAX_REDIRECTS {
                return attempt.error("too many redirects");
            }
            let next = attempt.url();
            if next.scheme() != "https"
                || next.host_str().is_none()
                || !next.username().is_empty()
                || next.password().is_some()
            {
                return attempt.error("redirect left the credential-free HTTPS boundary");
            }
            attempt.follow()
        }))
        .user_agent(concat!(
            "aros-tools/",
            env!("CARGO_PKG_VERSION"),
            " (+https://github.com/metaneutrons/aros-tools)"
        ))
        .build()
        .into_diagnostic()?;
    let response = client
        .get(download_url)
        .send()
        .await
        .map_err(reqwest::Error::without_url)
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to download '{url}'"))?;
    if !response.status().is_success() {
        bail!("failed to download '{url}': HTTP {}", response.status());
    }

    let limit = expected_size.unwrap_or(MAX_UNSIZED_ARCHIVE_BYTES);
    if let Some(length) = response.content_length() {
        if expected_size.is_some_and(|expected| length != expected) {
            bail!("download for '{url}' declares {length} bytes, expected exactly {limit} bytes");
        }
        if length > limit {
            bail!(
                "download for '{url}' declares {length} bytes, exceeding the {limit}-byte contract limit"
            );
        }
    }
    let progress = if crate::observability::human_progress_enabled() {
        ProgressBar::new(response.content_length().unwrap_or(0))
    } else {
        ProgressBar::hidden()
    };
    let progress_style = ProgressStyle::default_bar()
        .template(
            "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})",
        )
        .into_diagnostic()
        .wrap_err("invalid built-in download progress template")?
        .progress_chars("#>-");
    progress.set_style(progress_style);

    let named = tempfile::NamedTempFile::new_in(&cache_dir)
        .into_diagnostic()
        .wrap_err("failed to create temporary archive in cache")?;
    let (file, temp_path) = named.into_parts();
    let mut output = tokio::fs::File::from_std(file);
    let mut stream = response.bytes_stream();
    let mut received = 0_u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk
            .map_err(reqwest::Error::without_url)
            .into_diagnostic()
            .wrap_err("failed while downloading toolchain archive")?;
        received = received
            .checked_add(u64::try_from(chunk.len()).into_diagnostic()?)
            .ok_or_else(|| miette::miette!("download byte count overflowed"))?;
        if received > limit {
            bail!("download for '{url}' exceeded the {limit}-byte contract limit while streaming");
        }
        tokio::io::AsyncWriteExt::write_all(&mut output, &chunk)
            .await
            .into_diagnostic()
            .wrap_err("failed to write toolchain archive")?;
        progress.inc(chunk.len() as u64);
    }
    tokio::io::AsyncWriteExt::flush(&mut output)
        .await
        .into_diagnostic()
        .wrap_err("failed to flush toolchain archive")?;
    output
        .sync_all()
        .await
        .into_diagnostic()
        .wrap_err("failed to durably flush toolchain archive")?;
    drop(output);
    progress.finish_with_message("downloaded");

    verify_archive(&temp_path, &expected_sha256, expected_size)?;
    match temp_path.persist(&cache_path) {
        Ok(()) => {}
        Err(error) if cache_path.exists() => {
            drop(error);
            verify_archive(&cache_path, &expected_sha256, expected_size)?;
        }
        Err(error) => {
            return Err(error.error).into_diagnostic().wrap_err_with(|| {
                format!("failed to commit archive cache '{}'", cache_path.display())
            });
        }
    }
    sync_directory(&cache_dir)?;
    verify_archive(&cache_path, &expected_sha256, expected_size)?;
    Ok(cache_path)
}

fn validate_download_url(value: &str) -> Result<reqwest::Url> {
    parse_credential_free_https_url(value).map_err(|message| miette::miette!(message))
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .into_diagnostic()
        .wrap_err_with(|| {
            format!(
                "failed to durably flush cache directory '{}'",
                path.display()
            )
        })
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<()> {
    Ok(())
}

/// Extract a verified `.tar.xz` into an isolated staging directory.
///
/// Every archive path is checked before extraction and the resulting tree is
/// matched against the embedded toolchain manifest.
///
/// # Errors
///
/// Returns an error for unsafe members, unsupported entry types, malformed
/// manifests, or inventory mismatches.
pub fn extract_to_staging(
    archive_path: &Path,
    store_parent: &Path,
    strip_components: usize,
) -> Result<TempDir> {
    fs::create_dir_all(store_parent)
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to create store '{}'", store_parent.display()))?;
    let staging = tempfile::Builder::new()
        .prefix(".install-")
        .tempdir_in(store_parent)
        .into_diagnostic()
        .wrap_err("failed to create toolchain staging directory")?;

    let input = File::open(archive_path)
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to open '{}'", archive_path.display()))?;
    let stream = xz2::stream::Stream::new_stream_decoder(MAX_XZ_DECODER_MEMORY, 0)
        .into_diagnostic()
        .wrap_err("failed to initialize bounded XZ decoder")?;
    let decoder = XzDecoder::new_stream(input, stream);
    let mut archive = tar::Archive::new(decoder);
    let mut budget = ExtractionBudget::default();
    let mut extracted_paths = BTreeSet::new();
    for entry in archive
        .entries()
        .into_diagnostic()
        .wrap_err("failed to read tar archive")?
    {
        let mut entry = entry.into_diagnostic().wrap_err("invalid tar entry")?;
        let entry_type = entry.header().entry_type();
        let declared_size = entry
            .header()
            .size()
            .into_diagnostic()
            .wrap_err("invalid tar entry size")?;
        budget.account(entry_type.is_file(), declared_size)?;
        let source_path = entry
            .path()
            .into_diagnostic()
            .wrap_err("invalid tar entry path")?;
        let Some(relative_path) = safe_stripped_path(&source_path, strip_components)? else {
            continue;
        };
        let portable_key = casefold_path_key(&relative_path)
            .into_diagnostic()
            .wrap_err_with(|| {
                format!(
                    "archive entry '{}' is not portable across supported hosts",
                    relative_path.display()
                )
            })?;
        if !extracted_paths.insert(portable_key) {
            bail!(
                "archive contains a duplicate or case-folding path collision at '{}'",
                relative_path.display()
            );
        }
        ensure_no_symlink_ancestors(staging.path(), &relative_path)?;

        if entry_type.is_symlink() {
            let link = entry
                .link_name()
                .into_diagnostic()
                .wrap_err("invalid symlink target")?
                .ok_or_else(|| miette::miette!("symlink has no target"))?;
            validate_symlink_target(&relative_path, &link)?;
        } else if !(entry_type.is_file() || entry_type.is_dir()) {
            bail!(
                "unsupported tar entry type for '{}' (only files, directories, and safe symlinks are allowed)",
                relative_path.display()
            );
        }

        let destination = staging.path().join(&relative_path);
        match fs::symlink_metadata(&destination) {
            Ok(metadata)
                if entry_type.is_dir()
                    && metadata.is_dir()
                    && !metadata.file_type().is_symlink() => {}
            Ok(_) => bail!(
                "archive entry '{}' collides with an existing extracted path",
                relative_path.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error)
                    .into_diagnostic()
                    .wrap_err_with(|| format!("failed to inspect '{}'", destination.display()));
            }
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).into_diagnostic()?;
        }
        entry
            .unpack(&destination)
            .into_diagnostic()
            .wrap_err_with(|| format!("failed to extract '{}'", relative_path.display()))?;
    }
    normalize_extracted_permissions(staging.path())?;
    Ok(staging)
}

fn normalize_extracted_permissions(root: &Path) -> Result<()> {
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = fs::read_dir(&directory)
            .into_diagnostic()
            .wrap_err_with(|| format!("failed to read extracted tree '{}'", directory.display()))?;
        for entry in entries {
            let entry = entry.into_diagnostic()?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).into_diagnostic()?;
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                set_portable_permissions(&path, 0o755)?;
                pending.push(path);
            } else if metadata.is_file() {
                let mode = if normalized_file_mode(&metadata) == 0o755 {
                    0o755
                } else {
                    0o644
                };
                set_portable_permissions(&path, mode)?;
            } else {
                bail!("unsupported extracted entry '{}'", path.display());
            }
        }
    }
    set_portable_permissions(root, 0o755)
}

#[cfg(unix)]
fn set_portable_permissions(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;

    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to normalize permissions for '{}'", path.display()))
}

#[cfg(not(unix))]
fn set_portable_permissions(path: &Path, _mode: u32) -> Result<()> {
    let mut permissions = fs::metadata(path).into_diagnostic()?.permissions();
    permissions.set_readonly(false);
    fs::set_permissions(path, permissions)
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to normalize permissions for '{}'", path.display()))
}

#[derive(Default)]
struct ExtractionBudget {
    entries: u64,
    expanded_bytes: u64,
}

impl ExtractionBudget {
    fn account(&mut self, regular_file: bool, declared_size: u64) -> Result<()> {
        self.entries = self
            .entries
            .checked_add(1)
            .ok_or_else(|| miette::miette!("archive entry count overflowed"))?;
        if self.entries > MAX_ARCHIVE_ENTRIES {
            bail!("archive exceeds the {MAX_ARCHIVE_ENTRIES}-entry extraction limit");
        }
        if regular_file {
            self.expanded_bytes = self
                .expanded_bytes
                .checked_add(declared_size)
                .ok_or_else(|| miette::miette!("expanded archive byte count overflowed"))?;
            if self.expanded_bytes > MAX_EXPANDED_ARCHIVE_BYTES {
                bail!("archive exceeds the {MAX_EXPANDED_ARCHIVE_BYTES}-byte expanded-size limit");
            }
        }
        Ok(())
    }
}

/// Durably publish a fully verified staging tree without replacing anything.
///
/// # Errors
///
/// Returns an error when the destination already exists, the staging directory
/// is not beside it, the tree is unsafe, or durable publication cannot be
/// proven. A commit-state-uncertain error deliberately leaves the complete
/// destination in place for inspection.
pub fn commit_staging(staging: &TempDir, destination: &Path) -> Result<()> {
    let parent = destination
        .parent()
        .ok_or_else(|| miette::miette!("toolchain destination has no parent"))?;
    fs::create_dir_all(parent)
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to create store '{}'", parent.display()))?;
    match aros_common::publish_prepared_tree_noclobber(staging.path(), destination) {
        Ok(_) => Ok(()),
        Err(error) => {
            let failure_class = aros_common::publication_failure_class(&error);
            Err(error).into_diagnostic().wrap_err_with(|| {
                format!(
                    "failed to durably install toolchain at '{}' ({failure_class:?})",
                    destination.display()
                )
            })
        }
    }
}

/// Compute the producer-compatible tree digest over canonical JSON inventory
/// lines. The embedded manifest is excluded to avoid a hash cycle.
#[cfg(test)]
/// Compute the canonical digest of a directory inventory.
///
/// # Errors
///
/// Returns an error when the tree contains unsupported or unreadable entries.
pub fn tree_sha256(root: &Path) -> Result<String> {
    tree_inventory(root).map(|(digest, _)| digest)
}

/// Return a canonical tree digest and its sorted manifest entries.
///
/// # Errors
///
/// Returns an error for missing roots, unsafe paths, symbolic links, or files
/// that cannot be read completely.
pub fn tree_inventory(root: &Path) -> Result<(String, Vec<ArosToolchainManifestEntry>)> {
    tree_inventory_excluding(root, &[AROS_TOOLCHAIN_MANIFEST_FILE])
}

/// Return a canonical tree digest and sorted manifest entries while omitting
/// explicit top-level metadata namespaces.
///
/// Exclusions are validated portable relative paths and remove the named node
/// plus its complete subtree. This is intended for self-describing installed
/// payloads whose receipt must not participate in its own digest.
///
/// # Errors
///
/// Returns an error for invalid exclusions, missing roots, unsafe paths, or
/// entries that cannot be read as one stable no-follow inventory.
pub fn tree_inventory_excluding(
    root: &Path,
    exclusions: &[&str],
) -> Result<(String, Vec<ArosToolchainManifestEntry>)> {
    let exclusions = exclusions
        .iter()
        .map(|value| {
            let path = PathBuf::from(value);
            portable_relative_path(&path)?;
            Ok(path)
        })
        .collect::<Result<Vec<_>>>()?;
    let mut entries = Vec::new();
    collect_tree_entries(root, Path::new(""), &exclusions, &mut entries)?;
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    Ok((inventory_sha256(&entries)?, entries))
}

fn inventory_sha256(entries: &[ArosToolchainManifestEntry]) -> Result<String> {
    let mut tree = Sha256::new();
    for entry in entries {
        tree.update(serde_json::to_vec(&canonical_entry(entry)).into_diagnostic()?);
        tree.update(b"\n");
    }
    Ok(format!("{:x}", tree.finalize()))
}

fn collect_tree_entries(
    root: &Path,
    relative: &Path,
    exclusions: &[PathBuf],
    output: &mut Vec<ArosToolchainManifestEntry>,
) -> Result<()> {
    let directory = root.join(relative);
    let mut entries = fs::read_dir(&directory)
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to read '{}'", directory.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .into_diagnostic()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let child_relative = relative.join(entry.file_name());
        if exclusions
            .iter()
            .any(|excluded| child_relative == *excluded || child_relative.starts_with(excluded))
        {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path()).into_diagnostic()?;
        let path = portable_relative_path(&child_relative)?;
        if metadata.file_type().is_symlink() {
            let target = fs::read_link(entry.path()).into_diagnostic()?;
            let target = target.to_str().ok_or_else(|| {
                miette::miette!("symlink target is not UTF-8: {}", target.display())
            })?;
            output.push(ArosToolchainManifestEntry {
                path,
                mode: "0777".into(),
                kind: "symlink".into(),
                sha256: None,
                size: None,
                target: Some(target.into()),
            });
        } else if metadata.is_dir() {
            output.push(ArosToolchainManifestEntry {
                path,
                mode: "0755".into(),
                kind: "directory".into(),
                sha256: None,
                size: None,
                target: None,
            });
            collect_tree_entries(root, &child_relative, exclusions, output)?;
        } else if metadata.is_file() {
            let Some((_, contents)) = aros_common::measure_regular_file(&entry.path())
                .into_diagnostic()
                .wrap_err_with(|| {
                    format!(
                        "failed to measure regular payload file '{}' without following links",
                        entry.path().display()
                    )
                })?
            else {
                bail!("payload file '{}' disappeared", child_relative.display());
            };
            let measured = fs::symlink_metadata(entry.path()).into_diagnostic()?;
            if !measured.is_file()
                || measured.file_type().is_symlink()
                || measured.len() != u64::try_from(contents.len()).into_diagnostic()?
            {
                bail!(
                    "payload file '{}' changed while it was inventoried",
                    child_relative.display()
                );
            }
            output.push(ArosToolchainManifestEntry {
                path,
                mode: format!("{:04o}", normalized_file_mode(&measured)),
                kind: "file".into(),
                sha256: Some(aros_common::sha256_bytes(&contents).to_string()),
                size: Some(measured.len()),
                target: None,
            });
        } else {
            bail!("unsupported payload entry '{}'", child_relative.display());
        }
    }
    Ok(())
}

fn canonical_entry(
    entry: &ArosToolchainManifestEntry,
) -> std::collections::BTreeMap<&'static str, serde_json::Value> {
    let mut object = std::collections::BTreeMap::new();
    object.insert("mode", serde_json::Value::String(entry.mode.clone()));
    object.insert("path", serde_json::Value::String(entry.path.clone()));
    if let Some(sha256) = &entry.sha256 {
        object.insert("sha256", serde_json::Value::String(sha256.clone()));
    }
    if let Some(size) = entry.size {
        object.insert("size", serde_json::Value::Number(size.into()));
    }
    if let Some(target) = &entry.target {
        object.insert("target", serde_json::Value::String(target.clone()));
    }
    object.insert("type", serde_json::Value::String(entry.kind.clone()));
    object
}

fn portable_relative_path(path: &Path) -> Result<String> {
    path.components()
        .map(|component| match component {
            Component::Normal(value) => value
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| miette::miette!("toolchain path is not UTF-8: {}", path.display())),
            _ => bail!(
                "toolchain inventory path is not relative: {}",
                path.display()
            ),
        })
        .collect::<Result<Vec<_>>>()
        .map(|components| components.join("/"))
}

#[cfg(unix)]
fn normalized_file_mode(metadata: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    if metadata.permissions().mode() & 0o111 == 0 {
        0o644
    } else {
        0o755
    }
}

#[cfg(not(unix))]
fn normalized_file_mode(metadata: &fs::Metadata) -> u32 {
    if metadata.permissions().readonly() {
        0o644
    } else {
        0o755
    }
}

fn safe_stripped_path(path: &Path, strip_components: usize) -> Result<Option<PathBuf>> {
    let components = path.components().collect::<Vec<_>>();
    if components.iter().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        bail!("unsafe archive path '{}'", path.display());
    }
    if components.len() <= strip_components {
        return Ok(None);
    }
    let mut relative = PathBuf::new();
    for component in components.into_iter().skip(strip_components) {
        match component {
            Component::Normal(value) => relative.push(value),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                bail!("unsafe archive path '{}'", path.display());
            }
        }
    }
    if relative.as_os_str().is_empty() {
        Ok(None)
    } else {
        Ok(Some(relative))
    }
}

fn validate_symlink_target(entry_path: &Path, target: &Path) -> Result<()> {
    if target.is_absolute() {
        bail!(
            "symlink '{}' has absolute target '{}'",
            entry_path.display(),
            target.display()
        );
    }
    let mut depth = entry_path
        .parent()
        .map_or(0, |parent| parent.components().count());
    for component in target.components() {
        match component {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir if depth > 0 => depth -= 1,
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                bail!(
                    "symlink '{}' escapes the toolchain root via '{}'",
                    entry_path.display(),
                    target.display()
                );
            }
        }
    }
    Ok(())
}

fn ensure_no_symlink_ancestors(root: &Path, relative: &Path) -> Result<()> {
    let mut current = root.to_path_buf();
    let component_count = relative.components().count();
    for component in relative
        .components()
        .take(component_count.saturating_sub(1))
    {
        let Component::Normal(value) = component else {
            continue;
        };
        current.push(value);
        if let Ok(metadata) = fs::symlink_metadata(&current) {
            if metadata.file_type().is_symlink() {
                bail!(
                    "archive entry '{}' traverses symlink '{}'",
                    relative.display(),
                    current.display()
                );
            }
            if !metadata.is_dir() {
                bail!(
                    "archive entry '{}' traverses non-directory '{}'",
                    relative.display(),
                    current.display()
                );
            }
        }
    }
    Ok(())
}

/// Require and normalize one SHA-256 value from configuration.
///
/// # Errors
///
/// Returns an error when the value is absent or not exactly 64 hexadecimal
/// digits.
pub fn require_sha256(value: Option<&str>, description: &str) -> Result<String> {
    let value = value
        .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or_else(|| miette::miette!("{description} has no valid pinned SHA256"))?;
    Ok(value.to_ascii_lowercase())
}

/// Return whether a path names an executable regular file.
pub fn command_exists(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn state_paths_must_be_absolute() {
        assert!(require_absolute_state_path("AROS_HOME", PathBuf::from(".aros")).is_err());
        assert_eq!(
            require_absolute_state_path("AROS_HOME", PathBuf::from("/var/tmp/aros")).unwrap(),
            PathBuf::from("/var/tmp/aros")
        );
    }

    #[test]
    fn rejects_archive_and_symlink_escape() {
        assert!(safe_stripped_path(Path::new("root/../escape"), 1).is_err());
        assert!(
            validate_symlink_target(Path::new("bin/clang"), Path::new("../../escape")).is_err()
        );
        assert!(validate_symlink_target(Path::new("bin/clang++"), Path::new("clang")).is_ok());
    }

    #[test]
    fn verifies_sha256_and_tree_changes() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("payload");
        fs::write(&file, b"first").unwrap();
        let digest = sha256_file(&file).unwrap();
        verify_archive(&file, &digest, Some(5)).unwrap();
        assert!(verify_archive(&file, &"0".repeat(64), Some(5)).is_err());

        let first_tree = tree_sha256(directory.path()).unwrap();
        fs::write(&file, b"second").unwrap();
        assert_ne!(first_tree, tree_sha256(directory.path()).unwrap());
    }

    #[test]
    fn download_urls_are_credential_free_https_origins() {
        assert!(validate_download_url("https://example.invalid/archive.tar.xz").is_ok());
        for invalid in [
            "http://example.invalid/archive.tar.xz",
            "https://user@example.invalid/archive.tar.xz",
            "https://example.invalid/archive.tar.xz?token=secret",
            "https://example.invalid/archive.tar.xz#fragment",
            "file:///tmp/archive.tar.xz",
        ] {
            assert!(
                validate_download_url(invalid).is_err(),
                "accepted {invalid}"
            );
        }
    }

    #[test]
    fn extraction_budget_rejects_entry_and_expanded_size_bombs() {
        let mut entries = ExtractionBudget {
            entries: MAX_ARCHIVE_ENTRIES,
            expanded_bytes: 0,
        };
        assert!(entries.account(false, 0).is_err());

        let mut bytes = ExtractionBudget {
            entries: 0,
            expanded_bytes: MAX_EXPANDED_ARCHIVE_BYTES,
        };
        assert!(bytes.account(true, 1).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn staging_commit_is_durable_and_never_clobbers_an_install() {
        let store = tempfile::tempdir().unwrap();
        let destination = store.path().join("installed-toolchain");
        let staging = tempfile::Builder::new()
            .prefix(".install-")
            .tempdir_in(store.path())
            .unwrap();
        fs::write(staging.path().join("payload"), b"first").unwrap();

        commit_staging(&staging, &destination).unwrap();
        assert_eq!(fs::read(destination.join("payload")).unwrap(), b"first");

        let conflicting = tempfile::Builder::new()
            .prefix(".install-")
            .tempdir_in(store.path())
            .unwrap();
        fs::write(conflicting.path().join("payload"), b"second").unwrap();
        let error = commit_staging(&conflicting, &destination).unwrap_err();
        assert!(format!("{error:?}").contains("Conflict"));
        assert_eq!(fs::read(destination.join("payload")).unwrap(), b"first");
        assert_eq!(
            fs::read(conflicting.path().join("payload")).unwrap(),
            b"second"
        );
    }

    #[cfg(unix)]
    #[test]
    fn command_resolution_requires_an_executable_regular_target() {
        use std::os::unix::fs::{symlink, PermissionsExt as _};

        let directory = tempfile::tempdir().unwrap();
        let tool = directory.path().join("tool");
        fs::write(&tool, b"#!/bin/sh\n").unwrap();
        assert!(!command_exists(&tool));

        fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(command_exists(&tool));
        assert!(!command_exists(directory.path()));

        let link = directory.path().join("tool-link");
        symlink(&tool, &link).unwrap();
        assert!(command_exists(&link));

        let dangling = directory.path().join("dangling");
        symlink(directory.path().join("missing"), &dangling).unwrap();
        assert!(!command_exists(&dangling));
    }

    #[cfg(unix)]
    #[test]
    fn archive_verification_rejects_symlinks() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let payload = directory.path().join("payload");
        let link = directory.path().join("link");
        fs::write(&payload, b"content").unwrap();
        symlink(&payload, &link).unwrap();
        let digest = sha256_file(&payload).unwrap();
        assert!(verify_archive(&link, &digest, Some(7)).is_err());
    }

    #[test]
    fn extracts_safe_tar_into_staging() {
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("payload.tar.xz");
        let output = File::create(&archive_path).unwrap();
        let encoder = xz2::write::XzEncoder::new(output, 6);
        let mut builder = tar::Builder::new(encoder);
        let data = b"tool";
        let mut header = tar::Header::new_gnu();
        header.set_path("root/bin/clang").unwrap();
        header.set_size(data.len() as u64);
        header.set_mode(0o4777);
        header.set_cksum();
        builder.append(&header, &data[..]).unwrap();
        let encoder = builder.into_inner().unwrap();
        encoder.finish().unwrap().flush().unwrap();

        let staging = extract_to_staging(&archive_path, directory.path(), 1).unwrap();
        assert_eq!(fs::read(staging.path().join("bin/clang")).unwrap(), data);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;

            assert_eq!(
                fs::metadata(staging.path().join("bin/clang"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o7777,
                0o755
            );
        }
    }

    #[test]
    fn extraction_rejects_casefolding_path_collisions() {
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("collision.tar.xz");
        let output = File::create(&archive_path).unwrap();
        let encoder = xz2::write::XzEncoder::new(output, 6);
        let mut builder = tar::Builder::new(encoder);
        for path in ["root/bin/Tool", "root/bin/tool"] {
            let mut header = tar::Header::new_gnu();
            header.set_path(path).unwrap();
            header.set_size(1);
            header.set_mode(0o755);
            header.set_cksum();
            builder.append(&header, &b"x"[..]).unwrap();
        }
        let encoder = builder.into_inner().unwrap();
        encoder.finish().unwrap().flush().unwrap();

        assert!(extract_to_staging(&archive_path, directory.path(), 1).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn matches_producer_known_answer_with_unicode_and_symlink() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        for path in [
            "bin",
            "include/c++/v1",
            "lib/clang/11.0.0/lib/aros",
            "share/Größe",
        ] {
            fs::create_dir_all(root.join(path)).unwrap();
        }

        let mock_tool = include_bytes!("../tests/fixtures/mock-tool.sh");
        for tool in [
            "clang",
            "clang++",
            "ld.lld",
            "llvm-ar",
            "llvm-ranlib",
            "llvm-nm",
            "llvm-strip",
            "llvm-objcopy",
            "llvm-objdump",
        ] {
            let path = root.join("bin").join(tool);
            fs::write(&path, mock_tool).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }

        fs::write(
            root.join("include/c++/v1/vector"),
            b"// deterministic producer fixture\n",
        )
        .unwrap();
        for library in ["libc++.a", "libc++abi.a", "libunwind.a"] {
            fs::write(
                root.join("lib").join(library),
                format!("fixture {library}\n"),
            )
            .unwrap();
        }
        fs::write(
            root.join("lib/clang/11.0.0/lib/aros/libclang_rt.builtins-x86_64.a"),
            b"fixture x86_64 builtins\n",
        )
        .unwrap();
        fs::write(
            root.join("lib/clang/11.0.0/lib/aros/libclang_rt.builtins-i386.a"),
            b"fixture i386 builtins\n",
        )
        .unwrap();
        fs::write(
            root.join("share/Größe/marker-ä.txt"),
            b"UTF-8 inventory fixture\n",
        )
        .unwrap();
        symlink("../include/c++/v1/vector", root.join("share/vector-link")).unwrap();

        assert_eq!(
            tree_sha256(root).unwrap(),
            "4f78bdbc52ffbab2c6b337bb47d8c40b716574a82a03c2b0ac031ecca16fecef"
        );
    }

    #[test]
    fn matches_language_neutral_tree_digest_vector() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/tree-digest-v1.fixture.json"
        ))
        .unwrap();
        assert_eq!(fixture["schema"], "aros-toolchain-tree-digest-fixture-v1");
        let entries: Vec<ArosToolchainManifestEntry> =
            serde_json::from_value(fixture["entries"].clone()).unwrap();
        let expected = fixture["tree_sha256"].as_str().unwrap();
        assert_eq!(
            inventory_sha256(&entries).unwrap(),
            "11cbd45962f89c54c02fc9c1ae55eb283774b76425c08564da060bd5ca9c840b"
        );
        assert_eq!(inventory_sha256(&entries).unwrap(), expected);
    }
}
