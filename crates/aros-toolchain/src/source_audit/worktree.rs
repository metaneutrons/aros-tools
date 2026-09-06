//! No-follow worktree enumeration and byte comparison. Git metadata is the
//! only exemption, at each independently verified repository's root only.

use std::{collections::BTreeSet, fs::File, io::Read};

use aros_common::{sha256_bytes, sha256_reader};
use rustix::fs::{self as fs, AtFlags, Mode, OFlags, Stat};

use super::{
    inventory::{Entry, Inventory},
    mismatch, Budget, MAX_ENTRIES,
};
use crate::{
    filesystem::{open_directory, DIRECTORY},
    inspection::Checkout,
    ContractError,
};

pub(super) fn verify(
    checkout: &Checkout<'_>,
    entries: &Inventory,
    budget: &mut Budget,
    depth: usize,
) -> Result<(), ContractError> {
    let root = open_directory(checkout.root).map_err(io_failure)?;
    let before = fs::fstat(&root).map_err(io_failure)?;
    let mut seen = BTreeSet::new();
    visit(checkout, &root, "", entries, &mut seen, budget, depth)?;
    if let Some(missing) = entries.keys().find(|path| !seen.contains(*path)) {
        return Err(mismatch("worktree is missing committed source entries").source_path(missing));
    }
    let rebound = open_directory(checkout.root).map_err(io_failure)?;
    if !same(&before, &fs::fstat(&rebound).map_err(io_failure)?) {
        return Err(mismatch("source root changed during recursive inspection"));
    }
    Ok(())
}

fn visit(
    checkout: &Checkout<'_>,
    directory: &File,
    prefix: &str,
    entries: &Inventory,
    seen: &mut BTreeSet<String>,
    budget: &mut Budget,
    depth: usize,
) -> Result<(), ContractError> {
    budget.check(depth)?;
    let before = fs::fstat(directory).map_err(io_failure)?;
    let names = names(directory)?;
    for name in &names {
        budget.check(depth)?;
        let stat =
            fs::statat(directory, name.as_str(), AtFlags::SYMLINK_NOFOLLOW).map_err(io_failure)?;
        let kind = fs::FileType::from_raw_mode(stat.st_mode);
        if prefix.is_empty() && name == ".git" {
            if !kind.is_file() && !kind.is_dir() {
                return Err(mismatch(
                    "Git metadata must not be a symlink or special file",
                ));
            }
            continue;
        }
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        let entry = entries.get(&path).ok_or_else(|| mismatch("worktree contains an undeclared entry (including ignored, untracked or empty directories)"))?;
        match entry.mode {
            "040000" | "160000" if kind.is_dir() => {
                let child = File::from(
                    fs::openat(directory, name.as_str(), DIRECTORY, Mode::empty())
                        .map_err(io_failure)?,
                );
                if !same(&stat, &fs::fstat(&child).map_err(io_failure)?) {
                    return Err(mismatch("source directory changed before inspection"));
                }
                if entry.mode == "160000" {
                    submodule(checkout, &path, entry, budget, depth + 1)
                        .map_err(|error| error.source_path(&path))?;
                } else {
                    visit(checkout, &child, &path, entries, seen, budget, depth + 1)?;
                }
                if !same(&stat, &fs::fstat(&child).map_err(io_failure)?) {
                    return Err(mismatch("source directory changed during inspection"));
                }
            }
            "100644" | "100755" if kind.is_file() => {
                regular(directory, name, &stat, entry).map_err(|error| error.source_path(&path))?;
            }
            "120000" if kind.is_symlink() => {
                let target =
                    fs::readlinkat(directory, name.as_str(), Vec::new()).map_err(io_failure)?;
                if target.as_bytes().len() != entry.size
                    || Some(sha256_bytes(target.as_bytes())) != entry.digest
                {
                    return Err(
                        mismatch("source symlink target differs from its raw Git blob")
                            .source_path(&path),
                    );
                }
            }
            _ => {
                return Err(
                    mismatch("worktree entry kind differs from its committed Git mode")
                        .source_path(&path),
                )
            }
        }
        if !same(
            &stat,
            &fs::statat(directory, name.as_str(), AtFlags::SYMLINK_NOFOLLOW).map_err(io_failure)?,
        ) {
            return Err(mismatch(
                "source entry changed while inspecting its content",
            ));
        }
        seen.insert(path);
    }
    if !same(&before, &fs::fstat(directory).map_err(io_failure)?)
        || names != self::names(directory)?
    {
        return Err(mismatch(
            "source directory membership changed during inspection",
        ));
    }
    Ok(())
}

fn regular(parent: &File, name: &str, before: &Stat, entry: &Entry) -> Result<(), ContractError> {
    let flags = OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK;
    let mut file = File::from(fs::openat(parent, name, flags, Mode::empty()).map_err(io_failure)?);
    if !same(before, &fs::fstat(&file).map_err(io_failure)?) {
        return Err(mismatch("source file changed before reading"));
    }
    // Git records the owner executable bit, not ambient group/other/umask bits.
    if (before.st_mode & 0o100 != 0) != (entry.mode == "100755")
        || u64::try_from(before.st_size).ok() != Some(entry.size as u64)
    {
        return Err(mismatch(
            "source file size or executable bit differs from its committed blob",
        ));
    }
    let observed =
        sha256_reader(&mut (&mut file).take(entry.size as u64 + 1)).map_err(io_failure)?;
    if observed.size != entry.size as u64
        || Some(observed.digest) != entry.digest
        || !same(before, &fs::fstat(&file).map_err(io_failure)?)
    {
        return Err(mismatch(
            "source file differs from its committed raw bytes or changed while reading",
        ));
    }
    Ok(())
}

fn submodule(
    parent: &Checkout<'_>,
    path: &str,
    entry: &Entry,
    budget: &mut Budget,
    depth: usize,
) -> Result<(), ContractError> {
    budget.check(depth)?;
    let root = parent.root.join(path);
    // No .gitmodules URL lookup, submodule update, shell or lazy fetch. An
    // absent/deinitialized module fails; a populated module must be its own root.
    let checkout = Checkout::submodule(&root, &entry.oid, parent.deadline)?;
    super::verify(&checkout, budget, depth)
}

fn names(directory: &File) -> Result<BTreeSet<String>, ContractError> {
    let mut names = BTreeSet::new();
    for entry in fs::Dir::read_from(directory).map_err(io_failure)? {
        let entry = entry.map_err(io_failure)?;
        let bytes = entry.file_name().to_bytes();
        if matches!(bytes, b"." | b"..") {
            continue;
        }
        let name = std::str::from_utf8(bytes)
            .map_err(|_| mismatch("worktree contains a non-UTF-8 name"))?;
        if !names.insert(name.to_owned()) || names.len() > MAX_ENTRIES {
            return Err(mismatch(
                "worktree enumeration is unstable or exceeds 200000 entries",
            ));
        }
    }
    Ok(names)
}

const fn same(left: &Stat, right: &Stat) -> bool {
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

fn io_failure(_error: impl std::fmt::Display) -> ContractError {
    ContractError::preflight("cannot inspect source through readable non-symlink directories and stable regular files; no checkout is changed")
}
