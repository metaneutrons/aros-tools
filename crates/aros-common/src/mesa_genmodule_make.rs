//! Bounded, data-only parsing of original C `genmodule writemakefile` output.
//!
//! This module parses only a closed set of literal `+=` assignments. It does
//! not evaluate Make, identify or authenticate the producer, prove that
//! `genmodule` ran, or establish parity with another build system. Canonical
//! output is a normalized data representation, not a byte-for-byte source
//! identity claim.

use std::collections::BTreeMap;

const MAX_INPUT_BYTES: usize = 128 * 1024;
const MAX_MODULE_BYTES: usize = 128;

/// The exact assignment suffixes emitted by the supported original
/// `writemakefile` data projection, in canonical fragment order.
pub const MESA_GENMODULE_MAKE_SUFFIXES: [&str; 16] = [
    "STARTFILES",
    "ENDFILES",
    "MODDIR",
    "LINKLIBFILES",
    "RELLINKLIBFILES",
    "LINKLIBAFILES",
    "RELLINKLIBAFILES",
    "INCLUDES",
    "CPPFLAGS",
    "LINKLIBCPPFLAGS",
    "CFLAGS",
    "LINKLIBCFLAGS",
    "CXXFLAGS",
    "LINKLIBCXXFLAGS",
    "LDFLAGS",
    "LIBS",
];

/// Parsed literal assignments from one original C `genmodule writemakefile`
/// output.
///
/// `assignments` is keyed by the suffix in
/// [`MESA_GENMODULE_MAKE_SUFFIXES`]. Values preserve atom order, repetitions,
/// and defined-empty assignments exactly as data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MesaGenmoduleMakefile {
    /// The original owner prefix retained for canonical Make output.
    module: String,
    /// All sixteen suffixes, including those assigned an empty list.
    assignments: BTreeMap<String, Vec<String>>,
}

impl MesaGenmoduleMakefile {
    /// Parse a bounded, literal subset of original C `writemakefile` output.
    ///
    /// Supported input contains exactly one `MODULE_SUFFIX +=` assignment
    /// for each known suffix. Backslash-newline continuations are joined as
    /// whitespace; Make is never invoked or evaluated. This data structure
    /// does not authenticate the output's producer, source inputs, or
    /// execution history.
    ///
    /// # Errors
    /// Rejects unsafe module names, oversized or non-ASCII/control-containing
    /// input, malformed or unsupported assignments, invalid continuations,
    /// nonliteral atoms, unknown/foreign owners, duplicates, and missing
    /// suffixes.
    pub fn parse(module: &str, text: &str) -> Result<Self, String> {
        validate_module(module)?;
        if text.len() > MAX_INPUT_BYTES {
            return Err(format!(
                "genmodule Make output exceeds the {MAX_INPUT_BYTES}-byte limit"
            ));
        }
        if !text.is_ascii() {
            return Err("genmodule Make output must be ASCII".into());
        }
        if text
            .bytes()
            .any(|byte| byte == b'\r' || (byte < b' ' && byte != b'\n') || byte == 0x7f)
        {
            return Err("genmodule Make output contains a control character".into());
        }

        let mut assignments = BTreeMap::new();
        let mut pending: Option<(String, String)> = None;

        for (line_number, line) in text.split_terminator('\n').enumerate() {
            let line_number = line_number + 1;
            if let Some((key, mut value)) = pending.take() {
                if !line.starts_with(' ') {
                    return Err(format!(
                        "line {line_number}: expected an indented continuation"
                    ));
                }
                let continuation = line.trim_start_matches(' ');
                if continuation.is_empty() {
                    return Err(format!(
                        "line {line_number}: empty Make continuation is unsupported"
                    ));
                }
                let (fragment, continues) = continuation_fragment(continuation, line_number)?;
                value.push(' ');
                value.push_str(&fragment);
                if continues {
                    pending = Some((key, value));
                } else {
                    let suffix = validate_owner_and_suffix(module, &key, line_number)?;
                    assignments.insert(suffix.to_owned(), parse_atoms(&value, line_number)?);
                }
                continue;
            }

            if line.is_empty() {
                continue;
            }
            if line.starts_with(' ') {
                return Err(format!(
                    "line {line_number}: indented line has no preceding continuation"
                ));
            }

            let (key, value) = split_assignment(line, line_number)?;
            let suffix = validate_owner_and_suffix(module, &key, line_number)?;
            if assignments.contains_key(suffix) {
                return Err(format!(
                    "line {line_number}: duplicate assignment for {module}_{suffix}"
                ));
            }

            let (value, continues) = assignment_fragment(&value, line_number)?;
            if continues {
                pending = Some((key, value));
            } else {
                assignments.insert(suffix.to_owned(), parse_atoms(&value, line_number)?);
            }
        }

        if let Some((key, _)) = pending {
            return Err(format!(
                "unterminated Make continuation for assignment {key}"
            ));
        }

        for suffix in MESA_GENMODULE_MAKE_SUFFIXES {
            if !assignments.contains_key(suffix) {
                return Err(format!("missing required assignment {module}_{suffix}"));
            }
        }

        Ok(Self {
            module: module.to_owned(),
            assignments,
        })
    }

    /// Return the original genmodule module-name prefix.
    #[must_use]
    pub fn module(&self) -> &str {
        &self.module
    }

    /// Return the parsed suffix-to-ordered-atoms map.
    #[must_use]
    pub const fn assignments(&self) -> &BTreeMap<String, Vec<String>> {
        &self.assignments
    }

    /// Return one assignment's ordered atoms by suffix.
    pub fn values(&self, suffix: &str) -> Option<&[String]> {
        self.assignments.get(suffix).map(Vec::as_slice)
    }

    /// Render a normalized, single-line-per-assignment Make fragment.
    ///
    /// The original module owner is retained. Values remain literal atoms;
    /// empty assignments are emitted explicitly with no value atoms.
    ///
    /// # Panics
    /// Panics if the internal complete-assignment invariant is broken.
    /// [`Self::parse`] establishes it and callers cannot mutate the fields.
    #[must_use]
    pub fn canonical_make_fragment(&self) -> String {
        let mut fragment = String::new();
        for suffix in MESA_GENMODULE_MAKE_SUFFIXES {
            let values = self
                .assignments
                .get(suffix)
                .expect("validated Mesa genmodule assignment is present");
            fragment.push_str(&self.module);
            fragment.push('_');
            fragment.push_str(suffix);
            fragment.push_str(" +=");
            if !values.is_empty() {
                fragment.push(' ');
                fragment.push_str(&values.join(" "));
            }
            fragment.push('\n');
        }
        fragment
    }
}

fn validate_module(module: &str) -> Result<(), String> {
    if module.is_empty() || module.len() > MAX_MODULE_BYTES {
        return Err(format!(
            "genmodule module name must contain 1..={MAX_MODULE_BYTES} bytes"
        ));
    }
    if !module.is_ascii()
        || !module
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
    {
        return Err("genmodule module name contains an unsafe character".into());
    }
    Ok(())
}

fn split_assignment(line: &str, line_number: usize) -> Result<(String, String), String> {
    let mut operator = line.match_indices("+=");
    let Some((operator_index, _)) = operator.next() else {
        return Err(format!(
            "line {line_number}: expected a literal '+=' assignment"
        ));
    };
    if operator.next().is_some() {
        return Err(format!(
            "line {line_number}: multiple assignment operators are unsupported"
        ));
    }

    let lhs = line[..operator_index].trim_end_matches(' ');
    if lhs.is_empty()
        || lhs.starts_with(' ')
        || lhs.bytes().any(|byte| byte == b'\t' || byte == b' ')
    {
        return Err(format!(
            "line {line_number}: malformed genmodule assignment owner"
        ));
    }
    if operator_index == 0 || !line[..operator_index].ends_with(' ') {
        return Err(format!(
            "line {line_number}: '+=' must be separated from its assignment name"
        ));
    }

    let value = line[operator_index + 2..].trim_start_matches(' ');
    if value.contains('\t') {
        return Err(format!("line {line_number}: tab whitespace is unsupported"));
    }
    Ok((lhs.to_owned(), value.to_owned()))
}

fn validate_owner_and_suffix<'a>(
    module: &str,
    key: &'a str,
    line_number: usize,
) -> Result<&'a str, String> {
    let expected_prefix = format!("{module}_");
    if let Some(suffix) = key.strip_prefix(&expected_prefix) {
        if MESA_GENMODULE_MAKE_SUFFIXES.contains(&suffix) {
            return Ok(suffix);
        }
        return Err(format!(
            "line {line_number}: unknown genmodule assignment {key}"
        ));
    }

    for suffix in MESA_GENMODULE_MAKE_SUFFIXES {
        let suffix_with_separator = format!("_{suffix}");
        if let Some(owner) = key.strip_suffix(&suffix_with_separator) {
            if owner != module {
                return Err(format!(
                    "line {line_number}: foreign genmodule assignment owner {owner:?}"
                ));
            }
        }
    }
    Err(format!(
        "line {line_number}: unknown genmodule assignment {key}"
    ))
}

fn assignment_fragment(value: &str, line_number: usize) -> Result<(String, bool), String> {
    fragment(value, line_number, "assignment")
}

fn continuation_fragment(value: &str, line_number: usize) -> Result<(String, bool), String> {
    fragment(value, line_number, "continuation")
}

fn fragment(value: &str, line_number: usize, context: &str) -> Result<(String, bool), String> {
    let trimmed = value.trim_end_matches(' ');
    if trimmed.ends_with('\\') && value != trimmed {
        return Err(format!(
            "line {line_number}: backslash continuation marker must end the physical line"
        ));
    }
    let continued = trimmed.ends_with('\\');

    let contents = if continued {
        trimmed[..trimmed.len() - 1].trim_end_matches(' ')
    } else {
        trimmed
    };
    if contents.is_empty() && context == "continuation" {
        return Err(format!(
            "line {line_number}: empty Make continuation is unsupported"
        ));
    }
    Ok((contents.to_owned(), continued))
}

fn parse_atoms(value: &str, line_number: usize) -> Result<Vec<String>, String> {
    let mut atoms = Vec::new();
    for atom in value.split(' ').filter(|atom| !atom.is_empty()) {
        if !atom.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'/' | b'+' | b'-')
        }) {
            return Err(format!(
                "line {line_number}: nonliteral genmodule atom {atom:?}"
            ));
        }
        atoms.push(atom.to_owned());
    }
    Ok(atoms)
}

#[cfg(test)]
mod tests {
    use super::{MesaGenmoduleMakefile, MESA_GENMODULE_MAKE_SUFFIXES};
    use std::collections::BTreeMap;

    const MODULE: &str = "probe.name-2";

    fn synthetic_makefile() -> String {
        format!(
            "{MODULE}_STARTFILES += start\n\
             {MODULE}_ENDFILES += end\n\
             {MODULE}_MODDIR += Libs\n\
             {MODULE}_LINKLIBFILES += stub_a stub_a \\\n              stub_b\n\
             {MODULE}_RELLINKLIBFILES += rel_a\n\
             {MODULE}_LINKLIBAFILES +=\n\
             {MODULE}_RELLINKLIBAFILES +=\n\
             {MODULE}_INCLUDES += proto.h\n\
             {MODULE}_CPPFLAGS += -DREPEAT -DREPEAT \\\n              -DSECOND\n\
             {MODULE}_LINKLIBCPPFLAGS += -DLINK\n\
             {MODULE}_CFLAGS +=\n\
             {MODULE}_LINKLIBCFLAGS +=\n\
             {MODULE}_CXXFLAGS +=\n\
             {MODULE}_LINKLIBCXXFLAGS +=\n\
             {MODULE}_LDFLAGS +=\n\
             {MODULE}_LIBS += z1_rel stdc_rel\n"
        )
    }

    #[test]
    fn parses_literal_assignments_and_canonicalizes_data() {
        let text = synthetic_makefile();
        let parsed = MesaGenmoduleMakefile::parse(MODULE, &text).unwrap();

        assert_eq!(parsed.module(), MODULE);
        assert_eq!(
            parsed.assignments().len(),
            MESA_GENMODULE_MAKE_SUFFIXES.len()
        );
        assert_eq!(
            parsed.values("LINKLIBFILES").unwrap(),
            [
                "stub_a".to_owned(),
                "stub_a".to_owned(),
                "stub_b".to_owned()
            ]
        );
        assert_eq!(
            parsed.values("CPPFLAGS").unwrap(),
            [
                "-DREPEAT".to_owned(),
                "-DREPEAT".to_owned(),
                "-DSECOND".to_owned()
            ]
        );
        assert!(parsed.values("LINKLIBAFILES").unwrap().is_empty());
        assert_eq!(
            parsed.canonical_make_fragment(),
            format!(
                "{MODULE}_STARTFILES += start\n\
                 {MODULE}_ENDFILES += end\n\
                 {MODULE}_MODDIR += Libs\n\
                 {MODULE}_LINKLIBFILES += stub_a stub_a stub_b\n\
                 {MODULE}_RELLINKLIBFILES += rel_a\n\
                 {MODULE}_LINKLIBAFILES +=\n\
                 {MODULE}_RELLINKLIBAFILES +=\n\
                 {MODULE}_INCLUDES += proto.h\n\
                 {MODULE}_CPPFLAGS += -DREPEAT -DREPEAT -DSECOND\n\
                 {MODULE}_LINKLIBCPPFLAGS += -DLINK\n\
                 {MODULE}_CFLAGS +=\n\
                 {MODULE}_LINKLIBCFLAGS +=\n\
                 {MODULE}_CXXFLAGS +=\n\
                 {MODULE}_LINKLIBCXXFLAGS +=\n\
                 {MODULE}_LDFLAGS +=\n\
                 {MODULE}_LIBS += z1_rel stdc_rel\n"
            )
        );
    }

    #[test]
    fn rejects_duplicate_assignment() {
        let text = format!("{}{MODULE}_STARTFILES += another\n", synthetic_makefile());
        assert!(MesaGenmoduleMakefile::parse(MODULE, &text)
            .unwrap_err()
            .contains("duplicate assignment"));
    }

    #[test]
    fn rejects_unknown_assignment() {
        let text = format!("{MODULE}_UNKNOWN += value\n");
        assert!(MesaGenmoduleMakefile::parse(MODULE, &text)
            .unwrap_err()
            .contains("unknown genmodule assignment"));
    }

    #[test]
    fn rejects_missing_assignment() {
        let text = synthetic_makefile().replace(&format!("{MODULE}_MODDIR += Libs\n"), "");
        assert!(MesaGenmoduleMakefile::parse(MODULE, &text)
            .unwrap_err()
            .contains("missing required assignment"));
    }

    #[test]
    fn rejects_foreign_owner() {
        let text =
            synthetic_makefile().replacen(&format!("{MODULE}_STARTFILES"), "other_STARTFILES", 1);
        assert!(MesaGenmoduleMakefile::parse(MODULE, &text)
            .unwrap_err()
            .contains("foreign genmodule assignment owner"));
    }

    #[test]
    fn rejects_make_effect_syntax() {
        let text =
            synthetic_makefile().replacen("-DREPEAT -DREPEAT \\\n", "$(shell touch nope) \\\n", 1);
        assert!(MesaGenmoduleMakefile::parse(MODULE, &text)
            .unwrap_err()
            .contains("nonliteral genmodule atom"));
    }

    #[test]
    fn rejects_bad_continuations() {
        let unindented = synthetic_makefile().replacen("\\\n              stub_b", "\\\nstub_b", 1);
        assert!(MesaGenmoduleMakefile::parse(MODULE, &unindented)
            .unwrap_err()
            .contains("expected an indented continuation"));

        let whitespace_after_marker =
            synthetic_makefile().replacen("stub_a stub_a \\\n", "stub_a stub_a \\   \n", 1);
        assert!(
            MesaGenmoduleMakefile::parse(MODULE, &whitespace_after_marker)
                .unwrap_err()
                .contains("backslash continuation marker must end")
        );

        let dangling = format!("{MODULE}_STARTFILES += start\\\n");
        assert!(MesaGenmoduleMakefile::parse(MODULE, &dangling)
            .unwrap_err()
            .contains("unterminated Make continuation"));
    }

    #[test]
    fn rejects_oversized_input() {
        let text = "x".repeat(128 * 1024 + 1);
        assert!(MesaGenmoduleMakefile::parse(MODULE, &text)
            .unwrap_err()
            .contains("exceeds the 131072-byte limit"));
    }

    #[test]
    #[ignore = "requires the opt-in original genmodule writemakefile output"]
    fn parses_opt_in_real_mesa_output_and_rejects_mutation() {
        let path = std::env::var("AROS_TEST_GENMODULE_MAKEFILE")
            .expect("set AROS_TEST_GENMODULE_MAKEFILE to original writemakefile output");
        let text = std::fs::read_to_string(path).expect("read opt-in original Make output");
        let parsed = MesaGenmoduleMakefile::parse("mesa3dgl26-0", &text).unwrap();
        assert_eq!(parsed.values("LINKLIBFILES").unwrap().len(), 466);
        assert_eq!(parsed.values("RELLINKLIBFILES").unwrap().len(), 466);
        assert!(parsed
            .values("CPPFLAGS")
            .unwrap()
            .contains(&"-D__MESA3DGL26_0_NOLIBBASE__".to_owned()));
        assert!(!parsed
            .values("LINKLIBCPPFLAGS")
            .unwrap()
            .contains(&"-D__MESA3DGL26_0_NOLIBBASE__".to_owned()));

        let mutated = text.replacen("z1_rel", "$z1_rel", 1);
        assert_ne!(mutated, text, "the real-output mutation must take effect");
        assert!(MesaGenmoduleMakefile::parse("mesa3dgl26-0", &mutated).is_err());
    }

    #[test]
    #[ignore = "requires eight separately retained original genmodule outputs"]
    fn parses_opt_in_distinct_mesa_owners_without_conflating_module_names() {
        let root = std::path::PathBuf::from(
            std::env::var_os("AROS_TEST_GENMODULE_OWNERS_ROOT")
                .expect("set AROS_TEST_GENMODULE_OWNERS_ROOT to the retained owner directories"),
        );
        // These are explicit original-producer observations, not an inventory
        // selector for a target profile. In particular v3d is a declaration
        // probe, not proof that the selected ARM graph contains its binding.
        let cases = [
            (
                "mesa3dgl-library",
                "mesa3dgl26-0",
                "library",
                "Libs",
                466,
                466,
            ),
            ("workbench-libs-egl", "egl", "library", "Libs", 36, 0),
            ("workbench-libs-gallium", "gallium", "library", "Libs", 3, 0),
            ("hidd-gallium", "gallium", "hidd", "Devs/Drivers", 0, 0),
            ("hidd-llvmpipe", "llvmpipe", "hidd", "Devs/Drivers", 0, 0),
            (
                "hidd-vc4gallium",
                "vc4gallium",
                "hidd",
                "Devs/Drivers",
                0,
                0,
            ),
            ("hidd-v3d", "v3d", "hidd", "Devs/Drivers", 0, 0),
            (
                "workbench-libs-gl-clients",
                "gl",
                "library",
                "Libs",
                466,
                466,
            ),
        ];
        let mut owners = BTreeMap::new();
        for (owner, module, kind, directory, normal, relative) in cases {
            let path = root.join(owner).join(format!("Makefile.{module}{kind}"));
            let text = std::fs::read_to_string(&path).expect("read exact original owner output");
            let parsed = MesaGenmoduleMakefile::parse(module, &text)
                .unwrap_or_else(|error| panic!("{owner}: {error}"));
            assert_eq!(parsed.values("MODDIR").unwrap(), [directory], "{owner}");
            assert_eq!(
                parsed.values("STARTFILES").unwrap(),
                [format!("{module}_start")]
            );
            assert_eq!(
                parsed.values("ENDFILES").unwrap(),
                [format!("{module}_end")]
            );
            assert_eq!(
                parsed.values("LINKLIBFILES").unwrap().len(),
                normal,
                "{owner}"
            );
            assert_eq!(
                parsed.values("RELLINKLIBFILES").unwrap().len(),
                relative,
                "{owner}"
            );
            assert!(
                parsed.values("RELLINKLIBAFILES").unwrap().is_empty(),
                "{owner}"
            );
            assert_eq!(
                MesaGenmoduleMakefile::parse(module, &parsed.canonical_make_fragment()).unwrap(),
                parsed,
                "{owner} normalized data round trip"
            );
            let mutation = text.replacen(&format!("{module}_STARTFILES"), "foreign_STARTFILES", 1);
            assert_ne!(mutation, text, "{owner}: counter must change the owner");
            assert!(
                MesaGenmoduleMakefile::parse(module, &mutation).is_err(),
                "{owner}"
            );
            println!(
                "{owner} {module} {kind} {}",
                crate::sha256_bytes(text.as_bytes())
            );
            assert!(owners.insert(owner, parsed).is_none());
        }
        // Both original outputs have the same module prefix. A module-only
        // registry would silently overwrite one, despite different recipes.
        assert_eq!(
            owners["workbench-libs-gallium"].module(),
            owners["hidd-gallium"].module()
        );
        assert_ne!(owners["workbench-libs-gallium"], owners["hidd-gallium"]);
        let llvmpipe = &owners["hidd-llvmpipe"];
        assert_eq!(
            llvmpipe.values("CPPFLAGS").unwrap(),
            ["-D__POSIXC_RELLIBBASE__", "-D__STDC_RELLIBBASE__"]
        );
        assert!(
            llvmpipe.values("RELLINKLIBFILES").unwrap().is_empty(),
            "rellib dependencies do not enable relative clients"
        );
        let gl = &owners["workbench-libs-gl-clients"];
        assert_eq!(gl.values("CPPFLAGS").unwrap(), ["-D__GL_NOLIBBASE__"]);
        assert!(gl.values("LINKLIBCPPFLAGS").unwrap().is_empty());
        assert_ne!(
            gl.values("CPPFLAGS"),
            owners["mesa3dgl-library"].values("CPPFLAGS")
        );
        assert_eq!(
            owners["mesa3dgl-library"].values("CPPFLAGS").unwrap(),
            [
                "-D__Z1_RELLIBBASE__",
                "-D__POSIXC_RELLIBBASE__",
                "-D__STDC_RELLIBBASE__",
                "-D__MESA3DGL26_0_NOLIBBASE__"
            ]
        );
    }
}
