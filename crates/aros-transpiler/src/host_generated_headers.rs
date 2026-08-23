//! A public header written by a host tool the build compiles first.
//!
//! `arch/i386-all/include/mmakefile.src:24` and `arch/m68k-all/include:17` are
//! the only users, three headers from two tools, and nothing modelled or
//! reported them:
//!
//! ```text
//! $(AROS_INCLUDES)/aros/i386/libcall.h: $(HOSTGENDIR)/tools/gencall_i386 | ...
//!     $(HOSTGENDIR)/tools/gencall_i386 >$@
//!
//! $(HOSTGENDIR)/tools/gencall_i386: $(SRCDIR)/$(CURDIR)/gencall.c
//!     $(HOST_CC) -Wall -Werror -o $@ $<
//! ```
//!
//! Only i386 and m68k need it, because only their `aros/cpu.h` sets
//! `__AROS_LIBCALL_H_FILE` (`arch/i386-all/include/aros/cpu.h:148`). An x86_64
//! build never asks for the header, which is why its absence stayed invisible
//! until the 32-bit PC bootstrap was compiled.
//!
//! Deliberately narrow: one host tool from one C source, invoked with literal
//! arguments and its standard output redirected to one header. A recipe outside
//! that shape is reported, never guessed at.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// One header a host tool writes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostGeneratedHeader {
    /// Tool name, as the rule spells it under `$(HOSTGENDIR)/tools/`.
    pub tool: String,
    /// The tool's single C source, relative to the source root.
    pub source: String,
    /// Literal arguments the recipe passes before the redirect.
    pub arguments: Vec<String>,
    /// Header path relative to the include root, e.g. `aros/i386/libcall.h`.
    pub header: String,
}

fn recipe_lines(lines: &[&str], mut index: usize) -> Vec<String> {
    let mut out = Vec::new();
    index += 1;
    while let Some(line) = lines.get(index) {
        if !line.starts_with('\t') {
            break;
        }
        out.push(line.trim().to_owned());
        index += 1;
    }
    out
}

/// The tool name a `$(HOSTGENDIR)/tools/<name>` path denotes.
fn host_tool_name(path: &str) -> Option<&str> {
    let rest = path.trim().strip_prefix("$(HOSTGENDIR)/tools/")?;
    (!rest.is_empty() && !rest.contains(['/', '$', ' ', ';'])).then_some(rest)
}

/// Collects the host-tool header rules of one mmakefile.
///
/// Returns the declarations and, for reporting, every rule that looks like one
/// and is not representable.
pub fn collect_host_generated_headers(
    content: &str,
    rel_dir: &Path,
) -> (Vec<HostGeneratedHeader>, Vec<String>) {
    let directory = rel_dir.to_string_lossy().replace('\\', "/");
    let lines: Vec<&str> = content.lines().collect();
    let mut out: Vec<HostGeneratedHeader> = Vec::new();
    let mut skipped = Vec::new();
    // tool name -> its single C source, from the tool's own rule.
    let mut sources: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();

    // First pass: the rules that build the tools.
    for (index, line) in lines.iter().enumerate() {
        if line.starts_with(char::is_whitespace) || line.starts_with('#') {
            continue;
        }
        let Some((target, prereqs)) = line.split_once(':') else {
            continue;
        };
        let Some(tool) = host_tool_name(target) else {
            continue;
        };
        let recipe = recipe_lines(&lines, index);
        let prereq = prereqs.trim();
        let Some(relative) = prereq.strip_prefix("$(SRCDIR)/$(CURDIR)/") else {
            skipped.push(format!(
                "{directory}:{}: host tool {tool} is built from `{prereq}`, \
                 which is not one source in this directory",
                index + 1
            ));
            continue;
        };
        // One command, the host compiler, one source, one output.
        let compiles = recipe.len() == 1
            && recipe[0].contains("$(HOST_CC)")
            && recipe[0].contains("-o $@")
            && recipe[0].contains("$<");
        if !compiles {
            skipped.push(format!(
                "{directory}:{}: host tool {tool} has a recipe this does not \
                 model: {recipe:?}",
                index + 1
            ));
            continue;
        }
        let is_c = std::path::Path::new(relative)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("c"));
        if relative.contains(['$', ';', ' ']) || !is_c {
            skipped.push(format!(
                "{directory}:{}: host tool {tool} source `{relative}` is not a \
                 plain C file",
                index + 1
            ));
            continue;
        }
        sources.insert(tool.to_owned(), format!("{directory}/{relative}"));
    }

    // Second pass: the headers those tools write.
    for (index, line) in lines.iter().enumerate() {
        if line.starts_with(char::is_whitespace) || line.starts_with('#') {
            continue;
        }
        let Some((target, prereqs)) = line.split_once(':') else {
            continue;
        };
        let target = target.trim();
        let Some(header) = target.strip_prefix("$(AROS_INCLUDES)/") else {
            continue;
        };
        // `| $(dir)` is an order-only directory prerequisite.
        let prereq = prereqs.split('|').next().unwrap_or_default().trim();
        let Some(tool) = host_tool_name(prereq) else {
            continue;
        };
        let recipe = recipe_lines(&lines, index);
        if recipe.len() != 1 {
            skipped.push(format!(
                "{directory}:{}: {header} has {} recipe lines, not one",
                index + 1,
                recipe.len()
            ));
            continue;
        }
        // `$(HOSTGENDIR)/tools/<tool> [args] >$@`
        let Some((invocation, redirect)) = recipe[0].split_once('>') else {
            skipped.push(format!(
                "{directory}:{}: {header} recipe does not redirect to the header",
                index + 1
            ));
            continue;
        };
        if redirect.trim() != "$@" {
            skipped.push(format!(
                "{directory}:{}: {header} redirects to `{}`, not to the header",
                index + 1,
                redirect.trim()
            ));
            continue;
        }
        let mut words = invocation.split_whitespace();
        let Some(first) = words.next() else { continue };
        if host_tool_name(first) != Some(tool) {
            skipped.push(format!(
                "{directory}:{}: {header} runs `{first}`, not its prerequisite",
                index + 1
            ));
            continue;
        }
        let arguments: Vec<String> = words.map(str::to_owned).collect();
        if arguments
            .iter()
            .any(|argument| argument.contains(['$', ';', '|', '"']))
        {
            skipped.push(format!(
                "{directory}:{}: {header} passes arguments this does not model: \
                 {arguments:?}",
                index + 1
            ));
            continue;
        }
        let Some(source) = sources.get(tool) else {
            skipped.push(format!(
                "{directory}:{}: {header} needs host tool {tool}, which this \
                 file does not build",
                index + 1
            ));
            continue;
        };
        if header.contains(['$', ';', ' ']) {
            skipped.push(format!(
                "{directory}:{}: header path `{header}` is not plain",
                index + 1
            ));
            continue;
        }
        out.push(HostGeneratedHeader {
            tool: tool.to_owned(),
            source: source.clone(),
            arguments,
            header: header.to_owned(),
        });
    }

    (out, skipped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../..")
            .canonicalize()
            .unwrap()
    }

    #[test]
    fn the_i386_libcall_header_and_its_tool_are_recognised() {
        let root = root();
        let rel = PathBuf::from("arch/i386-all/include");
        let content = aros_common::read_source(&root.join(&rel).join("mmakefile.src")).unwrap();
        let (headers, skipped) = collect_host_generated_headers(&content, &rel);

        assert!(skipped.is_empty(), "{skipped:?}");
        assert_eq!(headers.len(), 1, "{headers:#?}");
        assert_eq!(headers[0].tool, "gencall_i386");
        assert_eq!(headers[0].source, "arch/i386-all/include/gencall.c");
        assert_eq!(headers[0].header, "aros/i386/libcall.h");
        assert!(headers[0].arguments.is_empty());
    }

    #[test]
    fn the_m68k_tool_writes_two_headers_and_takes_an_argument() {
        let root = root();
        let rel = PathBuf::from("arch/m68k-all/include");
        let content = aros_common::read_source(&root.join(&rel).join("mmakefile.src")).unwrap();
        let (headers, skipped) = collect_host_generated_headers(&content, &rel);

        assert!(skipped.is_empty(), "{skipped:?}");
        let mut named: Vec<(&str, &[String])> = headers
            .iter()
            .map(|header| (header.header.as_str(), header.arguments.as_slice()))
            .collect();
        named.sort_by_key(|(name, _)| *name);
        assert_eq!(named.len(), 2, "{headers:#?}");
        assert_eq!(named[0].0, "aros/m68k/asmcall.h");
        assert_eq!(named[0].1, ["asmcall"]);
        assert_eq!(named[1].0, "aros/m68k/libcall.h");
        assert_eq!(named[1].1, ["libcall"]);
    }

    #[test]
    fn a_recipe_outside_the_shape_is_reported() {
        let rel = PathBuf::from("fixture");
        let content = "$(HOSTGENDIR)/tools/gen: $(SRCDIR)/$(CURDIR)/gen.c\n\
                       \t$(HOST_CC) -o $@ $<\n\
                       $(AROS_INCLUDES)/x/y.h: $(HOSTGENDIR)/tools/gen\n\
                       \t$(HOSTGENDIR)/tools/gen | sort >$@\n";
        let (headers, skipped) = collect_host_generated_headers(content, &rel);
        assert!(headers.is_empty(), "{headers:#?}");
        assert_eq!(skipped.len(), 1, "{skipped:?}");
        assert!(skipped[0].contains("arguments"), "{skipped:?}");
    }
}
