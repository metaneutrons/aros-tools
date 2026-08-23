//! The default link set the compiler driver appends to every AROS link.
//!
//! Our generated rules invoke `ld.lld -r` directly rather than a compiler
//! driver (`cmake/AROS.cmake:244`), so nothing applies the target compiler's
//! spec file for us. That spec is where the library bases come from:
//!
//!   `config/elf-specs.in:19`
//!       *lib: %(autolib) %{!nostdc:%{!noposixc:-lposixc} -lstdcio -lstdc}
//!             %{!nosysbase:-lexec} %{nostdc:-lstdc.static}
//!   `config/elf-specs.in:16`
//!       *link: ... %include_noerr <.../Developer/lib/auto>
//!   `compiler/autoinit/auto`
//!       *autolib: -lmui -lamiga ... -loop -llibinit -lautoinit
//!
//! `lib<mod>.a` carries `<mod>_autoinit.c`, whose `AROS_LIBSET`
//! (`compiler/include/aros/symbolsets.h:118`) defines the module's library
//! base; `libexec.a` carries `struct ExecBase *SysBase`
//! (`rom/exec/exec_autoinit.c:22`). Modules link with `-nostartfiles`
//! (`configure.in:3468`), which suppresses `*startfile:` only, so a module
//! receives the same default set a program does.
//!
//! This module reads the spec rather than restating it, and refuses to guess:
//! an expression it cannot represent is an error, not a silent omission.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use aros_common::text::read_source;

/// One `-l<name>` from the spec, with the driver switches guarding it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefaultLinkItem {
    /// Archive base name, without `-l`.
    pub name: String,
    /// Switches that must be absent for this item to apply.
    pub require_absent: Vec<String>,
    /// Switches that must be present for this item to apply.
    pub require_present: Vec<String>,
}

/// The whole set, in spec order. Duplicates are kept: `compiler/autoinit/auto`
/// names `-lamiga` twice, and archive order decides symbol resolution.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefaultLinkSet {
    pub items: Vec<DefaultLinkItem>,
}

/// Reads the object-format spec that configure selects for this target.
///
/// `configure.in:3044` prefers `config/<format>-specs.in` and falls back to
/// `config/elf-specs.in`.
fn spec_path(source_dir: &Path, object_format: &str) -> std::path::PathBuf {
    let specific = source_dir
        .join("config")
        .join(format!("{object_format}-specs.in"));
    if specific.is_file() {
        return specific;
    }
    source_dir.join("config").join("elf-specs.in")
}

/// Returns the body of a `*<section>:` entry in a spec file, which is every
/// line up to the next blank line or the next section.
fn spec_section(content: &str, section: &str) -> Option<String> {
    let header = format!("*{section}:");
    let mut lines = content.lines().skip_while(|line| line.trim() != header);
    lines.next()?;
    let mut body = String::new();
    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('*') {
            break;
        }
        if !body.is_empty() {
            body.push(' ');
        }
        body.push_str(trimmed);
    }
    if body.is_empty() {
        None
    } else {
        Some(body)
    }
}

/// Parses one spec expression into guarded `-l` items.
///
/// Handles a bare `-l<name>`, `%{cond:...}`, `%{!cond:...}` and their nesting,
/// which is the whole of what `*lib:` uses. Anything else is refused by name so
/// it cannot vanish.
fn parse_spec_expression(
    expression: &str,
    inherited_absent: &[String],
    inherited_present: &[String],
    items: &mut Vec<DefaultLinkItem>,
) -> Result<(), String> {
    let bytes: Vec<char> = expression.chars().collect();
    let mut index = 0;
    while index < bytes.len() {
        let character = bytes[index];
        if character.is_whitespace() {
            index += 1;
            continue;
        }
        if character == '%' && index + 1 < bytes.len() && bytes[index + 1] == '{' {
            // %{[!]cond:body}, with balanced braces inside the body.
            let mut depth = 0;
            let mut end = index + 1;
            while end < bytes.len() {
                match bytes[end] {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                end += 1;
            }
            if depth != 0 {
                return Err(format!("unbalanced %{{ in spec expression `{expression}`"));
            }
            let inner: String = bytes[index + 2..end].iter().collect();
            let Some((condition, body)) = inner.split_once(':') else {
                return Err(format!(
                    "spec conditional without a body: `%{{{inner}}}` in `{expression}`"
                ));
            };
            let condition = condition.trim();
            let mut absent = inherited_absent.to_vec();
            let mut present = inherited_present.to_vec();
            match condition.strip_prefix('!') {
                Some(name) if !name.trim().is_empty() => absent.push(name.trim().to_owned()),
                Some(_) => {
                    return Err(format!("empty negated spec condition in `{expression}`"));
                }
                None if !condition.is_empty() => present.push(condition.to_owned()),
                None => return Err(format!("empty spec condition in `{expression}`")),
            }
            parse_spec_expression(body, &absent, &present, items)?;
            index = end + 1;
            continue;
        }
        // An ordinary whitespace-delimited token.
        let start = index;
        while index < bytes.len() && !bytes[index].is_whitespace() {
            if bytes[index] == '%' && index + 1 < bytes.len() && bytes[index + 1] == '{' {
                break;
            }
            index += 1;
        }
        let token: String = bytes[start..index].iter().collect();
        if token.is_empty() {
            index += 1;
            continue;
        }
        let Some(name) = token.strip_prefix("-l") else {
            return Err(format!(
                "spec token `{token}` in `{expression}` is not a -l<name> item"
            ));
        };
        if name.is_empty() {
            return Err(format!("empty -l item in `{expression}`"));
        }
        items.push(DefaultLinkItem {
            name: name.to_owned(),
            require_absent: inherited_absent.to_vec(),
            require_present: inherited_present.to_vec(),
        });
    }
    Ok(())
}

/// Reads and expands the default link set for one object format.
pub fn read_default_link_set(
    source_dir: &Path,
    object_format: &str,
) -> Result<DefaultLinkSet, String> {
    let spec = spec_path(source_dir, object_format);
    let content = read_source(&spec).map_err(|error| format!("{}: {error}", spec.display()))?;
    let lib = spec_section(&content, "lib")
        .ok_or_else(|| format!("{}: no *lib: section", spec.display()))?;

    // %(autolib) is the *autolib: section of the spec fragment that
    // compiler/autoinit installs next to the target libraries.
    let auto_path = source_dir.join("compiler/autoinit/auto");
    let expanded = if lib.contains("%(autolib)") {
        let auto =
            read_source(&auto_path).map_err(|error| format!("{}: {error}", auto_path.display()))?;
        let autolib = spec_section(&auto, "autolib")
            .ok_or_else(|| format!("{}: no *autolib: section", auto_path.display()))?;
        lib.replace("%(autolib)", &autolib)
    } else {
        lib
    };
    if expanded.contains("%(") {
        return Err(format!(
            "{}: *lib: still references an unexpanded spec section: `{expanded}`",
            spec.display()
        ));
    }

    let mut items = Vec::new();
    parse_spec_expression(&expanded, &[], &[], &mut items)
        .map_err(|error| format!("{}: {error}", spec.display()))?;
    if items.is_empty() {
        return Err(format!(
            "{}: *lib: expanded to no libraries",
            spec.display()
        ));
    }
    Ok(DefaultLinkSet { items })
}

/// True when `compiler/autoinit/auto` exists, which is what makes the set
/// meaningful for a source tree.
#[must_use]
pub fn default_link_set_available(source_dir: &Path) -> bool {
    fs::metadata(source_dir.join("compiler/autoinit/auto")).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_conditionals_carry_their_guards() {
        let mut items = Vec::new();
        parse_spec_expression(
            "-lamiga %{!nostdc:%{!noposixc:-lposixc} -lstdc} %{!nosysbase:-lexec} \
             %{nostdc:-lstdc.static}",
            &[],
            &[],
            &mut items,
        )
        .unwrap();
        let names: Vec<&str> = items.iter().map(|item| item.name.as_str()).collect();
        assert_eq!(
            names,
            ["amiga", "posixc", "stdc", "exec", "stdc.static"],
            "spec order must be preserved"
        );
        assert!(items[0].require_absent.is_empty());
        assert_eq!(items[1].require_absent, ["nostdc", "noposixc"]);
        assert_eq!(items[2].require_absent, ["nostdc"]);
        assert_eq!(items[3].require_absent, ["nosysbase"]);
        assert_eq!(items[4].require_present, ["nostdc"]);
    }

    #[test]
    fn an_unrepresentable_token_is_an_error_not_an_omission() {
        let mut items = Vec::new();
        let error = parse_spec_expression("-lamiga -Wl,--whatever", &[], &[], &mut items)
            .expect_err("an unknown token must be refused");
        assert!(error.contains("-Wl,--whatever"), "{error}");
    }

    #[test]
    fn a_section_ends_at_the_next_section_or_blank_line() {
        let spec = "*link:\n-Lsomewhere\n\n*lib:\n-lone -ltwo\n\n*libgcc:\n-lgcc\n";
        assert_eq!(spec_section(spec, "lib").as_deref(), Some("-lone -ltwo"));
        assert_eq!(spec_section(spec, "libgcc").as_deref(), Some("-lgcc"));
        assert_eq!(spec_section(spec, "missing"), None);
    }
}

#[cfg(test)]
mod tree_tests {
    use super::*;

    fn root() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../..")
            .canonicalize()
            .unwrap()
    }

    #[test]
    fn the_tree_spec_expands_to_the_documented_set() {
        let set = read_default_link_set(&root(), "elf").expect("elf spec");
        let names: Vec<&str> = set.items.iter().map(|item| item.name.as_str()).collect();
        // compiler/autoinit/auto first, then the C runtime, then exec.
        assert_eq!(names.first(), Some(&"mui"));
        assert_eq!(names.last(), Some(&"stdc.static"));
        assert!(names.contains(&"dos"));
        assert!(names.contains(&"utility"));
        assert_eq!(
            names.iter().filter(|name| **name == "amiga").count(),
            2,
            "the spec names -lamiga twice and order decides resolution"
        );
        let exec = set
            .items
            .iter()
            .find(|item| item.name == "exec")
            .expect("-lexec");
        assert_eq!(exec.require_absent, ["nosysbase"]);
    }
}
