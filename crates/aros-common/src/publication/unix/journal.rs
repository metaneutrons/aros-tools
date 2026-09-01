//! Durable journal validation, encoding, replacement, and stage cleanup.

use super::{
    casefold_path_key, combine_primary_and_cleanup, identity_at, io_failure, open_parent,
    read_regular, remove_regular_exact, rename_noclobber, rfs, sha256_bytes, sibling_name,
    test_fail_point, write_new_file_mode, BTreeSet, ErrorKind, Journal, JournalState, Path,
    PathBuf, PublicationError, JOURNAL_STAGE_MAGIC,
};
pub(super) fn validate_journal(journal: &Journal, journal_path: &Path) -> std::io::Result<()> {
    let mode_aware = match journal.schema.as_str() {
        "aros-publication-journal-v2" => false,
        "aros-publication-journal-v3" => true,
        _ => {
            return Err(io_failure(PublicationError::rollback(format!(
                "unsupported publication journal schema '{}'",
                journal.schema
            ))));
        }
    };
    let mut targets = BTreeSet::new();
    let mut collision_keys = BTreeSet::new();
    let scope = journal_path.parent().ok_or_else(|| {
        io_failure(PublicationError::rollback(
            "publication journal has no containment scope",
        ))
    })?;
    for (index, operation) in journal.operations.iter().enumerate() {
        if !operation.target.is_absolute()
            || !operation.target.starts_with(scope)
            || !targets.insert(operation.target.clone())
        {
            return Err(io_failure(PublicationError::rollback(
                "publication journal contains an invalid or duplicate target",
            )));
        }
        let relative = operation.target.strip_prefix(scope).map_err(|_| {
            io_failure(PublicationError::rollback(
                "publication journal target escaped its scope",
            ))
        })?;
        let collision_key = casefold_path_key(relative).map_err(|error| {
            io_failure(PublicationError::rollback(format!(
                "publication journal target is not portable: {error}"
            )))
        })?;
        if !collision_keys.insert(collision_key) {
            return Err(io_failure(PublicationError::rollback(
                "publication journal contains a case-folded target collision",
            )));
        }
        let original_proof_incomplete = operation.original.is_some()
            != operation.original_digest.is_some()
            || (mode_aware && operation.original.is_some() != operation.original_mode.is_some())
            || (!mode_aware && operation.original_mode.is_some());
        if original_proof_incomplete {
            return Err(io_failure(PublicationError::rollback(
                "publication journal original identity/digest/mode proof is incomplete",
            )));
        }
        if operation.original_mode.is_some_and(|mode| mode > 0o7777) {
            return Err(io_failure(PublicationError::rollback(
                "publication journal original mode is outside the portable permission mask",
            )));
        }
        let parent = operation.target.parent().ok_or_else(|| {
            io_failure(PublicationError::rollback(
                "publication journal target has no parent",
            ))
        })?;
        let leaf = operation.target.file_name().ok_or_else(|| {
            io_failure(PublicationError::rollback(
                "publication journal target has no leaf",
            ))
        })?;
        let expected_stage = parent.join(sibling_name(leaf, "stage", journal.nonce + index as u64));
        let expected_backup =
            parent.join(sibling_name(leaf, "backup", journal.nonce + index as u64));
        let stage_path_invalid = operation
            .stage
            .as_ref()
            .is_some_and(|path| path != &expected_stage);
        let backup_path_invalid = operation
            .backup
            .as_ref()
            .is_some_and(|path| path != &expected_backup);
        if stage_path_invalid
            || backup_path_invalid
            || operation.stage.is_some() != operation.desired_digest.is_some()
            || (mode_aware && operation.stage.is_some() != operation.desired_mode.is_some())
            || (!mode_aware && operation.desired_mode.is_some())
            || operation
                .desired_mode
                .is_some_and(|mode| !matches!(mode, 0o644 | 0o755))
            || operation.installed.is_some() && operation.stage.is_none()
            || operation.backup.is_some() != operation.original.is_some()
        {
            return Err(io_failure(PublicationError::rollback(
                "publication journal contains inconsistent or non-derived auxiliary state",
            )));
        }
    }
    Ok(())
}

pub(super) fn write_journal(path: &Path, journal: &Journal, _nonce: u64) -> std::io::Result<()> {
    let bytes = encode_journal(journal)?;
    let parent = open_parent(path, true)?;
    let staged_path = journal_stage_path(path)?;
    cleanup_journal_stage(path)?;
    let staged_identity = write_new_file_mode(&staged_path, &bytes, 0o600)?;
    let update = (|| {
        let staged = open_parent(&staged_path, false)?;
        if journal.state == JournalState::Committed {
            test_fail_point("before-committed-journal")?;
        }
        if identity_at(&parent, &parent.leaf)?.is_some() {
            // The lock serialises writers; replacement of the journal
            // itself is safe after both the new journal and its directory
            // are durable.
            rfs::renameat(&staged.fd, &staged.leaf, &parent.fd, &parent.leaf)?;
        } else {
            rename_noclobber(&staged, &staged.leaf, &parent, &parent.leaf)?;
        }
        if journal.state == JournalState::Committed {
            test_fail_point("committed-journal-after-rename-before-sync")?;
        }
        rfs::fsync(&parent.fd)?;
        Ok::<(), std::io::Error>(())
    })();
    if let Err(error) = update {
        let cleanup = remove_regular_exact(&staged_path, staged_identity, &sha256_bytes(&bytes))
            .err()
            .map_or_else(Vec::new, |cleanup| vec![cleanup.to_string()]);
        return Err(combine_primary_and_cleanup(error, &cleanup, true));
    }
    Ok(())
}

fn encode_journal(journal: &Journal) -> std::io::Result<Vec<u8>> {
    let json = serde_json::to_vec(journal).map_err(std::io::Error::other)?;
    let mut bytes = Vec::with_capacity(JOURNAL_STAGE_MAGIC.len() + json.len());
    bytes.extend_from_slice(JOURNAL_STAGE_MAGIC);
    bytes.extend_from_slice(&json);
    Ok(bytes)
}

pub(super) fn parse_journal(bytes: &[u8]) -> std::io::Result<Journal> {
    let json = bytes.strip_prefix(JOURNAL_STAGE_MAGIC).ok_or_else(|| {
        std::io::Error::new(
            ErrorKind::InvalidData,
            "publication journal has no owned schema marker",
        )
    })?;
    serde_json::from_slice(json).map_err(std::io::Error::other)
}

fn journal_stage_path(path: &Path) -> std::io::Result<PathBuf> {
    let parent = open_parent(path, true)?;
    Ok(parent
        .path
        .join(sibling_name(&parent.leaf, "journal-stage", 0)))
}

pub(super) fn cleanup_journal_stage(journal_path: &Path) -> std::io::Result<()> {
    let staged_path = journal_stage_path(journal_path)?;
    let Some((identity, bytes)) = read_regular(&staged_path)? else {
        return Ok(());
    };
    let owned = bytes.is_empty()
        || JOURNAL_STAGE_MAGIC.starts_with(&bytes)
        || bytes.starts_with(JOURNAL_STAGE_MAGIC);
    if !owned {
        return Err(io_failure(PublicationError::rollback(format!(
            "refusing to remove unowned journal stage '{}'; preserve it for inspection",
            staged_path.display()
        ))));
    }
    remove_regular_exact(&staged_path, identity, &sha256_bytes(&bytes))
}
