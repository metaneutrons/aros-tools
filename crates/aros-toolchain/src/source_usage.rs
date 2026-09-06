//! Exact source-use closure checks for the private MetaMake fetch bridge.

use std::collections::BTreeSet;
use std::fs::File;
use std::io::Read;
use std::path::Path;

#[cfg(unix)]
use rustix::fs::{self as rfs, FileType, Mode, OFlags};

use crate::source_lock::SourceLock;
use crate::ContractError;

const MAX_USAGE_BYTES: u64 = 1024 * 1024;

/// Check that the recorded source use is exactly the lock's build-source set.
///
/// Host Python packages are environment inputs, not `FETCH` bridge inputs, and
/// are therefore deliberately excluded from this ledger.
///
/// # Errors
///
/// Returns AX0302 when the ledger is missing, oversized, malformed or differs
/// from the selected source closure.  It never treats an incomplete ledger as
/// a successful subset.
pub fn verify(lock: &SourceLock, usage: &Path) -> Result<(), ContractError> {
    let text = read_usage(usage)?;
    verify_text(lock, &text)
}

pub(crate) fn verify_text(lock: &SourceLock, text: &str) -> Result<(), ContractError> {
    let mut observed = BTreeSet::new();
    for line in text.lines() {
        if !portable_basename(line) || !observed.insert(line) {
            return Err(ContractError::source_use(
                "verified source-use ledger contains an unsafe or duplicate payload name",
            ));
        }
    }
    let expected = lock
        .sources()
        .map(crate::source_lock::Payload::filename)
        .collect::<BTreeSet<_>>();
    if observed != expected {
        return Err(ContractError::source_use(
            "observed MetaMake source use differs from the selected source-lock closure",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn read_usage(usage: &Path) -> Result<String, ContractError> {
    let mut file = File::from(
        rfs::open(
            usage,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| ContractError::source_use("verified source-use ledger is unavailable"))?,
    );
    let metadata = rfs::fstat(&file)
        .map_err(|_| ContractError::source_use("cannot inspect verified source-use ledger"))?;
    if !FileType::from_raw_mode(metadata.st_mode).is_file()
        || metadata.st_size < 0
        || u64::try_from(metadata.st_size)
            .ok()
            .is_none_or(|size| size > MAX_USAGE_BYTES)
    {
        return Err(ContractError::source_use(
            "verified source-use ledger is not a bounded regular file",
        ));
    }
    let expected = usize::try_from(metadata.st_size)
        .map_err(|_| ContractError::source_use("verified source-use ledger has an invalid size"))?;
    let mut text = String::with_capacity(expected);
    Read::by_ref(&mut file)
        .take(MAX_USAGE_BYTES.saturating_add(1))
        .read_to_string(&mut text)
        .map_err(|_| ContractError::source_use("verified source-use ledger is not UTF-8"))?;
    let after = rfs::fstat(&file)
        .map_err(|_| ContractError::source_use("cannot recheck verified source-use ledger"))?;
    if text.len() != expected || !same_stat(&metadata, &after) {
        return Err(ContractError::source_use(
            "verified source-use ledger changed while it was read",
        ));
    }
    Ok(text)
}

#[cfg(unix)]
const fn same_stat(left: &rfs::Stat, right: &rfs::Stat) -> bool {
    left.st_dev == right.st_dev
        && left.st_ino == right.st_ino
        && left.st_mode == right.st_mode
        && left.st_nlink == right.st_nlink
        && left.st_size == right.st_size
        && left.st_mtime == right.st_mtime
        && left.st_mtime_nsec == right.st_mtime_nsec
        && left.st_ctime == right.st_ctime
        && left.st_ctime_nsec == right.st_ctime_nsec
}

#[cfg(not(unix))]
fn read_usage(_usage: &Path) -> Result<String, ContractError> {
    Err(ContractError::source_use(
        "verified source-use ledgers require a supported Unix host",
    ))
}

fn portable_basename(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._+-".contains(&byte))
}
