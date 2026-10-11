//! Explicit SDK release acquisition and host-independent installed state.
//!
//! A selected SDK is an application-build input, not an AROS source-build
//! generation directory. These operations never set AROS_SDK_INCLUDE_DIR.

use aros_common::sdk_release_lock::{SdkReleaseLock, SDK_RELEASE_LOCK_MAX_BYTES};
use aros_common::{measure_regular_file_bounded, sha256_bytes, FileIdentity, Sha256Digest};
use clap::Args;
use miette::{IntoDiagnostic, Result};
use std::path::{Path, PathBuf};

#[derive(Args)]
/// Exact independently selected SDK release metadata.
pub struct ReleaseArgs {
    /// Absolute closed SDK release-lock JSON document
    #[arg(long, value_name = "FILE")]
    release_lock: PathBuf,
    /// Independently selected SHA-256 of the exact release-lock document bytes
    #[arg(long, value_name = "SHA256")]
    lock_sha256: Sha256Digest,
}

#[derive(Args)]
/// Acquire locked SDK artifacts without installing or selecting an SDK.
pub struct DownloadArgs {
    #[command(flatten)]
    release: ReleaseArgs,
    /// Absolute SDK archive-cache parent; default: AROS archive cache/sdk-v2
    #[arg(long, value_name = "DIR")]
    cache: Option<PathBuf>,
    /// Use only existing verified cache entries; never access the network
    #[arg(long)]
    offline: bool,
}

#[derive(Args)]
/// Install a completely verified SDK without creating a global selection.
pub struct InstallArgs {
    #[command(flatten)]
    release: ReleaseArgs,
    /// Absolute SDK store; default: AROS_HOME/sdks
    #[arg(long, value_name = "DIR")]
    store: Option<PathBuf>,
    /// Exact local archive instead of acquisition; never an unchecked prefix
    #[arg(long, value_name = "FILE", conflicts_with_all = ["cache", "offline"])]
    archive: Option<PathBuf>,
    /// Absolute SDK archive-cache parent; default: AROS archive cache/sdk-v2
    #[arg(long, value_name = "DIR")]
    cache: Option<PathBuf>,
    /// Use only existing verified cache entries; never access the network
    #[arg(long)]
    offline: bool,
}

#[derive(Args)]
/// Read-only metadata observation; not a payload verification.
pub struct ListArgs {
    /// Absolute SDK store; default: AROS_HOME/sdks
    #[arg(long, value_name = "DIR")]
    store: Option<PathBuf>,
    /// Maximum namespace entries inspected; incomplete coverage is reported
    #[arg(long, default_value = "10000", value_parser = clap::value_parser!(u32).range(1..=100_000))]
    max_entries: u32,
}

#[derive(Args)]
/// Exact release selection and read-only installed payload verification.
pub struct InstalledArgs {
    #[command(flatten)]
    release: ReleaseArgs,
    /// Absolute SDK store; default: AROS_HOME/sdks
    #[arg(long, value_name = "DIR")]
    store: Option<PathBuf>,
}

struct SelectedRelease {
    path: PathBuf,
    identity: FileIdentity,
    bytes: Vec<u8>,
    lock: SdkReleaseLock,
}

impl SelectedRelease {
    fn read(args: &ReleaseArgs) -> Result<Self> {
        require_absolute("--release-lock", &args.release_lock)?;
        let (identity, bytes) =
            measure_regular_file_bounded(&args.release_lock, SDK_RELEASE_LOCK_MAX_BYTES as u64)
                .into_diagnostic()?
                .ok_or_else(|| miette::miette!("selected SDK release lock is absent"))?;
        if sha256_bytes(&bytes) != args.lock_sha256 {
            miette::bail!("SDK release lock differs from its independently selected SHA-256");
        }
        let lock = SdkReleaseLock::from_bytes(&bytes).map_err(|e| miette::miette!("{e}"))?;
        Ok(Self {
            path: args.release_lock.clone(),
            identity,
            bytes,
            lock,
        })
    }

    fn revalidate(&self) -> Result<()> {
        let current = measure_regular_file_bounded(&self.path, SDK_RELEASE_LOCK_MAX_BYTES as u64)
            .into_diagnostic()?;
        if current != Some((self.identity, self.bytes.clone())) {
            miette::bail!("SDK selection changed during the operation; any published installation is retained, not adopted or removed");
        }
        Ok(())
    }
}

fn require_absolute(label: &str, path: &Path) -> Result<()> {
    if path
        .to_str()
        .is_none_or(|value| value.chars().any(char::is_control))
    {
        miette::bail!("{label} requires UTF-8 without control characters");
    }
    let rebuilt: PathBuf = path.components().collect();
    if !path.is_absolute()
        || rebuilt.as_os_str() != path.as_os_str()
        || path.components().any(|part| {
            matches!(
                part,
                std::path::Component::CurDir | std::path::Component::ParentDir
            )
        })
        || path.parent().is_none()
    {
        miette::bail!("{label} requires a normalized absolute non-root path");
    }
    Ok(())
}

fn store_path(explicit: Option<PathBuf>) -> Result<PathBuf> {
    let path = match explicit {
        Some(path) => path,
        None => crate::artifact::aros_home()?.join("sdks"),
    };
    require_absolute("--store", &path)?;
    Ok(path)
}

fn cache_path(explicit: Option<PathBuf>) -> Result<PathBuf> {
    let path = match explicit {
        Some(path) => path,
        None => crate::artifact::archive_cache_root()?.join("sdk-v2"),
    };
    require_absolute("--cache", &path)?;
    Ok(path)
}

fn emit(operation: &str, result: &serde_json::Value) {
    aros_common::outputln!(
        "{}",
        serde_json::json!({
            "schema":"aros-sdk-operation-v2", "operation":operation,
            "assurance":"selected-byte-consistency-not-publisher-authentication", "result":result,
        })
    );
}

#[cfg(unix)]
pub async fn download(args: DownloadArgs) -> Result<()> {
    let selected = SelectedRelease::read(&args.release)?;
    let acquired =
        crate::sdk_acquisition::prepare(&selected.lock, &cache_path(args.cache)?, args.offline)
            .await?;
    selected.revalidate()?;
    acquired.revalidate()?;
    emit(
        "sdk.download",
        &serde_json::json!({
            "identity":selected.lock.identity, "lock_sha256":selected.lock.digest().map_err(|e| miette::miette!("{e}"))?,
            "archive":acquired.cache_archive, "manifest":acquired.cache_manifest,
            "archive_sha256":selected.lock.archive.sha256, "archive_size":selected.lock.archive.size,
            "installed":false,
        }),
    );
    Ok(())
}

#[cfg(unix)]
pub async fn install(args: InstallArgs) -> Result<()> {
    let selected = SelectedRelease::read(&args.release)?;
    let store = store_path(args.store)?;
    // Keep private verified snapshots alive through publication and readback.
    let acquired = if args.archive.is_none() {
        Some(
            crate::sdk_acquisition::prepare(&selected.lock, &cache_path(args.cache)?, args.offline)
                .await?,
        )
    } else {
        None
    };
    let archive = if let Some(path) = args.archive {
        require_absolute("--archive", &path)?;
        path
    } else {
        acquired
            .as_ref()
            .ok_or_else(|| miette::miette!("SDK acquisition is absent"))?
            .archive_path()
            .to_path_buf()
    };
    selected.revalidate()?;
    let installed =
        aros_toolchain::sdk_store::install(&aros_toolchain::sdk_store::SdkStoreRequest {
            store,
            lock: selected.lock.clone(),
            archive,
        })
        .map_err(|error| native_error(&error))?;
    if !installed.already_installed {
        crate::observability::record_committed_mutation();
    }
    selected.revalidate()?;
    if let Some(acquired) = acquired {
        acquired.revalidate()?;
    }
    emit(
        "sdk.install",
        &serde_json::to_value(installed).into_diagnostic()?,
    );
    Ok(())
}

#[cfg(unix)]
pub fn list(args: ListArgs) -> Result<()> {
    let observed =
        aros_toolchain::sdk_store::inventory(&store_path(args.store)?, args.max_entries as usize)
            .map_err(|error| native_error(&error))?;
    emit(
        "sdk.list",
        &serde_json::to_value(observed).into_diagnostic()?,
    );
    Ok(())
}

#[cfg(unix)]
pub fn verify(args: InstalledArgs, path_only: bool) -> Result<()> {
    let selected = SelectedRelease::read(&args.release)?;
    let installed = aros_toolchain::sdk_store::verify(&store_path(args.store)?, &selected.lock)
        .map_err(|error| native_error(&error))?;
    selected.revalidate()?;
    if path_only {
        aros_common::outputln!("{}", installed.payload.display());
    } else {
        emit(
            "sdk.verify",
            &serde_json::to_value(installed).into_diagnostic()?,
        );
    }
    Ok(())
}

#[cfg(unix)]
fn native_error(error: &aros_toolchain::ContractError) -> miette::Report {
    crate::observability::native_diagnostic(error.diagnostics().diagnostics[0].clone())
}
