//! Reading source files whose encoding predates UTF-8.

use std::fs;
use std::io;
use std::path::Path;

/// Reads a build-system source file as text, whatever its encoding.
///
/// 1488 files in the AROS tree are ISO-8859, not UTF-8; the copyright line
/// `Copyright © 1995-2026` is the usual reason. `fs::read_to_string` returns
/// `InvalidData` for those, and a caller that treats the error as "no such
/// declarations here" loses them without a word. Two mmakefiles were lost that
/// way, one of them a `%build_prog` with 14 sources.
///
/// Bytes that are not valid UTF-8 are decoded as ISO-8859-1, which maps each
/// byte to the code point of the same value. That is what the tree is: the
/// non-ASCII bytes only ever appear in comments, so the decoding cannot change
/// how a declaration reads, and it is exact rather than lossy for the encoding
/// actually in use.
///
/// # Errors
///
/// Only for an unreadable file. Encoding is never an error.
pub fn read_source(path: &Path) -> io::Result<String> {
    let bytes = fs::read(path)?;
    Ok(match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(e) => e.into_bytes().into_iter().map(char::from).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::read_source;
    use std::io::Write;

    fn temp_with(bytes: &[u8]) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(bytes).unwrap();
        f.flush().unwrap();
        f
    }

    #[test]
    fn reads_utf8_unchanged() {
        let f = temp_with("FILES := a b c\n# © 2026\n".as_bytes());
        assert_eq!(read_source(f.path()).unwrap(), "FILES := a b c\n# © 2026\n");
    }

    #[test]
    fn decodes_iso_8859_copyright() {
        // 0xa9 is © in ISO-8859-1 and not valid UTF-8 on its own. This is the
        // exact byte that made two mmakefiles unreadable.
        let f = temp_with(b"# Copyright \xa9 2012, The AROS Development Team\nFILES := x\n");
        let text = read_source(f.path()).unwrap();
        assert!(text.starts_with("# Copyright © 2012"), "got {text:?}");
        assert!(text.contains("FILES := x"));
    }

    #[test]
    fn keeps_declarations_readable_in_iso_8859() {
        let f = temp_with(b"# \xa9\n%build_prog mmake=t progname=T files=$(F)\n");
        assert!(read_source(f.path())
            .unwrap()
            .contains("%build_prog mmake=t progname=T files=$(F)"));
    }

    #[test]
    fn missing_file_is_an_error() {
        assert!(read_source(std::path::Path::new("/nonexistent/mmakefile.src")).is_err());
    }
}
