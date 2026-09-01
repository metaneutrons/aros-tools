//! No-clobber installation of one complete verified native release suite.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use aros_common::{
    publication_failure_class, CommitState, Diagnostic, DiagnosticCode, DiagnosticContext,
    DiagnosticStage, DurableFileSet, PublicationFailureClass, PublicationReceipt,
};

use crate::archive::BINARIES;
use crate::contract::InstallArgs;
use crate::{ReleaseFailure, ReleaseResult};

const MAX_BINARY_BYTES: u64 = 256 * 1024 * 1024;
const JOURNAL_NAME: &str = ".aros-tools-install.journal";

/// Successful installation and any predecessor recovery performed under the
/// durable publication lock.
#[derive(Debug)]
pub struct InstallOutcome {
    pub publication: PublicationReceipt,
}

/// Install exactly the eight release programs without replacing any existing
/// destination.
///
/// The complete source inventory is validated and snapshotted before the
/// first destination mutation. Publication uses one lock-protected,
/// compare-and-swap journal, so a failure or concurrent creator rolls every
/// owned destination back.
///
/// # Errors
///
/// Returns `AP01xx` for an unsafe invocation, `AP02xx` for an invalid source
/// suite, or `AP05xx` for a destination conflict or publication failure.
pub fn install(args: &InstallArgs) -> ReleaseResult<InstallOutcome> {
    args.validate()?;
    #[cfg(not(unix))]
    {
        let _ = args;
        return Err(publication_failure(
            Path::new("."),
            CommitState::RolledBack,
            "native suite installation requires Unix no-follow and durable rename primitives",
            "install on a supported Unix host or use the platform package manager",
        ));
    }
    #[cfg(unix)]
    install_unix(args)
}

#[cfg(unix)]
fn install_unix(args: &InstallArgs) -> ReleaseResult<InstallOutcome> {
    let source = validated_source_directory(&args.source_bin)?;
    let payloads = snapshot_suite(&source)?;
    let (prefix, destination) = prepare_destination(&args.prefix)?;
    let journal = prefix.join(JOURNAL_NAME);
    let mut transaction =
        DurableFileSet::new(&journal).map_err(|error| publication_io(&destination, &error))?;
    for (name, contents) in payloads {
        transaction
            .stage_create_mode(&destination.join(name), &contents, 0o755)
            .map_err(|error| publication_io(&destination, &error))?;
    }
    let publication = transaction
        .commit()
        .map_err(|error| publication_io(&destination, &error))?;
    Ok(InstallOutcome { publication })
}

#[cfg(unix)]
fn validated_source_directory(path: &Path) -> ReleaseResult<PathBuf> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        input_failure(path, format!("cannot inspect source directory: {error}"))
    })?;
    if !metadata.file_type().is_dir() {
        return Err(input_failure(
            path,
            "source-bin must be a real directory, not a link or special file",
        ));
    }
    path.canonicalize()
        .map_err(|error| input_failure(path, format!("cannot resolve source directory: {error}")))
}

#[cfg(unix)]
fn snapshot_suite(source: &Path) -> ReleaseResult<Vec<(String, Vec<u8>)>> {
    use rustix::fs::{openat, Mode, OFlags};
    use std::io::Read as _;
    use std::os::unix::fs::MetadataExt as _;

    let expected: BTreeSet<_> = BINARIES.iter().copied().collect();
    let mut observed = BTreeSet::new();
    for entry in fs::read_dir(source)
        .map_err(|error| input_failure(source, format!("cannot enumerate source suite: {error}")))?
    {
        let entry = entry.map_err(|error| {
            input_failure(source, format!("cannot enumerate source suite: {error}"))
        })?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| input_failure(source, "source suite contains a non-UTF-8 entry name"))?;
        observed.insert(name);
    }
    let observed_refs: BTreeSet<_> = observed.iter().map(String::as_str).collect();
    if observed_refs != expected {
        return Err(input_failure(
            source,
            format!(
                "source suite inventory is not exact; expected {expected:?}, observed {observed_refs:?}"
            ),
        ));
    }

    let directory = rustix::fs::open(
        source,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| {
        input_failure(
            source,
            format!("cannot open source directory without following links: {error}"),
        )
    })?;
    let mut payloads = Vec::with_capacity(BINARIES.len());
    for name in BINARIES {
        let path = source.join(name);
        let descriptor = openat(
            &directory,
            Path::new(name),
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| {
            input_failure(
                &path,
                format!("cannot open release binary without following links: {error}"),
            )
        })?;
        let mut file = fs::File::from(descriptor);
        let before = file
            .metadata()
            .map_err(|error| input_failure(&path, format!("cannot inspect binary: {error}")))?;
        if !before.file_type().is_file() || before.mode() & 0o777 != 0o755 {
            return Err(input_failure(
                &path,
                "release binary must be a regular file with exact mode 0755",
            ));
        }
        if before.len() > MAX_BINARY_BYTES {
            return Err(input_failure(
                &path,
                format!("release binary exceeds the {MAX_BINARY_BYTES}-byte safety limit"),
            ));
        }
        let mut contents = Vec::with_capacity(usize::try_from(before.len()).unwrap_or(0));
        file.by_ref()
            .take(MAX_BINARY_BYTES + 1)
            .read_to_end(&mut contents)
            .map_err(|error| input_failure(&path, format!("cannot read binary: {error}")))?;
        let after = file
            .metadata()
            .map_err(|error| input_failure(&path, format!("cannot re-inspect binary: {error}")))?;
        if contents.len() as u64 != before.len()
            || before.dev() != after.dev()
            || before.ino() != after.ino()
            || before.len() != after.len()
            || before.mtime() != after.mtime()
            || before.mtime_nsec() != after.mtime_nsec()
            || before.mode() != after.mode()
        {
            return Err(input_failure(
                &path,
                "release binary changed while it was being snapshotted",
            ));
        }
        payloads.push(((*name).to_owned(), contents));
    }
    Ok(payloads)
}

#[cfg(unix)]
fn prepare_destination(prefix: &Path) -> ReleaseResult<(PathBuf, PathBuf)> {
    use std::os::unix::fs::PermissionsExt as _;

    let metadata = fs::symlink_metadata(prefix)
        .map_err(|error| publication_input(prefix, format!("cannot inspect prefix: {error}")))?;
    if !metadata.file_type().is_dir() {
        return Err(publication_input(
            prefix,
            "installation prefix must be a real existing directory",
        ));
    }
    let prefix = prefix
        .canonicalize()
        .map_err(|error| publication_input(prefix, format!("cannot resolve prefix: {error}")))?;
    let bin = prefix.join("bin");
    match fs::symlink_metadata(&bin) {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => {
            return Err(publication_input(
                &bin,
                "installation bin path is not a real directory",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match fs::create_dir(&bin) {
                Ok(()) => fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).map_err(
                    |error| publication_input(&bin, format!("cannot set bin mode: {error}")),
                )?,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(publication_input(
                        &bin,
                        format!("cannot create bin directory: {error}"),
                    ));
                }
            }
            let metadata = fs::symlink_metadata(&bin).map_err(|error| {
                publication_input(
                    &bin,
                    format!("cannot inspect created bin directory: {error}"),
                )
            })?;
            if !metadata.file_type().is_dir() {
                return Err(publication_input(
                    &bin,
                    "concurrent creator installed an unsafe bin path",
                ));
            }
        }
        Err(error) => {
            return Err(publication_input(
                &bin,
                format!("cannot inspect bin directory: {error}"),
            ));
        }
    }
    Ok((prefix, bin))
}

fn input_failure(path: &Path, message: impl Into<String>) -> ReleaseFailure {
    ReleaseFailure::new(
        Diagnostic::error(
            DiagnosticCode::ReleaseInput,
            DiagnosticStage::ReleaseInput,
            message,
        )
        .with_context(DiagnosticContext {
            target: Some(path.display().to_string()),
            ..DiagnosticContext::default()
        })
        .with_hint(
            "discard the extracted tree and extract one fully verified release archive again",
        ),
    )
}

fn publication_input(path: &Path, message: impl Into<String>) -> ReleaseFailure {
    publication_failure(
        path,
        CommitState::RolledBack,
        message,
        "choose a real writable prefix whose bin leaf is absent or a real directory",
    )
}

fn publication_io(path: &Path, error: &std::io::Error) -> ReleaseFailure {
    let class = publication_failure_class(error);
    let state = match class {
        PublicationFailureClass::CommitStateUncertain
        | PublicationFailureClass::RecoveryIncomplete => CommitState::Indeterminate,
        _ => CommitState::RolledBack,
    };
    let hint = match class {
        PublicationFailureClass::Conflict => {
            "do not overwrite the existing suite; use the documented update or uninstall workflow"
        }
        PublicationFailureClass::UnsafeTarget => {
            "use a real prefix and bin directory without symbolic links or special-file targets"
        }
        PublicationFailureClass::Unsupported => {
            "install on a supported Unix filesystem with no-follow rename and directory-fsync support"
        }
        PublicationFailureClass::RecoveryIncomplete
        | PublicationFailureClass::CommitStateUncertain => {
            "preserve the prefix and journal, then rerun the identical command to complete recovery"
        }
        PublicationFailureClass::Io => {
            "ensure the prefix is writable, stable, and has sufficient free space, then retry"
        }
    };
    publication_failure(
        path,
        state,
        format!("cannot install native suite: {error}"),
        hint,
    )
}

fn publication_failure(
    path: &Path,
    state: CommitState,
    message: impl Into<String>,
    hint: impl Into<String>,
) -> ReleaseFailure {
    ReleaseFailure::new(
        Diagnostic::error(
            DiagnosticCode::ReleasePublication,
            DiagnosticStage::Publication,
            message,
        )
        .with_context(DiagnosticContext {
            output: Some(path.display().to_string()),
            commit_state: Some(state),
            ..DiagnosticContext::default()
        })
        .with_hint(hint),
    )
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;
    use std::sync::{Arc, Barrier};

    static FAULT_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn source(root: &Path, marker: &str) -> PathBuf {
        let bin = root.join(format!("source-{marker}"));
        fs::create_dir(&bin).unwrap();
        for name in BINARIES {
            let path = bin.join(name);
            fs::write(&path, format!("{marker}:{name}\n")).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        bin
    }

    const fn args(source_bin: PathBuf, prefix: PathBuf) -> InstallArgs {
        InstallArgs { source_bin, prefix }
    }

    fn assert_suite(prefix: &Path, marker: &str) {
        let bin = prefix.join("bin");
        for name in BINARIES {
            assert_eq!(
                fs::read_to_string(bin.join(name)).unwrap(),
                format!("{marker}:{name}\n")
            );
            assert_eq!(
                fs::metadata(bin.join(name)).unwrap().permissions().mode() & 0o777,
                0o755
            );
        }
    }

    #[test]
    fn installs_exact_suite_and_refuses_partial_or_invalid_inputs() {
        let root = tempfile::tempdir().unwrap();
        let valid = source(root.path(), "valid");
        let prefix = root.path().join("prefix");
        fs::create_dir(&prefix).unwrap();
        install(&args(valid.clone(), prefix.clone())).unwrap();
        assert_suite(&prefix, "valid");

        let partial_prefix = root.path().join("partial-prefix");
        fs::create_dir_all(partial_prefix.join("bin")).unwrap();
        fs::write(partial_prefix.join("bin/aros"), b"owned").unwrap();
        assert!(install(&args(valid, partial_prefix.clone())).is_err());
        assert_eq!(fs::read(partial_prefix.join("bin/aros")).unwrap(), b"owned");
        for name in &BINARIES[1..] {
            assert!(!partial_prefix.join("bin").join(name).exists());
        }

        let extra = source(root.path(), "extra");
        fs::write(extra.join("unexpected"), b"no").unwrap();
        let extra_prefix = root.path().join("extra-prefix");
        fs::create_dir(&extra_prefix).unwrap();
        assert!(install(&args(extra, extra_prefix)).is_err());

        let missing = source(root.path(), "missing");
        fs::remove_file(missing.join("aros-verify")).unwrap();
        let missing_prefix = root.path().join("missing-prefix");
        fs::create_dir(&missing_prefix).unwrap();
        assert!(install(&args(missing, missing_prefix)).is_err());
    }

    #[test]
    fn mid_apply_failure_rolls_back_every_binary_and_retry_recovers() {
        let _environment = FAULT_ENV_LOCK.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        let source = source(root.path(), "fault");
        let prefix = root.path().join("prefix");
        fs::create_dir(&prefix).unwrap();
        let point = format!("after-apply-2@{}", std::thread::current().name().unwrap());
        std::env::set_var("AROS_PUBLICATION_TEST_FAIL_AT", point);
        let result = install(&args(source.clone(), prefix.clone()));
        std::env::remove_var("AROS_PUBLICATION_TEST_FAIL_AT");
        assert!(result.is_err());
        for name in BINARIES {
            assert!(!prefix.join("bin").join(name).exists());
        }
        install(&args(source, prefix.clone())).unwrap();
        assert_suite(&prefix, "fault");
    }

    #[test]
    fn racing_installers_leave_one_complete_unmixed_suite() {
        let root = tempfile::tempdir().unwrap();
        let prefix = root.path().join("prefix");
        fs::create_dir(&prefix).unwrap();
        let first = source(root.path(), "first");
        let second = source(root.path(), "second");
        let barrier = Arc::new(Barrier::new(3));
        // Both threads must be spawned before the main test thread crosses the
        // three-party barrier, so this collection is intentional.
        #[allow(clippy::needless_collect)]
        let workers: Vec<_> = [(first, "first"), (second, "second")]
            .into_iter()
            .map(|(source_bin, marker)| {
                let prefix = prefix.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    (marker, install(&args(source_bin, prefix)).is_ok())
                })
            })
            .collect();
        barrier.wait();
        let results: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        assert_eq!(results.iter().filter(|(_, success)| *success).count(), 1);
        let winner = results
            .iter()
            .find_map(|(marker, success)| success.then_some(*marker))
            .unwrap();
        assert_suite(&prefix, winner);
    }
}
