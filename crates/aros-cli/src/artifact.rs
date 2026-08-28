//! Verified download, extraction, inventory, and atomic publication helpers.

use futures_util::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use miette::{bail, IntoDiagnostic, Result, WrapErr};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Component, Path, PathBuf};
use tempfile::TempDir;
use xz2::read::XzDecoder;

use aros_common::toolchain_manifest::{ArosToolchainManifestEntry, AROS_TOOLCHAIN_MANIFEST_FILE};

/// Marker published only after a toolchain envelope is complete.
pub const INSTALL_COMPLETE_FILE: &str = ".complete";

/// Resolve the user-controlled AROS state directory.
pub fn aros_home() -> PathBuf {
    std::env::var_os("AROS_HOME").map_or_else(
        || {
            std::env::var_os("HOME").map_or_else(
                || PathBuf::from(".aros"),
                |home| PathBuf::from(home).join(".aros"),
            )
        },
        PathBuf::from,
    )
}

/// Return the content-addressed archive-cache root.
pub fn archive_cache_root() -> PathBuf {
    std::env::var_os("AROS_CACHE_DIR").map_or_else(|| aros_home().join("cache"), PathBuf::from)
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
    let metadata = fs::metadata(path)
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to inspect archive '{}'", path.display()))?;
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
    let cache_dir = archive_cache_root().join("downloads").join("sha256");
    fs::create_dir_all(&cache_dir)
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to create cache '{}'", cache_dir.display()))?;
    let cache_path = cache_dir.join(format!("{}.tar.xz", expected_sha256.to_ascii_lowercase()));

    if cache_path.exists() && !force_download {
        verify_archive(&cache_path, expected_sha256, expected_size)?;
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
        .user_agent("aros-tools/0.1.0 (https://aros.org)")
        .build()
        .into_diagnostic()?;
    let response = client
        .get(url)
        .send()
        .await
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to download '{url}'"))?;
    if !response.status().is_success() {
        bail!("failed to download '{url}': HTTP {}", response.status());
    }

    let progress = ProgressBar::new(response.content_length().unwrap_or(0));
    progress.set_style(
        ProgressStyle::default_bar()
            .template(
                "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})",
            )
            .expect("valid built-in progress template")
            .progress_chars("#>-"),
    );

    let named = tempfile::NamedTempFile::new_in(&cache_dir)
        .into_diagnostic()
        .wrap_err("failed to create temporary archive in cache")?;
    let (file, temp_path) = named.into_parts();
    drop(file);
    let mut output = tokio::fs::File::create(&temp_path)
        .await
        .into_diagnostic()
        .wrap_err("failed to open temporary archive")?;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk
            .into_diagnostic()
            .wrap_err("failed while downloading toolchain archive")?;
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
    drop(output);
    progress.finish_with_message("downloaded");

    verify_archive(&temp_path, expected_sha256, expected_size)?;
    if force_download && cache_path.exists() {
        fs::remove_file(&cache_path)
            .into_diagnostic()
            .wrap_err_with(|| format!("failed to replace cache '{}'", cache_path.display()))?;
    }
    match temp_path.persist(&cache_path) {
        Ok(()) => {}
        Err(error) if cache_path.exists() => {
            drop(error);
            verify_archive(&cache_path, expected_sha256, expected_size)?;
        }
        Err(error) => {
            return Err(error.error).into_diagnostic().wrap_err_with(|| {
                format!("failed to commit archive cache '{}'", cache_path.display())
            });
        }
    }
    Ok(cache_path)
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
    let decoder = XzDecoder::new(BufReader::new(input));
    let mut archive = tar::Archive::new(decoder);
    for entry in archive
        .entries()
        .into_diagnostic()
        .wrap_err("failed to read tar archive")?
    {
        let mut entry = entry.into_diagnostic().wrap_err("invalid tar entry")?;
        let source_path = entry
            .path()
            .into_diagnostic()
            .wrap_err("invalid tar entry path")?;
        let Some(relative_path) = safe_stripped_path(&source_path, strip_components)? else {
            continue;
        };
        ensure_no_symlink_ancestors(staging.path(), &relative_path)?;

        let entry_type = entry.header().entry_type();
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
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).into_diagnostic()?;
        }
        entry
            .unpack(&destination)
            .into_diagnostic()
            .wrap_err_with(|| format!("failed to extract '{}'", relative_path.display()))?;
    }
    Ok(staging)
}

/// Atomically publish a fully verified staging tree at `destination`.
///
/// # Errors
///
/// Returns an error when the destination already exists or the final rename
/// cannot be completed on the same filesystem.
pub fn commit_staging(staging: &TempDir, destination: &Path) -> Result<()> {
    if destination.exists() {
        bail!("destination '{}' already exists", destination.display());
    }
    let parent = destination
        .parent()
        .ok_or_else(|| miette::miette!("toolchain destination has no parent"))?;
    fs::create_dir_all(parent).into_diagnostic()?;
    fs::rename(staging.path(), destination)
        .into_diagnostic()
        .wrap_err_with(|| {
            format!(
                "failed to atomically install toolchain at '{}'",
                destination.display()
            )
        })?;
    Ok(())
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
    let mut entries = Vec::new();
    collect_tree_entries(root, Path::new(""), &mut entries)?;
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
        if child_relative == Path::new(AROS_TOOLCHAIN_MANIFEST_FILE) {
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
            collect_tree_entries(root, &child_relative, output)?;
        } else if metadata.is_file() {
            output.push(ArosToolchainManifestEntry {
                path,
                mode: format!("{:04o}", normalized_file_mode(&metadata)),
                kind: "file".into(),
                sha256: Some(sha256_file(&entry.path())?),
                size: Some(metadata.len()),
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
    path.is_file()
        || path
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

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
        header.set_mode(0o755);
        header.set_cksum();
        builder.append(&header, &data[..]).unwrap();
        let encoder = builder.into_inner().unwrap();
        encoder.finish().unwrap().flush().unwrap();

        let staging = extract_to_staging(&archive_path, directory.path(), 1).unwrap();
        assert_eq!(fs::read(staging.path().join("bin/clang")).unwrap(), data);
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

        let mock_tool = include_bytes!("../../../../../scripts/toolchain/tests/mock-tool.sh");
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
            "946ecbea134cf9e109ac37cf28786765174228a64ed1dab23877515a38aa738b"
        );
    }

    #[test]
    fn matches_language_neutral_tree_digest_vector() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../toolchains/tree-digest-v1.fixture.json"
        ))
        .unwrap();
        assert_eq!(
            fixture["schema"],
            "aros-ng-toolchain-tree-digest-fixture-v1"
        );
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
