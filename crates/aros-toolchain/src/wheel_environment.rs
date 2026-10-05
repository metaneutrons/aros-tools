//! Rust-owned preparation of a byte-locked, offline vendor Python environment.
//!
//! This is separate from the AROS Mako source environment. Upstream CPython
//! and pip implement venv creation and wheel installation; Rust owns input
//! binding, snapshots, argv, the sterile environment, supervision and receipts.
//! No activation helper, resolver download or pre-existing venv is adopted.
//! Local byte locks do not authenticate distribution origins or authorize a
//! release. The observed host runtime still depends on its platform loader.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use aros_common::{
    create_unique_directory_nofollow, measure_regular_file_bounded,
    measure_tree_content_cas_bounded, run_output_with_input_and_control, sha256_bytes,
    validate_private_directory_nofollow, CancellationToken, PortableOutputName, Sha256Digest,
    TreeContentCas, TreeTraversalLimits,
};
use aros_fetch::engine::cache::{snapshot_verified_cache_payload, VerifiedCachePayload};
use serde::{Deserialize, Serialize};

use crate::ContractError;

const MAX_LOCK_BYTES: usize = 1024 * 1024;
const MAX_WHEEL_BYTES: u64 = 128 * 1024 * 1024;
const MAX_TOTAL_WHEEL_BYTES: u64 = 1024 * 1024 * 1024;
const CAPTURE_LIMIT: usize = 256 * 1024;
const RUNTIME_PROBE: &str = "import sys,json; print(json.dumps({'version':sys.version.split()[0],'implementation':sys.implementation.name,'base_prefix':sys.base_prefix}))";
const INSTALLED_PROBE: &str = "import importlib.metadata,json,sys; print(json.dumps({'prefix':sys.prefix,'base_prefix':sys.base_prefix,'import_paths':sys.path,'packages':[{'name':d.metadata['Name'],'version':d.version,'root':str(d.locate_file(''))} for d in importlib.metadata.distributions()]}))";

/// Exact local wheel bytes; origin/provenance review is a separate contract.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LockedWheel {
    pub name: String,
    pub version: String,
    pub filename: String,
    pub sha256: String,
    pub size_bytes: u64,
}

/// Exact observed CPython executable and prefix content, including link targets.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WheelRuntimePin {
    pub version: String,
    pub executable_sha256: String,
    pub prefix_tree_sha256: String,
}

/// Closed offline wheel preparation contract. It deliberately has no commands.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WheelEnvironmentLock {
    pub schema_version: u32,
    pub format: String,
    pub qualification: String,
    pub host: String,
    pub runtime: WheelRuntimePin,
    pub bootstrap_filename: String,
    pub wheels: Vec<LockedWheel>,
}

/// Validated raw-byte-bound lock; cannot be constructed without validation.
#[derive(Clone, Debug)]
pub struct BoundWheelEnvironmentLock {
    lock: WheelEnvironmentLock,
    sha256: Sha256Digest,
    raw_bytes: Vec<u8>,
}

/// Bind the supplied raw JSON to an explicit expected digest.
///
/// # Errors
/// Refuses unknown fields, malformed/duplicate identities, unsafe filenames,
/// missing pip bootstrap, unsupported host, and resource-limit violations.
pub fn bind_wheel_environment_lock(
    bytes: &[u8],
    expected_sha256: &Sha256Digest,
) -> Result<BoundWheelEnvironmentLock, ContractError> {
    if bytes.len() > MAX_LOCK_BYTES || sha256_bytes(bytes) != *expected_sha256 {
        return Err(ContractError::environment(
            "wheel lock raw-byte binding failed",
        ));
    }
    let lock: WheelEnvironmentLock = serde_json::from_slice(bytes)
        .map_err(|_| ContractError::environment("invalid closed wheel lock schema"))?;
    validate_lock(&lock)?;
    Ok(BoundWheelEnvironmentLock {
        lock,
        sha256: expected_sha256.clone(),
        raw_bytes: bytes.to_vec(),
    })
}

/// Explicit local preparation inputs. No PATH discovery or network bootstrap.
pub struct WheelEnvironmentRequest<'a> {
    pub lock: &'a BoundWheelEnvironmentLock,
    pub interpreter: &'a Path,
    pub runtime_prefix: &'a Path,
    pub wheel_cache: &'a Path,
    /// Existing private parent; each attempt reserves a new retained child.
    pub work_parent: &'a Path,
    pub timeout: Duration,
    pub cancellation: &'a CancellationToken,
}

/// A measured local environment, not release provenance or an IDF build receipt.
#[derive(Clone, Debug, Serialize)]
pub struct WheelEnvironmentReceipt {
    pub schema_version: u32,
    pub format: String,
    pub qualification: String,
    pub lock_sha256: Sha256Digest,
    pub host_runtime: WheelRuntimePin,
    pub interpreter: PathBuf,
    pub runtime_prefix: PathBuf,
    pub environment_root: PathBuf,
    pub environment_tree_sha256: Sha256Digest,
    pub phases: Vec<WheelPhaseReceipt>,
    pub wheels: Vec<LockedWheel>,
}

/// Durable subprocess evidence, including exact controlled environment/argv.
#[derive(Clone, Debug, Serialize)]
pub struct WheelPhaseReceipt {
    pub phase: String,
    pub program: PathBuf,
    pub arguments: Vec<String>,
    pub environment: BTreeMap<String, String>,
    pub elapsed_milliseconds: u128,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub cancelled: bool,
    pub stdout_sha256: Sha256Digest,
    pub stderr_sha256: Sha256Digest,
    pub stdout_omitted_bytes: u64,
    pub stderr_omitted_bytes: u64,
}

/// Retained output paths for subsequent explicitly controlled vendor commands.
#[derive(Debug)]
pub struct PreparedWheelEnvironment {
    pub work_root: PathBuf,
    pub interpreter: PathBuf,
    pub receipt: WheelEnvironmentReceipt,
    binding: PreparedWheelBinding,
}

#[derive(Debug)]
struct PreparedWheelBinding {
    work_root: PathBuf,
    interpreter: PathBuf,
    runtime_prefix: PathBuf,
    runtime: TreeContentCas,
    environment: TreeContentCas,
    receipt_sha256: Sha256Digest,
    lock_sha256: Sha256Digest,
}

impl PreparedWheelEnvironment {
    /// Revalidate this preparation before and after a later vendor subprocess.
    ///
    /// # Errors
    /// Refuses substituted paths/receipts, changed package bytes or host runtime.
    /// This does not authorize an arbitrary script, network transfer or build.
    pub fn revalidate(&self) -> Result<(), ContractError> {
        require_private_root(&self.binding.work_root)?;
        if self.work_root != self.binding.work_root
            || self.interpreter != self.binding.interpreter
            || regular_digest(
                &self.work_root.join("environment.receipt.json"),
                MAX_LOCK_BYTES as u64,
            )? != self.binding.receipt_sha256
            || regular_digest(
                &self.work_root.join("selected-inputs.json"),
                MAX_LOCK_BYTES as u64,
            )? != self.binding.lock_sha256
            || sha256_bytes(&serde_json::to_vec_pretty(&self.receipt).map_err(json_error)?)
                != self.binding.receipt_sha256
            || measure_runtime(&self.receipt.environment_root)? != self.binding.environment
            || measure_runtime(&self.binding.runtime_prefix)? != self.binding.runtime
        {
            return Err(ContractError::environment(
                "prepared wheel environment binding changed",
            ));
        }
        Ok(())
    }
}

/// Create, install and verify a fresh isolated environment entirely offline.
///
/// # Errors
/// Any failure retains the newly owned root and logs. Existing roots, caches,
/// interpreter files and host Python installations are never overwritten.
pub fn prepare_wheel_environment(
    request: &WheelEnvironmentRequest<'_>,
) -> Result<PreparedWheelEnvironment, ContractError> {
    if request.timeout.is_zero() || request.cancellation.is_cancelled() {
        return Err(ContractError::environment(
            "wheel preparation requires a live positive deadline",
        ));
    }
    for root in [
        request.interpreter,
        request.runtime_prefix,
        request.wheel_cache,
        request.work_parent,
    ] {
        if !root.is_absolute() {
            return Err(ContractError::environment(
                "wheel preparation roots must be absolute",
            ));
        }
    }
    validate_private_directory_nofollow(request.work_parent).map_err(|_| {
        ContractError::environment("wheel work parent is not a private no-follow directory")
    })?;
    aros_common::directory_entry_names_nofollow_bounded(request.wheel_cache, 4096).map_err(
        |_| ContractError::environment("wheel cache is not a bounded no-follow directory"),
    )?;
    let work_parent = request.work_parent.canonicalize().map_err(io_error)?;
    let wheel_cache = request.wheel_cache.canonicalize().map_err(io_error)?;
    let interpreter = request
        .interpreter
        .canonicalize()
        .map_err(|_| ContractError::environment("selected CPython executable is missing"))?;
    let runtime_prefix = request
        .runtime_prefix
        .canonicalize()
        .map_err(|_| ContractError::environment("selected CPython prefix is missing"))?;
    if !interpreter.starts_with(&runtime_prefix)
        || work_parent.starts_with(&runtime_prefix)
        || runtime_prefix.starts_with(&work_parent)
        || work_parent.starts_with(&wheel_cache)
        || wheel_cache.starts_with(&work_parent)
    {
        return Err(ContractError::environment(
            "wheel inputs and owned work parent must be disjoint",
        ));
    }
    let deadline = Instant::now()
        .checked_add(request.timeout)
        .ok_or_else(|| ContractError::environment("wheel deadline overflow"))?;
    verify_interpreter(&interpreter, &request.lock.lock.runtime)?;
    let runtime_before = measure_runtime(&runtime_prefix)?;
    verify_runtime_prefix_pin(&runtime_before, &request.lock.lock.runtime)?;
    // Verify every cache object before reserving an executable output root.
    let snapshots = request
        .lock
        .lock
        .wheels
        .iter()
        .map(|wheel| {
            let digest = Sha256Digest::parse(&wheel.sha256)
                .map_err(|_| ContractError::environment("invalid wheel digest"))?;
            snapshot_verified_cache_payload(
                request.wheel_cache,
                &wheel.filename,
                wheel.size_bytes,
                &digest,
            )
            .map_err(|_| {
                ContractError::environment(format!(
                    "locked wheel '{}' is absent, unsafe or changed",
                    wheel.filename
                ))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let root = create_unique_directory_nofollow(request.work_parent, "aros-wheel-environment")
        .map_err(|_| ContractError::environment("cannot reserve private wheel preparation root"))?;
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).map_err(io_error)?;
    require_private_root(&root)?;
    let result = prepare_owned(
        request,
        &interpreter,
        &runtime_prefix,
        &runtime_before,
        &snapshots,
        &root,
        deadline,
    );
    result.map_err(ContractError::retained_material)
}

fn prepare_owned(
    request: &WheelEnvironmentRequest<'_>,
    interpreter: &Path,
    runtime_prefix: &Path,
    runtime_before: &TreeContentCas,
    snapshots: &[VerifiedCachePayload],
    root: &Path,
    deadline: Instant,
) -> Result<PreparedWheelEnvironment, ContractError> {
    let wheelhouse = root.join("wheelhouse");
    fs::create_dir(&wheelhouse).map_err(io_error)?;
    fs::create_dir(root.join("tmp")).map_err(io_error)?;
    fs::create_dir(root.join("logs")).map_err(io_error)?;
    let mut requirements = String::new();
    for (wheel, snapshot) in request.lock.lock.wheels.iter().zip(snapshots) {
        let mut input =
            aros_common::open_regular_file_nofollow(snapshot.path()).map_err(io_error)?;
        let mut output = new_file(&wheelhouse.join(&wheel.filename))?;
        std::io::copy(&mut input, &mut output).map_err(io_error)?;
        output.sync_all().map_err(io_error)?;
        writeln!(
            requirements,
            "wheelhouse/{} --hash=sha256:{}",
            wheel.filename, wheel.sha256
        )
        .map_err(|_| ContractError::environment("cannot render wheel requirements"))?;
    }
    write_new(&root.join("requirements.txt"), requirements.as_bytes())?;
    write_new(&root.join("selected-inputs.json"), &request.lock.raw_bytes)?;
    let mut phases = Vec::new();
    let mut environment = base_environment(root);
    let observed = run_phase(
        root,
        "host-runtime",
        interpreter,
        &[
            "-I".into(),
            "-B".into(),
            "-S".into(),
            "-c".into(),
            RUNTIME_PROBE.into(),
        ],
        &environment,
        deadline,
        request.cancellation,
        &mut phases,
    )?;
    let observed: serde_json::Value = serde_json::from_slice(&observed).map_err(json_error)?;
    if observed["version"].as_str() != Some(request.lock.lock.runtime.version.as_str())
        || observed["implementation"] != "cpython"
        || observed["base_prefix"]
            .as_str()
            .map(Path::new)
            .and_then(|p| p.canonicalize().ok())
            .as_deref()
            != Some(runtime_prefix)
    {
        return Err(ContractError::environment(
            "selected runtime probe differs from the lock/prefix",
        ));
    }
    let venv = root.join("venv");
    run_phase(
        root,
        "create-venv",
        interpreter,
        &[
            "-I".into(),
            "-B".into(),
            "-m".into(),
            "venv".into(),
            "--copies".into(),
            "--without-pip".into(),
            utf8(&venv)?,
        ],
        &environment,
        deadline,
        request.cancellation,
        &mut phases,
    )?;
    let python = venv.join("bin/python");
    verify_interpreter(&python, &request.lock.lock.runtime)?;
    verify_venv_configuration(&venv)?;
    environment.insert(
        "PYTHONPATH".into(),
        utf8(&wheelhouse.join(&request.lock.lock.bootstrap_filename))?,
    );
    run_phase(
        root,
        "install-wheels",
        &python,
        &[
            "-B".into(),
            "-s".into(),
            "-P".into(),
            "-m".into(),
            "pip".into(),
            "install".into(),
            "--no-index".into(),
            "--no-cache-dir".into(),
            "--disable-pip-version-check".into(),
            "--require-hashes".into(),
            "--only-binary=:all:".into(),
            "--no-compile".into(),
            "--no-deps".into(),
            "--no-input".into(),
            "--report".into(),
            utf8(&root.join("install-report.json"))?,
            "--requirement".into(),
            utf8(&root.join("requirements.txt"))?,
        ],
        &environment,
        deadline,
        request.cancellation,
        &mut phases,
    )?;
    environment.remove("PYTHONPATH");
    run_phase(
        root,
        "check-dependencies",
        &python,
        &[
            "-I".into(),
            "-B".into(),
            "-m".into(),
            "pip".into(),
            "check".into(),
        ],
        &environment,
        deadline,
        request.cancellation,
        &mut phases,
    )?;
    let installed = run_phase(
        root,
        "installed-inventory",
        &python,
        &[
            "-I".into(),
            "-B".into(),
            "-c".into(),
            INSTALLED_PROBE.into(),
        ],
        &environment,
        deadline,
        request.cancellation,
        &mut phases,
    )?;
    verify_installed(&installed, &venv, runtime_prefix, &request.lock.lock)?;
    verify_install_report(
        &root.join("install-report.json"),
        &wheelhouse,
        &request.lock.lock,
    )?;
    for (wheel, snapshot) in request.lock.lock.wheels.iter().zip(snapshots) {
        snapshot
            .revalidate()
            .map_err(|_| ContractError::environment("wheel cache changed during preparation"))?;
        let bytes = regular_bytes(&wheelhouse.join(&wheel.filename), wheel.size_bytes)?;
        if bytes.len() as u64 != wheel.size_bytes
            || sha256_bytes(&bytes).to_string() != wheel.sha256
        {
            return Err(ContractError::environment(
                "owned wheel snapshot changed during installation",
            ));
        }
    }
    verify_interpreter(interpreter, &request.lock.lock.runtime)?;
    if measure_runtime(runtime_prefix)? != *runtime_before {
        return Err(ContractError::environment(
            "host Python runtime changed during preparation",
        ));
    }
    if Instant::now() >= deadline || request.cancellation.is_cancelled() {
        return Err(ContractError::environment(
            "wheel preparation cancelled or deadline exceeded during revalidation",
        ));
    }
    let environment_snapshot = measure_runtime(&venv)?;
    let receipt = WheelEnvironmentReceipt {
        schema_version: 1,
        format: "aros-python-wheel-environment-receipt-v1".into(),
        qualification: "local-byte-lock-only".into(),
        lock_sha256: request.lock.sha256.clone(),
        host_runtime: request.lock.lock.runtime.clone(),
        interpreter: interpreter.to_owned(),
        runtime_prefix: runtime_prefix.to_owned(),
        environment_root: venv.clone(),
        environment_tree_sha256: environment_snapshot.payload_digest_excluding(None),
        phases,
        wheels: request.lock.lock.wheels.clone(),
    };
    let receipt_bytes = serde_json::to_vec_pretty(&receipt).map_err(json_error)?;
    write_new(&root.join("environment.receipt.json"), &receipt_bytes)?;
    Ok(PreparedWheelEnvironment {
        work_root: root.to_owned(),
        interpreter: python,
        receipt,
        binding: PreparedWheelBinding {
            work_root: root.to_owned(),
            interpreter: venv.join("bin/python"),
            runtime_prefix: runtime_prefix.to_owned(),
            runtime: runtime_before.clone(),
            environment: environment_snapshot,
            receipt_sha256: sha256_bytes(&receipt_bytes),
            lock_sha256: request.lock.sha256.clone(),
        },
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_phase(
    root: &Path,
    phase: &str,
    program: &Path,
    arguments: &[String],
    environment: &BTreeMap<String, String>,
    deadline: Instant,
    cancellation: &CancellationToken,
    phases: &mut Vec<WheelPhaseReceipt>,
) -> Result<Vec<u8>, ContractError> {
    require_private_root(root)?;
    let timeout = deadline
        .checked_duration_since(Instant::now())
        .filter(|value| !value.is_zero())
        .ok_or_else(|| ContractError::environment("wheel preparation deadline expired"))?;
    let mut command = Command::new(program);
    command
        .args(arguments)
        .current_dir(root)
        .env_clear()
        .envs(environment);
    let output =
        run_output_with_input_and_control(&mut command, b"", CAPTURE_LIMIT, timeout, cancellation)
            .map_err(|_| {
                ContractError::environment(format!("cannot supervise wheel phase '{phase}'"))
            })?;
    require_private_root(root)?;
    let stdout = output.stdout.rendered_lossy().into_bytes();
    let stderr = output.stderr.rendered_lossy().into_bytes();
    write_new(&root.join(format!("logs/{phase}.stdout.log")), &stdout)?;
    write_new(&root.join(format!("logs/{phase}.stderr.log")), &stderr)?;
    let receipt = WheelPhaseReceipt {
        phase: phase.into(),
        program: program.to_owned(),
        arguments: arguments.to_vec(),
        environment: environment.clone(),
        elapsed_milliseconds: output.elapsed.as_millis(),
        exit_code: output.status.code(),
        timed_out: output.timed_out,
        cancelled: output.cancelled,
        stdout_sha256: sha256_bytes(&stdout),
        stderr_sha256: sha256_bytes(&stderr),
        stdout_omitted_bytes: output.stdout.omitted_bytes(),
        stderr_omitted_bytes: output.stderr.omitted_bytes(),
    };
    write_new(
        &root.join(format!("logs/{phase}.receipt.json")),
        &serde_json::to_vec_pretty(&receipt).map_err(json_error)?,
    )?;
    phases.push(receipt);
    if !output.status.success() || output.timed_out || output.cancelled {
        return Err(ContractError::environment(format!(
            "wheel phase '{phase}' failed; inspect retained logs"
        )));
    }
    output
        .stdout
        .exact_bytes()
        .map(<[u8]>::to_vec)
        .ok_or_else(|| {
            ContractError::environment(format!("wheel phase '{phase}' exceeded its output limit"))
        })
}

pub(crate) fn base_environment(root: &Path) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("PATH".into(), "/usr/bin:/bin".into()),
        ("LC_ALL".into(), "C".into()),
        ("LANG".into(), "C".into()),
        ("TZ".into(), "UTC".into()),
        (
            "TMPDIR".into(),
            root.join("tmp").to_string_lossy().into_owned(),
        ),
        ("PYTHONNOUSERSITE".into(), "1".into()),
        ("PYTHONDONTWRITEBYTECODE".into(), "1".into()),
        ("PYTHONHASHSEED".into(), "0".into()),
        ("PIP_CONFIG_FILE".into(), "/dev/null".into()),
        ("PIP_NO_INDEX".into(), "1".into()),
        ("PIP_DISABLE_PIP_VERSION_CHECK".into(), "1".into()),
    ])
}

fn verify_installed(
    bytes: &[u8],
    venv: &Path,
    prefix: &Path,
    lock: &WheelEnvironmentLock,
) -> Result<(), ContractError> {
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(json_error)?;
    if value["prefix"].as_str().map(Path::new) != Some(venv)
        || value["base_prefix"]
            .as_str()
            .map(Path::new)
            .and_then(|p| p.canonicalize().ok())
            .as_deref()
            != Some(prefix)
        || venv == prefix
    {
        return Err(ContractError::environment(
            "installed runtime is not the fresh isolated environment",
        ));
    }
    let import_paths = value["import_paths"]
        .as_array()
        .ok_or_else(|| ContractError::environment("missing isolated import path inventory"))?;
    for entry in import_paths {
        let path = entry
            .as_str()
            .map(Path::new)
            .filter(|path| path.is_absolute())
            .ok_or_else(|| ContractError::environment("unsafe interpreter import path"))?;
        let physical = if path.exists() {
            path.canonicalize().map_err(io_error)?
        } else {
            path.to_owned()
        };
        if !physical.starts_with(venv) && !physical.starts_with(prefix) {
            return Err(ContractError::environment(
                "interpreter imports outside the bound runtime and private environment",
            ));
        }
    }
    let packages = value["packages"]
        .as_array()
        .ok_or_else(|| ContractError::environment("missing installed package inventory"))?;
    let mut measured = BTreeMap::new();
    for package in packages {
        let name = package["name"]
            .as_str()
            .and_then(normalized_name)
            .ok_or_else(|| ContractError::environment("invalid installed package name"))?;
        let version = package["version"]
            .as_str()
            .ok_or_else(|| ContractError::environment("missing installed package version"))?;
        let package_root = package["root"]
            .as_str()
            .map(Path::new)
            .filter(|path| path.is_absolute())
            .and_then(|path| path.canonicalize().ok())
            .ok_or_else(|| {
                ContractError::environment("missing installed package origin directory")
            })?;
        if !package_root.starts_with(venv) {
            return Err(ContractError::environment(
                "installed package originates outside the fresh environment",
            ));
        }
        if measured.insert(name, version).is_some() {
            return Err(ContractError::environment(
                "duplicate installed package identity",
            ));
        }
    }
    if measured != expected_packages(lock) {
        return Err(ContractError::environment(
            "installed package set differs from the complete wheel lock",
        ));
    }
    Ok(())
}

fn verify_venv_configuration(venv: &Path) -> Result<(), ContractError> {
    use std::io::Read as _;
    let mut text = String::new();
    aros_common::open_regular_file_nofollow(&venv.join("pyvenv.cfg"))
        .map_err(io_error)?
        .take(16 * 1024 + 1)
        .read_to_string(&mut text)
        .map_err(io_error)?;
    let settings = text
        .lines()
        .filter_map(|line| line.split_once('='))
        .filter(|(key, _)| key.trim() == "include-system-site-packages")
        .collect::<Vec<_>>();
    if text.len() > 16 * 1024 || settings.len() != 1 || settings[0].1.trim() != "false" {
        return Err(ContractError::environment(
            "new virtual environment admits or ambiguously declares global site packages",
        ));
    }
    Ok(())
}

fn verify_install_report(
    path: &Path,
    wheelhouse: &Path,
    lock: &WheelEnvironmentLock,
) -> Result<(), ContractError> {
    use std::io::Read as _;
    let mut bytes = Vec::new();
    aros_common::open_regular_file_nofollow(path)
        .map_err(io_error)?
        .take(MAX_LOCK_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() > MAX_LOCK_BYTES {
        return Err(ContractError::environment("pip report exceeds its limit"));
    }
    let report: serde_json::Value = serde_json::from_slice(&bytes).map_err(json_error)?;
    let installed = report["install"]
        .as_array()
        .ok_or_else(|| ContractError::environment("missing pip install report"))?;
    if report["version"] != "1" || installed.len() != lock.wheels.len() {
        return Err(ContractError::environment(
            "pip report version or complete inventory differs",
        ));
    }
    let mut seen = BTreeSet::new();
    for entry in installed {
        let name = entry["metadata"]["name"]
            .as_str()
            .and_then(normalized_name)
            .ok_or_else(|| ContractError::environment("pip report package identity is invalid"))?;
        let wheel = lock
            .wheels
            .iter()
            .find(|wheel| wheel.name == name)
            .ok_or_else(|| ContractError::environment("pip installed an unlocked package"))?;
        let url = entry["download_info"]["url"]
            .as_str()
            .and_then(|value| url::Url::parse(value).ok())
            .ok_or_else(|| ContractError::environment("pip omitted the consumed wheel URL"))?;
        if !seen.insert(name)
            || entry["metadata"]["version"] != wheel.version
            || entry["download_info"]["archive_info"]["hashes"]["sha256"] != wheel.sha256
            || url.to_file_path().ok().as_deref()
                != Some(wheelhouse.join(&wheel.filename).as_path())
        {
            return Err(ContractError::environment(
                "pip report differs from the selected local wheel bytes",
            ));
        }
    }
    Ok(())
}

fn expected_packages(lock: &WheelEnvironmentLock) -> BTreeMap<String, &str> {
    lock.wheels
        .iter()
        .map(|wheel| (wheel.name.clone(), wheel.version.as_str()))
        .collect()
}

fn validate_lock(lock: &WheelEnvironmentLock) -> Result<(), ContractError> {
    if lock.schema_version != 1
        || lock.format != "aros-python-wheels-v1"
        || lock.qualification != "local-byte-lock-only"
        || lock.host != current_host()
        || lock.wheels.is_empty()
        || lock.wheels.len() > 256
        || !safe_version(&lock.runtime.version)
    {
        return Err(ContractError::environment(
            "unsupported wheel lock identity, host or budget",
        ));
    }
    for digest in [
        &lock.runtime.executable_sha256,
        &lock.runtime.prefix_tree_sha256,
    ] {
        Sha256Digest::parse(digest)
            .map_err(|_| ContractError::environment("invalid runtime digest"))?;
    }
    let mut names = BTreeSet::new();
    let mut filenames = BTreeSet::new();
    let mut total = 0_u64;
    let mut bootstrap = false;
    for wheel in &lock.wheels {
        if normalized_name(&wheel.name).as_deref() != Some(wheel.name.as_str())
            || !names.insert(&wheel.name)
            || !safe_version(&wheel.version)
            || PortableOutputName::new(&wheel.filename).is_err()
            || Path::new(&wheel.filename)
                .extension()
                .and_then(|ext| ext.to_str())
                != Some("whl")
            || wheel.filename.starts_with('.')
            || !wheel
                .filename
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            || !filenames.insert(wheel.filename.to_ascii_lowercase())
            || wheel.size_bytes == 0
            || wheel.size_bytes > MAX_WHEEL_BYTES
        {
            return Err(ContractError::environment(
                "unsafe, duplicate or oversized wheel declaration",
            ));
        }
        Sha256Digest::parse(&wheel.sha256)
            .map_err(|_| ContractError::environment("invalid wheel digest"))?;
        total = total
            .checked_add(wheel.size_bytes)
            .filter(|total| *total <= MAX_TOTAL_WHEEL_BYTES)
            .ok_or_else(|| ContractError::environment("wheel set exceeds its total byte budget"))?;
        if wheel.filename == lock.bootstrap_filename {
            bootstrap = wheel.name == "pip";
        }
    }
    if !bootstrap {
        return Err(ContractError::environment(
            "bootstrap must select the locked pip wheel",
        ));
    }
    Ok(())
}

fn normalized_name(value: &str) -> Option<String> {
    if value.is_empty()
        || value.len() > 128
        || !value.as_bytes()[0].is_ascii_alphanumeric()
        || !value.as_bytes()[value.len() - 1].is_ascii_alphanumeric()
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    {
        return None;
    }
    let mut result = String::new();
    for byte in value.bytes() {
        if b"-_.".contains(&byte) {
            if !result.ends_with('-') {
                result.push('-');
            }
        } else {
            result.push(char::from(byte.to_ascii_lowercase()));
        }
    }
    Some(result)
}

fn safe_version(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".!+_-".contains(&b))
}

fn current_host() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

fn measure_runtime(root: &Path) -> Result<TreeContentCas, ContractError> {
    let limits = TreeTraversalLimits::new(300_000, 4 * 1024 * 1024 * 1024).map_err(io_error)?;
    measure_tree_content_cas_bounded(root, limits).map_err(io_error)
}

fn verify_runtime_prefix_pin(
    observed: &TreeContentCas,
    pin: &WheelRuntimePin,
) -> Result<(), ContractError> {
    let measured = observed.payload_digest_excluding(None);
    if measured.as_str() != pin.prefix_tree_sha256 {
        return Err(ContractError::environment(format!(
            "CPython prefix content differs from the lock: expected SHA-256 {}, measured SHA-256 {}; preserve the lock and review or restore the exact runtime before preparing new inputs",
            pin.prefix_tree_sha256, measured
        )));
    }
    Ok(())
}

fn verify_interpreter(path: &Path, pin: &WheelRuntimePin) -> Result<(), ContractError> {
    if regular_digest(path, MAX_WHEEL_BYTES)?.to_string() != pin.executable_sha256 {
        return Err(ContractError::environment(
            "CPython executable differs from the lock",
        ));
    }
    Ok(())
}

fn require_private_root(root: &Path) -> Result<(), ContractError> {
    validate_private_directory_nofollow(root).map_err(io_error)?;
    let metadata = fs::symlink_metadata(root).map_err(io_error)?;
    if !metadata.is_dir() || metadata.permissions().mode() & 0o777 != 0o700 {
        return Err(ContractError::environment(
            "wheel preparation root must remain a private 0700 directory",
        ));
    }
    Ok(())
}

fn regular_bytes(path: &Path, limit: u64) -> Result<Vec<u8>, ContractError> {
    measure_regular_file_bounded(path, limit)
        .map_err(io_error)?
        .map(|(_, bytes)| bytes)
        .ok_or_else(|| ContractError::environment("wheel input is not a bounded regular file"))
}

fn regular_digest(path: &Path, limit: u64) -> Result<Sha256Digest, ContractError> {
    regular_bytes(path, limit).map(|bytes| sha256_bytes(&bytes))
}

fn utf8(path: &Path) -> Result<String, ContractError> {
    path.to_str()
        .filter(|s| !s.chars().any(char::is_control))
        .map(str::to_owned)
        .ok_or_else(|| ContractError::environment("runtime path is not safe UTF-8"))
}

fn new_file(path: &Path) -> Result<fs::File, ContractError> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(io_error)
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), ContractError> {
    let mut file = new_file(path)?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(io_error)
}

fn io_error(_: std::io::Error) -> ContractError {
    ContractError::environment("wheel environment filesystem boundary failed")
}
fn json_error(_: serde_json::Error) -> ContractError {
    ContractError::environment("wheel environment JSON boundary failed")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_roots_and_same_hash_symlink_inputs_are_enforced() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        require_private_root(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(require_private_root(&root).is_err());
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let original = root.join("original.whl");
        let alias = root.join("alias.whl");
        fs::write(&original, b"locked wheel").unwrap();
        assert_eq!(
            regular_digest(&original, 12).unwrap(),
            sha256_bytes(b"locked wheel")
        );
        assert!(regular_bytes(&original, 11).is_err());
        std::os::unix::fs::symlink(&original, &alias).unwrap();
        assert!(regular_digest(&alias, 12).is_err());
        assert!(regular_digest(&root, 12).is_err());
    }

    fn lock() -> WheelEnvironmentLock {
        WheelEnvironmentLock {
            schema_version: 1,
            format: "aros-python-wheels-v1".into(),
            qualification: "local-byte-lock-only".into(),
            host: current_host(),
            runtime: WheelRuntimePin {
                version: "3.14.8".into(),
                executable_sha256: "1".repeat(64),
                prefix_tree_sha256: "2".repeat(64),
            },
            bootstrap_filename: "pip-1.0-py3-none-any.whl".into(),
            wheels: vec![LockedWheel {
                name: "pip".into(),
                version: "1.0".into(),
                filename: "pip-1.0-py3-none-any.whl".into(),
                sha256: "3".repeat(64),
                size_bytes: 100,
            }],
        }
    }

    #[test]
    fn runtime_prefix_mismatch_reports_both_digests_without_updating_the_pin() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let file = root.join("stdlib.py");
        fs::write(&file, b"original runtime\n").unwrap();
        let before = measure_runtime(&root).unwrap();
        let mut pin = lock().runtime;
        pin.prefix_tree_sha256 = before.payload_digest_excluding(None).to_string();
        verify_runtime_prefix_pin(&before, &pin).unwrap();

        fs::write(&file, b"changed runtime\n").unwrap();
        let after = measure_runtime(&root).unwrap();
        let expected = pin.prefix_tree_sha256.clone();
        let measured = after.payload_digest_excluding(None).to_string();
        assert_ne!(expected, measured);
        let error = verify_runtime_prefix_pin(&after, &pin)
            .unwrap_err()
            .to_string();
        assert!(error.contains(&format!("expected SHA-256 {expected}")));
        assert!(error.contains(&format!("measured SHA-256 {measured}")));
        assert!(error.contains("preserve the lock"));
        assert_eq!(pin.prefix_tree_sha256, expected);
    }

    #[test]
    fn installed_inventory_binds_private_package_and_import_origins() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let venv = root.join("venv");
        let prefix = root.join("host");
        let packages = venv.join("lib/site-packages");
        fs::create_dir_all(&packages).unwrap();
        fs::create_dir_all(prefix.join("lib")).unwrap();
        let mut value = serde_json::json!({
            "prefix": venv, "base_prefix": prefix,
            "import_paths": [venv.join("lib"), prefix.join("lib")],
            "packages": [{"name":"Pip", "version":"1.0", "root":packages}]
        });
        let check = |value: &serde_json::Value| {
            verify_installed(&serde_json::to_vec(value).unwrap(), &venv, &prefix, &lock())
        };
        check(&value).unwrap();
        value["packages"][0]["root"] = serde_json::json!(prefix.join("lib"));
        assert!(check(&value).is_err());
        value["packages"][0]["root"] = serde_json::json!(packages);
        value["import_paths"][0] = serde_json::json!(root);
        assert!(check(&value).is_err());
        value["import_paths"][0] = serde_json::json!("relative");
        assert!(check(&value).is_err());
        value["import_paths"][0] = serde_json::json!(venv.join("lib"));
        let duplicate = value["packages"][0].clone();
        value["packages"].as_array_mut().unwrap().push(duplicate);
        assert!(check(&value).is_err());
        value["packages"].as_array_mut().unwrap().pop();
        value["packages"][0]["version"] = "different".into();
        assert!(check(&value).is_err());
    }

    #[test]
    fn install_report_requires_the_complete_local_hash_bound_wheel_set() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let wheelhouse = root.join("wheelhouse");
        fs::create_dir(&wheelhouse).unwrap();
        let lock = lock();
        let wheel = &lock.wheels[0];
        let path = root.join("report.json");
        let mut value = serde_json::json!({"version":"1", "install":[{
            "metadata":{"name":"pip","version":"1.0"},
            "download_info":{"url":url::Url::from_file_path(wheelhouse.join(&wheel.filename)).unwrap().to_string(),
                "archive_info":{"hashes":{"sha256":wheel.sha256}}}
        }]});
        let check = |value: &serde_json::Value| {
            fs::write(&path, serde_json::to_vec(value).unwrap()).unwrap();
            verify_install_report(&path, &wheelhouse, &lock)
        };
        check(&value).unwrap();
        value["install"][0]["download_info"]["url"] = "https://example.com/pip.whl".into();
        assert!(check(&value).is_err());
        value["install"][0]["download_info"]["url"] =
            serde_json::json!(url::Url::from_file_path(wheelhouse.join(&wheel.filename))
                .unwrap()
                .to_string());
        value["install"][0]["download_info"]["archive_info"]["hashes"]["sha256"] =
            "4".repeat(64).into();
        assert!(check(&value).is_err());
        value["install"] = serde_json::json!([]);
        assert!(check(&value).is_err());
    }

    #[test]
    fn virtual_environment_cannot_enable_or_duplicate_global_site_configuration() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        for (text, accepted) in [
            ("include-system-site-packages = false\n", true),
            ("include-system-site-packages = true\n", false),
            ("version = 3.14.8\n", false),
            (
                "include-system-site-packages = false\ninclude-system-site-packages = false\n",
                false,
            ),
        ] {
            fs::write(root.join("pyvenv.cfg"), text).unwrap();
            assert_eq!(verify_venv_configuration(&root).is_ok(), accepted);
        }
    }

    #[test]
    fn failed_and_timed_out_phases_retain_logs_and_exact_command_receipts() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        fs::create_dir(root.join("logs")).unwrap();
        fs::create_dir(root.join("tmp")).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let token = CancellationToken::default();
        let environment = base_environment(&root);
        assert!(!environment.contains_key("HOME"));
        assert!(!environment.contains_key("PYTHONPATH"));
        assert!(!environment.contains_key("PIP_INDEX_URL"));
        let mut receipts = Vec::new();
        let arguments = vec!["-c".into(), "printf preserved; exit 7".into()];
        assert!(run_phase(
            &root,
            "failure",
            Path::new("/bin/sh"),
            &arguments,
            &environment,
            Instant::now() + Duration::from_secs(5),
            &token,
            &mut receipts
        )
        .is_err());
        assert_eq!(receipts[0].exit_code, Some(7));
        assert_eq!(receipts[0].arguments, arguments);
        assert_eq!(
            fs::read(root.join("logs/failure.stdout.log")).unwrap(),
            b"preserved"
        );
        assert!(root.join("logs/failure.receipt.json").is_file());
        assert!(run_phase(
            &root,
            "timeout",
            Path::new("/bin/sh"),
            &["-c".into(), "sleep 5".into()],
            &environment,
            Instant::now() + Duration::from_millis(100),
            &token,
            &mut receipts
        )
        .is_err());
        assert!(receipts[1].timed_out);
        assert!(root.join("logs/timeout.receipt.json").is_file());
    }
}
