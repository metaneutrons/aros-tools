//! Fail-closed transport, integrity, extraction, and patch execution.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use aros_common::{
    run_status, sha256_file, Diagnostic, DiagnosticCode, DiagnosticContext, DiagnosticStage,
    LogLevel, Sha256Digest,
};
use bzip2::read::BzDecoder;
use flate2::read::GzDecoder;
use fs2::FileExt;
use futures_util::StreamExt;
use reqwest::redirect::Policy;
use tempfile::{Builder, TempDir};
use xz2::read::XzDecoder;

use crate::contract::{FetchRequest, PatchSpec};
use crate::observability::Logger;
use crate::{FetchFailure, FetchResult};

const LOCK_TIMEOUT: Duration = Duration::from_mins(5);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const TRANSFER_TIMEOUT: Duration = Duration::from_mins(15);
const RETRIES: usize = 3;
const MAX_DOWNLOAD_BYTES: u64 = 8 * 1024 * 1024 * 1024;

/// Execute one validated fetch contract to completion.
///
/// # Errors
///
/// Returns one stable diagnostic when cache locking, transport, integrity
/// validation, safe extraction, publication, or patch application fails.
pub async fn run(request: &FetchRequest, logger: &mut Logger) -> FetchResult<()> {
    create_directory(&request.location, "archive cache")?;
    create_directory(&request.destination, "archive destination")?;
    create_directory(&request.base, "patch cache")?;
    let _lock = FetchLock::acquire(&request.location, &request.archive)?;

    let context = context(request, None);
    logger.event(
        LogLevel::Info,
        "cache.locked",
        "exclusive fetch cache lock acquired",
        &context,
    )?;

    let client = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(TRANSFER_TIMEOUT)
        .redirect(Policy::limited(10))
        .user_agent(concat!("aros-fetch/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| network_failure(format!("cannot initialize HTTPS client: {error}")))?;

    let archive = fetch_candidates(
        &client,
        &request.archive_candidates,
        &request.archive_origins,
        &request.location,
        request,
        logger,
    )
    .await?;

    for patch in &request.patches {
        fetch_patch(&client, patch, request, logger).await?;
    }

    if request.archive_candidates.len() > 1 || archive != request.archive {
        extract_cached(
            &request.location.join(&archive),
            &archive,
            &request.destination,
            &request.base,
            request.force,
            logger,
        )?;
    }

    for patch in &request.patches {
        apply_patch(patch, request, logger)?;
    }
    Ok(())
}

async fn fetch_patch(
    client: &reqwest::Client,
    patch: &PatchSpec,
    request: &FetchRequest,
    logger: &mut Logger,
) -> FetchResult<()> {
    let mut candidates = vec![patch.name.clone()];
    candidates.extend(
        ["tar.bz2", "tar.gz", "zip"]
            .iter()
            .map(|suffix| format!("{}.{suffix}", patch.name)),
    );
    if request
        .checksums
        .keys()
        .any(|name| candidates.contains(name))
    {
        candidates.retain(|name| request.checksums.contains_key(name));
    }
    let fetched = fetch_candidates(
        client,
        &candidates,
        &request.patch_origins,
        &request.base,
        request,
        logger,
    )
    .await?;
    if fetched != patch.name {
        extract_cached(
            &request.base.join(&fetched),
            &fetched,
            &request.destination,
            &request.base,
            request.force,
            logger,
        )?;
        if !request.base.join(&patch.name).is_file() {
            return Err(extraction_failure(
                &fetched,
                format!(
                    "compressed patch payload did not produce expected patch '{}' in {}",
                    patch.name,
                    request.base.display()
                ),
            ));
        }
    }
    Ok(())
}

async fn fetch_candidates(
    client: &reqwest::Client,
    candidates: &[String],
    origins: &[String],
    cache: &Path,
    request: &FetchRequest,
    logger: &mut Logger,
) -> FetchResult<String> {
    for candidate in candidates {
        let cached = cache.join(candidate);
        if is_regular_file(&cached)? {
            verify(candidate, &cached, request.checksums.get(candidate), logger)?;
            return Ok(candidate.clone());
        }
    }

    let mut attempts = Vec::new();
    for candidate in candidates {
        for (origin_index, origin) in origins.iter().enumerate() {
            for source in expand_origin(origin, candidate)? {
                if request.offline && !matches!(&source, Source::Local(_)) {
                    continue;
                }
                println!(
                    "Trying     {candidate} from declared origin {}...",
                    origin_index + 1
                );
                let event_context = DiagnosticContext {
                    mode: Some(source.kind().to_owned()),
                    output: Some(candidate.clone()),
                    ..DiagnosticContext::default()
                };
                logger.event(
                    LogLevel::Debug,
                    "transfer.attempt",
                    "declared payload transfer attempted",
                    &event_context,
                )?;
                match transfer(client, &source, cache, candidate).await {
                    Ok(()) => {
                        let path = cache.join(candidate);
                        if let Err(error) =
                            verify(candidate, &path, request.checksums.get(candidate), logger)
                        {
                            let _ = fs::remove_file(&path);
                            return Err(error);
                        }
                        logger.event(
                            LogLevel::Info,
                            "transfer.complete",
                            "declared payload transfer completed",
                            &event_context,
                        )?;
                        return Ok(candidate.clone());
                    }
                    Err(error) => attempts.push(error.diagnostic().message.clone()),
                }
            }
        }
    }
    let detail = attempts
        .last()
        .map_or("no declared origin was usable", String::as_str);
    if request.offline {
        return Err(cache_failure(format!(
            "offline cache/local-origin miss for candidates {} in '{}': {detail}",
            candidates.join(", "),
            cache.display()
        ))
        .with_hint("seed the cache from a verified declared payload or rerun without --offline"));
    }
    Err(network_failure(format!(
        "could not fetch any candidate for '{}': {detail}",
        request.archive
    ))
    .with_hint("check the declared origins and network access; use --diagnostic-format json for automation"))
}

#[derive(Debug)]
enum Source {
    Http(String),
    Ftp(String),
    Local(PathBuf),
}

impl Source {
    const fn kind(&self) -> &'static str {
        match self {
            Self::Http(_) => "https/http",
            Self::Ftp(_) => "ftp",
            Self::Local(_) => "local",
        }
    }
}

fn expand_origin(origin: &str, candidate: &str) -> FetchResult<Vec<Source>> {
    let append = |base: &str, path: &str, name: &str| {
        if path.trim_matches('/').is_empty() {
            format!("{}/{}", base.trim_end_matches('/'), name)
        } else {
            format!(
                "{}/{}/{}",
                base.trim_end_matches('/'),
                path.trim_matches('/'),
                name
            )
        }
    };
    let direct = |base: &str| format!("{}/{}", base.trim_end_matches('/'), candidate);
    if let Some(path) = origin.strip_prefix("cache://") {
        validate_origin_path(path, origin)?;
        return Ok(vec![Source::Http(append(
            "https://github.com/aros-development-team/external-sources/raw/refs/heads/main",
            path,
            candidate,
        ))]);
    }
    if let Some(path) = origin.strip_prefix("gnu://") {
        validate_origin_path(path, origin)?;
        return Ok(["https://ftpmirror.gnu.org", "https://ftp.gnu.org/pub/gnu"]
            .iter()
            .map(|base| Source::Http(append(base, path, candidate)))
            .collect());
    }
    if let Some(path) = origin.strip_prefix("archives://") {
        validate_origin_path(path, origin)?;
        return Ok([
            "https://archives.aros-exec.org",
            "https://arosarchives.os4depot.net",
        ]
        .iter()
        .map(|base| Source::Http(append(base, path, candidate)))
        .collect());
    }
    if let Some(path) = origin
        .strip_prefix("sf://")
        .or_else(|| origin.strip_prefix("sourceforge://"))
    {
        validate_origin_path(path, origin)?;
        return Ok(vec![Source::Http(append(
            "https://downloads.sourceforge.net",
            path,
            candidate,
        ))]);
    }
    if let Some(path) = origin.strip_prefix("github://") {
        validate_origin_path(path, origin)?;
        return Ok(vec![Source::Http(append(
            "https://github.com",
            path,
            candidate,
        ))]);
    }
    if origin.starts_with("https://") || origin.starts_with("http://") {
        return Ok(vec![Source::Http(validate_remote_url(
            &direct(origin),
            &["http", "https"],
        )?)]);
    }
    if origin.starts_with("ftp://") {
        return Ok(vec![Source::Ftp(validate_remote_url(
            &direct(origin),
            &["ftp"],
        )?)]);
    }
    if origin.contains("://") {
        return Err(contract_failure(format!(
            "unsupported origin scheme in '{origin}'"
        )));
    }
    Ok(vec![Source::Local(PathBuf::from(origin).join(candidate))])
}

fn validate_origin_path(path: &str, origin: &str) -> FetchResult<()> {
    if path.contains(['\\', '?', '#', '%'])
        || path
            .split('/')
            .any(|component| matches!(component, "." | ".."))
    {
        return Err(contract_failure(format!(
            "special origin '{origin}' contains an unsafe or ambiguous path"
        )));
    }
    Ok(())
}

fn validate_remote_url(url: &str, schemes: &[&str]) -> FetchResult<String> {
    let parsed = reqwest::Url::parse(url)
        .map_err(|error| contract_failure(format!("invalid remote origin URL: {error}")))?;
    if !schemes.contains(&parsed.scheme()) || parsed.host_str().is_none() {
        return Err(contract_failure(
            "remote origin has no supported scheme and host",
        ));
    }
    if !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(contract_failure(
            "remote origin must not contain credentials, a query, or a fragment",
        ));
    }
    Ok(parsed.to_string())
}

async fn transfer(
    client: &reqwest::Client,
    source: &Source,
    cache: &Path,
    candidate: &str,
) -> FetchResult<()> {
    let temporary = cache.join(format!(".{candidate}.part.{}", std::process::id()));
    if fs::symlink_metadata(&temporary).is_ok() {
        fs::remove_file(&temporary).map_err(|error| {
            cache_failure(format!(
                "cannot remove stale staging path '{}': {error}",
                temporary.display()
            ))
        })?;
    }
    let result = match source {
        Source::Http(url) => download_http(client, url, &temporary).await,
        Source::Ftp(url) => download_ftp(url, &temporary),
        Source::Local(path) => copy_local(path, &temporary),
    };
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    let destination = cache.join(candidate);
    fs::rename(&temporary, &destination).map_err(|error| {
        cache_failure(format!(
            "cannot publish fetched payload '{}': {error}",
            destination.display()
        ))
    })
}

async fn download_http(client: &reqwest::Client, url: &str, output: &Path) -> FetchResult<()> {
    let mut last = None;
    for _ in 0..RETRIES {
        match client.get(url).send().await {
            Ok(response) => {
                if !response.status().is_success() {
                    last = Some(format!("HTTP server returned status {}", response.status()));
                    continue;
                }
                if response
                    .content_length()
                    .is_some_and(|size| size > MAX_DOWNLOAD_BYTES)
                {
                    return Err(network_failure(
                        "declared payload exceeds the 8 GiB safety limit",
                    ));
                }
                let mut file = new_temporary(output)?;
                let mut size = 0_u64;
                let mut stream = response.bytes_stream();
                let mut stream_error = None;
                while let Some(chunk) = stream.next().await {
                    let chunk = match chunk {
                        Ok(chunk) => chunk,
                        Err(error) => {
                            stream_error = Some(http_error_summary(&error));
                            break;
                        }
                    };
                    size = size
                        .checked_add(chunk.len() as u64)
                        .ok_or_else(|| network_failure("payload size overflow"))?;
                    if size > MAX_DOWNLOAD_BYTES {
                        return Err(network_failure(
                            "declared payload exceeds the 8 GiB safety limit",
                        ));
                    }
                    file.write_all(&chunk).map_err(|error| {
                        cache_failure(format!("cannot write download staging file: {error}"))
                    })?;
                }
                if let Some(error) = stream_error {
                    drop(file);
                    let _ = fs::remove_file(output);
                    last = Some(format!("HTTP response stream failed: {error}"));
                    continue;
                }
                file.sync_all().map_err(|error| {
                    cache_failure(format!("cannot sync download staging file: {error}"))
                })?;
                return Ok(());
            }
            Err(error) => last = Some(http_error_summary(&error)),
        }
    }
    Err(network_failure(format!(
        "HTTP transfer failed after {RETRIES} attempts: {}",
        last.unwrap_or_else(|| "unknown transport error".to_owned())
    )))
}

fn http_error_summary(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        "HTTP request timed out".to_owned()
    } else if error.is_connect() {
        "HTTP connection failed".to_owned()
    } else if error.is_redirect() {
        "HTTP redirect policy rejected the response".to_owned()
    } else if error.is_decode() {
        "HTTP response decoding failed".to_owned()
    } else {
        "HTTP transport failed".to_owned()
    }
}

fn download_ftp(url: &str, output: &Path) -> FetchResult<()> {
    let completed = run_status(
        Command::new("curl")
            .args([
                "--fail",
                "--location",
                "--retry",
                "3",
                "--retry-connrefused",
                "--connect-timeout",
                "20",
                "--max-time",
                "900",
                "--output",
            ])
            .arg(output)
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null()),
    )
    .map_err(|error| {
        network_failure(format!(
            "FTP requires curl, but the executable could not be started: {error}"
        ))
    })?;
    if completed.status.success() {
        let metadata = fs::symlink_metadata(output).map_err(|error| {
            network_failure(format!("cannot inspect completed FTP payload: {error}"))
        })?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(network_failure(
                "FTP client did not produce a regular payload file",
            ));
        }
        if metadata.len() > MAX_DOWNLOAD_BYTES {
            return Err(network_failure(
                "declared payload exceeds the 8 GiB safety limit",
            ));
        }
        Ok(())
    } else {
        Err(network_failure(format!(
            "FTP transfer failed with status {}; no insecure TLS fallback was attempted",
            completed.status
        )))
    }
}

fn copy_local(source: &Path, output: &Path) -> FetchResult<()> {
    let metadata = fs::symlink_metadata(source).map_err(|error| {
        network_failure(format!(
            "local payload '{}' is unavailable: {error}",
            source.display()
        ))
    })?;
    if !metadata.file_type().is_file() {
        return Err(network_failure(format!(
            "local payload '{}' is not a regular file",
            source.display()
        )));
    }
    let mut input = File::open(source).map_err(|error| {
        network_failure(format!(
            "cannot open local payload '{}': {error}",
            source.display()
        ))
    })?;
    let mut destination = new_temporary(output)?;
    io::copy(&mut input, &mut destination)
        .map_err(|error| cache_failure(format!("cannot stage local payload: {error}")))?;
    destination
        .sync_all()
        .map_err(|error| cache_failure(format!("cannot sync local payload: {error}")))
}

fn new_temporary(path: &Path) -> FetchResult<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            cache_failure(format!(
                "cannot create staging file '{}': {error}",
                path.display()
            ))
        })
}

fn verify(
    candidate: &str,
    path: &Path,
    expected: Option<&Sha256Digest>,
    logger: &mut Logger,
) -> FetchResult<()> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let actual = sha256_file(path).map_err(|error| {
        integrity_failure(candidate, format!("cannot hash cached payload: {error}"))
    })?;
    if &actual.digest != expected {
        return Err(integrity_failure(
            candidate,
            format!(
                "SHA-256 mismatch: expected {expected}, actual {}",
                actual.digest
            ),
        )
        .with_hint("do not replace the declared digest until the upstream payload change has been independently verified"));
    }
    println!("Verified   {candidate} (SHA-256)");
    logger.event(
        LogLevel::Info,
        "integrity.verified",
        "declared SHA-256 verified",
        &DiagnosticContext {
            output: Some(candidate.to_owned()),
            ..DiagnosticContext::default()
        },
    )
}

fn extract_cached(
    archive: &Path,
    name: &str,
    destination: &Path,
    base: &Path,
    force: bool,
    logger: &mut Logger,
) -> FetchResult<()> {
    let marker = base.join(format!(".{name}.unpacked"));
    if is_regular_file(&marker)? && !force {
        return Ok(());
    }
    println!("Unpacking  `{name}`...");
    let parent = destination.parent().unwrap_or(destination);
    create_directory(parent, "extraction staging parent")?;
    let staging = Builder::new()
        .prefix(".aros-fetch-extract-")
        .tempdir_in(parent)
        .map_err(|error| {
            extraction_failure(name, format!("cannot create staging directory: {error}"))
        })?;
    unpack_archive(archive, name, staging.path())?;
    publish_tree(&staging, destination, force, name)?;
    write_marker(&marker, "unpacked")?;
    logger.event(
        LogLevel::Info,
        "archive.extracted",
        "archive safely extracted and published",
        &DiagnosticContext {
            output: Some(name.to_owned()),
            ..DiagnosticContext::default()
        },
    )
}

fn unpack_archive(archive: &Path, name: &str, staging: &Path) -> FetchResult<()> {
    let file = File::open(archive)
        .map_err(|error| extraction_failure(name, format!("cannot open archive: {error}")))?;
    if has_extensions(name, &["tar", "gz"]) || has_extensions(name, &["tgz"]) {
        unpack_tar(GzDecoder::new(BufReader::new(file)), name, staging)
    } else if has_extensions(name, &["tar", "bz2"]) {
        unpack_tar(BzDecoder::new(BufReader::new(file)), name, staging)
    } else if has_extensions(name, &["tar", "xz"]) {
        unpack_tar(XzDecoder::new(BufReader::new(file)), name, staging)
    } else if has_extensions(name, &["zip"]) {
        unpack_zip(file, name, staging)
    } else {
        Err(extraction_failure(
            name,
            "unsupported archive format; expected .tar.gz, .tgz, .tar.bz2, .tar.xz, or .zip",
        ))
    }
}

fn has_extensions(name: &str, extensions: &[&str]) -> bool {
    let mut path = Path::new(name);
    for expected in extensions.iter().rev() {
        let Some(actual) = path.extension().and_then(|value| value.to_str()) else {
            return false;
        };
        if !actual.eq_ignore_ascii_case(expected) {
            return false;
        }
        let Some(stem) = path.file_stem() else {
            return false;
        };
        path = Path::new(stem);
    }
    true
}

fn unpack_tar(reader: impl Read, name: &str, staging: &Path) -> FetchResult<()> {
    let mut archive = tar::Archive::new(reader);
    let entries = archive
        .entries()
        .map_err(|error| extraction_failure(name, format!("cannot read archive index: {error}")))?;
    let mut count = 0_usize;
    for entry in entries {
        let mut entry = entry.map_err(|error| {
            extraction_failure(name, format!("cannot read archive entry: {error}"))
        })?;
        let path = entry
            .path()
            .map_err(|error| extraction_failure(name, format!("invalid entry path: {error}")))?
            .into_owned();
        validate_archive_path(&path, name)?;
        if let Some(link) = entry
            .link_name()
            .map_err(|error| extraction_failure(name, format!("invalid link target: {error}")))?
        {
            validate_link_target(&path, &link, name)?;
        }
        if !entry.unpack_in(staging).map_err(|error| {
            extraction_failure(
                name,
                format!("cannot extract '{}': {error}", path.display()),
            )
        })? {
            return Err(extraction_failure(
                name,
                format!("entry '{}' escapes the extraction root", path.display()),
            ));
        }
        count += 1;
    }
    if count == 0 {
        return Err(extraction_failure(name, "archive contains no entries"));
    }
    Ok(())
}

fn unpack_zip(file: File, name: &str, staging: &Path) -> FetchResult<()> {
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|error| extraction_failure(name, format!("cannot read ZIP directory: {error}")))?;
    if archive.is_empty() {
        return Err(extraction_failure(name, "archive contains no entries"));
    }
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| extraction_failure(name, format!("cannot read ZIP entry: {error}")))?;
        let enclosed = entry.enclosed_name().ok_or_else(|| {
            extraction_failure(
                name,
                format!("ZIP entry '{}' escapes the extraction root", entry.name()),
            )
        })?;
        validate_archive_path(&enclosed, name)?;
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170_000 == 0o120_000)
        {
            return Err(extraction_failure(
                name,
                format!(
                    "ZIP symlink '{}' is rejected by the safe extractor",
                    entry.name()
                ),
            ));
        }
        let output = staging.join(enclosed);
        if entry.is_dir() {
            fs::create_dir_all(&output).map_err(|error| {
                extraction_failure(
                    name,
                    format!("cannot create '{}': {error}", output.display()),
                )
            })?;
        } else {
            if let Some(parent) = output.parent() {
                fs::create_dir_all(parent).map_err(|error| {
                    extraction_failure(
                        name,
                        format!("cannot create '{}': {error}", parent.display()),
                    )
                })?;
            }
            let mut target = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&output)
                .map_err(|error| {
                    extraction_failure(
                        name,
                        format!("cannot create '{}': {error}", output.display()),
                    )
                })?;
            io::copy(&mut entry, &mut target).map_err(|error| {
                extraction_failure(
                    name,
                    format!("cannot extract '{}': {error}", output.display()),
                )
            })?;
        }
    }
    Ok(())
}

fn validate_archive_path(path: &Path, name: &str) -> FetchResult<()> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        return Err(extraction_failure(
            name,
            format!(
                "entry '{}' is not a contained relative path",
                path.display()
            ),
        ));
    }
    Ok(())
}

fn validate_link_target(entry: &Path, target: &Path, name: &str) -> FetchResult<()> {
    if target.is_absolute() {
        return Err(extraction_failure(
            name,
            format!(
                "link '{}' has absolute target '{}'",
                entry.display(),
                target.display()
            ),
        ));
    }
    let mut depth = entry.parent().map_or(0, |path| {
        path.components()
            .filter(|component| matches!(component, Component::Normal(_)))
            .count()
    });
    for component in target.components() {
        match component {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir if depth > 0 => depth -= 1,
            _ => {
                return Err(extraction_failure(
                    name,
                    format!(
                        "link '{}' escapes through target '{}'",
                        entry.display(),
                        target.display()
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn publish_tree(staging: &TempDir, destination: &Path, force: bool, name: &str) -> FetchResult<()> {
    create_directory(destination, "archive destination")?;
    let entries = fs::read_dir(staging.path()).map_err(|error| {
        extraction_failure(name, format!("cannot inspect staged archive: {error}"))
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            extraction_failure(name, format!("cannot inspect staged entry: {error}"))
        })?;
        let target = destination.join(entry.file_name());
        if fs::symlink_metadata(&target).is_ok() {
            if !force {
                return Err(extraction_failure(
                    name,
                    format!(
                        "refusing to replace existing destination '{}'; remove the stale tree or use --force explicitly",
                        target.display()
                    ),
                ));
            }
            remove_path(&target).map_err(|error| {
                publication_failure(
                    name,
                    format!("cannot replace '{}': {error}", target.display()),
                )
            })?;
        }
        fs::rename(entry.path(), &target).map_err(|error| {
            publication_failure(
                name,
                format!("cannot publish '{}': {error}", target.display()),
            )
        })?;
    }
    Ok(())
}

fn apply_patch(patch: &PatchSpec, request: &FetchRequest, logger: &mut Logger) -> FetchResult<()> {
    let marker = request.base.join(format!(".{}.applied", patch.name));
    if is_regular_file(&marker)? && !request.force {
        return Ok(());
    }
    let input_path = request.base.join(&patch.name);
    let input = File::open(&input_path).map_err(|error| {
        patch_failure(&patch.name, format!("cannot open patch payload: {error}"))
    })?;
    let directory = patch.subdirectory.as_deref().map_or_else(
        || request.destination.clone(),
        |path| request.destination.join(path),
    );
    let metadata = fs::symlink_metadata(&directory).map_err(|error| {
        patch_failure(
            &patch.name,
            format!(
                "patch directory '{}' is unavailable: {error}",
                directory.display()
            ),
        )
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(patch_failure(
            &patch.name,
            format!(
                "patch directory '{}' is not a real directory",
                directory.display()
            ),
        ));
    }
    let canonical_destination = request.destination.canonicalize().map_err(|error| {
        patch_failure(
            &patch.name,
            format!("cannot resolve patch destination: {error}"),
        )
    })?;
    let canonical_directory = directory.canonicalize().map_err(|error| {
        patch_failure(
            &patch.name,
            format!("cannot resolve patch directory: {error}"),
        )
    })?;
    if !canonical_directory.starts_with(&canonical_destination) {
        return Err(patch_failure(
            &patch.name,
            format!(
                "patch directory '{}' resolves outside destination '{}'",
                directory.display(),
                request.destination.display()
            ),
        ));
    }
    let completed = run_status(
        Command::new("patch")
            .args(["-Z", "-E"])
            .args(&patch.options)
            .current_dir(&directory)
            .stdin(Stdio::from(input)),
    )
    .map_err(|error| patch_failure(&patch.name, format!("cannot start patch tool: {error}")))?;
    if !completed.status.success() {
        return Err(patch_failure(
            &patch.name,
            format!("patch tool failed with status {}", completed.status),
        )
        .with_hint("inspect the patch context; a changed upstream source usually requires an explicit patch update"));
    }
    write_marker(&marker, "applied")?;
    logger.event(
        LogLevel::Info,
        "patch.applied",
        "declared patch applied",
        &DiagnosticContext {
            output: Some(patch.name.clone()),
            ..DiagnosticContext::default()
        },
    )
}

struct FetchLock {
    file: File,
}

impl FetchLock {
    fn acquire(cache: &Path, archive: &str) -> FetchResult<Self> {
        let path = cache.join(format!(".{archive}.fetch.lock"));
        if let Ok(metadata) = fs::symlink_metadata(&path) {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(cache_failure(format!(
                    "fetch lock '{}' is not a regular file",
                    path.display()
                )));
            }
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|error| {
                cache_failure(format!("cannot open lock '{}': {error}", path.display()))
            })?;
        let started = Instant::now();
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => return Ok(Self { file }),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    if started.elapsed() >= LOCK_TIMEOUT {
                        return Err(cache_failure(format!(
                            "timed out waiting for fetch lock '{}' after {} seconds",
                            path.display(),
                            LOCK_TIMEOUT.as_secs()
                        )));
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(error) => {
                    return Err(cache_failure(format!(
                        "cannot acquire fetch lock '{}': {error}",
                        path.display()
                    )));
                }
            }
        }
    }
}

impl Drop for FetchLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

fn create_directory(path: &Path, role: &str) -> FetchResult<()> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(cache_failure(format!(
                "{role} '{}' is not a real directory",
                path.display()
            )));
        }
        return Ok(());
    }
    fs::create_dir_all(path).map_err(|error| {
        cache_failure(format!(
            "cannot create {role} '{}': {error}",
            path.display()
        ))
    })
}

fn is_regular_file(path: &Path) -> FetchResult<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(cache_failure(format!(
            "cached payload '{}' is a symbolic link and is rejected",
            path.display()
        ))),
        Ok(metadata) if metadata.is_file() => Ok(true),
        Ok(_) => Err(cache_failure(format!(
            "cached payload '{}' is not a regular file",
            path.display()
        ))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(cache_failure(format!(
            "cannot inspect cached payload '{}': {error}",
            path.display()
        ))),
    }
}

fn write_marker(path: &Path, value: &str) -> FetchResult<()> {
    if fs::symlink_metadata(path).is_ok() {
        fs::remove_file(path).map_err(|error| {
            cache_failure(format!(
                "cannot replace marker '{}': {error}",
                path.display()
            ))
        })?;
    }
    let temporary = path.with_extension(format!("marker.{}", std::process::id()));
    let mut file = new_temporary(&temporary)?;
    writeln!(file, "{value}")
        .and_then(|()| file.sync_all())
        .map_err(|error| {
            cache_failure(format!("cannot write marker '{}': {error}", path.display()))
        })?;
    fs::rename(&temporary, path).map_err(|error| {
        cache_failure(format!(
            "cannot publish marker '{}': {error}",
            path.display()
        ))
    })
}

fn remove_path(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

fn context(request: &FetchRequest, output: Option<String>) -> DiagnosticContext {
    DiagnosticContext {
        mode: Some(if request.offline { "offline" } else { "online" }.into()),
        target: Some(request.destination.display().to_string()),
        output: output.or_else(|| Some(request.archive.clone())),
        ..DiagnosticContext::default()
    }
}

fn contract_failure(message: impl Into<String>) -> FetchFailure {
    failure(
        DiagnosticCode::FetchContract,
        DiagnosticStage::FetchContract,
        message,
    )
}

fn cache_failure(message: impl Into<String>) -> FetchFailure {
    failure(
        DiagnosticCode::FetchCache,
        DiagnosticStage::CacheOperation,
        message,
    )
}

fn network_failure(message: impl Into<String>) -> FetchFailure {
    failure(
        DiagnosticCode::FetchNetwork,
        DiagnosticStage::FetchTransfer,
        message,
    )
}

fn integrity_failure(name: &str, message: impl Into<String>) -> FetchFailure {
    FetchFailure::new(
        Diagnostic::error(
            DiagnosticCode::FetchIntegrity,
            DiagnosticStage::IntegrityValidation,
            message,
        )
        .with_context(DiagnosticContext {
            output: Some(name.to_owned()),
            ..DiagnosticContext::default()
        }),
    )
}

fn extraction_failure(name: &str, message: impl Into<String>) -> FetchFailure {
    FetchFailure::new(
        Diagnostic::error(
            DiagnosticCode::FetchExtraction,
            DiagnosticStage::ArchiveExtraction,
            message,
        )
        .with_context(DiagnosticContext {
            output: Some(name.to_owned()),
            ..DiagnosticContext::default()
        }),
    )
}

fn patch_failure(name: &str, message: impl Into<String>) -> FetchFailure {
    FetchFailure::new(
        Diagnostic::error(
            DiagnosticCode::FetchPatch,
            DiagnosticStage::PatchApplication,
            message,
        )
        .with_context(DiagnosticContext {
            output: Some(name.to_owned()),
            ..DiagnosticContext::default()
        }),
    )
}

fn publication_failure(name: &str, message: impl Into<String>) -> FetchFailure {
    FetchFailure::new(
        Diagnostic::error(
            DiagnosticCode::FetchPublication,
            DiagnosticStage::Publication,
            message,
        )
        .with_context(DiagnosticContext {
            output: Some(name.to_owned()),
            ..DiagnosticContext::default()
        }),
    )
}

fn failure(
    code: DiagnosticCode,
    stage: DiagnosticStage,
    message: impl Into<String>,
) -> FetchFailure {
    FetchFailure::new(Diagnostic::error(code, stage, message))
}

trait FailureHint {
    fn with_hint(self, hint: impl Into<String>) -> Self;
}

impl FailureHint for FetchFailure {
    fn with_hint(self, hint: impl Into<String>) -> Self {
        Self::new(self.into_diagnostic().with_hint(hint))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_paths_cannot_escape_the_staging_root() {
        assert!(validate_archive_path(Path::new("source/file.c"), "fixture.tar.gz").is_ok());
        let error = validate_archive_path(Path::new("../owned"), "fixture.tar.gz")
            .unwrap_err()
            .into_diagnostic();
        assert_eq!(error.code, DiagnosticCode::FetchExtraction);
    }

    #[test]
    fn archive_links_cannot_escape_the_staging_root() {
        assert!(validate_link_target(
            Path::new("source/include/current"),
            Path::new("../public"),
            "fixture.tar.gz"
        )
        .is_ok());
        assert!(validate_link_target(
            Path::new("source/link"),
            Path::new("../../outside"),
            "fixture.tar.gz"
        )
        .is_err());
        assert!(validate_link_target(
            Path::new("source/link"),
            Path::new("/outside"),
            "fixture.tar.gz"
        )
        .is_err());
    }

    #[test]
    fn origins_reject_ambiguous_paths_and_embedded_credentials() {
        assert!(expand_origin("cache://../private", "fixture.tar.gz").is_err());
        assert!(expand_origin("https://user:secret@example.test", "fixture.tar.gz").is_err());
        assert!(expand_origin("https://example.test/releases", "fixture.tar.gz").is_ok());
    }
}
