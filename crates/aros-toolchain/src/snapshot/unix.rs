//! Raw object sink with descriptor-relative exclusive writes and shared checks.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write;
use std::os::unix::fs::MetadataExt as _;
use std::path::PathBuf;
use std::time::Instant;

use aros_common::{publication::publish_prepared_source_tree_noclobber, CancellationToken};
use rustix::fs::{self as fs, AtFlags, Mode, OFlags};

use super::SourceRole;
use crate::{
    filesystem::{open_directory, DIRECTORY},
    inspection::Checkout,
    source_audit::{
        self,
        inventory::Entry,
        material::{Entry as MaterialEntry, Inventory},
        worktree, Budget,
    },
    workspace::RunDirectories,
    ContractError, Recipe,
};

pub(super) struct Material {
    pub root: PathBuf,
    pub(super) identity: (u64, u64),
    pub(super) entries: Inventory,
    pub(super) repositories: Vec<super::legacy::Repository>,
}

impl Material {
    pub fn revalidate(
        &self,
        deadline: Instant,
        cancellation: &CancellationToken,
    ) -> Result<(), ContractError> {
        let mut budget = Budget::controlled(deadline, cancellation);
        budget.check(0)?;
        self.check_root()?;
        worktree::verify_material(&self.root, &self.entries, &mut budget)?;
        self.check_root()?;
        budget.check(0)
    }

    pub(super) fn check_root(&self) -> Result<(), ContractError> {
        if identity(&open_directory(&self.root).map_err(io_failure)?)? != self.identity {
            return Err(ContractError::state(
                "source snapshot root identity changed",
            ));
        }
        Ok(())
    }
}

pub(super) fn prepare(
    run: &RunDirectories,
    role: SourceRole,
    recipe: &Recipe,
    deadline: Instant,
    cancellation: &CancellationToken,
) -> Result<Material, ContractError> {
    let (source, selected) = role.selection(run, recipe);
    let checkout = Checkout::inspect_controlled(source, selected, deadline, cancellation)?;
    // Fail dirty/missing material before creating a staging leaf. The second
    // pass copies committed blobs and repeats all checks under the SAME deadline.
    source_audit::verify(
        &checkout,
        &mut Budget::controlled(deadline, cancellation),
        0,
    )?;
    run.revalidate(cancellation)?;
    let leaf = role.leaf();
    let pending = format!(".{leaf}-pending");
    for name in [leaf, &pending] {
        match fs::statat(run.work_file(), name, AtFlags::SYMLINK_NOFOLLOW) {
            Err(rustix::io::Errno::NOENT) => {}
            Ok(_) => {
                return Err(ContractError::state(
                    "source snapshot destination or staging leaf already exists; refusing reuse",
                ))
            }
            Err(error) => return Err(io_failure(error)),
        }
    }
    fs::mkdirat(
        run.work_file(),
        pending.as_str(),
        Mode::RUSR | Mode::WUSR | Mode::XUSR,
    )
    .map_err(io_failure)?;
    let file = File::from(
        fs::openat(run.work_file(), pending.as_str(), DIRECTORY, Mode::empty())
            .map_err(io_failure)?,
    );
    let root_identity = identity(&file)?;
    run.revalidate(cancellation)?;
    let mut writer = Writer {
        root: &file,
        entries: Inventory::new(),
        links: BTreeMap::new(),
        budget: Budget::controlled(deadline, cancellation),
    };
    let mut repositories = Vec::new();
    source_audit::visit_repositories(
        &checkout,
        &mut Budget::controlled(deadline, cancellation),
        0,
        "",
        &mut |path, entry, bytes| writer.put(path, entry, bytes),
        &mut |path, checkout, entries| {
            if repositories.len() == 1024 {
                return Err(ContractError::invalid(
                    "snapshot exceeds 1024 repositories per input",
                ));
            }
            repositories.push(super::legacy::Repository::capture(path, checkout, entries));
            Ok(())
        },
    )?;
    super::links::validate(&writer.entries, &writer.links, &writer.budget)?;
    for (path, target) in &writer.links {
        writer.budget.check(0)?;
        let (parent, leaf) = parent(&file, path)?;
        fs::symlinkat(target.as_str(), &parent, leaf).map_err(io_failure)?;
    }
    let work = run
        .paths()
        .work
        .as_ref()
        .ok_or_else(|| ContractError::state("missing owned work root"))?;
    let mut material = Material {
        root: work.join(&pending),
        identity: root_identity,
        entries: writer.entries,
        repositories,
    };
    run.revalidate(cancellation)?;
    material.revalidate(deadline, cancellation)?;
    run.revalidate(cancellation)?;
    // Shared publication verifies no-follow tree/parent bindings and fsyncs
    // before a no-replace rename. Any uncertain commit remains a fatal error.
    let destination = work.join(leaf);
    publish_prepared_source_tree_noclobber(&material.root, &destination).map_err(|error| {
        let class = aros_common::publication::publication_failure_class(&error);
        ContractError::state(format!(
            "source snapshot publication failed ({class:?}); staging or complete material is retained"
        ))
    })?;
    material.root = destination;
    material.revalidate(deadline, cancellation)?;
    run.revalidate(cancellation)?;
    tracing::debug!(
        role = leaf,
        entries = material.entries.len(),
        "isolated raw source material prepared; execution remains blocked"
    );
    Ok(material)
}

struct Writer<'a> {
    root: &'a File,
    entries: Inventory,
    links: BTreeMap<String, String>,
    budget: Budget,
}

impl Writer<'_> {
    fn put(&mut self, path: &str, entry: &Entry, bytes: &[u8]) -> Result<(), ContractError> {
        self.budget.check(path.split('/').count())?;
        if path.len() > 4096 || self.entries.contains_key(path) {
            return Err(ContractError::identity(
                "flattened source path is duplicated or exceeds 4096 bytes",
            ));
        }
        let (parent, leaf) = parent(self.root, path)?;
        let mut record = MaterialEntry::from(entry);
        match entry.mode {
            "040000" | "160000" => {
                fs::mkdirat(&parent, leaf, Mode::RUSR | Mode::WUSR | Mode::XUSR)
                    .map_err(io_failure)?;
                record.mode = "040000";
            }
            "100644" | "100755" => {
                let mut file = File::from(
                    fs::openat(
                        &parent,
                        leaf,
                        OFlags::WRONLY
                            | OFlags::CREATE
                            | OFlags::EXCL
                            | OFlags::NOFOLLOW
                            | OFlags::CLOEXEC,
                        Mode::RUSR | Mode::WUSR,
                    )
                    .map_err(io_failure)?,
                );
                for chunk in bytes.chunks(64 * 1024) {
                    self.budget.check(0)?;
                    file.write_all(chunk).map_err(io_failure)?;
                }
                let mode = if entry.mode == "100755" { 0o700 } else { 0o600 };
                fs::fchmod(&file, Mode::from_raw_mode(mode)).map_err(io_failure)?;
                // Durability is batched by the shared publication traversal.
            }
            "120000" => {
                let target =
                    super::links::target(bytes).map_err(|error| error.source_path(path))?;
                self.links.insert(path.to_owned(), target.to_owned());
            }
            _ => return Err(ContractError::identity("unsupported snapshot entry mode")),
        }
        self.entries.insert(path.to_owned(), record);
        Ok(())
    }
}

pub(super) fn parent<'a>(root: &File, path: &'a str) -> Result<(File, &'a str), ContractError> {
    let mut parts = path.split('/').collect::<Vec<_>>();
    let leaf = parts
        .pop()
        .ok_or_else(|| ContractError::identity("empty snapshot path"))?;
    let mut parent = root.try_clone().map_err(io_failure)?;
    for part in parts {
        parent =
            File::from(fs::openat(&parent, part, DIRECTORY, Mode::empty()).map_err(io_failure)?);
    }
    Ok((parent, leaf))
}

pub(super) fn identity(file: &File) -> Result<(u64, u64), ContractError> {
    let metadata = file.metadata().map_err(io_failure)?;
    Ok((metadata.dev(), metadata.ino()))
}

fn io_failure(error: impl Into<std::io::Error>) -> ContractError {
    ContractError::state(format!(
        "source snapshot descriptor I/O failed ({:?}); partial material is retained",
        error.into().kind()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn cancelled_sink_retains_prior_material_and_writes_no_next_entry() {
        let temporary = tempfile::tempdir().unwrap();
        let root = open_directory(&temporary.path().canonicalize().unwrap()).unwrap();
        let token = CancellationToken::default();
        let mut writer = Writer {
            root: &root,
            entries: Inventory::new(),
            links: BTreeMap::new(),
            budget: Budget::controlled(Instant::now() + Duration::from_secs(5), &token),
        };
        let entry = Entry {
            mode: "100644",
            oid: crate::recipe::GitObjectId::try_from("1".repeat(40)).unwrap(),
            size: 4,
            digest: Some(aros_common::sha256_bytes(b"keep")),
        };
        writer.put("first", &entry, b"keep").unwrap();
        token.cancel();
        assert!(writer.put("second", &entry, b"keep").is_err());
        assert_eq!(
            std::fs::read(temporary.path().join("first")).unwrap(),
            b"keep"
        );
        assert!(!temporary.path().join("second").exists());
    }
}
