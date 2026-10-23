//! Literal syntax transport for an independently authorized GNU Make query.
//!
//! This renderer does not select process arguments, incoming environment,
//! runtime, ownership or execution authority. In particular a list of exported
//! Source values is not a substitute for GNU's original process context.
//! The caller must establish that the selected variable scope reproduces the
//! original effect; special engine variables can alter Make's own semantics.
//! No command or variable value is interpolated into Make syntax. They travel
//! through private literal files, with one appended newline removed by GNU's
//! `file` reader. The result is explicitly GNU-folded, not raw process stdout.

use std::{
    collections::BTreeSet,
    fmt::Write,
    path::{Path, PathBuf},
};

const MAX_QUERY_BYTES: usize = 1024 * 1024;
const MAX_VARIABLES: usize = 4096;

/// Private query-phase target; not an original Source goal or phase claim.
pub const QUERY_GOAL: &str = "__aros_private_shell_query";

/// A frozen value and/or independently selected export directive.
pub struct LiteralQueryVariable<'a> {
    pub name: &'a str,
    /// `None` preserves GNU's value rather than replacing it with empty text.
    pub value: Option<&'a str>,
    /// `None` preserves GNU's export selection rather than unexporting it.
    pub export: Option<bool>,
}

/// A separately established Source-file definition, not an override binding.
///
/// Keeping this distinct matters for engine-managed variables and command-line
/// precedence. This transports selected state; it does not infer that state or
/// prove GNU engine/export equivalence from an ordinary export map.
pub struct LiteralSourceQueryVariable<'a> {
    pub name: &'a str,
    /// A literal simple file assignment, or `None` for ordinary `undefine`.
    /// A command-line binding therefore keeps its original GNU precedence.
    pub value: Option<&'a str>,
    /// `None` emits no directive and preserves GNU's current export selection,
    /// including an export flag inherited by a later ordinary file assignment.
    pub export: Option<bool>,
}

/// A private file bundle, not an executable or authenticated observation.
pub struct GnuShellQuery {
    directory: PathBuf,
    files: Vec<(String, Vec<u8>)>,
}

impl GnuShellQuery {
    /// Render a bounded query without interpreting any payload as Make syntax.
    ///
    /// The directory must be independently selected, private, absolute and
    /// syntax-safe and fresh for this one query. The caller writes each payload
    /// with exclusive creation, verifies its retained bytes and requires both
    /// output paths to be absent (including symlinks) before execution. After
    /// bounded/cancellable execution it validates nonlinked regular output files
    /// and reads them with a size limit before allocation. Decoder limits do
    /// not bound Make's own buffering or the growth of output files.
    /// No filesystem access or process launch occurs here.
    ///
    /// # Errors
    /// Refuses unsafe coordinates/names, duplicates, NUL and excess size.
    pub fn render(
        directory: &Path,
        command: &str,
        variables: &[LiteralQueryVariable<'_>],
    ) -> Result<Self, String> {
        Self::render_with_source_bindings(directory, command, variables, &[])
    }

    /// Render explicit ordinary Source bindings without promoting their origin.
    ///
    /// Source bindings are emitted in supplied order before frozen overrides.
    /// Names must be disjoint across both selections; an export directive for a
    /// Source binding belongs on that binding. Values remain literal file data.
    /// This does not reconstruct historical engine mutations or grant execution.
    ///
    /// # Errors
    /// Refuses the same invalid inputs as [`Self::render`], including duplicate
    /// names or combined bounds exhausted across both selections.
    pub fn render_with_source_bindings(
        directory: &Path,
        command: &str,
        variables: &[LiteralQueryVariable<'_>],
        source: &[LiteralSourceQueryVariable<'_>],
    ) -> Result<Self, String> {
        let root = directory
            .to_str()
            .ok_or("GNU query directory is not UTF-8")?;
        if !directory.is_absolute()
            || root.len() > 4096
            || root
                .bytes()
                .any(|byte| !byte.is_ascii_alphanumeric() && !b"/_-.".contains(&byte))
            || directory.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            })
        {
            return Err("GNU query directory is not an admitted private coordinate".into());
        }
        if variables.len().saturating_add(source.len()) > MAX_VARIABLES {
            return Err("GNU query variable count exceeds its limit".into());
        }
        let mut names = BTreeSet::new();
        let mut size = command
            .len()
            .checked_add(1)
            .ok_or("GNU query size overflow")?;
        if variables
            .iter()
            .any(|variable| variable.value.is_none() && variable.export.is_none())
        {
            return Err("GNU query variable name is invalid or repeated".into());
        }
        for (name, value) in variables
            .iter()
            .map(|variable| (variable.name, variable.value))
            .chain(
                source
                    .iter()
                    .map(|variable| (variable.name, variable.value)),
            )
        {
            if name.is_empty()
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
                || !names.insert(name)
            {
                return Err("GNU query variable name is invalid or repeated".into());
            }
            size = size
                .checked_add(name.len())
                .and_then(|size| size.checked_add(value.map_or(0, str::len)))
                .and_then(|size| size.checked_add(1))
                .ok_or("GNU query size overflow")?;
            if value.is_some_and(|value| value.contains('\0')) {
                return Err("GNU query literal contains a NUL delimiter".into());
            }
        }
        if command.contains('\0') || size > MAX_QUERY_BYTES {
            return Err("GNU query command exceeds its admitted text domain".into());
        }
        let mut files = Vec::with_capacity(variables.len() + source.len() + 2);
        let mut syntax = String::new();
        for (index, variable) in source.iter().enumerate() {
            if let Some(value) = variable.value {
                let filename = format!("source-value-{index}.txt");
                writeln!(
                    syntax,
                    "{} := $(file <{})",
                    variable.name,
                    directory.join(&filename).display()
                )
                .map_err(|_| "GNU query syntax formatting failed")?;
                files.push((filename, literal_file(value)));
            } else {
                writeln!(syntax, "undefine {}", variable.name)
                    .map_err(|_| "GNU query syntax formatting failed")?;
            }
            if let Some(export) = variable.export {
                syntax.push_str(if export { "export " } else { "unexport " });
                syntax.push_str(variable.name);
                syntax.push('\n');
            }
        }
        for (index, variable) in variables.iter().enumerate() {
            if let Some(value) = variable.value {
                let filename = format!("value-{index}.txt");
                let path = directory.join(&filename);
                writeln!(
                    syntax,
                    "override {} := $(file <{})",
                    variable.name,
                    path.display()
                )
                .map_err(|_| "GNU query syntax formatting failed")?;
                files.push((filename, literal_file(value)));
            }
            if let Some(export) = variable.export {
                syntax.push_str(if export { "export " } else { "unexport " });
                syntax.push_str(variable.name);
                syntax.push('\n');
            }
        }
        let command_path = directory.join("command.txt");
        let value_path = directory.join("result.txt");
        let status_path = directory.join("status.txt");
        write!(
            syntax,
            "$(file >{},$(shell $(file <{})))\n$(file >{},$(.SHELLSTATUS))\n",
            value_path.display(),
            command_path.display(),
            status_path.display(),
        )
        .map_err(|_| "GNU query syntax formatting failed")?;
        // The caller must explicitly select this private query phase rather
        // than pretend the bundle is the original Source goal invocation.
        write!(syntax, ".PHONY: {QUERY_GOAL}\n{QUERY_GOAL}:\n")
            .map_err(|_| "GNU query syntax formatting failed")?;
        let retained_size = files
            .iter()
            .try_fold(syntax.len() + command.len() + 1, |size, (_, bytes)| {
                size.checked_add(bytes.len())
            })
            .ok_or("GNU query bundle size overflow")?;
        if retained_size > MAX_QUERY_BYTES {
            return Err("GNU query bundle exceeds its limit".into());
        }
        files.push(("command.txt".into(), literal_file(command)));
        files.push(("query.mk".into(), syntax.into_bytes()));
        Ok(Self {
            directory: directory.to_owned(),
            files,
        })
    }

    /// Relative filenames and exact bytes; create exclusively under the bound
    /// directory, never overwrite existing results or other query bundles.
    #[must_use]
    pub fn files(&self) -> &[(String, Vec<u8>)] {
        &self.files
    }

    #[must_use]
    pub fn makefile(&self) -> PathBuf {
        self.directory.join("query.mk")
    }

    #[must_use]
    pub fn result_file(&self) -> PathBuf {
        self.directory.join("result.txt")
    }

    #[must_use]
    pub fn status_file(&self) -> PathBuf {
        self.directory.join("status.txt")
    }
}

fn literal_file(value: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(value.len() + 1);
    bytes.extend_from_slice(value.as_bytes());
    bytes.push(b'\n');
    bytes
}

/// A value already folded by GNU `$(shell ...)`, never raw stdout.
#[derive(PartialEq, Eq)]
pub struct GnuShellQueryResult {
    pub value: String,
    pub status: u8,
}

impl std::fmt::Debug for GnuShellQueryResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GnuShellQueryResult")
            .field("value_bytes", &self.value.len())
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

impl GnuShellQueryResult {
    /// Decode the exact files written by GNU `$(file >...)`.
    /// Authentication, exclusive paths and process completion are caller duties.
    /// # Errors
    /// Refuses absent framing, noncanonical status, NUL, non-UTF-8 or excess size.
    pub fn decode(value: &[u8], status: &[u8]) -> Result<Self, String> {
        let value = value
            .strip_suffix(b"\n")
            .ok_or("GNU query value lacks file framing")?;
        if value.len() > MAX_QUERY_BYTES || value.contains(&0) || value.contains(&b'\n') {
            return Err("GNU query value exceeds its admitted text domain".into());
        }
        let status = status
            .strip_suffix(b"\n")
            .ok_or("GNU query status lacks file framing")?;
        if status.is_empty()
            || status.len() > 3
            || !status.iter().all(u8::is_ascii_digit)
            || status.len() > 1 && status[0] == b'0'
        {
            return Err("GNU query status is not canonical".into());
        }
        let status = std::str::from_utf8(status)
            .map_err(|_| "GNU query status is not UTF-8")?
            .parse::<u8>()
            .map_err(|_| "GNU query status is out of range")?;
        let value = std::str::from_utf8(value)
            .map_err(|_| "GNU query value is not UTF-8")?
            .to_owned();
        Ok(Self { value, status })
    }
}

#[cfg(test)]
mod tests;
