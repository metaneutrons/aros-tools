//! Source-owned literal MetaMake directory exclusions.

use aros_common::{native_build_contract::LoadedNativeBuildContract, ArosError, Result};
use std::{collections::BTreeSet, path::Path};

pub fn ignored_directories(
    root: &Path,
    native: Option<&LoadedNativeBuildContract>,
) -> Result<BTreeSet<String>> {
    let path = root.join("mmake.config.in");
    let Some((_, bytes)) = aros_common::measure_regular_file_bounded(&path, 64 * 1024)? else {
        return Ok(BTreeSet::new());
    };
    let digest = aros_common::sha256_bytes(&bytes);
    if native.is_some_and(|bound| {
        !bound
            .contract
            .inputs
            .iter()
            .any(|input| input.path == "mmake.config.in" && input.sha256 == digest)
    }) {
        return Err(ArosError::Configuration {
            file: path.display().to_string(),
            message: "native source discovery requires the exact mmake.config.in snapshot in contract inputs".into(),
        });
    }
    let text = std::str::from_utf8(&bytes).map_err(|error| ArosError::Configuration {
        file: path.display().to_string(),
        message: format!("MetaMake discovery input is not UTF-8: {error}"),
    })?;
    Ok(literal_directories(text))
}

fn literal_directories(text: &str) -> BTreeSet<String> {
    text.lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            if words.next()? != "ignoredir" {
                return None;
            }
            let directory = words.next()?;
            // Unconfigured substitutions are not directory names. Leaving these
            // entries in discovery is conservative: no source subtree is hidden.
            if words.next().is_some()
                || directory == "."
                || directory == ".."
                || !directory
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
            {
                return None;
            }
            Some(directory.to_owned())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_literal_source_directives_hide_directories() {
        let actual = literal_directories(
            "[AROS]\nignoredir .unmaintained\nignoredir CVS\n#ignoredir live\nignoredir distfiles@mmake_ignore_dirs@\nignoredir ../outside\nignoredir $(DYNAMIC)\nignoredir .\nignoredir ..\nignoredir first second\n",
        );
        assert_eq!(
            actual,
            BTreeSet::from([".unmaintained".into(), "CVS".into()])
        );
    }
}
