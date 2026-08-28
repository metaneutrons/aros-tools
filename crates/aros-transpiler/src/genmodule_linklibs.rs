//! Resolves link-library source lists populated by a build-time genmodule
//! `writefiles` command.
//!
//! GNU Make's `wildcard` legitimately returns an empty list before the command
//! runs.  That cannot be carried into CMake: a static-library target must know
//! its sources while CMake generates Ninja.  The output names are deterministic
//! from the `.conf`, so this module replaces only the recognised wildcard
//! families with a marker consumed by `aros_add_linklib`. Literal sources in
//! the same list remain ordinary sources.

use std::path::Path;

const MARKER_TAG: &str = "@AROS_GENMODULE";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedLinklibSources {
    /// Literal source stems plus one generated-manifest marker.
    pub sources: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Variant {
    Normal,
    Rel,
}

impl Variant {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Rel => "rel",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Component {
    StackStubs,
    RegcallStubs,
    Autoinit,
    Getlibbase,
}

impl Component {
    const ORDERED: [Self; 4] = [
        Self::StackStubs,
        Self::RegcallStubs,
        Self::Autoinit,
        Self::Getlibbase,
    ];

    const fn as_str(self) -> &'static str {
        match self {
            Self::StackStubs => "stackstubs",
            Self::RegcallStubs => "regcallstubs",
            Self::Autoinit => "autoinit",
            Self::Getlibbase => "getlibbase",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WildcardFamily {
    directory: String,
    variant: Variant,
    component: Component,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WritefilesCommand {
    config: String,
    stub_directory: String,
    module: String,
    module_type: String,
}

/// Split a Make list without splitting the whitespace inside nested `$(...)`.
fn split_make_fragments(raw: &str) -> Vec<&str> {
    let mut fragments = Vec::new();
    let mut start = None;
    let mut depth = 0usize;
    for (at, character) in raw.char_indices() {
        match character {
            '(' => {
                depth += 1;
                start.get_or_insert(at);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                start.get_or_insert(at);
            }
            _ if character.is_whitespace() && depth == 0 => {
                if let Some(begin) = start.take() {
                    fragments.push(&raw[begin..at]);
                }
            }
            _ => {
                start.get_or_insert(at);
            }
        }
    }
    if let Some(begin) = start {
        fragments.push(&raw[begin..]);
    }
    fragments
}

/// `files=$(NAME:.c=)` is how both declarations remove source suffixes after
/// collecting generated C names. Return the underlying variable and suffix.
fn source_variable(raw: &str) -> Option<(&str, bool)> {
    let body = raw.trim().strip_prefix("$(")?.strip_suffix(')')?.trim();
    if let Some(name) = body.strip_suffix(":.c=") {
        return valid_variable(name).then_some((name, true));
    }
    valid_variable(body).then_some((body, false))
}

fn valid_variable(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
}

fn wildcard_family(fragment: &str) -> Option<WildcardFamily> {
    let pattern = fragment
        .trim()
        .strip_prefix("$(wildcard")?
        .strip_suffix(')')?
        .trim();
    let (directory, file_pattern) = pattern.rsplit_once('/')?;
    let (variant, component) = match file_pattern {
        "*_stub.c" => (Variant::Normal, Component::StackStubs),
        "*_stubs.c" => (Variant::Normal, Component::RegcallStubs),
        "*_autoinit.c" => (Variant::Normal, Component::Autoinit),
        "*_getlibbase.c" => (Variant::Normal, Component::Getlibbase),
        "*_relstub.c" => (Variant::Rel, Component::StackStubs),
        "*_relstubs.c" => (Variant::Rel, Component::RegcallStubs),
        "*_relautoinit.c" => (Variant::Rel, Component::Autoinit),
        "*_relgetlibbase.c" => (Variant::Rel, Component::Getlibbase),
        _ => return None,
    };
    Some(WildcardFamily {
        directory: directory.trim_end_matches('/').to_owned(),
        variant,
        component,
    })
}

fn writefiles_commands(joined: &str) -> Vec<WritefilesCommand> {
    let mut commands = Vec::new();
    for line in joined.lines() {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        let Some(tool_at) = tokens
            .iter()
            .position(|token| token.trim_start_matches('@') == "$(GENMODULE)")
        else {
            continue;
        };
        let Some(write_at) = tokens.iter().position(|token| *token == "writefiles") else {
            continue;
        };
        if write_at <= tool_at || write_at + 2 >= tokens.len() {
            continue;
        }
        let options = &tokens[tool_at + 1..write_at];
        let option = |name: &str| {
            options
                .iter()
                .position(|token| *token == name)
                .and_then(|at| options.get(at + 1).copied())
        };
        let (Some(config), Some(gen_directory)) = (option("-c"), option("-d")) else {
            continue;
        };
        let stub_directory = option("-l").unwrap_or(gen_directory);
        commands.push(WritefilesCommand {
            config: config.to_owned(),
            stub_directory: stub_directory.trim_end_matches('/').to_owned(),
            module: tokens[write_at + 1].to_owned(),
            module_type: tokens[write_at + 2].to_owned(),
        });
    }
    commands
}

fn marker_config(config: &str, relative_dir: &Path) -> Result<String, String> {
    const LOCAL_PREFIX: &str = "$(SRCDIR)/$(CURDIR)/";
    if let Some(local) = config.strip_prefix(LOCAL_PREFIX) {
        return valid_marker_field(local, "config");
    }

    if let Some(from_root) = config.strip_prefix("$(SRCDIR)/") {
        let relative = relative_dir.to_string_lossy();
        if let Some(local) = from_root
            .strip_prefix(relative.as_ref())
            .and_then(|rest| rest.strip_prefix('/'))
        {
            return valid_marker_field(local, "config");
        }
        let from_root = valid_marker_field(from_root, "config")?;
        return Ok(format!("${{CMAKE_SOURCE_DIR}}/{from_root}"));
    }

    valid_marker_field(config, "config")
}

fn valid_marker_field(value: &str, purpose: &str) -> Result<String, String> {
    if value.is_empty()
        || value.contains(['|', ';', '$', '*', '?'])
        || value.chars().any(char::is_whitespace)
    {
        return Err(format!(
            "genmodule {purpose} `{value}` cannot be encoded in a source marker"
        ));
    }
    Ok(value.to_owned())
}

/// Resolve a generated linklib source expression.
///
/// `raw_variable` returns the right-hand side visible at the declaration line;
/// using a callback keeps this module independent of the parser's positional
/// variable-scope implementation. `Ok(None)` means the expression is not this
/// feature's recognised shape. Once a recognised genmodule wildcard is found,
/// ambiguity is an error: falling back to an empty Make wildcard would silently
/// restore the missing target.
///
/// # Errors
///
/// Returns an error when a recognized generated-linklib expression is
/// ambiguous, unsafe, or cannot be mapped to its declared generator.
pub fn resolve_generated_linklib_sources<F>(
    files: &str,
    joined: &str,
    relative_dir: &Path,
    mut raw_variable: F,
) -> Result<Option<GeneratedLinklibSources>, String>
where
    F: FnMut(&str) -> Option<String>,
{
    let Some((variable, strip_c_suffix)) = source_variable(files) else {
        return Ok(None);
    };
    let Some(raw_sources) = raw_variable(variable) else {
        return Ok(None);
    };

    let mut literals = Vec::new();
    let mut families = Vec::new();
    for fragment in split_make_fragments(&raw_sources) {
        if let Some(family) = wildcard_family(fragment) {
            families.push(family);
            continue;
        }
        if fragment.contains("$(wildcard") {
            // A wildcard exists but belongs to another generated source
            // family. Leave it to the ordinary bounded Make evaluator.
            return Ok(None);
        }
        if fragment.contains(['$', '(', ')']) {
            return Ok(None);
        }
        let literal = if strip_c_suffix {
            fragment.strip_suffix(".c").unwrap_or(fragment)
        } else {
            fragment
        };
        if !literal.is_empty() {
            literals.push(literal.to_owned());
        }
    }
    if families.is_empty() {
        return Ok(None);
    }

    let directory = &families[0].directory;
    let variant = families[0].variant;
    if families
        .iter()
        .any(|family| family.directory != *directory || family.variant != variant)
    {
        return Err(format!(
            "files={files} mixes genmodule output directories or normal/rel variants"
        ));
    }

    let commands: Vec<_> = writefiles_commands(joined)
        .into_iter()
        .filter(|command| command.stub_directory == *directory)
        .collect();
    let [command] = commands.as_slice() else {
        return Err(format!(
            "files={files} selects genmodule outputs in `{directory}`, but found {} matching writefiles commands",
            commands.len()
        ));
    };

    let mut component_names = Vec::new();
    for component in Component::ORDERED {
        if families.iter().any(|family| family.component == component) {
            component_names.push(component.as_str());
        }
    }
    let config = marker_config(&command.config, relative_dir)?;
    let module = valid_marker_field(&command.module, "module name")?;
    let module_type = valid_marker_field(&command.module_type, "module type")?;
    let marker = format!(
        "{MARKER_TAG}|{}|{}|{module}|{module_type}|{config}",
        variant.as_str(),
        component_names.join(",")
    );
    literals.push(marker);
    Ok(Some(GeneratedLinklibSources { sources: literals }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const GL_RULE: &str = "\
GL_LIB_SOURCES := gl_funcs.c $(wildcard $(GENDIR)/$(CURDIR)/*_stub.c) \
 $(wildcard $(GENDIR)/$(CURDIR)/*_stubs.c) \
 $(wildcard $(GENDIR)/$(CURDIR)/*_autoinit.c) \
 $(wildcard $(GENDIR)/$(CURDIR)/*_getlibbase.c)\n\
\t@$(GENMODULE) -c $(SRCDIR)/$(CURDIR)/gl.conf \
 -d $(GENDIR)/$(CURDIR) writefiles gl library\n";

    #[test]
    fn preserves_literals_and_encodes_the_full_normal_gl_family() {
        let result = resolve_generated_linklib_sources(
            "$(GL_LIB_SOURCES:.c=)",
            GL_RULE,
            Path::new("workbench/libs/gl"),
            |name| {
                (name == "GL_LIB_SOURCES").then(|| {
                    "gl_funcs.c $(wildcard $(GENDIR)/$(CURDIR)/*_stub.c) \
                     $(wildcard $(GENDIR)/$(CURDIR)/*_stubs.c) \
                     $(wildcard $(GENDIR)/$(CURDIR)/*_autoinit.c) \
                     $(wildcard $(GENDIR)/$(CURDIR)/*_getlibbase.c)"
                        .to_owned()
                })
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            result.sources,
            vec![
                "gl_funcs",
                "@AROS_GENMODULE|normal|stackstubs,regcallstubs,autoinit,getlibbase|gl|library|gl.conf"
            ]
        );
    }

    #[test]
    fn encodes_the_rel_lfa_stub_family_without_module_support_objects() {
        let joined = "\
\t@$(GENMODULE) -c $(SRCDIR)/$(CURDIR)/posixc_lfa.conf \
 -d $(GENDIR)/$(CURDIR)/lfa writefiles posixc library\n";
        let result = resolve_generated_linklib_sources(
            "$(LFA_RELLIB_SOURCES:.c=)",
            joined,
            Path::new("compiler/crt/posixc"),
            |_| {
                Some(
                    "$(wildcard $(GENDIR)/$(CURDIR)/lfa/*_relstub.c) \
                     $(wildcard $(GENDIR)/$(CURDIR)/lfa/*_relstubs.c)"
                        .to_owned(),
                )
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            result.sources,
            vec!["@AROS_GENMODULE|rel|stackstubs,regcallstubs|posixc|library|posixc_lfa.conf"]
        );
    }

    #[test]
    fn a_different_wildcard_family_is_not_claimed() {
        let result = resolve_generated_linklib_sources(
            "$(FILES)",
            GL_RULE,
            Path::new("workbench/libs/gl"),
            |_| Some("$(wildcard *.c)".to_owned()),
        )
        .unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn output_directory_must_match_one_writefiles_owner() {
        let error = resolve_generated_linklib_sources(
            "$(FILES)",
            GL_RULE,
            Path::new("workbench/libs/gl"),
            |_| Some("$(wildcard $(GENDIR)/elsewhere/*_stub.c)".to_owned()),
        )
        .unwrap_err();
        assert!(error.contains("found 0 matching writefiles commands"));
    }

    #[test]
    fn normal_and_rel_sources_cannot_be_merged() {
        let error = resolve_generated_linklib_sources(
            "$(FILES)",
            GL_RULE,
            Path::new("workbench/libs/gl"),
            |_| {
                Some(
                    "$(wildcard $(GENDIR)/$(CURDIR)/*_stub.c) \
                     $(wildcard $(GENDIR)/$(CURDIR)/*_relstub.c)"
                        .to_owned(),
                )
            },
        )
        .unwrap_err();
        assert!(error.contains("mixes genmodule output directories or normal/rel variants"));
    }

    #[test]
    fn unresolved_or_globbed_writefiles_fields_cannot_reach_the_marker() {
        for value in ["$(MODULE)", "*.conf", "library?"] {
            let error = valid_marker_field(value, "test field").unwrap_err();
            assert!(error.contains("cannot be encoded"), "{error}");
        }

        assert_eq!(
            marker_config(
                "$(SRCDIR)/compiler/crt/stdc/stdc.conf",
                Path::new("workbench/libs/gl")
            )
            .unwrap(),
            "${CMAKE_SOURCE_DIR}/compiler/crt/stdc/stdc.conf"
        );
    }
}
