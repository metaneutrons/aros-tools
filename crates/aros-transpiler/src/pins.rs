//! The digests this crate recognises its modelled capabilities by.
//!
//! The values are in `pinned-digests.pins`, next to this crate's manifest, and
//! that file carries the reasoning for each one. Here is only the embedding and
//! the lookup, over the shared reader in `aros_common::pins`.
//!
//! They used to be 26 `const` declarations in `parser.rs`, three of which held
//! the same archive digest. Keeping data about the tree in the 12 000-line file
//! the decomposition has to split meant every re-pin was a code edit there, and
//! every split would have had to carry the values along. OPEN-POINTS 46.

const PINS: &str = include_str!("../pinned-digests.pins");

const FILE: &str = "aros-transpiler/pinned-digests.pins";

/// One pinned digest by name.
///
/// # Panics
///
/// If the name is absent or its value is not a sha256, which is an error in the
/// data file rather than a property of the tree being transpiled. Failing there
/// beats reclassifying a capability on a typo.
#[must_use]
pub fn pin(name: &str) -> &'static str {
    aros_common::pins::pin(PINS, FILE, name)
}

/// Every pin this crate looks up.
///
/// Listed once so a test can resolve all of them, including the capabilities no
/// test exercises. A name added here without an entry in the file fails that
/// test rather than some later run.
pub const NAMES: &[&str] = &[
    "adflib-configure-manifest",
    "ahi-mmakefile",
    "cunit-archive",
    "glapi-generator-capability",
    "grub2-aros-mmakefile",
    "grub2-host-mmakefile",
    "grub2-version-file",
    "mako-archive",
    "markupsafe-archive",
    "mesa-local-patch",
    "mesa-sse41-capability",
    "mesa-sse41-config-context",
    "mesa-sse41-local-context",
    "mesa-sse41-manifest",
    "mesa20-archive",
    "mesa20-config",
    "mesa20-cxx-compat-new",
    "mesa20-driver-script",
    "mesa20-main-mmakefile",
    "mesautil-generator-capability",
    "nouveau-drm-mmakefile",
    "nouveau-drm-source-manifest",
    "nouveau-gallium-source-manifest",
    "wireless-configure-manifest",
];

#[cfg(test)]
mod tests {
    use super::{FILE, PINS};
    use std::collections::BTreeSet;

    /// The file as a whole, rather than only the entries some test happens to
    /// ask for: a malformed line or a duplicated name would otherwise wait
    /// until the capability that needs it is exercised.
    #[test]
    fn every_entry_is_a_unique_named_digest() {
        let entries = aros_common::pins::entries(PINS, FILE);
        assert!(entries.len() >= 24, "{} entries", entries.len());
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
            assert!(seen.insert(name), "{name} is pinned twice");
        }
    }

    /// Every name the crate asks for has to be in the file. A typo would
    /// otherwise be a panic on the first run that reaches that capability, and
    /// the capabilities are not all reached by the test suite.
    #[test]
    fn every_name_the_crate_uses_resolves() {
        for name in super::NAMES {
            let _ = super::pin(name);
        }
    }
}
