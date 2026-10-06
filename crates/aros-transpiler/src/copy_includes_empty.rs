//! Empty header lists are real MetaMake endpoints only when absence is known.
//! This does not use failed expansion as evidence that a list is empty.

use std::path::Path;

use super::CopyIncludesDecl;
use crate::make_vars::{variable_assignment, ConditionalTruth, VarScope};
use crate::parser::{macro_arg, macro_invocations, TargetContext};

// This proof accepts only complete scalar arguments, not a partial keyword
// scan that might overlook an additional command or duplicate override.
fn arguments(raw: &str) -> Option<std::collections::BTreeMap<String, String>> {
    if raw.len() > 65_536 || raw.contains(['\n', '\r', ';', '\\', '`']) {
        return None;
    }
    let mut result = std::collections::BTreeMap::new();
    let mut rest = raw.trim();
    while !rest.is_empty() {
        let (name, after) = rest.split_once('=')?;
        if !matches!(name, "includes" | "mmake") || result.contains_key(name) {
            return None;
        }
        let (value, after) = if let Some(quoted) = after.strip_prefix('"') {
            let (value, tail) = quoted.split_once('"')?;
            if !tail.is_empty() && !tail.starts_with(char::is_whitespace) {
                return None;
            }
            (value, tail)
        } else {
            let split = after.find(char::is_whitespace).unwrap_or(after.len());
            let (value, tail) = after.split_at(split);
            if value.is_empty() || value.contains(['"', '\'']) {
                return None;
            }
            (value, tail)
        };
        result.insert(name.to_owned(), value.to_owned());
        rest = after.trim_start();
    }
    Some(result)
}

pub fn collect_proven_empty(
    joined: &str,
    scope: &VarScope,
    states: &[ConditionalTruth],
    target: Option<&TargetContext>,
    rel_dir: &Path,
) -> Vec<CopyIncludesDecl> {
    if rel_dir.as_os_str().is_empty()
        || rel_dir
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Vec::new();
    }
    // The include expander deliberately leaves some global imports visible.
    // No absence proof may borrow a fallback through an unclosed import, a
    // deferred define body, or a dynamic Make assignment side effect.
    if joined.lines().any(|line| {
        let line = line.trim();
        if line.starts_with('#') {
            return false;
        }
        let words: Vec<_> = line.split_whitespace().collect();
        words
            .first()
            .is_some_and(|word| matches!(*word, "include" | "-include" | "sinclude"))
            || words.iter().any(|word| matches!(*word, "define" | "endef"))
            || line.contains("$(eval")
            || line.contains("${eval")
    }) {
        return Vec::new();
    }
    let invocations = macro_invocations(joined);
    let mut result = Vec::new();
    for invocation in &invocations {
        if invocation.name != "copy_includes"
            || states.get(invocation.line) != Some(&ConditionalTruth::True)
            || !joined
                .lines()
                .nth(invocation.line)
                .is_some_and(|line| line.starts_with("%copy_includes"))
        {
            continue;
        }
        let Some(arguments) = arguments(&invocation.args) else {
            continue;
        };
        let raw = arguments
            .get("includes")
            .map_or("$(INCLUDE_FILES)", String::as_str);
        let line = invocation.line + 1;
        let known_empty = if raw.trim().is_empty() {
            true
        } else if let Some(name) = raw.strip_prefix("$(").and_then(|raw| raw.strip_suffix(')')) {
            if name.is_empty()
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                || scope.conditionally_assigned_before(name, line)
            {
                false
            } else {
                scope.raw_at(name, line).map_or_else(
                    || {
                        !scope.is_known_local(name)
                            && target
                                .and_then(|target| target.value_of(name))
                                .is_some_and(|value| value.trim().is_empty())
                    },
                    |value| value.trim().is_empty(),
                )
            }
        } else {
            false
        };
        if !known_empty {
            continue;
        }
        let name = arguments
            .get("mmake")
            .cloned()
            .unwrap_or_else(|| "includes-copy".to_owned());
        if !name.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
            || !name.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            // The macro owns its Make endpoint. A separate handwritten rule
            // might contain a recipe or additional output prerequisites.
            || joined.lines().any(|line| {
                let line = line.trim();
                if line.starts_with('#') || variable_assignment(line).is_some() {
                    return false;
                }
                line.split_once(':').is_some_and(|(owners, _)| {
                    // A multi-target rule owns every spelling. Dynamic or
                    // pattern LHSs can alias this owner; absence is not proved.
                    owners.contains(['$', '%', '\\'])
                        || owners.split_whitespace().any(|owner| owner == name)
                })
            })
            || invocations.iter().any(|candidate| {
                macro_arg(&candidate.args, "mmake")
                    .is_some_and(|owner| owner.contains(['$', '%', '\\']))
            })
            || invocations.iter().filter(|candidate| {
                macro_arg(&candidate.args, "mmake")
                    .as_deref()
                    .unwrap_or(if candidate.name == "copy_includes" { "includes-copy" } else { "" })
                    == name
            }).count() != 1
        {
            continue;
        }
        result.push(CopyIncludesDecl {
            name,
            dest: ".".to_owned(),
            source_dir: rel_dir.to_string_lossy().replace('\\', "/"),
            patterns: Vec::new(),
            excludes: Vec::new(),
            flatten: false,
            proven_empty: true,
        });
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::make_vars::collect_vars_impl;

    fn collect(source: &str, configured: bool) -> Vec<CopyIncludesDecl> {
        let mut target = TargetContext::default();
        if configured {
            target
                .make_variables
                .insert("INCLUDE_FILES".to_owned(), String::new());
        }
        let (scope, states) = collect_vars_impl(source, Some(&target));
        collect_proven_empty(source, &scope, &states, Some(&target), Path::new("fixture"))
    }

    #[test]
    fn only_explicit_known_empty_lists_become_endpoints() {
        for source in [
            "INCLUDE_FILES :=\n%copy_includes\n",
            "%copy_includes includes=\"\"\n",
        ] {
            let declarations = collect(source, false);
            assert_eq!(declarations.len(), 1, "{source}");
            assert!(declarations[0].proven_empty);
            assert!(declarations[0].patterns.is_empty());
        }
        assert_eq!(collect("%copy_includes\n", true).len(), 1);
        // Macro arguments expand at declaration time, not after a later local
        // assignment. A known configured empty default remains empty here.
        assert_eq!(
            collect("%copy_includes\nINCLUDE_FILES = header.h\n", true).len(),
            1
        );
        assert!(collect("%copy_includes\n", false).is_empty());
    }

    #[test]
    fn uncertain_or_nonempty_local_values_cannot_borrow_configured_empty() {
        for source in [
            "INCLUDE_FILES = header.h\n%copy_includes\n",
            "INCLUDE_FILES = $(UNKNOWN)\n%copy_includes\n",
            "ifeq ($(UNKNOWN),yes)\nINCLUDE_FILES :=\nendif\n%copy_includes\n",
            "define INCLUDE_FILES\n\nendef\n%copy_includes\n",
            "ifeq ($(UNKNOWN),yes)\n%copy_includes includes=\"\"\nendif\n",
            "%copy_includes includes=\"$(shell echo)\"\n",
        ] {
            assert!(collect(source, true).is_empty(), "{source}");
        }
    }

    #[test]
    fn owner_collisions_and_argument_overrides_are_refused() {
        for source in [
            "%copy_includes includes=\"\" path=hidd\n",
            "%copy_includes includes=\"\" compiler=host\n",
            "%copy_includes includes=\"\" dir=include\n",
            "%copy_includes includes=\"\" includedir=/tmp\n",
            "%copy_includes includes=\"\" mmake=$(OWNER)\n",
            "%copy_includes includes=\"\" mmake=x;x\n",
            "%copy_includes includes=\"\"\nincludes-copy : output.h\n",
            "%copy_includes includes=\"\" mmake=owner\nowner alias : output.h\n",
            "OWNER = owner\n%copy_includes includes=\"\" mmake=owner\n$(OWNER): output.h\n",
            "%copy_includes includes=\"\" mmake=owner\n%copy_includes includes=\"foo.h\" mmake=owner\n",
            "OWNER = owner\n%copy_includes includes=\"\" mmake=owner\n%copy_includes includes=\"foo.h\" mmake=$(OWNER)\n",
            "%copy_includes includes=\"\"\n%copy_includes includes=\"\"\n",
            "%copy_includes includes=\"\" mmake=owner mmake=other\n",
            "%copy_includes includes=\"\" includes=header.h\n",
            "%copy_includes includes=\"\" garbage\n",
            "%copy_includes includes=\"\"&&echo\n",
            "include $(SRCDIR)/config/aros.cfg\n%copy_includes\n",
            "-include optional.mk\n%copy_includes\n",
            "sinclude optional.mk\n%copy_includes\n",
            "owner:\n\t%copy_includes includes=\"\"\n",
            "define inert\n%copy_includes includes=\"\"\nendef\n",
            "override define inert\n%copy_includes includes=\"\"\nendef\n",
            "override export private override define inert\n%copy_includes includes=\"\"\nendef\n",
            "$(eval INCLUDE_FILES = header.h)\n%copy_includes\n",
        ] {
            assert!(collect(source, true).is_empty(), "{source}");
        }
    }
}
