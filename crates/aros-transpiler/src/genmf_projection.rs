//! Bounded, deterministic text expansion for the GenMF subset used by AROS.
//!
//! This prototype reads GenMF template files and source text only. It does not
//! execute Make, a shell, a configure step, or a CMake graph, and makes no
//! producer, owner, or admission claim. Unsupported syntax is returned as an
//! error instead of being interpreted approximately.

use std::collections::{BTreeMap, HashSet};
use std::error::Error as StdError;
use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Keep origins only for MetaMake directive starts, without changing text
    /// expansion or any byte/work budget. Native graph admission uses this.
    pub metadata_provenance_only: bool,
    pub max_source_bytes: usize,
    pub max_template_bytes: usize,
    pub max_template_files: usize,
    pub max_definitions: usize,
    pub max_lines: usize,
    pub max_output_bytes: usize,
    pub max_work_bytes: usize,
    pub max_include_depth: usize,
    pub max_macro_depth: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            metadata_provenance_only: false,
            max_source_bytes: 1024 * 1024,
            max_template_bytes: 8 * 1024 * 1024,
            max_template_files: 256,
            max_definitions: 16_384,
            max_lines: 65_536,
            max_output_bytes: 16 * 1024 * 1024,
            max_work_bytes: 64 * 1024 * 1024,
            max_include_depth: 32,
            max_macro_depth: 32,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenmfError {
    pub file: Option<PathBuf>,
    pub line: Option<usize>,
    pub detail: String,
}

impl fmt::Display for GenmfError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(file) = &self.file {
            write!(formatter, "{}", file.display())?;
            if let Some(line) = self.line {
                write!(formatter, ":{line}")?;
            }
            write!(formatter, ": ")?;
        }
        formatter.write_str(&self.detail)
    }
}

impl StdError for GenmfError {}

/// Text plus the exact recursively imported template inputs used to render it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpandedText {
    pub text: String,
    pub template_snapshots: Vec<TemplateSnapshot>,
    /// Exact emitted-text provenance, in output order. Byte ranges are
    /// half-open UTF-8 offsets into `text`; output lines are one-based.
    pub provenance: Vec<ExpandedLineProvenance>,
}

/// Provenance for one physical output-line fragment.
///
/// Usually a span is one complete emitted line. It can be only a fragment
/// when the source's final line has no newline and synthetic output follows.
/// Repeated identical text remains distinguishable by byte range, source line,
/// and macro invocation stack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpandedLineProvenance {
    pub output_start_byte: usize,
    pub output_end_byte: usize,
    pub output_line: usize,
    /// Physical input line that emitted this text; absent only for synthetic
    /// separators such as the newline before an implicit `common` expansion.
    pub source_path: Option<PathBuf>,
    pub source_line: Option<usize>,
    /// Definition identity when the emitted line came from a template body.
    /// `source_path`/`source_line` then identify the exact body line.
    pub template_path: Option<PathBuf>,
    pub template_definition_line: Option<usize>,
    /// Outermost-to-innermost macro expansion frames.
    pub macro_stack: Vec<MacroExpansionFrame>,
    /// The top-level invocation in the source file, if this expansion began
    /// there. Automatically appended `common` has no such invocation.
    pub top_level_source_invocation: Option<SourceLineLocation>,
}

/// File and one-based physical line for a GenMF invocation or emitted line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceLineLocation {
    pub path: PathBuf,
    pub line: usize,
}

/// One exact frame in a recursively expanded GenMF call stack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MacroExpansionFrame {
    pub name: String,
    pub template_path: PathBuf,
    pub definition_line: usize,
    /// `None` only for an automatically appended template such as `common`.
    pub invocation: Option<SourceLineLocation>,
    /// Values as parsed from this call, preserving omitted versus explicit
    /// empty values and applying GenMF's quote removal.
    pub arguments: BTreeMap<String, Option<String>>,
    /// Effective values used by substitutions after template defaults.
    pub resolved_arguments: BTreeMap<String, String>,
}

/// Raw bytes and canonical path for one loaded template file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateSnapshot {
    pub path: PathBuf,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
struct Argument {
    default: Option<String>,
    required: bool,
    multi: bool,
}

#[derive(Clone, Debug)]
struct Template {
    arguments: BTreeMap<String, Argument>,
    multi_argument: Option<String>,
    body: Vec<String>,
    source: PathBuf,
    line: usize,
    body_start_line: usize,
    byte_size: usize,
}

#[derive(Default)]
struct Budget {
    template_bytes: usize,
    template_files: usize,
    definitions: usize,
    work_bytes: usize,
    output_bytes: usize,
}

impl Budget {
    fn charge_work(&mut self, amount: usize, limits: Limits) -> Result<(), GenmfError> {
        let Some(next) = self.work_bytes.checked_add(amount) else {
            return Err(error(None, None, "GenMF work accounting overflow"));
        };
        if next > limits.max_work_bytes {
            return Err(error(None, None, "GenMF work budget exceeded"));
        }
        self.work_bytes = next;
        Ok(())
    }

    fn charge_output(&mut self, amount: usize, limits: Limits) -> Result<(), GenmfError> {
        let Some(next) = self.output_bytes.checked_add(amount) else {
            return Err(error(None, None, "GenMF output accounting overflow"));
        };
        if next > limits.max_output_bytes {
            return Err(error(None, None, "GenMF output budget exceeded"));
        }
        self.output_bytes = next;
        Ok(())
    }
}

struct TemplateLoader {
    root: PathBuf,
    limits: Limits,
    budget: Budget,
    templates: BTreeMap<String, Template>,
    included: HashSet<PathBuf>,
    snapshots: Vec<TemplateSnapshot>,
}

/// Expands `source` with the template file and its relative `%include`s.
///
/// `source` is already decoded text. [`expand_files`] reads AROS text as
/// ISO-8859-15, which is the encoding used by GenMF's list-file mode.
///
/// # Errors
/// Refuses unsafe or unreadable template imports, invalid template/argument
/// syntax, recursive calls and resource excess. This is text expansion only.
pub fn expand_text(
    source: &str,
    source_file: &Path,
    template_file: &Path,
    limits: Limits,
) -> Result<ExpandedText, GenmfError> {
    if source.len() > limits.max_source_bytes {
        return Err(error(
            Some(source_file.to_path_buf()),
            None,
            "source exceeds byte budget",
        ));
    }
    // Python text-mode input uses universal-newline translation for both
    // source files and templates. Keep the raw snapshot bytes separately.
    let normalized_source = normalize_newlines(source);
    let physical_lines = split_lines(&normalized_source);
    if physical_lines.len() > limits.max_lines {
        return Err(error(
            Some(source_file.to_path_buf()),
            None,
            "source exceeds line budget",
        ));
    }

    let checked_template = reject_symlink_components(template_file)?;
    let canonical_template = checked_template.canonicalize().map_err(|cause| {
        error(
            Some(template_file.to_path_buf()),
            None,
            format!("cannot resolve template file: {cause}"),
        )
    })?;
    let root = canonical_template
        .parent()
        .ok_or_else(|| {
            error(
                Some(canonical_template.clone()),
                None,
                "template has no parent",
            )
        })?
        .to_path_buf();
    let mut loader = TemplateLoader {
        root,
        limits,
        budget: Budget::default(),
        templates: BTreeMap::new(),
        included: HashSet::new(),
        snapshots: Vec::new(),
    };
    loader.read_file(&canonical_template, 0)?;

    let mut runtime = ExpansionRuntime {
        budget: loader.budget,
        output_line: 1,
        ..ExpansionRuntime::default()
    };
    let mut output = String::new();
    let source_references = generate_template_references(&physical_lines, &loader.templates);
    write_lines(
        &physical_lines,
        &source_references,
        &loader.templates,
        &mut runtime,
        limits,
        &mut output,
        Some(source_file),
        1,
        None,
    )?;
    if !runtime.saw_common {
        append_output(
            &mut output,
            "\n",
            &mut runtime,
            limits,
            LineOrigin {
                path: None,
                line: None,
                template: None,
            },
        )?;
        if loader.templates.contains_key("common") {
            expand_template(
                "common",
                "",
                &loader.templates,
                &mut runtime,
                limits,
                &mut output,
                None,
            )?;
        }
    }
    Ok(ExpandedText {
        text: output,
        template_snapshots: loader.snapshots,
        provenance: runtime.provenance,
    })
}

/// Reads a source file as ISO-8859-15 and expands it without executing tools.
///
/// # Errors
/// Refuses an unsafe, changing, missing or oversized regular source snapshot,
/// and propagates all template and expansion errors from [`expand_text`].
pub fn expand_files(
    source_file: &Path,
    template_file: &Path,
    limits: Limits,
) -> Result<ExpandedText, GenmfError> {
    let checked_source = reject_symlink_components(source_file)?;
    let bytes = read_limited(&checked_source, limits.max_source_bytes)?;
    expand_bytes(&bytes, source_file, template_file, limits)
}

/// Expand the caller's captured source bytes, rather than rereading a path
/// after its digest was recorded. Template inputs retain their raw snapshots.
pub(crate) fn expand_bytes(
    bytes: &[u8],
    source_file: &Path,
    template_file: &Path,
    limits: Limits,
) -> Result<ExpandedText, GenmfError> {
    if bytes.len() > limits.max_source_bytes {
        return Err(error(
            Some(source_file.to_path_buf()),
            None,
            "source exceeds byte budget",
        ));
    }
    let source = decode_iso_8859_15(bytes);
    expand_text(&source, source_file, template_file, limits)
}

#[derive(Default)]
struct ExpansionRuntime {
    saw_common: bool,
    stack: Vec<MacroExpansionFrame>,
    budget: Budget,
    provenance: Vec<ExpandedLineProvenance>,
    output_line: usize,
}

#[derive(Clone, Copy)]
struct TemplateDefinitionOrigin<'a> {
    path: &'a Path,
    definition_line: usize,
}

#[derive(Clone, Copy)]
struct LineOrigin<'a> {
    path: Option<&'a Path>,
    line: Option<usize>,
    template: Option<TemplateDefinitionOrigin<'a>>,
}

impl TemplateLoader {
    fn read_file(&mut self, path: &Path, depth: usize) -> Result<(), GenmfError> {
        let checked_path = reject_symlink_components(path)?;
        let canonical = checked_path.canonicalize().map_err(|cause| {
            error(
                Some(path.to_path_buf()),
                None,
                format!("cannot resolve included template: {cause}"),
            )
        })?;
        if !canonical.starts_with(&self.root) {
            return Err(error(
                Some(canonical),
                None,
                "included template resolves outside the template root",
            ));
        }
        if self.included.contains(&canonical) {
            return Ok(());
        }
        if depth > self.limits.max_include_depth {
            return Err(error(
                Some(canonical),
                None,
                "template include depth exceeded",
            ));
        }
        let Some(next_file_count) = self.budget.template_files.checked_add(1) else {
            return Err(error(
                Some(canonical),
                None,
                "template file accounting overflow",
            ));
        };
        if next_file_count > self.limits.max_template_files {
            return Err(error(
                Some(canonical),
                None,
                "template file-count budget exceeded",
            ));
        }
        let remaining = self
            .limits
            .max_template_bytes
            .checked_sub(self.budget.template_bytes)
            .ok_or_else(|| {
                error(
                    Some(canonical.clone()),
                    None,
                    "template byte accounting overflow",
                )
            })?;
        let bytes = read_limited(&canonical, remaining)?;
        let Some(total_bytes) = self.budget.template_bytes.checked_add(bytes.len()) else {
            return Err(error(
                Some(canonical),
                None,
                "template byte accounting overflow",
            ));
        };
        if total_bytes > self.limits.max_template_bytes {
            return Err(error(
                Some(canonical),
                None,
                "template byte budget exceeded",
            ));
        }
        self.budget.template_bytes = total_bytes;
        self.budget.template_files = next_file_count;
        self.included.insert(canonical.clone());
        self.snapshots.push(TemplateSnapshot {
            path: canonical.clone(),
            bytes: bytes.clone(),
        });

        let decoded = decode_iso_8859_15(&bytes);
        let text = normalize_newlines(&decoded);
        let lines = split_lines(&text);
        if lines.len() > self.limits.max_lines {
            return Err(error(Some(canonical), None, "template exceeds line budget"));
        }
        self.budget.charge_work(bytes.len(), self.limits)?;
        self.read_template_lines(&canonical, &lines, depth)
    }

    fn read_template_lines(
        &mut self,
        file: &Path,
        lines: &[String],
        include_depth: usize,
    ) -> Result<(), GenmfError> {
        let mut index = 0usize;
        while index < lines.len() {
            let line = &lines[index];
            self.budget.charge_work(line.len(), self.limits)?;
            if let Some(rest) = line.strip_prefix("%include") {
                if rest.chars().next().is_some_and(is_python_whitespace) {
                    let include_name = unquote_path(trim_python_whitespace(rest));
                    if include_name.is_empty() {
                        return Err(error(
                            Some(file.to_path_buf()),
                            Some(index + 1),
                            "%include requires a file name",
                        ));
                    }
                    let include_path = Path::new(include_name);
                    let resolved = if include_path.is_absolute() {
                        include_path.to_path_buf()
                    } else {
                        file.parent()
                            .unwrap_or_else(|| Path::new("."))
                            .join(include_path)
                    };
                    self.read_file(&resolved, include_depth + 1)?;
                    index += 1;
                    continue;
                }
            }
            if let Some(rest) = line.strip_prefix("%define") {
                if rest.chars().next().is_some_and(is_python_whitespace) {
                    let header_line = index + 1;
                    let (header, last_header_line) =
                        self.join_header_continuations(file, lines, index)?;
                    let header_body = trim_python_whitespace(
                        header
                            .strip_prefix("%define")
                            .expect("matched GenMF definition prefix"),
                    );
                    let (name, arguments, multi_argument) = parse_definition_header(header_body)
                        .map_err(|detail| {
                            error(Some(file.to_path_buf()), Some(header_line), detail)
                        })?;
                    index = last_header_line + 1;
                    let body_start = index;
                    while index < lines.len() && !lines[index].starts_with("%end") {
                        self.budget.charge_work(lines[index].len(), self.limits)?;
                        index += 1;
                    }
                    if index == lines.len() {
                        return Err(error(
                            Some(file.to_path_buf()),
                            Some(header_line),
                            "end of template file inside definition",
                        ));
                    }
                    let body = lines[body_start..index].to_vec();
                    let byte_size = body.iter().try_fold(0usize, |total, body_line| {
                        total.checked_add(body_line.len())
                    });
                    let Some(byte_size) = byte_size else {
                        return Err(error(
                            Some(file.to_path_buf()),
                            Some(header_line),
                            "template body byte accounting overflow",
                        ));
                    };
                    let Some(definition_count) = self.budget.definitions.checked_add(1) else {
                        return Err(error(
                            Some(file.to_path_buf()),
                            Some(header_line),
                            "template definition accounting overflow",
                        ));
                    };
                    if definition_count > self.limits.max_definitions {
                        return Err(error(
                            Some(file.to_path_buf()),
                            Some(header_line),
                            "template definition budget exceeded",
                        ));
                    }
                    self.budget.definitions = definition_count;
                    self.templates.insert(
                        name.clone(),
                        Template {
                            arguments,
                            multi_argument,
                            body,
                            source: file.to_path_buf(),
                            line: header_line,
                            body_start_line: body_start + 1,
                            byte_size,
                        },
                    );
                    index += 1;
                    continue;
                }
            }
            index += 1;
        }
        Ok(())
    }

    fn join_header_continuations(
        &mut self,
        file: &Path,
        lines: &[String],
        start: usize,
    ) -> Result<(String, usize), GenmfError> {
        let mut joined = lines[start].clone();
        let mut index = start;
        while has_genmf_continuation(&joined) {
            if index + 1 >= lines.len() {
                return Err(error(
                    Some(file.to_path_buf()),
                    Some(start + 1),
                    "continued template definition reaches end of file",
                ));
            }
            let new_len = joined
                .len()
                .checked_sub(2)
                .and_then(|length| length.checked_add(lines[index + 1].len()))
                .ok_or_else(|| {
                    error(
                        Some(file.to_path_buf()),
                        Some(start + 1),
                        "continued header byte accounting overflow",
                    )
                })?;
            if new_len > self.limits.max_template_bytes {
                return Err(error(
                    Some(file.to_path_buf()),
                    Some(start + 1),
                    "continued header exceeds template byte budget",
                ));
            }
            joined.truncate(joined.len() - 2);
            joined.push_str(&lines[index + 1]);
            index += 1;
            self.budget.charge_work(lines[index].len(), self.limits)?;
        }
        Ok((joined, index))
    }
}

/// Reject links in the lexical path before canonicalizing it. GenMF deduplicates
/// imported files by lexical `abspath`, while this component snapshots canonical
/// paths; allowing aliases would make those import graphs differ. This is a
/// best-effort check, not race-proof filesystem confinement.
fn reject_symlink_components(path: &Path) -> Result<PathBuf, GenmfError> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|cause| {
                error(
                    Some(path.to_path_buf()),
                    None,
                    format!("cannot resolve current directory: {cause}"),
                )
            })?
            .join(path)
    };
    let mut checked = PathBuf::new();
    for component in absolute.components() {
        use std::path::Component;
        match component {
            Component::Prefix(prefix) => checked.push(prefix.as_os_str()),
            Component::RootDir => checked.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                checked.pop();
            }
            Component::Normal(name) => {
                checked.push(name);
                let metadata = std::fs::symlink_metadata(&checked).map_err(|cause| {
                    error(
                        Some(checked.clone()),
                        None,
                        format!("cannot inspect input path component: {cause}"),
                    )
                })?;
                if metadata.file_type().is_symlink() {
                    return Err(error(
                        Some(checked.clone()),
                        None,
                        "input path contains a symbolic link; alias import semantics are not modeled",
                    ));
                }
            }
        }
    }
    Ok(checked)
}

type DefinitionHeader = (String, BTreeMap<String, Argument>, Option<String>);

fn parse_definition_header(header: &str) -> Result<DefinitionHeader, String> {
    let mut cursor = skip_whitespace(header, 0);
    let (name, after_name) = parse_word(header, cursor, true)
        .ok_or_else(|| "invalid syntax of template name".to_owned())?;
    cursor = after_name;
    if cursor < header.len() && !is_python_whitespace_at(header, cursor) {
        return Err("template name must be followed by whitespace".to_owned());
    }

    let mut arguments = BTreeMap::new();
    while cursor < header.len() {
        cursor = skip_whitespace(header, cursor);
        if cursor >= header.len() {
            break;
        }
        let arg_start = cursor;
        let Some((arg_name, after_arg_name)) = parse_word(header, cursor, true) else {
            return Err(format!(
                "invalid syntax of argument {}: {}",
                arguments.len() + 1,
                &header[arg_start..]
            ));
        };
        cursor = after_arg_name;
        if header.as_bytes().get(cursor) != Some(&b'=') {
            return Err(format!(
                "invalid syntax of argument {}: {}",
                arguments.len() + 1,
                &header[arg_start..]
            ));
        }
        cursor += 1;
        let (raw_default, after_value) = parse_optional_value(header, cursor)?;
        cursor = after_value;
        let mut default = raw_default;
        let mut required = false;
        let mut multi = false;
        while let Some(value) = &default {
            if let Some(stripped) = value.strip_suffix("/A") {
                required = true;
                default = Some(stripped.to_owned());
            } else if let Some(stripped) = value.strip_suffix("/M") {
                multi = true;
                default = Some(stripped.to_owned());
            } else {
                break;
            }
        }
        if let Some(value) = &mut default {
            if value.starts_with('"') && value.len() >= 2 {
                *value = value[1..value.len() - 1].to_owned();
            }
        }
        arguments.insert(
            arg_name,
            Argument {
                default,
                required,
                multi,
            },
        );
        cursor = skip_whitespace(header, cursor);
    }

    let multi_arguments = arguments
        .iter()
        .filter(|(_, argument)| argument.multi)
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();
    if multi_arguments.len() > 1 {
        return Err("a template can have only one main (/M) argument".to_owned());
    }
    Ok((name, arguments, multi_arguments.into_iter().next()))
}

fn parse_optional_value(text: &str, start: usize) -> Result<(Option<String>, usize), String> {
    if start >= text.len() || is_python_whitespace_at(text, start) {
        return Ok((None, start));
    }
    if text.as_bytes()[start] == b'"' {
        let rest = &text[start + 1..];
        let Some(offset) = rest.find('"') else {
            return Err("unterminated quoted argument value".to_owned());
        };
        let end = start + 1 + offset;
        return Ok((Some(text[start..=end].to_owned()), end + 1));
    }
    let mut end = start;
    while end < text.len() {
        let character = text[end..]
            .chars()
            .next()
            .expect("end is before the end of text");
        if is_python_whitespace(character) || character == '"' {
            break;
        }
        end += character.len_utf8();
    }
    if end == start {
        Ok((None, start))
    } else {
        Ok((Some(text[start..end].to_owned()), end))
    }
}

fn parse_word(text: &str, start: usize, allow_underscore: bool) -> Option<(String, usize)> {
    let bytes = text.as_bytes();
    let first = *bytes.get(start)?;
    if !first.is_ascii_alphanumeric() {
        return None;
    }
    let mut end = start + 1;
    while let Some(byte) = bytes.get(end) {
        if byte.is_ascii_alphanumeric() || (allow_underscore && *byte == b'_') {
            end += 1;
        } else {
            break;
        }
    }
    Some((text[start..end].to_owned(), end))
}

fn parse_invocation_argument(text: &str) -> Option<(String, Option<String>, usize)> {
    let (name, after_name) = parse_word(text, 0, true)?;
    if text.as_bytes().get(after_name) != Some(&b'=') {
        return None;
    }
    let (value, after_value) = parse_optional_value(text, after_name + 1).ok()?;
    Some((name, value, after_value))
}

fn expand_template(
    name: &str,
    argument_text: &str,
    templates: &BTreeMap<String, Template>,
    runtime: &mut ExpansionRuntime,
    limits: Limits,
    output: &mut String,
    invocation: Option<SourceLineLocation>,
) -> Result<(), GenmfError> {
    let template = templates.get(name).ok_or_else(|| {
        error(
            invocation.as_ref().map(|site| site.path.clone()),
            invocation.as_ref().map(|site| site.line),
            format!("unknown GenMF template `{name}`"),
        )
    })?;
    if runtime.stack.iter().any(|active| active.name == name) {
        return Err(error(
            Some(template.source.clone()),
            Some(template.line),
            format!("template `{name}` called recursively"),
        ));
    }
    if runtime.stack.len() >= limits.max_macro_depth {
        return Err(error(
            Some(template.source.clone()),
            Some(template.line),
            "GenMF macro expansion depth exceeded",
        ));
    }
    runtime.budget.charge_work(template.byte_size, limits)?;
    let values = parse_invocation(template, argument_text).map_err(|detail| {
        error(
            invocation
                .as_ref()
                .map(|site| site.path.clone())
                .or_else(|| Some(template.source.clone())),
            invocation
                .as_ref()
                .map(|site| site.line)
                .or(Some(template.line)),
            format!("template `{name}`: {detail}"),
        )
    })?;
    for (argument_name, argument) in &template.arguments {
        if argument.required && values.get(argument_name).and_then(Option::as_ref).is_none() {
            return Err(error(
                invocation
                    .as_ref()
                    .map(|site| site.path.clone())
                    .or_else(|| Some(template.source.clone())),
                invocation
                    .as_ref()
                    .map(|site| site.line)
                    .or(Some(template.line)),
                format!("template `{name}`: required argument `{argument_name}` was not specified"),
            ));
        }
    }

    let body_bytes = template
        .body
        .iter()
        .try_fold(0usize, |total, line| total.checked_add(line.len()));
    let Some(body_bytes) = body_bytes else {
        return Err(error(
            Some(template.source.clone()),
            Some(template.line),
            "expanded template body accounting overflow",
        ));
    };
    if body_bytes > limits.max_output_bytes {
        return Err(error(
            Some(template.source.clone()),
            Some(template.line),
            "single template body exceeds output budget",
        ));
    }
    let mut substituted = Vec::with_capacity(template.body.len());
    let mut substituted_bytes = 0usize;
    for line in &template.body {
        let value = substitute_arguments(line, template, &values, &mut runtime.budget, limits)?;
        let Some(next_bytes) = substituted_bytes.checked_add(value.len()) else {
            return Err(error(
                Some(template.source.clone()),
                Some(template.line),
                "substituted body accounting overflow",
            ));
        };
        if next_bytes > limits.max_output_bytes {
            return Err(error(
                Some(template.source.clone()),
                Some(template.line),
                "substituted template body exceeds output budget",
            ));
        }
        substituted_bytes = next_bytes;
        substituted.push(value);
    }

    let resolved_arguments = template
        .arguments
        .iter()
        .map(|(argument_name, argument)| {
            let value = values
                .get(argument_name)
                .and_then(Option::as_deref)
                .or(argument.default.as_deref())
                .unwrap_or_default();
            (argument_name.clone(), value.to_owned())
        })
        .collect();
    runtime.stack.push(MacroExpansionFrame {
        name: name.to_owned(),
        template_path: template.source.clone(),
        definition_line: template.line,
        invocation,
        arguments: values,
        resolved_arguments,
    });
    // GenMF discovers nested invocations in the original body, before it
    // substitutes argument values. A placeholder that expands to `%name`
    // therefore remains literal unless that original line already had a
    // recognized template reference.
    let references = generate_template_references(&template.body, templates);
    let result = write_lines(
        &substituted,
        &references,
        templates,
        runtime,
        limits,
        output,
        Some(&template.source),
        template.body_start_line,
        Some(TemplateDefinitionOrigin {
            path: &template.source,
            definition_line: template.line,
        }),
    );
    runtime.stack.pop();
    result
}

fn parse_invocation(
    template: &Template,
    argument_text: &str,
) -> Result<BTreeMap<String, Option<String>>, String> {
    let mut values = BTreeMap::new();
    let mut rest = argument_text.to_owned();
    while !rest.is_empty() {
        rest = trim_python_whitespace_start(&rest).to_owned();
        if rest.is_empty() {
            break;
        }
        if let Some((name, value, consumed)) = parse_invocation_argument(&rest) {
            if template.arguments.contains_key(&name) {
                let mut stored_value = value.unwrap_or_default();
                if stored_value.starts_with('"') && stored_value.len() >= 2 {
                    stored_value.drain(..1);
                    stored_value.pop();
                }
                values.insert(name, Some(stored_value));
                rest = trim_python_whitespace_start(&rest[consumed..]).to_owned();
                continue;
            }
            if let Some(multi_name) = &template.multi_argument {
                values.insert(multi_name.clone(), Some(drop_final_character(&rest)));
                rest.clear();
                break;
            }
            return Err(format!("syntax error in arguments: {rest}"));
        }
        if let Some(multi_name) = &template.multi_argument {
            values.insert(multi_name.clone(), Some(drop_final_character(&rest)));
            rest.clear();
            break;
        }
        return Err(format!("syntax error in arguments: {rest}"));
    }
    for name in template.arguments.keys() {
        values.entry(name.clone()).or_insert(None);
    }
    Ok(values)
}

fn drop_final_character(value: &str) -> String {
    value
        .char_indices()
        .next_back()
        .map_or_else(String::new, |(index, _)| value[..index].to_owned())
}

fn substitute_arguments(
    line: &str,
    template: &Template,
    values: &BTreeMap<String, Option<String>>,
    budget: &mut Budget,
    limits: Limits,
) -> Result<String, GenmfError> {
    budget.charge_work(line.len(), limits)?;
    let references = find_argument_references(line, &template.arguments);
    if references.is_empty() {
        return Ok(line.to_owned());
    }
    let mut output = String::new();
    let mut position = 0usize;
    for reference in references {
        if reference.start > position {
            append_temporary(&mut output, &line[position..reference.start], limits)?;
        }
        let value = values
            .get(&reference.name)
            .and_then(Option::as_deref)
            .or_else(|| {
                template
                    .arguments
                    .get(&reference.name)
                    .and_then(|argument| argument.default.as_deref())
            });
        let words = value.map(split_python_whitespace).unwrap_or_default();
        if words.is_empty() {
            append_temporary(&mut output, &reference.prefix, limits)?;
            append_temporary(&mut output, &reference.suffix, limits)?;
        } else {
            for (index, word) in words.iter().enumerate() {
                if index != 0 {
                    append_temporary(&mut output, " ", limits)?;
                }
                append_temporary(&mut output, &reference.prefix, limits)?;
                append_temporary(&mut output, word, limits)?;
                append_temporary(&mut output, &reference.suffix, limits)?;
            }
        }
        position = reference.end;
    }
    if position < line.len() {
        append_temporary(&mut output, &line[position..], limits)?;
    }
    budget.charge_work(output.len(), limits)?;
    Ok(output)
}

struct ArgumentReference {
    start: usize,
    end: usize,
    name: String,
    prefix: String,
    suffix: String,
}

fn find_argument_references(
    text: &str,
    arguments: &BTreeMap<String, Argument>,
) -> Vec<ArgumentReference> {
    let mut references = Vec::new();
    let mut cursor = 0usize;
    while cursor + 2 <= text.len() {
        let Some(relative) = text[cursor..].find("%(") else {
            break;
        };
        let open = cursor + relative;
        let name_start = open + 2;
        let Some(relative_close) = text[name_start..].find(')') else {
            cursor = name_start;
            continue;
        };
        let close = name_start + relative_close;
        let name = &text[name_start..close];
        if !valid_reference_name(name) {
            cursor = name_start;
            continue;
        }
        let mut start = open;
        while start > cursor && substitution_character(text.as_bytes()[start - 1]) {
            start -= 1;
        }
        let mut end = close + 1;
        while end < text.len() && substitution_character(text.as_bytes()[end]) {
            end += 1;
        }
        if arguments.contains_key(name) {
            references.push(ArgumentReference {
                start,
                end,
                name: name.to_owned(),
                prefix: text[start..open].to_owned(),
                suffix: text[close + 1..end].to_owned(),
            });
        }
        cursor = end;
    }
    references
}

fn valid_reference_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

const fn substitution_character(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')
}

#[derive(Clone, Debug)]
struct TemplateReference {
    line_index: usize,
    name: String,
    end_char_index: usize,
}

fn generate_template_references(
    lines: &[String],
    templates: &BTreeMap<String, Template>,
) -> Vec<TemplateReference> {
    lines
        .iter()
        .enumerate()
        .filter_map(|(line_index, line)| {
            find_template_reference(line, templates).map(|(name, end_char_index)| {
                TemplateReference {
                    line_index,
                    name,
                    end_char_index,
                }
            })
        })
        .collect()
}

#[allow(clippy::too_many_arguments)] // Explicit expansion context includes source/template origins.
fn write_lines(
    lines: &[String],
    references: &[TemplateReference],
    templates: &BTreeMap<String, Template>,
    runtime: &mut ExpansionRuntime,
    limits: Limits,
    output: &mut String,
    source: Option<&Path>,
    source_line_start: usize,
    template_origin: Option<TemplateDefinitionOrigin<'_>>,
) -> Result<(), GenmfError> {
    let mut start = 0usize;
    for reference in references {
        if start < reference.line_index {
            for (line_index, line) in lines
                .iter()
                .enumerate()
                .take(reference.line_index)
                .skip(start)
            {
                let source_line = source_line_start.checked_add(line_index).ok_or_else(|| {
                    error(
                        source.map(Path::to_path_buf),
                        None,
                        "source line accounting overflow",
                    )
                })?;
                append_output(
                    output,
                    line,
                    runtime,
                    limits,
                    LineOrigin {
                        path: source,
                        line: source.map(|_| source_line),
                        template: template_origin,
                    },
                )?;
            }
        }
        // This deliberately follows GenMF's cursor behavior even when a
        // preceding continuation consumed this source line: references were
        // precomputed for every original line and are all still visited.
        start = reference.line_index + 1;
        let line = &lines[reference.line_index];
        runtime.budget.charge_work(line.len(), limits)?;
        let mut expanded_call = line.clone();
        let call_start_line = source_line_start
            .checked_add(reference.line_index)
            .ok_or_else(|| {
                error(
                    source.map(Path::to_path_buf),
                    None,
                    "call line accounting overflow",
                )
            })?;
        while has_genmf_continuation(&expanded_call) && start < lines.len() {
            let new_len = expanded_call
                .len()
                .checked_sub(2)
                .and_then(|length| length.checked_add(lines[start].len()))
                .ok_or_else(|| {
                    error(
                        source.map(Path::to_path_buf),
                        Some(call_start_line),
                        "call line accounting overflow",
                    )
                })?;
            if new_len > limits.max_source_bytes {
                return Err(error(
                    source.map(Path::to_path_buf),
                    Some(call_start_line),
                    "continued GenMF call exceeds source budget",
                ));
            }
            expanded_call.truncate(expanded_call.len() - 2);
            expanded_call.push_str(&lines[start]);
            runtime.budget.charge_work(lines[start].len(), limits)?;
            start += 1;
        }
        let argument_text = trim_python_whitespace_start(char_slice_from_char_index(
            &expanded_call,
            reference.end_char_index,
        ));
        if reference.name == "common" {
            runtime.saw_common = true;
        }
        expand_template(
            &reference.name,
            argument_text,
            templates,
            runtime,
            limits,
            output,
            source.map(|path| SourceLineLocation {
                path: path.to_path_buf(),
                line: call_start_line,
            }),
        )
        .map_err(|mut failure| {
            if failure.line.is_none() {
                failure.line = Some(call_start_line);
            }
            if failure.file.is_none() {
                failure.file = source.map(Path::to_path_buf);
            }
            failure
        })?;
    }
    if start < lines.len() {
        for (line_index, line) in lines.iter().enumerate().skip(start) {
            let source_line = source_line_start.checked_add(line_index).ok_or_else(|| {
                error(
                    source.map(Path::to_path_buf),
                    None,
                    "source line accounting overflow",
                )
            })?;
            append_output(
                output,
                line,
                runtime,
                limits,
                LineOrigin {
                    path: source,
                    line: source.map(|_| source_line),
                    template: template_origin,
                },
            )?;
        }
    }
    Ok(())
}

fn char_slice_from_char_index(text: &str, char_index: usize) -> &str {
    text.char_indices().nth(char_index).map_or_else(
        || &text[text.len()..],
        |(byte_index, _)| &text[byte_index..],
    )
}

fn find_template_reference(
    line: &str,
    templates: &BTreeMap<String, Template>,
) -> Option<(String, usize)> {
    if line.is_empty() || line.as_bytes().first() == Some(&b'#') {
        return None;
    }
    let bytes = line.as_bytes();
    let mut index = 0usize;
    while index + 1 < bytes.len() {
        if bytes[index] != b'%' || !bytes[index + 1].is_ascii_alphanumeric() {
            index += 1;
            continue;
        }
        let start = index;
        index += 2;
        while index < bytes.len() && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_')
        {
            index += 1;
        }
        if index < bytes.len() && !is_python_whitespace_at(line, index) {
            continue;
        }
        let name = &line[start + 1..index];
        if line.as_bytes().get(start.wrapping_sub(1)) == Some(&b'#') {
            return None;
        }
        if templates.contains_key(name) {
            return Some((name.to_owned(), line[..index].chars().count()));
        }
        return None;
    }
    None
}

fn append_output(
    output: &mut String,
    text: &str,
    runtime: &mut ExpansionRuntime,
    limits: Limits,
    origin: LineOrigin<'_>,
) -> Result<(), GenmfError> {
    runtime.budget.charge_output(text.len(), limits)?;
    runtime.budget.charge_work(text.len(), limits)?;
    let mut byte_offset = 0usize;
    let mut line_offset = 0usize;
    for segment in text.split_inclusive('\n') {
        let source_line = origin
            .line
            .map(|line| {
                line.checked_add(line_offset).ok_or_else(|| {
                    error(
                        origin.path.map(Path::to_path_buf),
                        None,
                        "source line accounting overflow",
                    )
                })
            })
            .transpose()?;
        let output_start_byte = output
            .len()
            .checked_add(byte_offset)
            .ok_or_else(|| error(None, None, "output byte accounting overflow"))?;
        let output_end_byte = output_start_byte
            .checked_add(segment.len())
            .ok_or_else(|| error(None, None, "output byte accounting overflow"))?;
        if !limits.metadata_provenance_only || segment.starts_with("#MM") {
            let template_path = origin.template.map(|template| template.path.to_path_buf());
            let top_level_source_invocation = runtime
                .stack
                .first()
                .and_then(|frame| frame.invocation.clone());
            let cost = provenance_work_cost(
                origin.path,
                origin.template,
                &runtime.stack,
                top_level_source_invocation.as_ref(),
            )?;
            runtime.budget.charge_work(cost, limits)?;
            runtime.provenance.push(ExpandedLineProvenance {
                output_start_byte,
                output_end_byte,
                output_line: runtime.output_line,
                source_path: origin.path.map(Path::to_path_buf),
                source_line,
                template_path,
                template_definition_line: origin.template.map(|template| template.definition_line),
                macro_stack: runtime.stack.clone(),
                top_level_source_invocation,
            });
        }
        byte_offset = byte_offset
            .checked_add(segment.len())
            .ok_or_else(|| error(None, None, "output byte accounting overflow"))?;
        if segment.ends_with('\n') {
            line_offset = line_offset
                .checked_add(1)
                .ok_or_else(|| error(None, None, "source line accounting overflow"))?;
            runtime.output_line = runtime
                .output_line
                .checked_add(1)
                .ok_or_else(|| error(None, None, "output line accounting overflow"))?;
        }
    }
    output.push_str(text);
    Ok(())
}

fn provenance_work_cost(
    source_path: Option<&Path>,
    template: Option<TemplateDefinitionOrigin<'_>>,
    stack: &[MacroExpansionFrame],
    top_level_source_invocation: Option<&SourceLineLocation>,
) -> Result<usize, GenmfError> {
    let mut cost = std::mem::size_of::<ExpandedLineProvenance>();
    add_provenance_cost(&mut cost, source_path.map_or(0, path_storage_bytes))?;
    if let Some(template) = template {
        add_provenance_cost(&mut cost, path_storage_bytes(template.path))?;
    }
    if let Some(invocation) = top_level_source_invocation {
        add_provenance_cost(&mut cost, std::mem::size_of::<SourceLineLocation>())?;
        add_provenance_cost(&mut cost, path_storage_bytes(&invocation.path))?;
    }
    for frame in stack {
        add_provenance_cost(&mut cost, std::mem::size_of::<MacroExpansionFrame>())?;
        add_provenance_cost(&mut cost, frame.name.len())?;
        add_provenance_cost(&mut cost, path_storage_bytes(&frame.template_path))?;
        if let Some(invocation) = &frame.invocation {
            add_provenance_cost(&mut cost, std::mem::size_of::<SourceLineLocation>())?;
            add_provenance_cost(&mut cost, path_storage_bytes(&invocation.path))?;
        }
        for (name, value) in &frame.arguments {
            add_provenance_cost(&mut cost, name.len())?;
            if let Some(value) = value {
                add_provenance_cost(&mut cost, value.len())?;
            }
        }
        for (name, value) in &frame.resolved_arguments {
            add_provenance_cost(&mut cost, name.len())?;
            add_provenance_cost(&mut cost, value.len())?;
        }
    }
    Ok(cost)
}

fn add_provenance_cost(total: &mut usize, amount: usize) -> Result<(), GenmfError> {
    *total = total
        .checked_add(amount)
        .ok_or_else(|| error(None, None, "GenMF provenance accounting overflow"))?;
    Ok(())
}

fn path_storage_bytes(path: &Path) -> usize {
    path.to_string_lossy().len()
}

fn append_temporary(output: &mut String, text: &str, limits: Limits) -> Result<(), GenmfError> {
    let next = output
        .len()
        .checked_add(text.len())
        .ok_or_else(|| error(None, None, "temporary output accounting overflow"))?;
    if next > limits.max_output_bytes {
        return Err(error(
            None,
            None,
            "temporary expansion exceeds output budget",
        ));
    }
    output.push_str(text);
    Ok(())
}

fn has_genmf_continuation(line: &str) -> bool {
    line.len() >= 2 && line.as_bytes()[line.len() - 2] == b'\\' && line.ends_with('\n')
}

fn split_lines(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut lines = text
        .split_inclusive('\n')
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if !text.ends_with('\n') && lines.is_empty() {
        lines.push(text.to_owned());
    }
    lines
}

fn normalize_newlines(text: &str) -> String {
    let mut normalized = String::with_capacity(text.len());
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\r' {
            if characters.peek() == Some(&'\n') {
                characters.next();
            }
            normalized.push('\n');
        } else {
            normalized.push(character);
        }
    }
    normalized
}

fn decode_iso_8859_15(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len());
    for byte in bytes {
        let codepoint = match byte {
            0xA4 => 0x20AC,
            0xA6 => 0x0160,
            0xA8 => 0x0161,
            0xB4 => 0x017D,
            0xB8 => 0x017E,
            0xBC => 0x0152,
            0xBD => 0x0153,
            0xBE => 0x0178,
            value => u32::from(*value),
        };
        output.push(char::from_u32(codepoint).expect("ISO-8859-15 maps to valid Unicode"));
    }
    output
}

fn unquote_path(value: &str) -> &str {
    if value.len() > 1 && value.starts_with('"') && value.ends_with('"') {
        &value[1..value.len() - 1]
    } else {
        value
    }
}

fn read_limited(path: &Path, limit: usize) -> Result<Vec<u8>, GenmfError> {
    let ceiling = u64::try_from(limit)
        .map_err(|_| error(Some(path.to_path_buf()), None, "file byte budget overflow"))?;
    // Descriptor-relative no-follow traversal prevents link redirection.
    // The shared reader opens nonblocking so a raced FIFO cannot wait for
    // a writer, then verifies a stable regular snapshot under the ceiling.
    aros_common::measure_regular_file_bounded(path, ceiling)
        .map_err(|cause| {
            error(
                Some(path.to_path_buf()),
                None,
                format!("cannot read stable regular file within byte budget: {cause}"),
            )
        })?
        .map(|(_, bytes)| bytes)
        .ok_or_else(|| {
            error(
                Some(path.to_path_buf()),
                None,
                "regular input file is absent",
            )
        })
}

fn skip_whitespace(text: &str, mut cursor: usize) -> usize {
    while cursor < text.len() && is_python_whitespace_at(text, cursor) {
        cursor += text[cursor..]
            .chars()
            .next()
            .expect("cursor is in text")
            .len_utf8();
    }
    cursor
}

const fn is_python_whitespace(character: char) -> bool {
    character.is_whitespace() || matches!(character, '\u{001c}'..='\u{001f}')
}

fn is_python_whitespace_at(text: &str, index: usize) -> bool {
    text.get(index..)
        .and_then(|rest| rest.chars().next())
        .is_some_and(is_python_whitespace)
}

fn trim_python_whitespace(text: &str) -> &str {
    text.trim_matches(is_python_whitespace)
}

fn trim_python_whitespace_start(text: &str) -> &str {
    text.trim_start_matches(is_python_whitespace)
}

fn split_python_whitespace(text: &str) -> Vec<&str> {
    text.split(is_python_whitespace)
        .filter(|word| !word.is_empty())
        .collect()
}

fn error(file: Option<PathBuf>, line: Option<usize>, detail: impl Into<String>) -> GenmfError {
    GenmfError {
        file,
        line,
        detail: detail.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::{expand_bytes, expand_files, expand_text, Limits};
    use std::fs;
    use std::path::{Path, PathBuf};

    struct TestTree {
        root: tempfile::TempDir,
    }

    impl TestTree {
        fn new() -> Self {
            let base = std::env::temp_dir()
                .canonicalize()
                .expect("temporary directory resolves without aliases");
            let root = tempfile::Builder::new()
                .prefix("genmf-projection-")
                .tempdir_in(base)
                .expect("create unique temporary tree");
            Self { root }
        }

        fn path(&self, relative: &str) -> PathBuf {
            self.root.path().join(relative)
        }

        fn write(&self, relative: &str, bytes: &[u8]) -> PathBuf {
            let path = self.path(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create fixture parent");
            }
            fs::write(&path, bytes).expect("write fixture");
            path
        }
    }

    #[test]
    fn expands_captured_bytes_without_rereading_source_path() {
        let tree = TestTree::new();
        let template = tree.write("root.tmpl", b"%define common\n%end\n");
        let source = tree.write("input.src", b"changed-path-content\n");
        let captured = b"captured-\xA4\n";
        let result = expand_bytes(captured, &source, &template, Limits::default()).unwrap();
        assert!(result.text.contains("captured-\u{20ac}"));
        assert!(!result.text.contains("changed-path-content"));
        let limits = Limits {
            max_source_bytes: captured.len() - 1,
            ..Limits::default()
        };
        assert!(expand_bytes(captured, &source, &template, limits).is_err());
    }

    #[test]
    fn expands_relative_includes_defaults_required_multi_prefix_suffix_and_common() {
        let tree = TestTree::new();
        let nested = "%define nested value=/A\nnested = [%(value)]\n%end\n";
        let root = concat!(
            "%include nested/children.tmpl\n",
            "%define emit required=/A items=/M label=default-value\n",
            "expanded = %(required)\n",
            "words = <%(items)>\n",
            "%nested value=%(required)\n",
            "defaulted = %(label)\n",
            "%end\n",
            "%define common\nCOMMON-LINE\n%end\n",
        );
        let root_path = tree.write("root.tmpl", root.as_bytes());
        let child_path = tree.write("nested/children.tmpl", nested.as_bytes());
        let source = concat!(
            "# %emit required=ignored items=comment\n",
            "%emit required=ready \\\n",
            "    red blue\n",
        );

        let output =
            expand_text(source, Path::new("input.mm"), &root_path, Limits::default()).unwrap();
        assert_eq!(
            output.text,
            concat!(
                "# %emit required=ignored items=comment\n",
                "expanded = ready\n",
                "words = <red blue>\n",
                "nested = [ready]\n",
                "defaulted = default-value\n",
                "\n",
                "COMMON-LINE\n",
            )
        );
        assert_eq!(output.template_snapshots.len(), 2);
        assert_eq!(
            output.template_snapshots[0].path,
            root_path.canonicalize().unwrap()
        );
        assert_eq!(output.template_snapshots[0].bytes, root.as_bytes());
        assert_eq!(
            output.template_snapshots[1].path,
            child_path.canonicalize().unwrap()
        );
        assert_eq!(output.template_snapshots[1].bytes, nested.as_bytes());
    }

    #[test]
    fn supports_multiline_calls_inline_comment_expansion_and_explicit_common() {
        let tree = TestTree::new();
        let template = concat!(
            "%define decorate value=default/M\n",
            "decorated = (%(value))\n",
            "%end\n",
            "%define common\n",
            "EXPLICIT-COMMON\n",
            "%end\n",
        );
        let template_path = tree.write("root.tmpl", template.as_bytes());
        let source = concat!(
            "before = untouched\n",
            "%decorate \\\n",
            "    multiline\n",
            "after = untouched\n",
            "X = keep # %decorate value=comment\n",
            "%common\n",
            "keep = %not_defined value=x\n",
        );
        let output = expand_text(
            source,
            Path::new("input.mm"),
            &template_path,
            Limits::default(),
        )
        .unwrap();
        assert_eq!(
            output.text,
            concat!(
                "before = untouched\n",
                "decorated = (multiline)\n",
                "after = untouched\n",
                "decorated = (comment)\n",
                "EXPLICIT-COMMON\n",
                "keep = %not_defined value=x\n",
            )
        );
    }

    #[test]
    fn expands_build_module_and_hidd_stubs_fixture() {
        let tree = TestTree::new();
        let template = concat!(
            "%define build_module modname=/A modtype=/A files=/A\n",
            "module=%(modname) type=%(modtype) files=%(files)\n",
            "%end\n",
            "%define make_hidd_stubs hidd=/A\n",
            "hidd-%(hidd)-stubs\n",
            "%end\n",
            "%define common\n",
            "COMMON\n",
            "%end\n",
        );
        let template_path = tree.write("make.tmpl", template.as_bytes());
        let source = concat!(
            "%build_module modname=projection modtype=resource files=sample\n",
            "%make_hidd_stubs hidd=fixture\n",
        );
        let output = expand_text(
            source,
            Path::new("input.mm"),
            &template_path,
            Limits::default(),
        )
        .unwrap();
        assert_eq!(
            output.text,
            "module=projection type=resource files=sample\nhidd-fixture-stubs\n\nCOMMON\n"
        );
        assert!(!output.text.contains("%build_module "));
        assert!(!output.text.contains("%make_hidd_stubs "));
    }

    #[test]
    fn metadata_only_origins_preserve_text_and_physical_line_identity_with_bounded_work() {
        let tree = TestTree::new();
        let template = format!(
            "%define module payload=\n{}#MM- module : child\n%end\n",
            "ordinary-recipe-text\n".repeat(100)
        );
        let template_path = tree.write("config/make.tmpl", template.as_bytes());
        let source = format!(
            "%module payload={}\n#MM- handwritten : child\n",
            "x".repeat(1024)
        );
        let full = expand_text(
            &source,
            Path::new("input.mm"),
            &template_path,
            Limits::default(),
        )
        .unwrap();
        let limits = Limits {
            max_work_bytes: 50_000,
            ..Limits::default()
        };
        assert!(expand_text(&source, Path::new("input.mm"), &template_path, limits).is_err());
        let limited = expand_text(
            &source,
            Path::new("input.mm"),
            &template_path,
            Limits {
                metadata_provenance_only: true,
                ..limits
            },
        )
        .unwrap();
        assert_eq!(limited.text, full.text);
        assert_eq!(limited.provenance.len(), 2);
        assert_eq!(limited.provenance[0].output_line, 101);
        assert_eq!(limited.provenance[1].output_line, 102);
        assert_eq!(limited.provenance[0].macro_stack.len(), 1);
        assert!(limited.provenance[1].macro_stack.is_empty());
    }

    #[test]
    fn records_nested_macro_provenance_without_rewriting_emitted_text() {
        let tree = TestTree::new();
        let template = concat!(
            "%define gen_archspecificrules target= subtarget=\n",
            "#MM- hook-%(target)%(subtarget) : missing-%(target)%(subtarget)\n",
            "%end\n",
            "%define build_module target= subtarget=\n",
            "%gen_archspecificrules target=%(target) subtarget=%(subtarget)\n",
            "%end\n",
            "%define common\n",
            "#MM- common-extension : no-owner\n",
            "%end\n",
        );
        let template_path = tree.write("config/make.tmpl", template.as_bytes());
        let canonical_template = template_path.canonicalize().unwrap();
        let generated = "#MM- hook--set-archincludes-variant : missing--set-archincludes-variant\n";
        let source = concat!(
            "%build_module target=-set-archincludes \\\n",
            "    subtarget=-variant\n",
            "%build_module target=-set-archincludes subtarget=-variant\n",
            "#MM- hook--set-archincludes-variant : missing--set-archincludes-variant\n",
        );
        let source_path = Path::new("input.mm");
        let output = expand_text(source, source_path, &template_path, Limits::default()).unwrap();
        let expected = concat!(
            "#MM- hook--set-archincludes-variant : missing--set-archincludes-variant\n",
            "#MM- hook--set-archincludes-variant : missing--set-archincludes-variant\n",
            "#MM- hook--set-archincludes-variant : missing--set-archincludes-variant\n",
            "\n",
            "#MM- common-extension : no-owner\n",
        );
        assert_eq!(output.text, expected);

        let matching = output
            .provenance
            .iter()
            .filter(|span| {
                output
                    .text
                    .get(span.output_start_byte..span.output_end_byte)
                    == Some(generated)
            })
            .collect::<Vec<_>>();
        assert_eq!(matching.len(), 3);
        for (span, invocation_line) in matching.iter().take(2).zip([1, 3]) {
            assert_eq!(
                span.template_path.as_deref(),
                Some(canonical_template.as_path())
            );
            assert_eq!(span.template_definition_line, Some(1));
            assert_eq!(span.source_line, Some(2));
            assert_eq!(span.macro_stack.len(), 2);
            assert_eq!(span.macro_stack[0].name, "build_module");
            assert_eq!(span.macro_stack[1].name, "gen_archspecificrules");
            assert_eq!(span.macro_stack[1].definition_line, 1);
            assert_eq!(span.macro_stack[1].template_path, canonical_template);
            assert_eq!(
                span.top_level_source_invocation
                    .as_ref()
                    .unwrap()
                    .path
                    .as_path(),
                source_path
            );
            assert_eq!(
                span.top_level_source_invocation.as_ref().unwrap().line,
                invocation_line
            );
            assert_eq!(
                span.macro_stack[0].resolved_arguments["target"],
                "-set-archincludes"
            );
            assert_eq!(
                span.macro_stack[1].resolved_arguments["target"],
                "-set-archincludes"
            );
            assert_eq!(
                span.macro_stack[1].resolved_arguments["subtarget"],
                "-variant"
            );
        }
        assert_eq!(matching[0].output_line, 1);
        assert_eq!(matching[1].output_line, 2);

        // Same emitted #MM bytes, but this line is a handwritten source claim,
        // not output from a template hook.
        let handwritten = matching[2];
        assert_eq!(handwritten.output_line, 3);
        assert_eq!(handwritten.source_path.as_deref(), Some(source_path));
        assert_eq!(handwritten.source_line, Some(4));
        assert!(handwritten.template_path.is_none());
        assert!(handwritten.template_definition_line.is_none());
        assert!(handwritten.macro_stack.is_empty());
        assert!(handwritten.top_level_source_invocation.is_none());

        let common = output
            .provenance
            .iter()
            .find(|span| {
                output
                    .text
                    .get(span.output_start_byte..span.output_end_byte)
                    == Some("#MM- common-extension : no-owner\n")
            })
            .unwrap();
        assert_eq!(
            common.template_path.as_deref(),
            Some(canonical_template.as_path())
        );
        assert_eq!(common.template_definition_line, Some(7));
        assert_eq!(common.source_line, Some(8));
        assert_eq!(common.macro_stack.len(), 1);
        assert_eq!(common.macro_stack[0].name, "common");
        assert!(common.macro_stack[0].invocation.is_none());
        assert!(common.top_level_source_invocation.is_none());
    }

    #[test]
    fn discovers_nested_invocations_before_argument_substitution() {
        let tree = TestTree::new();
        let template = concat!(
            "%define inner\n",
            "#MM injected\n",
            "injected :\n",
            "%end\n",
            "%define outer value=/A\n",
            "%(value)\n",
            "%end\n",
            "%define common\n",
            "#MM clean\n",
            "clean :\n",
            "%end\n",
        );
        let template_path = tree.write("root.tmpl", template.as_bytes());
        let output = expand_text(
            "%outer value=\"%inner\"\n",
            Path::new("input.mm"),
            &template_path,
            Limits::default(),
        )
        .unwrap();
        assert_eq!(output.text, "%inner\n\n#MM clean\nclean :\n");
    }

    #[test]
    fn uses_original_reference_character_offsets_after_substitution() {
        let tree = TestTree::new();
        let template = concat!(
            "%define inner arg=/M\n",
            "inner=[%(arg)]\n",
            "%end\n",
            "%define outer value=/A\n",
            "%(value)%inner arg=hello\n",
            "%end\n",
            "%define common\n",
            "#MM clean\n",
            "clean :\n",
            "%end\n",
        );
        let template_path = tree.write("root.tmpl", template.as_bytes());
        let output = expand_text(
            "%outer value=\"x\"\n",
            Path::new("input.mm"),
            &template_path,
            Limits::default(),
        )
        .unwrap();
        assert_eq!(output.text, "inner=[llo]\n\n#MM clean\nclean :\n");
    }

    #[test]
    fn continuation_consumption_does_not_erase_precomputed_physical_calls() {
        let tree = TestTree::new();
        let template = tree.write(
            "root.tmpl",
            b"%define outer rest=/M\nOUTER\n%end\n%define inner\nINNER\n%end\n",
        );
        let output = expand_text(
            "%outer ignored \\\n%inner\n",
            Path::new("input.mm"),
            &template,
            Limits::default(),
        )
        .unwrap();
        assert_eq!(output.text, "OUTER\nINNER\n\n");
    }

    #[test]
    fn bare_cr_source_lines_match_the_classic_reference_without_argument_merging() {
        let tree = TestTree::new();
        let template = tree.write("root.tmpl", b"%define common args=/M\nSEEN=%(args)\n%end\n");
        let output = expand_text(
            "%common\rX\r",
            Path::new("input.mm"),
            &template,
            Limits::default(),
        )
        .unwrap();
        assert_eq!(output.text, "SEEN=\nX\n");
    }

    #[test]
    fn rejects_missing_required_arguments_and_recursive_templates() {
        let tree = TestTree::new();
        let template = concat!(
            "%define required value=/A\n",
            "%(value)\n",
            "%end\n",
            "%define recurse\n",
            "%recurse\n",
            "%end\n",
        );
        let template_path = tree.write("root.tmpl", template.as_bytes());

        let missing = expand_text(
            "%required\n",
            Path::new("input.mm"),
            &template_path,
            Limits::default(),
        )
        .unwrap_err();
        assert!(missing.detail.contains("required argument"));
        let recursive = expand_text(
            "%recurse\n",
            Path::new("input.mm"),
            &template_path,
            Limits::default(),
        )
        .unwrap_err();
        assert!(recursive.detail.contains("recursively"));
    }

    #[test]
    fn matches_python_whitespace_and_latin9_argument_boundaries() {
        let tree = TestTree::new();
        let mut template = b"%define common\nCOMMON\n%end\n%define probe value=".to_vec();
        template.push(0xe9);
        template.extend_from_slice(
            b"\n%(value)\n%end\n%define quoted value=\"literal/A\"\n%(value)\n%end\n%define words value=\"default\"\n<%(value)>\n%end\n",
        );
        let template_path = tree.write("root.tmpl", &template);

        for separator in ['\u{00a0}', '\u{001c}'] {
            let source = format!("%common{separator}\n");
            let output = expand_text(
                &source,
                Path::new("unicode-input.mm"),
                &template_path,
                Limits::default(),
            )
            .unwrap();
            assert_eq!(output.text, "COMMON\n");
        }

        let source = "%probe\n%quoted\n%words value=\"red blue\"\n";
        let output = expand_text(
            source,
            Path::new("unicode-input.mm"),
            &template_path,
            Limits::default(),
        )
        .unwrap();
        assert_eq!(output.text, "é\nliteral/A\n<red blue>\n\nCOMMON\n");
    }

    #[test]
    fn normalizes_crlf_and_cr_in_source_and_template_text() {
        let tree = TestTree::new();
        let template = tree.write("root.tmpl", b"%define emit\r\nA\r\nB\rC\n%end\r\n");
        let output = expand_text(
            "%emit\r\n",
            Path::new("newline-input.mm"),
            &template,
            Limits::default(),
        )
        .unwrap();
        assert_eq!(output.text, "A\nB\nC\n\n");
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlink_import_aliases_and_source_paths() {
        use std::os::unix::fs::symlink;

        let tree = TestTree::new();
        let child = tree.write("child.tmpl", b"%define common\ncommon\n%end\n");
        symlink(&child, tree.path("alias.tmpl")).unwrap();
        let root = tree.write("root.tmpl", b"%include child.tmpl\n%include alias.tmpl\n");
        let alias_error =
            expand_text("source\n", Path::new("input.mm"), &root, Limits::default()).unwrap_err();
        assert!(alias_error.detail.contains("symbolic link"));

        let source = tree.write("source.mm", b"source\n");
        symlink(&source, tree.path("source-link.mm")).unwrap();
        let source_error =
            expand_files(&tree.path("source-link.mm"), &root, Limits::default()).unwrap_err();
        assert!(source_error.detail.contains("symbolic link"));
    }

    #[test]
    fn enforces_output_and_template_root_budgets_and_regular_files() {
        let tree = TestTree::new();
        fs::create_dir_all(tree.path("templates")).unwrap();
        fs::create_dir_all(tree.path("outside")).unwrap();
        let repeated = tree.write(
            "templates/root.tmpl",
            b"%define repeat value=/M\n%(value)%(value)%(value)\n%end\n",
        );
        let limits = Limits {
            max_output_bytes: 8,
            ..Limits::default()
        };
        let result = expand_text(
            "%repeat abcdefghijklmnop\n",
            Path::new("input.mm"),
            &repeated,
            limits,
        );
        assert!(result.unwrap_err().detail.contains("budget"));

        let oversized_source = tree.write("oversized.mm", b"12345");
        let source_limit = Limits {
            max_source_bytes: 4,
            ..Limits::default()
        };
        let result = expand_files(&oversized_source, &repeated, source_limit);
        assert!(result.unwrap_err().detail.contains("byte budget"));

        let small_source = tree.write("small.mm", b"source\n");
        let template_limit = Limits {
            max_template_bytes: 4,
            ..Limits::default()
        };
        let result = expand_files(&small_source, &repeated, template_limit);
        assert!(result.unwrap_err().detail.contains("byte budget"));

        tree.write("outside/child.tmpl", b"%define child\nok\n%end\n");
        let escape = tree.write("templates/root.tmpl", b"%include ../outside/child.tmpl\n");
        let result = expand_text(
            "source\n",
            Path::new("input.mm"),
            &escape,
            Limits::default(),
        );
        assert!(result
            .unwrap_err()
            .detail
            .contains("outside the template root"));

        fs::create_dir(tree.path("templates/directory.tmpl")).unwrap();
        let directory = tree.write("templates/root.tmpl", b"%include directory.tmpl\n");
        let result = expand_text(
            "source\n",
            Path::new("input.mm"),
            &directory,
            Limits::default(),
        );
        assert!(result.unwrap_err().detail.contains("regular file"));
    }

    #[test]
    fn keeps_unknown_reference_text_and_adds_implicit_common() {
        let tree = TestTree::new();
        let template = tree.write("root.tmpl", b"%define common\nCOMMON\n%end\n");
        let output = expand_text(
            "ordinary\n%unknown value=x\n",
            Path::new("input.mm"),
            &template,
            Limits::default(),
        )
        .unwrap();
        assert_eq!(output.text, "ordinary\n%unknown value=x\n\nCOMMON\n");
    }
}
