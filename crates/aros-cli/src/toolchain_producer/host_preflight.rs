//! Measured native-host prerequisites for the producer input phase.
//!
//! This command inspects the selected producer profile and prepares the
//! lock-owned Python modules needed by later lifecycle phases. It does not
//! configure or build a compiler, or make any release-qualification claim.

use std::fs;
use std::path::{Component, Path, PathBuf};

use aros_common::{
    measure_regular_file_bounded, sha256_bytes, validate_existing_directory_prefix_nofollow,
};
use aros_toolchain::preflight;
use aros_toolchain::profiles::Profiles;
use aros_toolchain::python_environment::PythonEnvironment;
use aros_toolchain::source_cache_request::SourceCacheRequest;
use aros_toolchain::source_lock::{CompilerFamily, SourceLock};
use clap::Args;

const INPUT_LIMIT: u64 = 1024 * 1024;
const SCHEMA: &str = "aros-native-host-preflight-v1";
const ASSURANCE: &str = "host-prerequisites-only-not-configure-build-or-release-qualification";

/// Inspect native prerequisites and materialize the selected lock-owned Python environment.
#[derive(Args)]
pub struct HostPreflightArgs {
    /// Exact source-lock JSON document
    #[arg(long)]
    source_lock: PathBuf,
    /// Exact producer profiles JSON document
    #[arg(long)]
    profiles: PathBuf,
    /// Exact preset selected from the profiles document
    #[arg(long)]
    preset: String,
    /// Expected native host key, such as macos-aarch64
    #[arg(long)]
    host: String,
    /// Existing real source cache directory
    #[arg(long)]
    cache_dir: PathBuf,
    /// Fresh absolute destination for the private Python environment
    #[arg(long)]
    python_env_dir: PathBuf,
}

/// Run one bounded host-prerequisite inspection and emit its JSON evidence.
pub fn run(args: &HostPreflightArgs) -> miette::Result<()> {
    let source_lock_bytes = read_bounded_input(&args.source_lock, "source lock")?;
    let profiles_bytes = read_bounded_input(&args.profiles, "profiles")?;
    let source_lock =
        SourceLock::parse(&source_lock_bytes).map_err(|error| super::native_error(&error))?;
    let profiles = Profiles::parse(&profiles_bytes).map_err(|error| super::native_error(&error))?;
    if source_lock.family() != profiles.family() {
        return Err(miette::miette!(
            "native host preflight source lock and profiles select different compiler families"
        ));
    }
    let profile = profiles
        .select(&args.preset)
        .map_err(|error| super::native_error(&error))?;

    let expected_host = aros_common::target::native_host_key()
        .ok_or_else(|| miette::miette!("native host preflight is unsupported on this host"))?;
    if args.host != expected_host {
        return Err(miette::miette!(
            "native host preflight --host does not match the current native host"
        ));
    }

    validate_cache_directory(&args.cache_dir)?;
    validate_python_environment_destination(
        &args.python_env_dir,
        &[&args.source_lock, &args.profiles, &args.cache_dir],
    )?;

    let host = preflight::inspect(profile).map_err(|error| super::native_error(&error))?;
    if host.host != args.host {
        return Err(miette::miette!(
            "measured native host differs from the expected host"
        ));
    }
    let interpreter = host
        .python_interpreter()
        .map_err(|error| super::native_error(&error))?;
    verify_python_package_cache(&source_lock_bytes, &source_lock, &args.cache_dir)?;

    // Check serialization before creating the caller-selected fresh directory.
    let host_observation = serde_json::to_value(&host)
        .map_err(|_| miette::miette!("cannot serialize native host preflight observations"))?;
    let interpreter_observation = serde_json::to_value(&interpreter)
        .map_err(|_| miette::miette!("cannot serialize host Python interpreter observation"))?;
    let environment_dir = args.python_env_dir.to_str().ok_or_else(|| {
        miette::miette!("native host preflight --python-env-dir must be UTF-8 representable")
    })?;

    let python = PythonEnvironment::prepare_with_interpreter(
        &source_lock,
        &args.cache_dir,
        &args.python_env_dir,
        &interpreter,
    )
    .map_err(|error| super::native_error(&error))?;

    let family = match source_lock.family() {
        CompilerFamily::Llvm => "llvm",
        CompilerFamily::Gnu => "gnu",
    };
    let document = serde_json::json!({
        "schema": SCHEMA,
        "assurance": ASSURANCE,
        "inputs": {
            "family": family,
            "profiles_upstream_commit": profiles.upstream_commit(),
            "source_lock_sha256": sha256_bytes(&source_lock_bytes),
            "profiles_sha256": sha256_bytes(&profiles_bytes),
        },
        "host_preflight": host_observation,
        "python": {
            "environment_dir": environment_dir,
            "interpreter": interpreter_observation,
            "packages": python.packages(),
        },
    });
    super::print_json(&document)
}

fn read_bounded_input(path: &Path, label: &str) -> miette::Result<Vec<u8>> {
    let (_, bytes) = measure_regular_file_bounded(path, INPUT_LIMIT)
        .map_err(|_| miette::miette!("native producer {label} is unavailable or exceeds 1 MiB"))?
        .ok_or_else(|| miette::miette!("native producer {label} is unavailable"))?;
    Ok(bytes)
}

fn validate_cache_directory(path: &Path) -> miette::Result<()> {
    if !is_absolute_normalized(path) {
        return Err(miette::miette!(
            "native host preflight --cache-dir must be an absolute normalized path"
        ));
    }
    validate_existing_directory_prefix_nofollow(path).map_err(|_| {
        miette::miette!(
            "native host preflight --cache-dir must be a real directory without symlink components"
        )
    })?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| miette::miette!("native host preflight --cache-dir is unavailable"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(miette::miette!(
            "native host preflight --cache-dir must be a real directory"
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn verify_python_package_cache(
    source_lock_bytes: &[u8],
    source_lock: &SourceLock,
    cache_dir: &Path,
) -> miette::Result<()> {
    let mut request = SourceCacheRequest::from_source_lock(source_lock_bytes)
        .map_err(|error| super::native_error(&error))?;
    request.entries.retain(|entry| {
        source_lock
            .host_python_packages()
            .iter()
            .any(|package| package.payload().filename() == entry.filename.as_str())
    });
    request
        .validate()
        .map_err(|error| super::native_error(&error))?;
    aros_toolchain::source_cache::verify_request(cache_dir, &request)
        .map_err(|error| super::native_error(&error))?;
    Ok(())
}

#[cfg(not(unix))]
fn verify_python_package_cache(
    source_lock_bytes: &[u8],
    source_lock: &SourceLock,
    cache_dir: &Path,
) -> miette::Result<()> {
    let _ = (source_lock_bytes, source_lock, cache_dir);
    Err(miette::miette!(
        "native host Python cache verification requires a Unix host"
    ))
}

fn validate_python_environment_destination(path: &Path, protected: &[&Path]) -> miette::Result<()> {
    if !is_absolute_normalized(path) {
        return Err(miette::miette!(
            "native host preflight --python-env-dir must be an absolute normalized path"
        ));
    }
    let parent = path
        .parent()
        .ok_or_else(|| miette::miette!("native host preflight --python-env-dir has no parent"))?;
    validate_existing_directory_prefix_nofollow(parent).map_err(|_| {
        miette::miette!(
            "native host preflight --python-env-dir ancestors must be existing real directories"
        )
    })?;
    if !parent.is_dir() {
        return Err(miette::miette!(
            "native host preflight --python-env-dir parent must already exist"
        ));
    }
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        _ => {
            return Err(miette::miette!(
                "native host preflight --python-env-dir must not already exist"
            ));
        }
    }
    for input in protected {
        let input = input.canonicalize().map_err(|_| {
            miette::miette!("cannot resolve native host preflight input for overlap checking")
        })?;
        if super::compatibility_export::filesystem_overlap(path, &input)? {
            return Err(miette::miette!(
                "native host preflight --python-env-dir must be separate from its inputs and cache"
            ));
        }
    }
    Ok(())
}

fn is_absolute_normalized(path: &Path) -> bool {
    path.is_absolute()
        && path
            .components()
            .all(|part| matches!(part, Component::RootDir | Component::Normal(_)))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{is_absolute_normalized, validate_cache_directory};

    #[test]
    fn cache_requires_an_absolute_normalized_real_directory() {
        assert!(!is_absolute_normalized(Path::new("relative/cache")));
        assert!(!is_absolute_normalized(Path::new("/tmp/cache/../other")));
        assert!(is_absolute_normalized(Path::new("/tmp/cache")));
    }

    #[test]
    fn cache_directory_validation_rejects_nonexistent_paths() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("missing-cache");
        assert!(validate_cache_directory(&path).is_err());
        assert!(validate_cache_directory(temporary.path()).is_ok());
    }
}
