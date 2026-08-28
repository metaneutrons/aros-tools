//! Atomic publication of verified Pi boot artifacts into a TFTP tree.

use super::config::Board;
use miette::Result;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use walkdir::WalkDir;

const DEPLOY_MARKER: &str = ".aros-board-deploy";
const DEPLOY_MARKER_CONTENT: &str = "AROS board deployment directory\n";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeployFile {
    pub relative_path: PathBuf,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeploymentPlan {
    pub board_name: String,
    pub source_dir: PathBuf,
    pub destination_dir: PathBuf,
    pub files: Vec<DeployFile>,
}

impl DeploymentPlan {
    /// Validate and inventory one board deployment before publication.
    ///
    /// # Errors
    ///
    /// Returns an error for unsafe paths, a missing artifact, invalid files,
    /// or a destination outside the configured TFTP root.
    pub fn create(
        board: &Board,
        repo_root: &Path,
        artifact_override: Option<&Path>,
    ) -> Result<Self> {
        let source_dir = resolve_artifact_dir(board, repo_root, artifact_override)?;
        let tftp_root = canonical_existing_directory(board.tftp_root()?, "tftp_root")?;
        reject_device_path(&tftp_root)?;
        let destination_dir = tftp_root.join(board.tftp_prefix()?);

        if destination_dir.starts_with(&source_dir) || source_dir.starts_with(&destination_dir) {
            miette::bail!(
                "Deployment destination '{}' overlaps the artifact directory '{}'. Choose a separate tftp_root.",
                destination_dir.display(),
                source_dir.display()
            );
        }

        let files = collect_files(&source_dir)?;
        if files.is_empty() {
            miette::bail!(
                "Artifact directory '{}' contains no regular files to deploy.",
                source_dir.display()
            );
        }

        Ok(Self {
            board_name: board.name.clone(),
            source_dir,
            destination_dir,
            files,
        })
    }

    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|file| file.bytes).sum()
    }
}

/// Publish a fully staged bundle to its configured local TFTP directory.
///
/// An existing destination is replaced only when it carries our marker,
/// preventing a typo from clobbering an unrelated TFTP tree.
///
/// # Errors
///
/// Returns an error when staging, validation, synchronization, or atomic
/// publication fails.
pub fn publish(plan: &DeploymentPlan) -> Result<()> {
    let parent = plan.destination_dir.parent().ok_or_else(|| {
        miette::miette!(
            "Deployment destination '{}' has no parent directory.",
            plan.destination_dir.display()
        )
    })?;
    if !parent.exists() {
        fs::create_dir_all(parent).map_err(|error| {
            miette::miette!(
                "Could not create configured deployment directory '{}': {error}",
                parent.display()
            )
        })?;
    }
    let parent = canonical_existing_directory(parent, "deployment parent")?;
    reject_device_path(&parent)?;
    let destination =
        parent.join(plan.destination_dir.file_name().ok_or_else(|| {
            miette::miette!("Deployment destination has no final path component.")
        })?);

    let stage = create_unique_directory(&parent, ".aros-board-stage")?;
    let publish_result = stage_and_publish(plan, &stage, &destination);
    if publish_result.is_err() && stage.exists() {
        let _ = fs::remove_dir_all(&stage);
    }
    publish_result
}

fn stage_and_publish(plan: &DeploymentPlan, stage: &Path, destination: &Path) -> Result<()> {
    for file in &plan.files {
        let source = plan.source_dir.join(&file.relative_path);
        let target = stage.join(&file.relative_path);
        let target_parent = target.parent().ok_or_else(|| {
            miette::miette!("Could not determine parent for '{}'.", target.display())
        })?;
        fs::create_dir_all(target_parent).map_err(|error| {
            miette::miette!(
                "Could not create staging directory '{}': {error}",
                target_parent.display()
            )
        })?;
        fs::copy(&source, &target).map_err(|error| {
            miette::miette!(
                "Could not copy '{}' to '{}': {error}",
                source.display(),
                target.display()
            )
        })?;
    }
    fs::write(stage.join(DEPLOY_MARKER), DEPLOY_MARKER_CONTENT).map_err(|error| {
        miette::miette!(
            "Could not mark staged deployment '{}': {error}",
            stage.display()
        )
    })?;

    if !destination.exists() {
        fs::rename(stage, destination).map_err(|error| {
            miette::miette!(
                "Could not publish staged deployment '{}' to '{}': {error}",
                stage.display(),
                destination.display()
            )
        })?;
        return Ok(());
    }

    ensure_managed_destination(destination)?;
    let parent = destination.parent().ok_or_else(|| {
        miette::miette!(
            "Deployment destination '{}' has no parent.",
            destination.display()
        )
    })?;
    let backup = create_unique_path(parent, ".aros-pi-previous");

    fs::rename(destination, &backup).map_err(|error| {
        miette::miette!(
            "Could not move existing deployment '{}' aside: {error}",
            destination.display()
        )
    })?;
    if let Err(error) = fs::rename(stage, destination) {
        let restore = fs::rename(&backup, destination);
        if let Err(restore_error) = restore {
            miette::bail!(
                "Could not publish staged deployment '{}': {error}; additionally could not restore the previous deployment '{}': {restore_error}",
                stage.display(),
                backup.display()
            );
        }
        return Err(miette::miette!(
            "Could not publish staged deployment '{}': {error}. The previous deployment was restored.",
            stage.display()
        ));
    }

    fs::remove_dir_all(&backup).map_err(|error| {
        miette::miette!(
            "Published a new deployment, but could not remove its previous managed version '{}': {error}",
            backup.display()
        )
    })?;
    Ok(())
}

fn resolve_artifact_dir(
    board: &Board,
    repo_root: &Path,
    artifact_override: Option<&Path>,
) -> Result<PathBuf> {
    let raw_path =
        artifact_override.map_or_else(|| board.artifact_dir(repo_root), Path::to_path_buf);
    let source = if raw_path.is_absolute() {
        raw_path
    } else {
        repo_root.join(raw_path)
    };
    canonical_existing_directory(&source, "artifact directory")
}

fn canonical_existing_directory(path: &Path, label: &str) -> Result<PathBuf> {
    let metadata = fs::metadata(path).map_err(|error| {
        miette::miette!("Could not access {label} '{}': {error}", path.display())
    })?;
    if !metadata.is_dir() {
        miette::bail!("{label} '{}' is not a directory.", path.display());
    }
    path.canonicalize()
        .map_err(|error| miette::miette!("Could not resolve {label} '{}': {error}", path.display()))
}

fn reject_device_path(path: &Path) -> Result<()> {
    if path.starts_with("/dev") {
        miette::bail!(
            "Refusing to deploy to '{}': raw device paths are never valid deployment roots.",
            path.display()
        );
    }
    Ok(())
}

fn collect_files(source_dir: &Path) -> Result<Vec<DeployFile>> {
    let mut files = Vec::new();
    for entry in WalkDir::new(source_dir)
        .follow_links(false)
        .sort_by_file_name()
    {
        let entry = entry.map_err(|error| {
            miette::miette!(
                "Could not enumerate artifact directory '{}': {error}",
                source_dir.display()
            )
        })?;
        if entry.path() == source_dir {
            continue;
        }
        if entry.file_type().is_symlink() {
            miette::bail!(
                "Artifact '{}' is a symbolic link. Deploy only regular, self-contained boot artifacts.",
                entry.path().display()
            );
        }
        if entry.file_type().is_dir() {
            continue;
        }
        if !entry.file_type().is_file() {
            miette::bail!(
                "Artifact '{}' is not a regular file.",
                entry.path().display()
            );
        }
        let relative_path = entry.path().strip_prefix(source_dir).map_err(|error| {
            miette::miette!(
                "Could not determine artifact path relative to '{}': {error}",
                source_dir.display()
            )
        })?;
        files.push(DeployFile {
            relative_path: relative_path.to_path_buf(),
            bytes: entry
                .metadata()
                .map_err(|error| {
                    miette::miette!(
                        "Could not inspect artifact '{}': {error}",
                        entry.path().display()
                    )
                })?
                .len(),
        });
    }
    Ok(files)
}

fn ensure_managed_destination(destination: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(destination).map_err(|error| {
        miette::miette!(
            "Could not inspect existing deployment '{}': {error}",
            destination.display()
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        miette::bail!(
            "Refusing to replace '{}': an existing deployment must be a real managed directory.",
            destination.display()
        );
    }
    let marker = destination.join(DEPLOY_MARKER);
    let content = fs::read_to_string(&marker).map_err(|error| {
        miette::miette!(
            "Refusing to replace '{}': it is not an AROS-managed deployment (missing '{}': {error}).",
            destination.display(),
            marker.display()
        )
    })?;
    if content != DEPLOY_MARKER_CONTENT {
        miette::bail!(
            "Refusing to replace '{}': its AROS deployment marker is invalid.",
            destination.display()
        );
    }
    Ok(())
}

fn create_unique_directory(parent: &Path, prefix: &str) -> Result<PathBuf> {
    for attempt in 0..100_u16 {
        let path = create_unique_path_with_attempt(parent, prefix, attempt);
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(miette::miette!(
                    "Could not create staging directory '{}': {error}",
                    path.display()
                ));
            }
        }
    }
    miette::bail!("Could not allocate a unique AROS deployment staging directory.");
}

fn create_unique_path(parent: &Path, prefix: &str) -> PathBuf {
    for attempt in 0..100_u16 {
        let path = create_unique_path_with_attempt(parent, prefix, attempt);
        if !path.exists() {
            return path;
        }
    }
    parent.join(format!("{prefix}-unavailable"))
}

fn create_unique_path_with_attempt(parent: &Path, prefix: &str, attempt: u16) -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    parent.join(format!(
        "{prefix}-{}-{timestamp}-{attempt}",
        std::process::id()
    ))
}

#[cfg(test)]
mod tests {
    use super::{publish, DeploymentPlan};
    use crate::config::{Board, BoardConfig, Transport};
    use std::path::Path;

    fn board(name: &str, tftp_root: &Path) -> Board {
        Board {
            name: name.to_string(),
            config: BoardConfig {
                model: "rpi4".to_string(),
                preset: "rpi-aarch64".to_string(),
                toolchain_preset: "rpi-aarch64".to_string(),
                build_target: "rpi-artifacts".to_string(),
                transport: Transport::NativeTftp,
                artifact_dir: None,
                dtb_path: None,
                core_kobj_dir: None,
                tftp_root: Some(tftp_root.to_path_buf()),
                tftp_prefix: None,
                serial_device: None,
                serial_baud: 115_200,
                debug_transport: None,
                power_control: None,
                network: None,
                usb_ecm: None,
            },
            config_path: tftp_root.join("boards.toml"),
        }
    }

    #[test]
    fn plan_collects_a_recursive_boot_bundle() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let artifacts = temp.path().join("artifacts");
        let tftp = temp.path().join("tftp");
        std::fs::create_dir_all(artifacts.join("dtb")).expect("artifact directory");
        std::fs::create_dir_all(&tftp).expect("tftp directory");
        std::fs::write(artifacts.join("kernel.img"), "kernel").expect("kernel");
        std::fs::write(artifacts.join("dtb/board.dtb"), "dtb").expect("dtb");

        let board = board("rpi4", &tftp);
        let plan = DeploymentPlan::create(&board, temp.path(), Some(&artifacts)).expect("plan");

        assert_eq!(
            plan.destination_dir,
            tftp.canonicalize().expect("canonical tftp").join("rpi4")
        );
        assert_eq!(plan.files.len(), 2);
        assert_eq!(plan.total_bytes(), 9);
    }

    #[test]
    fn publish_replaces_only_a_marked_deployment_directory() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let artifacts = temp.path().join("artifacts");
        let tftp = temp.path().join("tftp");
        std::fs::create_dir_all(&artifacts).expect("artifact directory");
        std::fs::create_dir_all(&tftp).expect("tftp directory");
        std::fs::write(artifacts.join("kernel.img"), "first").expect("artifact");
        let board = board("rpi4", &tftp);
        let plan = DeploymentPlan::create(&board, temp.path(), Some(&artifacts)).expect("plan");

        publish(&plan).expect("initial publish");
        std::fs::write(artifacts.join("kernel.img"), "second").expect("updated artifact");
        let updated = DeploymentPlan::create(&board, temp.path(), Some(&artifacts)).expect("plan");
        publish(&updated).expect("replacement publish");

        assert_eq!(
            std::fs::read_to_string(tftp.join("rpi4/kernel.img")).expect("published artifact"),
            "second"
        );
    }

    #[test]
    fn publish_refuses_to_clobber_an_unmanaged_directory() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let artifacts = temp.path().join("artifacts");
        let tftp = temp.path().join("tftp");
        std::fs::create_dir_all(&artifacts).expect("artifact directory");
        std::fs::create_dir_all(tftp.join("rpi4")).expect("unmanaged destination");
        std::fs::write(artifacts.join("kernel.img"), "kernel").expect("artifact");
        std::fs::write(tftp.join("rpi4/keep.txt"), "keep").expect("unmanaged file");
        let board = board("rpi4", &tftp);
        let plan = DeploymentPlan::create(&board, temp.path(), Some(&artifacts)).expect("plan");

        assert!(publish(&plan).is_err());
        assert_eq!(
            std::fs::read_to_string(tftp.join("rpi4/keep.txt")).expect("unmanaged file"),
            "keep"
        );
    }
}
