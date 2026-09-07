//! Private, verified Cargo vendor preparation for the native producer.
//!
//! The source cache is mutable input, not an execution root.  This module
//! copies the selected vendor tree through no-follow descriptors into a fresh
//! private directory, validates every Cargo checksum record, and writes the
//! only Cargo configuration supplied to later collector phases.  It does not
//! invoke Cargo, resolve a registry, compile code, or change the cache.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::unix::fs::MetadataExt as _;
use std::path::{Component, Path, PathBuf};

use aros_common::sha256_file;
use rustix::fs::{self as rfs, AtFlags, FileType, Mode, OFlags, Stat};
use serde::Deserialize;

use crate::filesystem::{open_directory, DIRECTORY};
use crate::ContractError;

const VENDOR_DIRECTORY: &str = "cargo-vendor";
const VENDOR_TEMPLATE: &str = "cargo-vendor-config.toml";
const VENDOR_PLACEHOLDER: &str = "__CARGO_VENDOR_DIRECTORY__";
const PRIVATE_VENDOR_DIRECTORY: &str = "vendor";
const CARGO_HOME_DIRECTORY: &str = "cargo-home";
const CARGO_CONFIG: &str = "config.toml";
const CHECKSUM_FILE: &str = ".cargo-checksum.json";
const MAX_ENTRIES: usize = 200_000;
const MAX_TOTAL_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_TEMPLATE_BYTES: u64 = 64 * 1024;
const MAX_CHECKSUM_BYTES: u64 = 8 * 1024 * 1024;
const MAX_LOCK_BYTES: u64 = 8 * 1024 * 1024;
const MAX_LOCK_PACKAGES: usize = 100_000;
const MAX_MANIFEST_BYTES: u64 = 8 * 1024 * 1024;

/// A private Cargo vendor environment ready for a later offline collector.
#[derive(Debug, Clone)]
pub struct CargoVendorEnvironment {
    root: PathBuf,
    vendor: PathBuf,
    cargo_home: PathBuf,
    config: PathBuf,
    packages: BTreeMap<PackageIdentity, Option<String>>,
}

impl CargoVendorEnvironment {
    /// Copy and validate the selected cache-owned Cargo vendor tree.
    ///
    /// `cache_root`, `cargo_lock` and `destination` must be absolute paths.
    /// The cache must already hold `cargo-vendor/` and
    /// `cargo-vendor-config.toml`; the latter may contain exactly one
    /// `__CARGO_VENDOR_DIRECTORY__` placeholder. `cargo_lock` must be the
    /// selected tools snapshot's direct `Cargo.lock`: every external package
    /// in it must be represented once by the copied vendor tree, with the
    /// exact registry checksum when Cargo records one. The destination must
    /// not exist and is created as a private owner-only tree.
    ///
    /// # Errors
    ///
    /// Returns AX0401 when the vendor cache/template or selected lock is
    /// absent, changes while copied, has an unsafe object, disagrees with a
    /// Cargo checksum/identity, or cannot be made private. It never invokes
    /// Cargo or a compiler.
    pub fn prepare(
        cache_root: &Path,
        cargo_lock: &Path,
        destination: &Path,
    ) -> Result<Self, ContractError> {
        let cache = open_existing_directory(cache_root, "selected source cache")?;
        let cache_before = rfs::fstat(&cache).map_err(|_| {
            ContractError::environment("cannot inspect selected source cache directory")
        })?;
        let template = read_direct_regular(&cache, VENDOR_TEMPLATE, MAX_TEMPLATE_BYTES)?;
        let template = std::str::from_utf8(&template)
            .map_err(|_| ContractError::environment("Cargo vendor configuration is not UTF-8"))?;

        let root = create_private_directory(destination)?;
        let vendor = root.join(PRIVATE_VENDOR_DIRECTORY);
        let cargo_home = root.join(CARGO_HOME_DIRECTORY);
        let root_file = open_directory(&root).map_err(|_| {
            ContractError::environment("cannot reopen private Cargo vendor environment")
        })?;
        mkdir_private(&root_file, PRIVATE_VENDOR_DIRECTORY)?;
        mkdir_private(&root_file, CARGO_HOME_DIRECTORY)?;

        let source_vendor = File::from(
            rfs::openat(&cache, VENDOR_DIRECTORY, DIRECTORY, Mode::empty()).map_err(|_| {
                ContractError::environment("verified Cargo vendor tree is absent or unsafe")
            })?,
        );
        let destination_vendor = File::from(
            rfs::openat(
                &root_file,
                PRIVATE_VENDOR_DIRECTORY,
                DIRECTORY,
                Mode::empty(),
            )
            .map_err(|_| {
                ContractError::environment("cannot open private Cargo vendor destination")
            })?,
        );
        let mut budget = CopyBudget::default();
        copy_directory(&source_vendor, &destination_vendor, &mut budget)?;

        if !same_stat(
            &cache_before,
            &rfs::fstat(&cache).map_err(|_| {
                ContractError::environment(
                    "selected source cache changed during vendor preparation",
                )
            })?,
        ) {
            return Err(ContractError::environment(
                "selected source cache changed during vendor preparation",
            ));
        }
        let packages = validate_vendor_tree(&vendor)?;
        validate_cargo_lock(cargo_lock, &packages)?;
        let config = cargo_home.join(CARGO_CONFIG);
        let configured = render_vendor_configuration(template, &vendor)?;
        write_private_file(&root_file, CARGO_HOME_DIRECTORY, CARGO_CONFIG, &configured)?;
        ensure_private_directory(&root)?;
        ensure_private_directory(&vendor)?;
        ensure_private_directory(&cargo_home)?;
        Ok(Self {
            root,
            vendor,
            cargo_home,
            config,
            packages,
        })
    }

    /// Fresh private environment root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Fresh private vendor tree.
    #[must_use]
    pub fn vendor(&self) -> &Path {
        &self.vendor
    }

    /// Cargo home containing only the generated offline configuration.
    #[must_use]
    pub fn cargo_home(&self) -> &Path {
        &self.cargo_home
    }

    /// Generated Cargo configuration path.
    #[must_use]
    pub fn config(&self) -> &Path {
        &self.config
    }

    /// Count of checksum-validated vendor packages.
    #[must_use]
    pub fn package_count(&self) -> usize {
        self.packages.len()
    }

    /// Apply the minimal offline vendor environment to a later Cargo command.
    ///
    /// The command still must pass Cargo's own `--locked --offline` flags.
    /// This method does not add a registry, credential, target-dir or compiler
    /// selection; those remain explicit lifecycle inputs.
    pub fn apply_to(&self, command: &mut std::process::Command) {
        command
            .env("CARGO_HOME", &self.cargo_home)
            .env("CARGO_NET_OFFLINE", "true")
            .env("CARGO_INCREMENTAL", "0");
    }
}

#[derive(Default)]
struct CopyBudget {
    entries: usize,
    bytes: u64,
}

impl CopyBudget {
    fn account_entry(&mut self) -> Result<(), ContractError> {
        self.entries = self
            .entries
            .checked_add(1)
            .ok_or_else(|| ContractError::environment("Cargo vendor entry accounting overflow"))?;
        if self.entries > MAX_ENTRIES {
            return Err(ContractError::environment(
                "Cargo vendor tree exceeds the 200000-entry safety limit",
            ));
        }
        Ok(())
    }

    fn account_file(&mut self, size: u64) -> Result<(), ContractError> {
        if size > MAX_FILE_BYTES {
            return Err(ContractError::environment(
                "Cargo vendor tree contains an oversized regular file",
            ));
        }
        self.bytes = self
            .bytes
            .checked_add(size)
            .ok_or_else(|| ContractError::environment("Cargo vendor byte accounting overflow"))?;
        if self.bytes > MAX_TOTAL_BYTES {
            return Err(ContractError::environment(
                "Cargo vendor tree exceeds the 8 GiB safety limit",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChecksumRecord {
    files: BTreeMap<String, String>,
    package: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PackageIdentity {
    name: String,
    version: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CargoLock {
    version: u32,
    #[serde(default)]
    package: Vec<CargoLockPackage>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CargoLockPackage {
    name: String,
    version: String,
    source: Option<String>,
    checksum: Option<String>,
    #[serde(default)]
    dependencies: Option<toml::Value>,
    replace: Option<String>,
}

fn open_existing_directory(path: &Path, label: &str) -> Result<File, ContractError> {
    if !path.is_absolute() {
        return Err(ContractError::environment(
            "selected Cargo vendor cache path must be absolute",
        ));
    }
    open_directory(path)
        .map_err(|_| ContractError::environment(format!("{label} is not a real directory")))
}

fn create_private_directory(destination: &Path) -> Result<PathBuf, ContractError> {
    let parent = destination.parent().ok_or_else(|| {
        ContractError::environment("private Cargo vendor destination has no parent directory")
    })?;
    let leaf = safe_leaf(destination)?;
    let parent = normalize_system_parent(parent)?;
    let parent_file = open_existing_directory(&parent, "private Cargo vendor destination parent")?;
    match rfs::statat(&parent_file, leaf.as_str(), AtFlags::SYMLINK_NOFOLLOW) {
        Ok(_) => {
            return Err(ContractError::environment(
                "private Cargo vendor destination already exists and will not be reused",
            ))
        }
        Err(rustix::io::Errno::NOENT) => {}
        Err(_) => {
            return Err(ContractError::environment(
                "cannot inspect private Cargo vendor destination",
            ))
        }
    }
    rfs::mkdirat(
        &parent_file,
        leaf.as_str(),
        Mode::RUSR | Mode::WUSR | Mode::XUSR,
    )
    .map_err(|_| ContractError::environment("cannot create private Cargo vendor destination"))?;
    let root = parent.join(leaf);
    ensure_private_directory(&root)?;
    Ok(root)
}

fn normalize_system_parent(parent: &Path) -> Result<PathBuf, ContractError> {
    if !parent.is_absolute() {
        return Err(ContractError::environment(
            "private Cargo vendor destination parent must be absolute",
        ));
    }
    for system_root in [Path::new("/var"), Path::new("/tmp"), Path::new("/etc")] {
        if let Ok(relative) = parent.strip_prefix(system_root) {
            return system_root
                .canonicalize()
                .map(|root| root.join(relative))
                .map_err(|_| {
                    ContractError::environment(
                        "cannot resolve system private Cargo vendor destination parent",
                    )
                });
        }
    }
    Ok(parent.to_path_buf())
}

fn safe_leaf(path: &Path) -> Result<String, ContractError> {
    let Some(leaf) = path.file_name().and_then(|value| value.to_str()) else {
        return Err(ContractError::environment(
            "private Cargo vendor destination must have a portable UTF-8 leaf",
        ));
    };
    if leaf.is_empty()
        || leaf == "."
        || leaf == ".."
        || !leaf
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
    {
        return Err(ContractError::environment(
            "private Cargo vendor destination must have a portable leaf",
        ));
    }
    Ok(leaf.to_owned())
}

fn mkdir_private(parent: &File, name: &str) -> Result<(), ContractError> {
    rfs::mkdirat(parent, name, Mode::RUSR | Mode::WUSR | Mode::XUSR)
        .map_err(|_| ContractError::environment("cannot create private Cargo vendor directory"))
}

fn read_direct_regular(parent: &File, name: &str, limit: u64) -> Result<Vec<u8>, ContractError> {
    let before = rfs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW).map_err(|_| {
        ContractError::environment("required Cargo vendor cache file is unavailable")
    })?;
    if !FileType::from_raw_mode(before.st_mode).is_file()
        || before.st_size < 0
        || u64::try_from(before.st_size)
            .ok()
            .is_none_or(|size| size > limit)
    {
        return Err(ContractError::environment(
            "required Cargo vendor cache file is not a bounded regular file",
        ));
    }
    let mut input = File::from(
        rfs::openat(
            parent,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| {
            ContractError::environment("cannot safely open required Cargo vendor cache file")
        })?,
    );
    if !same_stat(
        &before,
        &rfs::fstat(&input).map_err(|_| {
            ContractError::environment("required Cargo vendor cache file changed before reading")
        })?,
    ) {
        return Err(ContractError::environment(
            "required Cargo vendor cache file changed before reading",
        ));
    }
    let size = u64::try_from(before.st_size).map_err(|_| {
        ContractError::environment("required Cargo vendor cache file has an invalid size")
    })?;
    let mut bytes = Vec::with_capacity(usize::try_from(size).unwrap_or(0));
    Read::by_ref(&mut input)
        .take(size.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| ContractError::environment("cannot read required Cargo vendor cache file"))?;
    if u64::try_from(bytes.len()).ok() != Some(size)
        || !same_stat(
            &before,
            &rfs::fstat(&input).map_err(|_| {
                ContractError::environment("required Cargo vendor cache file changed while reading")
            })?,
        )
    {
        return Err(ContractError::environment(
            "required Cargo vendor cache file changed while reading",
        ));
    }
    Ok(bytes)
}

fn copy_directory(
    source: &File,
    destination: &File,
    budget: &mut CopyBudget,
) -> Result<(), ContractError> {
    let before = rfs::fstat(source)
        .map_err(|_| ContractError::environment("cannot inspect Cargo vendor source directory"))?;
    let names = directory_names(source)?;
    for name in &names {
        budget.account_entry()?;
        let entry_before = rfs::statat(source, name.as_str(), AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|_| ContractError::environment("Cargo vendor source entry disappeared"))?;
        let kind = FileType::from_raw_mode(entry_before.st_mode);
        if kind.is_dir() {
            rfs::mkdirat(
                destination,
                name.as_str(),
                Mode::RUSR | Mode::WUSR | Mode::XUSR,
            )
            .map_err(|_| {
                ContractError::environment("cannot create private Cargo vendor directory")
            })?;
            let child_source = File::from(
                rfs::openat(source, name.as_str(), DIRECTORY, Mode::empty()).map_err(|_| {
                    ContractError::environment("cannot safely open Cargo vendor source directory")
                })?,
            );
            let child_destination = File::from(
                rfs::openat(destination, name.as_str(), DIRECTORY, Mode::empty()).map_err(
                    |_| ContractError::environment("cannot open private Cargo vendor directory"),
                )?,
            );
            if !same_stat(
                &entry_before,
                &rfs::fstat(&child_source).map_err(|_| {
                    ContractError::environment(
                        "Cargo vendor source directory changed before copying",
                    )
                })?,
            ) {
                return Err(ContractError::environment(
                    "Cargo vendor source directory changed before copying",
                ));
            }
            copy_directory(&child_source, &child_destination, budget)?;
        } else if kind.is_file() {
            copy_regular_file(source, destination, name, &entry_before, budget)?;
        } else {
            return Err(ContractError::environment(
                "Cargo vendor tree contains a symlink or unsupported filesystem object",
            ));
        }
        if !same_stat(
            &entry_before,
            &rfs::statat(source, name.as_str(), AtFlags::SYMLINK_NOFOLLOW).map_err(|_| {
                ContractError::environment("Cargo vendor source entry changed while copying")
            })?,
        ) {
            return Err(ContractError::environment(
                "Cargo vendor source entry changed while copying",
            ));
        }
    }
    if !same_stat(
        &before,
        &rfs::fstat(source).map_err(|_| {
            ContractError::environment("Cargo vendor source directory changed while copying")
        })?,
    ) || directory_names(source)? != names
    {
        return Err(ContractError::environment(
            "Cargo vendor source directory changed while copying",
        ));
    }
    Ok(())
}

fn copy_regular_file(
    source_parent: &File,
    destination_parent: &File,
    name: &str,
    expected: &Stat,
    budget: &mut CopyBudget,
) -> Result<(), ContractError> {
    let size = u64::try_from(expected.st_size)
        .map_err(|_| ContractError::environment("Cargo vendor file has an invalid size"))?;
    budget.account_file(size)?;
    let mut source = File::from(
        rfs::openat(
            source_parent,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| ContractError::environment("cannot safely open Cargo vendor source file"))?,
    );
    if !same_stat(
        expected,
        &rfs::fstat(&source).map_err(|_| {
            ContractError::environment("Cargo vendor source file changed before copying")
        })?,
    ) {
        return Err(ContractError::environment(
            "Cargo vendor source file changed before copying",
        ));
    }
    let mut destination = File::from(
        rfs::openat(
            destination_parent,
            name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(|_| ContractError::environment("cannot create private Cargo vendor file"))?,
    );
    let copied = std::io::copy(
        &mut Read::by_ref(&mut source).take(size.saturating_add(1)),
        &mut destination,
    )
    .map_err(|_| ContractError::environment("cannot copy Cargo vendor source bytes"))?;
    if copied != size {
        return Err(ContractError::environment(
            "Cargo vendor source file ended before its declared size",
        ));
    }
    destination
        .sync_all()
        .map_err(|_| ContractError::environment("cannot sync private Cargo vendor file"))?;
    if !same_stat(
        expected,
        &rfs::fstat(&source).map_err(|_| {
            ContractError::environment("Cargo vendor source file changed while copying")
        })?,
    ) {
        return Err(ContractError::environment(
            "Cargo vendor source file changed while copying",
        ));
    }
    Ok(())
}

fn directory_names(directory: &File) -> Result<Vec<String>, ContractError> {
    let mut names = BTreeSet::new();
    for entry in rfs::Dir::read_from(directory)
        .map_err(|_| ContractError::environment("cannot enumerate Cargo vendor directory"))?
    {
        let entry = entry
            .map_err(|_| ContractError::environment("cannot enumerate Cargo vendor directory"))?;
        let bytes = entry.file_name().to_bytes();
        if matches!(bytes, b"." | b"..") {
            continue;
        }
        let name = std::str::from_utf8(bytes).map_err(|_| {
            ContractError::environment("Cargo vendor tree contains a non-UTF-8 path")
        })?;
        if name.is_empty() || name.len() > 255 || !names.insert(name.to_owned()) {
            return Err(ContractError::environment(
                "Cargo vendor directory has an invalid or unstable entry name",
            ));
        }
    }
    Ok(names.into_iter().collect())
}

fn validate_vendor_tree(
    vendor: &Path,
) -> Result<BTreeMap<PackageIdentity, Option<String>>, ContractError> {
    let mut packages = BTreeMap::new();
    let entries = sorted_entries(vendor)?;
    if entries.is_empty() {
        return Err(ContractError::environment(
            "private Cargo vendor tree contains no packages",
        ));
    }
    for entry in entries {
        let metadata = fs::symlink_metadata(entry.path()).map_err(|_| {
            ContractError::environment("cannot inspect private Cargo vendor package")
        })?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(ContractError::environment(
                "private Cargo vendor root contains a non-directory package entry",
            ));
        }
        let (identity, checksum) = validate_vendor_package(&entry.path())?;
        if packages.insert(identity, checksum).is_some() {
            return Err(ContractError::environment(
                "private Cargo vendor tree has a duplicate package identity",
            ));
        }
    }
    Ok(packages)
}

fn validate_vendor_package(
    root: &Path,
) -> Result<(PackageIdentity, Option<String>), ContractError> {
    let manifest = read_private_regular(&root.join("Cargo.toml"), MAX_MANIFEST_BYTES)?;
    let manifest: toml::Value = toml::from_str(
        std::str::from_utf8(&manifest)
            .map_err(|_| ContractError::environment("vendored Cargo.toml is not UTF-8"))?,
    )
    .map_err(|_| ContractError::environment("vendored Cargo.toml is malformed"))?;
    let package = manifest
        .get("package")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| ContractError::environment("vendored Cargo.toml has no package table"))?;
    let identity = package_identity(
        package.get("name").and_then(toml::Value::as_str),
        package.get("version").and_then(toml::Value::as_str),
        "vendored Cargo.toml",
    )?;
    let checksum = read_private_regular(&root.join(CHECKSUM_FILE), MAX_CHECKSUM_BYTES)?;
    let checksum: ChecksumRecord = serde_json::from_slice(&checksum)
        .map_err(|_| ContractError::environment("vendored Cargo checksum record is malformed"))?;
    if checksum.files.is_empty()
        || checksum
            .package
            .as_deref()
            .is_some_and(|digest| !sha256_hex(digest))
        || checksum
            .files
            .iter()
            .any(|(path, digest)| !safe_relative_file(path) || !sha256_hex(digest))
    {
        return Err(ContractError::environment(
            "vendored Cargo checksum record has an unsafe path or digest",
        ));
    }
    let expected = checksum.files.keys().cloned().collect::<BTreeSet<_>>();
    let mut actual = BTreeSet::new();
    collect_vendor_files(root, root, &mut actual)?;
    if expected != actual {
        return Err(ContractError::environment(
            "vendored Cargo package files differ from its checksum record",
        ));
    }
    for (relative, expected_digest) in checksum.files {
        let measured = sha256_file(&root.join(relative))
            .map_err(|_| ContractError::environment("cannot hash private vendored Cargo file"))?;
        if measured.digest.as_str() != expected_digest {
            return Err(ContractError::environment(
                "vendored Cargo package file differs from its declared checksum",
            ));
        }
    }
    Ok((identity, checksum.package))
}

fn validate_cargo_lock(
    path: &Path,
    vendor_packages: &BTreeMap<PackageIdentity, Option<String>>,
) -> Result<(), ContractError> {
    let bytes = read_cargo_lock(path)?;
    let lock: CargoLock = toml::from_str(
        std::str::from_utf8(&bytes)
            .map_err(|_| ContractError::environment("selected Cargo.lock is not UTF-8"))?,
    )
    .map_err(|_| ContractError::environment("selected Cargo.lock is malformed or unsupported"))?;
    if lock.version != 4 || lock.package.len() > MAX_LOCK_PACKAGES {
        return Err(ContractError::environment(
            "selected Cargo.lock has an unsupported format or package count",
        ));
    }
    let mut expected = BTreeMap::new();
    for package in lock.package {
        if package.replace.is_some() {
            return Err(ContractError::environment(
                "selected Cargo.lock uses an unsupported package replacement",
            ));
        }
        let _ = package.dependencies;
        let Some(source) = package.source else {
            if package.checksum.is_some() {
                return Err(ContractError::environment(
                    "local Cargo.lock package has an unsupported source or checksum field",
                ));
            }
            continue;
        };
        let identity = package_identity(
            Some(&package.name),
            Some(&package.version),
            "selected Cargo.lock",
        )?;
        let package_checksum = lock_package_checksum(&source, package.checksum.as_deref())?;
        if expected.insert(identity, package_checksum).is_some() {
            return Err(ContractError::environment(
                "selected Cargo.lock has multiple external sources for one package identity",
            ));
        }
    }
    if expected.is_empty() || expected != *vendor_packages {
        return Err(ContractError::environment(
            "private Cargo vendor tree does not exactly match the selected Cargo.lock closure",
        ));
    }
    Ok(())
}

fn read_cargo_lock(path: &Path) -> Result<Vec<u8>, ContractError> {
    if !path.is_absolute() {
        return Err(ContractError::environment(
            "selected Cargo.lock path must be absolute",
        ));
    }
    let parent = path
        .parent()
        .ok_or_else(|| ContractError::environment("selected Cargo.lock has no parent directory"))?;
    let leaf = safe_leaf(path)?;
    let parent = normalize_system_parent(parent)?;
    let parent = open_existing_directory(&parent, "selected Cargo.lock parent")?;
    read_direct_regular(&parent, &leaf, MAX_LOCK_BYTES)
}

fn package_identity(
    name: Option<&str>,
    version: Option<&str>,
    label: &str,
) -> Result<PackageIdentity, ContractError> {
    let Some(name) = name else {
        return Err(ContractError::environment(format!(
            "{label} has no package name"
        )));
    };
    let Some(version) = version else {
        return Err(ContractError::environment(format!(
            "{label} has no package version"
        )));
    };
    if !crate_name(name) || semver::Version::parse(version).is_err() {
        return Err(ContractError::environment(format!(
            "{label} has an unsafe package identity"
        )));
    }
    Ok(PackageIdentity {
        name: name.to_owned(),
        version: version.to_owned(),
    })
}

fn crate_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn lock_package_checksum(
    source: &str,
    checksum: Option<&str>,
) -> Result<Option<String>, ContractError> {
    if let Some(url) = source
        .strip_prefix("registry+")
        .or_else(|| source.strip_prefix("sparse+"))
    {
        let url = url::Url::parse(url).map_err(|_| {
            ContractError::environment("selected Cargo.lock has an invalid registry source URL")
        })?;
        if url.scheme() != "https"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(ContractError::environment(
                "selected Cargo.lock has an unsafe registry source URL",
            ));
        }
        let checksum = checksum.filter(|value| sha256_hex(value)).ok_or_else(|| {
            ContractError::environment("selected Cargo.lock registry package has no valid checksum")
        })?;
        return Ok(Some(checksum.to_owned()));
    }
    let Some(url) = source.strip_prefix("git+") else {
        return Err(ContractError::environment(
            "selected Cargo.lock uses an unsupported external source kind",
        ));
    };
    let url = url::Url::parse(url).map_err(|_| {
        ContractError::environment("selected Cargo.lock has an invalid git source URL")
    })?;
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return Err(ContractError::environment(
            "selected Cargo.lock has an unsafe git source URL",
        ));
    }
    if checksum.is_some() {
        return Err(ContractError::environment(
            "selected Cargo.lock git package unexpectedly has a registry checksum",
        ));
    }
    Ok(None)
}

fn collect_vendor_files(
    root: &Path,
    current: &Path,
    files: &mut BTreeSet<String>,
) -> Result<(), ContractError> {
    for entry in sorted_entries(current)? {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|_| {
            ContractError::environment("cannot inspect private vendored Cargo path")
        })?;
        if metadata.file_type().is_symlink() {
            return Err(ContractError::environment(
                "private vendored Cargo tree contains a symlink",
            ));
        }
        if metadata.is_dir() {
            collect_vendor_files(root, &path, files)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(ContractError::environment(
                "private vendored Cargo tree contains a non-regular file",
            ));
        }
        let relative = path
            .strip_prefix(root)
            .ok()
            .and_then(Path::to_str)
            .map(str::to_owned)
            .ok_or_else(|| {
                ContractError::environment("private vendored Cargo path is not UTF-8")
            })?;
        if relative == CHECKSUM_FILE {
            continue;
        }
        if !safe_relative_file(&relative) || !files.insert(relative) {
            return Err(ContractError::environment(
                "private vendored Cargo tree has an unsafe or duplicate file path",
            ));
        }
    }
    Ok(())
}

fn sorted_entries(path: &Path) -> Result<Vec<fs::DirEntry>, ContractError> {
    let mut entries = fs::read_dir(path)
        .map_err(|_| ContractError::environment("cannot enumerate private Cargo vendor tree"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| ContractError::environment("cannot enumerate private Cargo vendor tree"))?;
    entries.sort_by_key(fs::DirEntry::file_name);
    Ok(entries)
}

fn read_private_regular(path: &Path, limit: u64) -> Result<Vec<u8>, ContractError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| {
        ContractError::environment("required private vendored Cargo file is missing")
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > limit {
        return Err(ContractError::environment(
            "required private vendored Cargo file is not a bounded regular file",
        ));
    }
    fs::read(path)
        .map_err(|_| ContractError::environment("cannot read required private vendored Cargo file"))
}

fn render_vendor_configuration(template: &str, vendor: &Path) -> Result<Vec<u8>, ContractError> {
    let mut document: toml::Value = toml::from_str(template).map_err(|_| {
        ContractError::environment("Cargo vendor configuration template is malformed")
    })?;
    let sources = document
        .get_mut("source")
        .and_then(toml::Value::as_table_mut)
        .ok_or_else(|| {
            ContractError::environment("Cargo vendor configuration has no source table")
        })?;
    if sources.len() < 2 {
        return Err(ContractError::environment(
            "Cargo vendor configuration has an incomplete source mapping",
        ));
    }
    let Some(vendored) = sources
        .get_mut("vendored-sources")
        .and_then(toml::Value::as_table_mut)
    else {
        return Err(ContractError::environment(
            "Cargo vendor configuration has no vendored-sources mapping",
        ));
    };
    if vendored.len() != 1
        || vendored.get("directory").and_then(toml::Value::as_str) != Some(VENDOR_PLACEHOLDER)
    {
        return Err(ContractError::environment(
            "Cargo vendor configuration has no unique vendor directory placeholder",
        ));
    }
    vendored.insert(
        "directory".to_owned(),
        toml::Value::String(vendor.to_string_lossy().into_owned()),
    );
    for (name, entry) in sources.iter() {
        if name == "vendored-sources" {
            continue;
        }
        let table = entry.as_table().ok_or_else(|| {
            ContractError::environment("Cargo vendor configuration source entry is not a table")
        })?;
        if table.get("replace-with").and_then(toml::Value::as_str) != Some("vendored-sources")
            || table.keys().any(|key| {
                !matches!(
                    key.as_str(),
                    "replace-with" | "registry" | "git" | "branch" | "tag" | "rev"
                )
            })
        {
            return Err(ContractError::environment(
                "Cargo vendor configuration permits an unsupported source mapping",
            ));
        }
    }
    let rendered = toml::to_string(&document)
        .map_err(|_| ContractError::environment("cannot encode Cargo vendor configuration"))?;
    if rendered.contains(VENDOR_PLACEHOLDER) {
        return Err(ContractError::environment(
            "Cargo vendor configuration left an unresolved directory placeholder",
        ));
    }
    Ok(rendered.into_bytes())
}

fn write_private_file(
    root: &File,
    directory: &str,
    filename: &str,
    bytes: &[u8],
) -> Result<(), ContractError> {
    let parent = File::from(
        rfs::openat(root, directory, DIRECTORY, Mode::empty())
            .map_err(|_| ContractError::environment("cannot open private Cargo home directory"))?,
    );
    let mut output = File::from(
        rfs::openat(
            &parent,
            filename,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(|_| ContractError::environment("cannot create private Cargo configuration"))?,
    );
    output
        .write_all(bytes)
        .and_then(|()| output.sync_all())
        .map_err(|_| ContractError::environment("cannot persist private Cargo configuration"))?;
    Ok(())
}

fn ensure_private_directory(path: &Path) -> Result<(), ContractError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| ContractError::environment("cannot inspect private Cargo vendor directory"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() || metadata.mode() & 0o077 != 0 {
        return Err(ContractError::environment(
            "Cargo vendor private directory is not owner-only",
        ));
    }
    Ok(())
}

fn safe_relative_file(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 4096
        && !Path::new(value).is_absolute()
        && Path::new(value).components().all(|component| {
            matches!(component, Component::Normal(part) if !part.is_empty() && part != "." && part != "..")
        })
}

fn sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

const fn same_stat(left: &Stat, right: &Stat) -> bool {
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
