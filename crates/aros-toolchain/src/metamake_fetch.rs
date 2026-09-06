//! Private validated input bridge for upstream MetaMake `%fetch` calls.
//!
//! This is deliberately a library boundary, not a second `fetch` executable.
//! A later native lifecycle invokes it from the existing frontend before
//! handing the unchanged, source-owned arguments to the selected upstream
//! script. The bridge has no transport capability: a request resolves only to
//! one exact, already-verified source-lock archive in the explicit cache.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use aros_fetch::engine::cache::snapshot_verified_cache_payload;
use fs2::FileExt;
use rustix::fs::{self as rfs, Mode, OFlags};

use crate::filesystem::open_directory;
use crate::source_lock::SourceLock;
use crate::{source_usage, ContractError};

const LEDGER_LOCK_TIMEOUT: Duration = Duration::from_secs(10);
const LEDGER_POLL_INTERVAL: Duration = Duration::from_millis(20);
const MAX_LEDGER_BYTES: u64 = 1024 * 1024;

/// Parsed source-owned `%fetch` arguments, retained without shell evaluation.
#[derive(Debug, Clone)]
pub struct MetaMakeFetchInvocation {
    arguments: Vec<OsString>,
    archive: String,
    candidates: Vec<String>,
    location: Option<PathBuf>,
}

impl MetaMakeFetchInvocation {
    /// Parse only the source-selection fields relevant to the offline bridge.
    ///
    /// Unknown arguments remain opaque and are preserved for the selected
    /// upstream script. Shell metacharacters are not interpreted here or by
    /// this library.
    ///
    /// # Errors
    ///
    /// Returns AX0302 when archive selection, suffixes or the declared cache
    /// location is absent, ambiguous or unsafe.
    pub fn parse(arguments: &[OsString]) -> Result<Self, ContractError> {
        let mut archive = None;
        let mut suffixes = None;
        let mut location = None;
        let mut index = 0;
        while index < arguments.len() {
            let argument = os_text(&arguments[index])?;
            match argument {
                "-a" => {
                    let value = next_value(arguments, &mut index, "archive")?;
                    if archive.replace(value.to_owned()).is_some() || !portable_basename(value) {
                        return Err(ContractError::source_use(
                            "MetaMake fetch request has an unsafe or repeated archive name",
                        ));
                    }
                }
                "-s" => {
                    let value = next_value(arguments, &mut index, "suffix list")?;
                    if suffixes.replace(value.to_owned()).is_some() {
                        return Err(ContractError::source_use(
                            "MetaMake fetch request repeats its suffix list",
                        ));
                    }
                }
                "-l" => {
                    let value = next_value(arguments, &mut index, "cache location")?;
                    if location.replace(PathBuf::from(value)).is_some() {
                        return Err(ContractError::source_use(
                            "MetaMake fetch request repeats its cache location",
                        ));
                    }
                }
                _ => {}
            }
            index += 1;
        }
        let archive = archive.ok_or_else(|| {
            ContractError::source_use("MetaMake fetch request has no archive selection")
        })?;
        let candidates = match suffixes {
            Some(suffixes) => suffixes
                .split_ascii_whitespace()
                .map(|suffix| {
                    if portable_suffix(suffix) {
                        Ok(format!("{archive}.{suffix}"))
                    } else {
                        Err(ContractError::source_use(
                            "MetaMake fetch request has an unsafe archive suffix",
                        ))
                    }
                })
                .collect::<Result<Vec<_>, _>>()?,
            None => vec![archive.clone()],
        };
        if candidates.is_empty()
            || candidates.len()
                != candidates
                    .iter()
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
        {
            return Err(ContractError::source_use(
                "MetaMake fetch request has no candidate or repeats a candidate",
            ));
        }
        // The current qualified LLVM recipe names one `tar.xz` archive for
        // every `%fetch` target.  Accepting a fallback list would let the
        // unchanged upstream helper choose an unvalidated cache entry before
        // it reaches the one lock-selected candidate.  Expand the lock and
        // this bridge together if an upstream recipe genuinely needs a
        // multi-format source declaration.
        if candidates.len() != 1 {
            return Err(ContractError::source_use(
                "MetaMake fetch request must select exactly one locked archive candidate",
            ));
        }
        Ok(Self {
            arguments: arguments.to_vec(),
            archive,
            candidates,
            location,
        })
    }

    /// Resolve exactly one lock-selected source archive in the verified cache.
    ///
    /// The source-owned location must exactly equal `cache_root` after normal
    /// filesystem canonicalization. No network fallback, source execution or
    /// cache mutation is possible on this path.
    ///
    /// # Errors
    ///
    /// Returns AX0302 unless exactly one requested candidate is an unchanged
    /// source-lock entry in the explicitly selected cache root.
    pub fn resolve(
        &self,
        lock: &SourceLock,
        cache_root: &Path,
    ) -> Result<ResolvedMetaMakeSource, ContractError> {
        let cache = cache_root.canonicalize().map_err(|_| {
            ContractError::source_use("selected verified source cache cannot be canonicalized")
        })?;
        if !cache.is_dir() {
            return Err(ContractError::source_use(
                "selected verified source cache is not a directory",
            ));
        }
        let location = self.location.as_ref().ok_or_else(|| {
            ContractError::source_use("MetaMake fetch request has no explicit cache location")
        })?;
        let location = location.canonicalize().map_err(|_| {
            ContractError::source_use(
                "MetaMake fetch request cache location cannot be canonicalized",
            )
        })?;
        if location != cache {
            return Err(ContractError::source_use(
                "MetaMake fetch request selects a cache outside the verified source cache",
            ));
        }
        let selected = lock
            .sources()
            .filter(|payload| {
                self.candidates
                    .iter()
                    .any(|name| name == payload.filename())
            })
            .collect::<Vec<_>>();
        let [payload] = selected.as_slice() else {
            return Err(ContractError::source_use(
                "MetaMake fetch candidates do not resolve to exactly one locked source archive",
            ));
        };
        let snapshot = snapshot_verified_cache_payload(
            &cache,
            payload.filename(),
            payload.size(),
            payload.sha256(),
        )
        .map_err(|_| {
            ContractError::source_use(
                "MetaMake fetch selected a missing, unsafe or changed locked source archive",
            )
        })?;
        snapshot.revalidate().map_err(|_| {
            ContractError::source_use(
                "MetaMake fetch selected source archive changed during verification",
            )
        })?;
        Ok(ResolvedMetaMakeSource {
            arguments: self.arguments.clone(),
            archive: self.archive.clone(),
            filename: payload.filename().to_owned(),
        })
    }
}

/// One exact source selection authorized for the unchanged upstream script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedMetaMakeSource {
    arguments: Vec<OsString>,
    archive: String,
    filename: String,
}

impl ResolvedMetaMakeSource {
    /// Original arguments for the selected upstream `fetch.sh` invocation.
    #[must_use]
    pub fn arguments(&self) -> &[OsString] {
        &self.arguments
    }

    /// Source-owned archive stem from `-a`.
    #[must_use]
    pub fn archive(&self) -> &str {
        &self.archive
    }

    /// Exact source-lock filename selected by the invocation.
    #[must_use]
    pub fn filename(&self) -> &str {
        &self.filename
    }
}

/// Append-only, duplicate-rejecting source-use record owned by one build.
#[derive(Debug)]
pub struct SourceUseLedger {
    path: PathBuf,
}

impl SourceUseLedger {
    /// Create one fresh, regular ledger below an existing absolute parent.
    ///
    /// # Errors
    ///
    /// Returns AX0302 if the parent/path cannot be traversed without links or
    /// the selected ledger already exists. Existing ledgers are never adopted.
    pub fn create(path: &Path) -> Result<Self, ContractError> {
        let parent = path.parent().ok_or_else(|| {
            ContractError::source_use("source-use ledger has no parent directory")
        })?;
        let leaf = path.file_name().and_then(OsStr::to_str).ok_or_else(|| {
            ContractError::source_use("source-use ledger has no portable UTF-8 filename")
        })?;
        if !portable_basename(leaf) || !parent.is_absolute() {
            return Err(ContractError::source_use(
                "source-use ledger must be an absolute direct path with a portable filename",
            ));
        }
        let parent = canonical_system_parent(parent)?;
        let parent_file = open_directory(&parent).map_err(|_| {
            ContractError::source_use("source-use ledger parent is not a real directory")
        })?;
        let mut file = File::from(
            rfs::openat(
                &parent_file,
                leaf,
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            )
            .map_err(|_| ContractError::source_use("cannot create fresh source-use ledger"))?,
        );
        file.write_all(b"")
            .and_then(|()| file.sync_all())
            .map_err(|_| ContractError::source_use("cannot initialize fresh source-use ledger"))?;
        Ok(Self {
            path: parent.join(leaf),
        })
    }

    /// Record one lock-selected payload exactly once.
    ///
    /// A bounded advisory lock serializes concurrent `make` children. The
    /// record is synced before this function returns, so later success evidence
    /// never relies on a buffered append.
    ///
    /// # Errors
    ///
    /// Returns AX0302 when the ledger cannot be locked, changes, is malformed
    /// or would record one source archive more than once.
    pub fn record(&self, payload: &ResolvedMetaMakeSource) -> Result<(), ContractError> {
        let mut file = File::from(
            rfs::open(
                &self.path,
                OFlags::RDWR | OFlags::APPEND | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|_| ContractError::source_use("cannot safely open source-use ledger"))?,
        );
        lock_ledger(&file)?;
        let result = record_locked(&mut file, payload.filename());
        let unlock = FileExt::unlock(&file);
        match (result, unlock) {
            (Ok(()), Ok(())) => Ok(()),
            (Ok(()), Err(_)) => Err(ContractError::source_use(
                "cannot release exclusive source-use ledger access",
            )),
            (Err(error), _) => Err(error),
        }
    }

    /// Validate the final exact source closure after all successful MetaMake calls.
    ///
    /// # Errors
    ///
    /// Returns AX0302 when the durable ledger is incomplete, malformed or
    /// contains any undeclared source use.
    pub fn verify_complete(&self, lock: &SourceLock) -> Result<(), ContractError> {
        source_usage::verify(lock, &self.path)
    }

    /// Read-only path for controlled child-environment construction.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn next_value<'a>(
    arguments: &'a [OsString],
    index: &mut usize,
    label: &str,
) -> Result<&'a str, ContractError> {
    *index = index
        .checked_add(1)
        .ok_or_else(|| ContractError::source_use("MetaMake fetch argument index overflowed"))?;
    let value = arguments.get(*index).ok_or_else(|| {
        ContractError::source_use(format!("MetaMake fetch request omits its {label}"))
    })?;
    os_text(value)
}

fn os_text(value: &OsString) -> Result<&str, ContractError> {
    value.to_str().ok_or_else(|| {
        ContractError::source_use("MetaMake fetch request contains a non-UTF-8 argument")
    })
}

fn portable_basename(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._+-".contains(&byte))
}

fn portable_suffix(value: &str) -> bool {
    portable_basename(value) && !value.starts_with('.') && !value.ends_with('.')
}

fn canonical_system_parent(parent: &Path) -> Result<PathBuf, ContractError> {
    for system_root in [Path::new("/var"), Path::new("/tmp"), Path::new("/etc")] {
        if let Ok(relative) = parent.strip_prefix(system_root) {
            return system_root
                .canonicalize()
                .map(|root| root.join(relative))
                .map_err(|_| {
                    ContractError::source_use("cannot resolve source-use ledger system parent")
                });
        }
    }
    Ok(parent.to_path_buf())
}

fn lock_ledger(file: &File) -> Result<(), ContractError> {
    let deadline = Instant::now() + LEDGER_LOCK_TIMEOUT;
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(()),
            Err(_) if Instant::now() < deadline => thread::sleep(LEDGER_POLL_INTERVAL),
            Err(_) => {
                return Err(ContractError::source_use(
                    "timed out waiting for exclusive source-use ledger access",
                ))
            }
        }
    }
}

fn record_locked(file: &mut File, filename: &str) -> Result<(), ContractError> {
    let length = file
        .metadata()
        .map_err(|_| ContractError::source_use("cannot inspect source-use ledger"))?
        .len();
    if length > MAX_LEDGER_BYTES {
        return Err(ContractError::source_use(
            "source-use ledger exceeds the 1 MiB safety limit",
        ));
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|_| ContractError::source_use("cannot seek source-use ledger"))?;
    let mut existing = String::new();
    file.take(MAX_LEDGER_BYTES.saturating_add(1))
        .read_to_string(&mut existing)
        .map_err(|_| ContractError::source_use("source-use ledger is not UTF-8"))?;
    if existing.lines().any(|line| line == filename) {
        return Err(ContractError::source_use(
            "MetaMake fetch attempted to consume a locked source archive more than once",
        ));
    }
    file.write_all(filename.as_bytes())
        .and_then(|()| file.write_all(b"\n"))
        .and_then(|()| file.sync_all())
        .map_err(|_| ContractError::source_use("cannot append the source-use ledger"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use aros_common::{sha256_bytes, DiagnosticCode};
    use serde_json::json;

    use super::{MetaMakeFetchInvocation, SourceUseLedger};
    use crate::source_lock::SourceLock;

    fn lock(bytes: &[u8]) -> SourceLock {
        let document = json!({
            "schema": "aros-toolchain-source-lock-v2", "family": "llvm", "version": "11.0.0",
            "sources": [{
                "component": "llvm", "version": "11.0.0", "purpose": "toolchain-component",
                "filename": "llvm-11.0.0.src.tar.xz", "url": "https://example.invalid/llvm-11.0.0.src.tar.xz",
                "sha256": sha256_bytes(bytes), "size": bytes.len()
            }],
            "host_python_packages": [{
                "name": "mako", "version": "1.3.10", "filename": "mako.tar.gz",
                "url": "https://example.invalid/mako.tar.gz", "sha256": sha256_bytes(bytes), "size": bytes.len(),
                "source_root": "mako", "python_path": "."
            }]
        });
        SourceLock::parse(&serde_json::to_vec(&document).unwrap()).unwrap()
    }

    #[test]
    fn resolves_one_locked_source_and_records_one_durable_use() {
        let temporary = tempfile::tempdir().unwrap();
        let cache = temporary.path().join("cache");
        fs::create_dir(&cache).unwrap();
        let bytes = b"locked source\n";
        fs::write(cache.join("llvm-11.0.0.src.tar.xz"), bytes).unwrap();
        let invocation = MetaMakeFetchInvocation::parse(&[
            "-a".into(),
            "llvm-11.0.0.src".into(),
            "-s".into(),
            "tar.xz".into(),
            "-l".into(),
            cache.clone().into_os_string(),
        ])
        .unwrap();
        let selected = invocation.resolve(&lock(bytes), &cache).unwrap();
        assert_eq!(selected.filename(), "llvm-11.0.0.src.tar.xz");
        let ledger = SourceUseLedger::create(&temporary.path().join("use.log")).unwrap();
        ledger.record(&selected).unwrap();
        ledger.verify_complete(&lock(bytes)).unwrap();
        assert_eq!(
            fs::read_to_string(ledger.path()).unwrap(),
            "llvm-11.0.0.src.tar.xz\n"
        );
    }

    #[test]
    fn cannot_redirect_fetch_or_reuse_a_locked_archive() {
        let temporary = tempfile::tempdir().unwrap();
        let cache = temporary.path().join("cache");
        let elsewhere = temporary.path().join("elsewhere");
        fs::create_dir(&cache).unwrap();
        fs::create_dir(&elsewhere).unwrap();
        let bytes = b"locked source\n";
        fs::write(cache.join("llvm-11.0.0.src.tar.xz"), bytes).unwrap();
        let invocation = MetaMakeFetchInvocation::parse(&[
            "-a".into(),
            "llvm-11.0.0.src".into(),
            "-s".into(),
            "tar.xz".into(),
            "-l".into(),
            elsewhere.into_os_string(),
        ])
        .unwrap();
        let error = invocation.resolve(&lock(bytes), &cache).unwrap_err();
        assert_eq!(
            error.diagnostics().diagnostics[0].code,
            DiagnosticCode::ProducerSourceUse
        );

        let allowed = MetaMakeFetchInvocation::parse(&[
            "-a".into(),
            "llvm-11.0.0.src".into(),
            "-s".into(),
            "tar.xz".into(),
            "-l".into(),
            cache.into_os_string(),
        ])
        .unwrap()
        .resolve(&lock(bytes), &temporary.path().join("cache"))
        .unwrap();
        let ledger = SourceUseLedger::create(&temporary.path().join("use.log")).unwrap();
        ledger.record(&allowed).unwrap();
        assert!(ledger.record(&allowed).is_err());
    }

    #[test]
    fn rejects_fallback_suffix_lists_before_an_upstream_helper_can_choose_one() {
        let temporary = tempfile::tempdir().unwrap();
        let cache = temporary.path().join("cache");
        fs::create_dir(&cache).unwrap();
        let error = MetaMakeFetchInvocation::parse(&[
            "-a".into(),
            "llvm-11.0.0.src".into(),
            "-s".into(),
            "tar.xz zip".into(),
            "-l".into(),
            cache.into_os_string(),
        ])
        .unwrap_err();
        assert_eq!(
            error.diagnostics().diagnostics[0].code,
            DiagnosticCode::ProducerSourceUse
        );
    }
}
