//! Fail-closed transport, integrity, extraction, and patch execution.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use aros_common::{
    exchange_prepared_tree_if_unchanged, measure_regular_file, measure_tree_content_cas,
    publication_failure_class, publish_atomic_file, publish_flat_tree_noclobber, run_status,
    sha256_bytes, AtomicFilePolicy, CommitState, DiagnosticContext, LogLevel, PortableOutputName,
    PublicationFailureClass, Sha256Digest,
};
use bzip2::read::BzDecoder;
use flate2::read::GzDecoder;
use futures_util::StreamExt;
use reqwest::redirect::Policy;
use serde::{Deserialize, Serialize};
use tempfile::{Builder, TempDir};
use xz2::read::XzDecoder;

use crate::contract::{FetchRequest, PatchSpec};
use crate::observability::Logger;
use crate::FetchResult;

mod budget;
use budget::{ExtractionBudget, MAX_ARCHIVE_ENTRIES};
mod diagnostics;
use diagnostics::{
    cache_failure, context, contract_failure, extraction_failure, integrity_failure,
    network_failure, patch_failure, publication_failure, publication_failure_with_state,
    FailureHint,
};
mod payload;
use payload::PreparedPayload;
mod locking;
use locking::FetchLock;

const LOCK_TIMEOUT: Duration = Duration::from_mins(5);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const TRANSFER_TIMEOUT: Duration = Duration::from_mins(15);
const RETRIES: usize = 3;
const MAX_DOWNLOAD_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_PATCH_BYTES: u64 = 64 * 1024 * 1024;
const RECEIPT_NAMESPACE: &str = ".aros-fetch";

struct PreparedPatch {
    spec: PatchSpec,
    origin: PreparedPayload,
    payload: PreparedPayload,
    selected_candidate: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PatchReceiptDeclaration {
    name: String,
    selected_candidate: String,
    sha256: String,
    subdirectory: Option<String>,
    options: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceReceiptDeclaration {
    schema: String,
    destination_binding: String,
    archive_name: String,
    archive_sha256: String,
    patches: Vec<PatchReceiptDeclaration>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceReceipt {
    declaration: SourceReceiptDeclaration,
    payload_tree_sha256: String,
}

/// Successful execution state used to classify any later observability error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FetchOutcome {
    /// Whether this invocation crossed the source-tree publication boundary.
    pub committed: bool,
}

/// Execute one validated fetch contract to completion.
///
/// # Errors
///
/// Returns one stable diagnostic when cache locking, transport, integrity
/// validation, safe extraction, publication, or patch application fails.
pub async fn run(request: &FetchRequest, logger: &mut Logger) -> FetchResult<FetchOutcome> {
    create_directory(&request.location, "archive cache")?;
    create_directory(&request.destination, "archive destination")?;
    create_directory(&request.base, "patch cache")?;
    let _destination_lock = FetchLock::acquire_destination(&request.destination)?;
    // Patch declarations intentionally share a cache root. Serialize that
    // namespace independently of archive and destination names so two source
    // transactions cannot alias the same historical patch basename.
    let _patch_cache_lock = FetchLock::acquire_patch_base(&request.base)?;

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
        false,
    )
    .await?;

    let mut patches = Vec::with_capacity(request.patches.len());
    for patch in &request.patches {
        patches.push(fetch_patch(&client, patch, request, logger).await?);
    }

    publish_source_transaction(&archive, &patches, request, logger)
        .map(|committed| FetchOutcome { committed })
}

async fn fetch_patch(
    client: &reqwest::Client,
    patch: &PatchSpec,
    request: &FetchRequest,
    logger: &mut Logger,
) -> FetchResult<PreparedPatch> {
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
    let namespace = patch_declaration_key(patch, request);
    let cache_root = request
        .base
        .join(RECEIPT_NAMESPACE)
        .join("patch-cache")
        .join(namespace);
    let downloads = cache_root.join("downloads");
    create_real_directory_chain(&request.base, &downloads, &patch.name)?;
    let fetched = fetch_candidates(
        client,
        &candidates,
        &request.patch_origins,
        &downloads,
        request,
        logger,
        true,
    )
    .await?;
    let payload = if fetched.name == patch.name {
        read_bounded_patch(&fetched.path, &patch.name)?
    } else {
        extract_patch_payload(&fetched.path, &fetched.name, &patch.name)?
    };
    let digest = sha256_bytes(&payload);
    let payload_root = cache_root.join("payloads").join(digest.to_string());
    let receipt = serde_json::to_vec(&PatchReceiptDeclaration {
        name: patch.name.clone(),
        selected_candidate: fetched.name.clone(),
        sha256: digest.to_string(),
        subdirectory: patch
            .subdirectory
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
        options: patch.options.clone(),
    })
    .map_err(|error| cache_failure(format!("cannot encode patch cache receipt: {error}")))?;
    create_real_directory_chain(
        &request.base,
        payload_root
            .parent()
            .expect("content-addressed payload has parent"),
        &patch.name,
    )?;
    let payload_name = PortableOutputName::new("payload.patch")
        .map_err(|error| cache_failure(format!("invalid internal patch member: {error}")))?;
    let receipt_name = PortableOutputName::new("receipt.json")
        .map_err(|error| cache_failure(format!("invalid internal receipt member: {error}")))?;
    match publish_flat_tree_noclobber(
        &payload_root,
        &[
            (payload_name, payload.as_slice()),
            (receipt_name, receipt.as_slice()),
        ],
    ) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            verify_cached_patch_tree(&payload_root, &payload, &receipt, &patch.name)?;
        }
        Err(error) => {
            return Err(cache_failure(format!(
                "cannot publish declaration-bound patch cache '{}': {error}",
                payload_root.display()
            )))
        }
    }
    let prepared_payload = PreparedPayload::import(
        &payload_root.join("payload.patch"),
        &patch.name,
        MAX_PATCH_BYTES,
    )?;
    Ok(PreparedPatch {
        spec: patch.clone(),
        selected_candidate: fetched.name.clone(),
        origin: fetched,
        payload: prepared_payload,
    })
}

fn patch_declaration_key(patch: &PatchSpec, request: &FetchRequest) -> String {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"aros-fetch-patch-declaration-v1\0");
    bytes.extend_from_slice(patch.name.as_bytes());
    bytes.push(0);
    if let Some(subdirectory) = &patch.subdirectory {
        bytes.extend_from_slice(subdirectory.to_string_lossy().as_bytes());
    }
    for option in &patch.options {
        bytes.push(0);
        bytes.extend_from_slice(option.as_bytes());
    }
    for origin in &request.patch_origins {
        bytes.push(0xff);
        bytes.extend_from_slice(origin.as_bytes());
    }
    for candidate in std::iter::once(patch.name.clone()).chain(
        ["tar.bz2", "tar.gz", "zip"]
            .into_iter()
            .map(|suffix| format!("{}.{suffix}", patch.name)),
    ) {
        bytes.push(0xfe);
        bytes.extend_from_slice(candidate.as_bytes());
        if let Some(digest) = request.checksums.get(&candidate) {
            bytes.push(b'=');
            bytes.extend_from_slice(digest.to_string().as_bytes());
        }
    }
    sha256_bytes(&bytes).to_string()
}

fn read_bounded_patch(path: &Path, name: &str) -> FetchResult<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        extraction_failure(name, format!("cannot inspect patch payload: {error}"))
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(extraction_failure(
            name,
            "patch payload is not a real regular file",
        ));
    }
    if metadata.len() > MAX_PATCH_BYTES {
        return Err(extraction_failure(
            name,
            format!("patch payload exceeds the {MAX_PATCH_BYTES}-byte safety limit"),
        ));
    }
    let bytes = fs::read(path)
        .map_err(|error| extraction_failure(name, format!("cannot read patch payload: {error}")))?;
    if bytes.len() as u64 != metadata.len() {
        return Err(extraction_failure(
            name,
            "patch payload changed while it was read",
        ));
    }
    Ok(bytes)
}

fn extract_patch_payload(
    archive: &Path,
    archive_name: &str,
    patch_name: &str,
) -> FetchResult<Vec<u8>> {
    let staging = Builder::new()
        .prefix(".aros-fetch-patch-extract-")
        .tempdir()
        .map_err(|error| {
            extraction_failure(
                archive_name,
                format!("cannot create patch extraction staging: {error}"),
            )
        })?;
    unpack_archive(archive, archive_name, staging.path())?;
    read_bounded_patch(&staging.path().join(patch_name), patch_name)
}

fn verify_cached_patch_tree(
    root: &Path,
    payload: &[u8],
    receipt: &[u8],
    name: &str,
) -> FetchResult<()> {
    let before = measure_tree_content_cas(root).map_err(|error| {
        cache_failure(format!(
            "cannot safely measure cached patch '{name}' tree: {error}"
        ))
    })?;
    if before.entry_count() != 2 {
        return Err(cache_failure(format!(
            "cached patch '{name}' contains unexpected members"
        )));
    }
    for (member, expected) in [("payload.patch", payload), ("receipt.json", receipt)] {
        let path = root.join(member);
        let actual = measure_regular_file(&path)
            .map_err(|error| {
                cache_failure(format!(
                    "cannot safely read cached patch '{name}' member '{member}': {error}"
                ))
            })?
            .map(|(_, bytes)| bytes)
            .ok_or_else(|| {
                cache_failure(format!("cached patch '{name}' member '{member}' is absent"))
            })?;
        if actual != expected {
            return Err(cache_failure(format!(
                "cached patch '{name}' member '{member}' conflicts with its declaration/content address"
            ))
            .with_hint("remove only the reported .aros-fetch/patch-cache namespace and retry"));
        }
    }
    let after = measure_tree_content_cas(root).map_err(|error| {
        cache_failure(format!(
            "cannot remeasure cached patch '{name}' tree: {error}"
        ))
    })?;
    if after != before {
        return Err(cache_failure(format!(
            "cached patch '{name}' changed while it was verified"
        )));
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
    refresh_local: bool,
) -> FetchResult<PreparedPayload> {
    let mut attempts = Vec::new();
    for candidate in candidates {
        let cached = cache.join(candidate);
        let candidate_lock = FetchLock::acquire_candidate(&cached)?;
        let has_local_origin = origins.iter().any(|origin| !origin.contains("://"));
        if refresh_local && has_local_origin {
            for origin in origins {
                for source in expand_origin(origin, candidate)? {
                    let Source::Local(path) = source else {
                        continue;
                    };
                    match fs::symlink_metadata(&path) {
                        Ok(_) => {
                            let payload =
                                PreparedPayload::import(&path, candidate, MAX_DOWNLOAD_BYTES)?;
                            verify(
                                candidate,
                                &payload,
                                request.checksums.get(candidate),
                                logger,
                            )?;
                            candidate_lock.revalidate()?;
                            return Ok(payload);
                        }
                        Err(error) if error.kind() == io::ErrorKind::NotFound => attempts
                            .push(format!("local payload '{}' is unavailable", path.display())),
                        Err(error) => {
                            return Err(cache_failure(format!(
                                "cannot inspect local payload '{}': {error}",
                                path.display()
                            )))
                        }
                    }
                }
            }
        }
        let has_remote_origin = origins.iter().any(|origin| origin.contains("://"));
        if (!refresh_local || !has_local_origin || has_remote_origin) && is_regular_file(&cached)? {
            let payload = PreparedPayload::import(&cached, candidate, MAX_DOWNLOAD_BYTES)?;
            verify(
                candidate,
                &payload,
                request.checksums.get(candidate),
                logger,
            )?;
            candidate_lock.revalidate()?;
            return Ok(payload);
        }
        for (origin_index, origin) in origins.iter().enumerate() {
            for source in expand_origin(origin, candidate)? {
                if refresh_local && matches!(source, Source::Local(_)) {
                    continue;
                }
                if request.offline && !matches!(&source, Source::Local(_)) {
                    continue;
                }
                aros_common::outputln!(
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
                        let payload =
                            PreparedPayload::import(&path, candidate, MAX_DOWNLOAD_BYTES)?;
                        if let Err(error) = verify(
                            candidate,
                            &payload,
                            request.checksums.get(candidate),
                            logger,
                        ) {
                            let _ = fs::remove_file(&path);
                            return Err(error);
                        }
                        logger.event(
                            LogLevel::Info,
                            "transfer.complete",
                            "declared payload transfer completed",
                            &event_context,
                        )?;
                        candidate_lock.revalidate()?;
                        return Ok(payload);
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
    payload::publish_download_noclobber(&temporary, &destination, candidate)
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
    payload: &PreparedPayload,
    expected: Option<&Sha256Digest>,
    logger: &mut Logger,
) -> FetchResult<()> {
    let Some(expected) = expected else {
        return Ok(());
    };
    if &payload.digest != expected {
        return Err(integrity_failure(
            candidate,
            format!(
                "SHA-256 mismatch: expected {expected}, actual {}",
                payload.digest
            ),
        )
        .with_hint("do not replace the declared digest until the upstream payload change has been independently verified"));
    }
    aros_common::outputln!("Verified   {candidate} (SHA-256)");
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

fn publish_source_transaction(
    archive: &PreparedPayload,
    patches: &[PreparedPatch],
    request: &FetchRequest,
    logger: &mut Logger,
) -> FetchResult<bool> {
    publish_source_transaction_inner(archive, patches, request, logger)
        .map_err(|error| error.with_commit_state_if_absent(CommitState::RolledBack))
}

fn publish_source_transaction_inner(
    archive: &PreparedPayload,
    patches: &[PreparedPatch],
    request: &FetchRequest,
    logger: &mut Logger,
) -> FetchResult<bool> {
    let archive_name = &archive.name;
    let archive_marker = request.base.join(format!(".{archive_name}.unpacked"));
    let extracts_archive =
        request.archive_candidates.len() > 1 || archive_name.as_str() != request.archive;
    if !extracts_archive && patches.is_empty() {
        return Ok(false);
    }

    let destination = request.destination.canonicalize().map_err(|error| {
        publication_failure(
            archive_name,
            format!("cannot resolve archive destination: {error}"),
        )
    })?;
    let destination_before = measure_tree_content_cas(&destination).map_err(|error| {
        publication_failure(
            archive_name,
            format!("cannot measure destination transaction precondition: {error}"),
        )
    })?;
    let destination_binding = sha256_bytes(destination.as_os_str().as_encoded_bytes());
    let declaration = SourceReceiptDeclaration {
        schema: "aros-fetch-source-receipt-v1".into(),
        destination_binding: destination_binding.to_string(),
        archive_name: archive_name.to_owned(),
        archive_sha256: archive.digest.to_string(),
        patches: patches
            .iter()
            .map(|patch| PatchReceiptDeclaration {
                name: patch.spec.name.clone(),
                selected_candidate: patch.selected_candidate.clone(),
                sha256: patch.payload.digest.to_string(),
                subdirectory: patch
                    .spec
                    .subdirectory
                    .as_ref()
                    .map(|path| path.to_string_lossy().into_owned()),
                options: patch.spec.options.clone(),
            })
            .collect(),
    };
    let contract_id = source_contract_id(&declaration)?;
    let receipt_path = destination
        .join(RECEIPT_NAMESPACE)
        .join("receipts")
        .join(format!("{contract_id}.json"));
    if !request.force
        && source_receipt_matches(
            &receipt_path,
            &declaration,
            &destination_before,
            archive_name,
        )?
    {
        return Ok(false);
    }
    let parent = destination.parent().ok_or_else(|| {
        publication_failure(archive_name, "archive destination has no parent directory")
    })?;
    let staging = Builder::new()
        .prefix(".aros-fetch-publish-")
        .tempdir_in(parent)
        .map_err(|error| {
            publication_failure(
                archive_name,
                format!("cannot create publication staging directory: {error}"),
            )
        })?;
    copy_tree_contents(&destination, staging.path(), archive_name)?;

    if extracts_archive {
        aros_common::outputln!("Unpacking  `{archive_name}`...");
        let extraction = Builder::new()
            .prefix(".aros-fetch-extract-")
            .tempdir_in(parent)
            .map_err(|error| {
                extraction_failure(
                    archive_name,
                    format!("cannot create extraction staging directory: {error}"),
                )
            })?;
        unpack_archive(&archive.path, archive_name, extraction.path())?;
        merge_staged_archive(&extraction, staging.path(), request.force, archive_name)?;
    }

    for patch in patches {
        apply_patch_to(&patch.spec, &patch.payload.path, staging.path())?;
    }

    let mut post_commit_markers = Vec::new();
    if extracts_archive {
        stage_or_defer_marker(
            &archive_marker,
            &destination,
            staging.path(),
            "unpacked",
            &mut post_commit_markers,
        )?;
    }
    for patch in patches {
        let marker = request.base.join(format!(".{}.applied", patch.spec.name));
        stage_or_defer_marker(
            &marker,
            &destination,
            staging.path(),
            "applied",
            &mut post_commit_markers,
        )?;
    }

    let receipt_directory = staging.path().join(RECEIPT_NAMESPACE).join("receipts");
    create_real_directory_chain(staging.path(), &receipt_directory, archive_name)?;
    let staged_snapshot = measure_tree_content_cas(staging.path()).map_err(|error| {
        publication_failure(
            archive_name,
            format!("cannot measure prepared source payload: {error}"),
        )
    })?;
    let receipt = SourceReceipt {
        declaration,
        payload_tree_sha256: staged_snapshot
            .payload_digest_excluding(Some(RECEIPT_NAMESPACE))
            .to_string(),
    };
    let receipt_bytes = serde_json::to_vec_pretty(&receipt).map_err(|error| {
        publication_failure(
            archive_name,
            format!("cannot encode source receipt: {error}"),
        )
    })?;
    let staged_receipt = receipt_directory.join(format!("{contract_id}.json"));
    fs::write(&staged_receipt, &receipt_bytes).map_err(|error| {
        publication_failure(
            archive_name,
            format!(
                "cannot stage source receipt '{}': {error}",
                staged_receipt.display()
            ),
        )
    })?;

    #[cfg(debug_assertions)]
    fetch_test_pause("before-payload-revalidation");
    archive.revalidate()?;
    for patch in patches {
        patch.origin.revalidate()?;
        patch.payload.revalidate()?;
    }

    if let Err(error) =
        exchange_prepared_tree_if_unchanged(staging.path(), &destination, &destination_before)
    {
        let preserved = staging.keep();
        let state = match publication_failure_class(&error) {
            PublicationFailureClass::CommitStateUncertain => CommitState::Indeterminate,
            _ => CommitState::RolledBack,
        };
        return Err(publication_failure_with_state(
            archive_name,
            format!(
                "cannot atomically publish prepared source tree: {error}; transaction tree retained at '{}'",
                preserved.display()
            ),
            state,
        ));
    }
    // Everything after this boundary is advisory. The internal receipt was
    // committed with the tree and is authoritative; legacy mirrors and logs
    // must never turn a successful publication into an unclassified failure.
    let cleanup_warning = staging.close().err().map(|error| {
        format!("previous destination cleanup failed after atomic publication: {error}")
    });
    for (marker, value) in post_commit_markers {
        let _ = write_marker(&marker, value);
    }

    if extracts_archive {
        let _ = logger.event(
            LogLevel::Info,
            "archive.extracted",
            "archive safely extracted and atomically published",
            &DiagnosticContext {
                output: Some(archive_name.to_owned()),
                ..DiagnosticContext::default()
            },
        );
    }
    for patch in patches {
        let _ = logger.event(
            LogLevel::Info,
            "patch.applied",
            "declared patch applied before atomic publication",
            &DiagnosticContext {
                output: Some(patch.spec.name.clone()),
                ..DiagnosticContext::default()
            },
        );
    }
    if let Some(warning) = cleanup_warning {
        let _ = logger.event(
            LogLevel::Warn,
            "publication.cleanup",
            &warning,
            &DiagnosticContext {
                output: Some(archive_name.to_owned()),
                ..DiagnosticContext::default()
            },
        );
    }
    Ok(true)
}

fn source_contract_id(declaration: &SourceReceiptDeclaration) -> FetchResult<String> {
    let bytes = serde_json::to_vec(declaration).map_err(|error| {
        cache_failure(format!("cannot encode source receipt declaration: {error}"))
    })?;
    Ok(sha256_bytes(&bytes).to_string())
}

fn source_receipt_matches(
    receipt_path: &Path,
    declaration: &SourceReceiptDeclaration,
    current_tree: &aros_common::TreeContentCas,
    archive_name: &str,
) -> FetchResult<bool> {
    let measured = match measure_regular_file(receipt_path) {
        Ok(measured) => measured,
        Err(error) => {
            return Err(publication_failure(
                archive_name,
                format!("cannot inspect internal source receipt: {error}"),
            )
            .with_hint("remove any symlink or special object from the reserved .aros-fetch receipt path and retry"))
        }
    };
    let Some((_identity, bytes)) = measured else {
        return Ok(false);
    };
    let receipt: SourceReceipt = serde_json::from_slice(&bytes).map_err(|error| {
        publication_failure(
            archive_name,
            format!("internal source receipt is malformed: {error}"),
        )
        .with_hint("remove the malformed receipt and retry; legacy markers cannot authorize a skip")
    })?;
    if &receipt.declaration != declaration {
        return Err(publication_failure(
            archive_name,
            "internal source receipt does not match its declaration-bound path",
        )
        .with_hint("remove the conflicting receipt and retry; do not reuse receipts across destinations or declarations"));
    }
    Ok(receipt.payload_tree_sha256
        == current_tree
            .payload_digest_excluding(Some(RECEIPT_NAMESPACE))
            .to_string())
}

fn create_real_directory_chain(root: &Path, target: &Path, name: &str) -> FetchResult<()> {
    let relative = target.strip_prefix(root).map_err(|_| {
        publication_failure(name, "receipt directory escapes publication staging root")
    })?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(publication_failure(
                name,
                "invalid receipt directory component",
            ));
        };
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => {
                return Err(publication_failure(
                    name,
                    format!(
                        "reserved receipt namespace '{}' is not a real directory",
                        current.display()
                    ),
                )
                .with_hint("remove the conflicting .aros-fetch object and retry"))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::create_dir(&current).map_err(|error| {
                    publication_failure(
                        name,
                        format!(
                            "cannot create receipt directory '{}': {error}",
                            current.display()
                        ),
                    )
                })?;
            }
            Err(error) => {
                return Err(publication_failure(
                    name,
                    format!(
                        "cannot inspect receipt directory '{}': {error}",
                        current.display()
                    ),
                ))
            }
        }
    }
    Ok(())
}

fn copy_tree_contents(source: &Path, destination: &Path, name: &str) -> FetchResult<()> {
    fn copy_directory(
        root: &Path,
        source: &Path,
        destination: &Path,
        name: &str,
    ) -> FetchResult<()> {
        for entry in fs::read_dir(source).map_err(|error| {
            publication_failure(
                name,
                format!(
                    "cannot read existing destination '{}': {error}",
                    source.display()
                ),
            )
        })? {
            let entry = entry.map_err(|error| {
                publication_failure(name, format!("cannot inspect destination entry: {error}"))
            })?;
            let source_path = entry.path();
            let destination_path = destination.join(entry.file_name());
            let metadata = fs::symlink_metadata(&source_path).map_err(|error| {
                publication_failure(
                    name,
                    format!("cannot inspect '{}': {error}", source_path.display()),
                )
            })?;
            if metadata.file_type().is_symlink() {
                let target = fs::read_link(&source_path).map_err(|error| {
                    publication_failure(
                        name,
                        format!("cannot read link '{}': {error}", source_path.display()),
                    )
                })?;
                let relative = source_path.strip_prefix(root).map_err(|_| {
                    publication_failure(name, "destination copy escaped its source root")
                })?;
                validate_link_target(relative, &target, name)?;
                #[cfg(unix)]
                std::os::unix::fs::symlink(&target, &destination_path).map_err(|error| {
                    publication_failure(
                        name,
                        format!(
                            "cannot stage link '{}': {error}",
                            destination_path.display()
                        ),
                    )
                })?;
                #[cfg(not(unix))]
                return Err(publication_failure(
                    name,
                    "safe symbolic-link staging is unsupported on this host",
                ));
            } else if metadata.is_dir() {
                fs::create_dir(&destination_path).map_err(|error| {
                    publication_failure(
                        name,
                        format!(
                            "cannot stage directory '{}': {error}",
                            destination_path.display()
                        ),
                    )
                })?;
                copy_directory(root, &source_path, &destination_path, name)?;
                fs::set_permissions(&destination_path, metadata.permissions()).map_err(
                    |error| {
                        publication_failure(
                            name,
                            format!("cannot preserve '{}': {error}", destination_path.display()),
                        )
                    },
                )?;
            } else if metadata.is_file() {
                fs::copy(&source_path, &destination_path).map_err(|error| {
                    publication_failure(
                        name,
                        format!("cannot stage file '{}': {error}", source_path.display()),
                    )
                })?;
                fs::set_permissions(&destination_path, metadata.permissions()).map_err(
                    |error| {
                        publication_failure(
                            name,
                            format!("cannot preserve '{}': {error}", destination_path.display()),
                        )
                    },
                )?;
            } else {
                return Err(publication_failure(
                    name,
                    format!(
                        "existing destination entry '{}' is not a regular file, directory, or symbolic link",
                        source_path.display()
                    ),
                ));
            }
        }
        Ok(())
    }

    copy_directory(source, source, destination, name)
}

fn merge_staged_archive(
    extraction: &TempDir,
    destination: &Path,
    force: bool,
    name: &str,
) -> FetchResult<()> {
    let entries = fs::read_dir(extraction.path()).map_err(|error| {
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
                    format!(
                        "cannot replace staged destination '{}': {error}",
                        target.display()
                    ),
                )
            })?;
        }
        fs::rename(entry.path(), &target).map_err(|error| {
            publication_failure(
                name,
                format!(
                    "cannot assemble staged destination '{}': {error}",
                    target.display()
                ),
            )
        })?;
    }
    Ok(())
}

fn stage_or_defer_marker<'a>(
    marker: &Path,
    destination: &Path,
    staging: &Path,
    value: &'a str,
    deferred: &mut Vec<(PathBuf, &'a str)>,
) -> FetchResult<()> {
    let absolute_marker = if marker.is_absolute() {
        marker.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| cache_failure(format!("cannot resolve marker path: {error}")))?
            .join(marker)
    };
    if let Ok(relative) = absolute_marker.strip_prefix(destination) {
        let staged = staging.join(relative);
        if let Some(parent) = staged.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                cache_failure(format!("cannot stage marker directory: {error}"))
            })?;
        }
        fs::write(&staged, format!("{value}\n")).map_err(|error| {
            cache_failure(format!(
                "cannot stage marker '{}': {error}",
                staged.display()
            ))
        })?;
    } else {
        deferred.push((marker.to_path_buf(), value));
    }
    Ok(())
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
    let mut budget = ExtractionBudget::default();
    for entry in entries {
        let mut entry = entry.map_err(|error| {
            extraction_failure(name, format!("cannot read archive entry: {error}"))
        })?;
        let path = entry
            .path()
            .map_err(|error| extraction_failure(name, format!("invalid entry path: {error}")))?
            .into_owned();
        validate_archive_path(&path, name)?;
        budget.account(entry.size(), name, &path)?;
        if entry.header().entry_type().is_hard_link() {
            return Err(extraction_failure(
                name,
                format!(
                    "TAR hard link '{}' is rejected because its logical expansion cannot be independently budgeted",
                    path.display()
                ),
            ));
        }
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
    }
    if budget.entries == 0 {
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
    if archive.len() as u64 > MAX_ARCHIVE_ENTRIES {
        return Err(extraction_failure(
            name,
            format!("archive exceeds the {MAX_ARCHIVE_ENTRIES}-entry safety limit"),
        ));
    }
    let mut budget = ExtractionBudget::default();
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
        let output_probe_limit = budget
            .output_probe_limit()
            .min(entry.size().saturating_add(1));
        budget.account(entry.size(), name, &enclosed)?;
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
            let declared = entry.size();
            let mut limited = entry.by_ref().take(output_probe_limit);
            let copied = io::copy(&mut limited, &mut target).map_err(|error| {
                extraction_failure(
                    name,
                    format!("cannot extract '{}': {error}", output.display()),
                )
            })?;
            if copied != declared {
                return Err(extraction_failure(
                    name,
                    format!(
                        "ZIP entry '{}' expanded to {copied} bytes but declared {}",
                        output.display(),
                        declared
                    ),
                ));
            }
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

fn apply_patch_to(patch: &PatchSpec, input_path: &Path, destination: &Path) -> FetchResult<()> {
    let input = File::open(input_path).map_err(|error| {
        patch_failure(&patch.name, format!("cannot open patch payload: {error}"))
    })?;
    let directory = patch
        .subdirectory
        .as_deref()
        .map_or_else(|| destination.to_path_buf(), |path| destination.join(path));
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
    let canonical_destination = destination.canonicalize().map_err(|error| {
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
                destination.display()
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
    Ok(())
}

#[cfg(debug_assertions)]
fn fetch_test_pause(point: &str) {
    if std::env::var("AROS_FETCH_TEST_PAUSE_AT").as_deref() == Ok(point) {
        let millis = std::env::var("AROS_FETCH_TEST_PAUSE_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(250);
        std::thread::sleep(Duration::from_millis(millis));
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
    let contents = format!("{value}\n");
    let policy = match measure_regular_file(path).map_err(|error| {
        cache_failure(format!(
            "cannot measure marker '{}': {error}",
            path.display()
        ))
    })? {
        None => AtomicFilePolicy::NoClobber,
        Some((identity, previous)) => AtomicFilePolicy::ReplaceIf {
            identity,
            sha256: sha256_bytes(&previous),
        },
    };
    publish_atomic_file(path, contents.as_bytes(), policy)
        .map(|_| ())
        .map_err(|error| {
            cache_failure(format!(
                "cannot atomically publish marker '{}': {error}",
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

#[cfg(test)]
mod tests;
