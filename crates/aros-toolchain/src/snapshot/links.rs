//! Resolve links against committed, flattened metadata, never the live host.

use std::collections::{BTreeMap, VecDeque};

use crate::{
    source_audit::{material::Inventory, Budget},
    ContractError,
};

pub(super) fn target(bytes: &[u8]) -> Result<&str, ContractError> {
    let target = std::str::from_utf8(bytes).map_err(|_| invalid())?;
    if target.is_empty()
        || target.len() > 4096
        || target.starts_with('/')
        || target
            .chars()
            .any(|character| character.is_control() || matches!(character, '\\' | ':'))
    {
        return Err(invalid());
    }
    Ok(target)
}

pub(super) fn validate(
    entries: &Inventory,
    links: &BTreeMap<String, String>,
    budget: &Budget,
) -> Result<(), ContractError> {
    for (path, link) in links {
        resolve(path, link, entries, links, budget).map_err(|error| error.source_path(path))?;
    }
    Ok(())
}

fn resolve(
    path: &str,
    link: &str,
    entries: &Inventory,
    links: &BTreeMap<String, String>,
    budget: &Budget,
) -> Result<(), ContractError> {
    let mut resolved: Vec<_> = path.split('/').collect();
    resolved.pop();
    let mut pending: VecDeque<_> = link.split('/').collect();
    let mut followed = 0;
    while let Some(part) = pending.pop_front() {
        budget.check(0)?;
        match part {
            "" | "." => continue,
            ".." => {
                if resolved.pop().is_none() {
                    return Err(invalid());
                }
                continue;
            }
            _ => {}
        }
        resolved.push(part);
        let selected = resolved.join("/");
        let entry = entries.get(&selected).ok_or_else(invalid)?;
        if entry.mode == "120000" {
            followed += 1;
            if followed > 40 {
                return Err(invalid());
            }
            resolved.pop();
            let target = links.get(&selected).ok_or_else(invalid)?;
            for part in target.split('/').rev() {
                pending.push_front(part);
            }
        } else if !pending.is_empty() && entry.mode != "040000" {
            return Err(invalid());
        }
    }
    Ok(())
}

fn invalid() -> ContractError {
    ContractError::identity("snapshot symlink must resolve inside its selected input; absolute/unsafe targets, escapes, missing targets, non-directory traversal and more than 40 link expansions are rejected")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source_audit::material::Entry;
    use std::time::{Duration, Instant};

    #[test]
    fn target_syntax_rejects_private_unsafe_bytes_without_echoing_them() {
        for bytes in [
            b"".as_slice(),
            b"/private/value",
            b"a\\private",
            b"a:private",
            b"a\nprivate",
            b"a\0private",
            b"\xffprivate",
        ] {
            let error = target(bytes).unwrap_err();
            assert!(!error.to_string().contains("private"));
        }
        assert!(target(&vec![b'a'; 4097]).is_err());
        assert_eq!(
            target(b"../Gr\xc3\xb6\xc3\x9fe file").unwrap(),
            "../Größe file"
        );
    }

    #[test]
    fn link_expansion_precedes_parent_traversal_and_has_a_finite_budget() {
        let mut entries = Inventory::new();
        for (path, mode) in [
            ("dir", "040000"),
            ("dir/deep", "040000"),
            ("empty", "100644"),
            ("alias", "120000"),
            ("start", "120000"),
        ] {
            entries.insert(
                path.to_owned(),
                Entry {
                    mode,
                    size: 0,
                    digest: None,
                },
            );
        }
        let mut links = BTreeMap::from([
            ("alias".into(), "dir/deep".into()),
            ("start".into(), "alias/../../empty".into()),
        ]);
        let budget = Budget::new(Instant::now() + Duration::from_secs(5));
        // Lexically this looks like a root escape, but after alias expansion it
        // stays inside. Conversely a shorter alias must not hide a real escape.
        assert!(validate(&entries, &links, &budget).is_ok());
        links.insert("alias".into(), "dir".into());
        assert!(validate(&entries, &links, &budget).is_err());
        links.insert("alias".into(), "start".into());
        assert!(validate(&entries, &links, &budget).is_err());
        assert!(validate(&entries, &links, &Budget::new(Instant::now())).is_err());
    }
}
