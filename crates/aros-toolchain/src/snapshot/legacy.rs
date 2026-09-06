//! Consuming raw-material conversion; no build permission or metadata exemptions.

mod objects;
mod operations;
mod store;

#[cfg(test)]
mod fault_tests;

pub(super) use operations::{Operations, SystemOperations};

use std::{
    path::{Path, PathBuf},
    time::Instant,
};

use aros_common::CancellationToken;
use rustix::fs::{self as fs, AtFlags};

use super::unix::Material;
use crate::{
    inspection::Checkout,
    recipe::GitObjectId,
    source_audit::{inventory, Budget},
    workspace::RunDirectories,
    ContractError,
};

pub(super) struct Repository {
    relative: String,
    source: PathBuf,
    commit: GitObjectId,
    tree: GitObjectId,
    entries: inventory::Inventory,
}

impl Repository {
    pub(super) fn capture(
        relative: &str,
        checkout: &Checkout<'_>,
        entries: &inventory::Inventory,
    ) -> Self {
        let (commit, tree) = checkout.identity();
        Self {
            relative: relative.into(),
            source: checkout.root.to_owned(),
            commit: commit.clone(),
            tree: tree.clone(),
            entries: entries.clone(),
        }
    }

    fn checkout<'a>(
        &self,
        root: &'a Path,
        deadline: Instant,
        cancellation: &CancellationToken,
    ) -> Result<Checkout<'a>, ContractError> {
        Checkout::inspect_controlled(root, (&self.commit, &self.tree), deadline, cancellation)
    }
}

pub(super) fn convert(
    run: &RunDirectories,
    mut material: Material,
    deadline: Instant,
    cancellation: &CancellationToken,
    operations: &mut impl Operations,
) -> Result<Material, ContractError> {
    material.revalidate(deadline, cancellation)?;
    for repository in &material.repositories {
        let depth = if repository.relative.is_empty() {
            0
        } else {
            repository.relative.split('/').count()
        };
        Budget::controlled(deadline, cancellation).check(depth + 4)?;
    }
    let leaf = material
        .root
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| ContractError::state("snapshot has no owned source role"))?;
    let pending = format!(".{leaf}-legacy-pending");
    let destination = format!("{leaf}-legacy");
    for name in [&pending, &destination] {
        match fs::statat(run.work_file(), name.as_str(), AtFlags::SYMLINK_NOFOLLOW) {
            Err(rustix::io::Errno::NOENT) => (),
            Ok(_) => {
                return Err(ContractError::state(
                    "legacy view destination or staging already exists; refusing reuse",
                ))
            }
            Err(error) => return Err(store::io_failure(error)),
        }
    }
    let work = run
        .paths()
        .work
        .as_ref()
        .ok_or_else(|| ContractError::state("missing owned work root"))?;
    relocate(
        run,
        &mut material,
        &work.join(pending),
        deadline,
        cancellation,
        operations,
    )?;
    let mut transfer_budget = Budget::controlled(deadline, cancellation);
    let mut verification_budget = Budget::controlled(deadline, cancellation);
    for repository in &material.repositories {
        transfer_budget.check(0)?;
        run.revalidate(cancellation)?;
        material.check_root()?;
        let root = material.root.join(&repository.relative);
        let source = repository.checkout(&repository.source, deadline, cancellation)?;
        let mut store = store::Store::create(
            &root,
            &repository.commit,
            deadline,
            cancellation,
            operations,
        )?;
        objects::transfer(&source, repository, &mut store, &mut transfer_budget)?;
        objects::verify(repository, &mut store, &mut verification_budget)?;
        source.recheck()?;
        store.seal(&mut material.entries, &repository.relative)?;
        run.revalidate(cancellation)?;
        material.check_root()?;
    }
    revalidate(run, &material, deadline, cancellation)?;
    relocate(
        run,
        &mut material,
        &work.join(destination),
        deadline,
        cancellation,
        operations,
    )?;
    revalidate(run, &material, deadline, cancellation)?;
    tracing::debug!(
        repositories = material.repositories.len(),
        "isolated legacy Git view prepared; execution remains blocked"
    );
    Ok(material)
}

fn relocate(
    run: &RunDirectories,
    material: &mut Material,
    destination: &Path,
    deadline: Instant,
    cancellation: &CancellationToken,
    operations: &mut impl Operations,
) -> Result<(), ContractError> {
    run.revalidate(cancellation)?;
    material.check_root()?;
    Budget::controlled(deadline, cancellation).check(0)?;
    operations.publish(&material.root, destination).map_err(|error| {
        let operations::PublicationFailure { class, kind } = error;
        ContractError::state(format!(
            "legacy view publication failed ({class:?}, {kind:?}); staged or complete material is retained"
        ))
    })?;
    destination.clone_into(&mut material.root);
    material.check_root()?;
    Budget::controlled(deadline, cancellation).check(0)?;
    run.revalidate(cancellation)
}

pub(super) fn revalidate(
    run: &RunDirectories,
    material: &Material,
    deadline: Instant,
    cancellation: &CancellationToken,
) -> Result<(), ContractError> {
    run.revalidate(cancellation)?;
    material.revalidate(deadline, cancellation)?;
    for repository in &material.repositories {
        let root = material.root.join(&repository.relative);
        let checkout = repository.checkout(&root, deadline, cancellation)?;
        inventory::verify_index(&checkout, &repository.entries)?;
        if !checkout
            .git(&["status", "--porcelain", "--untracked-files=no"])?
            .is_empty()
        {
            return Err(ContractError::identity(
                "isolated source is incompatible with the legacy clean-status contract",
            ));
        }
    }
    material.revalidate(deadline, cancellation)?;
    run.revalidate(cancellation)
}
