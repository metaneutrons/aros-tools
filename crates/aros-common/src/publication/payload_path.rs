//! Cross-host collision keys for source-controlled payload paths.

use std::io::ErrorKind;
use std::path::{Component, Path};

use unicode_normalization::UnicodeNormalization;

/// Return a host-independent collision key for a source-controlled payload.
///
/// Unlike generated-output collision keys, this accepts UTF-8 names because
/// upstream toolchain payloads may legitimately contain them. It still rejects
/// components that cannot be represented safely on every supported host and
/// normalizes canonically equivalent Unicode spellings before folding case, so
/// decomposed names on APFS/HFS+ cannot alias a distinct archive entry.
///
/// # Errors
///
/// Returns `InvalidInput` when a component is unsafe, non-UTF-8, or the path
/// is not a non-empty relative path.
pub fn payload_casefold_path_key(path: &Path) -> std::io::Result<String> {
    let mut folded = Vec::new();
    for component in path.components() {
        let Component::Normal(value) = component else {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                format!("'{}' is not a relative payload path", path.display()),
            ));
        };
        let value = value.to_str().ok_or_else(|| {
            std::io::Error::new(
                ErrorKind::InvalidInput,
                format!("'{}' is not valid UTF-8", path.display()),
            )
        })?;
        let invalid_character = value.chars().any(|character| {
            character.is_control()
                || matches!(
                    character,
                    '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
                )
        });
        if value.is_empty()
            || value == "."
            || value == ".."
            || value.ends_with(['.', ' '])
            || invalid_character
            || value.len() > 255
        {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                format!("'{value}' is not a portable payload component"),
            ));
        }
        let stem = value
            .split('.')
            .next()
            .unwrap_or(value)
            .to_ascii_uppercase();
        let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || stem
                .strip_prefix("COM")
                .or_else(|| stem.strip_prefix("LPT"))
                .is_some_and(|suffix| {
                    suffix.len() == 1 && matches!(suffix.as_bytes()[0], b'1'..=b'9')
                });
        if reserved {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                format!("'{value}' is a reserved Windows device name"),
            ));
        }
        folded.push(value.nfc().flat_map(char::to_lowercase).collect::<String>());
    }
    if folded.is_empty() {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            "a payload path must contain at least one component",
        ));
    }
    Ok(folded.join("/"))
}
