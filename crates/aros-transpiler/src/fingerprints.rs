//! Fingerprints for opaque capability inputs the transpiler expands into
//! hard-coded jobs.
//!
//! The values are in `capability-fingerprints.pins`, next to this crate's
//! manifest. That file states the narrow admission policy and reasoning.
//! Download archives, ordinary repository files and local patches are
//! deliberately excluded.

const FINGERPRINTS: &str = include_str!("../capability-fingerprints.pins");
const FILE: &str = "aros-transpiler/capability-fingerprints.pins";

/// Returns one embedded capability fingerprint by name.
///
/// # Errors
///
/// Returns an error if the embedded capability data is malformed or the name
/// is absent.
pub fn fingerprint(name: &str) -> Result<&'static str, String> {
    aros_common::pins::try_pin(FINGERPRINTS, FILE, name)
}

/// Validates the complete embedded fingerprint registry at process startup.
///
/// # Errors
///
/// Returns an error for malformed values, duplicate names, missing names, or
/// entries which are not used by this binary.
pub fn validate() -> Result<(), String> {
    let entries = aros_common::pins::try_entries(FINGERPRINTS, FILE)?;
    let mut names = std::collections::BTreeSet::new();
    for (name, value) in &entries {
        if !names.insert(*name) {
            return Err(format!("{FILE}: duplicate fingerprint name {name}"));
        }
        if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(format!("{FILE}: {name} is not a sha256: {value:?}"));
        }
    }
    for name in NAMES {
        let _ = fingerprint(name)?;
    }
    if names.len() != NAMES.len() {
        return Err(format!(
            "{FILE}: contains {} entries but this binary declares {} names",
            names.len(),
            NAMES.len()
        ));
    }
    Ok(())
}

/// Every fingerprint this crate looks up.
pub const NAMES: &[&str] = &[
    "glapi-generator-capability",
    "mesa-sse41-capability",
    "mesa-sse41-config-context",
    "mesa-sse41-local-context",
    "mesa-sse41-manifest",
    "mesa20-compiler-manifest",
    "mesa20-compiler-recipe",
    "mesa20-core-manifest",
    "mesa20-core-recipe",
    "mesa20-galliumaux-manifest",
    "mesa20-galliumaux-recipe",
    "mesa20-vc4-manifest",
    "mesa20-vc4-recipe",
    "mesa20-v3d-recipe",
    "mesautil-generator-capability",
];

#[cfg(test)]
mod tests {
    use super::{FILE, FINGERPRINTS};
    use std::collections::BTreeSet;

    #[test]
    fn every_entry_is_a_unique_named_digest() {
        let entries = aros_common::pins::entries(FINGERPRINTS, FILE);
        assert_eq!(entries.len(), 15, "{} entries", entries.len());
        let mut seen = BTreeSet::new();
        for (name, value) in entries {
            assert!(
                !name.is_empty()
                    && name
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "{name} is not a kebab-case name"
            );
            assert!(
                value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "{name} is not a sha256"
            );
            assert!(seen.insert(name), "{name} occurs twice");
        }
    }

    #[test]
    fn every_name_the_crate_uses_resolves() {
        for name in super::NAMES {
            let _ = super::fingerprint(name).unwrap();
        }
    }

    #[test]
    fn complete_registry_passes_non_panicking_validation() {
        super::validate().unwrap();
    }
}
