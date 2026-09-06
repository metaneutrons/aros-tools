//! Filesystem expectations without pretending generated metadata has a Git OID.

use std::collections::BTreeMap;

use aros_common::Sha256Digest;

/// Raw files and generated Git metadata share byte/type verification, not identity.
#[derive(Clone)]
pub struct Entry {
    pub mode: &'static str,
    pub size: usize,
    pub digest: Option<Sha256Digest>,
}

pub type Inventory = BTreeMap<String, Entry>;

impl From<&super::inventory::Entry> for Entry {
    fn from(entry: &super::inventory::Entry) -> Self {
        Self {
            mode: entry.mode,
            size: entry.size,
            digest: entry.digest.clone(),
        }
    }
}
