//! Third-party source fetching from `%fetch`.
//!
//! Some modules build against sources that are not in the tree. ACPICA is the
//! one that matters for a bootable x86_64 target: `arch/all-native/acpica`
//! declares an archive, a list of mirrors and an in-tree patch, and the headers
//! that `libraries/acpica.h` pulls in come from the unpacked result.
//!
//! Only the declaration is transpiled. Downloading is left to the tree's own
//! `scripts/fetch.sh`, which already handles the origin flavours the tree uses
//! (plain mirrors, GNU, SourceForge, GitHub, and a local `cache://`). Rebuilding
//! that in CMake would be a lot of surface for no gain; a Rust replacement can
//! come later without changing the declarations.
//!
//! The generated CMake target is never part of `all`. Fetching reaches out to
//! the network, so it stays an explicit step (`ninja fetch-ports`).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// One `%fetch` declaration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchDecl {
    /// `mmake=`: the target name.
    pub name: String,
    /// `archive=`: base name of the archive, without suffix.
    pub archive: String,
    /// `suffixes=`: candidate suffixes, e.g. `tar.gz`.
    pub suffixes: String,
    /// `archive_origins=`: mirrors to try, in order.
    pub origins: String,
    /// `location=`: where the downloaded archive is kept.
    pub location: String,
    /// `destination=`: where it is unpacked.
    pub destination: String,
    /// `patches_origins=`: directory holding the patches.
    pub patch_origins: String,
    /// `patches_specs=`: `<patch>[:<subdir>[:<options>]]` entries.
    pub patches: String,
    /// Directory of the declaring mmakefile, relative to the source root.
    pub dir: String,
}

/// Collects `VAR := value` assignments with their raw right-hand side.
fn collect_raw_vars(content: &str) -> HashMap<String, String> {
    let mut vars: HashMap<String, String> = HashMap::new();
    let mut pending: Option<String> = None;

    for line in content.lines() {
        let trimmed = line.trim();
        let continues = trimmed.ends_with('\\');
        let payload = trimmed.trim_end_matches('\\').trim();

        if let Some(name) = pending.take() {
            let entry = vars.entry(name.clone()).or_default();
            if !entry.is_empty() {
                entry.push(' ');
            }
            entry.push_str(payload);
            if continues {
                pending = Some(name);
            }
            continue;
        }
        if trimmed.starts_with('#') || trimmed.starts_with('%') {
            continue;
        }
        let Some((lhs, rhs, append)) = split_assignment(payload) else {
            continue;
        };
        let name = lhs.trim().to_owned();
        if name.is_empty() || name.contains(char::is_whitespace) {
            continue;
        }
        let entry = vars.entry(name.clone()).or_default();
        if append {
            if !entry.is_empty() {
                entry.push(' ');
            }
        } else {
            entry.clear();
        }
        entry.push_str(rhs.trim());
        if continues {
            pending = Some(name);
        }
    }
    vars
}

fn split_assignment(line: &str) -> Option<(&str, &str, bool)> {
    if let Some((l, r)) = line.split_once(":=") {
        return Some((l, r, false));
    }
    if let Some((l, r)) = line.split_once("+=") {
        return Some((l, r, true));
    }
    let (l, r) = line.split_once('=')?;
    if l.contains(':') || l.is_empty() {
        return None;
    }
    Some((l, r, false))
}

/// Make variables that map onto a CMake location.
fn map_var(name: &str) -> Option<&'static str> {
    match name {
        "SRCDIR" | "TOP" => Some("${CMAKE_SOURCE_DIR}"),
        // The reference puts unpacked ports under $(TARGETDIR)/Ports and keeps
        // downloaded archives in a separate, configure-chosen directory.
        "PORTSDIR" => Some("${AROS_PORTS_DIR}"),
        "PORTSSOURCEDIR" => Some("${AROS_PORTS_SOURCE_DIR}"),
        "GENDIR" | "OBJDIR" => Some("${CMAKE_BINARY_DIR}"),
        "CPU" | "AROS_TARGET_CPU" => Some("${AROS_TARGET_CPU}"),
        "ARCH" | "AROS_TARGET_PLATFORM" => Some("${AROS_TARGET_PLATFORM}"),
        _ => None,
    }
}

/// Expands `$(VAR)` against local assignments, then against the CMake mapping.
///
/// `$(CURDIR)` resolves to the declaring directory. An unresolved variable is
/// left verbatim so the caller can see and report it.
fn expand(raw: &str, vars: &HashMap<String, String>, dir: &str, depth: usize) -> String {
    if depth == 0 || !raw.contains("$(") {
        return raw.to_owned();
    }
    let mut out = String::with_capacity(raw.len() + 32);
    let mut rest = raw;
    while let Some(start) = rest.find("$(") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find(')') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let name = &after[..end];
        if name == "CURDIR" {
            out.push_str(dir);
        } else if let Some(m) = map_var(name) {
            out.push_str(m);
        } else if let Some(v) = vars.get(name) {
            out.push_str(&expand(v, vars, dir, depth - 1));
        } else {
            out.push_str(&rest[start..=start + 2 + end]);
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Parses the `%fetch` declarations of one `mmakefile.src`.
///
/// Returns the resolved declarations plus, for reporting, the ones that still
/// reference an unmapped Make variable.
#[must_use]
pub fn collect_fetches(content: &str, rel_dir: &Path) -> (Vec<FetchDecl>, Vec<String>) {
    let vars = collect_raw_vars(content);
    let dir = rel_dir.to_string_lossy().replace('\\', "/");
    let mut out = Vec::new();
    let mut skipped = Vec::new();

    for body in crate::includes::directive_bodies_pub(content, "%fetch") {
        let get = |key: &str| -> Option<String> {
            crate::includes::arg_value_quoted(&body, key)
                .or_else(|| crate::includes::arg_value(&body, key))
                .map(|v| expand(&v, &vars, &dir, 8))
                // Values reach here with whatever quoting the mmakefile used;
                // the generator adds its own, so strip any leftovers.
                .map(|v| v.trim_matches('"').trim().to_owned())
        };

        let Some(name) = get("mmake") else { continue };
        let Some(archive) = get("archive") else { continue };

        let decl = FetchDecl {
            name,
            archive,
            suffixes: get("suffixes").unwrap_or_else(|| "tar.bz2 tar.gz".to_owned()),
            origins: get("archive_origins").unwrap_or_else(|| ".".to_owned()),
            location: get("location").unwrap_or_default(),
            destination: get("destination").unwrap_or_else(|| ".".to_owned()),
            patch_origins: get("patches_origins")
                .unwrap_or_else(|| format!("${{CMAKE_SOURCE_DIR}}/{dir}")),
            patches: get("patches_specs").unwrap_or_else(|| "::".to_owned()),
            dir: dir.clone(),
        };

        // Every field has to be fully resolved. A leftover Make construct is not
        // just unusable: it would reach the generated build file verbatim and
        // break its escaping, so this covers all fields, not only the paths.
        // `${...}` is fine, that is a CMake reference the generator expands.
        let unresolved = [
            &decl.archive,
            &decl.suffixes,
            &decl.origins,
            &decl.location,
            &decl.destination,
            &decl.patch_origins,
            &decl.patches,
        ]
        .iter()
        .any(|f| f.contains("$("));
        if unresolved {
            skipped.push(format!("{dir}: {}", decl.name));
            continue;
        }

        out.push(decl);
    }

    (out, skipped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const ACPICA: &str = r#"
ACPICAPACKAGE      := acpica
ACPICAVERSION      := 20260408
INTELID            := 917611

ACPICAREPOSITORIES := \
    cache:// \
    https://downloadmirror.intel.com/$(INTELID) \
    https://axrt.org/download/thirdparty
ACPICAARCHBASE     := $(ACPICAPACKAGE)-unix-$(ACPICAVERSION)
ACPICASRCDIR       := $(PORTSDIR)/acpica/$(ACPICAARCHBASE)

ACPICAPSPECS := $(ACPICAARCHBASE)-aros.diff:$(ACPICAARCHBASE):-f,-p1

%fetch mmake=acpica-fetch archive=$(ACPICAARCHBASE) \
    destination=$(PORTSDIR)/acpica \
    location=$(PORTSSOURCEDIR) \
    archive_origins=$(ACPICAREPOSITORIES) \
    suffixes="tar.gz" patches_specs=$(ACPICAPSPECS)
"#;

    #[test]
    fn resolves_the_acpica_declaration() {
        let (decls, skipped) =
            collect_fetches(ACPICA, &PathBuf::from("arch/all-native/acpica"));
        assert!(skipped.is_empty(), "skipped: {skipped:?}");
        assert_eq!(decls.len(), 1);
        let d = &decls[0];
        assert_eq!(d.name, "acpica-fetch");
        assert_eq!(d.archive, "acpica-unix-20260408");
        assert_eq!(d.destination, "${AROS_PORTS_DIR}/acpica");
        assert_eq!(d.location, "${AROS_PORTS_SOURCE_DIR}");
        assert_eq!(d.dir, "arch/all-native/acpica");
    }

    #[test]
    fn nested_variables_are_expanded() {
        let (decls, _) = collect_fetches(ACPICA, &PathBuf::from("arch/all-native/acpica"));
        let d = &decls[0];
        // $(ACPICAREPOSITORIES) -> the three origins, with $(INTELID) resolved.
        assert!(d.origins.contains("cache://"));
        assert!(d.origins.contains("https://downloadmirror.intel.com/917611"));
        assert!(d.origins.contains("https://axrt.org/download/thirdparty"));
        // $(ACPICAPSPECS) -> patch:subdir:options, all substituted.
        assert_eq!(
            d.patches,
            "acpica-unix-20260408-aros.diff:acpica-unix-20260408:-f,-p1"
        );
    }

    #[test]
    fn patch_origin_defaults_to_the_declaring_directory() {
        let (decls, _) = collect_fetches(ACPICA, &PathBuf::from("arch/all-native/acpica"));
        assert_eq!(
            decls[0].patch_origins,
            "${CMAKE_SOURCE_DIR}/arch/all-native/acpica"
        );
    }

    #[test]
    fn unresolved_variables_are_reported_not_emitted() {
        let src = "%fetch mmake=x archive=$(UNKNOWN_ARCHIVE) destination=$(PORTSDIR)/x\n";
        let (decls, skipped) = collect_fetches(src, &PathBuf::from("d"));
        assert!(decls.is_empty());
        assert_eq!(skipped.len(), 1);
        assert!(skipped[0].contains('x'));
    }

    #[test]
    fn suffix_default_matches_the_macro() {
        let src = "%fetch mmake=y archive=pkg-1.0 destination=$(PORTSDIR)/y\n";
        let (decls, _) = collect_fetches(src, &PathBuf::from("d"));
        assert_eq!(decls[0].suffixes, "tar.bz2 tar.gz");
    }

    #[test]
    fn a_make_function_anywhere_rejects_the_declaration() {
        // workbench/libs/expat uses $(subst .,_,$(EXPATVERSION)) in its origin
        // list. Emitting that verbatim breaks the generated build file's
        // escaping, so the whole declaration is skipped.
        let src = "\
EXPATVERSION := 2.8.2
%fetch mmake=expat-fetch archive=expat-$(EXPATVERSION) \\
    destination=$(PORTSDIR)/expat location=$(PORTSSOURCEDIR) \\
    archive_origins=\"cache:// https://example.org/R_$(subst .,_,$(EXPATVERSION))\" \\
    suffixes=\"tar.bz2\"
";
        let (decls, skipped) = collect_fetches(src, &PathBuf::from("workbench/libs/expat"));
        assert!(decls.is_empty(), "decls: {decls:?}");
        assert_eq!(skipped.len(), 1);
    }

    #[test]
    fn quotes_are_not_carried_into_the_value() {
        let src = "%fetch mmake=z archive=pkg-1 destination=$(PORTSDIR)/z suffixes=\"tar.gz\"\n";
        let (decls, _) = collect_fetches(src, &PathBuf::from("d"));
        assert_eq!(decls[0].suffixes, "tar.gz", "no stray quotes");
    }
}
