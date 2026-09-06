//! Strict bounded Git tree/index and raw batch parsing. No filters or checkout.

use std::collections::{BTreeMap, BTreeSet};

use aros_common::{sha256_bytes, Sha256Digest};

use super::{mismatch, Budget, MAX_BLOB_BYTES, MAX_INVENTORY_BYTES};
use crate::{
    inspection::Checkout,
    recipe::{safe_relative_path, GitObjectId},
    ContractError,
};

#[derive(Clone)]
pub struct Entry {
    pub mode: &'static str,
    pub oid: GitObjectId,
    pub size: usize,
    pub digest: Option<Sha256Digest>,
}

pub type Inventory = BTreeMap<String, Entry>;

pub fn read(checkout: &Checkout<'_>, budget: &mut Budget) -> Result<Inventory, ContractError> {
    let bytes = checkout.git_input(
        &[
            "ls-tree",
            "-r",
            "-t",
            "-l",
            "-z",
            "--full-tree",
            "--abbrev=40",
            checkout.tree.as_str(),
        ],
        &[],
        MAX_INVENTORY_BYTES,
    )?;
    parse(&bytes, budget)
}

fn parse(bytes: &[u8], budget: &mut Budget) -> Result<Inventory, ContractError> {
    let mut entries = BTreeMap::new();
    for record in records(bytes)? {
        budget.check(0)?;
        let (metadata, path) = record
            .split_once('\t')
            .ok_or_else(|| mismatch("malformed Git tree record"))?;
        if !safe_relative_path(path)
            || path.len() > 4096
            || path
                .split('/')
                .any(|part| part.eq_ignore_ascii_case(".git"))
        {
            return Err(mismatch(
                "committed source path is unsafe or exceeds 4096 bytes",
            ));
        }
        let fields: Vec<_> = metadata.split_ascii_whitespace().collect();
        let (mode, blob) = match fields.as_slice() {
            ["040000", "tree", _, "-"] => ("040000", false),
            ["160000", "commit", _, "-"] => ("160000", false),
            ["100644", "blob", _, _] => ("100644", true),
            ["100755", "blob", _, _] => ("100755", true),
            ["120000", "blob", _, _] => ("120000", true),
            _ => return Err(mismatch("unsupported Git tree mode, kind or size")),
        };
        let oid = GitObjectId::try_from(fields[2].to_owned()).map_err(ContractError::identity)?;
        let size = if blob {
            fields[3]
                .parse::<usize>()
                .map_err(|_| mismatch("invalid or missing Git blob size"))?
        } else {
            0
        };
        if size > MAX_BLOB_BYTES {
            return Err(ContractError::invalid(
                "source blob exceeds the 64 MiB inspection limit",
            ));
        }
        budget.entry(size)?;
        if entries
            .insert(
                path.to_owned(),
                Entry {
                    mode,
                    oid,
                    size,
                    digest: None,
                },
            )
            .is_some()
        {
            return Err(mismatch("duplicate committed source path"));
        }
    }
    Ok(entries)
}

pub fn verify_index(checkout: &Checkout<'_>, entries: &Inventory) -> Result<(), ContractError> {
    let bytes = checkout.git_input(
        &["ls-files", "--stage", "--full-name", "-z"],
        &[],
        MAX_INVENTORY_BYTES,
    )?;
    let mut observed = BTreeSet::new();
    for record in records(&bytes)? {
        let (metadata, path) = record
            .split_once('\t')
            .ok_or_else(|| mismatch("malformed Git index record"))?;
        let fields: Vec<_> = metadata.split_ascii_whitespace().collect();
        let Some(expected) = entries.get(path) else {
            return Err(mismatch("index contains an undeclared source path"));
        };
        if fields.as_slice() != [expected.mode, expected.oid.as_str(), "0"]
            || expected.mode == "040000"
            || !observed.insert(path)
        {
            return Err(mismatch(
                "source index differs from the selected tree or contains unresolved entries",
            )
            .source_path(path));
        }
    }
    if observed.len()
        != entries
            .values()
            .filter(|entry| entry.mode != "040000")
            .count()
    {
        return Err(mismatch("source index omits committed entries"));
    }
    Ok(())
}

pub fn measure_blobs(
    checkout: &Checkout<'_>,
    entries: &mut Inventory,
    budget: &Budget,
    visitor: &mut super::Visitor<'_>,
) -> Result<(), ContractError> {
    // One bounded batch handles many small files, not one process per file.
    // IDs, never paths, are supplied on stdin: no filters or symlink following.
    let mut pending = entries
        .iter_mut()
        .filter(|(_, entry)| matches!(entry.mode, "100644" | "100755" | "120000"))
        .peekable();
    while pending.peek().is_some() {
        budget.check(0)?;
        let mut batch = Vec::new();
        let mut input = String::new();
        let mut limit = 0;
        while let Some((path, entry)) =
            pending.next_if(|(_, entry)| limit == 0 || limit + entry.size + 128 <= 8 * 1024 * 1024)
        {
            limit += entry.size + 128;
            input.push_str(entry.oid.as_str());
            input.push('\n');
            batch.push((path, entry));
            if batch.len() == 4096 {
                break;
            }
        }
        let output = checkout.git_input(&["cat-file", "--batch"], input.as_bytes(), limit)?;
        let mut remaining = output.as_slice();
        for (path, entry) in batch {
            budget.check(0)?;
            let header = format!("{} blob {}\n", entry.oid.as_str(), entry.size);
            remaining = remaining
                .strip_prefix(header.as_bytes())
                .ok_or_else(|| mismatch("raw Git batch identity, kind or size mismatch"))?;
            let bytes = remaining
                .get(..entry.size)
                .ok_or_else(|| mismatch("incomplete raw Git blob"))?;
            entry.digest = Some(sha256_bytes(bytes));
            visitor(path, entry, bytes)?;
            remaining = remaining
                .get(entry.size..)
                .and_then(|rest| rest.strip_prefix(b"\n"))
                .ok_or_else(|| mismatch("incomplete raw Git batch delimiter"))?;
        }
        if !remaining.is_empty() {
            return Err(mismatch("unexpected trailing raw Git batch data"));
        }
    }
    Ok(())
}

fn records(bytes: &[u8]) -> Result<impl Iterator<Item = &str>, ContractError> {
    let text =
        std::str::from_utf8(bytes).map_err(|_| mismatch("Git source inventory is not UTF-8"))?;
    if !text.is_empty() && !text.ends_with('\0') {
        return Err(mismatch("incomplete Git source inventory"));
    }
    Ok(text.split_terminator('\0'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn budget() -> Budget {
        Budget::new(Instant::now() + Duration::from_secs(10))
    }

    #[test]
    fn malformed_inventory_fails_closed_without_echoing_material() {
        for record in [
            "100644 blob OID 0\tsecret-private", // missing delimiter
            "100644 blob OID 0\t../secret-private\0",
            "100644 blob OID 0\t.git/secret-private\0",
            "100644 blob OID 0\tsecret-private/.GIT/config\0",
            "100644 blob OID 0\t/secret-private\0",
            "100644 blob OID 0\tsecret-private\\escape\0",
            "100644 blob OID 0\tsecret-private\nspoof\0",
            "100644 blob OID -\tsecret-private\0",
            "100644 blob OID 67108865\tsecret-private\0",
            "100644 blob BAD 0\tsecret-private\0",
            "040000 blob OID -\tsecret-private\0",
            "100600 blob OID 0\tsecret-private\0",
            "160000 commit OID 0\tsecret-private\0",
            "\0",
        ] {
            let material = record.replace("OID", &"1".repeat(40));
            let error = parse(material.as_bytes(), &mut budget())
                .err()
                .expect("must reject");
            assert!(!error.to_string().contains("secret-private"));
        }
        assert!(parse(b"\xff\0", &mut budget()).is_err());
    }

    #[test]
    fn inventory_accepts_raw_utf8_spaces_and_rejects_duplicates() {
        let record = format!("100644 blob {} 0\tGröße with spaces\0", "1".repeat(40));
        let entries = parse(record.as_bytes(), &mut budget()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries["Größe with spaces"].size, 0);
        assert!(parse(record.repeat(2).as_bytes(), &mut budget()).is_err());
    }

    #[test]
    fn all_roots_share_entry_byte_depth_and_time_limits() {
        let record = format!("100644 blob {} 1\tfile\0", "1".repeat(40));
        let mut entries = budget();
        entries.entries = super::super::MAX_ENTRIES;
        assert!(parse(record.as_bytes(), &mut entries).is_err());
        let mut bytes = budget();
        bytes.bytes = super::super::MAX_SOURCE_BYTES;
        assert!(parse(record.as_bytes(), &mut bytes).is_err());
        let mut expired = Budget::new(Instant::now());
        assert!(parse(record.as_bytes(), &mut expired).is_err());
        assert!(budget().check(65).is_err());
    }
}
