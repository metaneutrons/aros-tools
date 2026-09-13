//! Atomic publication of verified Pi boot artifacts into a TFTP tree.

use super::config::{Board, Transport};
use crate::canonical_existing_directory;
use aros_common::{
    copy_tree_from_snapshot_nofollow, create_unique_directory_nofollow,
    exchange_prepared_tree_if_unchanged, is_rollback_incomplete, measure_tree_content_cas_bounded,
    open_regular_file_nofollow, publication_failure_class, publish_atomic_file,
    publish_prepared_tree_noclobber, remove_tree_from_snapshot_nofollow,
    validate_existing_directory_prefix_nofollow, AtomicFilePolicy, PublicationFailureClass,
    TreeContentCas, TreeTraversalLimits,
};
use miette::Result;
use std::io::{ErrorKind, Read as _};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

const DEPLOY_MARKER: &str = ".aros-board-deploy";
const DEPLOY_MARKER_CONTENT: &str = "AROS board deployment directory\n";
const DEPLOY_TREE_LIMITS: TreeTraversalLimits = TreeTraversalLimits {
    max_entries: 16_384,
    max_regular_file_bytes: 2 * 1024 * 1024 * 1024,
};

#[derive(Debug)]
enum StageFailure {
    Cleanup(miette::Report),
    Retain(miette::Report),
}

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
    source_snapshot: TreeContentCas,
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
        if board.config.transport == Transport::UefiEsp {
            miette::bail!(
                "Board '{}' uses uefi-esp. Create and write its verified boot image with `aros board sd image` and `aros board sd write`; TFTP deploy does not apply.",
                board.name
            );
        }
        let source_dir = resolve_artifact_dir(board, repo_root, artifact_override)?;
        let tftp_root = canonical_existing_directory(board.tftp_root()?, "tftp_root")?;
        reject_device_path(&tftp_root)?;
        let destination_dir = tftp_root.join(board.tftp_prefix()?);
        validate_existing_directory_prefix_nofollow(&destination_dir).map_err(|error| {
            miette::miette!(
                "Configured deployment path '{}' is unsafe: {error}",
                destination_dir.display()
            )
        })?;

        if destination_dir.starts_with(&source_dir) || source_dir.starts_with(&destination_dir) {
            miette::bail!(
                "Deployment destination '{}' overlaps the artifact directory '{}'. Choose a separate tftp_root.",
                destination_dir.display(),
                source_dir.display()
            );
        }

        let source_snapshot = measure_tree_content_cas_bounded(&source_dir, DEPLOY_TREE_LIMITS)
            .map_err(|error| {
                miette::miette!(
                    "Could not take a stable no-follow snapshot of artifact directory '{}': {error}",
                    source_dir.display()
                )
            })?;
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
            source_snapshot,
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
    let stage = create_unique_directory_nofollow(parent, ".aros-board-stage").map_err(|error| {
        miette::miette!(
            "Could not create a contained staging directory below '{}': {error}",
            parent.display()
        )
    })?;
    match stage_and_publish(plan, &stage) {
        Ok(()) => Ok(()),
        Err(StageFailure::Retain(error)) => Err(error),
        Err(StageFailure::Cleanup(error)) => match remove_staging_tree(&stage) {
            Ok(()) => Err(error),
            Err(cleanup_error) => Err(miette::miette!(
                "{error}; additionally could not remove the contained staging directory '{}': {cleanup_error}",
                stage.display()
            )),
        },
    }
}

fn stage_and_publish(plan: &DeploymentPlan, stage: &Path) -> std::result::Result<(), StageFailure> {
    if let Err(error) = copy_tree_from_snapshot_nofollow(
        &plan.source_dir,
        stage,
        &plan.source_snapshot,
        DEPLOY_TREE_LIMITS,
    ) {
        return Err(StageFailure::Cleanup(miette::miette!(
            "Could not copy the verified boot bundle into contained staging '{}': {error}",
            stage.display()
        )));
    }
    if let Err(error) = publish_atomic_file(
        &stage.join(DEPLOY_MARKER),
        DEPLOY_MARKER_CONTENT.as_bytes(),
        AtomicFilePolicy::NoClobber,
    ) {
        return Err(StageFailure::Cleanup(miette::miette!(
            "Could not mark staged deployment '{}': {error}",
            stage.display()
        )));
    }

    let destination_snapshot =
        match measure_tree_content_cas_bounded(&plan.destination_dir, DEPLOY_TREE_LIMITS) {
            Ok(snapshot) => Some(snapshot),
            Err(error) if error.kind() == ErrorKind::NotFound => None,
            Err(error) => {
                return Err(StageFailure::Cleanup(miette::miette!(
                    "Could not inspect configured deployment destination '{}': {error}",
                    plan.destination_dir.display()
                )));
            }
        };

    let Some(destination_snapshot) = destination_snapshot else {
        return match publish_prepared_tree_noclobber(stage, &plan.destination_dir) {
            Ok(_) => Ok(()),
            Err(error) if must_retain_stage(&error) => Err(StageFailure::Retain(miette::miette!(
                "Could not atomically publish staged deployment '{}' to '{}': {error}",
                stage.display(),
                plan.destination_dir.display()
            ))),
            Err(error) => Err(StageFailure::Cleanup(miette::miette!(
                "Could not atomically publish staged deployment '{}' to '{}': {error}",
                stage.display(),
                plan.destination_dir.display()
            ))),
        };
    };

    if let Err(error) = ensure_managed_destination(&plan.destination_dir) {
        return Err(StageFailure::Cleanup(error));
    }
    if let Err(error) =
        exchange_prepared_tree_if_unchanged(stage, &plan.destination_dir, &destination_snapshot)
    {
        let failure = miette::miette!(
            "Could not atomically replace configured deployment '{}': {error}",
            plan.destination_dir.display()
        );
        return if must_retain_stage(&error) {
            Err(StageFailure::Retain(failure))
        } else {
            Err(StageFailure::Cleanup(failure))
        };
    }
    if let Err(error) =
        remove_tree_from_snapshot_nofollow(stage, &destination_snapshot, DEPLOY_TREE_LIMITS)
    {
        return Err(StageFailure::Retain(miette::miette!(
                "Published the new deployment at '{}', but could not safely remove the retained previous deployment '{}': {error}",
                plan.destination_dir.display(),
                stage.display()
        )));
    }
    Ok(())
}

fn must_retain_stage(error: &std::io::Error) -> bool {
    is_rollback_incomplete(error)
        || publication_failure_class(error) == PublicationFailureClass::CommitStateUncertain
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
    let marker = destination.join(DEPLOY_MARKER);
    let mut file = open_regular_file_nofollow(&marker).map_err(|error| {
        miette::miette!(
            "Refusing to replace '{}': it is not an AROS-managed deployment (missing '{}': {error}).",
            destination.display(),
            marker.display()
        )
    })?;
    let mut content = String::new();
    file.read_to_string(&mut content).map_err(|error| {
        miette::miette!(
            "Could not read deployment marker '{}' without following links: {error}",
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

fn remove_staging_tree(stage: &Path) -> Result<()> {
    let snapshot = match measure_tree_content_cas_bounded(stage, DEPLOY_TREE_LIMITS) {
        Ok(snapshot) => snapshot,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(miette::miette!(
                "Could not inspect staging directory '{}' for safe cleanup: {error}",
                stage.display()
            ));
        }
    };
    remove_tree_from_snapshot_nofollow(stage, &snapshot, DEPLOY_TREE_LIMITS).map_err(|error| {
        miette::miette!(
            "Could not safely remove staging directory '{}': {error}",
            stage.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::{publish, DeploymentPlan};
    use crate::config::{
        Board, BoardBackend, BoardConfig, BoardModel, RaspberryPiConfig, Transport,
    };
    use std::path::Path;

    fn board(name: &str, tftp_root: &Path) -> Board {
        Board {
            name: name.to_string(),
            config: BoardConfig {
                backend: BoardBackend::RaspberryPi,
                model: BoardModel::Rpi4,
                preset: "rpi-aarch64".to_string(),
                toolchain_preset: "rpi-aarch64".to_string(),
                build_target: "rpi-artifacts".to_string(),
                transport: Transport::NativeTftp,
                artifact_dir: None,
                raspberry_pi: Some(RaspberryPiConfig {
                    dtb_path: tftp_root.join("bcm2711-rpi-4-b.dtb"),
                    core_kobj_dir: tftp_root.join("kobjs"),
                }),
                opensbi_uefi: None,
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

    fn board_with_prefix(name: &str, tftp_root: &Path, prefix: &str) -> Board {
        let mut board = board(name, tftp_root);
        board.config.tftp_prefix = Some(prefix.into());
        board
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
        #[cfg(unix)]
        use std::os::unix::fs::PermissionsExt as _;

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
        #[cfg(unix)]
        assert_eq!(
            std::fs::metadata(tftp.join("rpi4"))
                .expect("published directory")
                .permissions()
                .mode()
                & 0o777,
            0o755
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

    #[cfg(unix)]
    #[test]
    fn plan_rejects_a_symlinked_tftp_prefix_without_touching_its_target() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("temporary directory");
        let artifacts = temp.path().join("artifacts");
        let tftp = temp.path().join("tftp");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&artifacts).expect("artifact directory");
        std::fs::create_dir_all(&tftp).expect("tftp directory");
        std::fs::create_dir_all(&outside).expect("outside directory");
        std::fs::write(artifacts.join("kernel.img"), "kernel").expect("artifact");
        symlink(&outside, tftp.join("redirect")).expect("symlinked prefix");

        let board = board_with_prefix("rpi4", &tftp, "redirect/current");
        let error = DeploymentPlan::create(&board, temp.path(), Some(&artifacts)).unwrap_err();

        assert!(error.to_string().contains("unsafe"));
        assert!(!outside.join("current/kernel.img").exists());
    }

    #[cfg(unix)]
    #[test]
    fn publish_rejects_a_prefix_parent_swapped_after_preview() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("temporary directory");
        let artifacts = temp.path().join("artifacts");
        let tftp = temp.path().join("tftp");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&artifacts).expect("artifact directory");
        std::fs::create_dir_all(tftp.join("stable")).expect("tftp prefix parent");
        std::fs::create_dir_all(&outside).expect("outside directory");
        std::fs::write(artifacts.join("kernel.img"), "kernel").expect("artifact");

        let board = board_with_prefix("rpi4", &tftp, "stable/current");
        let plan = DeploymentPlan::create(&board, temp.path(), Some(&artifacts)).expect("plan");

        std::fs::rename(tftp.join("stable"), tftp.join("stable-before-swap"))
            .expect("move original parent");
        symlink(&outside, tftp.join("stable")).expect("swapped prefix parent");

        assert!(publish(&plan).is_err());
        assert!(!outside.join("current/kernel.img").exists());
    }
}
