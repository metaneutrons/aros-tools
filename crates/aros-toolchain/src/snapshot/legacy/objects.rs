//! Exact bounded Git closure transfer, followed by independently measured objects.

use std::collections::BTreeSet;

use super::{store::Store, Repository};
use crate::{
    inspection::Checkout,
    source_audit::{inventory, Budget, MAX_BLOB_BYTES, MAX_INVENTORY_BYTES},
    ContractError,
};

struct Object<'a> {
    id: &'a str,
    kind: &'static str,
    size: usize,
}

fn selected(repository: &Repository) -> Vec<(&str, &'static str)> {
    let mut seen = BTreeSet::new();
    repository
        .entries
        .values()
        .filter(|entry| matches!(entry.mode, "100644" | "100755" | "120000"))
        .map(|entry| (entry.oid.as_str(), "blob"))
        // A reverse preorder visits descendants before their ancestors.
        .chain(
            repository
                .entries
                .values()
                .rev()
                .filter(|entry| entry.mode == "040000")
                .map(|entry| (entry.oid.as_str(), "tree")),
        )
        .chain([
            (repository.tree.as_str(), "tree"),
            (repository.commit.as_str(), "commit"),
        ])
        .filter(|(id, _)| seen.insert(*id))
        .collect()
}

fn sizes<'a>(
    source: &Checkout<'_>,
    selected: &[(&'a str, &'static str)],
    budget: &mut Budget,
) -> Result<Vec<Object<'a>>, ContractError> {
    let mut objects = Vec::new();
    for batch in selected.chunks(4096) {
        budget.check(0)?;
        let mut input = String::new();
        for (id, _) in batch {
            input.push_str(id);
            input.push('\n');
        }
        let output = source.git_input(
            &[
                "cat-file",
                "--batch-check=%(objectname) %(objecttype) %(objectsize)",
            ],
            input.as_bytes(),
            batch.len() * 128,
        )?;
        let text = std::str::from_utf8(&output).map_err(|_| invalid())?;
        let rows: Vec<_> = text.split_terminator('\n').collect();
        if !text.ends_with('\n') || rows.len() != batch.len() {
            return Err(invalid());
        }
        for (row, (id, kind)) in rows.into_iter().zip(batch) {
            let parts: Vec<_> = row.split(' ').collect();
            if parts.len() != 3 || parts[0] != *id || parts[1] != *kind {
                return Err(invalid());
            }
            let size: usize = parts[2].parse().map_err(|_| invalid())?;
            if size > MAX_BLOB_BYTES {
                return Err(ContractError::invalid(
                    "Git metadata transfer object exceeds 64 MiB",
                ));
            }
            budget.entry(size)?;
            objects.push(Object { id, kind, size });
        }
    }
    Ok(objects)
}

pub(super) fn transfer(
    source: &Checkout<'_>,
    repository: &Repository,
    store: &mut Store,
    budget: &mut Budget,
) -> Result<(), ContractError> {
    let selected = selected(repository);
    let objects = sizes(source, &selected, budget)?;
    let mut remaining = objects.iter().peekable();
    while remaining.peek().is_some() {
        budget.check(0)?;
        let mut input = String::new();
        let mut declared = 0;
        let mut count = 0;
        while let Some(object) = remaining
            .next_if(|object| declared == 0 || declared + object.size + 128 <= 8 * 1024 * 1024)
        {
            input.push_str(object.id);
            input.push('\n');
            declared += object.size + 128;
            count += 1;
            if count == 4096 {
                break;
            }
        }
        // Compression=0 with no reuse/deltas; this deliberately loose bound is
        // checked by exact capture AND the receiving Git max-input-size gate.
        let packed = source.git_input(
            &[
                "pack-objects",
                "--stdout",
                "--no-reuse-object",
                "--window=0",
                "--depth=0",
                "--threads=1",
                "--compression=0",
            ],
            input.as_bytes(),
            declared * 2 + 4096,
        )?;
        store.import(&packed)?;
    }
    let observed = store.git(
        &[
            "cat-file",
            "--batch-all-objects",
            "--batch-check=%(objectname) %(objecttype) %(objectsize)",
        ],
        &[],
        MAX_INVENTORY_BYTES,
    )?;
    let expected: BTreeSet<_> = objects
        .iter()
        .map(|object| format!("{} {} {}", object.id, object.kind, object.size))
        .collect();
    let text = std::str::from_utf8(&observed).map_err(|_| invalid())?;
    let rows: Vec<_> = text.split_terminator('\n').collect();
    let actual: BTreeSet<_> = rows.iter().map(|row| (*row).to_owned()).collect();
    if !text.ends_with('\n') || rows.len() != actual.len() || expected != actual {
        return Err(ContractError::identity(
            "isolated Git object IDs, kinds or sizes differ from the complete expected closure",
        ));
    }
    Ok(())
}

pub(super) fn verify(
    repository: &Repository,
    store: &mut Store,
    budget: &mut Budget,
) -> Result<(), ContractError> {
    if !store
        .git(
            &[
                "fsck",
                "--strict",
                "--full",
                "--no-reflogs",
                "--no-dangling",
                repository.commit.as_str(),
            ],
            &[],
            MAX_INVENTORY_BYTES,
        )?
        .is_empty()
    {
        return Err(ContractError::identity(
            "isolated Git object validation reported findings",
        ));
    }
    store.index(&repository.tree)?;
    store.check()?;
    let checkout = repository.checkout(&store.root, store.deadline, &store.cancellation)?;
    let mut entries = inventory::read(&checkout, budget)?;
    inventory::verify_index(&checkout, &entries)?;
    inventory::measure_blobs(&checkout, &mut entries, budget, &mut |_, _, _| Ok(()))?;
    if entries.len() != repository.entries.len()
        || entries.iter().any(|(path, actual)| {
            repository.entries.get(path).is_none_or(|expected| {
                actual.mode != expected.mode
                    || actual.oid != expected.oid
                    || actual.size != expected.size
                    || actual.digest != expected.digest
            })
        })
    {
        return Err(ContractError::identity(
            "verified Git tree/blob material differs from the raw snapshot inventory",
        ));
    }
    checkout.recheck()?;
    store.check()
}

fn invalid() -> ContractError {
    ContractError::identity("invalid Git object identity, kind or size response")
}
