//! Read-only snapshots of source-declared host-C generator inputs.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read as _};
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use aros_common::native_host_generator::{NativeHostFileGenerator, NativeHostFileInput};
use aros_fetch::engine::cache::VerifiedCachePayload;

use crate::filesystem::open_directory;
use crate::ContractError;

/// Verified private copies of the raw files selected by host generators.
pub(super) struct HostGeneratorInputs {
    cache: CacheRoot,
    staged: CacheRoot,
    input_names: Vec<String>,
    snapshots: Vec<VerifiedCachePayload>,
    // Keep the private named copies alive for the complete consumer execution.
    _directory: tempfile::TempDir,
}

impl fmt::Debug for HostGeneratorInputs {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HostGeneratorInputs")
            .field("root", &self.staged.root)
            .field("input_count", &self.snapshots.len())
            .finish_non_exhaustive()
    }
}

impl HostGeneratorInputs {
    /// Private verified directory consumed by CMake, never the mutable cache.
    pub(super) fn root(&self) -> &Path {
        &self.staged.root
    }

    pub(super) fn cache_root(&self) -> &Path {
        &self.cache.root
    }

    /// Recheck the selected cache directory and every input snapshot.
    pub(super) fn revalidate(&self) -> Result<(), ContractError> {
        if self.input_names.len() != self.snapshots.len() {
            return Err(ContractError::compatibility(
                "host generator input snapshot metadata is inconsistent",
            ));
        }

        self.cache.revalidate()?;
        self.staged.revalidate()?;
        let entry_count = fs::read_dir(&self.staged.root)
            .map_err(|error| input_error("directory", "cannot inspect staged inputs", error))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| input_error("directory", "cannot enumerate staged inputs", error))?
            .len();
        if entry_count != self.input_names.len() {
            return Err(ContractError::compatibility(
                "private host generator input directory has unexpected entries",
            ));
        }

        for (filename, snapshot) in self.input_names.iter().zip(&self.snapshots) {
            snapshot
                .revalidate()
                .map_err(|error| input_error(filename, "changed after preparation", error))?;
            verify_staged_input(&self.staged.root.join(filename), snapshot)?;
        }
        Ok(())
    }
}

/// Snapshot every distinct locked input from an existing cache, without transport.
///
/// The contract metadata has already been validated by `aros-common`. This
/// function still rejects conflicting declarations that share a
/// case-insensitive filename so the cache namespace remains unambiguous.
pub(super) fn prepare(
    generators: &[NativeHostFileGenerator],
    cache_root: Option<&Path>,
) -> Result<Option<HostGeneratorInputs>, ContractError> {
    let mut inputs = BTreeMap::<String, NativeHostFileInput>::new();
    for generator in generators {
        for input in &generator.inputs {
            let key = input.filename.to_ascii_lowercase();
            if let Some(previous) = inputs.get(&key) {
                if previous != input {
                    return Err(ContractError::compatibility(format!(
                        "host generator inputs have conflicting declarations for filename {}",
                        input.filename
                    )));
                }
            } else {
                inputs.insert(key, input.clone());
            }
        }
    }
    if inputs.is_empty() {
        return Ok(None);
    }

    let cache_root = cache_root.ok_or_else(|| {
        ContractError::compatibility(
            "host generator inputs require an explicit existing cache directory",
        )
    })?;
    let root = checked_cache_root(cache_root)?;
    let directory = tempfile::Builder::new()
        .prefix("aros-compat-host-inputs-")
        .tempdir()
        .map_err(|error| input_error("directory", "cannot create private staging", error))?;
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
        .map_err(|error| input_error("directory", "cannot restrict private staging", error))?;
    let staged = checked_cache_root(directory.path())?;
    let mut input_names = Vec::with_capacity(inputs.len());
    let mut snapshots = Vec::with_capacity(inputs.len());
    for input in inputs.values() {
        let snapshot = aros_fetch::engine::cache::snapshot_verified_cache_payload(
            &root.root,
            &input.filename,
            input.size,
            &input.sha256,
        )
        .map_err(|error| input_error(&input.filename, "cannot verify cached bytes", error))?;
        snapshot
            .revalidate()
            .map_err(|error| input_error(&input.filename, "changed while being prepared", error))?;
        let destination = staged.root.join(&input.filename);
        let mut source = open_regular(snapshot.path())?;
        let mut output = File::options()
            .write(true)
            .create_new(true)
            .open(&destination)
            .map_err(|error| input_error(&input.filename, "cannot stage private input", error))?;
        io::copy(&mut source, &mut output).map_err(|error| {
            input_error(&input.filename, "cannot copy verified snapshot", error)
        })?;
        output
            .set_permissions(fs::Permissions::from_mode(0o444))
            .map_err(|error| input_error(&input.filename, "cannot seal private input", error))?;
        verify_staged_input(&destination, &snapshot)?;
        input_names.push(input.filename.clone());
        snapshots.push(snapshot);
    }

    let prepared = HostGeneratorInputs {
        cache: root,
        staged,
        input_names,
        snapshots,
        _directory: directory,
    };
    prepared.revalidate()?;
    Ok(Some(prepared))
}

struct CacheRoot {
    root: PathBuf,
    handle: File,
    identity: DirectoryIdentity,
}

impl CacheRoot {
    fn revalidate(&self) -> Result<(), ContractError> {
        let current = checked_cache_root(&self.root)?;
        let retained = directory_identity(&self.handle.metadata().map_err(|error| {
            input_error("directory", "cannot inspect retained directory", error)
        })?)?;
        let identities_unchanged = [current.identity, retained]
            .into_iter()
            .all(|identity| identity == self.identity);
        if current.root != self.root || !identities_unchanged {
            return Err(ContractError::compatibility(
                "host generator input directory changed after preparation",
            ));
        }
        Ok(())
    }
}

fn open_regular(path: &Path) -> Result<File, ContractError> {
    let descriptor = rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .map_err(|error| input_error("snapshot", "cannot open private regular input", error))?;
    let file = File::from(descriptor);
    if !file
        .metadata()
        .map_err(|error| input_error("snapshot", "cannot inspect private input", error))?
        .is_file()
    {
        return Err(ContractError::compatibility(
            "private host generator input is not a regular file",
        ));
    }
    Ok(file)
}

fn verify_staged_input(path: &Path, snapshot: &VerifiedCachePayload) -> Result<(), ContractError> {
    let file = open_regular(path)?;
    let metadata = file
        .metadata()
        .map_err(|error| input_error("snapshot", "cannot inspect staged input", error))?;
    if metadata.len() != snapshot.size() || metadata.permissions().mode() & 0o222 != 0 {
        return Err(ContractError::compatibility(
            "private host generator input size or read-only mode changed",
        ));
    }
    let limit = snapshot.size().checked_add(1).ok_or_else(|| {
        ContractError::compatibility("host generator input size is not representable")
    })?;
    let measured = aros_common::sha256_reader(&mut file.take(limit))
        .map_err(|error| input_error("snapshot", "cannot measure staged input", error))?;
    if measured.size != snapshot.size() || measured.digest != *snapshot.sha256() {
        return Err(ContractError::compatibility(
            "private host generator input differs from its verified snapshot",
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct DirectoryIdentity {
    device: u64,
    inode: u64,
}

fn checked_cache_root(path: &Path) -> Result<CacheRoot, ContractError> {
    let supplied_metadata = fs::symlink_metadata(path).map_err(|_| {
        ContractError::compatibility(
            "host generator cache directory is absent or cannot be inspected",
        )
    })?;
    if supplied_metadata.file_type().is_symlink() || !supplied_metadata.is_dir() {
        return Err(ContractError::compatibility(
            "host generator cache root must be an existing non-symlink directory",
        ));
    }

    let absolute = path.canonicalize().map_err(|error| {
        input_error(
            "directory",
            "cannot resolve host generator cache directory",
            error,
        )
    })?;
    let root = super::checked_directory(&absolute, "host generator cache directory")?;
    let root_metadata = fs::symlink_metadata(&root).map_err(|_| {
        ContractError::compatibility("cannot inspect the canonical host generator cache root")
    })?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err(ContractError::compatibility(
            "canonical host generator cache root is not a real directory",
        ));
    }
    let handle = open_directory(&root).map_err(|_| {
        ContractError::compatibility(
            "cannot open the host generator cache root without following links",
        )
    })?;
    let supplied_identity = directory_identity(&supplied_metadata)?;
    let root_identity = directory_identity(&root_metadata)?;
    let handle_identity = directory_identity(&handle.metadata().map_err(|_| {
        ContractError::compatibility("cannot inspect the opened host generator cache root")
    })?)?;
    let final_supplied_metadata = fs::symlink_metadata(path).map_err(|_| {
        ContractError::compatibility("host generator cache root changed while it was checked")
    })?;
    if final_supplied_metadata.file_type().is_symlink() || !final_supplied_metadata.is_dir() {
        return Err(ContractError::compatibility(
            "host generator cache root changed to a symlink or non-directory",
        ));
    }
    let final_supplied_identity = directory_identity(&final_supplied_metadata)?;
    if supplied_identity != root_identity
        || root_identity != handle_identity
        || final_supplied_identity != supplied_identity
    {
        return Err(ContractError::compatibility(
            "host generator cache root changed while it was being checked",
        ));
    }

    Ok(CacheRoot {
        root,
        handle,
        identity: root_identity,
    })
}

fn directory_identity(metadata: &fs::Metadata) -> Result<DirectoryIdentity, ContractError> {
    if !metadata.is_dir() {
        return Err(ContractError::compatibility(
            "host generator cache root is no longer a directory",
        ));
    }
    Ok(DirectoryIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

fn input_error(filename: &str, action: &str, error: impl fmt::Display) -> ContractError {
    ContractError::compatibility(format!("host generator input {filename} {action}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aros_common::sha256_bytes;

    fn input(filename: &str, bytes: &[u8]) -> NativeHostFileInput {
        NativeHostFileInput {
            filename: filename.into(),
            url: format!("https://example.invalid/1/{filename}"),
            sha256: sha256_bytes(bytes),
            size: u64::try_from(bytes.len()).unwrap(),
        }
    }

    fn generator(inputs: Vec<NativeHostFileInput>) -> NativeHostFileGenerator {
        NativeHostFileGenerator {
            owner: "test-generator".into(),
            recipe: "test/mmakefile.src".into(),
            tool_recipe: "tools/test/Makefile".into(),
            tool_source: "tools/test/tool.c".into(),
            tool_variable: "TESTGEN".into(),
            output: "gen/test/out.c".into(),
            input_directory: "gen/test-data".into(),
            compile_flags: Vec::new(),
            arguments: vec![
                "@INPUT_DIRECTORY@".into(),
                "@OUTPUT_DIRECTORY@".into(),
                "test".into(),
            ],
            inputs,
        }
    }

    fn canonical(path: &Path) -> PathBuf {
        path.canonicalize().unwrap()
    }

    #[test]
    fn snapshots_two_exact_raw_files_from_the_cache() {
        let temporary = tempfile::tempdir().unwrap();
        let root = canonical(temporary.path());
        let first = input("Alpha.txt", b"alpha raw bytes\n");
        let second = input("Beta.txt", b"beta raw bytes\n");
        fs::write(root.join(&first.filename), b"alpha raw bytes\n").unwrap();
        fs::write(root.join(&second.filename), b"beta raw bytes\n").unwrap();

        let prepared = prepare(&[generator(vec![first, second])], Some(&root))
            .unwrap()
            .unwrap();

        assert_eq!(prepared.cache_root(), root);
        assert_ne!(prepared.root(), root);
        assert_eq!(prepared.snapshots.len(), 2);
        assert_eq!(prepared.input_names, ["Alpha.txt", "Beta.txt"]);
        assert_eq!(
            fs::read(prepared.snapshots[0].path()).unwrap(),
            b"alpha raw bytes\n"
        );
        assert_eq!(
            fs::read(prepared.snapshots[1].path()).unwrap(),
            b"beta raw bytes\n"
        );
        prepared.revalidate().unwrap();
    }

    #[test]
    fn identical_case_insensitive_declarations_are_snapshotted_once() {
        let temporary = tempfile::tempdir().unwrap();
        let root = canonical(temporary.path());
        let declared = input("Shared.txt", b"shared bytes\n");
        fs::write(root.join(&declared.filename), b"shared bytes\n").unwrap();

        let prepared = prepare(
            &[generator(vec![declared.clone()]), generator(vec![declared])],
            Some(&root),
        )
        .unwrap()
        .unwrap();

        assert_eq!(prepared.snapshots.len(), 1);
    }

    #[test]
    fn consumer_reads_private_bytes_even_if_cache_is_changed_and_restored() {
        let temporary = tempfile::tempdir().unwrap();
        let root = canonical(temporary.path());
        let bytes = b"locked bytes\n";
        let declared = input("raw.txt", bytes);
        fs::write(root.join("raw.txt"), bytes).unwrap();
        let prepared = prepare(&[generator(vec![declared])], Some(&root))
            .unwrap()
            .unwrap();
        let consumed = prepared.root().join("raw.txt");
        assert_ne!(consumed, root.join("raw.txt"));
        fs::write(root.join("raw.txt"), b"malice bytes\n").unwrap();
        assert_eq!(fs::read(&consumed).unwrap(), bytes);
        assert!(prepared.revalidate().is_err());
        fs::write(root.join("raw.txt"), bytes).unwrap();
        assert_eq!(fs::read(&consumed).unwrap(), bytes);
        // Restoring bytes does not restore the retained source timestamps.
        // The cache mutation remains a failure, and CMake never saw its bytes.
        assert!(prepared.revalidate().is_err());
        let private_root = prepared.root().to_path_buf();
        drop(prepared);
        assert!(!private_root.exists());
        assert_eq!(fs::read(root.join("raw.txt")).unwrap(), bytes);
    }

    #[test]
    fn private_input_mutation_or_extra_entries_are_rejected() {
        let temporary = tempfile::tempdir().unwrap();
        let root = canonical(temporary.path());
        let bytes = b"locked bytes\n";
        fs::write(root.join("raw.txt"), bytes).unwrap();
        let prepared = prepare(&[generator(vec![input("raw.txt", bytes)])], Some(&root))
            .unwrap()
            .unwrap();
        let consumed = prepared.root().join("raw.txt");
        fs::set_permissions(&consumed, fs::Permissions::from_mode(0o644)).unwrap();
        fs::write(&consumed, b"malice bytes\n").unwrap();
        fs::set_permissions(&consumed, fs::Permissions::from_mode(0o444)).unwrap();
        assert!(prepared.revalidate().is_err());
        fs::set_permissions(&consumed, fs::Permissions::from_mode(0o644)).unwrap();
        fs::write(&consumed, bytes).unwrap();
        fs::set_permissions(&consumed, fs::Permissions::from_mode(0o444)).unwrap();
        prepared.revalidate().unwrap();
        fs::write(prepared.root().join("unexpected.txt"), b"extra bytes").unwrap();
        assert!(prepared.revalidate().is_err());
    }

    #[test]
    fn relative_cache_root_is_canonicalized_without_changing_working_directory() {
        let current = std::env::current_dir().unwrap();
        let temporary = tempfile::tempdir_in(&current).unwrap();
        let relative = temporary.path().strip_prefix(&current).unwrap();
        fs::write(temporary.path().join("raw.txt"), b"bytes\n").unwrap();
        let prepared = prepare(
            &[generator(vec![input("raw.txt", b"bytes\n")])],
            Some(relative),
        )
        .unwrap()
        .unwrap();
        assert_eq!(prepared.cache_root(), canonical(temporary.path()));
        prepared.revalidate().unwrap();
    }

    #[test]
    fn absent_cache_root_or_input_fails_without_creating_cache_state() {
        let temporary = tempfile::tempdir().unwrap();
        let absent_root = temporary.path().join("absent-cache");
        let declared = input("raw.txt", b"raw\n");
        let generators = [generator(vec![declared])];

        assert!(prepare(&generators, None).is_err());
        assert!(prepare(&generators, Some(&absent_root)).is_err());
        assert!(!absent_root.exists());

        let cache = temporary.path().join("cache");
        fs::create_dir(&cache).unwrap();
        assert!(prepare(&generators, Some(&canonical(&cache))).is_err());
        assert_eq!(fs::read_dir(&cache).unwrap().count(), 0);
    }

    #[test]
    fn corrupt_size_and_digest_are_rejected() {
        let temporary = tempfile::tempdir().unwrap();
        let root = canonical(temporary.path());
        let bytes = b"same-size bytes\n";
        fs::write(root.join("raw.txt"), bytes).unwrap();

        let mut wrong_size = input("raw.txt", bytes);
        wrong_size.size += 1;
        assert!(prepare(&[generator(vec![wrong_size])], Some(&root)).is_err());

        let mut wrong_digest = input("raw.txt", bytes);
        wrong_digest.sha256 = sha256_bytes(b"same-size byteS\n");
        assert!(prepare(&[generator(vec![wrong_digest])], Some(&root)).is_err());
    }

    #[test]
    fn conflicting_case_insensitive_declarations_are_rejected() {
        let first = input("Raw.txt", b"first\n");
        let mut second = first.clone();
        second.filename = "raw.TXT".into();
        second.url = "https://example.invalid/1/raw.TXT".into();
        second.sha256 = sha256_bytes(b"other\n");

        assert!(prepare(&[generator(vec![first]), generator(vec![second])], None,).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_input_and_cache_root_are_rejected() {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::tempdir().unwrap();
        let real_root = temporary.path().join("real-cache");
        fs::create_dir(&real_root).unwrap();
        let root = canonical(&real_root);
        let declared = input("raw.txt", b"raw bytes\n");
        fs::write(root.join("actual.txt"), b"raw bytes\n").unwrap();
        symlink("actual.txt", root.join("raw.txt")).unwrap();
        assert!(prepare(&[generator(vec![declared.clone()])], Some(&root)).is_err());

        fs::remove_file(root.join("raw.txt")).unwrap();
        fs::write(root.join("raw.txt"), b"raw bytes\n").unwrap();
        let linked_root = temporary.path().join("linked-cache");
        symlink(&real_root, &linked_root).unwrap();
        assert!(prepare(&[generator(vec![declared])], Some(&linked_root)).is_err());
    }

    #[test]
    fn revalidate_rejects_changed_input_and_replaced_cache_directory() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("cache");
        fs::create_dir(&root).unwrap();
        let canonical_root = canonical(&root);
        let declared = input("raw.txt", b"original bytes\n");
        fs::write(canonical_root.join(&declared.filename), b"original bytes\n").unwrap();
        let prepared = prepare(&[generator(vec![declared.clone()])], Some(&canonical_root))
            .unwrap()
            .unwrap();

        fs::write(canonical_root.join(&declared.filename), b"changed bytes!\n").unwrap();
        assert!(prepared.revalidate().is_err());

        drop(prepared);
        fs::write(canonical_root.join(&declared.filename), b"original bytes\n").unwrap();
        let prepared = prepare(&[generator(vec![declared])], Some(&canonical_root))
            .unwrap()
            .unwrap();
        let moved_root = temporary.path().join("old-cache");
        fs::rename(&canonical_root, &moved_root).unwrap();
        fs::create_dir(&canonical_root).unwrap();
        fs::write(canonical_root.join("raw.txt"), b"original bytes\n").unwrap();
        assert!(prepared.revalidate().is_err());
    }

    #[test]
    fn empty_generators_do_not_inspect_or_create_a_cache_root() {
        let temporary = tempfile::tempdir().unwrap();
        let absent_root = temporary.path().join("cache-not-created");

        assert!(prepare(&[], None).unwrap().is_none());
        assert!(prepare(&[generator(Vec::new())], Some(&absent_root))
            .unwrap()
            .is_none());
        assert!(!absent_root.exists());
    }
}
