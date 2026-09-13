//! Immutable, content-addressed GenMF reference expansions.
//!
//! The verifier used to cache expansions below a work directory using a
//! slash-to-percent filename and mtime comparison. This module makes every
//! expansion a separately selected immutable object instead: an exact source
//! file, the complete template closure, GenMF, the selected Python interpreter
//! and the generation format all participate in its identity.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use aros_cache::{
    observe_root, resolve_explicit_root, CacheCapability, CacheSideEffects, RootObservation,
};
use aros_common::{
    bounded_output_detail, directory_entry_names_nofollow_bounded, ensure_directory_nofollow,
    measure_regular_file_bounded, publish_flat_tree_noclobber, run_output_with_control,
    sha256_bytes, AdvisoryFileLock, CancellationToken, PortableOutputName, DEFAULT_CAPTURE_LIMIT,
};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::coverage_io::find_mmakefiles;

const CACHE_NAMESPACE: &str = "genmf";
const CACHE_VERSION: &str = "v1";
const EXPANSION_FILE: &str = "expansion.mk";
const RECEIPT_FILE: &str = "receipt.json";
const MAX_INPUT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_EXPANSION_BYTES: u64 = 32 * 1024 * 1024;
const MAX_INTERPRETER_BYTES: u64 = 256 * 1024 * 1024;
const MAX_RECEIPT_BYTES: u64 = 256 * 1024;
const MAX_GENERATION_ENTRIES: usize = 2;

/// Stable schema for an immutable GenMF expansion receipt.
pub const GENMF_RECEIPT_SCHEMA: &str = "aros-genmf-cache-generation-v1";
/// Stable schema for a passive GenMF cache-root observation.
pub const GENMF_STATUS_SCHEMA: &str = "aros-cache-genmf-status-v1";
/// Stable schema for a selected GenMF cache list.
pub const GENMF_LIST_SCHEMA: &str = "aros-cache-genmf-list-v1";
/// Stable schema for GenMF cache verification.
pub const GENMF_VERIFY_SCHEMA: &str = "aros-cache-genmf-verify-v1";
/// Stable schema for GenMF cache refresh.
pub const GENMF_REFRESH_SCHEMA: &str = "aros-cache-genmf-refresh-v1";

/// One failure at the GenMF cache authority boundary.
#[derive(Debug, Error)]
#[error("{message}")]
pub struct GenmfCacheError {
    message: String,
    timed_out: bool,
}

impl GenmfCacheError {
    fn input(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            timed_out: false,
        }
    }

    fn state(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            timed_out: false,
        }
    }

    fn timeout(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            timed_out: true,
        }
    }

    /// Whether the rejected operation exhausted its explicit execution budget.
    #[must_use]
    pub const fn timed_out(&self) -> bool {
        self.timed_out
    }
}

/// Explicit inputs selecting GenMF expansion objects.
#[derive(Clone, Debug)]
pub struct GenmfCacheRequest {
    /// Existing, no-follow AROS source root containing GenMF and MMake inputs.
    pub source_dir: PathBuf,
    /// Existing, no-follow cache root that owns the `genmf/v1` namespace.
    pub cache_dir: PathBuf,
    /// Selected Python interpreter invocation path.
    pub python: PathBuf,
    /// Shared hard budget for one cache-lock wait and one GenMF invocation.
    pub timeout: Duration,
}

/// Passive observation of a caller-selected GenMF cache root.
#[derive(Clone, Debug, Serialize)]
pub struct GenmfCacheStatus {
    /// Versioned result schema.
    pub schema: &'static str,
    /// Stable operation identifier.
    pub operation: &'static str,
    /// This report observes root metadata only.
    pub observation: &'static str,
    /// Hard side-effect contract.
    pub side_effects: CacheSideEffects,
    /// Available GenMF cache operations.
    pub capabilities: [CacheCapability; 4],
    /// Caller-selected cache-root observation.
    pub root: RootObservation,
    /// Immutable object layout below `root`.
    pub object_layout: &'static str,
    /// Boundary excluding verifier reports and source/build trees.
    pub boundary: &'static str,
}

/// A byte identity for an input regular file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenmfInputIdentity {
    /// Source-root-relative, portable input name.
    pub relative_path: String,
    /// Exact SHA-256 of the input bytes.
    pub sha256: String,
}

/// Exact interpreter identity used by GenMF.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenmfInterpreterIdentity {
    /// Exact SHA-256 of the resolved interpreter executable.
    pub sha256: String,
    /// Normalized `python --version` output.
    pub version: String,
}

/// Portable identity selecting one GenMF expansion.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenmfExpansionIdentity {
    /// Identity schema, kept separate from the published receipt schema.
    pub schema: String,
    /// Cache format and generator invocation semantics.
    pub format: String,
    /// Source MMake input.
    pub source: GenmfInputIdentity,
    /// Complete recursively discovered template closure.
    pub templates: Vec<GenmfInputIdentity>,
    /// The historic upstream GenMF script.
    pub generator: GenmfInputIdentity,
    /// Selected Python interpreter identity.
    pub interpreter: GenmfInterpreterIdentity,
    /// Generator options affecting output semantics.
    pub options: Vec<String>,
}

/// Selected generation metadata, including diagnostic-only filesystem paths.
#[derive(Clone, Debug, Serialize)]
pub struct GenmfCacheSelection {
    /// Cache root after explicit root validation.
    pub cache_dir: PathBuf,
    /// Selected source root after no-follow validation.
    pub source_dir: PathBuf,
    /// Absolute interpreter invocation path used for a refresh.
    pub python_invocation_path: PathBuf,
    /// Canonical regular executable whose bytes selected the interpreter.
    pub python_resolved_path: PathBuf,
    /// Shared template closure for all selected MMake inputs.
    pub templates: Vec<GenmfInputIdentity>,
    /// Exact GenMF source identity.
    pub generator: GenmfInputIdentity,
    /// Interpreter identity used by all entries.
    pub interpreter: GenmfInterpreterIdentity,
    /// Every discovered reference expansion selection in stable path order.
    pub entries: Vec<GenmfCacheEntrySelection>,
}

/// One exact GenMF expansion selection.
#[derive(Clone, Debug, Serialize)]
pub struct GenmfCacheEntrySelection {
    /// Deterministic immutable generation key.
    pub generation: String,
    /// Complete portable identity bound by the receipt.
    pub identity: GenmfExpansionIdentity,
    /// Source-root-relative MMake path used only for the generator invocation.
    pub source_relative_path: String,
    /// Existing no-follow source file used only for the generator invocation.
    #[serde(skip_serializing)]
    pub source_path: PathBuf,
}

/// Metadata-only state of one selected immutable generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GenmfCacheEntryState {
    /// No final generation exists for the selected identity.
    Missing,
    /// A regular generation directory is present but unverified.
    PresentUnverified,
    /// The final path is unsafe or not a directory.
    Unsafe,
    /// Metadata could not be read.
    Inaccessible,
}

/// One metadata-only list result.
#[derive(Clone, Debug, Serialize)]
pub struct GenmfCacheListEntry {
    /// Exact portable selection.
    pub selection: GenmfCacheEntrySelection,
    /// Direct final-path state without reading generation contents.
    pub state: GenmfCacheEntryState,
}

/// Versioned GenMF cache list report.
#[derive(Clone, Debug, Serialize)]
pub struct GenmfCacheList {
    /// Versioned result schema.
    pub schema: &'static str,
    /// Stable operation identifier.
    pub operation: &'static str,
    /// Side effects performed by selection and metadata observation.
    pub side_effects: CacheSideEffects,
    /// Exact source/interpreter/closure selection.
    pub selection: GenmfCacheSelection,
    /// One result per selected MMake input.
    pub entries: Vec<GenmfCacheListEntry>,
    /// Deliberate list boundary.
    pub boundary: &'static str,
}

/// Verified immutable GenMF generation metadata.
#[derive(Clone, Debug, Serialize)]
pub struct GenmfCacheGeneration {
    /// Stable result schema selected by the caller.
    pub schema: &'static str,
    /// Stable operation identifier selected by the caller.
    pub operation: &'static str,
    /// Exact generation selection.
    pub selection: GenmfCacheEntrySelection,
    /// Final immutable generation directory.
    pub generation_dir: PathBuf,
    /// Measured output SHA-256.
    pub output_sha256: String,
    /// Measured output size.
    pub output_size: u64,
}

/// Versioned verification result for every current GenMF selection.
#[derive(Clone, Debug, Serialize)]
pub struct GenmfCacheVerification {
    /// Versioned result schema.
    pub schema: &'static str,
    /// Stable operation identifier.
    pub operation: &'static str,
    /// Verification side-effect contract.
    pub side_effects: CacheSideEffects,
    /// Verified generations in stable source-path order.
    pub entries: Vec<GenmfCacheGeneration>,
    /// Verification boundary.
    pub boundary: &'static str,
}

/// One selected expansion materialized for verifier consumption.
#[derive(Clone, Debug)]
pub struct GenmfCacheMaterializedEntry {
    /// Exact selection that was either reused or rejected.
    pub selection: GenmfCacheEntrySelection,
    /// Verified immutable output or the isolated failure for this input.
    pub result: Result<GenmfCacheGeneration, GenmfCacheMaterializationFailure>,
}

/// A per-input failure retained by the verifier rather than hiding other results.
#[derive(Clone, Debug)]
pub struct GenmfCacheMaterializationFailure {
    /// Human-readable reason without host-specific cache paths.
    pub message: String,
    /// Whether the failure exhausted the selected execution budget.
    pub timed_out: bool,
}

/// Selection plus one materialization outcome for every discovered MMake input.
#[derive(Clone, Debug)]
pub struct GenmfCacheMaterialization {
    /// Exact source/interpreter/template selection used for this operation.
    pub selection: GenmfCacheSelection,
    /// One stable-path outcome per selected MMake input.
    pub entries: Vec<GenmfCacheMaterializedEntry>,
}

/// Passive GenMF cache-root observation.
///
/// # Errors
///
/// Returns an error only when `dir` is relative or result-root resolution is
/// invalid. The observation never creates or scans cache state.
pub fn status(dir: &Path) -> Result<GenmfCacheStatus, GenmfCacheError> {
    let root = resolve_explicit_root(dir.to_path_buf())
        .map_err(|error| GenmfCacheError::input(error.to_string()))?;
    Ok(GenmfCacheStatus {
        schema: GENMF_STATUS_SCHEMA,
        operation: "genmf.status",
        observation: "passive",
        side_effects: passive_side_effects(),
        capabilities: [
            CacheCapability::Status,
            CacheCapability::List,
            CacheCapability::Verify,
            CacheCapability::Refresh,
        ],
        root: observe_root(root),
        object_layout: "genmf/v1/<selection-sha256>/{expansion.mk,receipt.json}",
        boundary: "status observes only the explicit cache root; source trees, verifier reports and build trees remain outside GenMF cache authority",
    })
}

/// Select all current GenMF reference expansions without creating cache state.
///
/// # Errors
///
/// Returns an error for a relative/unsafe root, source closure, interpreter or
/// bounded interpreter probe. Selection hashes source inputs but does not read
/// any cache generation.
pub fn select(request: &GenmfCacheRequest) -> Result<GenmfCacheSelection, GenmfCacheError> {
    select_with_cancellation(request, &CancellationToken::default())
}

fn select_with_cancellation(
    request: &GenmfCacheRequest,
    cancellation: &CancellationToken,
) -> Result<GenmfCacheSelection, GenmfCacheError> {
    let source_dir = checked_directory(&request.source_dir, "selected GenMF source root")?;
    let cache_dir = checked_directory(&request.cache_dir, "selected GenMF cache root")?;
    let (python_invocation_path, python_resolved_path, interpreter) =
        select_interpreter(request, cancellation)?;
    let generator = capture_input(&source_dir, &source_dir.join("tools/genmf/genmf.py"))?;
    let templates = template_closure(&source_dir)?;
    let files = find_mmakefiles(&source_dir).map_err(|error| {
        GenmfCacheError::input(format!("cannot discover GenMF inputs: {error}"))
    })?;
    if files.is_empty() {
        return Err(GenmfCacheError::input(
            "selected GenMF source tree contains no mmakefile or mmakefile.src",
        ));
    }
    let entries = files
        .into_iter()
        .map(|source_path| {
            select_entry(
                &source_dir,
                source_path,
                templates.as_slice(),
                generator.clone(),
                interpreter.clone(),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(GenmfCacheSelection {
        cache_dir,
        source_dir,
        python_invocation_path,
        python_resolved_path,
        templates,
        generator,
        interpreter,
        entries,
    })
}

/// List selected GenMF generation paths without hashing their contents.
///
/// # Errors
///
/// Returns an error for an invalid selection. Cache generation payloads and
/// receipts are not read by this operation.
pub fn list(request: &GenmfCacheRequest) -> Result<GenmfCacheList, GenmfCacheError> {
    let selection = select(request)?;
    let entries = selection
        .entries
        .iter()
        .cloned()
        .map(|entry| {
            let state = generation_state(&generation_path(&selection.cache_dir, &entry));
            GenmfCacheListEntry {
                selection: entry,
                state,
            }
        })
        .collect();
    Ok(GenmfCacheList {
        schema: GENMF_LIST_SCHEMA,
        operation: "genmf.list",
        side_effects: selection_side_effects(),
        selection,
        entries,
        boundary: "list hashes current source/template/generator/interpreter inputs and runs only a bounded selected-Python version probe; it reads no published expansion or receipt and creates no cache state",
    })
}

/// Verify every selected immutable GenMF generation.
///
/// # Errors
///
/// Returns an error when any selected final generation is absent, unsafe,
/// incomplete, altered, or bound to different current inputs.
pub fn verify(request: &GenmfCacheRequest) -> Result<GenmfCacheVerification, GenmfCacheError> {
    let selection = select(request)?;
    let entries = selection
        .entries
        .iter()
        .map(|entry| {
            verify_generation(
                &selection.cache_dir,
                entry,
                GENMF_VERIFY_SCHEMA,
                "genmf.verify",
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(GenmfCacheVerification {
        schema: GENMF_VERIFY_SCHEMA,
        operation: "genmf.verify",
        side_effects: verify_side_effects(),
        entries,
        boundary: "verify hashes current selection inputs, runs only a bounded selected-Python version probe, and reads final immutable GenMF generations; it never invokes GenMF, creates cache state, repairs or replaces an object",
    })
}

/// Reuse verified expansions and create only missing immutable generations.
///
/// Existing incomplete, unsafe, changed or otherwise invalid generations are
/// rejected instead of being repaired. Set `refresh` to run GenMF again and
/// prove each existing immutable generation still matches fresh output.
///
/// # Errors
///
/// Returns an error when the source selection itself is invalid. Per-input
/// generation errors are retained in the result so the verifier can report
/// every affected MMake file deterministically.
pub fn materialize(
    request: &GenmfCacheRequest,
    refresh: bool,
    cancellation: &CancellationToken,
) -> Result<GenmfCacheMaterialization, GenmfCacheError> {
    let selection = select_with_cancellation(request, cancellation)?;
    let entries = selection
        .entries
        .par_iter()
        .map(|entry| {
            let result =
                materialize_entry(&selection, entry, request.timeout, refresh, cancellation)
                    .map_err(|error| GenmfCacheMaterializationFailure {
                        message: error.to_string(),
                        timed_out: error.timed_out(),
                    });
            GenmfCacheMaterializedEntry {
                selection: entry.clone(),
                result,
            }
        })
        .collect();
    Ok(GenmfCacheMaterialization { selection, entries })
}

/// Refresh every selected GenMF expansion through its exact interpreter.
///
/// A refresh always invokes GenMF into a private temporary file. Existing
/// immutable entries must match that fresh output byte-for-byte; they are never
/// replaced. Missing entries are atomically published only after the complete
/// output and receipt have been measured.
///
/// # Errors
///
/// Returns an error on cancellation, deadline, source mutation, generator
/// failure, non-deterministic output, unsafe cache state or publication error.
pub fn refresh(
    request: &GenmfCacheRequest,
    cancellation: &CancellationToken,
) -> Result<GenmfCacheVerification, GenmfCacheError> {
    let selection = select_with_cancellation(request, cancellation)?;
    let results = selection
        .entries
        .par_iter()
        .map(|entry| refresh_entry(&selection, entry, request.timeout, cancellation))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(GenmfCacheVerification {
        schema: GENMF_REFRESH_SCHEMA,
        operation: "genmf.refresh",
        side_effects: refresh_side_effects(),
        entries: results,
        boundary: "refresh invokes only the selected resolved Python and upstream GenMF inputs in a private environment; it publishes a missing complete immutable generation or proves an existing generation is byte-identical",
    })
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GenmfGenerationReceipt {
    schema: String,
    identity: GenmfExpansionIdentity,
    output_sha256: String,
    output_size: u64,
}

fn checked_directory(path: &Path, subject: &str) -> Result<PathBuf, GenmfCacheError> {
    let root = resolve_explicit_root(path.to_path_buf())
        .map_err(|error| GenmfCacheError::input(error.to_string()))?;
    aros_common::validate_existing_directory_prefix_nofollow(&root.path).map_err(|_| {
        GenmfCacheError::input(format!(
            "{subject} has a symlinked, missing or unsafe component"
        ))
    })?;
    let metadata = fs::symlink_metadata(&root.path).map_err(|_| {
        GenmfCacheError::input(format!("{subject} does not exist or cannot be inspected"))
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(GenmfCacheError::input(format!(
            "{subject} must be an existing no-follow directory"
        )));
    }
    Ok(root.path)
}

fn select_interpreter(
    request: &GenmfCacheRequest,
    cancellation: &CancellationToken,
) -> Result<(PathBuf, PathBuf, GenmfInterpreterIdentity), GenmfCacheError> {
    if cancellation.is_cancelled() {
        return Err(GenmfCacheError::state("GenMF cache operation cancelled"));
    }
    if !request.python.is_absolute() {
        return Err(GenmfCacheError::input(
            "selected GenMF Python interpreter must be an absolute path",
        ));
    }
    let invocation = request.python.clone();
    let resolved = fs::canonicalize(&invocation).map_err(|_| {
        GenmfCacheError::input("selected GenMF Python interpreter cannot be resolved")
    })?;
    let (_, bytes) = measure_regular_file_bounded(&resolved, MAX_INTERPRETER_BYTES)
        .map_err(|_| {
            GenmfCacheError::input("selected GenMF Python interpreter is not a stable regular file")
        })?
        .ok_or_else(|| GenmfCacheError::input("selected GenMF Python interpreter is missing"))?;
    let mut command = Command::new(&resolved);
    command
        .arg("--version")
        .env_clear()
        .env("LC_ALL", "C")
        .env("PYTHONNOUSERSITE", "1")
        .env("PYTHONDONTWRITEBYTECODE", "1");
    let output = run_output_with_control(
        &mut command,
        DEFAULT_CAPTURE_LIMIT,
        request.timeout,
        cancellation,
    )
    .map_err(|_| GenmfCacheError::input("cannot execute selected GenMF Python interpreter"))?;
    if output.cancelled {
        return Err(GenmfCacheError::state("GenMF cache operation cancelled"));
    }
    if output.timed_out {
        return Err(GenmfCacheError::timeout(
            "selected GenMF Python interpreter exceeded its version-probe deadline",
        ));
    }
    if !output.status.success() {
        return Err(GenmfCacheError::input(format!(
            "selected GenMF Python interpreter cannot report a bounded successful version: {}",
            bounded_output_detail(&output.stdout, &output.stderr)
        )));
    }
    let version = normalized_python_version(&output)?;
    Ok((
        invocation,
        resolved,
        GenmfInterpreterIdentity {
            sha256: sha256_bytes(&bytes).to_string(),
            version,
        },
    ))
}

fn normalized_python_version(
    output: &aros_common::ProcessOutput,
) -> Result<String, GenmfCacheError> {
    let stdout = output.stdout.exact_bytes().ok_or_else(|| {
        GenmfCacheError::input("Python version output exceeded the capture limit")
    })?;
    let stderr = output.stderr.exact_bytes().ok_or_else(|| {
        GenmfCacheError::input("Python version output exceeded the capture limit")
    })?;
    let bytes = if stdout.is_empty() { stderr } else { stdout };
    let version = std::str::from_utf8(bytes)
        .map_err(|_| GenmfCacheError::input("Python version output is not UTF-8"))?
        .trim();
    if version.is_empty() || version.contains('\n') || version.len() > 256 {
        return Err(GenmfCacheError::input(
            "Python version output must be one bounded non-empty line",
        ));
    }
    Ok(version.to_owned())
}

fn template_closure(source_dir: &Path) -> Result<Vec<GenmfInputIdentity>, GenmfCacheError> {
    let mut discovered = BTreeMap::new();
    let mut pending = vec![source_dir.join("config/make.tmpl")];
    while let Some(path) = pending.pop() {
        let input = capture_input(source_dir, &path)?;
        if discovered.contains_key(&input.relative_path) {
            continue;
        }
        let (_, bytes) = measure_regular_file_bounded(&path, MAX_INPUT_BYTES)
            .map_err(|_| GenmfCacheError::input("cannot capture GenMF template bytes"))?
            .ok_or_else(|| GenmfCacheError::input("GenMF template disappeared during selection"))?;
        let text = std::str::from_utf8(&bytes)
            .map_err(|_| GenmfCacheError::input("GenMF template is not UTF-8"))?;
        discovered.insert(input.relative_path.clone(), input);
        let parent = path.parent().ok_or_else(|| {
            GenmfCacheError::input("GenMF template has no source-root-contained parent")
        })?;
        for include in template_includes(text) {
            pending.push(resolve_template_include(source_dir, parent, include)?);
        }
    }
    Ok(discovered.into_values().collect())
}

fn template_includes(text: &str) -> Vec<&str> {
    text.lines()
        .filter_map(|line| {
            let tail = line.strip_prefix("%include")?;
            tail.chars()
                .next()
                .filter(|character| character.is_whitespace())?;
            let include = tail.trim();
            let include = include
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
                .unwrap_or(include);
            (!include.is_empty()).then_some(include)
        })
        .collect()
}

fn resolve_template_include(
    source_dir: &Path,
    parent: &Path,
    include: &str,
) -> Result<PathBuf, GenmfCacheError> {
    let include = Path::new(include);
    if include.is_absolute()
        || include.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(GenmfCacheError::input(
            "GenMF template include must be a source-root-contained relative path",
        ));
    }
    let path = parent.join(include);
    let relative = path.strip_prefix(source_dir).map_err(|_| {
        GenmfCacheError::input("GenMF template include escapes the selected source root")
    })?;
    if relative.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(GenmfCacheError::input(
            "GenMF template include escapes the selected source root",
        ));
    }
    Ok(path)
}

fn select_entry(
    source_dir: &Path,
    source_path: PathBuf,
    templates: &[GenmfInputIdentity],
    generator: GenmfInputIdentity,
    interpreter: GenmfInterpreterIdentity,
) -> Result<GenmfCacheEntrySelection, GenmfCacheError> {
    let source = capture_input(source_dir, &source_path)?;
    let identity = GenmfExpansionIdentity {
        schema: "aros-genmf-expansion-identity-v1".to_owned(),
        format: "genmf-expansion-v1".to_owned(),
        source: source.clone(),
        templates: templates.to_vec(),
        generator,
        interpreter,
        options: vec!["template-source-output-v1".to_owned()],
    };
    let canonical = serde_json::to_vec(&identity)
        .map_err(|_| GenmfCacheError::state("cannot encode GenMF expansion identity"))?;
    Ok(GenmfCacheEntrySelection {
        generation: sha256_bytes(&canonical).to_string(),
        source_relative_path: source.relative_path,
        source_path,
        identity,
    })
}

fn capture_input(source_dir: &Path, path: &Path) -> Result<GenmfInputIdentity, GenmfCacheError> {
    let relative_path = portable_relative_path(source_dir, path)?;
    let (_, bytes) = measure_regular_file_bounded(path, MAX_INPUT_BYTES)
        .map_err(|_| {
            GenmfCacheError::input(format!(
                "GenMF input '{relative_path}' is not a stable no-follow regular file"
            ))
        })?
        .ok_or_else(|| {
            GenmfCacheError::input(format!("GenMF input '{relative_path}' is missing"))
        })?;
    Ok(GenmfInputIdentity {
        relative_path,
        sha256: sha256_bytes(&bytes).to_string(),
    })
}

fn portable_relative_path(source_dir: &Path, path: &Path) -> Result<String, GenmfCacheError> {
    let relative = path
        .strip_prefix(source_dir)
        .map_err(|_| GenmfCacheError::input("GenMF input escapes the selected source root"))?;
    let mut parts = Vec::new();
    for component in relative.components() {
        let Component::Normal(part) = component else {
            return Err(GenmfCacheError::input(
                "GenMF input path must be normalized beneath the selected source root",
            ));
        };
        let part = part
            .to_str()
            .ok_or_else(|| GenmfCacheError::input("GenMF input path is not UTF-8"))?;
        if part.is_empty() || part.contains(['/', '\\']) {
            return Err(GenmfCacheError::input("GenMF input path is not portable"));
        }
        parts.push(part);
    }
    if parts.is_empty() {
        return Err(GenmfCacheError::input(
            "GenMF input cannot be the selected source root",
        ));
    }
    Ok(parts.join("/"))
}

fn generation_path(cache_dir: &Path, entry: &GenmfCacheEntrySelection) -> PathBuf {
    cache_dir
        .join(CACHE_NAMESPACE)
        .join(CACHE_VERSION)
        .join(&entry.generation)
}

fn generation_state(path: &Path) -> GenmfCacheEntryState {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            GenmfCacheEntryState::Unsafe
        }
        Ok(_) => GenmfCacheEntryState::PresentUnverified,
        Err(error) if error.kind() == ErrorKind::NotFound => GenmfCacheEntryState::Missing,
        Err(_) => GenmfCacheEntryState::Inaccessible,
    }
}

fn verify_generation(
    cache_dir: &Path,
    entry: &GenmfCacheEntrySelection,
    schema: &'static str,
    operation: &'static str,
) -> Result<GenmfCacheGeneration, GenmfCacheError> {
    let generation_dir = generation_path(cache_dir, entry);
    let names = directory_entry_names_nofollow_bounded(&generation_dir, MAX_GENERATION_ENTRIES)
        .map_err(|_| {
            GenmfCacheError::state(
                "selected GenMF generation is missing, unsafe, incomplete or changed",
            )
        })?;
    let names = names
        .into_iter()
        .map(|name| {
            name.into_string().map_err(|_| {
                GenmfCacheError::state("selected GenMF generation has a non-UTF-8 member")
            })
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    let expected = BTreeSet::from([EXPANSION_FILE.to_owned(), RECEIPT_FILE.to_owned()]);
    if names != expected {
        return Err(GenmfCacheError::state(
            "selected GenMF generation does not contain exactly expansion.mk and receipt.json",
        ));
    }
    let (_, receipt_bytes) =
        measure_regular_file_bounded(&generation_dir.join(RECEIPT_FILE), MAX_RECEIPT_BYTES)
            .map_err(|_| {
                GenmfCacheError::state("cannot safely read selected GenMF generation receipt")
            })?
            .ok_or_else(|| {
                GenmfCacheError::state("selected GenMF generation receipt is missing")
            })?;
    let receipt: GenmfGenerationReceipt = serde_json::from_slice(&receipt_bytes)
        .map_err(|_| GenmfCacheError::state("selected GenMF generation receipt is malformed"))?;
    if receipt.schema != GENMF_RECEIPT_SCHEMA || receipt.identity != entry.identity {
        return Err(GenmfCacheError::state(
            "selected GenMF generation receipt does not match the current exact input identity",
        ));
    }
    let (_, output) =
        measure_regular_file_bounded(&generation_dir.join(EXPANSION_FILE), MAX_EXPANSION_BYTES)
            .map_err(|_| GenmfCacheError::state("cannot safely read selected GenMF expansion"))?
            .ok_or_else(|| GenmfCacheError::state("selected GenMF expansion is missing"))?;
    let output_sha256 = sha256_bytes(&output).to_string();
    let output_size = output.len() as u64;
    if receipt.output_sha256 != output_sha256 || receipt.output_size != output_size {
        return Err(GenmfCacheError::state(
            "selected GenMF generation output disagrees with its receipt",
        ));
    }
    Ok(GenmfCacheGeneration {
        schema,
        operation,
        selection: entry.clone(),
        generation_dir,
        output_sha256,
        output_size,
    })
}

fn refresh_entry(
    selection: &GenmfCacheSelection,
    entry: &GenmfCacheEntrySelection,
    timeout: Duration,
    cancellation: &CancellationToken,
) -> Result<GenmfCacheGeneration, GenmfCacheError> {
    let parent = selection
        .cache_dir
        .join(CACHE_NAMESPACE)
        .join(CACHE_VERSION);
    ensure_directory_nofollow(&parent).map_err(|_| {
        GenmfCacheError::state("cannot create or validate the GenMF cache namespace")
    })?;
    let lock_path = parent.join(format!("{}.lock", entry.generation));
    let deadline = Instant::now() + timeout;
    let _lock = acquire_generation_lock(&lock_path, deadline, cancellation)?;
    let final_path = generation_path(&selection.cache_dir, entry);
    let output = generate_output(selection, entry, deadline, cancellation)?;
    assert_inputs_stable(selection, entry)?;
    match generation_state(&final_path) {
        GenmfCacheEntryState::Missing => publish_generation(&final_path, entry, &output)?,
        GenmfCacheEntryState::PresentUnverified => {
            let existing = verify_generation(
                &selection.cache_dir,
                entry,
                GENMF_REFRESH_SCHEMA,
                "genmf.refresh",
            )?;
            if existing.output_sha256 != sha256_bytes(&output).to_string()
                || existing.output_size != output.len() as u64
            {
                return Err(GenmfCacheError::state(
                    "fresh GenMF output differs from the existing immutable generation",
                ));
            }
        }
        GenmfCacheEntryState::Unsafe | GenmfCacheEntryState::Inaccessible => {
            return Err(GenmfCacheError::state(
                "selected GenMF final generation is unsafe or inaccessible and cannot be replaced",
            ));
        }
    }
    verify_generation(
        &selection.cache_dir,
        entry,
        GENMF_REFRESH_SCHEMA,
        "genmf.refresh",
    )
}

fn materialize_entry(
    selection: &GenmfCacheSelection,
    entry: &GenmfCacheEntrySelection,
    timeout: Duration,
    refresh: bool,
    cancellation: &CancellationToken,
) -> Result<GenmfCacheGeneration, GenmfCacheError> {
    if refresh
        || generation_state(&generation_path(&selection.cache_dir, entry))
            == GenmfCacheEntryState::Missing
    {
        return refresh_entry(selection, entry, timeout, cancellation);
    }
    verify_generation(
        &selection.cache_dir,
        entry,
        GENMF_VERIFY_SCHEMA,
        "genmf.materialize",
    )
}

fn acquire_generation_lock(
    lock_path: &Path,
    deadline: Instant,
    cancellation: &CancellationToken,
) -> Result<AdvisoryFileLock, GenmfCacheError> {
    loop {
        if cancellation.is_cancelled() {
            return Err(GenmfCacheError::state(
                "GenMF cache refresh cancelled while waiting for its generation lock",
            ));
        }
        match AdvisoryFileLock::acquire(lock_path) {
            Ok(lock) => return Ok(lock),
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(GenmfCacheError::timeout(
                        "GenMF cache refresh exceeded its generation-lock deadline",
                    ));
                }
                thread::sleep(Duration::from_millis(25));
            }
            Err(_) => {
                return Err(GenmfCacheError::state(
                    "cannot acquire GenMF cache generation lock",
                ))
            }
        }
    }
}

fn generate_output(
    selection: &GenmfCacheSelection,
    entry: &GenmfCacheEntrySelection,
    deadline: Instant,
    cancellation: &CancellationToken,
) -> Result<Vec<u8>, GenmfCacheError> {
    if cancellation.is_cancelled() {
        return Err(GenmfCacheError::state("GenMF cache refresh cancelled"));
    }
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .ok_or_else(|| {
            GenmfCacheError::timeout("GenMF cache refresh exceeded its deadline before invocation")
        })?;
    let temporary = tempfile::Builder::new()
        .prefix("aros-genmf-cache-")
        .tempdir()
        .map_err(|_| GenmfCacheError::state("cannot create private GenMF refresh directory"))?;
    let output = temporary.path().join(EXPANSION_FILE);
    let template = selection.source_dir.join("config/make.tmpl");
    let mut command = Command::new(&selection.python_resolved_path);
    command
        .arg(selection.source_dir.join("tools/genmf/genmf.py"))
        .arg(template)
        .arg(&entry.source_path)
        .arg(&output)
        .env_clear()
        .env("LC_ALL", "C")
        .env("PYTHONNOUSERSITE", "1")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("HOME", temporary.path())
        .env("TMPDIR", temporary.path());
    let result =
        run_output_with_control(&mut command, DEFAULT_CAPTURE_LIMIT, remaining, cancellation)
            .map_err(|_| GenmfCacheError::state("cannot execute selected GenMF generator"))?;
    if result.cancelled {
        return Err(GenmfCacheError::state("GenMF cache refresh cancelled"));
    }
    if result.timed_out {
        return Err(GenmfCacheError::timeout(
            "GenMF cache refresh exceeded its invocation deadline",
        ));
    }
    if !result.status.success() {
        return Err(GenmfCacheError::state(format!(
            "genmf exited with {}: {}",
            result.status,
            bounded_output_detail(&result.stdout, &result.stderr)
        )));
    }
    let (_, bytes) = measure_regular_file_bounded(&output, MAX_EXPANSION_BYTES)
        .map_err(|_| {
            GenmfCacheError::state("GenMF succeeded without a stable bounded output file")
        })?
        .ok_or_else(|| {
            GenmfCacheError::state("GenMF succeeded without producing an output file")
        })?;
    Ok(bytes)
}

fn assert_inputs_stable(
    selection: &GenmfCacheSelection,
    entry: &GenmfCacheEntrySelection,
) -> Result<(), GenmfCacheError> {
    let source = capture_input(&selection.source_dir, &entry.source_path)?;
    let generator = capture_input(
        &selection.source_dir,
        &selection.source_dir.join("tools/genmf/genmf.py"),
    )?;
    let templates = template_closure(&selection.source_dir)?;
    let (_, interpreter_bytes) =
        measure_regular_file_bounded(&selection.python_resolved_path, MAX_INTERPRETER_BYTES)
            .map_err(|_| {
                GenmfCacheError::state("selected GenMF Python interpreter changed during refresh")
            })?
            .ok_or_else(|| {
                GenmfCacheError::state(
                    "selected GenMF Python interpreter disappeared during refresh",
                )
            })?;
    if source != entry.identity.source
        || generator != entry.identity.generator
        || templates != entry.identity.templates
        || sha256_bytes(&interpreter_bytes).to_string() != entry.identity.interpreter.sha256
    {
        return Err(GenmfCacheError::state(
            "GenMF source, template closure, generator or interpreter changed during refresh; no generation was published",
        ));
    }
    Ok(())
}

fn publish_generation(
    final_path: &Path,
    entry: &GenmfCacheEntrySelection,
    output: &[u8],
) -> Result<(), GenmfCacheError> {
    let receipt = GenmfGenerationReceipt {
        schema: GENMF_RECEIPT_SCHEMA.to_owned(),
        identity: entry.identity.clone(),
        output_sha256: sha256_bytes(output).to_string(),
        output_size: output.len() as u64,
    };
    let receipt = serde_json::to_vec_pretty(&receipt)
        .map_err(|_| GenmfCacheError::state("cannot encode GenMF cache receipt"))?;
    let expansion_name = PortableOutputName::new(EXPANSION_FILE)
        .map_err(|_| GenmfCacheError::state("invalid fixed GenMF expansion output name"))?;
    let receipt_name = PortableOutputName::new(RECEIPT_FILE)
        .map_err(|_| GenmfCacheError::state("invalid fixed GenMF receipt output name"))?;
    match publish_flat_tree_noclobber(
        final_path,
        &[(expansion_name, output), (receipt_name, &receipt)],
    ) {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => Ok(()),
        Err(_) => Err(GenmfCacheError::state(
            "cannot atomically publish the complete GenMF cache generation",
        )),
    }
}

const fn passive_side_effects() -> CacheSideEffects {
    CacheSideEffects {
        creates_state: false,
        mutates_state: false,
        network: false,
        backend_process: false,
        locks: false,
        hashes_payloads: false,
    }
}

const fn selection_side_effects() -> CacheSideEffects {
    CacheSideEffects {
        creates_state: false,
        mutates_state: false,
        network: false,
        backend_process: false,
        locks: false,
        hashes_payloads: true,
    }
}

const fn verify_side_effects() -> CacheSideEffects {
    CacheSideEffects {
        creates_state: false,
        mutates_state: false,
        network: false,
        backend_process: false,
        locks: false,
        hashes_payloads: true,
    }
}

const fn refresh_side_effects() -> CacheSideEffects {
    CacheSideEffects {
        creates_state: true,
        mutates_state: true,
        network: false,
        backend_process: false,
        locks: true,
        hashes_payloads: true,
    }
}
