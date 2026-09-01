//! Stable, no-follow `.gitmodules` snapshot parsing.

use super::{capture_git_stdout, git_at, path_argument, validate_url, TransportKind};
use miette::{bail, IntoDiagnostic, Result, WrapErr};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

const MAX_GITMODULES_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DirectSubmodule {
    pub(super) name: String,
    pub(super) path: PathBuf,
    pub(super) url: String,
}

pub(super) fn direct_submodules(repo_root: &Path) -> Result<Vec<DirectSubmodule>> {
    let modules = repo_root.join(".gitmodules");
    let Some((_, snapshot)) = aros_common::measure_regular_file(&modules)
        .into_diagnostic()
        .wrap_err_with(|| {
            format!(
                "could not read '.gitmodules' as a stable no-follow regular-file snapshot at '{}'",
                modules.display()
            )
        })?
    else {
        return Ok(Vec::new());
    };
    if snapshot.len() > MAX_GITMODULES_BYTES {
        bail!(
            "'.gitmodules' at '{}' exceeds the {}-byte source-contract limit",
            modules.display(),
            MAX_GITMODULES_BYTES
        );
    }

    // Git remains the syntax oracle, but it sees only these already measured
    // bytes. Includes are disabled at the parser and rejected as declarations.
    let snapshot_directory = tempfile::Builder::new()
        .prefix(".aros-gitmodules-")
        .tempdir_in(repo_root)
        .into_diagnostic()
        .wrap_err("could not create isolated .gitmodules snapshot")?;
    let snapshot_path = snapshot_directory.path().join("config");
    fs::write(&snapshot_path, snapshot)
        .into_diagnostic()
        .wrap_err("could not materialize isolated .gitmodules snapshot")?;
    let mut command = git_at(repo_root, TransportKind::Local);
    command.args([
        "config",
        "--no-includes",
        "--file",
        path_argument(&snapshot_path)?,
        "--null",
        "--list",
    ]);
    let inventory = capture_git_stdout(&mut command, "isolated submodule configuration snapshot")?;
    parse_snapshot(&modules, &inventory)
}

fn parse_snapshot(modules: &Path, inventory: &str) -> Result<Vec<DirectSubmodule>> {
    let mut declarations = BTreeMap::<String, (Option<PathBuf>, Option<String>)>::new();
    for record in inventory.split_terminator('\0') {
        let (key, value) = record
            .split_once('\n')
            .ok_or_else(|| miette::miette!("malformed entry in '{}'", modules.display()))?;
        let key_lower = key.to_ascii_lowercase();
        if key_lower == "include.path"
            || (key_lower.starts_with("includeif.") && key_lower.as_bytes().ends_with(b".path"))
        {
            bail!(
                "'.gitmodules' at '{}' must not contain include or includeIf directives",
                modules.display()
            );
        }
        let Some(rest) = key.strip_prefix("submodule.") else {
            continue;
        };
        let (name, field) = if let Some(name) = rest.strip_suffix(".path") {
            (name, "path")
        } else if let Some(name) = rest.strip_suffix(".url") {
            (name, "url")
        } else {
            continue;
        };
        if name.is_empty() || name.contains('=') || name.chars().any(char::is_control) {
            bail!("submodule name '{name}' is unsafe for isolated Git configuration");
        }
        let declaration = declarations.entry(name.to_owned()).or_default();
        match field {
            "path" if declaration.0.is_none() => declaration.0 = Some(PathBuf::from(value)),
            "url" if declaration.1.is_none() => declaration.1 = Some(value.to_owned()),
            _ => bail!("submodule '{name}' declares duplicate {field} values"),
        }
    }

    let mut entries = Vec::with_capacity(declarations.len());
    let mut paths = BTreeSet::new();
    for (name, (path, url)) in declarations {
        let path = path.ok_or_else(|| miette::miette!("submodule '{name}' has no path"))?;
        let url = url.ok_or_else(|| miette::miette!("submodule '{name}' has no URL"))?;
        let value = path
            .to_str()
            .ok_or_else(|| miette::miette!("submodule '{name}' path is not valid UTF-8"))?;
        if value.trim() != value || value.chars().any(char::is_control) {
            bail!("submodule '{name}' has an unsafe path value");
        }
        if path.as_os_str().is_empty()
            || path.is_absolute()
            || path
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
            || !paths.insert(path.clone())
        {
            bail!(
                "submodule '{name}' has an unsafe or duplicate path '{}'",
                path.display()
            );
        }
        validate_url(&url, "submodule")?;
        entries.push(DirectSubmodule { name, path, url });
    }
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(entries)
}
