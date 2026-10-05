//! Bounded parsing of configured MetaMake project metadata.
//!
//! Inputs are explicit snapshots, not files from an existing build tree or
//! ambient environment. This module does not configure a project, discover
//! filesystem inputs, run a generator, or establish native graph admission.
//! The caller must prove the snapshots, configured substitutions and complete
//! file inventory before using the resulting owner graph as evidence.

use std::collections::{BTreeMap, BTreeSet};

const MAX_CONFIG_BYTES: usize = 64 * 1024;
const MAX_GLOBAL_BYTES: usize = 8 * 1024 * 1024;
const MAX_BINDINGS: usize = 16_384;
const MAX_VALUE_BYTES: usize = 4095;
const MAX_RETAINED_BYTES: usize = 16 * 1024 * 1024;
// project.c uses fgets(line, sizeof(line), ...) with a 256-byte buffer and
// unconditionally drops the last byte. Refuse chunked or unterminated lines
// rather than silently interpreting them as ordinary logical lines.
const MAX_PHYSICAL_LINE_BYTES: usize = 255;

/// The selected configured project; command strings are retained, never run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfiguredProject {
    pub name: String,
    pub top: Option<String>,
    pub default_makefile: String,
    pub default_target: String,
    pub ignored_directories: BTreeSet<String>,
    pub added_makefiles: Vec<String>,
    pub global_variable_files: Vec<String>,
    pub generator_inputs: BTreeSet<String>,
    pub generator_command: Option<String>,
    pub global_generator_command: Option<String>,
    /// Effective reference command, retained as text and never executed.
    /// Includes the built-in default when no `maketool` directive is present.
    pub make_command: Option<String>,
    /// Unknown configuration keys are project variables, lowercased by mmake.
    /// Their values are not substituted while parsing the configuration.
    pub variables: BTreeMap<String, String>,
}

impl ConfiguredProject {
    /// Parse one explicitly named project from configured input text.
    ///
    /// # Errors
    /// Refuses ambiguous projects, modified default-project inheritance,
    /// unresolved configure tokens, C-buffer chunking, unsafe discovery names
    /// and syntax outside the bounded reference-compatible subset.
    pub fn parse(text: &str, project: &str) -> Result<Self, String> {
        if text.len() > MAX_CONFIG_BYTES || !variable_name(project) {
            return Err("invalid or oversized MetaMake project configuration".into());
        }
        let mut selected = Self {
            name: project.into(),
            top: None,
            default_makefile: "Makefile".into(),
            default_target: "all".into(),
            ignored_directories: BTreeSet::new(),
            added_makefiles: Vec::new(),
            global_variable_files: Vec::new(),
            generator_inputs: BTreeSet::new(),
            generator_command: None,
            global_generator_command: None,
            make_command: Some(
                "make \"TOP=$(TOP)\" \"SRCDIR=$(SRCDIR)\" \"CURDIR=$(CURDIR)\"".into(),
            ),
            variables: BTreeMap::new(),
        };
        let mut section = None;
        let mut found = false;
        for (index, physical) in text.split_inclusive('\n').enumerate() {
            let line = physical_line(physical, "project configuration", index + 1)?;
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.starts_with('[') {
                let name = line
                    .strip_prefix('[')
                    .and_then(|s| s.strip_suffix(']'))
                    .ok_or("unsupported MetaMake section syntax")?;
                if !variable_name(name) {
                    return Err("invalid MetaMake project name".into());
                }
                if name == "default" {
                    return Err("explicit default-project inheritance is not modeled".into());
                }
                section = Some(name);
                if name == project {
                    if found {
                        return Err("duplicate selected MetaMake project".into());
                    }
                    found = true;
                }
                continue;
            }
            if section.is_none() {
                return Err(
                    "default-project directives require an explicit inheritance proof".into(),
                );
            }
            if section != Some(project) {
                continue;
            }
            if !line.is_ascii() || line.contains(['\r', '\0', '@']) {
                return Err("nonliteral or unconfigured MetaMake directive".into());
            }
            let line = line.trim_start_matches(|c: char| c.is_ascii_whitespace());
            let end = line
                .find(|c: char| c.is_ascii_whitespace())
                .unwrap_or(line.len());
            let key = line[..end].to_ascii_lowercase();
            let value = line[end..].trim_start_matches(|c: char| c.is_ascii_whitespace());
            match key.as_str() {
                "defaultmakefilename" => {
                    single_component(value)?;
                    selected.default_makefile = value.into();
                }
                "defaulttarget" => selected.default_target = value.into(),
                "top" => selected.top = Some(value.into()),
                "ignoredir" => {
                    single_component(value)?;
                    selected.ignored_directories.insert(value.into());
                }
                "add" => {
                    relative_path(value)?;
                    selected.added_makefiles.push(value.into());
                }
                "globalvarfile" => selected.global_variable_files.push(value.into()),
                "genmakefiledeps" => {
                    // getargs tokenizes before substitution; quotes have a
                    // reference bug and are deliberately not approximated.
                    if value.contains('"') {
                        return Err("quoted GenMF input tokens are not modeled".into());
                    }
                    selected
                        .generator_inputs
                        .extend(value.split_ascii_whitespace().map(str::to_owned));
                }
                "genmakefilescript" => selected.generator_command = Some(value.into()),
                "genglobalvarfile" => selected.global_generator_command = Some(value.into()),
                "maketool" => selected.make_command = Some(value.into()),
                _ => {
                    if !variable_name(&key) {
                        return Err("unsupported MetaMake project variable name".into());
                    }
                    selected.variables.insert(key, value.into());
                }
            }
        }
        if !found {
            return Err(format!("MetaMake project {project:?} is absent"));
        }
        Ok(selected)
    }
}

/// One sequential source assignment, preserving its provenance for the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalAssignment {
    pub file: String,
    pub line: usize,
    pub name: String,
    pub value: String,
}

/// Explicit variables plus their ordered assignment history. No environment
/// fallback or recursive GNU Make evaluation is performed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectGlobals {
    pub values: BTreeMap<String, String>,
    pub assignments: Vec<GlobalAssignment>,
}

impl ProjectGlobals {
    /// Read global-variable snapshots in exact configured order, resolving
    /// each value against the variables known at that point, once only.
    ///
    /// # Errors
    /// Refuses unresolved variables, unsupported syntax, C-buffer chunking,
    /// missing final newlines and resource excess. Initial values still need
    /// source/profile provenance from the caller; supplying a value alone is
    /// not evidence that the selected build uses that value.
    pub fn parse(
        initial: &BTreeMap<String, String>,
        files: &[(String, String)],
    ) -> Result<Self, String> {
        let mut retained = 0usize;
        let mut work = 0usize;
        if initial.len() > MAX_BINDINGS || files.len() > 256 {
            return Err("MetaMake globals exceed entry limit".into());
        }
        for (name, value) in initial {
            if !variable_name(name)
                || value.len() > MAX_VALUE_BYTES
                || value.contains(['\0', '\r', '\n'])
            {
                return Err("invalid initial MetaMake global".into());
            }
            charge(&mut retained, name.len() + value.len(), MAX_RETAINED_BYTES)?;
        }
        let mut projection = Self {
            values: initial.clone(),
            assignments: Vec::new(),
        };
        let mut input_bytes = 0usize;
        for (path, text) in files {
            if path.len() > MAX_VALUE_BYTES {
                return Err("MetaMake global provenance path exceeds limit".into());
            }
            charge(&mut input_bytes, text.len(), MAX_GLOBAL_BYTES)?;
            for (index, physical) in text.split_inclusive('\n').enumerate() {
                let line = physical_line(physical, path, index + 1)?;
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let line = line.trim_start_matches(|c: char| c.is_ascii_whitespace());
                let end = line
                    .find(|c: char| c.is_ascii_whitespace() || matches!(c, ':' | '='))
                    .unwrap_or(line.len());
                let name = &line[..end];
                if !variable_name(name) {
                    return Err(format!(
                        "unsupported global assignment in {path}:{}",
                        index + 1
                    ));
                }
                let value = line[end..].trim_start_matches(|c: char| {
                    c.is_ascii_whitespace() || matches!(c, ':' | '=')
                });
                let value = value.split('#').next().unwrap_or_default();
                let value = substitute_globals(value, &projection.values)?;
                charge(&mut work, value.len() + line.len(), MAX_GLOBAL_BYTES)?;
                charge(
                    &mut retained,
                    path.len() + name.len() + value.len() * 2,
                    MAX_RETAINED_BYTES,
                )?;
                if projection.assignments.len() >= MAX_BINDINGS {
                    return Err("MetaMake global assignment count exceeds limit".into());
                }
                projection.assignments.push(GlobalAssignment {
                    file: path.clone(),
                    line: index + 1,
                    name: name.into(),
                    value: value.clone(),
                });
                projection.values.insert(name.into(), value);
                if projection.values.len() > MAX_BINDINGS {
                    return Err("MetaMake global binding count exceeds limit".into());
                }
            }
        }
        Ok(projection)
    }
}

/// Single-pass MetaMake substitution, without environment fallback.
///
/// # Errors
/// Refuses unsupported dollar syntax, absent bindings and oversized output.
pub fn substitute_globals(
    text: &str,
    variables: &BTreeMap<String, String>,
) -> Result<String, String> {
    if text.len() > MAX_VALUE_BYTES || text.contains(['\0', '\r', '\n']) {
        return Err("invalid MetaMake substitution input".into());
    }
    let mut output = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('$') {
        append(&mut output, &rest[..start])?;
        let reference = &rest[start..];
        let body = reference
            .strip_prefix("$(")
            .ok_or("unsupported MetaMake dollar syntax")?;
        let close = body.find(')').ok_or("unterminated MetaMake variable")?;
        let name = &body[..close];
        if !variable_name(name) {
            return Err("unsupported MetaMake variable name".into());
        }
        let value = variables
            .get(name)
            .ok_or_else(|| format!("unbound MetaMake project variable {name}"))?;
        if value.contains(['\0', '\r', '\n']) {
            return Err("invalid MetaMake variable value".into());
        }
        append(&mut output, value)?;
        rest = &body[close + 1..];
    }
    append(&mut output, rest)?;
    Ok(output)
}

fn append(output: &mut String, value: &str) -> Result<(), String> {
    if output
        .len()
        .checked_add(value.len())
        .is_none_or(|n| n > MAX_VALUE_BYTES)
    {
        return Err("MetaMake substitution exceeds reference buffer".into());
    }
    output.push_str(value);
    Ok(())
}

fn physical_line<'a>(line: &'a str, path: &str, number: usize) -> Result<&'a str, String> {
    if line.len() > MAX_PHYSICAL_LINE_BYTES || !line.ends_with('\n') || line.contains(['\0', '\r'])
    {
        return Err(format!(
            "unsupported MetaMake physical line in {path}:{number}"
        ));
    }
    Ok(&line[..line.len() - 1])
}

fn variable_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() < 256
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

fn single_component(value: &str) -> Result<(), String> {
    if !variable_name(value) || matches!(value, "." | "..") {
        return Err("MetaMake discovery requires a literal basename".into());
    }
    Ok(())
}

fn relative_path(value: &str) -> Result<(), String> {
    if value.len() > MAX_VALUE_BYTES {
        return Err("MetaMake added path exceeds limit".into());
    }
    for component in value.split('/') {
        single_component(component)?;
    }
    Ok(())
}

fn charge(current: &mut usize, amount: usize, limit: usize) -> Result<(), String> {
    *current = current
        .checked_add(amount)
        .filter(|n| *n <= limit)
        .ok_or("MetaMake project resource limit exceeded")?;
    Ok(())
}

/// Entry kinds from a caller's complete, source-identity-bound inventory.
/// Links are not interpreted as files or traversable directories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InventoryEntry {
    Directory,
    RegularFile,
    Link,
    Other,
}

/// Logical generated owner and actual immutable input path. A `.src` input
/// wins over a same-directory direct makefile, independently of file dates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnerInput {
    pub owner: String,
    pub input: String,
    pub generated: bool,
}

/// Plan default source makefiles from a complete inventory, without reading
/// or executing any source. No filesystem completeness claim is made here.
///
/// # Errors
/// Refuses absent directory provenance, unsafe entries, unresolved top
/// selection and explicit `add` directives. Classic `add` searches the build
/// tree after regeneration; a source-only inventory cannot prove its inputs.
/// A caller must handle that separate closure, never silently omit it.
pub fn plan_source_inputs(
    project: &ConfiguredProject,
    inventory: &BTreeMap<String, InventoryEntry>,
) -> Result<Vec<OwnerInput>, String> {
    if inventory.len() > 200_000 {
        return Err("MetaMake source inventory exceeds entry limit".into());
    }
    if project.top.as_deref().is_some_and(|top| top != ".") {
        return Err("MetaMake top selection requires a separate root identity proof".into());
    }
    if !project.added_makefiles.is_empty() {
        return Err("MetaMake added makefiles require build-tree input closure".into());
    }
    single_component(&project.default_makefile)?;
    let source_basename = format!("{}.src", project.default_makefile);
    let mut bytes = 0usize;
    let mut owners = BTreeMap::<String, OwnerInput>::new();
    for (path, kind) in inventory {
        relative_path(path)?;
        charge(&mut bytes, path.len(), MAX_RETAINED_BYTES)?;
        let components: Vec<_> = path.split('/').collect();
        // Directory names, not file basenames, control ignoredir. A regular
        // makefile named exactly like an ignored directory remains visible.
        let directory_count = components.len() - usize::from(*kind != InventoryEntry::Directory);
        if components[..directory_count]
            .iter()
            .any(|name| project.ignored_directories.contains(*name))
        {
            continue;
        }
        let mut parent = String::new();
        for component in &components[..components.len() - 1] {
            if !parent.is_empty() {
                parent.push('/');
            }
            parent.push_str(component);
            if inventory.get(&parent) != Some(&InventoryEntry::Directory) {
                return Err(format!(
                    "MetaMake inventory lacks regular directory provenance for {parent}"
                ));
            }
        }
        if matches!(kind, InventoryEntry::Link | InventoryEntry::Other) {
            return Err(format!(
                "unsupported MetaMake source inventory entry {path}"
            ));
        }
        let basename = components.last().ok_or("empty inventory path")?;
        if *kind != InventoryEntry::RegularFile
            || (*basename != project.default_makefile && *basename != source_basename)
        {
            continue;
        }
        let generated = *basename == source_basename;
        if generated && project.generator_command.is_none() {
            return Err("MetaMake .src input requires a configured generator contract".into());
        }
        let owner = if generated {
            path.strip_suffix(".src").ok_or("source suffix lost")?
        } else {
            path
        };
        if generated || !owners.contains_key(owner) {
            owners.insert(
                owner.into(),
                OwnerInput {
                    owner: owner.into(),
                    input: path.clone(),
                    generated,
                },
            );
        }
        if owners.len() > 20_000 {
            return Err("MetaMake owner input count exceeds limit".into());
        }
    }
    Ok(owners.into_values().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_project_defaults_match_reference_without_executing_make() {
        let project = ConfiguredProject::parse("[probe]\n", "probe").unwrap();
        assert_eq!(project.default_makefile, "Makefile");
        assert_eq!(project.default_target, "all");
        assert_eq!(
            project.make_command.as_deref(),
            Some("make \"TOP=$(TOP)\" \"SRCDIR=$(SRCDIR)\" \"CURDIR=$(CURDIR)\"")
        );
        assert!(project.generator_command.is_none());
        assert!(project.global_generator_command.is_none());
    }

    #[test]
    fn configured_directives_are_retained_without_running_commands() {
        let project = ConfiguredProject::parse(
            "[probe]\ndefaultmakefilename mmakefile\nignoredir .git\nadd special/rules.src\nglobalvarfile $(TOP)/project.vars\ngenmakefiledeps $(SRCDIR)/root.tmpl $(SRCDIR)/import.tmpl\ngenmakefilescript forbidden-command\ngenglobalvarfile forbidden-configure\nmaketool forbidden-make\nCPU arm\n", "probe").unwrap();
        assert_eq!(project.default_makefile, "mmakefile");
        assert_eq!(project.added_makefiles, ["special/rules.src"]);
        assert_eq!(project.variables["cpu"], "arm");
        assert!(!project.variables.contains_key("CPU"));
        assert_eq!(project.generator_inputs.len(), 2);
        assert_eq!(project.make_command.as_deref(), Some("forbidden-make"));
    }

    #[test]
    fn configuration_does_not_hide_unconfigured_or_ambiguous_discovery() {
        for input in [
            "[probe]\nignoredir distfiles@mmake_ignore_dirs@\n",
            "[probe]\nignoredir ../hidden\n",
            "[probe]\nadd ../outside\n",
            "[probe]\nadd /outside\n",
            "[probe]\ndefaultmakefilename $(DYNAMIC)\n",
            "[probe]\ngenmakefiledeps \"quoted input\"\n",
            "[probe]\n[probe]\n",
            "defaultmakefilename custom\n[probe]\n",
            "[default]\n[probe]\n",
        ] {
            assert!(ConfiguredProject::parse(input, "probe").is_err(), "{input}");
        }
    }

    #[test]
    fn global_files_resolve_sequentially_and_preserve_source_assignment_history() {
        let actual = ProjectGlobals::parse(
            &BTreeMap::new(),
            &[
                ("host.cfg".into(), "CPU arm\nALIAS := $(CPU)\n".into()),
                (
                    "target.cfg".into(),
                    "CPU=riscv\nARCH := $(CPU)\nEMPTY :=\nCOMMENT := value #ignored\n".into(),
                ),
            ],
        )
        .unwrap();
        assert_eq!(actual.values["ALIAS"], "arm");
        assert_eq!(actual.values["ARCH"], "riscv");
        assert_eq!(actual.values["EMPTY"], "");
        assert_eq!(actual.values["COMMENT"], "value ");
        assert_eq!(actual.assignments[0].file, "host.cfg");
        assert_eq!(actual.assignments[3].line, 2);
    }

    #[test]
    fn missing_globals_never_use_environment_or_make_local_values() {
        assert!(
            ProjectGlobals::parse(&BTreeMap::new(), &[("x".into(), "X := $(PATH)\n".into())])
                .is_err()
        );
        let variables = BTreeMap::from([("VALUE".into(), "$(MISSING)".into())]);
        assert_eq!(
            substitute_globals("$(VALUE)", &variables).unwrap(),
            "$(MISSING)"
        );
    }

    #[test]
    fn c_buffer_chunks_and_unterminated_lines_are_refused_not_normalized() {
        for value in ["CPU arm", "CPU arm\r\n", " CPU arm\0\n", " \n"] {
            assert!(
                ProjectGlobals::parse(&BTreeMap::new(), &[("x".into(), value.into())]).is_err()
            );
        }
        let long = format!("X {}\n", "a".repeat(254));
        assert!(ProjectGlobals::parse(&BTreeMap::new(), &[("x".into(), long)]).is_err());
        assert!(ConfiguredProject::parse("[probe]\nignoredir .git", "probe").is_err());
    }

    #[test]
    fn unsupported_make_expressions_are_not_approximated() {
        for input in [
            "$(strip VALUE)",
            "${VALUE}",
            "$V",
            "$$(VALUE)",
            "$(VALUE",
            "$(nested$(VALUE))",
        ] {
            assert!(substitute_globals(input, &BTreeMap::new()).is_err());
        }
        let variables = BTreeMap::from([("X".into(), "x".repeat(MAX_VALUE_BYTES))]);
        assert!(substitute_globals("prefix$(X)", &variables).is_err());
    }

    #[test]
    fn discovery_uses_configured_basename_and_prefers_source_over_generated_output() {
        use InventoryEntry::{Directory, RegularFile};
        let project = ConfiguredProject::parse("[probe]\ndefaultmakefilename rules\ngenmakefilescript explicit-generator\nignoredir ignored\n", "probe").unwrap();
        let inventory = BTreeMap::from([
            ("a".into(), Directory),
            ("a/rules".into(), RegularFile),
            ("a/rules.src".into(), RegularFile),
            ("a/mmakefile".into(), RegularFile),
            ("ignored".into(), Directory),
            ("ignored/rules.src".into(), RegularFile),
            ("rules".into(), RegularFile),
        ]);
        let actual = plan_source_inputs(&project, &inventory).unwrap();
        assert_eq!(
            actual,
            [
                OwnerInput {
                    owner: "a/rules".into(),
                    input: "a/rules.src".into(),
                    generated: true
                },
                OwnerInput {
                    owner: "rules".into(),
                    input: "rules".into(),
                    generated: false
                },
            ]
        );
    }

    #[test]
    fn discovery_preserves_the_build_tree_add_obligation() {
        let project = ConfiguredProject::parse("[probe]\nadd custom/rules.src\n", "probe").unwrap();
        assert!(plan_source_inputs(&project, &BTreeMap::new())
            .unwrap_err()
            .contains("build-tree"));
    }

    #[test]
    fn discovery_requires_regular_directory_provenance_and_an_explicit_generator() {
        let project = ConfiguredProject::parse("[probe]\n", "probe").unwrap();
        for inventory in [
            BTreeMap::from([("deep/Makefile".into(), InventoryEntry::RegularFile)]),
            BTreeMap::from([("deep".into(), InventoryEntry::Link)]),
            BTreeMap::from([("Makefile".into(), InventoryEntry::Link)]),
            BTreeMap::from([("Makefile.src".into(), InventoryEntry::RegularFile)]),
        ] {
            assert!(plan_source_inputs(&project, &inventory).is_err());
        }
    }
}
