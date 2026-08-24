//! Pinned digests, read from a data file instead of a `.rs` constant.
//!
//! Several capabilities in this workspace are recognised only for the exact
//! inputs that were audited, which means carrying a sha256 of each of them.
//! Those values are data about a tree: they change on a different schedule than
//! the code that reads them, and while they sat in source files every re-pin
//! was a code edit in the file the reader happened to live in. That is the
//! coupling OPEN-POINTS 7 recorded for `aros-verify` and 46 for the
//! transpiler's 26.
//!
//! Each crate embeds its own file with `include_str!` and asks for entries by
//! name, so cargo rebuilds when the data changes and the binary and the tests
//! read the same bytes.
//!
//! Format: `name = <64 hex digits>`, one per line, `#` comments and blank lines
//! ignored.

/// Reads one pin by name.
///
/// # Panics
///
/// A missing or malformed entry is an error in the data file, not a property of
/// the tree being read, so it stops the run. Returning something plausible
/// instead would silently reclassify a capability, which is the failure these
/// pins exist to prevent.
#[must_use]
pub fn pin<'a>(source: &'a str, file: &str, name: &str) -> &'a str {
    for line in source.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            panic!("{file}: malformed line {line:?}");
        };
        if key.trim() != name {
            continue;
        }
        let value = value.trim();
        assert!(
            is_sha256(value),
            "{file}: {name} is not a sha256: {value:?}"
        );
        return value;
    }
    panic!("{file}: no pin named {name}");
}

/// Every `name = value` pair in a pin file, in file order.
///
/// Exposed so a crate can check its own file as a whole -- that every value is
/// a digest and no name is spelled twice -- rather than only discovering a bad
/// line when some capability happens to ask for it.
///
/// # Panics
///
/// On a line that is neither blank, a comment, nor a `name = value` pair.
#[must_use]
pub fn entries<'a>(source: &'a str, file: &str) -> Vec<(&'a str, &'a str)> {
    let mut found = Vec::new();
    for line in source.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            panic!("{file}: malformed line {line:?}");
        };
        found.push((key.trim(), value.trim()));
    }
    found
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# a comment
first  = 0000000000000000000000000000000000000000000000000000000000000001

second = 0000000000000000000000000000000000000000000000000000000000000002
";

    #[test]
    fn reads_a_named_pin_past_comments_and_blank_lines() {
        assert!(pin(SAMPLE, "sample", "first").ends_with("01"));
        assert!(pin(SAMPLE, "sample", "second").ends_with("02"));
    }

    #[test]
    fn lists_every_entry_in_file_order() {
        let entries = entries(SAMPLE, "sample");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].0, "first");
        assert_eq!(entries[1].0, "second");
    }

    #[test]
    #[should_panic(expected = "no pin named third")]
    fn a_missing_pin_stops_the_run() {
        let _ = pin(SAMPLE, "sample", "third");
    }

    #[test]
    #[should_panic(expected = "is not a sha256")]
    fn a_value_that_is_not_a_digest_stops_the_run() {
        let _ = pin("bad = nothex\n", "sample", "bad");
    }
}
