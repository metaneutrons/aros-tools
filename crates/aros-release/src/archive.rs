//! Normalized archive creation and strict read-back verification.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use aros_common::{
    sha256_file, sha256_reader, Diagnostic, DiagnosticCode, DiagnosticContext, DiagnosticStage,
};
use flate2::{read::GzDecoder, Compression, GzBuilder};
use serde::{Deserialize, Serialize};
use tar::{Archive, Builder, EntryType, Header};
use tempfile::NamedTempFile;

use crate::contract::{require_regular, PackageArgs, VerifyArgs};
use crate::{ReleaseFailure, ReleaseResult};

const MANIFEST_SCHEMA: u32 = 1;
const BINARIES: &[&str] = &[
    "aros",
    "aros-ahi-runner",
    "aros-collect",
    "aros-fetch",
    "aros-genmodule",
    "aros-romtool",
    "aros-transpiler",
    "aros-verify",
];
const DOCUMENTS: &[&str] = &["LICENSE-APACHE", "LICENSE-MIT", "README.md"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseManifest {
    pub schema: u32,
    pub package: String,
    pub version: String,
    pub target: String,
    pub source_commit: String,
    pub source_date_epoch: u64,
    pub archive: String,
    pub archive_sha256: String,
    pub archive_size: u64,
    pub files: Vec<ReleaseFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseFile {
    pub path: String,
    pub mode: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageOutput {
    pub archive: PathBuf,
    pub manifest: PathBuf,
    pub checksum: PathBuf,
}

/// Produce one normalized archive and verify it before returning.
///
/// # Errors
///
/// Returns a stable `AP02xx`-`AP04xx` diagnostic for missing input, unsafe
/// input types, publication failures, or any read-back mismatch.
pub fn package(args: &PackageArgs) -> ReleaseResult<PackageOutput> {
    args.validate()?;
    prepare_output_directory(&args.output_dir)?;

    let archive_name = format!("aros-tools-v{}-{}.tar.gz", args.version, args.target);
    let archive_path = args.output_dir.join(&archive_name);
    let manifest_path = args
        .output_dir
        .join(format!("{archive_name}.manifest.json"));
    let checksum_path = args.output_dir.join(format!("{archive_name}.sha256"));
    for path in [&archive_path, &manifest_path, &checksum_path] {
        if path.exists() {
            return Err(publication_failure(
                path,
                "refusing to replace an existing release artifact",
            ));
        }
    }

    let mut inputs = Vec::with_capacity(BINARIES.len() + DOCUMENTS.len());
    for name in BINARIES {
        let source = args.bin_dir.join(name);
        require_regular(&source, "release binary")?;
        inputs.push(InputFile::new(source, format!("bin/{name}"), 0o755)?);
    }
    for name in DOCUMENTS {
        let source = args.repository_root.join(name);
        require_regular(&source, "release document")?;
        inputs.push(InputFile::new(source, (*name).to_string(), 0o644)?);
    }
    inputs.sort_by(|left, right| left.path.cmp(&right.path));

    let root = format!("aros-tools-v{}-{}", args.version, args.target);
    let mut temporary = NamedTempFile::new_in(&args.output_dir)
        .map_err(|error| packaging_io(&archive_path, "create temporary archive", error))?;
    {
        let encoder = GzBuilder::new()
            .mtime(0)
            .write(temporary.as_file_mut(), Compression::best());
        let mut builder = Builder::new(encoder);
        builder.mode(tar::HeaderMode::Deterministic);
        append_directory(&mut builder, &root, args.source_date_epoch)?;
        append_directory(&mut builder, &format!("{root}/bin"), args.source_date_epoch)?;
        for input in &inputs {
            append_file(
                &mut builder,
                input,
                &format!("{root}/{}", input.path),
                args.source_date_epoch,
            )?;
        }
        let encoder = builder
            .into_inner()
            .map_err(|error| packaging_io(&archive_path, "finish tar stream", error))?;
        encoder
            .finish()
            .map_err(|error| packaging_io(&archive_path, "finish gzip stream", error))?;
    }
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| packaging_io(&archive_path, "synchronize archive", error))?;
    persist_new(temporary, &archive_path)?;

    let archive_hash = sha256_file(&archive_path)
        .map_err(|error| integrity_io(&archive_path, "hash archive", error))?;
    let manifest = ReleaseManifest {
        schema: MANIFEST_SCHEMA,
        package: "aros-tools".into(),
        version: args.version.clone(),
        target: args.target.clone(),
        source_commit: args.source_commit.clone(),
        source_date_epoch: args.source_date_epoch,
        archive: archive_name.clone(),
        archive_sha256: archive_hash.digest.to_string(),
        archive_size: archive_hash.size,
        files: inputs.into_iter().map(InputFile::into_manifest).collect(),
    };
    let mut document = serde_json::to_vec_pretty(&manifest).map_err(|error| {
        packaging_failure(
            &manifest_path,
            format!("cannot serialize release manifest: {error}"),
        )
    })?;
    document.push(b'\n');
    write_new_atomic(&manifest_path, &document)?;
    write_new_atomic(
        &checksum_path,
        format!("{}  {}\n", archive_hash.digest, archive_name).as_bytes(),
    )?;

    verify(&VerifyArgs {
        archive: archive_path.clone(),
        manifest: manifest_path.clone(),
    })?;
    Ok(PackageOutput {
        archive: archive_path,
        manifest: manifest_path,
        checksum: checksum_path,
    })
}

/// Strictly verify archive identity, metadata, inventory and file digests.
///
/// # Errors
///
/// Returns `AP0401` for any mismatch or non-regular payload member.
pub fn verify(args: &VerifyArgs) -> ReleaseResult<ReleaseManifest> {
    require_regular(&args.archive, "release archive")?;
    require_regular(&args.manifest, "release manifest")?;
    let manifest_bytes = fs::read(&args.manifest)
        .map_err(|error| integrity_io(&args.manifest, "read manifest", error))?;
    let manifest: ReleaseManifest = serde_json::from_slice(&manifest_bytes).map_err(|error| {
        integrity_failure(
            &args.manifest,
            format!("cannot parse release manifest: {error}"),
        )
    })?;
    validate_manifest(&manifest, &args.archive)?;

    let measured = sha256_file(&args.archive)
        .map_err(|error| integrity_io(&args.archive, "hash archive", error))?;
    if measured.digest.as_str() != manifest.archive_sha256 || measured.size != manifest.archive_size
    {
        return Err(integrity_failure(
            &args.archive,
            format!(
                "archive identity mismatch: manifest sha256={} size={}, measured sha256={} size={}",
                manifest.archive_sha256, manifest.archive_size, measured.digest, measured.size
            ),
        ));
    }

    let file = File::open(&args.archive)
        .map_err(|error| integrity_io(&args.archive, "open archive", error))?;
    let decoder = GzDecoder::new(file);
    if decoder.header().map_or(0, flate2::GzHeader::mtime) != 0 {
        return Err(integrity_failure(
            &args.archive,
            "gzip header mtime is not normalized to zero",
        ));
    }
    let mut archive = Archive::new(decoder);
    let root = format!("aros-tools-v{}-{}", manifest.version, manifest.target);
    let expected: BTreeMap<String, &ReleaseFile> = manifest
        .files
        .iter()
        .map(|file| (format!("{root}/{}", file.path), file))
        .collect();
    let expected_directories = BTreeSet::from([root.clone(), format!("{root}/bin")]);
    let mut seen_files = BTreeSet::new();
    let mut seen_directories = BTreeSet::new();

    let entries = archive
        .entries()
        .map_err(|error| integrity_io(&args.archive, "read archive inventory", error))?;
    for entry in entries {
        let mut entry =
            entry.map_err(|error| integrity_io(&args.archive, "read archive member", error))?;
        let path = entry
            .path()
            .map_err(|error| integrity_io(&args.archive, "decode archive path", error))?
            .to_string_lossy()
            .trim_end_matches('/')
            .to_string();
        let header = entry.header();
        verify_common_header(header, manifest.source_date_epoch, &args.archive)?;
        if header.entry_type() == EntryType::Directory {
            if !expected_directories.contains(&path) || !seen_directories.insert(path.clone()) {
                return Err(integrity_failure(
                    &args.archive,
                    format!("unexpected or duplicate directory member {path:?}"),
                ));
            }
            verify_mode(header, 0o755, &path, &args.archive)?;
            continue;
        }
        if header.entry_type() != EntryType::Regular {
            return Err(integrity_failure(
                &args.archive,
                format!("archive member {path:?} is not a regular file"),
            ));
        }
        let expected_file = expected.get(&path).ok_or_else(|| {
            integrity_failure(
                &args.archive,
                format!("archive contains undeclared member {path:?}"),
            )
        })?;
        if !seen_files.insert(path.clone()) {
            return Err(integrity_failure(
                &args.archive,
                format!("archive contains duplicate member {path:?}"),
            ));
        }
        let expected_mode = parse_mode(&expected_file.mode, &args.manifest)?;
        verify_mode(header, expected_mode, &path, &args.archive)?;
        let measured = sha256_reader(&mut entry)
            .map_err(|error| integrity_io(&args.archive, "hash archive member", error))?;
        if measured.digest.as_str() != expected_file.sha256 || measured.size != expected_file.size {
            return Err(integrity_failure(
                &args.archive,
                format!("payload identity mismatch for {path:?}"),
            ));
        }
    }
    if seen_directories != expected_directories {
        return Err(integrity_failure(
            &args.archive,
            "archive directory inventory is incomplete",
        ));
    }
    if seen_files.len() != expected.len() {
        let missing: Vec<_> = expected
            .keys()
            .filter(|path| !seen_files.contains(*path))
            .cloned()
            .collect();
        return Err(integrity_failure(
            &args.archive,
            format!(
                "archive is missing declared members: {}",
                missing.join(", ")
            ),
        ));
    }
    Ok(manifest)
}

#[derive(Debug)]
struct InputFile {
    source: PathBuf,
    path: String,
    mode: u32,
    sha256: String,
    size: u64,
}

impl InputFile {
    fn new(source: PathBuf, path: String, mode: u32) -> ReleaseResult<Self> {
        let measured = sha256_file(&source)
            .map_err(|error| integrity_io(&source, "hash release input", error))?;
        Ok(Self {
            source,
            path,
            mode,
            sha256: measured.digest.to_string(),
            size: measured.size,
        })
    }

    fn into_manifest(self) -> ReleaseFile {
        ReleaseFile {
            path: self.path,
            mode: format!("{:04o}", self.mode),
            sha256: self.sha256,
            size: self.size,
        }
    }
}

fn append_directory<W: Write>(
    builder: &mut Builder<W>,
    path: &str,
    mtime: u64,
) -> ReleaseResult<()> {
    let mut header = normalized_header(0, 0o755, mtime, EntryType::Directory);
    builder
        .append_data(&mut header, path, io::empty())
        .map_err(|error| packaging_io(Path::new(path), "append archive directory", error))
}

fn append_file<W: Write>(
    builder: &mut Builder<W>,
    input: &InputFile,
    path: &str,
    mtime: u64,
) -> ReleaseResult<()> {
    let mut source = File::open(&input.source)
        .map_err(|error| packaging_io(&input.source, "open release input", error))?;
    let mut header = normalized_header(input.size, input.mode, mtime, EntryType::Regular);
    builder
        .append_data(&mut header, path, &mut source)
        .map_err(|error| packaging_io(&input.source, "append release input", error))
}

fn normalized_header(size: u64, mode: u32, mtime: u64, kind: EntryType) -> Header {
    let mut header = Header::new_gnu();
    header.set_entry_type(kind);
    header.set_size(size);
    header.set_mode(mode);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(mtime);
    header.set_cksum();
    header
}

fn validate_manifest(manifest: &ReleaseManifest, archive: &Path) -> ReleaseResult<()> {
    if manifest.schema != MANIFEST_SCHEMA || manifest.package != "aros-tools" {
        return Err(integrity_failure(
            archive,
            format!(
                "unsupported release manifest identity schema={} package={:?}",
                manifest.schema, manifest.package
            ),
        ));
    }
    let archive_name = archive.file_name().and_then(|name| name.to_str());
    if archive_name != Some(manifest.archive.as_str()) {
        return Err(integrity_failure(
            archive,
            format!(
                "archive basename does not match manifest value {:?}",
                manifest.archive
            ),
        ));
    }
    if manifest.files.len() != BINARIES.len() + DOCUMENTS.len() {
        return Err(integrity_failure(
            archive,
            format!(
                "manifest declares {} files; expected 11",
                manifest.files.len()
            ),
        ));
    }
    let mut paths = BTreeSet::new();
    for file in &manifest.files {
        if !paths.insert(file.path.as_str())
            || file.path.starts_with('/')
            || file
                .path
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return Err(integrity_failure(
                archive,
                format!("manifest contains unsafe or duplicate path {:?}", file.path),
            ));
        }
        if aros_common::Sha256Digest::parse(&file.sha256).is_err() {
            return Err(integrity_failure(
                archive,
                format!("manifest has an invalid SHA-256 for {:?}", file.path),
            ));
        }
    }
    let required: BTreeSet<String> = BINARIES
        .iter()
        .map(|name| format!("bin/{name}"))
        .chain(DOCUMENTS.iter().map(|name| (*name).to_string()))
        .collect();
    if paths
        .into_iter()
        .map(str::to_string)
        .collect::<BTreeSet<_>>()
        != required
    {
        return Err(integrity_failure(
            archive,
            "manifest payload inventory is not the closed aros-tools release set",
        ));
    }
    if aros_common::Sha256Digest::parse(&manifest.archive_sha256).is_err() {
        return Err(integrity_failure(
            archive,
            "manifest archive_sha256 is not a SHA-256 digest",
        ));
    }
    Ok(())
}

fn verify_common_header(header: &Header, mtime: u64, archive: &Path) -> ReleaseResult<()> {
    let uid = header
        .uid()
        .map_err(|error| integrity_io(archive, "read archive uid", error))?;
    let gid = header
        .gid()
        .map_err(|error| integrity_io(archive, "read archive gid", error))?;
    let measured_mtime = header
        .mtime()
        .map_err(|error| integrity_io(archive, "read archive mtime", error))?;
    if uid != 0 || gid != 0 || measured_mtime != mtime {
        return Err(integrity_failure(
            archive,
            format!(
                "archive metadata is not normalized: uid={uid} gid={gid} mtime={measured_mtime}"
            ),
        ));
    }
    Ok(())
}

fn verify_mode(header: &Header, expected: u32, member: &str, archive: &Path) -> ReleaseResult<()> {
    let mode = header
        .mode()
        .map_err(|error| integrity_io(archive, "read archive mode", error))?;
    if mode != expected {
        return Err(integrity_failure(
            archive,
            format!("archive member {member:?} has mode {mode:04o}; expected {expected:04o}"),
        ));
    }
    Ok(())
}

fn parse_mode(value: &str, manifest: &Path) -> ReleaseResult<u32> {
    u32::from_str_radix(value, 8).map_err(|error| {
        integrity_failure(
            manifest,
            format!("manifest contains invalid file mode {value:?}: {error}"),
        )
    })
}

fn prepare_output_directory(path: &Path) -> ReleaseResult<()> {
    fs::create_dir_all(path)
        .map_err(|error| packaging_io(path, "create output directory", error))?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| packaging_io(path, "inspect output directory", error))?;
    if !metadata.file_type().is_dir() {
        return Err(publication_failure(path, "output-dir is not a directory"));
    }
    Ok(())
}

fn write_new_atomic(path: &Path, bytes: &[u8]) -> ReleaseResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| publication_failure(path, "output path has no parent directory"))?;
    let mut temporary = NamedTempFile::new_in(parent)
        .map_err(|error| packaging_io(path, "create temporary sidecar", error))?;
    temporary
        .write_all(bytes)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|error| packaging_io(path, "write and synchronize sidecar", error))?;
    persist_new(temporary, path)
}

fn persist_new(temporary: NamedTempFile, path: &Path) -> ReleaseResult<()> {
    temporary
        .persist_noclobber(path)
        .map(|_| ())
        .map_err(|error| {
            publication_failure(
                path,
                format!("cannot publish new artifact atomically: {}", error.error),
            )
        })
}

fn packaging_io(path: &Path, action: &str, error: io::Error) -> ReleaseFailure {
    let detail = error.to_string();
    drop(error);
    packaging_failure(path, format!("cannot {action}: {detail}"))
}

fn packaging_failure(path: &Path, message: impl Into<String>) -> ReleaseFailure {
    ReleaseFailure::new(
        Diagnostic::error(
            DiagnosticCode::ReleasePackaging,
            DiagnosticStage::ArchivePackaging,
            message,
        )
        .with_context(DiagnosticContext {
            output: Some(path.display().to_string()),
            ..DiagnosticContext::default()
        }),
    )
}

fn integrity_io(path: &Path, action: &str, error: io::Error) -> ReleaseFailure {
    let detail = error.to_string();
    drop(error);
    integrity_failure(path, format!("cannot {action}: {detail}"))
}

fn integrity_failure(path: &Path, message: impl Into<String>) -> ReleaseFailure {
    ReleaseFailure::new(
        Diagnostic::error(
            DiagnosticCode::ReleaseIntegrity,
            DiagnosticStage::ReleaseIntegrity,
            message,
        )
        .with_context(DiagnosticContext {
            target: Some(path.display().to_string()),
            ..DiagnosticContext::default()
        })
        .with_hint("discard the artifact; rebuild from the immutable source and verify again"),
    )
}

fn publication_failure(path: &Path, message: impl Into<String>) -> ReleaseFailure {
    ReleaseFailure::new(
        Diagnostic::error(
            DiagnosticCode::ReleasePublication,
            DiagnosticStage::Publication,
            message,
        )
        .with_context(DiagnosticContext {
            output: Some(path.display().to_string()),
            ..DiagnosticContext::default()
        })
        .with_hint("use a new empty release destination; published artifacts are immutable"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_fixture(path: &Path, bytes: &[u8]) {
        fs::write(path, bytes).unwrap();
    }

    fn fixture(root: &Path, output: &Path) -> PackageArgs {
        let bin = root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        for name in BINARIES {
            write_fixture(&bin.join(name), format!("binary:{name}\n").as_bytes());
        }
        for name in DOCUMENTS {
            write_fixture(&root.join(name), format!("document:{name}\n").as_bytes());
        }
        PackageArgs {
            version: "1.2.3".into(),
            source_commit: "a".repeat(40),
            source_date_epoch: 1_700_000_000,
            target: "x86_64-unknown-linux-gnu".into(),
            bin_dir: bin,
            repository_root: root.to_path_buf(),
            output_dir: output.to_path_buf(),
        }
    }

    #[test]
    fn two_independent_packages_are_byte_identical_and_verify() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let first_output = package(&fixture(source.path(), first.path())).unwrap();
        let second_output = package(&fixture(source.path(), second.path())).unwrap();
        assert_eq!(
            fs::read(&first_output.archive).unwrap(),
            fs::read(&second_output.archive).unwrap()
        );
        assert_eq!(
            fs::read(&first_output.manifest).unwrap(),
            fs::read(&second_output.manifest).unwrap()
        );
        verify(&VerifyArgs {
            archive: first_output.archive,
            manifest: first_output.manifest,
        })
        .unwrap();
    }

    #[test]
    fn changed_archive_is_rejected() {
        let output = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let produced = package(&fixture(source.path(), output.path())).unwrap();
        let mut bytes = fs::read(&produced.archive).unwrap();
        bytes[0] ^= 0xff;
        fs::write(&produced.archive, bytes).unwrap();
        let failure = verify(&VerifyArgs {
            archive: produced.archive,
            manifest: produced.manifest,
        })
        .unwrap_err();
        assert_eq!(failure.diagnostic().code, DiagnosticCode::ReleaseIntegrity);
    }

    #[cfg(unix)]
    #[test]
    fn symbolic_link_input_is_rejected() {
        use std::os::unix::fs::symlink;

        let output = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let args = fixture(source.path(), output.path());
        fs::remove_file(args.bin_dir.join("aros")).unwrap();
        symlink(args.bin_dir.join("aros-fetch"), args.bin_dir.join("aros")).unwrap();
        let failure = package(&args).unwrap_err();
        assert_eq!(failure.diagnostic().code, DiagnosticCode::ReleaseInput);
    }
}
