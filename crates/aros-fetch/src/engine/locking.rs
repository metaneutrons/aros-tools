//! No-follow, identity-revalidated advisory fetch locks.

use std::fs::File;
#[cfg(not(unix))]
use std::fs::OpenOptions;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use aros_common::sha256_bytes;
use fs2::FileExt as _;

#[cfg(debug_assertions)]
use super::fetch_test_pause;
use super::{cache_failure, FetchResult, LOCK_TIMEOUT};

#[cfg(unix)]
use rustix::fs::{self as rfs, Mode, OFlags};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt as _;

pub(super) struct FetchLock {
    file: File,
    path: PathBuf,
    #[cfg(unix)]
    identity: (u64, u64),
}

impl FetchLock {
    pub(super) fn acquire_candidate(candidate: &Path) -> FetchResult<Self> {
        let absolute = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|error| cache_failure(format!("cannot resolve candidate lock: {error}")))?
                .join(candidate)
        };
        let parent = absolute
            .parent()
            .ok_or_else(|| cache_failure("candidate cache path has no parent"))?;
        let digest = sha256_bytes(absolute.as_os_str().as_encoded_bytes()).to_string();
        Self::acquire_path(&parent.join(format!(".aros-fetch-candidate-{}.lock", &digest[..32])))
    }

    pub(super) fn acquire_destination(destination: &Path) -> FetchResult<Self> {
        let canonical = destination.canonicalize().map_err(|error| {
            cache_failure(format!(
                "cannot resolve fetch destination lock namespace '{}': {error}",
                destination.display()
            ))
        })?;
        let parent = canonical.parent().ok_or_else(|| {
            cache_failure("fetch destination lock namespace has no parent directory")
        })?;
        let digest = sha256_bytes(canonical.as_os_str().as_encoded_bytes()).to_string();
        Self::acquire_path(&parent.join(format!(".aros-fetch-destination-{}.lock", &digest[..32])))
    }

    pub(super) fn acquire_patch_base(base: &Path) -> FetchResult<Self> {
        Self::acquire_path(&base.join(".aros-fetch-patch-cache.lock"))
    }

    pub(super) fn acquire_path(path: &Path) -> FetchResult<Self> {
        #[cfg(debug_assertions)]
        fetch_test_pause("lock-before-open");
        #[cfg(unix)]
        let file = File::from(
            rfs::open(
                path,
                OFlags::CREATE | OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            )
            .map_err(|error| {
                cache_failure(format!(
                    "cannot open lock '{}' without following links: {error}",
                    path.display()
                ))
            })?,
        );
        #[cfg(not(unix))]
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)
            .map_err(|error| {
                cache_failure(format!("cannot open lock '{}': {error}", path.display()))
            })?;
        #[cfg(unix)]
        let identity = lock_identity(&file, path)?;
        let started = Instant::now();
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => {
                    #[cfg(unix)]
                    if lock_identity(&file, path)? != identity {
                        let _ = file.unlock();
                        return Err(cache_failure(format!(
                            "fetch lock '{}' changed while flock was acquired",
                            path.display()
                        )));
                    }
                    return Ok(Self {
                        file,
                        path: path.to_path_buf(),
                        #[cfg(unix)]
                        identity,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    if started.elapsed() >= LOCK_TIMEOUT {
                        return Err(cache_failure(format!(
                            "timed out waiting for fetch lock '{}' after {} seconds",
                            path.display(),
                            LOCK_TIMEOUT.as_secs()
                        )));
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(error) => {
                    return Err(cache_failure(format!(
                        "cannot acquire fetch lock '{}': {error}",
                        path.display()
                    )))
                }
            }
        }
    }

    pub(super) fn revalidate(&self) -> FetchResult<()> {
        #[cfg(unix)]
        if lock_identity(&self.file, &self.path)? != self.identity {
            return Err(cache_failure(format!(
                "fetch lock '{}' changed while protected work was running",
                self.path.display()
            )));
        }
        Ok(())
    }
}

#[cfg(unix)]
fn lock_identity(file: &File, path: &Path) -> FetchResult<(u64, u64)> {
    let descriptor = file.metadata().map_err(|error| {
        cache_failure(format!("cannot inspect lock '{}': {error}", path.display()))
    })?;
    if !descriptor.is_file() {
        return Err(cache_failure(format!(
            "fetch lock '{}' is not a regular file",
            path.display()
        )));
    }
    let path_metadata = std::fs::symlink_metadata(path).map_err(|error| {
        cache_failure(format!(
            "cannot revalidate lock '{}': {error}",
            path.display()
        ))
    })?;
    if !path_metadata.is_file()
        || path_metadata.file_type().is_symlink()
        || path_metadata.dev() != descriptor.dev()
        || path_metadata.ino() != descriptor.ino()
    {
        return Err(cache_failure(format!(
            "fetch lock '{}' no longer names its opened regular file",
            path.display()
        )));
    }
    Ok((descriptor.dev(), descriptor.ino()))
}

impl Drop for FetchLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}
