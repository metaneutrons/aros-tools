//! Frozen closure verification of the released aros-tools runtime.
//!
//! Port of `validate_lock`, `validate_manifest_data`, `receipt` and
//! `verify_extracted_frozen` from `scripts/toolchain/materialize-aros-tools-runtime.py`
//! (aros-toolchains). The caller supplies caller-authenticated lock bytes; this
//! module never reopens a lock pathname and never changes a file mode.
//!
//! It proves only that the retained directory is exactly the closure the lock
//! and the signed manifest describe at one point in time. Publication
//! authenticity (signatures, attestations) belongs to the materializer.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use rustix::fs::{fstat, openat, statat, AtFlags, Dir, FileType, Mode, OFlags, Stat};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use super::{fail, input, integrity, require, require_integrity, Result};

/// Host to Rust target (Python `HOST_TARGETS`).
pub const HOST_TARGETS: [(&str, &str); 3] = [
    ("linux-x86_64", "x86_64-unknown-linux-gnu"),
    ("linux-aarch64", "aarch64-unknown-linux-gnu"),
    ("macos-aarch64", "aarch64-apple-darwin"),
];

/// Exact runtime inventory: path, mode and whether it is a native executable.
pub const EXPECTED_FILES: [(&str, &str, bool); 10] = [
    ("LICENSE", "0644", false),
    ("README.md", "0644", false),
    ("bin/aros", "0755", true),
    ("bin/aros-ahi-runner", "0755", true),
    ("bin/aros-collect", "0755", true),
    ("bin/aros-fetch", "0755", true),
    ("bin/aros-genmodule", "0755", true),
    ("bin/aros-romtool", "0755", true),
    ("bin/aros-transpiler", "0755", true),
    ("bin/aros-verify", "0755", true),
];

pub const MANIFEST_NAME: &str = ".aros-tools-runtime-manifest.json";
pub const RECEIPT_NAME: &str = ".aros-tools-runtime-receipt-v1.json";
pub const RUNTIME_REPOSITORY: &str = "metaneutrons/aros-tools";
const MAX_METADATA: usize = 1024 * 1024;
const BLOCK: usize = 1024 * 1024;

fn expected_file(path: &str) -> Option<(&'static str, bool)> {
    EXPECTED_FILES
        .iter()
        .find(|(name, _, _)| *name == path)
        .map(|(_, mode, native)| (*mode, *native))
}

fn target_of(host: &str) -> Option<&'static str> {
    HOST_TARGETS
        .iter()
        .find(|(name, _)| *name == host)
        .map(|(_, target)| *target)
}

fn exact_keys<'a>(value: &'a Value, keys: &[&str], label: &str) -> Result<&'a Map<String, Value>> {
    let expected: BTreeSet<&str> = keys.iter().copied().collect();
    match value.as_object() {
        Some(object) if object.keys().map(String::as_str).collect::<BTreeSet<_>>() == expected => {
            Ok(object)
        }
        _ => fail(format!("{label} must contain exactly the closed field set")),
    }
}

fn hex<'a>(value: &'a Value, length: usize, label: &str) -> Result<&'a str> {
    match value.as_str() {
        Some(text)
            if text.len() == length
                && text
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)) =>
        {
            Ok(text)
        }
        _ => fail(format!(
            "{label} must be {length} lowercase hexadecimal characters"
        )),
    }
}

fn positive(value: &Value) -> Option<u64> {
    value.as_u64().filter(|number| *number > 0)
}

fn validate_asset(value: &Value, label: &str) -> Result<()> {
    let asset = exact_keys(value, &["name", "sha256", "size"], label)?;
    match asset["name"].as_str() {
        Some(name)
            if !name.is_empty()
                && !name.contains('/')
                && !name.contains('\\')
                && name != "."
                && name != ".." => {}
        _ => return fail(format!("{label}.name must be a non-empty basename")),
    }
    hex(&asset["sha256"], 64, &format!("{label}.sha256"))?;
    require(
        positive(&asset["size"]).is_some(),
        &format!("{label}.size must be a positive integer"),
    )
}

/// Validate the closed runtime lock (Python `validate_lock`).
///
/// # Errors
///
/// Fails for every deviation from the closed lock contract.
pub fn validate_lock(lock: &Value) -> Result<()> {
    let top = exact_keys(
        lock,
        &[
            "schema",
            "repository",
            "release_id",
            "version",
            "tag",
            "tag_object",
            "source_commit",
            "source_tree",
            "source_date_epoch",
            "signer_workflow",
            "certificate_identity",
            "certificate_oidc_issuer",
            "hosts",
        ],
        "runtime lock",
    )?;
    require(
        top["schema"] == "aros-tools-runtime-v1",
        "unsupported runtime lock schema",
    )?;
    let repository = top["repository"].as_str().unwrap_or_default();
    require(
        repository.matches('/').count() == 1,
        "runtime lock repository must be an OWNER/REPO name",
    )?;
    require(
        positive(&top["release_id"]).is_some(),
        "runtime lock release_id must be a positive integer",
    )?;
    let version = top["version"].as_str().unwrap_or_default();
    require(
        !version.is_empty(),
        "runtime lock version must be non-empty",
    )?;
    let tag = format!("v{version}");
    require(
        top["tag"] == tag.as_str(),
        "runtime lock tag must be the exact vVERSION tag",
    )?;
    hex(&top["tag_object"], 40, "runtime lock tag_object")?;
    hex(&top["source_commit"], 40, "runtime lock source_commit")?;
    hex(&top["source_tree"], 40, "runtime lock source_tree")?;
    require(
        positive(&top["source_date_epoch"]).is_some(),
        "runtime lock source_date_epoch must be a positive integer",
    )?;
    let workflow = format!("{repository}/.github/workflows/release.yml");
    require(
        top["signer_workflow"] == workflow.as_str(),
        "runtime lock signer_workflow must be its canonical release workflow",
    )?;
    let identity = format!("https://github.com/{workflow}@refs/tags/{tag}");
    require(
        top["certificate_identity"] == identity.as_str(),
        "runtime lock certificate_identity must bind the exact release tag",
    )?;
    require(
        top["certificate_oidc_issuer"] == "https://token.actions.githubusercontent.com",
        "runtime lock certificate_oidc_issuer must be GitHub Actions",
    )?;
    let hosts = top["hosts"].as_object();
    require(
        hosts.is_some_and(|map| {
            map.keys().map(String::as_str).collect::<BTreeSet<_>>()
                == HOST_TARGETS.iter().map(|(host, _)| *host).collect()
        }),
        "runtime lock hosts must be exactly the supported host set",
    )?;
    let hosts = top["hosts"].clone();
    let mut seen = BTreeSet::new();
    for (host, target) in HOST_TARGETS {
        let entry = exact_keys(
            &hosts[host],
            &[
                "target",
                "archive",
                "manifest",
                "archive_bundle",
                "manifest_bundle",
            ],
            host,
        )?;
        require(
            entry["target"] == target,
            "runtime lock host target differs from the supported target",
        )?;
        for field in ["archive", "manifest", "archive_bundle", "manifest_bundle"] {
            validate_asset(&entry[field], &format!("{host}.{field}"))?;
            let name = entry[field]["name"].as_str().unwrap_or_default().to_owned();
            require(seen.insert(name), "runtime lock reuses an asset name")?;
        }
        let archive = format!("aros-tools-{tag}-{target}.tar.gz");
        require(
            entry["archive"]["name"] == archive.as_str(),
            "runtime lock archive does not match version and target",
        )?;
        require(
            entry["manifest"]["name"] == format!("{archive}.manifest.json").as_str(),
            "runtime lock manifest name is not archive-bound",
        )?;
        require(
            entry["archive_bundle"]["name"] == format!("{archive}.sigstore.json").as_str(),
            "runtime lock archive bundle name is not archive-bound",
        )?;
        require(
            entry["manifest_bundle"]["name"]
                == format!("{archive}.manifest.json.sigstore.json").as_str(),
            "runtime lock manifest bundle name is not manifest-bound",
        )?;
    }
    Ok(())
}

/// Validate signed-manifest bytes against the lock (Python
/// `validate_manifest_bytes`). The lock must already be validated.
///
/// # Errors
///
/// Fails when size, digest or any field differs from the lock.
pub fn validate_manifest(lock: &Value, host: &str, bytes: &[u8]) -> Result<Value> {
    let Some(target) = target_of(host) else {
        return fail("unsupported runtime host");
    };
    require(
        bytes.len() <= MAX_METADATA,
        "released runtime manifest exceeds the metadata size limit",
    )?;
    let asset = &lock["hosts"][host]["manifest"];
    require_integrity(
        Some(bytes.len() as u64) == asset["size"].as_u64()
            && asset["sha256"] == sha256_hex(bytes).as_str(),
        "released runtime manifest checksum or size differs from runtime lock",
    )?;
    let manifest: Value = match serde_json::from_slice(bytes) {
        Ok(value) => value,
        Err(_) => return fail("cannot read JSON released runtime manifest"),
    };
    exact_keys(
        &manifest,
        &[
            "schema",
            "package",
            "version",
            "target",
            "source_commit",
            "source_date_epoch",
            "archive",
            "archive_sha256",
            "archive_size",
            "files",
        ],
        "released runtime manifest",
    )?;
    let entry = &lock["hosts"][host];
    let expected = [
        ("schema", json!(1)),
        ("package", json!("aros-tools")),
        ("version", lock["version"].clone()),
        ("target", json!(target)),
        ("source_commit", lock["source_commit"].clone()),
        ("source_date_epoch", lock["source_date_epoch"].clone()),
        ("archive", entry["archive"]["name"].clone()),
        ("archive_sha256", entry["archive"]["sha256"].clone()),
        ("archive_size", entry["archive"]["size"].clone()),
    ];
    for (field, wanted) in expected {
        require(
            manifest[field] == wanted,
            "released runtime manifest differs from runtime lock",
        )?;
    }
    let files = manifest["files"].as_array();
    require(
        files.is_some_and(|list| list.len() == EXPECTED_FILES.len()),
        "released runtime manifest must declare the exact runtime file inventory",
    )?;
    let mut observed = BTreeSet::new();
    for item in files.unwrap_or(&Vec::new()) {
        let file = exact_keys(
            item,
            &["path", "mode", "sha256", "size"],
            "released runtime file",
        )?;
        let path = file["path"].as_str().unwrap_or_default();
        let Some((mode, _)) = expected_file(path) else {
            return fail("released runtime manifest has an unknown file");
        };
        require(
            observed.insert(path.to_owned()),
            "released runtime manifest has a duplicate file",
        )?;
        require(
            file["mode"] == mode,
            "released runtime manifest mode differs from the inventory",
        )?;
        hex(&file["sha256"], 64, "released runtime file SHA-256")?;
        require(
            positive(&file["size"]).is_some(),
            "released runtime manifest size is invalid",
        )?;
    }
    require(
        observed.len() == EXPECTED_FILES.len(),
        "released runtime manifest file set differs from the expected inventory",
    )?;
    Ok(manifest)
}

/// Canonical materialization receipt for a locked host (Python `receipt`).
#[must_use]
pub fn receipt(
    lock: &Value,
    host: &str,
    manifest: &Value,
    manifest_sha256: &str,
    lock_sha256: &str,
) -> Value {
    json!({
        "schema": "aros-tools-runtime-receipt-v1",
        "runtime_lock_sha256": lock_sha256,
        "host": host,
        "repository": lock["repository"],
        "release_id": lock["release_id"],
        "tag": lock["tag"],
        "tag_object": lock["tag_object"],
        "source_commit": lock["source_commit"],
        "source_tree": lock["source_tree"],
        "source_date_epoch": lock["source_date_epoch"],
        "archive": manifest["archive"],
        "archive_sha256": manifest["archive_sha256"],
        "manifest_sha256": manifest_sha256,
    })
}

fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(64), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

fn sha256_hex(bytes: &[u8]) -> String {
    to_hex(&Sha256::digest(bytes))
}

/// Comparable file state (Python `_stable_file_state`).
type State = [i128; 9];

fn state(stat: &Stat) -> State {
    [
        i128::from(stat.st_dev),
        i128::from(stat.st_ino),
        i128::from(stat.st_mode),
        i128::from(stat.st_nlink),
        i128::from(stat.st_size),
        i128::from(stat.st_mtime),
        i128::from(stat.st_mtime_nsec),
        i128::from(stat.st_ctime),
        i128::from(stat.st_ctime_nsec),
    ]
}

fn same_identity(left: &Stat, right: &Stat) -> bool {
    i128::from(left.st_dev) == i128::from(right.st_dev)
        && i128::from(left.st_ino) == i128::from(right.st_ino)
}

fn is_dir(stat: &Stat) -> bool {
    FileType::from_raw_mode(stat.st_mode) == FileType::Directory
}

fn is_file(stat: &Stat) -> bool {
    FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile
}

struct Held {
    file: File,
    parent: usize,
    name: String,
    before: Stat,
}

fn list_exact(directory: &File, expected: &BTreeSet<String>, label: &str) -> Result<()> {
    let Ok(entries) = Dir::read_from(directory) else {
        return input(format!("cannot inspect runtime {label}"));
    };
    let mut observed = BTreeSet::new();
    for entry in entries {
        let Ok(entry) = entry else {
            return input(format!("cannot inspect runtime {label}"));
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        if name != "." && name != ".." {
            observed.insert(name);
        }
    }
    require_integrity(
        &observed == expected,
        &format!("runtime {label} differs from the expected closure"),
    )
}

fn open_directory(parent: &File, name: &str, relative: &str) -> Result<(File, Stat)> {
    let Ok(before) = statat(parent, name, AtFlags::SYMLINK_NOFOLLOW) else {
        return input(format!("cannot inspect runtime directory {relative}"));
    };
    require_integrity(
        is_dir(&before),
        &format!("runtime path is not a real directory: {relative}"),
    )?;
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let Ok(descriptor) = openat(parent, name, flags, Mode::empty()) else {
        return input(format!(
            "cannot open runtime directory {relative} without following symlinks"
        ));
    };
    let file = File::from(descriptor);
    let Ok(after) = fstat(&file) else {
        return input(format!("cannot inspect runtime directory {relative}"));
    };
    require_integrity(
        is_dir(&after) && same_identity(&before, &after),
        &format!("runtime directory changed during inspection: {relative}"),
    )?;
    Ok((file, after))
}

fn open_file(parent: &File, name: &str, relative: &str) -> Result<(File, Stat)> {
    let Ok(before) = statat(parent, name, AtFlags::SYMLINK_NOFOLLOW) else {
        return input(format!("cannot inspect runtime file {relative}"));
    };
    require_integrity(
        is_file(&before) && before.st_nlink == 1,
        &format!("runtime file is not a singly-linked regular file: {relative}"),
    )?;
    let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
    let Ok(descriptor) = openat(parent, name, flags, Mode::empty()) else {
        return input(format!(
            "cannot open runtime file {relative} without following symlinks"
        ));
    };
    let file = File::from(descriptor);
    let Ok(after) = fstat(&file) else {
        return input(format!("cannot inspect runtime file {relative}"));
    };
    require_integrity(
        is_file(&after) && after.st_nlink == 1 && same_identity(&before, &after),
        &format!("runtime file changed or is hard-linked: {relative}"),
    )?;
    Ok((file, after))
}

fn read_held(file: &File, label: &str, expected_size: Option<u64>) -> Result<Vec<u8>> {
    let Ok(before) = fstat(file) else {
        return input(format!("cannot inspect runtime metadata {label}"));
    };
    require(
        before.st_size >= 0 && (before.st_size as usize) <= MAX_METADATA,
        &format!("runtime metadata exceeds the size limit: {label}"),
    )?;
    if let Some(size) = expected_size {
        require_integrity(
            Some(before.st_size as u64) == Some(size),
            &format!("runtime metadata size differs from runtime lock: {label}"),
        )?;
    }
    let mut reader = file;
    if reader.seek(SeekFrom::Start(0)).is_err() {
        return input(format!("cannot read runtime metadata {label}"));
    }
    let mut contents = Vec::new();
    if reader
        .take(MAX_METADATA as u64 + 1)
        .read_to_end(&mut contents)
        .is_err()
    {
        return input(format!("cannot read runtime metadata {label}"));
    }
    require(
        contents.len() <= MAX_METADATA,
        &format!("runtime metadata exceeds the size limit: {label}"),
    )?;
    let Ok(after) = fstat(file) else {
        return input(format!("cannot inspect runtime metadata {label}"));
    };
    require_integrity(
        state(&before) == state(&after)
            && i128::try_from(contents.len()).is_ok_and(|len| len == i128::from(after.st_size)),
        &format!("runtime file changed while being read: {label}"),
    )?;
    Ok(contents)
}

fn hash_held(file: &File, label: &str, expected_size: u64) -> Result<(String, Stat)> {
    let Ok(before) = fstat(file) else {
        return input(format!("cannot inspect runtime file {label}"));
    };
    require_integrity(
        before.st_size >= 0 && before.st_size as u64 == expected_size,
        &format!("runtime file size differs from signed manifest: {label}"),
    )?;
    let mut reader = file;
    if reader.seek(SeekFrom::Start(0)).is_err() {
        return input(format!("cannot read runtime file {label}"));
    }
    let mut digest = Sha256::new();
    let mut block = vec![0_u8; BLOCK];
    loop {
        match reader.read(&mut block) {
            Ok(0) => break,
            Ok(count) => digest.update(&block[..count]),
            Err(_) => return input(format!("cannot read runtime file {label}")),
        }
    }
    let Ok(after) = fstat(file) else {
        return input(format!("cannot inspect runtime file {label}"));
    };
    require_integrity(
        state(&before) == state(&after),
        &format!("runtime file changed while being read: {label}"),
    )?;
    Ok((to_hex(&digest.finalize()), after))
}

fn mode_of(stat: &Stat) -> u32 {
    (i128::from(stat.st_mode) & 0o7777) as u32
}

/// Inspect the whole held runtime closure against caller-authenticated lock
/// bytes (Python `verify_extracted_frozen`). Returns the validated manifest.
///
/// # Errors
///
/// Fails when any name, file identity, byte, mode, receipt or concurrent
/// change differs from the locked closure.
pub fn verify_extracted_frozen(
    lock: &Value,
    lock_sha256: &str,
    host: &str,
    directory: &Path,
) -> Result<Value> {
    hex(&Value::from(lock_sha256), 64, "frozen runtime lock digest")?;
    validate_lock(lock)?;
    require(target_of(host).is_some(), "unsupported runtime host")?;

    let root_names: BTreeSet<String> = EXPECTED_FILES
        .iter()
        .filter_map(|(path, _, _)| path.split('/').next().map(str::to_owned))
        .chain([MANIFEST_NAME.to_owned(), RECEIPT_NAME.to_owned()])
        .collect();
    let bin_names: BTreeSet<String> = EXPECTED_FILES
        .iter()
        .filter_map(|(path, _, _)| path.strip_prefix("bin/").map(str::to_owned))
        .collect();

    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let Ok(root_fd) = rustix::fs::open(directory, flags, Mode::empty()) else {
        return input("cannot open runtime root without following symlinks");
    };
    let root = File::from(root_fd);
    let Ok(root_state) = fstat(&root) else {
        return input("cannot inspect runtime root");
    };
    require_integrity(
        is_dir(&root_state),
        "runtime verification requires a real directory for a supported host",
    )?;
    list_exact(&root, &root_names, "file set")?;
    let (bin, bin_state) = open_directory(&root, "bin", "bin")?;
    list_exact(&bin, &bin_names, "bin file set")?;

    let parents = [&root, &bin];
    let mut held: BTreeMap<String, Held> = BTreeMap::new();
    let mut relatives: Vec<String> = EXPECTED_FILES
        .iter()
        .map(|(p, _, _)| (*p).to_owned())
        .collect();
    relatives.sort();
    relatives.push(MANIFEST_NAME.to_owned());
    relatives.push(RECEIPT_NAME.to_owned());
    for relative in &relatives {
        let (parent, name) = relative
            .strip_prefix("bin/")
            .map_or((0, relative.as_str()), |rest| (1, rest));
        let (file, before) = open_file(parents[parent], name, relative)?;
        held.insert(
            relative.clone(),
            Held {
                file,
                parent,
                name: name.to_owned(),
                before,
            },
        );
    }

    let manifest_asset = &lock["hosts"][host]["manifest"];
    let manifest_bytes = read_held(
        &held[MANIFEST_NAME].file,
        MANIFEST_NAME,
        manifest_asset["size"].as_u64(),
    )?;
    let manifest = validate_manifest(lock, host, &manifest_bytes)?;
    let manifest_sha = sha256_hex(&manifest_bytes);
    let expected_receipt = receipt(lock, host, &manifest, &manifest_sha, lock_sha256);
    let receipt_bytes = read_held(&held[RECEIPT_NAME].file, RECEIPT_NAME, None)?;
    let observed_receipt: Value = match serde_json::from_slice(&receipt_bytes) {
        Ok(value) => value,
        Err(_) => return fail("cannot read JSON runtime receipt"),
    };
    require_integrity(
        observed_receipt == expected_receipt,
        "runtime receipt differs from locked release identity",
    )?;

    let mut expected_hashes = BTreeMap::from([
        (MANIFEST_NAME.to_owned(), manifest_sha),
        (RECEIPT_NAME.to_owned(), sha256_hex(&receipt_bytes)),
    ]);
    let mut expected_sizes = BTreeMap::from([
        (MANIFEST_NAME.to_owned(), manifest_bytes.len() as u64),
        (RECEIPT_NAME.to_owned(), receipt_bytes.len() as u64),
    ]);
    let files = manifest["files"].as_array().cloned().unwrap_or_default();
    let mut details: BTreeMap<String, (String, u64, u32)> = BTreeMap::new();
    for item in &files {
        let path = item["path"].as_str().unwrap_or_default().to_owned();
        let size = item["size"].as_u64().unwrap_or_default();
        let digest = item["sha256"].as_str().unwrap_or_default().to_owned();
        let mode = u32::from_str_radix(item["mode"].as_str().unwrap_or("0"), 8).unwrap_or(0);
        let Some(entry) = held.get(&path) else {
            return integrity("runtime file is missing from the held closure");
        };
        let (measured, stat) = hash_held(&entry.file, &path, size)?;
        require_integrity(
            stat.st_size as u64 == size && measured == digest,
            &format!("runtime file differs from signed manifest: {path}"),
        )?;
        require_integrity(
            mode_of(&stat) == mode,
            &format!("runtime file mode differs from the signed manifest: {path}"),
        )?;
        expected_hashes.insert(path.clone(), digest.clone());
        expected_sizes.insert(path.clone(), size);
        details.insert(path, (digest, size, mode));
    }

    // Recheck the closure through the held descriptors: names, identities,
    // bytes and modes, then the directory states (verify-only, no mutation).
    list_exact(&root, &root_names, "file set")?;
    list_exact(&bin, &bin_names, "bin file set")?;
    let (Ok(root_path), Ok(bin_path)) = (
        rustix::fs::stat(directory),
        statat(&root, "bin", AtFlags::SYMLINK_NOFOLLOW),
    ) else {
        return input("cannot recheck runtime directories");
    };
    let (Ok(root_now), Ok(bin_now)) = (fstat(&root), fstat(&bin)) else {
        return input("cannot recheck runtime directories");
    };
    require_integrity(
        is_dir(&root_path)
            && same_identity(&root_state, &root_now)
            && same_identity(&root_path, &root_now)
            && is_dir(&bin_path)
            && same_identity(&bin_state, &bin_now)
            && same_identity(&bin_path, &bin_now),
        "runtime directory changed during frozen closure verification",
    )?;
    for (relative, entry) in &held {
        let parent = parents[entry.parent];
        let (Ok(named), Ok(current)) = (
            statat(parent, entry.name.as_str(), AtFlags::SYMLINK_NOFOLLOW),
            fstat(&entry.file),
        ) else {
            return input(format!("cannot recheck runtime file {relative}"));
        };
        require_integrity(
            is_file(&named)
                && named.st_nlink == 1
                && current.st_nlink == 1
                && same_identity(&named, &current)
                && same_identity(&entry.before, &current)
                && state(&entry.before) == state(&current)
                && state(&named) == state(&current),
            &format!("runtime file changed during frozen closure verification: {relative}"),
        )?;
        let (digest, stable) = hash_held(&entry.file, relative, expected_sizes[relative])?;
        require_integrity(
            digest == expected_hashes[relative] && state(&entry.before) == state(&stable),
            &format!("runtime file differs from signed manifest: {relative}"),
        )?;
        if let Some((_, _, mode)) = details.get(relative) {
            require_integrity(
                mode_of(&stable) == *mode,
                &format!("runtime file mode differs from the signed manifest: {relative}"),
            )?;
        }
    }
    let (Ok(root_end), Ok(bin_end)) = (fstat(&root), fstat(&bin)) else {
        return input("cannot recheck runtime directories");
    };
    require_integrity(
        state(&root_state) == state(&root_end) && state(&bin_state) == state(&bin_end),
        "runtime directory state changed during frozen closure verification",
    )?;
    Ok(manifest)
}
