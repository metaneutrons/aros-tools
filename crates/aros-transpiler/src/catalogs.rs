//! `%build_catalogs`: translated Locale catalogs and optional source headers.
//!
//! The legacy macro supplies useful defaults for every argument except its
//! identity and install name.  Keep those defaults here rather than in the
//! CMake layer: the transpiler can resolve local Make variables at the exact
//! declaration line and can enumerate the source tree's `.ct` / `.cd` files
//! without inventing configure-time outputs.

use crate::dirs::DirVars;
use crate::parser::{ConditionalTruth, VarScope};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One fully resolved `%build_catalogs` declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogDecl {
    /// `mmake=`: the outer build target.
    pub mmake: String,
    /// `name=`: installed catalog basename, without `.catalog`.
    pub name: String,
    /// `subdir=` below each language directory.
    pub subdir: String,
    /// Language names, without `.ct`.
    pub catalogs: Vec<String>,
    /// Optional generated source/header path. Relative paths retain the
    /// legacy spelling and are rooted in the declaration's generated tree by
    /// the CMake helper.
    pub source: Option<String>,
    /// Catalog description basename/path, without a required `.cd` suffix.
    pub description: String,
    /// Base destination directory for installed catalogs.
    pub dir: String,
    /// FlexCat source-description basename/path, without a required `.sd`.
    pub source_description: String,
    /// Directory containing the `.cd` and `.ct` inputs.
    pub srcdir: String,
    /// Directory containing the declaring mmakefile, relative to the source
    /// root. This is distinct from `srcdir=`, which may be overridden.
    pub declaring_dir: String,
    /// One-based line in continuation-joined input, for diagnostics.
    pub line: usize,
}

/// Complete catalog scan of one mmakefile.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CatalogScan {
    /// Declarations whose complete output graph can be emitted.
    pub declarations: Vec<CatalogDecl>,
    /// Declarations which remain intentionally absent from generated CMake.
    /// Keeping them absent means coverage cannot claim an unresolved stub.
    pub skipped: Vec<String>,
}

#[derive(Debug)]
struct Invocation {
    args: String,
    /// Zero-based line in continuation-joined input.
    line: usize,
}

/// Collects every representable `%build_catalogs` declaration.
///
/// `joined` must have Make line continuations joined in the same way as the
/// [`VarScope`], so assignment/declaration line numbers stay comparable.
#[must_use]
pub fn collect_catalogs(
    joined: &str,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
) -> CatalogScan {
    collect_catalogs_with_line_states(joined, scope, dirs, root, rel_dir, None)
}

/// Profile-aware catalog collection used by the main parser.
///
/// A declaration in a false Make branch does not exist for that profile. An
/// unresolved branch is deliberately reported and omitted, matching concrete
/// compiled targets instead of manufacturing coverage from both branches.
#[must_use]
pub(crate) fn collect_catalogs_with_line_states(
    joined: &str,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
    line_states: Option<&[ConditionalTruth]>,
) -> CatalogScan {
    let mut scan = CatalogScan::default();

    for invocation in invocations(joined) {
        let line_display = invocation.line + 1;
        let context = |detail: &str| {
            format!(
                "{}:{line_display}: %build_catalogs {detail}",
                slash_path(rel_dir)
            )
        };

        if let Some(states) = line_states {
            match states
                .get(invocation.line)
                .copied()
                .unwrap_or(ConditionalTruth::Unknown)
            {
                ConditionalTruth::True => {}
                ConditionalTruth::False => continue,
                ConditionalTruth::Unknown => {
                    let mmake = arg(&invocation.args, "mmake")
                        .map_or_else(String::new, |value| format!("mmake={value}: "));
                    scan.skipped.push(context(&format!(
                        "{mmake}is guarded by an unresolved Make conditional"
                    )));
                    continue;
                }
            }
        }

        let mmake = match required_arg(
            &invocation.args,
            "mmake",
            scope,
            dirs,
            rel_dir,
            invocation.line,
        ) {
            Ok(value) => value,
            Err(reason) => {
                scan.skipped.push(context(&reason));
                continue;
            }
        };

        let name = match required_arg(
            &invocation.args,
            "name",
            scope,
            dirs,
            rel_dir,
            invocation.line,
        ) {
            Ok(value) => value,
            Err(reason) => {
                scan.skipped
                    .push(context(&format!("mmake={mmake}: {reason}")));
                continue;
            }
        };
        let subdir = match required_arg(
            &invocation.args,
            "subdir",
            scope,
            dirs,
            rel_dir,
            invocation.line,
        ) {
            Ok(value) => value,
            Err(reason) => {
                scan.skipped
                    .push(context(&format!("mmake={mmake}: {reason}")));
                continue;
            }
        };

        if !safe_catalog_leaf(&name) {
            scan.skipped.push(context(&format!(
                "mmake={mmake}: name={name} is not a catalog basename"
            )));
            continue;
        }
        if !safe_catalog_subdir(&subdir) {
            scan.skipped.push(context(&format!(
                "mmake={mmake}: subdir={subdir} is not a contained relative directory"
            )));
            continue;
        }

        let expand = |key: &str, default: &str| -> Result<String, String> {
            let raw = arg(&invocation.args, key).unwrap_or_else(|| default.to_owned());
            expand_at(&raw, scope, dirs, rel_dir, invocation.line)
                .map_err(|reason| format!("mmake={mmake} {key}={raw}: {reason}"))
        };

        let srcdir = match expand("srcdir", "$(SRCDIR)/$(CURDIR)") {
            Ok(value) if !value.trim().is_empty() => trim_quotes(&value),
            Ok(_) => {
                scan.skipped
                    .push(context(&format!("mmake={mmake}: srcdir resolved empty")));
                continue;
            }
            Err(reason) => {
                scan.skipped.push(context(&reason));
                continue;
            }
        };
        let source_input_dir = rendered_source_path(&srcdir, root, rel_dir);

        let raw_catalogs = arg(&invocation.args, "catalogs").unwrap_or_default();
        let catalogs = if raw_catalogs.trim().is_empty() {
            match scan_suffix(&source_input_dir, "ct") {
                Ok(values) if !values.is_empty() => values,
                Ok(_) => {
                    scan.skipped.push(context(&format!(
                        "mmake={mmake}: default catalogs matched no .ct files in {srcdir}"
                    )));
                    continue;
                }
                Err(reason) => {
                    scan.skipped.push(context(&format!(
                        "mmake={mmake}: cannot evaluate default catalogs in {srcdir}: {reason}"
                    )));
                    continue;
                }
            }
        } else {
            match expand_at(&raw_catalogs, scope, dirs, rel_dir, invocation.line) {
                Ok(value) => make_words(&value)
                    .into_iter()
                    .map(|language| language.strip_suffix(".ct").unwrap_or(&language).to_owned())
                    .collect(),
                Err(reason) => {
                    scan.skipped.push(context(&format!(
                        "mmake={mmake} catalogs={raw_catalogs}: {reason}"
                    )));
                    continue;
                }
            }
        };
        if catalogs.is_empty()
            || catalogs.iter().any(|language| {
                language.is_empty()
                    || language.contains('/')
                    || language.contains('\\')
                    || language.contains(';')
                    || matches!(language.as_str(), "." | "..")
            })
        {
            scan.skipped.push(context(&format!(
                "mmake={mmake}: catalogs resolved to an empty or invalid language list"
            )));
            continue;
        }

        let raw_description = arg(&invocation.args, "description").unwrap_or_default();
        let description = if raw_description.trim().is_empty() {
            match scan_suffix(&source_input_dir, "cd") {
                Ok(mut values) if values.len() == 1 => values.pop().unwrap_or_default(),
                Ok(values) => {
                    scan.skipped.push(context(&format!(
                        "mmake={mmake}: default description expected one .cd in {srcdir}, found {}",
                        values.len()
                    )));
                    continue;
                }
                Err(reason) => {
                    scan.skipped.push(context(&format!(
                        "mmake={mmake}: cannot evaluate default description in {srcdir}: {reason}"
                    )));
                    continue;
                }
            }
        } else {
            match expand_at(&raw_description, scope, dirs, rel_dir, invocation.line) {
                Ok(value) if !value.trim().is_empty() => trim_suffix(&trim_quotes(&value), ".cd"),
                Ok(_) => {
                    scan.skipped.push(context(&format!(
                        "mmake={mmake}: description resolved empty"
                    )));
                    continue;
                }
                Err(reason) => {
                    scan.skipped.push(context(&format!(
                        "mmake={mmake} description={raw_description}: {reason}"
                    )));
                    continue;
                }
            }
        };

        let dir = match expand("dir", "$(AROS_CATALOGS)") {
            Ok(value) if !value.trim().is_empty() => trim_quotes(&value),
            Ok(_) => {
                scan.skipped
                    .push(context(&format!("mmake={mmake}: dir resolved empty")));
                continue;
            }
            Err(reason) => {
                scan.skipped.push(context(&reason));
                continue;
            }
        };

        let raw_source =
            arg(&invocation.args, "source").unwrap_or_else(|| "../strings.h".to_owned());
        let source = if raw_source.trim().is_empty() {
            None
        } else {
            match expand_at(&raw_source, scope, dirs, rel_dir, invocation.line) {
                Ok(value) if !value.trim().is_empty() => {
                    let value = trim_quotes(&value);
                    if !looks_absolute_or_deferred(&value)
                        && !relative_source_is_contained(rel_dir, &value)
                    {
                        scan.skipped.push(context(&format!(
                            "mmake={mmake}: relative source={value} escapes the generated tree"
                        )));
                        continue;
                    }
                    Some(value)
                }
                Ok(_) => None,
                Err(reason) => {
                    scan.skipped.push(context(&format!(
                        "mmake={mmake} source={raw_source}: {reason}"
                    )));
                    continue;
                }
            }
        };

        let source_description = match expand("sourcedescription", "$(TOOLDIR)/C_h_aros") {
            Ok(value) if !value.trim().is_empty() => trim_suffix(&trim_quotes(&value), ".sd"),
            Ok(_) => {
                scan.skipped.push(context(&format!(
                    "mmake={mmake}: sourcedescription resolved empty"
                )));
                continue;
            }
            Err(reason) => {
                scan.skipped.push(context(&reason));
                continue;
            }
        };

        scan.declarations.push(CatalogDecl {
            mmake,
            name,
            subdir: subdir.trim_matches('/').to_owned(),
            catalogs,
            source,
            description,
            dir: dir.trim_end_matches('/').to_owned(),
            source_description,
            srcdir: srcdir.trim_end_matches('/').to_owned(),
            declaring_dir: slash_path(rel_dir),
            line: line_display,
        });
    }

    scan
}

fn required_arg(
    args: &str,
    key: &str,
    scope: &VarScope,
    dirs: &DirVars,
    rel_dir: &Path,
    line: usize,
) -> Result<String, String> {
    let raw = arg(args, key).ok_or_else(|| format!("without required {key}="))?;
    let value = expand_at(&raw, scope, dirs, rel_dir, line)
        .map_err(|reason| format!("{key}={raw}: {reason}"))?;
    let value = trim_quotes(&value);
    if value.trim().is_empty() {
        Err(format!("{key}={raw} resolved empty"))
    } else if value.contains(';') {
        Err(format!("{key}={raw} resolved to an invalid semicolon list"))
    } else {
        Ok(value)
    }
}

fn safe_catalog_leaf(value: &str) -> bool {
    !matches!(value, "." | "..") && !value.contains(['/', '\\'])
}

fn safe_catalog_subdir(value: &str) -> bool {
    let value = value.replace('\\', "/");
    !value.starts_with('/')
        && value
            .split('/')
            .all(|component| !matches!(component, "." | ".."))
}

fn looks_absolute_or_deferred(value: &str) -> bool {
    Path::new(value).is_absolute()
        || value.starts_with("${")
        || (value.as_bytes().get(1) == Some(&b':')
            && value
                .as_bytes()
                .get(2)
                .is_some_and(|separator| matches!(separator, b'/' | b'\\')))
}

fn relative_source_is_contained(declaring_dir: &Path, source: &str) -> bool {
    let mut depth = 0usize;
    let declaring_dir = slash_path(declaring_dir);
    if !apply_relative_components(&declaring_dir, &mut depth) {
        return false;
    }
    apply_relative_components(&source.replace('\\', "/"), &mut depth)
}

fn apply_relative_components(value: &str, depth: &mut usize) -> bool {
    for component in value.split('/') {
        match component {
            "" | "." => {}
            ".." => match depth.checked_sub(1) {
                Some(parent_depth) => *depth = parent_depth,
                None => return false,
            },
            _ => *depth += 1,
        }
    }
    true
}

fn expand_at(
    raw: &str,
    scope: &VarScope,
    dirs: &DirVars,
    rel_dir: &Path,
    line: usize,
) -> Result<String, String> {
    let local = |name: &str| {
        (name == "CURDIR")
            .then(|| slash_path(rel_dir))
            .or_else(|| scope.raw_at(name, line))
    };
    dirs.expand_with(raw, &local)
        .map_err(|missing| format!("unresolved Make variable(s): {}", missing.join(", ")))
}

fn invocations(joined: &str) -> Vec<Invocation> {
    joined
        .lines()
        .enumerate()
        .filter_map(|(line, raw)| {
            let trimmed = raw.trim_start();
            let args = trimmed.strip_prefix("%build_catalogs")?;
            if !args.is_empty() && !args.starts_with(char::is_whitespace) {
                return None;
            }
            Some(Invocation {
                args: args.trim_start().to_owned(),
                line,
            })
        })
        .collect()
}

/// Reads `key=value`, including a deliberately empty quoted value.
fn arg(args: &str, key: &str) -> Option<String> {
    let mut from = 0usize;
    loop {
        let hit = args[from..].find(key)? + from;
        let before_ok = hit == 0
            || args[..hit]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace);
        let rest = &args[hit + key.len()..];
        if before_ok {
            if let Some(value) = rest.strip_prefix("=\"") {
                let end = value.find('"')?;
                return Some(value[..end].to_owned());
            }
            if let Some(value) = rest.strip_prefix('=') {
                let end = value.find(char::is_whitespace).unwrap_or(value.len());
                return Some(value[..end].trim().to_owned());
            }
        }
        from = hit + 1;
    }
}

fn rendered_source_path(rendered: &str, root: &Path, rel_dir: &Path) -> PathBuf {
    const SOURCE_ROOT: &str = "${CMAKE_SOURCE_DIR}";
    if rendered == SOURCE_ROOT {
        return root.to_path_buf();
    }
    if let Some(relative) = rendered.strip_prefix("${CMAKE_SOURCE_DIR}/") {
        return root.join(relative);
    }
    let path = Path::new(rendered);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(rel_dir).join(path)
    }
}

fn scan_suffix(directory: &Path, suffix: &str) -> Result<Vec<String>, String> {
    let entries = std::fs::read_dir(directory)
        .map_err(|error| format!("cannot read {}: {error}", directory.display()))?;
    let mut values = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            (path.extension().and_then(|ext| ext.to_str()) == Some(suffix))
                .then(|| path.file_stem()?.to_str().map(str::to_owned))
                .flatten()
        })
        .collect::<Vec<_>>();
    values.sort();
    values.dedup();
    Ok(values)
}

fn make_words(value: &str) -> Vec<String> {
    value
        .split_whitespace()
        .map(|word| word.trim_matches(['"', '\\']))
        .filter(|word| !word.is_empty() && *word != "\\")
        .map(str::to_owned)
        .collect()
}

fn trim_quotes(value: &str) -> String {
    value.trim().trim_matches('"').to_owned()
}

fn trim_suffix(value: &str, suffix: &str) -> String {
    value.strip_suffix(suffix).unwrap_or(value).to_owned()
}

fn slash_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::collect_catalogs;
    use crate::dirs::DirVars;
    use crate::parser::collect_vars;
    use std::fs;
    use std::path::{Path, PathBuf};

    fn repo_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..")
    }

    #[test]
    fn production_dos_catalogs_resolve_all_legacy_defaults() {
        let root = repo_root();
        let file = root.join("rom/dos/catalogs/mmakefile.src");
        let source = fs::read_to_string(&file).unwrap();
        let joined = source.replace("\\\n", "");
        let scope = collect_vars(&joined);
        let dirs = DirVars::load(&root);

        let scan = collect_catalogs(&joined, &scope, &dirs, &root, Path::new("rom/dos/catalogs"));

        assert!(scan.skipped.is_empty(), "{:#?}", scan.skipped);
        assert_eq!(scan.declarations.len(), 1);
        let catalog = &scan.declarations[0];
        assert_eq!(catalog.mmake, "workbench-libs-dos-catalogs");
        assert_eq!(catalog.name, "dos");
        assert_eq!(catalog.subdir, "System/Libs");
        assert_eq!(catalog.description, "dos");
        assert_eq!(catalog.source.as_deref(), Some("../strings.h"));
        assert_eq!(catalog.catalogs.len(), 19);
        assert!(catalog.catalogs.contains(&"polish".to_owned()));
        assert!(catalog.catalogs.contains(&"russian".to_owned()));
        assert_eq!(catalog.dir, "${AROS_BUILD_DIR}/SYS/Locale/Catalogs");
        assert_eq!(catalog.srcdir, "${CMAKE_SOURCE_DIR}/rom/dos/catalogs");
        assert_eq!(
            catalog.source_description,
            "${AROS_BUILD_DIR}/hosttools/C_h_aros"
        );
    }

    #[test]
    fn empty_catalog_and_description_arguments_use_source_wildcards() {
        let root = repo_root();
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("messages.cd"), "").unwrap();
        fs::write(tmp.path().join("zulu.ct"), "").unwrap();
        fs::write(tmp.path().join("alpha.ct"), "").unwrap();
        let text = format!(
            concat!(
                "%build_catalogs mmake=sample name=Sample subdir=Tools \\\n",
                "    catalogs=\"\" description=\"\" source=\"\" srcdir=\"{}\" \\\n",
                "    dir=\"$(AROS_CATALOGS)\" sourcedescription=custom\n"
            ),
            tmp.path().display()
        );
        let joined = text.replace("\\\n", "");
        let scope = collect_vars(&joined);
        let dirs = DirVars::load(&root);
        let scan = collect_catalogs(&joined, &scope, &dirs, &root, Path::new("synthetic"));

        assert!(scan.skipped.is_empty(), "{:#?}", scan.skipped);
        let catalog = &scan.declarations[0];
        assert_eq!(catalog.catalogs, ["alpha", "zulu"]);
        assert_eq!(catalog.description, "messages");
        assert_eq!(catalog.source, None);
        assert_eq!(catalog.source_description, "custom");
    }

    #[test]
    fn local_variables_expand_at_the_declaration_line() {
        let root = repo_root();
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("first.cd"), "").unwrap();
        fs::write(tmp.path().join("german.ct"), "").unwrap();
        let text = format!(
            concat!(
                "LANGS := german\n",
                "BASE := {}\n",
                "%build_catalogs mmake=first name=First subdir=Tools catalogs=$(LANGS) \\\n",
                "    description=first source=generated.h srcdir=$(BASE) \\\n",
                "    dir=$(AROS_CATALOGS) sourcedescription=$(TOOLDIR)/C_h_aros\n",
                "LANGS := french\n"
            ),
            tmp.path().display()
        );
        let joined = text.replace("\\\n", "");
        let scope = collect_vars(&joined);
        let dirs = DirVars::load(&root);
        let scan = collect_catalogs(&joined, &scope, &dirs, &root, Path::new("synthetic"));

        assert!(scan.skipped.is_empty(), "{:#?}", scan.skipped);
        assert_eq!(scan.declarations[0].catalogs, ["german"]);
    }

    #[test]
    fn unresolved_or_ambiguous_defaults_remain_unmodelled() {
        let root = repo_root();
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("one.cd"), "").unwrap();
        fs::write(tmp.path().join("two.cd"), "").unwrap();
        fs::write(tmp.path().join("german.ct"), "").unwrap();
        let text = format!(
            "%build_catalogs mmake=sample name=Sample subdir=Tools catalogs=german srcdir={}\n",
            tmp.path().display()
        );
        let scope = collect_vars(&text);
        let dirs = DirVars::load(&root);
        let scan = collect_catalogs(&text, &scope, &dirs, &root, Path::new("synthetic"));

        assert!(scan.declarations.is_empty());
        assert_eq!(scan.skipped.len(), 1);
        assert!(scan.skipped[0].contains("expected one .cd"));
    }

    #[test]
    fn output_path_traversal_is_reported_instead_of_emitted() {
        let root = repo_root();
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("messages.cd"), "").unwrap();
        fs::write(tmp.path().join("german.ct"), "").unwrap();
        let text = format!(
            concat!(
                "%build_catalogs mmake=bad-name name=../escape subdir=Tools ",
                "catalogs=german description=messages source= srcdir={}\n",
                "%build_catalogs mmake=bad-subdir name=Sample subdir=../../escape ",
                "catalogs=german description=messages source= srcdir={}\n",
                "%build_catalogs mmake=bad-language name=Sample subdir=Tools ",
                "catalogs=.. description=messages source= srcdir={}\n",
                "%build_catalogs mmake=bad-source name=Sample subdir=Tools ",
                "catalogs=german description=messages source=../../escape.h srcdir={}\n"
            ),
            tmp.path().display(),
            tmp.path().display(),
            tmp.path().display(),
            tmp.path().display()
        );
        let scope = collect_vars(&text);
        let dirs = DirVars::load(&root);
        let scan = collect_catalogs(&text, &scope, &dirs, &root, Path::new("synthetic"));

        assert!(scan.declarations.is_empty());
        assert_eq!(scan.skipped.len(), 4, "{:#?}", scan.skipped);
        assert!(scan.skipped[0].contains("not a catalog basename"));
        assert!(scan.skipped[1].contains("contained relative directory"));
        assert!(scan.skipped[2].contains("invalid language list"));
        assert!(scan.skipped[3].contains("escapes the generated tree"));
    }
}
