//! Private Rust bridge for source-owned MetaMake `%fetch` invocations.
//!
//! This hidden command resolves exactly one source-lock payload, then executes
//! the same validated offline fetch contract as `aros-fetch`. The upstream
//! `fetch.sh` is measured only to anchor local patch-origin containment; it is
//! never executed.

use std::env;
use std::ffi::{OsStr, OsString};
use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use aros_common::{open_regular_file_nofollow, sha256_file, sha256_reader, Sha256Digest};
use aros_fetch::contract::{normalize_legacy_arguments, Cli, FetchRequest};
use aros_fetch::engine;
use aros_fetch::observability::{LogFormat, LogLevel, Logger};
use aros_toolchain::metamake_fetch::{
    MetaMakeFetchInvocation, ResolvedMetaMakeSource, SourceUseLedger,
};
use aros_toolchain::source_lock::SourceLock;
use clap::Parser;

/// Arguments forwarded by the source-owned MetaMake rule.
#[derive(clap::Args)]
pub struct MetaMakeFetchArgs {
    /// Opaque source-owned `fetch.sh` arguments.
    #[arg(allow_hyphen_values = true, trailing_var_arg = true)]
    pub arguments: Vec<OsString>,
}

/// Execute one internal, verified MetaMake fetch request.
pub async fn run(args: &MetaMakeFetchArgs) -> miette::Result<()> {
    let lock = SourceLock::parse(&read_environment_file("AROS_TOOLCHAIN_FETCH_LOCK")?)
        .map_err(|error| miette::miette!("native MetaMake fetch bridge: {error}"))?;
    let checksum_was_supplied = reject_duplicate_checksums(&args.arguments)?;
    let invocation = MetaMakeFetchInvocation::parse_for_lock(&args.arguments, &lock)
        .map_err(|error| miette::miette!("native MetaMake fetch bridge: {error}"))?;
    let cache = environment_path("AROS_TOOLCHAIN_FETCH_CACHE")?;
    let ledger = SourceUseLedger::open(&environment_path("AROS_TOOLCHAIN_FETCH_LEDGER")?)
        .map_err(|error| miette::miette!("native MetaMake fetch bridge: {error}"))?;
    let source = invocation
        .resolve(&lock, &cache)
        .map_err(|error| miette::miette!("native MetaMake fetch bridge: {error}"))?;
    let private_cache = materialize_private_source(&source, &ledger)?;
    let anchor = source_snapshot_anchor()?;
    let request = fetch_request(&source, &private_cache, &anchor, checksum_was_supplied)?;

    let mut logger = Logger::open(LogLevel::Off, LogFormat::Human, None)
        .map_err(|error| miette::miette!("native MetaMake fetch bridge: {error}"))?;
    engine::run(&request, &mut logger)
        .await
        .map_err(|error| miette::miette!("native MetaMake fetch bridge: {error}"))?;

    source
        .revalidate()
        .map_err(|error| miette::miette!("native MetaMake fetch bridge: {error}"))?;
    revalidate_private_source(&source, &private_cache)?;
    anchor.revalidate()?;
    ledger
        .record(&source)
        .map_err(|error| miette::miette!("native MetaMake fetch bridge: {error}"))?;
    Ok(())
}

fn fetch_request(
    source: &ResolvedMetaMakeSource,
    private_cache: &tempfile::TempDir,
    anchor: &SourceSnapshotAnchor,
    checksum_was_supplied: bool,
) -> miette::Result<FetchRequest> {
    let mut arguments = vec![OsString::from("aros-fetch")];
    arguments.extend(normalize_legacy_arguments(source.arguments().to_vec()));
    let mut cli = Cli::try_parse_from(arguments)
        .map_err(|error| miette::miette!("native MetaMake fetch bridge: {error}"))?;

    let lock_checksum = format!("{}=sha256:{}", source.filename(), source.sha256());
    if checksum_was_supplied
        && cli.checksums.split_ascii_whitespace().collect::<Vec<_>>() != [lock_checksum.as_str()]
    {
        return Err(miette::miette!(
            "native MetaMake fetch bridge: source checksum differs from the selected source lock"
        ));
    }
    cli.checksums = lock_checksum;

    let cache_path = private_cache.path();
    let cache_origin = cache_path.to_str().ok_or_else(|| {
        miette::miette!("native MetaMake fetch bridge: private cache path is not UTF-8")
    })?;
    if cache_origin.chars().any(char::is_whitespace) {
        return Err(miette::miette!(
            "native MetaMake fetch bridge: private cache path contains unsupported whitespace"
        ));
    }
    cache_origin.clone_into(&mut cli.archive_origins);
    cli.location = cache_path.to_path_buf();
    cli.patch_origins = if has_patch_origins_option(source.arguments()) {
        contained_patch_origins(&cli.patch_origins, &anchor.root)?
    } else {
        path_as_origin(&anchor.root)?
    };
    // Clap reads these two fields from the ambient environment. Override both
    // after parsing so the typed engine request always remains local/strict.
    cli.offline = true;
    cli.require_checksums = true;

    let request = FetchRequest::from_cli(&cli)
        .map_err(|error| miette::miette!("native MetaMake fetch bridge: {error}"))?;
    if request.archive_candidates.as_slice() != [source.filename()] {
        return Err(miette::miette!(
            "native MetaMake fetch bridge: parsed archive differs from the unique locked suffix"
        ));
    }
    Ok(request)
}

fn materialize_private_source(
    source: &ResolvedMetaMakeSource,
    ledger: &SourceUseLedger,
) -> miette::Result<tempfile::TempDir> {
    source
        .revalidate()
        .map_err(|error| miette::miette!("native MetaMake fetch bridge: {error}"))?;
    let parent = ledger.path().parent().ok_or_else(|| {
        miette::miette!("native MetaMake fetch bridge: source ledger has no owned parent")
    })?;
    let directory = tempfile::Builder::new()
        .prefix("metamake-source-")
        .tempdir_in(parent)
        .map_err(|_| {
            miette::miette!("native MetaMake fetch bridge: cannot stage private source")
        })?;
    let destination = directory.path().join(source.filename());
    let mut input = open_regular_file_nofollow(source.snapshot_path())
        .map_err(|_| miette::miette!("native MetaMake fetch bridge: private snapshot is unsafe"))?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o400)
        .open(&destination)
        .map_err(|_| {
            miette::miette!("native MetaMake fetch bridge: cannot create private payload")
        })?;
    io::copy(&mut input, &mut output)
        .and_then(|_| output.sync_all())
        .map_err(|_| {
            miette::miette!("native MetaMake fetch bridge: cannot seal private payload")
        })?;
    let measured = sha256_file(&destination).map_err(|_| {
        miette::miette!("native MetaMake fetch bridge: cannot verify staged source")
    })?;
    if measured.size != source.size() || &measured.digest != source.sha256() {
        return Err(miette::miette!(
            "native MetaMake fetch bridge: staged source differs from the selected lock"
        ));
    }
    Ok(directory)
}

fn revalidate_private_source(
    source: &ResolvedMetaMakeSource,
    private_cache: &tempfile::TempDir,
) -> miette::Result<()> {
    let path = private_cache.path().join(source.filename());
    let mut file = open_regular_file_nofollow(&path).map_err(|_| {
        miette::miette!("native MetaMake fetch bridge: staged source is no longer safe")
    })?;
    let measured = sha256_reader(&mut file).map_err(|_| {
        miette::miette!("native MetaMake fetch bridge: cannot revalidate staged source")
    })?;
    if measured.size != source.size() || &measured.digest != source.sha256() {
        return Err(miette::miette!(
            "native MetaMake fetch bridge: staged source changed during fetch"
        ));
    }
    Ok(())
}

fn reject_duplicate_checksums(arguments: &[OsString]) -> miette::Result<bool> {
    let mut occurrences = 0_usize;
    let mut has_nonempty_declaration = false;
    let mut index = 0_usize;
    while let Some(argument) = arguments.get(index).and_then(|value| value.to_str()) {
        if argument == "-cs" || argument == "--checksums" {
            occurrences += 1;
            if let Some(value) = arguments.get(index + 1).and_then(|value| value.to_str()) {
                has_nonempty_declaration |= !value.bytes().all(|byte| byte.is_ascii_whitespace());
            }
            index = index.saturating_add(2);
            continue;
        }
        if let Some(value) = argument.strip_prefix("--checksums=") {
            occurrences += 1;
            has_nonempty_declaration |= !value.bytes().all(|byte| byte.is_ascii_whitespace());
        }
        // Skip values of the legacy two-token options so a value resembling
        // `-cs` is not misclassified as another option.
        if matches!(
            argument,
            "-ao" | "-a" | "-s" | "-d" | "-po" | "-p" | "-b" | "-l" | "-rn"
        ) {
            index = index.saturating_add(2);
        } else {
            index = index.saturating_add(1);
        }
    }
    if occurrences > 1 {
        return Err(miette::miette!(
            "native MetaMake fetch bridge: repeated -cs/--checksums options are not allowed"
        ));
    }
    Ok(has_nonempty_declaration)
}

fn contained_patch_origins(origins: &str, source_root: &Path) -> miette::Result<String> {
    let current_directory = env::current_dir().map_err(|_| {
        miette::miette!("native MetaMake fetch bridge: cannot resolve patch-origin base")
    })?;
    let mut validated = Vec::new();
    for origin in origins.split_ascii_whitespace() {
        if origin.contains("://") {
            return Err(miette::miette!(
                "native MetaMake fetch bridge: patch origins must be local to the source snapshot"
            ));
        }
        let path = PathBuf::from(origin);
        let path = if path.is_absolute() {
            path
        } else {
            current_directory.join(path)
        };
        let canonical = path.canonicalize().map_err(|_| {
            miette::miette!("native MetaMake fetch bridge: patch origin is unavailable")
        })?;
        if !canonical.is_dir() || !canonical.starts_with(source_root) {
            return Err(miette::miette!(
                "native MetaMake fetch bridge: patch origin escapes the source snapshot"
            ));
        }
        let canonical = canonical.to_str().ok_or_else(|| {
            miette::miette!("native MetaMake fetch bridge: patch origin is not UTF-8")
        })?;
        if canonical.chars().any(char::is_whitespace) {
            return Err(miette::miette!(
                "native MetaMake fetch bridge: patch origin contains unsupported whitespace"
            ));
        }
        validated.push(canonical.to_owned());
    }
    if validated.is_empty() {
        return Err(miette::miette!(
            "native MetaMake fetch bridge: patch origins are empty"
        ));
    }
    Ok(validated.join(" "))
}

fn has_patch_origins_option(arguments: &[OsString]) -> bool {
    let mut index = 0;
    while let Some(argument) = arguments.get(index).and_then(|value| value.to_str()) {
        if matches!(argument, "-po" | "--patch-origins") || argument.starts_with("--patch-origins=")
        {
            return true;
        }
        if matches!(
            argument,
            "-ao" | "-a" | "-s" | "-d" | "-po" | "-p" | "-b" | "-l" | "-rn" | "-cs"
        ) {
            index = index.saturating_add(2);
        } else {
            index = index.saturating_add(1);
        }
    }
    false
}

fn path_as_origin(path: &Path) -> miette::Result<String> {
    let value = path
        .to_str()
        .ok_or_else(|| miette::miette!("native MetaMake fetch bridge: source path is not UTF-8"))?;
    if value.chars().any(char::is_whitespace) {
        return Err(miette::miette!(
            "native MetaMake fetch bridge: source path contains unsupported whitespace"
        ));
    }
    Ok(value.to_owned())
}

struct SourceSnapshotAnchor {
    script: PathBuf,
    root: PathBuf,
    size: u64,
    digest: Sha256Digest,
}

impl SourceSnapshotAnchor {
    fn revalidate(&self) -> miette::Result<()> {
        let measured = measure_fetch_script(&self.script)?;
        if measured.size != self.size || measured.digest != self.digest {
            return Err(miette::miette!(
                "native MetaMake fetch bridge: source snapshot anchor changed during fetch"
            ));
        }
        Ok(())
    }
}

fn source_snapshot_anchor() -> miette::Result<SourceSnapshotAnchor> {
    let requested_script = environment_path("AROS_TOOLCHAIN_FETCH_UPSTREAM")?;
    let script = requested_script.canonicalize().map_err(|_| {
        miette::miette!("native MetaMake fetch bridge: source snapshot anchor is unavailable")
    })?;
    if script.file_name().and_then(OsStr::to_str) != Some("fetch.sh")
        || script
            .parent()
            .and_then(Path::file_name)
            .and_then(OsStr::to_str)
            != Some("scripts")
    {
        return Err(miette::miette!(
            "native MetaMake fetch bridge: source snapshot anchor must be canonical source/scripts/fetch.sh"
        ));
    }
    let root = script
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| {
            miette::miette!("native MetaMake fetch bridge: source snapshot root is unavailable")
        })?
        .to_path_buf();
    if !root.is_dir() {
        return Err(miette::miette!(
            "native MetaMake fetch bridge: source snapshot root is unavailable"
        ));
    }
    let measured = measure_fetch_script(&script)?;
    Ok(SourceSnapshotAnchor {
        script,
        root,
        size: measured.size,
        digest: measured.digest,
    })
}

fn measure_fetch_script(path: &Path) -> miette::Result<aros_common::Sha256Result> {
    let metadata = fs::symlink_metadata(path).map_err(|_| {
        miette::miette!("native MetaMake fetch bridge: source fetch script is unavailable")
    })?;
    if !metadata.file_type().is_file() {
        return Err(miette::miette!(
            "native MetaMake fetch bridge: source fetch script is not a regular file"
        ));
    }
    let mut file = open_regular_file_nofollow(path).map_err(|_| {
        miette::miette!("native MetaMake fetch bridge: source fetch script is unsafe")
    })?;
    sha256_reader(&mut file).map_err(|_| {
        miette::miette!("native MetaMake fetch bridge: cannot measure source fetch script")
    })
}

fn environment_path(name: &str) -> miette::Result<PathBuf> {
    let value = env::var_os(name)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            miette::miette!(
                "native MetaMake fetch bridge: required controlled environment is absent"
            )
        })?;
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err(miette::miette!(
            "native MetaMake fetch bridge: controlled path is not absolute"
        ));
    }
    Ok(path)
}

fn read_environment_file(name: &str) -> miette::Result<Vec<u8>> {
    let path = environment_path(name)?;
    let metadata = fs::symlink_metadata(&path).map_err(|_| {
        miette::miette!("native MetaMake fetch bridge: controlled file is unavailable")
    })?;
    if !metadata.file_type().is_file() {
        return Err(miette::miette!(
            "native MetaMake fetch bridge: controlled file is not regular"
        ));
    }
    let file = open_regular_file_nofollow(&path)
        .map_err(|_| miette::miette!("native MetaMake fetch bridge: controlled file is unsafe"))?;
    let mut bytes = Vec::new();
    file.take((aros_toolchain::canonical::MAX_DOCUMENT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| {
            miette::miette!("native MetaMake fetch bridge: cannot read controlled file")
        })?;
    if bytes.len() > aros_toolchain::canonical::MAX_DOCUMENT_BYTES {
        return Err(miette::miette!(
            "native MetaMake fetch bridge: controlled source lock is too large"
        ));
    }
    Ok(bytes)
}
