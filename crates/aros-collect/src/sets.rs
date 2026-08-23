//! Symbol sets: what they are, and the layout the linker has to give them.
//!
//! `ADD2SET(symbol, INITLIB, 10)` puts a pointer to `symbol` into a section
//! called `.aros.set.INITLIB.10`
//! (`compiler/include/aros/symbolsets.h:41`). The set the code reads is a
//! different thing: `DEFINESET` declares it as a *weak* two-pointer array
//! `__INITLIB_LIST__[] = {0, 0}` (`:39`), and nothing in the compiler connects
//! the two. The linker does, and for an AROS target the linker is
//! `collect-aros` (`scripts/aros-ld.in:5`): after a first `ld -r` pass it reads
//! the section names out of the result and generates a script laying each set
//! out as
//!
//!     __INITLIB_LIST__ = .;
//!     QUAD((__INITLIB_END__ - __INITLIB_LIST__) / 8 - 2)   <- element count
//!     KEEP(*(.aros.set.INITLIB.-127))                      <- ascending by
//!     KEEP(*(.aros.set.INITLIB.10))                           priority
//!     KEEP(*(.aros.set.INITLIB))
//!     QUAD(0)                                              <- terminator
//!     __INITLIB_END__ = .;
//!
//! which is what `ForeachElementInSet` (`symbolsets.h:177`) reads: `set[0]` is
//! the count, `set[1..count]` the entries, and a NULL ends the walk. The script
//! assignment overrides the weak array, so a set with no entries keeps the
//! `{0, 0}` and reads as empty.
//!
//! Without that pass every set in every module stays `{0, 0}`, so no INITLIB,
//! EXPUNGELIB, OPENLIB, CLOSELIB, PREINITLIB, LIBS, INIT, CTORS or INIT_ARRAY
//! function ever runs. That was this build's state, and the boot died in
//! `Exec_init` at the first thing that needed one.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::elf::Class;

/// A section-name prefix that denotes a set, and where the set's name starts
/// inside it.
///
/// `tools/collect-aros/gensets.c:127` uses the same table. Its `.ctors`/
/// `.dtors` tests compare only five characters, so they also match `.ctor`;
/// the prefixes are spelled in full here because every real section name is
/// `.ctors` or `.ctors.<pri>` and the shorter compare cannot tell them apart
/// from a name this would rather report.
const PREFIXES: &[(&str, usize)] = &[
    (".aros.set.", 10),
    (".ctors", 1),
    (".dtors", 1),
    (".init_array", 1),
    (".fini_array", 1),
];

/// One set found in an object, with the priorities it has sections for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolSet {
    /// Uppercased set name, as `SETNAME(x)` spells it between the underscores.
    pub name: String,
    /// The section name without the priority suffix, e.g. `.aros.set.INITLIB`.
    pub section: String,
    /// Priorities that have a section, ascending. `get_setnode` orders them
    /// this way and `set_call_libfuncs` walks the array forward, so the order
    /// is the calling order.
    pub priorities: Vec<i64>,
    /// Whether an unsuffixed section exists as well. The reference emits a
    /// `KEEP` for it either way; doing the same keeps the script identical
    /// whether or not one turns up later.
    pub bare: bool,
    /// `.aros.set.*` rather than a compiler set. Only these get the
    /// handler-missing reference (`collect-aros.c:342`).
    pub aros_set: bool,
}

/// What one set section name says.
struct Parsed {
    /// Uppercased set name.
    name: String,
    /// The section name without the priority suffix.
    section: String,
    /// The priority, absent for an unsuffixed section.
    priority: Option<i64>,
    /// `.aros.set.*` rather than a compiler set.
    aros_set: bool,
}

/// Splits a section name into its set name, its unsuffixed section name and
/// its priority.
///
/// Returns `Err` with a reason for a name that looks like a set section but
/// cannot be laid out, rather than guessing. `strtol` in the reference returns
/// 0 for an unparsable suffix, so `.aros.set.FOO.bar` would be emitted as
/// `KEEP(*(.aros.set.FOO.0))` and silently match nothing.
fn parse(section: &str) -> Option<Result<Parsed, String>> {
    let (prefix, offset) = PREFIXES
        .iter()
        .find(|(prefix, _)| section.starts_with(*prefix))?;
    let aros_set = *prefix == ".aros.set.";
    let rest = &section[*offset..];
    // The priority is what follows the first dot after the set name.
    let (name, priority, base) = match rest.split_once('.') {
        Some((name, suffix)) => match suffix.parse::<i64>() {
            Ok(priority) => (
                name,
                Some(priority),
                section[..*offset + name.len()].to_owned(),
            ),
            Err(_) => {
                return Some(Err(format!(
                    "{section}: `{suffix}` is not a priority, so the section \
                     cannot be placed in a set"
                )))
            }
        },
        None => (rest, None, section.to_owned()),
    };
    if name.is_empty() {
        return Some(Err(format!("{section}: the set has no name")));
    }
    if !name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Some(Err(format!(
            "{section}: `{name}` is not usable in a symbol name"
        )));
    }
    Some(Ok(Parsed {
        name: name.to_ascii_uppercase(),
        section: base,
        priority,
        aros_set,
    }))
}

/// Finds every symbol set the object's section names imply.
///
/// Returns the sets and, for reporting, the section names that look like a set
/// and could not be laid out. Nothing is dropped silently.
pub fn discover(sections: &[String]) -> (Vec<SymbolSet>, Vec<String>) {
    let mut found: BTreeMap<String, SymbolSet> = BTreeMap::new();
    let mut skipped = Vec::new();

    for section in sections {
        match parse(section) {
            None => {}
            Some(Err(reason)) => skipped.push(reason),
            Some(Ok(parsed)) => {
                let base = parsed.section;
                let entry = found.entry(parsed.name.clone()).or_insert_with(|| SymbolSet {
                    name: parsed.name,
                    section: base.clone(),
                    priorities: Vec::new(),
                    bare: false,
                    aros_set: parsed.aros_set,
                });
                if entry.section != base {
                    skipped.push(format!(
                        "{section}: set {} already comes from {}, and one set \
                         cannot have two section names",
                        entry.name, entry.section
                    ));
                    continue;
                }
                match parsed.priority {
                    Some(priority) => {
                        if !entry.priorities.contains(&priority) {
                            entry.priorities.push(priority);
                        }
                    }
                    None => entry.bare = true,
                }
            }
        }
    }

    let mut sets: Vec<SymbolSet> = found.into_values().collect();
    for set in &mut sets {
        set.priorities.sort_unstable();
    }
    // Sets are independent arrays, so the order they appear in the script has
    // no meaning; sorted by name it is stable across runs. Within a set the
    // order is the priority order, and that one does matter.
    sets.sort_by(|left, right| left.name.cmp(&right.name));
    (sets, skipped)
}

/// The linker script that lays the sets out.
///
/// The arrays go into a section of their own rather than at the end of
/// `.rodata`, where `tools/collect-aros/ldscript.h` puts them. The reference
/// can place them there because its script rebuilds every output section
/// anyway; ours is added to a link that already has its layout, and claiming
/// `.rodata` here would refold every `.rodata.*` input as a side effect. A
/// section of its own is allocatable and read-only just the same, and the ELF
/// loader treats it like any other.
pub fn script(sets: &[SymbolSet], class: Class) -> String {
    let word = class.pointer_directive();
    let width = class.pointer_bytes();
    let mut out = String::new();

    // `collect-aros.c:342` references this for every .aros.set.* set it finds.
    // THIS_PROGRAM_HANDLES_SYMBOLSET (symbolsets.h:189) defines it as a weak
    // absolute in the code that walks the set, so the reference fails a final
    // link whose sets nothing handles. Our links are all relocatable and
    // cannot fail on an undefined symbol, so it lands in the symbol audit
    // instead -- which is the same statement, made later.
    for set in sets.iter().filter(|set| set.aros_set) {
        let _ = writeln!(out, "EXTERN(__{}__symbol_set_handler_missing)", set.name);
    }

    out.push_str("SECTIONS\n{\n  .aros.sets : {\n");
    for set in sets {
        let name = &set.name;
        let _ = writeln!(out, "    . = ALIGN({width});");
        let _ = writeln!(out, "    __{name}_LIST__ = .;");
        let _ = writeln!(
            out,
            "    {word}((__{name}_END__ - __{name}_LIST__) / {width} - 2)"
        );
        for priority in &set.priorities {
            let _ = writeln!(out, "    KEEP(*({}.{priority}))", set.section);
        }
        let _ = writeln!(out, "    KEEP(*({}))", set.section);
        let _ = writeln!(out, "    {word}(0)");
        let _ = writeln!(out, "    __{name}_END__ = .;");
    }
    out.push_str("  }\n}\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn priorities_are_ordered_numerically_not_lexically() {
        let (sets, skipped) = discover(&names(&[
            ".text",
            ".aros.set.INITLIB.10",
            ".aros.set.INITLIB.-127",
            ".aros.set.INITLIB.0",
        ]));
        assert!(skipped.is_empty(), "{skipped:?}");
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].name, "INITLIB");
        assert_eq!(sets[0].section, ".aros.set.INITLIB");
        assert_eq!(sets[0].priorities, [-127, 0, 10]);
        assert!(!sets[0].bare);
        assert!(sets[0].aros_set);
    }

    #[test]
    fn an_unsuffixed_section_is_the_bare_one() {
        let (sets, skipped) = discover(&names(&[".aros.set.LIBS", ".aros.set.LIBS.0"]));
        assert!(skipped.is_empty(), "{skipped:?}");
        assert_eq!(sets[0].priorities, [0]);
        assert!(sets[0].bare);
    }

    #[test]
    fn the_compiler_sets_keep_their_own_names() {
        let (sets, skipped) = discover(&names(&[
            ".ctors.65435",
            ".dtors",
            ".init_array.100",
            ".fini_array",
        ]));
        assert!(skipped.is_empty(), "{skipped:?}");
        let found: Vec<(&str, &str, bool)> = sets
            .iter()
            .map(|set| (set.name.as_str(), set.section.as_str(), set.aros_set))
            .collect();
        assert_eq!(
            found,
            [
                ("CTORS", ".ctors", false),
                ("DTORS", ".dtors", false),
                ("FINI_ARRAY", ".fini_array", false),
                ("INIT_ARRAY", ".init_array", false),
            ]
        );
    }

    #[test]
    fn a_suffix_that_is_not_a_priority_is_reported_not_guessed() {
        let (sets, skipped) = discover(&names(&[".aros.set.FOO.bar"]));
        assert!(sets.is_empty(), "{sets:?}");
        assert_eq!(skipped.len(), 1, "{skipped:?}");
        assert!(skipped[0].contains("not a priority"), "{skipped:?}");
    }

    #[test]
    fn the_script_states_the_count_and_the_terminator() {
        let (sets, _) = discover(&names(&[".aros.set.INITLIB.10", ".aros.set.INITLIB.-1"]));
        let text = script(&sets, Class::Elf64);
        assert!(text.contains("EXTERN(__INITLIB__symbol_set_handler_missing)"));
        assert!(text.contains("__INITLIB_LIST__ = .;"));
        assert!(text.contains("QUAD((__INITLIB_END__ - __INITLIB_LIST__) / 8 - 2)"));
        // Ascending, and the unsuffixed section last.
        let minus_one = text.find("KEEP(*(.aros.set.INITLIB.-1))").unwrap();
        let ten = text.find("KEEP(*(.aros.set.INITLIB.10))").unwrap();
        let bare = text.find("KEEP(*(.aros.set.INITLIB))").unwrap();
        assert!(minus_one < ten && ten < bare, "{text}");
        assert!(text.contains("QUAD(0)"));
    }

    #[test]
    fn a_32_bit_object_gets_32_bit_entries() {
        let (sets, _) = discover(&names(&[".aros.set.INITLIB.0"]));
        let text = script(&sets, Class::Elf32);
        assert!(text.contains("LONG((__INITLIB_END__ - __INITLIB_LIST__) / 4 - 2)"));
    }
}
