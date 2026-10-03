//! Private validated input bridge for upstream MetaMake `%fetch` calls.
//!
//! This is deliberately a library boundary, not a second `fetch` executable.
//! A later native lifecycle invokes it from the existing frontend before
//! handing the unchanged, source-owned arguments to the selected upstream
//! script. The bridge has no transport capability: a request resolves only to
//! one exact, already-verified source-lock archive in the explicit cache.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use aros_common::Sha256Digest;
use aros_fetch::engine::cache::snapshot_verified_cache_payload;
use aros_fetch::engine::cache::VerifiedCachePayload;
use fs2::FileExt;
use rustix::fs::{self as rfs, Mode, OFlags};

use crate::filesystem::{open_directory, DIRECTORY};
use crate::source_lock::{CompilerFamily, SourceLock};
use crate::{source_usage, ContractError};

const LEDGER_LOCK_TIMEOUT: Duration = Duration::from_secs(10);
const LEDGER_POLL_INTERVAL: Duration = Duration::from_millis(20);
const MAX_LEDGER_BYTES: u64 = 1024 * 1024;
const MAX_GNU_CACHE_NAMESPACE_COMPONENTS: usize = 16;

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
    /// upstream script, except transport-policy overrides: offline fetching
    /// and checksum validation are mandatory at this boundary. Shell
    /// metacharacters are not interpreted here or by this library.
    ///
    /// # Errors
    ///
    /// Returns AX0302 when archive selection, suffixes or the declared cache
    /// location is absent, ambiguous or unsafe.
    pub fn parse(arguments: &[OsString]) -> Result<Self, ContractError> {
        Self::parse_inner(arguments, false)
    }

    /// Parse the selected family's source request without admitting an
    /// upstream format fallback. GNU lists are narrowed by [`Self::resolve`]
    /// to one exact lock-selected archive before any source helper runs.
    ///
    /// # Errors
    /// Returns AX0302 for unsafe, repeated or unbounded source selectors.
    pub fn parse_for_lock(
        arguments: &[OsString],
        lock: &SourceLock,
    ) -> Result<Self, ContractError> {
        Self::parse_inner(arguments, lock.family() == CompilerFamily::Gnu)
    }

    fn parse_inner(arguments: &[OsString], allow_gnu_list: bool) -> Result<Self, ContractError> {
        let mut archive = None;
        let mut suffixes = None;
        let mut location = None;
        let mut index = 0;
        while index < arguments.len() {
            let argument = os_text(&arguments[index])?;
            if matches!(argument, "--offline" | "--require-checksums")
                || argument.starts_with("--offline=")
                || argument.starts_with("--require-checksums=")
            {
                return Err(ContractError::source_use(
                    "MetaMake fetch request cannot override mandatory offline/checksum policy",
                ));
            }
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
        // Legacy/LLVM callers retain the single-format contract. GNU source
        // rules declare format lists, but no list reaches the upstream helper:
        // resolve proves a unique lock match and rewrites -s to that suffix.
        if candidates.len() > 16 || (!allow_gnu_list && candidates.len() != 1) {
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
    /// LLVM requires the source-owned location to exactly equal `cache_root`
    /// after normal filesystem canonicalization. GNU may name a bounded
    /// descendant namespace for its source-owned `.fetched` stamp; after the
    /// flat-cache payload is verified, this method creates only that empty
    /// directory namespace and rewrites the helper's `-l` argument to the
    /// verified flat cache root. It does not copy payloads or use the network.
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
        let gnu_namespace = if lock.family() == CompilerFamily::Gnu {
            Some(gnu_cache_namespace(location, cache_root, &cache)?)
        } else {
            let canonical_location = location.canonicalize().map_err(|_| {
                ContractError::source_use(
                    "MetaMake fetch request cache location cannot be canonicalized",
                )
            })?;
            if canonical_location != cache {
                return Err(ContractError::source_use(
                    "MetaMake fetch request selects a cache outside the verified source cache",
                ));
            }
            None
        };
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
        if let Some(namespace) = &gnu_namespace {
            ensure_gnu_cache_namespace(&cache, namespace)?;
        }
        let mut arguments = self.arguments.clone();
        if gnu_namespace.is_some() {
            let index = arguments
                .iter()
                .position(|value| value == "-l")
                .ok_or_else(|| {
                    ContractError::source_use("GNU cache namespace has no location argument")
                })?;
            arguments[index + 1] = cache.into_os_string();
        }
        if self.candidates.len() > 1 {
            if lock.family() != CompilerFamily::Gnu {
                return Err(ContractError::source_use(
                    "only GNU lock-bound requests admit a format list",
                ));
            }
            let suffix = payload
                .filename()
                .strip_prefix(&format!("{}.", self.archive))
                .ok_or_else(|| {
                    ContractError::source_use("locked archive differs from the requested basename")
                })?;
            let index = arguments
                .iter()
                .position(|value| value == "-s")
                .ok_or_else(|| {
                    ContractError::source_use("GNU format list has no suffix argument")
                })?;
            arguments[index + 1] = suffix.into();
        }
        Ok(ResolvedMetaMakeSource {
            arguments,
            archive: self.archive.clone(),
            filename: payload.filename().to_owned(),
            snapshot: Arc::new(snapshot),
        })
    }
}

fn gnu_cache_namespace(
    location: &Path,
    cache_root: &Path,
    canonical_cache: &Path,
) -> Result<Vec<PathBuf>, ContractError> {
    let relative = if location == cache_root || location == canonical_cache {
        Path::new("")
    } else if let Ok(relative) = location.strip_prefix(cache_root) {
        relative
    } else if let Ok(relative) = location.strip_prefix(canonical_cache) {
        relative
    } else {
        return Err(ContractError::source_use(
            "GNU MetaMake fetch location is outside the verified flat source cache",
        ));
    };

    let mut components = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(name) => components.push(PathBuf::from(name)),
            _ => {
                return Err(ContractError::source_use(
                    "GNU MetaMake fetch namespace contains a non-normal path component",
                ));
            }
        }
        if components.len() > MAX_GNU_CACHE_NAMESPACE_COMPONENTS {
            return Err(ContractError::source_use(
                "GNU MetaMake fetch namespace exceeds the 16-component limit",
            ));
        }
    }
    Ok(components)
}

fn ensure_gnu_cache_namespace(
    canonical_cache: &Path,
    components: &[PathBuf],
) -> Result<(), ContractError> {
    let mut current = open_directory(canonical_cache).map_err(|_| {
        ContractError::source_use("verified GNU source cache is not a real directory")
    })?;

    // Validate every existing ancestor before creating anything. Once a
    // component is absent, descendants cannot already exist beneath it.
    let mut first_missing = components.len();
    for (index, component) in components.iter().enumerate() {
        match rfs::openat(&current, component, DIRECTORY, Mode::empty()) {
            Ok(directory) => current = File::from(directory),
            Err(rustix::io::Errno::NOENT) => {
                first_missing = index;
                break;
            }
            Err(_) => {
                return Err(ContractError::source_use(
                    "GNU MetaMake fetch namespace has a symlink or non-directory ancestor",
                ));
            }
        }
    }

    for component in components.iter().skip(first_missing) {
        match rfs::mkdirat(&current, component, Mode::RUSR | Mode::WUSR | Mode::XUSR) {
            Ok(()) | Err(rustix::io::Errno::EXIST) => {}
            Err(_) => {
                return Err(ContractError::source_use(
                    "cannot create GNU MetaMake fetch stamp namespace",
                ));
            }
        }
        current = File::from(
            rfs::openat(&current, component, DIRECTORY, Mode::empty()).map_err(|_| {
                ContractError::source_use(
                    "GNU MetaMake fetch namespace has a symlink or non-directory ancestor",
                )
            })?,
        );
    }
    Ok(())
}

/// One exact source selection authorized for the unchanged upstream script.
#[derive(Clone)]
pub struct ResolvedMetaMakeSource {
    arguments: Vec<OsString>,
    archive: String,
    filename: String,
    snapshot: Arc<VerifiedCachePayload>,
}

impl fmt::Debug for ResolvedMetaMakeSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedMetaMakeSource")
            .field("arguments", &self.arguments)
            .field("archive", &self.archive)
            .field("filename", &self.filename)
            .finish_non_exhaustive()
    }
}

impl PartialEq for ResolvedMetaMakeSource {
    fn eq(&self, other: &Self) -> bool {
        self.arguments == other.arguments
            && self.archive == other.archive
            && self.filename == other.filename
    }
}

impl Eq for ResolvedMetaMakeSource {}

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

    /// Path to the private verified snapshot, not the mutable flat cache entry.
    #[must_use]
    pub fn snapshot_path(&self) -> &Path {
        self.snapshot.path()
    }

    /// SHA-256 digest measured and verified for the retained private snapshot.
    #[must_use]
    pub fn sha256(&self) -> &Sha256Digest {
        self.snapshot.sha256()
    }

    /// Byte size measured and verified for the retained private snapshot.
    #[must_use]
    pub fn size(&self) -> u64 {
        self.snapshot.size()
    }

    /// Revalidate both the source cache entry and retained snapshot.
    ///
    /// # Errors
    ///
    /// Returns AX0302 if the selected cache entry or private snapshot changed
    /// after source selection.
    pub fn revalidate(&self) -> Result<(), ContractError> {
        self.snapshot.revalidate().map_err(|_| {
            ContractError::source_use(
                "MetaMake fetch selected source archive changed after verification",
            )
        })
    }
}

/// Append-only, unique source-use record owned by one build.
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

    /// Reopen one previously created ledger for a controlled bridge invocation.
    ///
    /// The native lifecycle creates the ledger before starting `make`. Each
    /// source-owned MetaMake child then reopens that exact no-follow regular
    /// file to append one resolved payload. This does not adopt arbitrary
    /// historic state: callers must have reserved the enclosing work root and
    /// created the ledger in the same operation before any child starts.
    ///
    /// # Errors
    ///
    /// Returns AX0302 when the selected path is not an existing direct,
    /// bounded regular file below an absolute real directory.
    pub fn open(path: &Path) -> Result<Self, ContractError> {
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
        let file = File::from(
            rfs::openat(
                &parent_file,
                leaf,
                OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|_| ContractError::source_use("cannot safely open source-use ledger"))?,
        );
        let metadata = rfs::fstat(&file)
            .map_err(|_| ContractError::source_use("cannot inspect source-use ledger"))?;
        if !rfs::FileType::from_raw_mode(metadata.st_mode).is_file()
            || metadata.st_size < 0
            || u64::try_from(metadata.st_size)
                .ok()
                .is_none_or(|size| size > MAX_LEDGER_BYTES)
        {
            return Err(ContractError::source_use(
                "source-use ledger is not a bounded regular file",
            ));
        }
        Ok(Self {
            path: parent.join(leaf),
        })
    }

    /// Record one lock-selected payload on its first use.
    ///
    /// A bounded advisory lock serializes concurrent `make` children. The
    /// first record is synced before this function returns, so later success
    /// evidence never relies on a buffered append. Repeated requests for the
    /// same already-resolved payload are idempotent: the unchanged upstream
    /// MetaMake graph can request a component again in a later dependency
    /// phase, but it cannot expand the selected source closure or add a second
    /// ledger entry.
    ///
    /// # Errors
    ///
    /// Returns AX0302 when the ledger cannot be locked, changes or is
    /// malformed.
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
                ));
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
        return Ok(());
    }
    file.write_all(filename.as_bytes())
        .and_then(|()| file.write_all(b"\n"))
        .and_then(|()| file.sync_all())
        .map_err(|_| ContractError::source_use("cannot append the source-use ledger"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::path::Path;

    use aros_common::{sha256_bytes, DiagnosticCode};
    use serde_json::json;

    use super::{MetaMakeFetchInvocation, SourceUseLedger, MAX_GNU_CACHE_NAMESPACE_COMPONENTS};
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

    fn gnu_lock_document(bytes: &[u8], filename: &str) -> serde_json::Value {
        let mut document: serde_json::Value =
            serde_json::from_slice(include_bytes!("../tests/fixtures/gnu-source-lock-v3.json"))
                .unwrap();
        document["sources"][0]["filename"] = json!(filename);
        document["sources"][0]["url"] = json!(format!("https://example.invalid/{filename}"));
        document["sources"][0]["sha256"] = json!(sha256_bytes(bytes));
        document["sources"][0]["size"] = json!(bytes.len());
        document
    }

    fn gnu_lock(bytes: &[u8], filename: &str) -> SourceLock {
        SourceLock::parse(&serde_json::to_vec(&gnu_lock_document(bytes, filename)).unwrap())
            .unwrap()
    }

    fn gnu_invocation(location: &Path, lock: &SourceLock) -> MetaMakeFetchInvocation {
        let arguments = [
            OsString::from("-D"),
            OsString::from("destination"),
            OsString::from("-a"),
            OsString::from("payload"),
            OsString::from("-s"),
            OsString::from("tar.gz tar.xz"),
            OsString::from("-P"),
            OsString::from("patch.diff"),
            OsString::from("-l"),
            location.as_os_str().to_owned(),
        ];
        MetaMakeFetchInvocation::parse_for_lock(&arguments, lock).unwrap()
    }

    #[test]
    fn transport_policy_cannot_be_overridden_by_opaque_arguments() {
        let mut arguments = vec!["-a".into(), "payload".into(), "-l".into(), "cache".into()];
        assert!(MetaMakeFetchInvocation::parse(&arguments).is_ok());
        for policy in [
            "--offline=false",
            "--offline",
            "--require-checksums=false",
            "--require-checksums",
        ] {
            arguments.push(policy.into());
            let error = MetaMakeFetchInvocation::parse(&arguments).unwrap_err();
            assert_eq!(
                error.diagnostics().diagnostics[0].code,
                DiagnosticCode::ProducerSourceUse
            );
            assert!(error
                .to_string()
                .contains("mandatory offline/checksum policy"));
            let gnu = gnu_lock(b"payload", "gcc.tar.xz");
            assert!(MetaMakeFetchInvocation::parse_for_lock(&arguments, &gnu).is_err());
            arguments.pop();
        }
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
        let path = temporary.path().join("use.log");
        let ledger = SourceUseLedger::create(&path).unwrap();
        let ledger = SourceUseLedger::open(ledger.path()).unwrap();
        ledger.record(&selected).unwrap();
        ledger.verify_complete(&lock(bytes)).unwrap();
        assert_eq!(
            fs::read_to_string(ledger.path()).unwrap(),
            "llvm-11.0.0.src.tar.xz\n"
        );
    }

    #[test]
    fn cannot_redirect_fetch_and_idempotently_reuse_a_locked_archive() {
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
        ledger.record(&allowed).unwrap();
        assert_eq!(
            fs::read_to_string(ledger.path()).unwrap(),
            "llvm-11.0.0.src.tar.xz\n"
        );
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

    #[test]
    fn gnu_format_lists_are_narrowed_to_one_verified_lock_payload() {
        let temporary = tempfile::tempdir().unwrap();
        let cache = temporary.path().canonicalize().unwrap();
        let bytes = b"selected GNU source";
        let mut document: serde_json::Value =
            serde_json::from_slice(include_bytes!("../tests/fixtures/gnu-source-lock-v3.json"))
                .unwrap();
        document["sources"][0]["sha256"] = json!(sha256_bytes(bytes));
        document["sources"][0]["size"] = json!(bytes.len());
        let selected = SourceLock::parse(&serde_json::to_vec(&document).unwrap()).unwrap();
        fs::write(cache.join("gcc.tar.xz"), bytes).unwrap();
        // An earlier unverified local format must not win upstream's search.
        fs::write(cache.join("gcc.tar.gz"), b"not the selected archive").unwrap();
        let arguments = [
            "-a".into(),
            "gcc".into(),
            "-s".into(),
            "tar.gz tar.xz tar.bz2".into(),
            "-l".into(),
            cache.clone().into_os_string(),
        ];
        let invocation = MetaMakeFetchInvocation::parse_for_lock(&arguments, &selected).unwrap();
        let resolved = invocation.resolve(&selected, &cache).unwrap();
        assert_eq!(resolved.filename(), "gcc.tar.xz");
        assert_eq!(resolved.arguments()[3], "tar.xz");
        assert_eq!(resolved.arguments()[..3], arguments[..3]);
        assert_eq!(resolved.arguments()[4..], arguments[4..]);
        assert_eq!(
            fs::read(cache.join("gcc.tar.gz")).unwrap(),
            b"not the selected archive"
        );

        fs::write(cache.join("gcc.tar.xz"), b"corrupt").unwrap();
        assert!(invocation.resolve(&selected, &cache).is_err());
        assert!(MetaMakeFetchInvocation::parse_for_lock(&arguments, &lock(bytes)).is_err());

        let mut second = document["sources"][0].clone();
        second["component"] = json!("gcc-format-alternative");
        second["filename"] = json!("gcc.tar.gz");
        document["sources"].as_array_mut().unwrap().push(second);
        let ambiguous = SourceLock::parse(&serde_json::to_vec(&document).unwrap()).unwrap();
        assert!(invocation.resolve(&ambiguous, &cache).is_err());
    }

    #[test]
    fn gnu_cache_location_is_normalized_after_flat_payload_verification() {
        let temporary = tempfile::tempdir().unwrap();
        let cache = temporary.path().join("cache");
        fs::create_dir(&cache).unwrap();
        let canonical_cache = cache.canonicalize().unwrap();
        let bytes = b"generic locked source payload";
        let source_lock = gnu_lock(bytes, "payload.tar.xz");
        fs::write(cache.join("payload.tar.xz"), bytes).unwrap();

        for location in [
            cache.clone(),
            canonical_cache.clone(),
            cache.join("stamps/gnu"),
        ] {
            let invocation = gnu_invocation(&location, &source_lock);
            let resolved = invocation.resolve(&source_lock, &cache).unwrap();
            assert_eq!(resolved.filename(), "payload.tar.xz");
            assert_eq!(resolved.arguments()[0], "-D");
            assert_eq!(resolved.arguments()[1], "destination");
            assert_eq!(resolved.arguments()[2], "-a");
            assert_eq!(resolved.arguments()[3], "payload");
            assert_eq!(resolved.arguments()[4], "-s");
            assert_eq!(resolved.arguments()[5], "tar.xz");
            assert_eq!(resolved.arguments()[6], "-P");
            assert_eq!(resolved.arguments()[7], "patch.diff");
            assert_eq!(resolved.arguments()[8], "-l");
            assert_eq!(resolved.arguments()[9], canonical_cache.as_os_str());
        }

        let namespace = cache.join("stamps/gnu");
        assert!(namespace.is_dir());
        assert!(!namespace.join("payload.tar.xz").exists());
    }

    #[test]
    fn invalid_gnu_payload_hash_does_not_create_stamp_namespace() {
        let bytes = b"generic locked source payload";
        let filename = "payload.tar.xz";

        let corrupt_temporary = tempfile::tempdir().unwrap();
        let corrupt_cache = corrupt_temporary.path().join("cache");
        fs::create_dir(&corrupt_cache).unwrap();
        fs::write(corrupt_cache.join(filename), b"corrupt payload").unwrap();
        let valid_lock = gnu_lock(bytes, filename);
        let corrupt_path = corrupt_cache.join("stamp-a/one/two");
        let corrupt_invocation = gnu_invocation(&corrupt_path, &valid_lock);
        assert!(corrupt_invocation
            .resolve(&valid_lock, &corrupt_cache)
            .is_err());
        assert!(!corrupt_cache.join("stamp-a").exists());

        let digest_temporary = tempfile::tempdir().unwrap();
        let digest_cache = digest_temporary.path().join("cache");
        fs::create_dir(&digest_cache).unwrap();
        fs::write(digest_cache.join(filename), bytes).unwrap();
        let mut wrong_digest = gnu_lock_document(bytes, filename);
        wrong_digest["sources"][0]["sha256"] =
            json!("0000000000000000000000000000000000000000000000000000000000000000");
        let wrong_digest_lock =
            SourceLock::parse(&serde_json::to_vec(&wrong_digest).unwrap()).unwrap();
        let wrong_digest_path = digest_cache.join("stamp-b/one/two");
        let wrong_digest_invocation = gnu_invocation(&wrong_digest_path, &wrong_digest_lock);
        assert!(wrong_digest_invocation
            .resolve(&wrong_digest_lock, &digest_cache)
            .is_err());
        assert!(!digest_cache.join("stamp-b").exists());
    }

    #[test]
    fn rejects_unsafe_gnu_stamp_namespaces_without_following_links() {
        let temporary = tempfile::tempdir().unwrap();
        let cache = temporary.path().join("cache");
        let outside = temporary.path().join("outside");
        let inside_target = cache.join("real-inside");
        fs::create_dir(&cache).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::create_dir(&inside_target).unwrap();
        fs::write(
            cache.join("payload.tar.xz"),
            b"generic locked source payload",
        )
        .unwrap();
        fs::write(cache.join("regular-file"), b"not a directory").unwrap();
        symlink(&inside_target, cache.join("linked-inside")).unwrap();
        symlink(&outside, cache.join("linked-outside")).unwrap();

        let bytes = b"generic locked source payload";
        let source_lock = gnu_lock(bytes, "payload.tar.xz");
        let mut too_deep = cache.clone();
        for index in 0..=MAX_GNU_CACHE_NAMESPACE_COMPONENTS {
            too_deep.push(format!("component-{index}"));
        }
        let unsafe_locations = [
            outside.join("external-stamp"),
            cache.join("../outside/traversal-stamp"),
            cache.join("linked-inside/new-stamp"),
            cache.join("linked-outside/new-stamp"),
            cache.join("regular-file/new-stamp"),
            too_deep,
        ];
        for location in unsafe_locations {
            let invocation = gnu_invocation(&location, &source_lock);
            assert!(
                invocation.resolve(&source_lock, &cache).is_err(),
                "accepted unsafe GNU namespace {}",
                location.display()
            );
        }
        assert!(!cache.join("../outside/traversal-stamp").exists());
        assert!(!cache.join("linked-inside/new-stamp").exists());
        assert!(!cache.join("linked-outside/new-stamp").exists());
        assert!(!cache.join("regular-file/new-stamp").exists());
        assert!(!cache.join("component-0").exists());
    }

    #[test]
    fn llvm_rejects_nested_cache_location_without_creating_it() {
        let temporary = tempfile::tempdir().unwrap();
        let cache = temporary.path().join("cache");
        fs::create_dir(&cache).unwrap();
        let bytes = b"locked LLVM source\n";
        fs::write(cache.join("llvm-11.0.0.src.tar.xz"), bytes).unwrap();
        let nested = cache.join("nested/stamp");
        let invocation = MetaMakeFetchInvocation::parse(&[
            "-a".into(),
            "llvm-11.0.0.src".into(),
            "-s".into(),
            "tar.xz".into(),
            "-l".into(),
            nested.into_os_string(),
        ])
        .unwrap();

        assert!(invocation.resolve(&lock(bytes), &cache).is_err());
        assert!(!cache.join("nested").exists());
    }

    #[test]
    fn resolved_source_retains_and_revalidates_private_snapshot() {
        let temporary = tempfile::tempdir().unwrap();
        let cache = temporary.path().join("cache");
        fs::create_dir(&cache).unwrap();
        let bytes = b"generic locked source payload";
        let source_lock = gnu_lock(bytes, "payload.tar.xz");
        let cache_payload = cache.join("payload.tar.xz");
        fs::write(&cache_payload, bytes).unwrap();
        let invocation = gnu_invocation(&cache, &source_lock);
        let resolved = invocation.resolve(&source_lock, &cache).unwrap();

        assert_eq!(resolved.size(), bytes.len() as u64);
        assert_eq!(resolved.sha256(), &sha256_bytes(bytes));
        assert_eq!(fs::read(resolved.snapshot_path()).unwrap(), bytes);
        resolved.revalidate().unwrap();

        fs::write(cache_payload, b"changed after resolve").unwrap();
        assert_eq!(fs::read(resolved.snapshot_path()).unwrap(), bytes);
        let error = resolved.revalidate().unwrap_err();
        assert_eq!(
            error.diagnostics().diagnostics[0].code,
            DiagnosticCode::ProducerSourceUse
        );
    }
}
