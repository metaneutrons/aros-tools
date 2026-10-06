//! Native MetaMake template reading and hash-verified macro contracts.

use super::macro_edges::{closed_macro_arguments, genmf_argument_value, ClosedMacroArguments};
use super::{
    MAX_LINES, MAX_MACRO_ARGUMENTS, MAX_MACRO_ARGUMENT_BYTES, MAX_TEMPLATE_BYTES,
    MAX_TEMPLATE_CLOSURE_DEFINITIONS, MAX_TEMPLATE_CLOSURE_DEPTH, MAX_TEMPLATE_CLOSURE_WORK,
};
use crate::dirs::DirVars;
use crate::make_expr::{evaluate_make_expr, MakeExprContext};
use crate::make_vars::VarScope;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

const BUILD_PROG_SHA256: &str = "e276e953080db3a15e844057a5d2454c0c537da800904c9544c0c106566d375e";

pub(super) const COMPILE_MULTI_SHA256: &str =
    "a55875ba53ff3445e8083c15f07348e7c294cf08c42235b142caf6af9ee50bc7";

const ASSEMBLE_MULTI_SHA256: &str =
    "b8718f311b850bb70a5c20a9ab5c703ee9e96858d15956b255bc065e7affeb36";

const LINK_BINARY_SHA256: &str = "5d2fade4d4af278786918729526950b2d010e06760fe682671de001542fbc120";

const ADD_COMPILERLINKFLAGS_SHA256: &str =
    "e7cf94a8764675c98d39f614d10339187821b7f535740b8a8fcf138191139163";

const ASSEMBLE_Q_SHA256: &str = "46dff6d25108327406ecb87f8cdf807aeefc6fcae10a75e5a6af5fd2766912ea";

const COMPILE_Q_SHA256: &str = "3412141396c0a321e8b14d632a412b7563981564d5bdeabe957fd67c788dd0b2";

const FILEACTIONMSG_SHA256: &str =
    "f812409a32e02d8f6535b8d4708de68aefde558396e47f6e304a91af79263cff";

const GEN_ARCHSPECIFICRULES_SHA256: &str =
    "fc50253aa57b06aac5fdd739e2e871391d4e9a78d84989f83e52af71b162a062";

const INCLUDE_DEPS_SHA256: &str =
    "8c5f632ffd7bb7ced511ee6441c5a1c272b382ccd1a4ed58c5391ae7febdee82";

const LINK_Q_SHA256: &str = "3b10e411a1974ff5c0b29b038e9db3a916826ce4abf8b682d17d7086bc3af7d4";

const MKDEPEND_Q_SHA256: &str = "3e6afefd7813acc49f0c80df32b99f9ae3e0976420f04ffbf6be7872d2ff1c66";

const MKDIR_Q_SHA256: &str = "9c2abcb165988c8032728270f471abf11fd80b3ac2b3c64841b2b8ff8126c5f9";

const RULE_COMPILE_CXX_MULTI_SHA256: &str =
    "6ae587679b601c38f074aa2e23c4b63ba859f0ca6d62a0b68b5212767566371c";

const RULE_COMPILE_OBJC_MULTI_SHA256: &str =
    "afad565befd2a2f9b9d6118c54711f9400538857f2855a00f648ac5668edd446";

const RULE_LINK_PROG_SHA256: &str =
    "2abb2a483e55353cbfad1397bd24fcf957e9c937a501e45b2c6124a62f25a3c6";

const RULE_MAKEDIRS_SHA256: &str =
    "8e010ec89890a894073dc26eb68ecf68a56e27faa4e69f4f6c17696098e12414";

const STRIP_Q_SHA256: &str = "94bf874bbdf35a70dc73feaba1f3f562126f27c27a47bf46bf81642cbc8f58f3";

const TRUSTED_TEMPLATE_CLOSURE_REFERENCES: &[&str] = &[
    "add_compilerlinkflags",
    "assemble_q",
    "compile_q",
    "fileactionmsg",
    "gen_archspecificrules",
    "include_deps",
    "link_q",
    "mkdepend_q",
    "mkdir_q",
    "rule_assemble_multi",
    "rule_compile_cxx_multi",
    "rule_compile_multi",
    "rule_compile_objc_multi",
    "rule_link_prog",
    "rule_makedirs",
    "strip_q",
];

#[derive(Debug, Clone, Copy)]
pub(super) enum MacroForm {
    BuildProg,
    CompileMulti,
    AssembleMulti,
    LinkBinary,
}

/// Finite subset of the four independently hash-verified macro forms.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct VerifiedMacros([bool; 4]);

impl VerifiedMacros {
    pub(super) const fn allows(self, form: MacroForm) -> bool {
        self.0[form as usize]
    }

    #[cfg(test)]
    pub(super) fn from_forms(forms: &[MacroForm]) -> Self {
        let mut verified = Self::default();
        for form in forms {
            verified.0[*form as usize] = true;
        }
        verified
    }
}

pub(super) type MacroContract = BTreeMap<String, MacroArgumentSpec>;

#[derive(Debug)]
pub(super) struct NativeMacroDefinition {
    pub(super) sha256: String,
    pub(super) body_lines: Vec<String>,
}

pub(super) type NativeMacroDefinitions = BTreeMap<String, Vec<NativeMacroDefinition>>;

type NativeMacroFileParts = (Vec<(String, NativeMacroDefinition)>, Vec<PathBuf>);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MacroArgumentSpec {
    pub(super) default: Option<String>,
    pub(super) required: bool,
}

pub(super) fn verify_native_macros(root: &Path) -> VerifiedMacros {
    let Some((template, definitions)) = read_native_template_set(root) else {
        return VerifiedMacros::default();
    };
    VerifiedMacros(std::array::from_fn(|index| {
        let Some((name, expected_sha256)) = macro_contract_identity(index) else {
            return false;
        };
        verified_macro_contract(&template, name, expected_sha256).is_some()
            && verified_macro_closure(name, &definitions)
    }))
}

#[cfg(test)]
pub(super) fn verified_macro(template: &str, name: &str, expected_sha256: &str) -> bool {
    verified_macro_contract(template, name, expected_sha256).is_some()
}

fn read_native_make_template(root: &Path) -> Option<String> {
    let path = root.join("config/make.tmpl");
    // This is diagnostic attribution, but the hash-bound source read must
    // still be bounded and must not accept a symlink in place of the template.
    let metadata = fs::symlink_metadata(&path).ok()?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_TEMPLATE_BYTES as u64 {
        return None;
    }
    let file = fs::File::open(path).ok()?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    if file
        .take(MAX_TEMPLATE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() > MAX_TEMPLATE_BYTES
    {
        return None;
    }
    String::from_utf8(bytes)
        .ok()
        .map(|template| template.replace("\r\n", "\n"))
}

fn read_native_template_set(root: &Path) -> Option<(String, NativeMacroDefinitions)> {
    let main_template = read_native_make_template(root)?;
    let mut queued = VecDeque::from([Path::new("make.tmpl").to_path_buf()]);
    let mut visited_files = BTreeSet::new();
    let mut definitions = NativeMacroDefinitions::new();
    let mut total_bytes = 0usize;
    let mut total_lines = 0usize;

    while let Some(relative_path) = queued.pop_front() {
        if !visited_files.insert(relative_path.clone()) {
            continue;
        }
        if visited_files.len() > 32 {
            return None;
        }
        let template = if relative_path == Path::new("make.tmpl") {
            main_template.clone()
        } else {
            read_included_native_template(root, &relative_path)?
        };
        total_bytes = total_bytes.checked_add(template.len())?;
        total_lines = total_lines.checked_add(template.lines().count())?;
        if total_bytes > MAX_TEMPLATE_BYTES || total_lines > MAX_LINES {
            return None;
        }

        let (file_definitions, includes) = parse_native_macro_file(&template)?;
        for (name, definition) in file_definitions {
            definitions.entry(name).or_default().push(definition);
            if definitions.len() > MAX_TEMPLATE_CLOSURE_DEFINITIONS {
                return None;
            }
        }
        for include in includes {
            let parent = relative_path.parent().unwrap_or_else(|| Path::new(""));
            let include_path = parent.join(include);
            if !safe_template_relative_path(&include_path) {
                return None;
            }
            queued.push_back(include_path);
        }
    }

    Some((main_template, definitions))
}

fn read_included_native_template(root: &Path, relative_path: &Path) -> Option<String> {
    if !safe_template_relative_path(relative_path) {
        return None;
    }
    let config = root.join("config");
    let mut path = config.clone();
    for component in relative_path.components() {
        let std::path::Component::Normal(part) = component else {
            return None;
        };
        path.push(part);
        let metadata = fs::symlink_metadata(&path).ok()?;
        let is_final = path == config.join(relative_path);
        if is_final {
            if !metadata.file_type().is_file() || metadata.len() > MAX_TEMPLATE_BYTES as u64 {
                return None;
            }
        } else if !metadata.file_type().is_dir() {
            return None;
        }
    }

    let metadata = fs::symlink_metadata(&path).ok()?;
    let file = fs::File::open(path).ok()?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    if file
        .take(MAX_TEMPLATE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() > MAX_TEMPLATE_BYTES
    {
        return None;
    }
    String::from_utf8(bytes)
        .ok()
        .map(|template| template.replace("\r\n", "\n"))
}

fn safe_template_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
        && path.to_str().is_some_and(|value| {
            value.len() <= 256
                && path
                    .extension()
                    .is_some_and(|extension| extension == "tmpl")
                && !value.contains('\\')
        })
}

pub(super) fn parse_native_macro_file(template: &str) -> Option<NativeMacroFileParts> {
    let lines = template.lines().collect::<Vec<_>>();
    if lines.len() > MAX_LINES {
        return None;
    }
    let mut definitions = Vec::new();
    let mut includes = Vec::new();
    let mut cursor = 0usize;
    while cursor < lines.len() {
        let raw = lines[cursor];
        if let Some(name) = native_define_name(raw) {
            let start = cursor;
            let name = name.to_owned();
            if !valid_genmf_name(&name) {
                return None;
            }
            let mut header_end = cursor;
            while lines[header_end].ends_with('\\') {
                header_end = header_end.checked_add(1)?;
                if header_end >= lines.len() || header_end - start > MAX_LINES {
                    return None;
                }
            }
            let end = lines[header_end + 1..]
                .iter()
                .position(|line| line.starts_with("%end"))?
                + header_end
                + 1;
            let body = lines[start..=end].join("\n");
            let body_lines = lines[header_end + 1..end]
                .iter()
                .map(|line| (*line).to_owned())
                .collect();
            definitions.push((
                name,
                NativeMacroDefinition {
                    sha256: aros_common::sha256_bytes(body.as_bytes()).to_string(),
                    body_lines,
                },
            ));
            cursor = end.checked_add(1)?;
            continue;
        }
        if let Some(include) = raw.strip_prefix("%include") {
            if include.chars().next().is_some_and(char::is_whitespace) {
                let include = include.trim();
                let include =
                    if include.starts_with('"') && include.ends_with('"') && include.len() >= 2 {
                        &include[1..include.len() - 1]
                    } else {
                        include
                    };
                if include.is_empty() || include.len() > 256 || include.contains(['$', '\\']) {
                    return None;
                }
                includes.push(PathBuf::from(include));
            }
        }
        cursor += 1;
    }
    Some((definitions, includes))
}

fn valid_genmf_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn native_define_name(line: &str) -> Option<&str> {
    let tail = line.strip_prefix("%define")?;
    if !tail.chars().next().is_some_and(char::is_whitespace) {
        return None;
    }
    tail.split_whitespace().next()
}

fn verified_macro_closure(root_name: &str, definitions: &NativeMacroDefinitions) -> bool {
    let mut verifier = MacroClosureVerifier::new(definitions, |name| {
        trusted_template_hash(name).map(str::to_owned)
    });
    verifier.visit(root_name, 0)
}

#[cfg(test)]
pub(super) fn verified_macro_closure_with_hashes(
    root_name: &str,
    definitions: &NativeMacroDefinitions,
    trusted_hash: impl Fn(&str) -> Option<String>,
) -> bool {
    let mut verifier = MacroClosureVerifier::new(definitions, trusted_hash);
    verifier.visit(root_name, 0)
}

struct MacroClosureVerifier<'a, F> {
    definitions: &'a NativeMacroDefinitions,
    template_names: BTreeSet<String>,
    visited: BTreeSet<String>,
    active: BTreeSet<String>,
    work: usize,
    trusted_hash: F,
}

impl<'a, F> MacroClosureVerifier<'a, F>
where
    F: Fn(&str) -> Option<String>,
{
    fn new(definitions: &'a NativeMacroDefinitions, trusted_hash: F) -> Self {
        let mut template_names = definitions.keys().cloned().collect::<BTreeSet<_>>();
        template_names.extend(
            TRUSTED_TEMPLATE_CLOSURE_REFERENCES
                .iter()
                .map(|name| (*name).to_owned()),
        );
        Self {
            definitions,
            template_names,
            visited: BTreeSet::new(),
            active: BTreeSet::new(),
            work: 0,
            trusted_hash,
        }
    }

    fn visit(&mut self, name: &str, depth: usize) -> bool {
        if depth > MAX_TEMPLATE_CLOSURE_DEPTH || self.active.contains(name) {
            return false;
        }
        if self.visited.contains(name) {
            return true;
        }
        self.work = self.work.saturating_add(1);
        if self.work > MAX_TEMPLATE_CLOSURE_WORK {
            return false;
        }
        let Some(matches) = self.definitions.get(name) else {
            return false;
        };
        if matches.len() != 1 {
            return false;
        }
        let definition = &matches[0];
        if (self.trusted_hash)(name).as_deref() != Some(definition.sha256.as_str()) {
            return false;
        }
        // Copy the bounded body lines before recursive visits so mutable
        // traversal state does not overlap a borrow of the definition map.
        let body_lines = definition.body_lines.clone();

        self.active.insert(name.to_owned());
        for line in &body_lines {
            self.work = self.work.saturating_add(1);
            if self.work > MAX_TEMPLATE_CLOSURE_WORK {
                self.active.remove(name);
                return false;
            }
            let Some((reference, _)) =
                first_genmf_template_reference(line, Some(&self.template_names))
            else {
                continue;
            };
            if !self.visit(reference, depth + 1) {
                self.active.remove(name);
                return false;
            }
        }
        self.active.remove(name);
        self.visited.insert(name.to_owned());
        true
    }
}

fn first_genmf_template_reference<'a>(
    line: &'a str,
    template_names: Option<&BTreeSet<String>>,
) -> Option<(&'a str, usize)> {
    if line.is_empty() || line.as_bytes()[0] == b'#' {
        return None;
    }
    let bytes = line.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != b'%' || !bytes.get(index + 1).is_some_and(u8::is_ascii_alphanumeric) {
            index += 1;
            continue;
        }
        let start = index + 1;
        let mut end = start + 1;
        while bytes
            .get(end)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        {
            end += 1;
        }
        if line
            .get(end..)?
            .chars()
            .next()
            .is_some_and(|character| !character.is_whitespace())
        {
            index = end;
            continue;
        }
        if index > 0 && bytes[index - 1] == b'#' {
            return None;
        }
        let name = line.get(start..end)?;
        return template_names
            .is_none_or(|names| names.contains(name))
            .then_some((name, index));
    }
    None
}

pub(super) fn has_inline_genmf_template_reference(
    line: &str,
    template_names: Option<&BTreeSet<String>>,
) -> bool {
    let Some((_, start)) = first_genmf_template_reference(line, template_names) else {
        return false;
    };
    let first_non_whitespace = line.len().saturating_sub(line.trim_start().len());
    start > first_non_whitespace
}

pub(super) fn native_template_names(root: &Path) -> Option<BTreeSet<String>> {
    if let Some((_, definitions)) = read_native_template_set(root) {
        return Some(definitions.into_keys().collect());
    }

    #[cfg(test)]
    if fs::symlink_metadata(root.join("config/make.tmpl")).is_err() {
        return Some(
            [
                "build_prog",
                "rule_assemble_multi",
                "rule_compile",
                "rule_compile_multi",
                "rule_link_binary",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        );
    }
    None
}

fn trusted_template_hash(name: &str) -> Option<&'static str> {
    Some(match name {
        "add_compilerlinkflags" => ADD_COMPILERLINKFLAGS_SHA256,
        "assemble_q" => ASSEMBLE_Q_SHA256,
        "build_prog" => BUILD_PROG_SHA256,
        "compile_q" => COMPILE_Q_SHA256,
        "fileactionmsg" => FILEACTIONMSG_SHA256,
        "gen_archspecificrules" => GEN_ARCHSPECIFICRULES_SHA256,
        "include_deps" => INCLUDE_DEPS_SHA256,
        "link_q" => LINK_Q_SHA256,
        "mkdepend_q" => MKDEPEND_Q_SHA256,
        "mkdir_q" => MKDIR_Q_SHA256,
        "rule_assemble_multi" => ASSEMBLE_MULTI_SHA256,
        "rule_compile_cxx_multi" => RULE_COMPILE_CXX_MULTI_SHA256,
        "rule_compile_multi" => COMPILE_MULTI_SHA256,
        "rule_compile_objc_multi" => RULE_COMPILE_OBJC_MULTI_SHA256,
        "rule_link_binary" => LINK_BINARY_SHA256,
        "rule_link_prog" => RULE_LINK_PROG_SHA256,
        "rule_makedirs" => RULE_MAKEDIRS_SHA256,
        "strip_q" => STRIP_Q_SHA256,
        _ => return None,
    })
}

fn verified_macro_contract(
    template: &str,
    name: &str,
    expected_sha256: &str,
) -> Option<MacroContract> {
    let lines = template.lines().collect::<Vec<_>>();
    if lines.len() > MAX_LINES {
        return None;
    }
    let mut matches = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| native_define_name(line) == Some(name));
    let (start, _) = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    let end = lines[start + 1..]
        .iter()
        .position(|line| line.starts_with("%end"))?
        + start
        + 1;
    let body = lines[start..=end].join("\n");
    if aros_common::sha256_bytes(body.as_bytes()).as_str() != expected_sha256 {
        return None;
    }
    parse_macro_header(&lines, start, name)
}

fn parse_macro_header(lines: &[&str], start: usize, expected_name: &str) -> Option<MacroContract> {
    let mut header = String::new();
    let mut cursor = start;
    loop {
        let line = lines.get(cursor)?.trim_end();
        let continued = line.ends_with('\\');
        let segment = if continued {
            line.strip_suffix('\\')?
        } else {
            line
        };
        if !header.is_empty() {
            header.push(' ');
        }
        header.push_str(segment.trim());
        if !continued {
            break;
        }
        cursor = cursor.checked_add(1)?;
    }

    let rest = header.strip_prefix("%define")?;
    if !rest.chars().next().is_some_and(char::is_whitespace) {
        return None;
    }
    let rest = rest.trim_start();
    let (name, arguments) = rest.split_once(char::is_whitespace)?;
    if name != expected_name || arguments.len() > MAX_MACRO_ARGUMENT_BYTES {
        return None;
    }
    parse_macro_argument_specs(arguments.trim())
}

fn parse_macro_argument_specs(raw: &str) -> Option<MacroContract> {
    let bytes = raw.as_bytes();
    let mut cursor = 0usize;
    let mut specs = MacroContract::new();
    while cursor < bytes.len() {
        while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        if cursor == bytes.len() {
            break;
        }
        let key_start = cursor;
        if !bytes.get(cursor).is_some_and(u8::is_ascii_alphanumeric) {
            return None;
        }
        cursor += 1;
        while bytes
            .get(cursor)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        {
            cursor += 1;
        }
        if cursor - key_start > 64 || bytes.get(cursor) != Some(&b'=') {
            return None;
        }
        let key = std::str::from_utf8(&bytes[key_start..cursor])
            .ok()?
            .to_owned();
        cursor += 1;

        let (mut default, end) = if bytes.get(cursor) == Some(&b'"') {
            let (value, end) = genmf_argument_value(raw, cursor)?;
            (Some(value), end)
        } else {
            let start = cursor;
            while cursor < bytes.len() && !bytes[cursor].is_ascii_whitespace() {
                if bytes[cursor] == b'"' {
                    return None;
                }
                cursor += 1;
            }
            let value = if cursor > start {
                Some(raw.get(start..cursor)?.to_owned())
            } else {
                None
            };
            (value, cursor)
        };
        cursor = end;

        let mut required = false;
        if let Some(value) = default.as_mut() {
            if value.ends_with("/A") {
                value.truncate(value.len() - 2);
                required = true;
            } else if value.ends_with("/M") {
                // These four source graph macros have no GenMF multiarg field;
                // do not approximate its remainder-capture semantics.
                return None;
            }
        }
        if specs
            .insert(key, MacroArgumentSpec { default, required })
            .is_some()
            || specs.len() > MAX_MACRO_ARGUMENTS
        {
            return None;
        }
    }
    Some(specs)
}

pub(super) fn effective_macro_arguments(
    raw: &str,
    contract: &MacroContract,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
    line: usize,
) -> Option<ClosedMacroArguments> {
    let supplied = closed_macro_arguments(raw)?;
    if supplied.keys().any(|key| !contract.contains_key(key))
        || contract
            .iter()
            .any(|(key, spec)| spec.required && !supplied.contains_key(key))
    {
        return None;
    }

    let effective = contract
        .iter()
        .map(|(key, spec)| {
            (
                key.clone(),
                supplied
                    .get(key)
                    .cloned()
                    .or_else(|| spec.default.clone())
                    .unwrap_or_default(),
            )
        })
        .collect::<ClosedMacroArguments>();
    let context = MakeExprContext::new(scope, dirs, line, root, rel_dir);
    for value in effective.values() {
        evaluate_make_expr(value, &context).ok()?;
    }
    Some(effective)
}

const fn macro_contract_identity(index: usize) -> Option<(&'static str, &'static str)> {
    match index {
        0 => Some(("build_prog", BUILD_PROG_SHA256)),
        1 => Some(("rule_compile_multi", COMPILE_MULTI_SHA256)),
        2 => Some(("rule_assemble_multi", ASSEMBLE_MULTI_SHA256)),
        3 => Some(("rule_link_binary", LINK_BINARY_SHA256)),
        _ => None,
    }
}

pub(super) fn verified_macro_contracts(
    root: &Path,
    macros: VerifiedMacros,
) -> [Option<MacroContract>; 4] {
    if let Some(template) = read_native_make_template(root) {
        return std::array::from_fn(|index| {
            if !macros.0[index] {
                return None;
            }
            let (name, hash) = macro_contract_identity(index)?;
            verified_macro_contract(&template, name, hash)
        });
    }

    #[cfg(test)]
    if fs::symlink_metadata(root.join("config/make.tmpl")).is_err() {
        return std::array::from_fn(|index| {
            if !macros.0[index] {
                return None;
            }
            test_macro_contract(index)
        });
    }

    std::array::from_fn(|_| None)
}

#[cfg(test)]
fn test_macro_header(index: usize) -> Option<&'static str> {
    const HEADERS: [&str; 4] = [
        "%define build_prog mmake=/A progname=/A \\\n            files= objcfiles= cxxfiles= alwayscxxlink=no \\\n            asmfiles= objs= objdir=\"$(GENDIR)/$(CURDIR)\" targetdir=\"$(AROSDIR)/$(CURDIR)\" \\\n            cppflags=\"$(CPPFLAGS)\" cflags= dflags= cxxflags= dxxflags= ldflags= \\\n            aflags=\"$(AFLAGS)\" uselibs= usehostlibs= usestartup=yes detach=no nix=no \\\n            includedir= libdir= usetree=no \\\n            compiler=target linker= \\\n            coverageinstr=\"$(TARGET_COVERAGEINSTR)\" funcinstr=\"$(TARGET_FUNCINSTR)\" lto=\"$(TARGET_LTO)\"",
        "%define rule_compile_multi mmake=TMP basenames=/A cppflags=$(CPPFLAGS) cflags=$(CFLAGS) dflags= srcdir= targetdir= \\\n            compiler=target usetree=no incextra=\"$(TOP)/$(CURDIR)\"",
        "%define rule_assemble_multi mmake=TMP cmd=\"$(strip $(CC) $(TARGET_SYSROOT))\"  basenames=/A cppflags=$(CPPFLAGS) aflags=$(AFLAGS) targetdir= suffix=.s",
        "%define rule_link_binary mmake=BD file=/A name=/A objs= files= asmfiles= start=0 ldflags=",
    ];
    HEADERS.get(index).copied()
}

#[cfg(test)]
pub(super) fn test_macro_contract(index: usize) -> Option<MacroContract> {
    // The synthetic headers mirror the hash-bound config/make.tmpl headers.
    let header = test_macro_header(index)?;
    let lines = header.lines().collect::<Vec<_>>();
    parse_macro_header(&lines, 0, macro_contract_identity(index)?.0)
}
