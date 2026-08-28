//! Third-party source fetching from `%fetch`.
//!
//! Some modules build against sources that are not in the tree. ACPICA is the
//! one that matters for a bootable x86_64 target: `arch/all-native/acpica`
//! declares an archive, a list of mirrors and an in-tree patch, and the headers
//! that `libraries/acpica.h` pulls in come from the unpacked result.
//!
//! Only the declaration is transpiled. Downloading is left to the tree's own
//! `aros-fetch`, which handles the closed origin flavours the tree uses
//! (plain mirrors, GNU, SourceForge, GitHub, and a local `cache://`). Rebuilding
//! that in CMake would be a lot of surface for no gain; a Rust replacement can
//! come later without changing the declarations.
//!
//! The generated CMake target is never part of `all`. Fetching reaches out to
//! the network, so it stays an explicit step (`ninja fetch-ports`).

use crate::make_vars::VarScope;
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
    /// `checksums=`: exact `filename=sha256:<digest>` archive contracts.
    #[serde(default)]
    pub checksums: String,
    /// `location=`: where the downloaded archive is kept.
    pub location: String,
    /// `destination=`: where it is unpacked.
    pub destination: String,
    /// `base=`: working directory used while unpacking and applying patches.
    pub base: String,
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
        "SRCDIR" => Some("${CMAKE_SOURCE_DIR}"),
        "TOP" => Some("${AROS_BUILD_DIR}"),
        // The reference puts unpacked ports under $(TARGETDIR)/Ports and keeps
        // downloaded archives in a separate, configure-chosen directory.
        "PORTSDIR" => Some("${AROS_PORTS_DIR}"),
        "PORTSSOURCEDIR" => Some("${AROS_PORTS_SOURCE_DIR}"),
        "GENDIR" | "OBJDIR" => Some("${CMAKE_BINARY_DIR}"),
        "CPU" | "AROS_TARGET_CPU" => Some("${AROS_TARGET_CPU}"),
        "ARCH" => Some("${AROS_TARGET_PLATFORM}"),
        "AROS_TARGET_PLATFORM" => Some("${AROS_TARGET_LEGACY_PLATFORM}"),
        _ => None,
    }
}

fn reference_end(raw: &str, start: usize) -> Option<usize> {
    let bytes = raw.as_bytes();
    let mut depth = 1usize;
    let mut cursor = start + 2;
    while cursor < bytes.len() {
        if bytes[cursor] == b'$' && bytes.get(cursor + 1) == Some(&b'(') {
            depth += 1;
            cursor += 2;
            continue;
        }
        if bytes[cursor] == b')' {
            depth -= 1;
            if depth == 0 {
                return Some(cursor);
            }
        }
        cursor += 1;
    }
    None
}

fn split_function_args(raw: &str) -> Option<[&str; 3]> {
    let bytes = raw.as_bytes();
    let mut depth = 0usize;
    let mut commas = Vec::new();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if bytes[cursor] == b'$' && bytes.get(cursor + 1) == Some(&b'(') {
            depth += 1;
            cursor += 2;
            continue;
        }
        if bytes[cursor] == b')' && depth > 0 {
            depth -= 1;
        } else if bytes[cursor] == b',' && depth == 0 {
            commas.push(cursor);
        }
        cursor += 1;
    }
    (commas.len() == 2).then(|| {
        [
            &raw[..commas[0]],
            &raw[commas[0] + 1..commas[1]],
            &raw[commas[1] + 1..],
        ]
    })
}

fn split_function_args2(raw: &str) -> Option<[&str; 2]> {
    let bytes = raw.as_bytes();
    let mut depth = 0usize;
    let mut comma = None;
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if bytes[cursor] == b'$' && bytes.get(cursor + 1) == Some(&b'(') {
            depth += 1;
            cursor += 2;
            continue;
        }
        if bytes[cursor] == b')' && depth > 0 {
            depth -= 1;
        } else if bytes[cursor] == b',' && depth == 0 {
            if comma.is_some() {
                return None;
            }
            comma = Some(cursor);
        }
        cursor += 1;
    }
    comma.map(|comma| [&raw[..comma], &raw[comma + 1..]])
}

/// Expands `$(VAR)` against local assignments, then against the CMake mapping.
///
/// `$(CURDIR)` resolves to the declaring directory. An unresolved variable is
/// left verbatim so the caller can see and report it.
fn expand(raw: &str, lookup: &dyn Fn(&str) -> Option<String>, dir: &str, depth: usize) -> String {
    if depth == 0 || !raw.contains("$(") {
        return raw.to_owned();
    }
    let mut out = String::with_capacity(raw.len() + 32);
    let mut rest = raw;
    while let Some(start) = rest.find("$(") {
        out.push_str(&rest[..start]);
        let Some(end) = reference_end(rest, start) else {
            out.push_str(&rest[start..]);
            return out;
        };
        let body = &rest[start + 2..end];
        if body == "CURDIR" {
            out.push_str(dir);
        } else if let Some(args) = body.strip_prefix("subst ").and_then(split_function_args) {
            let from = expand(args[0], lookup, dir, depth - 1);
            let to = expand(args[1], lookup, dir, depth - 1);
            let text = expand(args[2], lookup, dir, depth - 1);
            if from.is_empty() {
                out.push_str(&rest[start..=end]);
            } else {
                out.push_str(&text.replace(&from, &to));
            }
        } else if let Some(args) = body.strip_prefix("word ").and_then(split_function_args2) {
            let index = expand(args[0], lookup, dir, depth - 1)
                .trim()
                .parse::<usize>()
                .ok();
            let words = expand(args[1], lookup, dir, depth - 1);
            if let Some(index) = index.filter(|index| *index > 0) {
                if let Some(word) = words.split_whitespace().nth(index - 1) {
                    out.push_str(word);
                }
            } else {
                out.push_str(&rest[start..=end]);
            }
        } else if let Some(m) = map_var(body) {
            out.push_str(m);
        } else if let Some(v) = lookup(body) {
            out.push_str(&expand(&v, lookup, dir, depth - 1));
        } else {
            out.push_str(&rest[start..=end]);
        }
        rest = &rest[end + 1..];
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
    collect_fetches_with_lookup(content, rel_dir, &|name| vars.get(name).cloned())
}

/// Collects `%fetch` declarations against the target-selected final Make scope.
///
/// Fetch recipes expand their variables when the recipe runs, after Make has
/// read the complete file. This differs from `%build_*`, whose simple
/// assignments freeze arguments at the declaration line. Using the final
/// target-aware scope preserves later conditional assignments such as the
/// GNU-only libheif compatibility patches without applying them to LLVM.
pub(crate) fn collect_fetches_with_scope(
    content: &str,
    rel_dir: &Path,
    scope: &VarScope,
) -> (Vec<FetchDecl>, Vec<String>) {
    collect_fetches_with_lookup(content, rel_dir, &|name| {
        if scope.conditionally_assigned_before(name, usize::MAX) {
            // Preserve the reference so the normal unresolved-field gate
            // reports it. Treating an undecidable conditional value as an
            // absent optional patch would silently emit an unpatched fetch.
            Some(format!("$({name})"))
        } else {
            scope.raw_at(name, usize::MAX)
        }
    })
}

fn collect_fetches_with_lookup(
    content: &str,
    rel_dir: &Path,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> (Vec<FetchDecl>, Vec<String>) {
    let dir = rel_dir.to_string_lossy().replace('\\', "/");
    let mut out = Vec::new();
    let mut skipped = Vec::new();

    for body in crate::includes::directive_bodies_pub(content, "%fetch") {
        let get = |key: &str| -> Option<String> {
            last_arg_value(&body, key)
                .map(|v| expand(&v, lookup, &dir, 8))
                // Values reach here with whatever quoting the mmakefile used;
                // the generator adds its own, so strip any leftovers.
                .map(|v| v.trim_matches('"').trim().to_owned())
        };

        let Some(name) = get("mmake") else { continue };
        let Some(archive) = get("archive") else {
            continue;
        };

        let decl = FetchDecl {
            name,
            archive,
            suffixes: get("suffixes").unwrap_or_else(|| "tar.bz2 tar.gz".to_owned()),
            origins: get("archive_origins").unwrap_or_else(|| ".".to_owned()),
            checksums: get("checksums").unwrap_or_default(),
            location: get("location").unwrap_or_default(),
            destination: get("destination").unwrap_or_else(|| ".".to_owned()),
            base: get("base").unwrap_or_default(),
            patch_origins: get("patches_origins")
                .unwrap_or_else(|| format!("${{CMAKE_SOURCE_DIR}}/{dir}")),
            patches: get("patches_specs")
                .filter(|value| {
                    let trimmed = value.trim();
                    let Some(name) = trimmed
                        .strip_prefix("$(")
                        .and_then(|rest| rest.strip_suffix(')'))
                    else {
                        return true;
                    };
                    // An optional, wholly undefined patch variable is Make's
                    // ordinary way to disable patching. It expands to empty,
                    // unlike an unresolved archive/destination field.
                    lookup(name).is_some() || map_var(name).is_some()
                })
                .unwrap_or_else(|| "::".to_owned()),
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
            &decl.checksums,
            &decl.location,
            &decl.destination,
            &decl.base,
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

/// Reads the final `key=value` argument from a directive.
///
/// GNU Make macros receive the last assignment when an invocation repeats an
/// argument.  A few legacy declarations intentionally rely on that behaviour
/// (zlib supplies a broad destination first, then the concrete unpack path),
/// so using the first whitespace token would silently change the recipe.
fn last_arg_value(body: &str, key: &str) -> Option<String> {
    let needle = format!("{key}=");
    let mut from = 0usize;
    let mut value = None;

    while let Some(relative_hit) = body[from..].find(&needle) {
        let hit = from + relative_hit;
        let boundary = hit == 0
            || body[..hit]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace);
        let start = hit + needle.len();
        if boundary && start < body.len() {
            let rest = &body[start..];
            if let Some(quoted) = rest.strip_prefix('"') {
                if let Some(end) = quoted.find('"') {
                    value = Some(quoted[..end].to_owned());
                    from = start + end + 2;
                    continue;
                }
            } else {
                let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
                if end > 0 {
                    value = Some(rest[..end].to_owned());
                }
            }
        }
        from = hit + 1;
    }

    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::make_vars::{collect_vars, collect_vars_with_context};
    use crate::parser::{join_continuations, TargetContext};
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
        let (decls, skipped) = collect_fetches(ACPICA, &PathBuf::from("arch/all-native/acpica"));
        assert!(skipped.is_empty(), "skipped: {skipped:?}");
        assert_eq!(decls.len(), 1);
        let d = &decls[0];
        assert_eq!(d.name, "acpica-fetch");
        assert_eq!(d.archive, "acpica-unix-20260408");
        assert_eq!(d.destination, "${AROS_PORTS_DIR}/acpica");
        assert!(d.base.is_empty());
        assert_eq!(d.location, "${AROS_PORTS_SOURCE_DIR}");
        assert_eq!(d.dir, "arch/all-native/acpica");
    }

    #[test]
    fn nested_variables_are_expanded() {
        let (decls, _) = collect_fetches(ACPICA, &PathBuf::from("arch/all-native/acpica"));
        let d = &decls[0];
        // $(ACPICAREPOSITORIES) -> the three origins, with $(INTELID) resolved.
        assert!(d.origins.contains("cache://"));
        assert!(d
            .origins
            .contains("https://downloadmirror.intel.com/917611"));
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
    fn explicit_checksums_are_resolved_without_inference() {
        let digest = "0123456789abcdef".repeat(4);
        let src = format!(
            "VERSION := 1.0\n%fetch mmake=pkg-fetch archive=pkg-$(VERSION) \\\n+             destination=$(PORTSDIR)/pkg suffixes=\"tar.xz tar.gz\" \\\n+             checksums=\"pkg-$(VERSION).tar.xz=sha256:{digest} pkg-$(VERSION).tar.gz=sha256:{digest}\"\n"
        );
        let (decls, skipped) = collect_fetches(&src, &PathBuf::from("external/pkg"));
        assert!(skipped.is_empty(), "skipped: {skipped:?}");
        assert_eq!(
            decls[0].checksums,
            format!("pkg-1.0.tar.xz=sha256:{digest} pkg-1.0.tar.gz=sha256:{digest}")
        );
    }

    #[test]
    fn subst_and_an_undefined_optional_patch_match_make() {
        // workbench/libs/expat uses subst in its release URL and deliberately
        // leaves the optional patch variable undefined.
        let src = "\
EXPATVERSION := 2.8.2
%fetch mmake=expat-fetch archive=expat-$(EXPATVERSION) \\
    destination=$(PORTSDIR)/expat location=$(PORTSSOURCEDIR) \\
    archive_origins=\"cache:// https://example.org/R_$(subst .,_,$(EXPATVERSION))\" \\
    suffixes=\"tar.bz2\" patches_specs=$(EXPATPATCHSPEC)
";
        let (decls, skipped) = collect_fetches(src, &PathBuf::from("workbench/libs/expat"));
        assert!(skipped.is_empty(), "skipped: {skipped:?}");
        assert_eq!(decls.len(), 1);
        assert!(decls[0].origins.contains("R_2_8_2"));
        assert_eq!(decls[0].patches, "::");
    }

    #[test]
    fn target_conditionals_select_late_fetch_patch_values() {
        // The HEIC port declares its fetch recipes before a GNU-only
        // compatibility branch. Make expands these recipe variables after it
        // has parsed that branch; LLVM must not inherit the GNU patches.
        let src = r"
HEIFPATCHSPEC := base.diff:libheif:-p1
%fetch mmake=heif-fetch archive=heif destination=$(PORTSDIR)/heif \
    patches_specs=$(HEIFPATCHSPEC)
%fetch mmake=de265-fetch archive=de265 destination=$(PORTSDIR)/de265 \
    patches_specs=$(DE265PATCHSPEC)
ifeq ($(AROS_TOOLCHAIN),gnu)
DE265PATCHSPEC := compat.diff:libde265:-p1
HEIFPATCHSPEC += compat-cxx.diff:libheif:-p1
endif
";
        let collect = |toolchain: &str| {
            let target = TargetContext {
                cpu: None,
                platform: None,
                family: None,
                variant: None,
                toolchain: Some(toolchain.to_owned()),
                cpu32: None,
                use_mmu: None,
                float_abi: None,
            };
            let scope = collect_vars_with_context(&join_continuations(src), &target);
            collect_fetches_with_scope(src, Path::new("workbench/classes/datatypes/heic"), &scope)
        };

        let (llvm, skipped) = collect("llvm");
        assert!(skipped.is_empty(), "skipped: {skipped:?}");
        assert_eq!(llvm[0].patches, "base.diff:libheif:-p1");
        assert_eq!(llvm[1].patches, "::");

        let (gnu, skipped) = collect("gnu");
        assert!(skipped.is_empty(), "skipped: {skipped:?}");
        assert_eq!(
            gnu[0].patches,
            "base.diff:libheif:-p1 compat-cxx.diff:libheif:-p1"
        );
        assert_eq!(gnu[1].patches, "compat.diff:libde265:-p1");

        let unknown = collect_vars(&join_continuations(src));
        let (decls, skipped) = collect_fetches_with_scope(
            src,
            Path::new("workbench/classes/datatypes/heic"),
            &unknown,
        );
        assert!(decls.is_empty());
        assert_eq!(skipped.len(), 2);
    }

    #[test]
    fn quotes_are_not_carried_into_the_value() {
        let src = "%fetch mmake=z archive=pkg-1 destination=$(PORTSDIR)/z suffixes=\"tar.gz\"\n";
        let (decls, _) = collect_fetches(src, &PathBuf::from("d"));
        assert_eq!(decls[0].suffixes, "tar.gz", "no stray quotes");
    }

    #[test]
    fn the_last_duplicate_argument_wins_and_base_is_preserved() {
        let src = "%fetch mmake=z archive=zlib destination=$(PORTSDIR)/zlib \\\n            base=$(PORTSDIR)/zlib destination=$(PORTSDIR)/zlib/zlib\n";
        let (decls, skipped) = collect_fetches(src, &PathBuf::from("workbench/libs/z"));
        assert!(skipped.is_empty(), "skipped: {skipped:?}");
        assert_eq!(decls[0].base, "${AROS_PORTS_DIR}/zlib");
        assert_eq!(decls[0].destination, "${AROS_PORTS_DIR}/zlib/zlib");
    }

    #[test]
    fn distinguishes_machine_arch_from_the_compound_legacy_platform() {
        let src = "%fetch mmake=z archive=pkg-$(ARCH)-$(AROS_TARGET_PLATFORM) destination=$(PORTSDIR)/z\n";
        let (decls, skipped) = collect_fetches(src, &PathBuf::from("d"));
        assert!(skipped.is_empty(), "skipped: {skipped:?}");
        assert_eq!(
            decls[0].archive,
            "pkg-${AROS_TARGET_PLATFORM}-${AROS_TARGET_LEGACY_PLATFORM}"
        );
    }
}
