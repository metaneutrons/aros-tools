//! The library-version markers, and why they have to become data.
//!
//! `AROS_LIBREQ(bname, ver)` emits `__aros_libreq_<bname>.<ver>` as a weak
//! absolute symbol whose value is the version
//! (`compiler/include/aros/symbolsets.h:158`). The code does not read that
//! symbol. genmodule's generated `InitLib` reads the *unversioned* name as
//! memory:
//!
//! ```text
//! movzwl 0x26(%rdx), %eax          ; SysBase->lib_Version
//! cmpl   %eax, (%rip)              ; R_X86_64_PC32 __aros_libreq_SysBase - 4
//! ```
//!
//! `collect_libs` (`tools/collect-aros/backend-generic.c:64`) and `emit_libs`
//! (`gensets.c:167`) are what connect the two: the second linker pass emits
//! `PROVIDE(__aros_libreq_<base> = .); LONG(<version>)`, so the unversioned name
//! becomes the address of a word holding the required version.
//!
//! Without it nothing defines the unversioned symbol, the operand address is 0,
//! and the check faults with CR2=0 -- which is exactly where this build's boot
//! stopped once the symbol sets worked. 1210 of 1238 built artefacts reference a
//! marker, so this is the whole tree.
//!
//! One deliberate divergence. The reference prepends a node per `nm` line and
//! `PROVIDE` binds the name to the first one emitted, so where several
//! requirements for one base meet in a single link -- the kickstart has SysBase
//! at 0, 33, 36 and 50 -- which version wins is symbol-table order. The maximum
//! is what the check actually needs, because it has to satisfy every
//! requirement, so that is what this takes. A base with more than one version in
//! one link is reported, so the choice is visible rather than assumed.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use aros_common::elf::{Binding, Home, Symbol};

const PREFIX: &str = "__aros_libreq_";

/// One base's version requirement, resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requirement {
    /// The unversioned symbol name, e.g. `__aros_libreq_SysBase`.
    pub symbol: String,
    /// The version to publish.
    pub version: i64,
}

/// Whether a symbol is one the reference's `nm` filter would accept.
///
/// It takes `A`, `W` and `V` -- a global or weak *absolute* -- for the versioned
/// form, and `w`, a weak *undefined*, for a bare reference with no stated
/// version. A local absolute is `a` and is not accepted, which matters: the
/// kickstart's members have had their markers localised by the time the
/// kickstart itself is linked (`scripts/kickstart/localise-symbols.py` covers
/// `__aros_lib*`), and each member already carries its own definition from its
/// own pass. Re-publishing them at the kickstart level would bind one member's
/// requirement to another's.
fn eligible(symbol: &Symbol) -> bool {
    if !symbol.name.starts_with(PREFIX) {
        return false;
    }
    match symbol.home {
        Home::Absolute => symbol.binding != Binding::Local,
        Home::Undefined => symbol.binding == Binding::Weak,
        Home::Section(_) => false,
    }
}

/// Collects the version requirements of one object.
///
/// Returns the requirements in symbol-name order and, for reporting, the
/// markers that could not be read and the bases that had more than one version.
pub fn discover(symbols: &[Symbol]) -> (Vec<Requirement>, Vec<String>) {
    let mut seen: BTreeMap<String, Vec<i64>> = BTreeMap::new();
    let mut skipped = Vec::new();

    for symbol in symbols.iter().filter(|symbol| eligible(symbol)) {
        let rest = &symbol.name[PREFIX.len()..];
        // The first dot after the prefix separates the base from the version;
        // a library base name never contains one.
        let (base, version) = if let Some((base, suffix)) = rest.split_once('.') {
            if let Ok(version) = suffix.parse::<i64>() {
                (base, version)
            } else {
                skipped.push(format!("{}: `{suffix}` is not a version", symbol.name));
                continue;
            }
        } else {
            (rest, 0)
        };
        if base.is_empty() {
            skipped.push(format!("{}: no library base in the name", symbol.name));
            continue;
        }
        seen.entry(format!("{PREFIX}{base}"))
            .or_default()
            .push(version);
    }

    let mut out = Vec::with_capacity(seen.len());
    for (symbol, mut versions) in seen {
        versions.sort_unstable();
        versions.dedup();
        let version = *versions.last().unwrap_or(&0);
        // Reported only when two or more *stated* requirements meet, which is
        // where the choice is a choice. `[0, 36]` is the ordinary shape --
        // genmodule emits AROS_LIBREQ(base, 0) for a caller that names no
        // minimum -- and reporting it would bury the real cases: across the
        // tree it accounts for 2529 of the 2529 lines this used to print.
        if versions.iter().filter(|version| **version != 0).count() > 1 {
            skipped.push(format!(
                "{symbol}: {versions:?} are all required here, publishing {version}"
            ));
        }
        out.push(Requirement { symbol, version });
    }
    (out, skipped)
}

/// The script lines that publish the requirements.
///
/// `LONG` regardless of the ELF class, as `emit_libs` writes it: the generated
/// check reads the word with a 32-bit `cmpl`, so the width is the check's, not
/// the pointer's.
pub fn script(requirements: &[Requirement]) -> String {
    let mut out = String::new();
    for requirement in requirements {
        let _ = writeln!(out, "    . = ALIGN(4);");
        let _ = writeln!(out, "    PROVIDE({} = .);", requirement.symbol);
        let _ = writeln!(out, "    LONG({})", requirement.version);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn absolute(name: &str) -> Symbol {
        Symbol {
            name: name.to_owned(),
            value: 0,
            size: 0,
            home: Home::Absolute,
            binding: Binding::Weak,
        }
    }

    #[test]
    fn the_highest_requirement_for_a_base_is_the_one_published() {
        let (found, reported) = discover(&[
            absolute("__aros_libreq_SysBase.33"),
            absolute("__aros_libreq_SysBase.50"),
            absolute("__aros_libreq_SysBase.0"),
        ]);
        // Two stated requirements, so the choice is worth a line.
        assert_eq!(
            found,
            [Requirement {
                symbol: "__aros_libreq_SysBase".to_owned(),
                version: 50
            }]
        );
        assert_eq!(reported.len(), 1, "{reported:?}");
        assert!(reported[0].contains("publishing 50"), "{reported:?}");
    }

    #[test]
    fn the_ordinary_zero_and_one_version_pair_is_not_reported() {
        let (found, reported) = discover(&[
            absolute("__aros_libreq_DOSBase.0"),
            absolute("__aros_libreq_DOSBase.36"),
        ]);
        assert_eq!(found[0].version, 36);
        assert!(reported.is_empty(), "{reported:?}");
    }

    #[test]
    fn a_bare_weak_reference_asks_for_version_zero() {
        let symbol = Symbol {
            name: "__aros_libreq_GfxBase".to_owned(),
            value: 0,
            size: 0,
            home: Home::Undefined,
            binding: Binding::Weak,
        };
        let (found, reported) = discover(&[symbol]);
        assert!(reported.is_empty(), "{reported:?}");
        assert_eq!(found[0].version, 0);
        assert_eq!(found[0].symbol, "__aros_libreq_GfxBase");
    }

    #[test]
    fn a_localised_marker_is_left_alone() {
        let mut symbol = absolute("__aros_libreq_SysBase.33");
        symbol.binding = Binding::Local;
        let (found, reported) = discover(&[symbol]);
        assert!(found.is_empty(), "{found:?}");
        assert!(reported.is_empty(), "{reported:?}");
    }

    #[test]
    fn an_unrelated_symbol_is_ignored() {
        let (found, reported) = discover(&[absolute("__aros_set_INITLIB_cpu_Init")]);
        assert!(found.is_empty());
        assert!(reported.is_empty());
    }

    #[test]
    fn a_suffix_that_is_not_a_version_is_reported() {
        let (found, reported) = discover(&[absolute("__aros_libreq_SysBase.beta")]);
        assert!(found.is_empty(), "{found:?}");
        assert_eq!(reported.len(), 1, "{reported:?}");
        assert!(reported[0].contains("not a version"), "{reported:?}");
    }

    #[test]
    fn the_script_provides_the_name_and_the_word() {
        let text = script(&[Requirement {
            symbol: "__aros_libreq_SysBase".to_owned(),
            version: 50,
        }]);
        assert!(
            text.contains("PROVIDE(__aros_libreq_SysBase = .);"),
            "{text}"
        );
        assert!(text.contains("LONG(50)"), "{text}");
    }
}
