//! Private, no-follow snapshots of verified external payloads.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use aros_common::{sha256_reader, Sha256Digest};
use tempfile::{Builder, TempDir};

use super::{cache_failure, FailureHint, FetchResult, MAX_DOWNLOAD_BYTES};

#[cfg(unix)]
use rustix::fs::{self as rfs, Mode, OFlags, RenameFlags, CWD};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt as _;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct StableSnapshot {
    device: u64,
    inode: u64,
    mode: u32,
    size: u64,
    mtime: i64,
    mtime_nsec: i64,
    ctime: i64,
    ctime_nsec: i64,
}

/// Exact bytes imported into a private directory plus their source CAS.
pub(super) struct PreparedPayload {
    pub(super) name: String,
    pub(super) path: PathBuf,
    pub(super) digest: Sha256Digest,
    source: PathBuf,
    source_snapshot: StableSnapshot,
    _private: TempDir,
}

impl PreparedPayload {
    pub(super) fn import(source: &Path, name: &str, maximum: u64) -> FetchResult<Self> {
        #[cfg(not(unix))]
        {
            let _ = (source, name, maximum);
            return Err(cache_failure(
                "stable no-follow payload snapshots are unavailable on this host",
            ));
        }
        #[cfg(unix)]
        {
            let descriptor = rfs::open(
                source,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|error| {
                cache_failure(format!(
                    "cannot open payload '{}' without following links: {error}",
                    source.display()
                ))
            })?;
            let mut input = File::from(descriptor);
            let before = stable_snapshot(&input.metadata().map_err(|error| {
                cache_failure(format!(
                    "cannot inspect payload '{}': {error}",
                    source.display()
                ))
            })?)?;
            if before.size > maximum {
                return Err(cache_failure(format!(
                    "payload '{}' exceeds the {maximum}-byte safety limit",
                    source.display()
                )));
            }
            let private = Builder::new()
                .prefix(".aros-fetch-payload-")
                .tempdir()
                .map_err(|error| {
                    cache_failure(format!("cannot create private payload snapshot: {error}"))
                })?;
            let path = private.path().join("payload");
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(|error| {
                    cache_failure(format!("cannot create private payload file: {error}"))
                })?;
            let mut limited = input.by_ref().take(maximum.saturating_add(1));
            let copied = io::copy(&mut limited, &mut output).map_err(|error| {
                cache_failure(format!("cannot snapshot payload bytes: {error}"))
            })?;
            if copied > maximum || copied != before.size {
                return Err(cache_failure(format!(
                    "payload '{}' changed size while it was snapshotted",
                    source.display()
                )));
            }
            output.sync_all().map_err(|error| {
                cache_failure(format!("cannot sync private payload snapshot: {error}"))
            })?;
            let after = stable_snapshot(&input.metadata().map_err(|error| {
                cache_failure(format!(
                    "cannot remeasure payload '{}': {error}",
                    source.display()
                ))
            })?)?;
            if after != before {
                return Err(cache_failure(format!(
                    "payload '{}' changed while it was snapshotted",
                    source.display()
                )));
            }
            drop(output);
            let digest = hash_private(&path, name)?;
            Ok(Self {
                name: name.to_owned(),
                path,
                digest,
                source: source.to_path_buf(),
                source_snapshot: before,
                _private: private,
            })
        }
    }

    pub(super) fn revalidate(&self) -> FetchResult<()> {
        #[cfg(not(unix))]
        {
            return Err(cache_failure(
                "stable no-follow payload revalidation is unavailable on this host",
            ));
        }
        #[cfg(unix)]
        {
            let descriptor = rfs::open(
                &self.source,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|error| {
                cache_failure(format!(
                    "cannot reopen payload source '{}' for completion CAS: {error}",
                    self.source.display()
                ))
            })?;
            let mut source = File::from(descriptor);
            let before = stable_snapshot(&source.metadata().map_err(|error| {
                cache_failure(format!("cannot remeasure payload source: {error}"))
            })?)?;
            if before != self.source_snapshot {
                return Err(cache_failure(format!(
                    "payload source '{}' changed before source-tree commit",
                    self.source.display()
                )));
            }
            let digest = sha256_reader(&mut source)
                .map_err(|error| cache_failure(format!("cannot rehash payload source: {error}")))?
                .digest;
            let after = stable_snapshot(&source.metadata().map_err(|error| {
                cache_failure(format!("cannot finish payload source CAS: {error}"))
            })?)?;
            if after != before || digest != self.digest {
                return Err(cache_failure(format!(
                    "payload source '{}' changed while completing source-tree CAS",
                    self.source.display()
                )));
            }
            if hash_private(&self.path, &self.name)? != self.digest {
                return Err(cache_failure(format!(
                    "private payload snapshot '{}' changed before source-tree commit",
                    self.name
                )));
            }
            Ok(())
        }
    }
}

pub(super) fn publish_download_noclobber(
    temporary: &Path,
    destination: &Path,
    candidate: &str,
) -> FetchResult<()> {
    #[cfg(unix)]
    match rfs::renameat_with(CWD, temporary, CWD, destination, RenameFlags::NOREPLACE) {
        Ok(()) => {
            let parent = destination
                .parent()
                .ok_or_else(|| cache_failure("download destination has no parent"))?;
            let directory = rfs::open(
                parent,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|error| {
                cache_failure(format!("cannot open download cache parent: {error}"))
            })?;
            rfs::fsync(&directory).map_err(|error| {
                cache_failure(format!("cannot sync download cache parent: {error}"))
            })?;
            Ok(())
        }
        Err(rustix::io::Errno::EXIST) => {
            let staged = PreparedPayload::import(temporary, candidate, MAX_DOWNLOAD_BYTES)?;
            let installed = PreparedPayload::import(destination, candidate, MAX_DOWNLOAD_BYTES)?;
            if staged.digest != installed.digest {
                return Err(cache_failure(format!(
                    "candidate cache destination '{}' already contains different bytes",
                    destination.display()
                ))
                .with_hint("keep the existing immutable cache object or use a separate cache root for the changed payload"));
            }
            fs::remove_file(temporary).map_err(|error| {
                cache_failure(format!(
                    "cannot remove duplicate download staging file: {error}"
                ))
            })?;
            Ok(())
        }
        Err(error) => Err(cache_failure(format!(
            "cannot publish fetched payload '{}' without clobbering: {error}",
            destination.display()
        ))),
    }
    #[cfg(not(unix))]
    {
        let _ = (temporary, destination, candidate);
        Err(cache_failure(
            "durable no-clobber download publication is unavailable on this host",
        ))
    }
}

#[cfg(unix)]
fn stable_snapshot(metadata: &fs::Metadata) -> FetchResult<StableSnapshot> {
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(cache_failure("payload source is not a real regular file"));
    }
    Ok(StableSnapshot {
        device: metadata.dev(),
        inode: metadata.ino(),
        mode: metadata.mode(),
        size: metadata.size(),
        mtime: metadata.mtime(),
        mtime_nsec: metadata.mtime_nsec(),
        ctime: metadata.ctime(),
        ctime_nsec: metadata.ctime_nsec(),
    })
}

#[cfg(unix)]
fn hash_private(path: &Path, name: &str) -> FetchResult<Sha256Digest> {
    let descriptor = rfs::open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| cache_failure(format!("cannot open private payload '{name}': {error}")))?;
    let mut file = File::from(descriptor);
    let before = stable_snapshot(&file.metadata().map_err(|error| {
        cache_failure(format!("cannot inspect private payload '{name}': {error}"))
    })?)?;
    let result = sha256_reader(&mut file)
        .map_err(|error| cache_failure(format!("cannot hash private payload '{name}': {error}")))?;
    let after = stable_snapshot(&file.metadata().map_err(|error| {
        cache_failure(format!(
            "cannot remeasure private payload '{name}': {error}"
        ))
    })?)?;
    if before != after || result.size != before.size {
        return Err(cache_failure(format!(
            "private payload '{name}' changed while it was hashed"
        )));
    }
    Ok(result.digest)
}
