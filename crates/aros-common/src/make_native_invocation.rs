//! Bounded native child-call data, not producer or execution authority.
//!
//! Capturing the observer process does not identify the intended Make executable
//! or prove a parent, target, input closure, phase or successful Make execution.
//! Those independent bindings must be supplied by the supervising owner.

use crate::{
    make_native_collection::{LEGACY_NATIVE_MAKE_CALL_LIMIT, MAX_NATIVE_MAKE_CALLS},
    ArosError, Result,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fmt,
    path::{Component, Path, PathBuf},
};

const MAX_ENTRIES: usize = 4096;
const MAX_ARGUMENT_BYTES: usize = 128 * 1024;
const MAX_ENVIRONMENT_BYTES: usize = 128 * 1024;
const MAX_DOCUMENT_BYTES: usize = 2 * 1024 * 1024;
const MAX_CWD_BYTES: usize = 4096;
const MAX_PRIVATE_CAPTURE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    version: u32,
    arguments: Vec<String>,
    working_directory: PathBuf,
    // A sequence prevents duplicate names being silently overwritten on read.
    environment: Vec<(String, String)>,
    // Missing is the legacy version-1 representation. Explicit null is invalid.
    #[serde(
        default,
        deserialize_with = "deserialize_argument_zero",
        skip_serializing_if = "Option::is_none"
    )]
    argument_zero: Option<String>,
}

fn deserialize_argument_zero<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    String::deserialize(deserializer).map(Some)
}

/// Inert UTF-8 argument/cwd/environment snapshot. Debug never displays values.
///
/// Arguments exclude argv[0] and observer-control options. An optional
/// `argument_zero` is an explicit caller selection; it is neither inferred from
/// this process nor evidence of the executable that will run. Limits: 4096
/// arguments and 4096 environment entries, aggregate 128 KiB each (including
/// delimiters), 4096-byte Unix cwd, 2 MiB encoded JSON. When `argument_zero` is
/// present, its bytes and delimiter share the argument byte limit. No smaller
/// per-item cap is imposed. Environment names are opaque OS data, not shell/Make
/// identifiers; empty names are retained if the OS interface surfaces them. No
/// name is silently dropped.
#[derive(Clone, Eq, PartialEq)]
pub struct NativeMakeInvocation {
    arguments: Vec<String>,
    argument_zero: Option<String>,
    working_directory: PathBuf,
    environment: BTreeMap<String, String>,
}

impl fmt::Debug for NativeMakeInvocation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NativeMakeInvocation")
            .field("argument_count", &self.arguments.len())
            .field("environment_count", &self.environment.len())
            .finish_non_exhaustive()
    }
}

impl NativeMakeInvocation {
    /// Validate literal data without filesystem lookup or environment access.
    /// No variables are expanded and no names are filtered or synthesized.
    ///
    /// # Errors
    /// Refuses OS delimiters, duplicate keys, unsupported cwd and resource limits.
    pub fn new(
        arguments: impl IntoIterator<Item = String>,
        working_directory: PathBuf,
        environment: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Self> {
        normalized_absolute(&working_directory)?;
        if working_directory
            .to_str()
            .is_none_or(|path| path.len() > MAX_CWD_BYTES || path.contains('\0'))
        {
            return Err(invalid("unsupported native invocation working directory"));
        }
        let mut args = Vec::new();
        let mut argument_bytes = 0usize;
        for argument in arguments {
            argument_bytes = argument_bytes
                .checked_add(argument.len())
                .and_then(|n| n.checked_add(1))
                .filter(|n| *n <= MAX_ARGUMENT_BYTES)
                .ok_or_else(|| invalid("native invocation argument byte limit"))?;
            if args.len() == MAX_ENTRIES || argument.contains('\0') {
                return Err(invalid("native invocation argument count or OS delimiter"));
            }
            args.push(argument);
        }
        let mut values = BTreeMap::new();
        let mut environment_bytes = 0usize;
        for (name, value) in environment {
            environment_bytes = environment_bytes
                .checked_add(name.len())
                .and_then(|n| n.checked_add(value.len()))
                .and_then(|n| n.checked_add(2))
                .filter(|n| *n <= MAX_ENVIRONMENT_BYTES)
                .ok_or_else(|| invalid("native invocation environment byte limit"))?;
            if values.len() == MAX_ENTRIES || name.contains(['=', '\0']) || value.contains('\0') {
                return Err(invalid(
                    "native invocation environment count or OS delimiter",
                ));
            }
            if values.insert(name, value).is_some() {
                return Err(invalid("duplicate native invocation environment name"));
            }
        }
        Ok(Self {
            arguments: args,
            argument_zero: None,
            working_directory,
            environment: values,
        })
    }

    /// Select the child process's `argv[0]` string explicitly.
    ///
    /// This value controls only the argument string supplied at launch. It does
    /// not authenticate the process, select the executable path, or claim that
    /// the current process's actual `argv[0]` was observed.
    ///
    /// # Errors
    /// Refuses an empty string, an OS delimiter, or aggregate argument data
    /// beyond the existing 128 KiB bound.
    pub fn with_argument_zero(mut self, argument_zero: String) -> Result<Self> {
        if argument_zero.is_empty() || argument_zero.contains('\0') {
            return Err(invalid("invalid native invocation argument zero"));
        }
        let mut argument_bytes = argument_zero
            .len()
            .checked_add(1)
            .filter(|bytes| *bytes <= MAX_ARGUMENT_BYTES)
            .ok_or_else(|| invalid("native invocation argument byte limit"))?;
        for argument in &self.arguments {
            argument_bytes = argument_bytes
                .checked_add(argument.len())
                .and_then(|bytes| bytes.checked_add(1))
                .filter(|bytes| *bytes <= MAX_ARGUMENT_BYTES)
                .ok_or_else(|| invalid("native invocation argument byte limit"))?;
        }
        self.argument_zero = Some(argument_zero);
        Ok(self)
    }

    /// Parse bounded data. Successful parsing never grants execution authority.
    ///
    /// # Errors
    /// Refuses malformed/unknown fields, versions and all constructor errors.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_DOCUMENT_BYTES {
            return Err(invalid("native invocation document byte limit"));
        }
        let document: Document = serde_json::from_slice(bytes)
            .map_err(|_| invalid("invalid native invocation document"))?;
        match (document.version, document.argument_zero.is_some()) {
            (1, false) | (2, true) => {}
            (1, true) => return Err(invalid("version 1 native invocation has argument zero")),
            (2, false) => {
                return Err(invalid(
                    "version 2 native invocation is missing argument zero",
                ))
            }
            _ => return Err(invalid("unsupported native invocation version")),
        }
        let invocation = Self::new(
            document.arguments,
            document.working_directory,
            document.environment,
        )?;
        match document.argument_zero {
            Some(argument_zero) => invocation.with_argument_zero(argument_zero),
            None => Ok(invocation),
        }
    }

    /// Serialize validated data; the output may contain sensitive values.
    ///
    /// # Errors
    /// Refuses encoding failures or document-size exhaustion.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let document = Document {
            version: if self.argument_zero.is_some() { 2 } else { 1 },
            arguments: self.arguments.clone(),
            working_directory: self.working_directory.clone(),
            environment: self
                .environment
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect(),
            argument_zero: self.argument_zero.clone(),
        };
        let bytes = serde_json::to_vec(&document)
            .map_err(|_| invalid("cannot encode native invocation"))?;
        if bytes.len() > MAX_DOCUMENT_BYTES {
            return Err(invalid("native invocation encoded byte limit"));
        }
        Ok(bytes)
    }

    #[must_use]
    pub fn arguments(&self) -> &[String] {
        &self.arguments
    }
    #[must_use]
    pub fn argument_zero(&self) -> Option<&str> {
        self.argument_zero.as_deref()
    }
    #[must_use]
    pub fn working_directory(&self) -> &Path {
        &self.working_directory
    }
    #[must_use]
    pub const fn environment(&self) -> &BTreeMap<String, String> {
        &self.environment
    }
}

fn normalized_absolute(path: &Path) -> Result<()> {
    let Some(text) = path.to_str() else {
        return Err(invalid("native invocation path is not UTF-8"));
    };
    if !text.starts_with('/')
        || !path.is_absolute()
        || !path
            .components()
            .all(|part| matches!(part, Component::RootDir | Component::Normal(_)))
        || (text != "/"
            && text[1..]
                .split('/')
                .any(|part| part.is_empty() || matches!(part, "." | "..")))
    {
        return Err(invalid(
            "native invocation path must be normalized and absolute",
        ));
    }
    Ok(())
}

/// Capture current UTF-8 std-process arguments/environment to a private file.
/// This entry point intentionally reads this process; ordinary interpreters and
/// deserializers never do. The caller passes only the original child arguments,
/// excluding observer-control options. No program is executed. This uses the
/// standard process-environment interface, not a decoder for malformed raw
/// execve entries. Output requires canonical spelling (e.g. /private/tmp rather
/// than its /tmp symlink alias on macOS). Failed writes are retained; they must not be retried or
/// treated as completed observations. A supervising owner requires exit0.
///
/// # Errors
/// Refuses unrepresentable OS strings, invalid data, unsafe publication and I/O.
#[cfg(unix)]
fn open_capture_parent(path: &Path) -> Result<Vec<std::os::fd::OwnedFd>> {
    use rustix::fs::{self as rfs, Mode, OFlags};
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let root = rfs::open("/", flags, Mode::empty())
        .map_err(|_| invalid("cannot bind native capture root"))?;
    let mut chain = vec![root];
    for component in path.components() {
        if let Component::Normal(name) = component {
            let next = rfs::openat(
                chain.last().expect("root descriptor"),
                name,
                flags,
                Mode::empty(),
            )
            .map_err(|_| invalid("cannot bind native capture path component"))?;
            chain.push(next);
        }
    }
    recheck_capture_parent(path, &chain)?;
    Ok(chain)
}

#[cfg(unix)]
fn recheck_capture_parent(path: &Path, chain: &[std::os::fd::OwnedFd]) -> Result<()> {
    use rustix::fs::{self as rfs, AtFlags};
    for (index, name) in path
        .components()
        .filter_map(|part| match part {
            Component::Normal(name) => Some(name),
            _ => None,
        })
        .enumerate()
    {
        let named = rfs::statat(&chain[index], name, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|_| invalid("cannot recheck native capture path component"))?;
        let held = rfs::fstat(&chain[index + 1])
            .map_err(|_| invalid("cannot inspect native capture path component"))?;
        if named.st_dev != held.st_dev
            || named.st_ino != held.st_ino
            || named.st_mode != held.st_mode
        {
            return Err(invalid("native capture path component changed"));
        }
    }
    Ok(())
}

#[cfg(unix)]
/// Capture literal process data without running a child. Requires an owned
/// exact-0700 parent with no symlink in any path component.
///
/// Failed output is
/// retained and is not a completed observation; supervisors require exit0.
///
/// # Errors
/// Refuses unsupported OS data, unsafe paths, resource limits and I/O failures.
pub fn capture_current(output: &Path, arguments: &[OsString]) -> Result<()> {
    capture_current_with_snapshot(output, arguments).map(|_| ())
}

/// Capture and return the same literal process data written to the private file.
///
/// This avoids a later path reread when a live supervisor needs the snapshot.
/// It grants no child, program, phase or successful-execution authority.
///
/// # Errors
/// Refuses the same unsafe paths, data, resource bounds and I/O as `capture_current`.
#[cfg(unix)]
pub fn capture_current_with_snapshot(
    output: &Path,
    arguments: &[OsString],
) -> Result<NativeMakeInvocation> {
    capture_current_snapshot(output, arguments, None)
}

/// Capture current process data while explicitly selecting the child `argv[0]`.
///
/// Return the same snapshot that was published, without rereading its path.
/// The selection is not inferred from, or proof of, this process's actual
/// `argv[0]`; it does not authenticate the executable selected by a supervisor.
///
/// # Errors
/// Refuses the same unsafe paths, data, resource bounds and I/O as
/// [`capture_current_with_snapshot`], as well as an invalid selected string.
#[cfg(unix)]
pub fn capture_current_with_selected_argument_zero(
    output: &Path,
    arguments: &[OsString],
    argument_zero: &str,
) -> Result<NativeMakeInvocation> {
    capture_current_snapshot(output, arguments, Some(argument_zero))
}

#[cfg(unix)]
fn capture_current_snapshot(
    output: &Path,
    arguments: &[OsString],
    argument_zero: Option<&str>,
) -> Result<NativeMakeInvocation> {
    normalized_absolute(output)?;
    let parent = output
        .parent()
        .ok_or_else(|| invalid("missing native capture parent"))?;
    let directory = PrivateNativeCaptureDirectory::open(parent)?;
    let mut argument_text = Vec::new();
    let mut argument_bytes = 0usize;
    for argument in arguments {
        argument_bytes = argument_bytes
            .saturating_add(argument.as_encoded_bytes().len())
            .saturating_add(1);
        if argument_text.len() == MAX_ENTRIES || argument_bytes > MAX_ARGUMENT_BYTES {
            return Err(invalid("native capture argument limit"));
        }
        argument_text.push(
            argument
                .to_str()
                .ok_or_else(|| invalid("non-UTF-8 native invocation argument"))?
                .to_owned(),
        );
    }
    let mut environment = Vec::new();
    let mut environment_bytes = 0usize;
    for (name, value) in std::env::vars_os() {
        environment_bytes = environment_bytes
            .saturating_add(name.as_encoded_bytes().len())
            .saturating_add(value.as_encoded_bytes().len())
            .saturating_add(2);
        if environment.len() == MAX_ENTRIES || environment_bytes > MAX_ENVIRONMENT_BYTES {
            return Err(invalid("native capture environment limit"));
        }
        environment.push((
            name.into_string()
                .map_err(|_| invalid("non-UTF-8 native environment name"))?,
            value
                .into_string()
                .map_err(|_| invalid("non-UTF-8 native environment value"))?,
        ));
    }
    let mut snapshot = NativeMakeInvocation::new(
        argument_text,
        std::env::current_dir().map_err(|_| invalid("cannot inspect native invocation cwd"))?,
        environment,
    )?;
    if let Some(argument_zero) = argument_zero {
        snapshot = snapshot.with_argument_zero(argument_zero.to_owned())?;
    }
    let leaf = output
        .file_name()
        .ok_or_else(|| invalid("missing native capture output name"))?;
    directory.write_new(leaf, &snapshot.to_bytes()?)?;
    Ok(snapshot)
}

/// Held owned-0700 capture directory; all files are exclusive and mode0600.
///
/// Every path component is opened without following links and rechecked against
/// held descriptors before and after publication. No raw payload is formatted.
#[cfg(unix)]
pub struct PrivateNativeCaptureDirectory {
    path: PathBuf,
    chain: Vec<std::os::fd::OwnedFd>,
    initial: rustix::fs::Stat,
}

#[cfg(unix)]
impl PrivateNativeCaptureDirectory {
    /// Return the descriptive namespace path for this held private directory.
    ///
    /// The path is a locator only. It grants no producer authority and does not
    /// establish that an observation completed. A hostile process with the same
    /// uid on the host remains outside this primitive's threat model.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Permanently claim one bounded child-call namespace and return its held
    /// private directory. Slot numbers are data-collection namespace only; a
    /// claim or directory proves neither producer authority nor completion.
    /// Failed slots remain consumed and are never removed or reused.
    ///
    /// `maximum_calls` must be between 1 and 256 inclusive. The bounded method
    /// accepts up to 4096 slots.
    /// Existing claim files are skipped only when a held, no-follow descriptor
    /// proves an owned, exact-0600, empty, regular, single-link file. A fresh
    /// claim is created exclusively before its exact-0700 child directory.
    /// Existing child directories are never adopted. This does not defend
    /// against a hostile same-uid process on the host.
    /// If the inherited umask removes directory-owner read/search permissions,
    /// binding the fresh child may refuse; the claim stays consumed. This API
    /// never changes the process-global umask or repairs an unbound path.
    ///
    /// # Errors
    /// Refuses unsupported bounds, unsafe or replaced entries, exhausted slots,
    /// filesystem failures, or any parent/child identity or permission change.
    pub fn claim_observation_slot(&self, maximum_calls: usize) -> Result<(usize, Self)> {
        if !(1..=LEGACY_NATIVE_MAKE_CALL_LIMIT).contains(&maximum_calls) {
            return Err(invalid("unsupported native observation call bound"));
        }
        self.claim_observation_slot_bounded(maximum_calls)
    }

    /// Permanently claim one child-call namespace under the expanded bound.
    /// Slot numbers describe data-collection namespaces only. Existing valid
    /// claims are skipped and failed claims remain consumed, as in the legacy
    /// API. A claim proves neither producer authority nor completion.
    ///
    /// `maximum_calls` must be between 1 and 4096 inclusive. Existing claim
    /// files are skipped only when a held, no-follow descriptor proves an
    /// owned, exact-0600, empty, regular, single-link file. A fresh claim is
    /// created exclusively before its exact-0700 child directory. Existing
    /// child directories are never adopted. This does not defend against a
    /// hostile same-uid process on the host. If the inherited umask removes
    /// directory-owner read/search permissions, binding the fresh child may
    /// refuse; the claim stays consumed. This API never changes the
    /// process-global umask or repairs an unbound path.
    ///
    /// # Errors
    /// Refuses unsupported bounds, unsafe or replaced entries, exhausted slots,
    /// filesystem failures, or any parent/child identity or permission change.
    pub fn claim_observation_slot_bounded(&self, maximum_calls: usize) -> Result<(usize, Self)> {
        use rustix::fs::{self as rfs, FileType, Mode, OFlags};
        use std::ffi::OsStr;

        if !(1..=MAX_NATIVE_MAKE_CALLS).contains(&maximum_calls) {
            return Err(invalid("unsupported native observation call bound"));
        }
        self.revalidate()?;

        for slot in 0..maximum_calls {
            let child_name = format!("call-{slot:04}");
            let claim_name = format!("{child_name}.claim");
            let child_os = OsStr::new(&child_name);
            let claim_os = OsStr::new(&claim_name);
            let parent = self
                .chain
                .last()
                .ok_or_else(|| invalid("missing capture root descriptor"))?;

            self.revalidate()?;
            let claim = match rfs::openat(
                parent,
                claim_os,
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::from_raw_mode(0o600),
            ) {
                Ok(claim) => claim,
                Err(rustix::io::Errno::EXIST) => {
                    verify_existing_observation_claim(parent, claim_os)?;
                    self.revalidate()?;
                    continue;
                }
                Err(_) => return Err(invalid("cannot claim native observation slot")),
            };

            // A failed claim is deliberately retained. Exact fchmod avoids
            // treating umask-derived permissions as the private-file contract.
            rfs::fchmod(&claim, Mode::from_raw_mode(0o600))
                .map_err(|_| invalid("cannot set private native observation claim mode"))?;
            let claim_stat = rfs::fstat(&claim)
                .map_err(|_| invalid("cannot inspect native observation claim"))?;
            validate_observation_claim(&claim_stat)?;
            verify_named_identity(parent, claim_os, &claim_stat)?;
            rfs::fsync(&claim).map_err(|_| invalid("cannot sync native observation claim"))?;
            let synced_claim_stat = rfs::fstat(&claim)
                .map_err(|_| invalid("cannot recheck native observation claim"))?;
            validate_observation_claim(&synced_claim_stat)?;
            if !same_stat_identity(&claim_stat, &synced_claim_stat) {
                return Err(invalid("native observation claim changed"));
            }
            verify_named_identity(parent, claim_os, &synced_claim_stat)?;
            self.revalidate()?;
            rfs::fsync(parent).map_err(|_| invalid("cannot sync native observation parent"))?;
            self.revalidate()?;

            // mkdirat is exclusive: an unclaimed existing child is an error,
            // never something to adopt or overwrite. The claim remains spent.
            rfs::mkdirat(parent, child_os, Mode::from_raw_mode(0o700))
                .map_err(|_| invalid("cannot create fresh native observation directory"))?;
            rfs::fsync(parent).map_err(|_| invalid("cannot sync native observation parent"))?;

            let child = rfs::openat(
                parent,
                child_os,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|_| invalid("cannot bind native observation directory"))?;
            rfs::fchmod(&child, Mode::from_raw_mode(0o700))
                .map_err(|_| invalid("cannot set private native observation directory mode"))?;
            let initial = rfs::fstat(&child)
                .map_err(|_| invalid("cannot inspect native observation directory"))?;
            if !FileType::from_raw_mode(initial.st_mode).is_dir()
                || initial.st_mode & 0o7777 != 0o700
                || initial.st_uid != rustix::process::geteuid().as_raw()
            {
                return Err(invalid("native observation directory is not private"));
            }
            verify_named_identity(parent, child_os, &initial)?;
            rfs::fsync(&child).map_err(|_| invalid("cannot sync native observation directory"))?;
            self.revalidate()?;
            rfs::fsync(parent).map_err(|_| invalid("cannot sync native observation parent"))?;

            let mut chain = Vec::with_capacity(self.chain.len() + 1);
            for descriptor in &self.chain {
                chain.push(
                    descriptor
                        .try_clone()
                        .map_err(|_| invalid("cannot retain native observation parent"))?,
                );
            }
            chain.push(child);
            let directory = Self {
                path: self.path.join(child_name),
                chain,
                initial,
            };
            self.revalidate()?;
            directory.revalidate()?;
            return Ok((slot, directory));
        }

        Err(invalid("native observation call limit exhausted"))
    }

    /// Read exact externally selected bytes through the held private directory.
    ///
    /// This is data selection, not authority derived from a manifest's claims.
    ///
    /// # Errors
    /// Refuses unsafe names, linked/nonprivate files, changed identities,
    /// oversized contents, digest mismatch or any read failure.
    pub fn read_selected(
        &self,
        name: &std::ffi::OsStr,
        expected: &crate::Sha256Digest,
        limit: usize,
    ) -> Result<Vec<u8>> {
        use rustix::fs::{self as rfs, FileType, Mode, OFlags};
        use std::io::Read as _;
        validate_capture_filename(name)?;
        if limit == 0 || limit > MAX_PRIVATE_CAPTURE_BYTES {
            return Err(invalid("unsupported private native capture read bound"));
        }
        self.revalidate()?;
        let parent = self
            .chain
            .last()
            .ok_or_else(|| invalid("missing capture root descriptor"))?;
        // NONBLOCK prevents a hostile FIFO from blocking before its kind check.
        let file = rfs::openat(
            parent,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .map_err(|_| invalid("cannot open selected private native capture"))?;
        let before = rfs::fstat(&file)
            .map_err(|_| invalid("cannot inspect selected private native capture"))?;
        if !FileType::from_raw_mode(before.st_mode).is_file()
            || before.st_mode & 0o7777 != 0o600
            || before.st_uid != rustix::process::geteuid().as_raw()
            || before.st_nlink != 1
            || usize::try_from(before.st_size).map_or(true, |size| size > limit)
        {
            return Err(invalid(
                "selected native capture is not a bounded private regular file",
            ));
        }
        let mut file = std::fs::File::from(file);
        let mut bytes = Vec::new();
        file.by_ref()
            .take((limit + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| invalid("cannot read selected private native capture"))?;
        let after = rfs::fstat(&file)
            .map_err(|_| invalid("cannot recheck selected private native capture"))?;
        self.revalidate()?;
        if before.st_dev != after.st_dev
            || before.st_ino != after.st_ino
            || before.st_mode != after.st_mode
            || before.st_uid != after.st_uid
            || before.st_nlink != after.st_nlink
            || before.st_size != after.st_size
            || bytes.len() > limit
            || crate::sha256_bytes(&bytes) != *expected
        {
            return Err(invalid(
                "selected private native capture differs from its binding",
            ));
        }
        Ok(bytes)
    }

    /// Bind an existing private directory, never create or adopt an alias.
    ///
    /// # Errors
    /// Refuses linked/replaced components, noncanonical paths, ownership or mode.
    pub fn open(path: &Path) -> Result<Self> {
        use rustix::fs::{self as rfs, FileType};
        normalized_absolute(path)?;
        let chain = open_capture_parent(path)?;
        let descriptor = chain
            .last()
            .ok_or_else(|| invalid("missing capture root descriptor"))?;
        let initial =
            rfs::fstat(descriptor).map_err(|_| invalid("cannot inspect native capture parent"))?;
        if !FileType::from_raw_mode(initial.st_mode).is_dir()
            || initial.st_mode & 0o7777 != 0o700
            || initial.st_uid != rustix::process::geteuid().as_raw()
        {
            return Err(invalid(
                "native capture requires an owned private canonical directory",
            ));
        }
        Ok(Self {
            path: path.into(),
            chain,
            initial,
        })
    }

    /// Recheck every held directory identity and its private mode/ownership.
    ///
    /// # Errors
    /// Refuses replaced paths, inaccessible descriptors or changed permissions.
    pub fn revalidate(&self) -> Result<()> {
        recheck_capture_parent(&self.path, &self.chain)?;
        let descriptor = self
            .chain
            .last()
            .ok_or_else(|| invalid("missing capture root descriptor"))?;
        let after = rustix::fs::fstat(descriptor)
            .map_err(|_| invalid("cannot recheck held capture parent"))?;
        if self.initial.st_dev != after.st_dev
            || self.initial.st_ino != after.st_ino
            || self.initial.st_uid != after.st_uid
            || self.initial.st_mode != after.st_mode
        {
            return Err(invalid("native capture parent changed"));
        }
        Ok(())
    }

    /// Retain bounded private bytes under one literal filename without overwrite.
    ///
    /// Failed output remains retained, never a completed observation.
    ///
    /// # Errors
    /// Refuses unsafe filenames, payloads above4 MiB, existing or linked output,
    /// replaced directories, changed file kind/mode/ownership, or I/O failure.
    pub fn write_new(&self, name: &std::ffi::OsStr, bytes: &[u8]) -> Result<()> {
        use rustix::fs::{self as rfs, FileType, Mode, OFlags};
        use std::io::Write as _;
        validate_capture_filename(name)?;
        if bytes.len() > MAX_PRIVATE_CAPTURE_BYTES {
            return Err(invalid("unsupported private native capture output"));
        }
        self.revalidate()?;
        let directory = self
            .chain
            .last()
            .ok_or_else(|| invalid("missing capture root descriptor"))?;
        let file = rfs::openat(
            directory,
            name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )
        .map_err(|_| invalid("cannot create exclusive native capture"))?;
        rfs::fchmod(&file, Mode::from_raw_mode(0o600))
            .map_err(|_| invalid("cannot set private native capture mode"))?;
        let file_stat =
            rfs::fstat(&file).map_err(|_| invalid("cannot inspect native capture file"))?;
        if !FileType::from_raw_mode(file_stat.st_mode).is_file()
            || file_stat.st_mode & 0o7777 != 0o600
            || file_stat.st_uid != rustix::process::geteuid().as_raw()
            || file_stat.st_nlink != 1
        {
            return Err(invalid("native capture file is not private and regular"));
        }
        let mut file = std::fs::File::from(file);
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|_| invalid("cannot finish native capture"))?;
        rfs::fsync(directory).map_err(|_| invalid("cannot sync native capture parent"))?;
        self.revalidate()
    }
}

#[cfg(unix)]
fn validate_observation_claim(stat: &rustix::fs::Stat) -> Result<()> {
    use rustix::fs::FileType;

    if !FileType::from_raw_mode(stat.st_mode).is_file()
        || stat.st_mode & 0o7777 != 0o600
        || stat.st_uid != rustix::process::geteuid().as_raw()
        || stat.st_nlink != 1
        || stat.st_size != 0
    {
        return Err(invalid("native observation claim is not private and empty"));
    }
    Ok(())
}

#[cfg(unix)]
fn verify_existing_observation_claim(
    parent: &std::os::fd::OwnedFd,
    name: &std::ffi::OsStr,
) -> Result<()> {
    use rustix::fs::{self as rfs, Mode, OFlags};

    let held = rfs::openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map_err(|_| invalid("cannot inspect existing native observation claim"))?;
    let before = rfs::fstat(&held)
        .map_err(|_| invalid("cannot inspect existing native observation claim"))?;
    validate_observation_claim(&before)?;
    verify_named_identity(parent, name, &before)?;
    let after = rfs::fstat(&held)
        .map_err(|_| invalid("cannot recheck existing native observation claim"))?;
    validate_observation_claim(&after)?;
    verify_named_identity(parent, name, &after)?;
    if !same_stat_identity(&before, &after) || before.st_size != after.st_size {
        return Err(invalid("existing native observation claim changed"));
    }
    Ok(())
}

#[cfg(unix)]
fn verify_named_identity(
    parent: &std::os::fd::OwnedFd,
    name: &std::ffi::OsStr,
    held: &rustix::fs::Stat,
) -> Result<()> {
    use rustix::fs::{self as rfs, AtFlags};

    let named = rfs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW)
        .map_err(|_| invalid("cannot recheck native observation entry"))?;
    if !same_stat_identity(held, &named) || held.st_uid != named.st_uid {
        return Err(invalid("native observation entry identity changed"));
    }
    Ok(())
}

#[cfg(unix)]
const fn same_stat_identity(left: &rustix::fs::Stat, right: &rustix::fs::Stat) -> bool {
    left.st_dev == right.st_dev && left.st_ino == right.st_ino && left.st_mode == right.st_mode
}

#[cfg(unix)]
fn validate_capture_filename(name: &std::ffi::OsStr) -> Result<()> {
    let mut components = Path::new(name).components();
    if !matches!(components.next(), Some(Component::Normal(part)) if part == name)
        || components.next().is_some()
        || name.as_encoded_bytes().len() > MAX_CWD_BYTES
    {
        return Err(invalid("unsupported private native capture filename"));
    }
    Ok(())
}

fn invalid(message: &'static str) -> ArosError {
    ArosError::Configuration {
        file: "Native Make invocation".into(),
        message: message.into(),
    }
}

#[cfg(test)]
#[path = "make_native_invocation_tests.rs"]
mod tests;

#[cfg(all(test, unix))]
#[path = "make_native_capture_collection_tests.rs"]
mod collection_tests;
